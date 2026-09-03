use std::path::PathBuf;

use deskkin_desktop_host::profile::{
    BindMode, PhysicalProfile, ProfileAvailability, ProfileRecording, ProfileStore,
};
use deskkin_desktop_host::{
    IdentityActor, IdentityStore, OwnerCommand, OwnerResponse, acquire_owner_lock, bind_loopback,
    call_owner_control, discover_owner, new_control_id, query_command_result,
    run_host_runtime_with_recording, run_owner_control,
    run_private_lan_host_runtime_with_recording, serve_one,
};
use deskkin_protocol::AvailabilityResult;
use local_run_recorder::RecordingMode;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or_else(usage)?;
    match command.as_str() {
        "profile" => profile_command(args)?,
        "profile-host" | "profile-status" | "profile-stop" => {
            profile_runtime_command(&command, &mut args)?;
        }
        "identity-init" => {
            identity_init(identity_root(args.next()))?;
        }
        "identity-list" => identity_list(identity_root(args.next()))?,
        "unpair" => {
            let peer = args.next().ok_or_else(usage)?;
            unpair(identity_root(args.next()), &peer)?;
        }
        "pairing-window-open" => {
            let root = identity_root(args.next());
            let Some(response) = owner_mutation(&root, |command_id, owner_generation| {
                OwnerCommand::PairingWindowOpen {
                    command_id,
                    owner_generation,
                }
            })?
            else {
                return Err("host runtime owner is not running".into());
            };
            if response != OwnerResponse::Paired {
                return Err(format!("owner pairing: {response:?}"));
            }
            println!("paired");
        }
        "serve-once" => {
            let address = args.next().ok_or_else(usage)?.parse().map_err(debug)?;
            let result = match args.next().as_deref() {
                Some("available") => AvailabilityResult::Available,
                Some("unavailable") => AvailabilityResult::Unavailable,
                Some("read_failed") => AvailabilityResult::ReadFailed,
                _ => return Err(usage()),
            };
            let root = identity_root(args.next());
            let mut owner = standalone_owner_lock(&root)?;
            let store = IdentityStore::new(root);
            let listener = bind_loopback(address).map_err(debug)?;
            owner.ready();
            println!("listening {}", listener.local_addr().map_err(debug)?);
            serve_one(&listener, &store, result).map_err(debug)?;
        }
        "owner" => {
            let role_root =
                PathBuf::from(args.next().unwrap_or_else(|| ".deskkin/phase3/host".into()));
            let actor = IdentityActor::start(IdentityStore::new(role_root.join("identity")));
            let generation = new_control_id().map_err(debug)?;
            run_owner_control(&role_root.join("control"), &actor, &generation).map_err(debug)?;
        }
        "run" => {
            let address = args.next().ok_or_else(usage)?.parse().map_err(debug)?;
            let result = match args.next().as_deref() {
                Some("available") => AvailabilityResult::Available,
                Some("unavailable") => AvailabilityResult::Unavailable,
                Some("read_failed") => AvailabilityResult::ReadFailed,
                _ => return Err(usage()),
            };
            let remaining: Vec<_> = args.collect();
            let recording = if remaining.iter().any(|value| value == "--recording-off") {
                RecordingMode::Off
            } else {
                RecordingMode::On
            };
            let role_root = PathBuf::from(
                remaining
                    .into_iter()
                    .find(|value| value != "--recording-off")
                    .unwrap_or_else(|| ".deskkin/phase3/host".into()),
            );
            run_host_runtime_with_recording(address, &role_root, result, recording)
                .map_err(debug)?;
        }
        "run-private-lan" => {
            let address = args.next().ok_or_else(usage)?.parse().map_err(debug)?;
            let result = parse_availability(args.next().as_deref())?;
            let remaining: Vec<_> = args.collect();
            let recording = if remaining.iter().any(|value| value == "--recording-off") {
                RecordingMode::Off
            } else {
                RecordingMode::On
            };
            let role_root = PathBuf::from(
                remaining
                    .into_iter()
                    .find(|value| value != "--recording-off")
                    .unwrap_or_else(|| ".deskkin/phase3/host".into()),
            );
            run_private_lan_host_runtime_with_recording(address, &role_root, result, recording)
                .map_err(debug)?;
        }
        _ => return Err(usage()),
    }
    Ok(())
}

