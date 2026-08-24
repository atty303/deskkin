use std::io;
use std::path::PathBuf;

use deskkin_desktop_host::{
    IdentityActor, IdentityStore, OwnerCommand, OwnerResponse, acquire_owner_lock, bind_loopback,
    call_owner_control, discover_owner, new_control_id, query_command_result,
    run_host_runtime_with_recording, run_owner_control, serve_one,
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
        "identity-init" => {
            identity_init(identity_root(args.next()))?;
        }
        "identity-list" => println!(
            "{:?}",
            IdentityStore::new(identity_root(args.next()))
                .peer()
                .map_err(debug)?
        ),
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
            let _owner = standalone_owner_lock(&root)?;
            let store = IdentityStore::new(root);
            let listener = bind_loopback(address).map_err(debug)?;
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
        _ => return Err(usage()),
    }
    Ok(())
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

fn standalone_owner_lock(identity_root: &std::path::Path) -> Result<std::fs::File, String> {
    acquire_owner_lock(&control_root(identity_root)?).map_err(debug)
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
            println!("confirm? type yes");
            let mut input = String::new();
            let confirmed = io::stdin().read_line(&mut input).is_ok() && input.trim() == "yes";
            let decision_id = new_control_id().map_err(debug)?;
            let decision = OwnerCommand::PairingDecide {
                command_id: decision_id.clone(),
                owner_generation: owner_generation.to_owned(),
                parent_command_id: command_id.to_owned(),
                pairing_transaction_id: pairing_transaction_id.clone(),
                confirmed,
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
    "usage: deskkin-desktop-host identity-init|identity-list [ROOT] | unpair PEER [ROOT] | pairing-window-open [ROOT] | serve-once ADDRESS available|unavailable|read_failed [ROOT] | owner [ROLE_ROOT] | run ADDRESS available|unavailable|read_failed [ROLE_ROOT]".into()
}

fn debug(error: impl std::fmt::Debug) -> String {
    format!("{error:?}")
}
