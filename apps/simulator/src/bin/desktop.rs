use std::io;
use std::path::PathBuf;

use deskkin_desktop_host::{
    IdentityActor, IdentityStore, OwnerCommand, OwnerResponse, acquire_owner_lock,
    call_owner_control, discover_owner, new_control_id, query_command_result,
};
use deskkin_simulator::{RecordingMode, run_desktop, run_protocol_desktop_with_recording};
use local_run_recorder::ResourceRole;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[allow(clippy::too_many_lines)]
fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("identity-init") => {
            let root = identity_root(args.next());
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
                IdentityActor::start(IdentityStore::new_for_role(
                    root,
                    ResourceRole::DeviceSimulator,
                ))
                .init()
                .map_err(debug)?;
            }
            println!("identity_initialized");
            Ok(())
        }
        Some("identity-list") => {
            let actor = IdentityActor::start(IdentityStore::new_for_role(
                identity_root(args.next()),
                ResourceRole::DeviceSimulator,
            ));
            println!("{:?}", actor.peer().map_err(debug)?);
            Ok(())
        }
        Some("unpair") => {
            let peer = args.next().ok_or_else(usage)?;
            let root = identity_root(args.next());
            if let Some(response) =
                owner_mutation(&root, |command_id, owner_generation| OwnerCommand::Unpair {
                    command_id,
                    owner_generation,
                    peer_id: peer.clone(),
                })?
            {
                if response != OwnerResponse::Unpaired {
                    return Err(format!("owner mutation: {response:?}"));
                }
            } else {
                let _owner = standalone_owner_lock(&root)?;
                IdentityActor::start(IdentityStore::new_for_role(
                    root,
                    ResourceRole::DeviceSimulator,
                ))
                .unpair(peer)
                .map_err(debug)?;
            }
            println!("unpaired");
            Ok(())
        }
        Some("pair-start") => {
            let address = args.next().ok_or_else(usage)?;
            let root = identity_root(args.next());
            let Some(response) = owner_mutation(&root, |command_id, owner_generation| {
                OwnerCommand::PairStart {
                    command_id,
                    owner_generation,
                    loopback_address: address,
                }
            })?
            else {
                return Err("simulator runtime owner is not running".into());
            };
            if response != OwnerResponse::Paired {
                return Err(format!("owner pairing: {response:?}"));
            }
            println!("paired");
            Ok(())
        }
        Some("run") => {
            let address = args.next().ok_or_else(usage)?.parse().map_err(debug)?;
            let remaining: Vec<_> = args.collect();
            let recording = if remaining.iter().any(|value| value == "--recording-off") {
                RecordingMode::Off
            } else {
                RecordingMode::On
            };
            let root = remaining
                .into_iter()
                .find(|value| value != "--recording-off");
            run_protocol_desktop_with_recording(address, &identity_root(root), recording)
        }
        Some("--recording-off") => run_desktop(RecordingMode::Off),
        None => run_desktop(RecordingMode::On),
        _ => Err(usage()),
    }
}

fn identity_root(root: Option<String>) -> PathBuf {
    PathBuf::from(root.unwrap_or_else(|| ".deskkin/phase3/device-simulator/identity".into()))
}

fn control_root(identity_root: &std::path::Path) -> Result<PathBuf, String> {
    identity_root
        .parent()
        .map(|role| role.join("control"))
        .ok_or_else(|| "identity root has no role parent".into())
}

struct StandaloneOwner {
    _startup: Option<std::fs::File>,
    _owner: std::fs::File,
}
fn standalone_owner_lock(identity_root: &std::path::Path) -> Result<StandaloneOwner, String> {
    let role_root = identity_root
        .parent()
        .ok_or_else(|| "identity root has no role parent".to_owned())?;
    let startup = deskkin_desktop_host::profile::managed_startup_barrier(role_root)
        .map_err(|error| error.to_string())?;
    let owner = acquire_owner_lock(&control_root(identity_root)?).map_err(debug)?;
    Ok(StandaloneOwner {
        _startup: startup,
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
    "usage: deskkin-desktop [--recording-off] | identity-init|identity-list [ROOT] | unpair PEER [ROOT] | pair-start ADDRESS [ROOT] | run ADDRESS [ROOT]".into()
}

fn debug(error: impl std::fmt::Debug) -> String {
    format!("{error:?}")
}