fn identity_list(root: PathBuf) -> Result<(), String> {
    println!("{:?}", IdentityStore::new(root).peer().map_err(debug)?);
    Ok(())
}

fn profile_runtime_command(
    command: &str,
    args: &mut impl Iterator<Item = String>,
) -> Result<(), String> {
    let name = profile_argument(args)?;
    reject_remaining(args)?;
    let store = ProfileStore::local();
    match command {
        "profile-host" => store.run(&name).map_err(debug),
        "profile-status" => store
            .status(&name)
            .map(|status| println!("{status}"))
            .map_err(debug),
        "profile-stop" => store
            .stop(&name)
            .map(|()| println!("stopped"))
            .map_err(debug),
        _ => Err(usage()),
    }
}

fn profile_command(mut args: impl Iterator<Item = String>) -> Result<(), String> {
    let command = args.next().ok_or_else(usage)?;
    let store = ProfileStore::local();
    match command.as_str() {
        "list" => {
            reject_remaining(&mut args)?;
            for name in store.list().map_err(debug)? {
                println!("{name}");
            }
        }
        "show" => {
            let name = args.next().ok_or_else(usage)?;
            reject_remaining(&mut args)?;
            print!("{}", store.show(&name).map_err(debug)?);
        }
        "delete" => {
            let name = args.next().ok_or_else(usage)?;
            reject_remaining(&mut args)?;
            store.delete(&name).map_err(debug)?;
            println!("deleted");
        }
        "set" => {
            let name = args.next().ok_or_else(usage)?;
            let mut role_root = None;
            let mut bind_mode = None;
            let mut address = None;
            let mut availability = None;
            let mut recording = None;
            while let Some(flag) = args.next() {
                let value = args.next().ok_or_else(usage)?;
                match flag.as_str() {
                    "--role-root" if role_root.is_none() => role_root = Some(value),
                    "--bind-mode" if bind_mode.is_none() => {
                        bind_mode = Some(match value.as_str() {
                            "loopback" => BindMode::Loopback,
                            "private_lan" => BindMode::PrivateLan,
                            _ => return Err(usage()),
                        });
                    }
                    "--address" if address.is_none() => {
                        address = Some(value.parse().map_err(debug)?);
                    }
                    "--availability" if availability.is_none() => {
                        availability = Some(match value.as_str() {
                            "available" => ProfileAvailability::Available,
                            "unavailable" => ProfileAvailability::Unavailable,
                            "read_failed" => ProfileAvailability::ReadFailed,
                            _ => return Err(usage()),
                        });
                    }
                    "--recording" if recording.is_none() => {
                        recording = Some(match value.as_str() {
                            "on" => ProfileRecording::On,
                            "off" => ProfileRecording::Off,
                            _ => return Err(usage()),
                        });
                    }
                    _ => return Err(usage()),
                }
            }
            let profile = PhysicalProfile::new(
                role_root.ok_or_else(usage)?,
                bind_mode.ok_or_else(usage)?,
                address.ok_or_else(usage)?,
                availability.ok_or_else(usage)?,
                recording.ok_or_else(usage)?,
            )
            .map_err(debug)?;
            store.set(&name, &profile).map_err(debug)?;
            println!("stored");
        }
        _ => return Err(usage()),
    }
    Ok(())
}

fn profile_argument(args: &mut impl Iterator<Item = String>) -> Result<String, String> {
    if args.next().as_deref() != Some("--profile") {
        return Err(usage());
    }
    args.next().ok_or_else(usage)
}

fn reject_remaining(args: &mut impl Iterator<Item = String>) -> Result<(), String> {
    if args.next().is_some() {
        return Err(usage());
    }
    Ok(())
}

fn parse_availability(value: Option<&str>) -> Result<AvailabilityResult, String> {
    match value {
        Some("available") => Ok(AvailabilityResult::Available),
        Some("unavailable") => Ok(AvailabilityResult::Unavailable),
        Some("read_failed") => Ok(AvailabilityResult::ReadFailed),
        _ => Err(usage()),
    }
}

fn identity_init(root: PathBuf) -> Result<(), String> {
    if let Some(response) = owner_mutation(&root, |command_id, owner_generation| {
        OwnerCommand::IdentityInit {
            command_id,
            owner_generation,
        }
    })? {
        if response != OwnerResponse::IdentityInitialized {
            return Err(format!("owner mutation: {response:?}"));
        }
    } else {
        let _owner = standalone_owner_lock(&root)?;
        IdentityActor::start(IdentityStore::new(root))
            .init()
            .map_err(debug)?;
    }
    println!("identity_initialized");
    Ok(())
}

fn unpair(root: PathBuf, peer: &str) -> Result<(), String> {
    if let Some(response) =
        owner_mutation(&root, |command_id, owner_generation| OwnerCommand::Unpair {
            command_id,
            owner_generation,
            peer_id: peer.to_owned(),
        })?
    {
        if response != OwnerResponse::Unpaired {
            return Err(format!("owner mutation: {response:?}"));
        }
    } else {
        let _owner = standalone_owner_lock(&root)?;
        IdentityActor::start(IdentityStore::new(root))
            .unpair(peer.to_owned())
            .map_err(debug)?;
    }
    println!("unpaired {peer}");
    Ok(())
}

fn identity_root(root: Option<String>) -> PathBuf {
    PathBuf::from(root.unwrap_or_else(|| ".deskkin/phase3/host/identity".into()))
}

fn control_root(identity_root: &std::path::Path) -> Result<PathBuf, String> {
    identity_root
        .parent()
        .map(|role| role.join("control"))
        .ok_or_else(|| "identity root has no role parent".into())
}

struct StandaloneOwner {
    startup_guard: Option<std::fs::File>,
    _owner: std::fs::File,
}

impl StandaloneOwner {
    fn ready(&mut self) {
        self.startup_guard.take();
    }
}

fn standalone_owner_lock(identity_root: &std::path::Path) -> Result<StandaloneOwner, String> {
    let role_root = identity_root
        .parent()
        .ok_or_else(|| "identity root has no role parent".to_owned())?;
    let startup = deskkin_desktop_host::profile::managed_startup_barrier(role_root)
        .map_err(|error| error.to_string())?;
    let owner = acquire_owner_lock(&control_root(identity_root)?).map_err(debug)?;
    Ok(StandaloneOwner {
        startup_guard: startup,
        _owner: owner,
    })
}

fn owner_mutation(
    identity_root: &std::path::Path,
    command: impl FnOnce(String, String) -> OwnerCommand,
) -> Result<Option<OwnerResponse>, String> {
    let control = control_root(identity_root)?;
    let Some(owner_generation) = discover_owner(&control).map_err(debug)? else {
        return Ok(None);
    };
    let command_id = new_control_id().map_err(debug)?;
    let request = command(command_id.clone(), owner_generation.clone());
    let (response, ambiguous) = match call_owner_control(&control, &request) {
        Ok(response) => (response, false),
        Err(_) => (OwnerResponse::CommandUnknown, true),
    };
    let response = await_owner_result(
        &control,
        &command_id,
        &owner_generation,
        response,
        ambiguous,
    )?;
    Ok(Some(response))
}

fn await_owner_result(
    control: &std::path::Path,
    command_id: &str,
    owner_generation: &str,
    mut response: OwnerResponse,
    ambiguous: bool,
) -> Result<OwnerResponse, String> {
    let ambiguous_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut decision_sent = false;
    while matches!(
        response,
        OwnerResponse::CommandAccepted
            | OwnerResponse::CommandPending
            | OwnerResponse::PairingWaiting
            | OwnerResponse::PairingConfirmationRequired { .. }
    ) || (ambiguous && response == OwnerResponse::CommandUnknown)
    {
        if response == OwnerResponse::CommandUnknown
            && std::time::Instant::now() >= ambiguous_deadline
        {
            return Err("owner_lost_result_unknown".into());
        }
        if let OwnerResponse::PairingConfirmationRequired {
            pairing_transaction_id,
            authentication_string,
        } = &response
            && !decision_sent
        {
            println!("authentication {authentication_string}");
            let decision_id = new_control_id().map_err(debug)?;
            let decision = OwnerCommand::PairingDecide {
                command_id: decision_id.clone(),
                owner_generation: owner_generation.to_owned(),
                parent_command_id: command_id.to_owned(),
                pairing_transaction_id: pairing_transaction_id.clone(),
                confirmed: true,
            };
            if submit_pairing_decision(control, &decision_id, owner_generation, &decision)?
                != OwnerResponse::PairingDecisionAccepted
            {
                return Err("pairing decision was not accepted".into());
            }
            decision_sent = true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        response = query_owner_result(control, command_id, owner_generation)?;
    }
    if response == OwnerResponse::StaleOwner {
        return Err("owner_lost_result_unknown".into());
    }
    Ok(response)
}

fn submit_pairing_decision(
    control: &std::path::Path,
    command_id: &str,
    owner_generation: &str,
    command: &OwnerCommand,
) -> Result<OwnerResponse, String> {
    let mut response =
        call_owner_control(control, command).unwrap_or(OwnerResponse::CommandUnknown);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while matches!(
        response,
        OwnerResponse::CommandAccepted
            | OwnerResponse::CommandPending
            | OwnerResponse::CommandUnknown
    ) {
        if std::time::Instant::now() >= deadline {
            return Err("owner_lost_result_unknown".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        response = query_owner_result(control, command_id, owner_generation)?;
    }
    if response == OwnerResponse::StaleOwner {
        Err("owner_lost_result_unknown".into())
    } else {
        Ok(response)
    }
}

fn query_owner_result(
    control: &std::path::Path,
    command_id: &str,
    owner_generation: &str,
) -> Result<OwnerResponse, String> {
    query_command_result(control, command_id, owner_generation)
        .map_err(|_| "owner_lost_result_unknown".into())
}

fn usage() -> String {
    "usage: deskkin-desktop-host profile list|show NAME|set NAME --role-root ROOT --bind-mode loopback|private_lan --address ADDRESS --availability available|unavailable|read_failed --recording on|off|delete NAME | profile-host|profile-status|profile-stop --profile NAME | identity-init|identity-list [ROOT] | unpair PEER [ROOT] | pairing-window-open [ROOT] | serve-once ADDRESS available|unavailable|read_failed [ROOT] | owner [ROLE_ROOT] | run ADDRESS available|unavailable|read_failed [ROLE_ROOT] | run-private-lan RFC1918_IPV4:39042 available|unavailable|read_failed [ROLE_ROOT]".into()
}

fn debug(error: impl std::fmt::Debug) -> String {
    format!("{error:?}")
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::time::{Duration, Instant};

    use super::*;
    use deskkin_desktop_host::profile::ProfileState;

    struct TempCleanup(PathBuf);

    impl Drop for TempCleanup {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_dir_all(&self.0)
                && error.kind() != io::ErrorKind::NotFound
                && !std::thread::panicking()
            {
                panic!("failed to clean temporary test path: {error}");
            }
        }
    }

    #[test]
    fn standalone_listener_releases_startup_barrier_at_readiness() {
        let base = std::env::temp_dir().join(format!(
            "deskkin-standalone-ready-{}-{}",
            std::process::id(),
            new_control_id().unwrap()
        ));
        let _base_cleanup = TempCleanup(base.clone());
        let state_root = base.join(".deskkin");
        let role_root = state_root.join("roles/host");
        let identity_root = role_root.join("identity");
        let _ = std::fs::remove_dir_all(&base);
        let store = ProfileStore::at(state_root);
        let profile = PhysicalProfile::new(
            "roles/host".into(),
            BindMode::Loopback,
            "127.0.0.1:39032".parse().unwrap(),
            ProfileAvailability::Available,
            ProfileRecording::Off,
        )
        .unwrap();
        store.set("physical", &profile).unwrap();
        let mut owner = standalone_owner_lock(&identity_root).unwrap();
        let listener = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
        owner.ready();

        let started = Instant::now();
        assert_eq!(
            store.status("physical").unwrap(),
            ProfileState::OwnerUnknown
        );
        assert!(started.elapsed() < Duration::from_millis(250));
        drop(listener);
        drop(owner);
        std::fs::remove_dir_all(base).unwrap();
    }
}
