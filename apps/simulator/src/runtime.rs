use std::cell::RefCell;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use deskkin_application::{
    Application, ApplicationEffectId, ApplicationInput, ApplicationView, Effect, EffectRequest,
    FeatureId, Lifecycle,
    availability::{
        self, Availability, Input as AvailabilityInput, ReadCompleted, ReadError, RefreshDue,
        TimerArmCompleted, TimerArmError,
    },
};
use slint::{ComponentHandle, Timer, TimerMode};

use deskkin_desktop_host::{
    ClientSession, IdentityActor, IdentityStore, OwnerCommand, OwnerEvent, OwnerPairingTask,
    OwnerResponse, SessionError, call_owner_control, discover_owner, new_context_id,
    new_control_id, pair_initiator_until, run_owner_control_with_events_scoped,
};
use deskkin_protocol::HelloRejectReason;

use crate::StatusWindow;
use crate::diagnostics::{
    ClosedValue, Completeness, DiagnosticRun, ErrorType, Operation, OperationStatus, Publication,
    Recorder, RecordingHealth, RecordingMode, ResourceRole, RunOutcome, SemanticRecord,
    finalize_operation_records, in_progress_run, new_run_id, now_unix_ms, resource_identity,
    resource_identity_for,
};
use crate::presenter::apply_view;
use deskkin_protocol_client::{AvailabilityValue, ProtocolAdapter, ProtocolEvent};

struct NativeRuntime {
    core: Application,
    ui: StatusWindow,
    read_timer: Timer,
    read_timeout_timer: Timer,
    refresh_timer: Timer,
    read_index: usize,
    logical_time_ms: u64,
    session_run_id: String,
    recorder: Recorder,
    active_read: Option<ActiveRead>,
}

struct ActiveRead {
    effect: Effect,
    run_id: String,
    started_ms: u64,
    started_at: Instant,
}

/// Runs the native Linux status window until it is closed.
///
/// # Errors
///
/// Returns an error when the Slint component, core startup, timer adapter, or
/// native event loop cannot start.
pub fn run_desktop(recording: RecordingMode) -> Result<(), String> {
    let ui = StatusWindow::new().map_err(|error| error.to_string())?;
    apply_view(&ui, ApplicationView::Empty);
    let runtime = Rc::new(RefCell::new(NativeRuntime {
        core: Application::new(),
        ui: ui.clone_strong(),
        read_timer: Timer::default(),
        read_timeout_timer: Timer::default(),
        refresh_timer: Timer::default(),
        read_index: 0,
        logical_time_ms: 0,
        session_run_id: new_run_id("native"),
        recorder: Recorder::from_environment(recording),
        active_read: None,
    }));
    let transition = runtime
        .borrow_mut()
        .core
        .transition(ApplicationInput::Lifecycle(Lifecycle::Start))
        .map_err(|error| format!("application start: {error:?}"))?;
    apply_view(&runtime.borrow().ui, transition.view);
    dispatch_effect(
        &runtime,
        transition
            .effects
            .get(0)
            .ok_or("start did not request read")?,
    )?;
    let result = ui.run().map_err(|error| error.to_string());
    let mut state = runtime.borrow_mut();
    state.read_timer.stop();
    state.read_timeout_timer.stop();
    state.refresh_timer.stop();
    if let Some(active) = state.active_read.take() {
        let elapsed_ms = u64::try_from(active.started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
        state.logical_time_ms = active.started_ms.saturating_add(elapsed_ms);
        let _ = publish_interrupted_refresh(
            &state.recorder,
            &state.session_run_id,
            active,
            state.logical_time_ms,
            RunOutcome::Cancel,
        );
    }
    result
}

struct ProtocolRuntime {
    core: Application,
    ui: StatusWindow,
    refresh_timer: Timer,
    reconnect_timer: Timer,
    owner_event_timer: Timer,
    network_event_timer: Timer,
    adapter: ProtocolAdapter,
    connected: bool,
    connecting: bool,
    pending_session: Option<[u8; 16]>,
    network_commands: std::sync::mpsc::SyncSender<NetworkCommand>,
    network_control: std::sync::mpsc::Sender<NetworkControl>,
    network_events: std::sync::mpsc::Receiver<NetworkEvent>,
    network: Option<JoinHandle<()>>,
    revocation_join_ack: Option<std::sync::mpsc::SyncSender<()>>,
    shutdown_join_ack: Option<std::sync::mpsc::SyncSender<()>>,
    control_root: PathBuf,
    owner: Option<JoinHandle<std::io::Result<()>>>,
    protocol_diagnostics: Option<std::sync::mpsc::SyncSender<DiagnosticRun>>,
    diagnostic_join: Option<JoinHandle<()>>,
    diagnostic_dropped: Arc<AtomicBool>,
    diagnostic_spans: std::sync::Mutex<std::collections::HashMap<String, ProtocolDiagnosticSpan>>,
    owner_events: std::sync::mpsc::Receiver<OwnerEvent>,
    identity: IdentityStore,
}

struct ProtocolDiagnosticSpan {
    run_id: String,
    created_unix_ms: u64,
    started_at: Instant,
    operation_kind: Operation,
    session_context: Option<[u8; 16]>,
    operation_context: Option<[u8; 16]>,
}

enum NetworkCommand {
    Pair {
        address: SocketAddr,
        task: OwnerPairingTask,
    },
    Connect {
        session: [u8; 16],
    },
    Read {
        session: [u8; 16],
        request_id: u32,
        operation: [u8; 16],
    },
}

enum NetworkControl {
    Close,
    Revoke,
    Shutdown,
}

enum NetworkEvent {
    Paired {
        trust_paired: bool,
        result: Result<(), SessionError>,
    },
    Connected {
        session: [u8; 16],
        result: Result<(), SessionError>,
    },
    ReadCompleted {
        session: [u8; 16],
        generation: u64,
        request_id: u32,
        operation: [u8; 16],
        result: Result<deskkin_protocol::AvailabilityResult, SessionError>,
    },
    Closed,
    WorkerStopped,
}

#[allow(clippy::too_many_lines)]
fn run_network_worker(
    address: SocketAddr,
    identity: &IdentityStore,
    commands: &std::sync::mpsc::Receiver<NetworkCommand>,
    control: &std::sync::mpsc::Receiver<NetworkControl>,
    events: &std::sync::mpsc::Sender<NetworkEvent>,
) {
    let mut client: Option<ClientSession> = None;
    let mut revoked = false;
    loop {
        match control.try_recv() {
            Ok(NetworkControl::Close) => {
                if let Some(client) = client.take() {
                    let _ = client.close();
                }
                if events.send(NetworkEvent::Closed).is_err() {
                    break;
                }
                continue;
            }
            Ok(NetworkControl::Revoke) => {
                revoked = true;
                if let Some(client) = client.take() {
                    let _ = client.close();
                }
                drain_revoked_commands(commands, events);
                if events.send(NetworkEvent::Closed).is_err() {
                    break;
                }
                continue;
            }
            Ok(NetworkControl::Shutdown) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                if let Some(client) = client.take() {
                    let _ = client.close();
                }
                break;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
        let command = match commands.recv_timeout(Duration::from_millis(10)) {
            Ok(command) => command,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if revoked && !matches!(command, NetworkCommand::Pair { .. }) {
            reject_revoked_command(command, events);
            continue;
        }
        match command {
            NetworkCommand::Pair { address, task } => {
                let session = new_context_id().ok();
                let event = run_pair_command(identity, address, task, session);
                if matches!(
                    &event,
                    NetworkEvent::Paired {
                        trust_paired: true,
                        ..
                    }
                ) {
                    revoked = false;
                }
                if events.send(event).is_err() {
                    break;
                }
            }
            NetworkCommand::Connect { session } => {
                let result =
                    ClientSession::connect_with_external_diagnostics(address, identity, session);
                let event_result = result.as_ref().map(|_| ()).map_err(Clone::clone);
                client = result.ok();
                if events
                    .send(NetworkEvent::Connected {
                        session,
                        result: event_result,
                    })
                    .is_err()
                {
                    break;
                }
            }
            NetworkCommand::Read {
                session,
                request_id,
                operation,
            } => {
                let generation = client.as_ref().map_or(0, ClientSession::generation);
                let result = client
                    .as_mut()
                    .ok_or(SessionError::Io)
                    .and_then(|client| client.read_availability(operation));
                if result.is_err() {
                    client = None;
                }
                if events
                    .send(NetworkEvent::ReadCompleted {
                        session,
                        generation,
                        request_id,
                        operation,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
            }
        }
    }
    let _ = events.send(NetworkEvent::WorkerStopped);
}

fn drain_revoked_commands(
    commands: &std::sync::mpsc::Receiver<NetworkCommand>,
    events: &std::sync::mpsc::Sender<NetworkEvent>,
) {
    while let Ok(command) = commands.try_recv() {
        reject_revoked_command(command, events);
    }
}

fn reject_revoked_command(command: NetworkCommand, events: &std::sync::mpsc::Sender<NetworkEvent>) {
    match command {
        NetworkCommand::Pair { task, .. } => task.finish(false),
        NetworkCommand::Connect { session } => {
            let _ = events.send(NetworkEvent::Connected {
                session,
                result: Err(SessionError::Identity),
            });
        }
        NetworkCommand::Read {
            session,
            request_id,
            operation,
        } => {
            let _ = events.send(NetworkEvent::ReadCompleted {
                session,
                generation: 0,
                request_id,
                operation,
                result: Err(SessionError::Identity),
            });
        }
    }
}

fn run_pair_command(
    identity: &IdentityStore,
    address: SocketAddr,
    task: OwnerPairingTask,
    session: Option<[u8; 16]>,
) -> NetworkEvent {
    let was_unpaired = matches!(
        identity.peer(),
        Ok(deskkin_desktop_host::PeerState::Unpaired)
    );
    let result = session.ok_or(SessionError::Noise).and_then(|session| {
        let confirmation = task.clone();
        let deadline = task.deadline();
        pair_initiator_until(
            address,
            identity,
            session,
            move |transaction, sas| confirmation.confirm(transaction, sas),
            deadline,
        )
    });
    let paired_now = was_unpaired
        && matches!(
            identity.peer(),
            Ok(deskkin_desktop_host::PeerState::Paired { .. })
        );
    let success = paired_now;
    task.finish(paired_now);
    NetworkEvent::Paired {
        trust_paired: paired_now,
        result: match result {
            Ok(_) if success => Ok(()),
            Ok(_) => Err(SessionError::Identity),
            Err(error) => Err(error),
        },
    }
}

/// Runs the shared status UI against an authenticated loopback host.
///
/// The portable core remains the sole owner of refresh state. Transport loss
/// invalidates the current availability immediately, while the existing core
/// timer determines the next read attempt.
///
/// # Errors
///
/// Returns an error for a non-loopback address, missing or invalid identity,
/// owner-control startup, authenticated transport, core, timer, or UI failure.
pub fn run_protocol_desktop(address: SocketAddr, identity_root: &Path) -> Result<(), String> {
    run_protocol_desktop_with_recording(address, identity_root, RecordingMode::On)
}

/// Runs the authenticated loopback UI with explicit diagnostic recording mode.
///
/// # Errors
///
/// Returns the same startup, transport, core, timer, and UI errors as
/// [`run_protocol_desktop`].
#[allow(clippy::too_many_lines)]
pub fn run_protocol_desktop_with_recording(
    address: SocketAddr,
    identity_root: &Path,
    recording: RecordingMode,
) -> Result<(), String> {
    if !address.ip().is_loopback() {
        return Err("protocol host must be loopback".into());
    }
    let ui = StatusWindow::new().map_err(|error| error.to_string())?;
    let role_root = identity_root
        .parent()
        .ok_or("identity root has no role parent")?
        .to_path_buf();
    let control_root = role_root.join("control");
    let identity =
        IdentityStore::new_for_role(identity_root.to_path_buf(), ResourceRole::DeviceSimulator);
    let actor = IdentityActor::start(identity.clone());
    actor
        .peer()
        .map_err(|error| format!("identity state: {error:?}"))?;
    let generation = new_control_id().map_err(|error| format!("owner generation: {error:?}"))?;
    let owner_actor = actor.clone();
    let owner_root = control_root.clone();
    let (owner_event_sender, owner_events) = std::sync::mpsc::channel();
    let startup = deskkin_desktop_host::profile::managed_startup_barrier(&role_root)
        .map_err(|error| format!("profile startup barrier: {error}"))?;
    let (owner_ready, owner_readiness) = std::sync::mpsc::sync_channel(1);
    let owner_cancel = Arc::new(AtomicBool::new(false));
    let owner_startup_cancel = owner_cancel.clone();
    let owner = thread::spawn(move || {
        run_owner_control_with_events_scoped(
            &owner_root,
            &owner_actor,
            &generation,
            Some(owner_event_sender),
            startup,
            owner_ready,
            owner_startup_cancel,
        )
    });
    if owner_readiness
        .recv_timeout(Duration::from_secs(2))
        .is_err()
    {
        owner_cancel.store(true, Ordering::Release);
        return match owner.join() {
            Ok(Ok(())) => Err("simulator owner control start timed out".into()),
            Ok(Err(error)) => Err(format!("simulator owner control failed to start: {error}")),
            Err(_) => Err("simulator owner control startup panicked".into()),
        };
    }
    apply_view(&ui, ApplicationView::Empty);
    let (network_commands, network_command_receiver) = std::sync::mpsc::sync_channel(8);
    let (network_control, network_control_receiver) = std::sync::mpsc::channel();
    let (network_event_sender, network_events) = std::sync::mpsc::channel();
    let network_identity = identity.clone();
    let network = thread::spawn(move || {
        run_network_worker(
            address,
            &network_identity,
            &network_command_receiver,
            &network_control_receiver,
            &network_event_sender,
        );
    });
    let diagnostic_recorder = Recorder::new(role_root, recording, 16 * 1024 * 1024);
    let (protocol_diagnostics, diagnostic_runs) =
        std::sync::mpsc::sync_channel::<DiagnosticRun>(32);
    let diagnostic_dropped = Arc::new(AtomicBool::new(false));
    let worker_dropped = diagnostic_dropped.clone();
    let diagnostic_join = thread::spawn(move || {
        let mut live_runs = std::collections::HashMap::new();
        while let Ok(run) = diagnostic_runs.recv() {
            if !run.terminal
                && let Ok(marker) = diagnostic_recorder.begin_live_run(&run.run_id)
            {
                live_runs.insert(run.run_id.clone(), marker);
            }
            let terminal_run_id = run.terminal.then(|| run.run_id.clone());
            let _ = diagnostic_recorder.publish(run);
            if let Some(run_id) = terminal_run_id
                && let Some(marker) = live_runs.remove(&run_id)
            {
                let _ = diagnostic_recorder.end_live_run(&run_id, marker);
            }
            if worker_dropped.swap(false, Ordering::AcqRel) {
                let _ = diagnostic_recorder.publish_health_best_effort(&Publication {
                    run_id: "diagnostic-queue-full".into(),
                    completeness: Completeness::Dropped,
                    health: RecordingHealth::StorageUnavailable,
                    stored: false,
                });
            }
        }
    });
    let runtime = Rc::new(RefCell::new(ProtocolRuntime {
        core: Application::new(),
        ui: ui.clone_strong(),
        refresh_timer: Timer::default(),
        reconnect_timer: Timer::default(),
        owner_event_timer: Timer::default(),
        network_event_timer: Timer::default(),
        adapter: ProtocolAdapter::new(),
        connected: false,
        connecting: false,
        pending_session: None,
        network_commands,
        network_control,
        network_events,
        network: Some(network),
        revocation_join_ack: None,
        shutdown_join_ack: None,
        control_root,
        owner: Some(owner),
        protocol_diagnostics: Some(protocol_diagnostics),
        diagnostic_join: Some(diagnostic_join),
        diagnostic_dropped,
        diagnostic_spans: std::sync::Mutex::new(std::collections::HashMap::new()),
        owner_events,
        identity,
    }));
    let weak = Rc::downgrade(&runtime);
    runtime.borrow().owner_event_timer.start(
        TimerMode::Repeated,
        Duration::from_millis(25),
        move || handle_owner_events(&weak),
    );
    let weak = Rc::downgrade(&runtime);
    runtime.borrow().network_event_timer.start(
        TimerMode::Repeated,
        Duration::from_millis(10),
        move || handle_network_events(&weak),
    );
    attempt_protocol_connect(&runtime)?;
    let effect = runtime
        .borrow_mut()
        .core
        .transition(ApplicationInput::Lifecycle(Lifecycle::Start))
        .map_err(|error| format!("application start: {error:?}"))?
        .effects
        .get(0)
        .ok_or("start did not request read")?;
    apply_view(&runtime.borrow().ui, runtime.borrow().core.view());
    dispatch_protocol_effect(&runtime, effect)?;
    let result = ui.run().map_err(|error| error.to_string());
    let mut state = runtime.borrow_mut();
    state.refresh_timer.stop();
    state.reconnect_timer.stop();
    state.owner_event_timer.stop();
    state.network_event_timer.stop();
    let _ = state.network_control.send(NetworkControl::Shutdown);
    if let Some(network) = state.network.take() {
        network
            .join()
            .map_err(|_| "simulator network worker panicked".to_owned())?;
    }
    let owner_generation = discover_owner(&state.control_root)
        .map_err(|error| format!("simulator owner discovery: {error}"))?
        .ok_or_else(|| "simulator owner is not running".to_owned())?;
    let shutdown_response = call_owner_control(
        &state.control_root,
        &OwnerCommand::Shutdown { owner_generation },
    )
    .map_err(|error| format!("simulator owner shutdown: {error}"))?;
    if shutdown_response != OwnerResponse::ShutdownAccepted {
        return Err("simulator owner rejected shutdown".into());
    }
    loop {
        match state.owner_events.recv() {
            Ok(OwnerEvent::RuntimeShutdown { joined }) => {
                let _ = joined.send(());
                break;
            }
            Ok(OwnerEvent::IdentityRevoked { joined }) => {
                let _ = joined.send(());
            }
            Ok(OwnerEvent::PairStart { task, .. } | OwnerEvent::PairingWindowOpen { task }) => {
                task.finish(false);
            }
            Err(_) => return Err("simulator owner shutdown coordinator was lost".into()),
        }
    }
    if let Some(owner) = state.owner.take() {
        owner
            .join()
            .map_err(|_| "simulator owner control panicked".to_owned())?
            .map_err(|error| error.to_string())?;
    }
    drain_protocol_spans(&state, RunOutcome::Cancel, Some(ErrorType::Cancelled));
    state.protocol_diagnostics.take();
    if let Some(diagnostic_join) = state.diagnostic_join.take() {
        let _ = diagnostic_join.join();
    }
    result
}

fn handle_owner_events(weak: &Weak<RefCell<ProtocolRuntime>>) {
    let Some(runtime) = weak.upgrade() else {
        return;
    };
    loop {
        let event = runtime.borrow().owner_events.try_recv();
        let Ok(event) = event else {
            break;
        };
        match event {
            OwnerEvent::IdentityRevoked { joined } => {
                let mut state = runtime.borrow_mut();
                state.connected = false;
                state.connecting = false;
                state.reconnect_timer.stop();
                state.adapter.stop();
                let invalidation_effect = state
                    .core
                    .transition(ApplicationInput::Lifecycle(Lifecycle::SessionInvalidated))
                    .ok()
                    .and_then(|transition| {
                        apply_view(&state.ui, transition.view);
                        transition.effects.get(0)
                    });
                state.revocation_join_ack = Some(joined);
                let _ = state.network_control.send(NetworkControl::Revoke);
                drop(state);
                if let Some(effect) = invalidation_effect {
                    let _ = dispatch_protocol_effect(&runtime, effect);
                }
            }
            OwnerEvent::PairStart { address, task } => {
                start_protocol(&runtime.borrow(), Operation::PairingPersist, None, None);
                if let Err(error) = runtime
                    .borrow()
                    .network_commands
                    .try_send(NetworkCommand::Pair { address, task })
                {
                    let command = match error {
                        std::sync::mpsc::TrySendError::Full(command)
                        | std::sync::mpsc::TrySendError::Disconnected(command) => command,
                    };
                    if let NetworkCommand::Pair { task, .. } = command {
                        task.finish(false);
                    }
                    record_protocol(
                        &runtime.borrow(),
                        Operation::PairingPersist,
                        RunOutcome::Error,
                        Some(ErrorType::QueueFull),
                        None,
                        None,
                    );
                }
            }
            OwnerEvent::PairingWindowOpen { task } => task.finish(false),
            OwnerEvent::RuntimeShutdown { joined } => {
                let mut state = runtime.borrow_mut();
                if state.network.is_none() {
                    let _ = joined.send(());
                } else {
                    state.shutdown_join_ack = Some(joined);
                    let _ = state.network_control.send(NetworkControl::Shutdown);
                }
            }
        }
    }
}

#[allow(clippy::too_many_lines)]
fn handle_network_events(weak: &Weak<RefCell<ProtocolRuntime>>) {
    let Some(runtime) = weak.upgrade() else {
        return;
    };
    loop {
        let event = runtime.borrow().network_events.try_recv();
        let Ok(event) = event else {
            break;
        };
        match event {
            NetworkEvent::Paired {
                trust_paired,
                result,
            } => {
                record_protocol(
                    &runtime.borrow(),
                    Operation::PairingPersist,
                    if trust_paired {
                        RunOutcome::Success
                    } else {
                        RunOutcome::Error
                    },
                    (!trust_paired).then_some(pairing_error_type(result.as_ref().err().copied())),
                    None,
                    None,
                );
                if trust_paired {
                    let mut state = runtime.borrow_mut();
                    state.connected = false;
                    state.connecting = false;
                    if !apply_pairing_outcome(&mut state.adapter, result) {
                        continue;
                    }
                    drop(state);
                    let _ = schedule_protocol_reconnect(&runtime);
                }
            }
            NetworkEvent::Connected { session, result } => {
                handle_connect_result(&runtime, session, result);
            }
            NetworkEvent::ReadCompleted {
                session,
                generation,
                request_id,
                operation,
                result,
            } => handle_read_result(&runtime, session, generation, request_id, operation, result),
            NetworkEvent::Closed => {
                drain_protocol_spans(
                    &runtime.borrow(),
                    RunOutcome::Cancel,
                    Some(ErrorType::Cancelled),
                );
                let acknowledgement = runtime.borrow_mut().revocation_join_ack.take();
                if let Some(acknowledgement) = acknowledgement {
                    let _ = acknowledgement.send(());
                }
            }
            NetworkEvent::WorkerStopped => {
                drain_protocol_spans(
                    &runtime.borrow(),
                    RunOutcome::Cancel,
                    Some(ErrorType::Cancelled),
                );
                let (network, acknowledgement) = {
                    let mut state = runtime.borrow_mut();
                    (state.network.take(), state.shutdown_join_ack.take())
                };
                if let Some(network) = network {
                    let _ = network.join();
                }
                if let Some(acknowledgement) = acknowledgement {
                    let _ = acknowledgement.send(());
                }
            }
        }
    }
}

fn apply_pairing_outcome(adapter: &mut ProtocolAdapter, result: Result<(), SessionError>) -> bool {
    adapter.restart_after_pairing();
    match result {
        Err(SessionError::Incompatible) => {
            adapter.hello_rejected(HelloRejectReason::NoCommonVersion);
        }
        Err(SessionError::AuthorizationDenied) => {
            adapter.hello_rejected(HelloRejectReason::PermissionDenied);
        }
        _ => {}
    }
    adapter.terminal_reason().is_none()
}

fn pairing_error_type(error: Option<SessionError>) -> ErrorType {
    match error {
        Some(SessionError::PairingTimeout) => ErrorType::PairingExpired,
        Some(SessionError::PairingRejected) => ErrorType::PairingRejected,
        Some(SessionError::StoreFailed | SessionError::Identity) => ErrorType::StoreFailed,
        _ => ErrorType::PairingIncomplete,
    }
}

fn dispatch_protocol_effect(
    runtime: &Rc<RefCell<ProtocolRuntime>>,
    effect: Effect,
) -> Result<(), String> {
    match effect.request {
        EffectRequest::Availability(availability::EffectRequest::ReadAvailability) => {
            perform_protocol_read(runtime, effect)
        }
        EffectRequest::Availability(availability::EffectRequest::ArmRefreshTimer { delay_ms }) => {
            arm_protocol_refresh(runtime, effect, delay_ms)
        }
        EffectRequest::SyntheticNotice(_) => Ok(()),
    }
}

fn perform_protocol_read(
    runtime: &Rc<RefCell<ProtocolRuntime>>,
    effect: Effect,
) -> Result<(), String> {
    if !runtime.borrow().connected {
        return complete_read_while_disconnected(runtime, effect);
    }

    let operation = new_context_id().map_err(|error| format!("operation context: {error:?}"))?;
    let request_id = {
        let mut state = runtime.borrow_mut();
        state
            .adapter
            .begin_read(effect.id.local.get(), operation)
            .map_err(|error| format!("begin availability read: {error:?}"))?
    };
    let session = runtime
        .borrow()
        .adapter
        .session_context()
        .ok_or("authenticated session context missing")?;
    start_protocol(
        &runtime.borrow(),
        Operation::AvailabilityRead,
        Some(session),
        Some(operation),
    );
    let command = NetworkCommand::Read {
        session,
        request_id,
        operation,
    };
    match runtime.borrow().network_commands.try_send(command) {
        Ok(()) => return Ok(()),
        Err(std::sync::mpsc::TrySendError::Full(_)) => {
            let _ = runtime.borrow().network_control.send(NetworkControl::Close);
        }
        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {}
    }
    record_protocol(
        &runtime.borrow(),
        Operation::AvailabilityRead,
        RunOutcome::Error,
        Some(ErrorType::QueueFull),
        Some(session),
        Some(operation),
    );
    protocol_disconnected(runtime)
}

fn complete_read_while_disconnected(
    runtime: &Rc<RefCell<ProtocolRuntime>>,
    effect: Effect,
) -> Result<(), String> {
    let next = {
        let mut state = runtime.borrow_mut();
        let transition = state
            .core
            .transition(ApplicationInput::availability(
                effect.id,
                AvailabilityInput::ReadCompleted(ReadCompleted {
                    effect_id: effect.id.local,
                    result: Err(ReadError::Unavailable),
                }),
            ))
            .map_err(|error| format!("disconnected read completion: {error:?}"))?;
        apply_view(&state.ui, transition.view);
        transition.effects.get(0)
    };
    if !runtime.borrow().connecting
        && runtime.borrow().adapter.terminal_reason().is_none()
        && !runtime.borrow().reconnect_timer.running()
    {
        schedule_protocol_reconnect(runtime)?;
    }
    if let Some(effect) = next {
        dispatch_protocol_effect(runtime, effect)?;
    }
    Ok(())
}

fn handle_read_result(
    runtime: &Rc<RefCell<ProtocolRuntime>>,
    session: [u8; 16],
    generation: u64,
    request_id: u32,
    operation: [u8; 16],
    result: Result<deskkin_protocol::AvailabilityResult, SessionError>,
) {
    let Ok(result) = result else {
        record_protocol(
            &runtime.borrow(),
            Operation::AvailabilityRead,
            RunOutcome::Error,
            Some(ErrorType::ConnectionLost),
            Some(session),
            Some(operation),
        );
        let _ = protocol_disconnected(runtime);
        return;
    };
    let identity = runtime.borrow().identity.clone();
    let next = identity.with_paired_generation(generation, || {
        let mut state = runtime.borrow_mut();
        if !state.connected || state.adapter.session_context() != Some(session) {
            return None;
        }
        let Ok(event) = state.adapter.result(session, request_id, operation, result) else {
            return None;
        };
        let input = protocol_event_input(event).ok()?;
        let transition = state.core.transition(input).ok()?;
        apply_view(&state.ui, transition.view);
        record_protocol(
            &state,
            Operation::AvailabilityRead,
            if result == deskkin_protocol::AvailabilityResult::ReadFailed {
                RunOutcome::Error
            } else {
                RunOutcome::Success
            },
            (result == deskkin_protocol::AvailabilityResult::ReadFailed)
                .then_some(ErrorType::ReadUnavailable),
            Some(session),
            Some(operation),
        );
        transition.effects.get(0)
    });
    let Ok(next) = next else {
        record_protocol(
            &runtime.borrow(),
            Operation::AvailabilityRead,
            RunOutcome::Error,
            Some(ErrorType::ConnectionLost),
            Some(session),
            Some(operation),
        );
        let _ = protocol_disconnected(runtime);
        return;
    };
    if next.is_none()
        && protocol_span_active(
            &runtime.borrow(),
            Operation::AvailabilityRead,
            Some(session),
            Some(operation),
        )
    {
        record_protocol(
            &runtime.borrow(),
            Operation::AvailabilityRead,
            RunOutcome::Error,
            Some(ErrorType::ConnectionLost),
            Some(session),
            Some(operation),
        );
    }
    if let Some(effect) = next {
        let _ = dispatch_protocol_effect(runtime, effect);
    }
}

fn protocol_disconnected(runtime: &Rc<RefCell<ProtocolRuntime>>) -> Result<(), String> {
    let (next, schedule_reconnect) = {
        let mut state = runtime.borrow_mut();
        let session = state.adapter.session_context();
        state.connected = false;
        state.connecting = false;
        state.pending_session = None;
        let next = if let Some(event) = state.adapter.disconnected() {
            let input = protocol_event_input(event)?;
            let transition = state
                .core
                .transition(input)
                .map_err(|error| format!("disconnect invalidation: {error:?}"))?;
            apply_view(&state.ui, transition.view);
            transition.effects.get(0)
        } else {
            None
        };
        record_protocol(
            &state,
            Operation::ProtocolNegotiate,
            RunOutcome::Error,
            Some(ErrorType::ConnectionLost),
            session,
            None,
        );
        (next, !state.reconnect_timer.running())
    };
    if schedule_reconnect {
        schedule_protocol_reconnect(runtime)?;
    }
    if let Some(effect) = next {
        dispatch_protocol_effect(runtime, effect)?;
    }
    Ok(())
}

fn protocol_event_input(event: ProtocolEvent) -> Result<ApplicationInput, String> {
    match event {
        ProtocolEvent::SessionInvalidated => {
            Ok(ApplicationInput::Lifecycle(Lifecycle::SessionInvalidated))
        }
        ProtocolEvent::AvailabilityCompleted { effect_id, value } => {
            let effect_id = deskkin_application::LocalEffectId::new(effect_id)
                .ok_or_else(|| "protocol returned zero effect identity".to_owned())?;
            let result = match value {
                AvailabilityValue::Available => Ok(Availability::Available),
                AvailabilityValue::Unavailable => Ok(Availability::Unavailable),
                AvailabilityValue::ReadFailed => Err(ReadError::Unavailable),
            };
            Ok(ApplicationInput::availability(
                ApplicationEffectId {
                    feature: FeatureId::Availability,
                    local: effect_id,
                },
                AvailabilityInput::ReadCompleted(ReadCompleted { effect_id, result }),
            ))
        }
    }
}

fn schedule_protocol_reconnect(runtime: &Rc<RefCell<ProtocolRuntime>>) -> Result<(), String> {
    let delay_ms = runtime
        .borrow_mut()
        .adapter
        .connection_failed()
        .map_err(|error| format!("reconnect backoff: {error:?}"))?;
    let weak = Rc::downgrade(runtime);
    runtime.borrow().reconnect_timer.start(
        TimerMode::SingleShot,
        Duration::from_millis(u64::from(delay_ms)),
        move || {
            if let Some(runtime) = weak.upgrade() {
                let _ = attempt_protocol_connect(&runtime);
            }
        },
    );
    if runtime.borrow().reconnect_timer.running() {
        Ok(())
    } else {
        Err("Slint reconnect timer did not start".into())
    }
}

fn attempt_protocol_connect(runtime: &Rc<RefCell<ProtocolRuntime>>) -> Result<(), String> {
    if runtime.borrow().connected || runtime.borrow().connecting {
        return Ok(());
    }
    let session = new_context_id().map_err(|error| format!("session context: {error:?}"))?;
    {
        let mut state = runtime.borrow_mut();
        state.adapter.connecting();
        state.connecting = true;
        state.pending_session = Some(session);
        start_protocol(&state, Operation::ProtocolNegotiate, Some(session), None);
        if state
            .network_commands
            .try_send(NetworkCommand::Connect { session })
            .is_err()
        {
            state.connecting = false;
            state.pending_session = None;
            record_protocol(
                &state,
                Operation::ProtocolNegotiate,
                RunOutcome::Error,
                Some(ErrorType::QueueFull),
                Some(session),
                None,
            );
            return Err("network command queue unavailable".to_owned());
        }
    }
    Ok(())
}

fn handle_connect_result(
    runtime: &Rc<RefCell<ProtocolRuntime>>,
    session: [u8; 16],
    result: Result<(), SessionError>,
) {
    if runtime.borrow().pending_session != Some(session) {
        return;
    }
    {
        let mut state = runtime.borrow_mut();
        state.connecting = false;
        state.pending_session = None;
    }
    match result {
        Ok(()) => {
            let mut state = runtime.borrow_mut();
            state.reconnect_timer.stop();
            state.adapter.authenticated(session);
            state.connected = true;
        }
        Err(SessionError::Incompatible) => {
            let _ = protocol_terminal(runtime, session, HelloRejectReason::NoCommonVersion);
        }
        Err(SessionError::AuthorizationDenied) => {
            let _ = protocol_terminal(runtime, session, HelloRejectReason::PermissionDenied);
        }
        Err(_) => {
            record_protocol(
                &runtime.borrow(),
                Operation::ProtocolNegotiate,
                RunOutcome::Error,
                Some(ErrorType::ConnectionLost),
                Some(session),
                None,
            );
            let _ = schedule_protocol_reconnect(runtime);
        }
    }
}

fn protocol_terminal(
    runtime: &Rc<RefCell<ProtocolRuntime>>,
    session: [u8; 16],
    reason: HelloRejectReason,
) -> Result<(), String> {
    let mut state = runtime.borrow_mut();
    state.connected = false;
    state.connecting = false;
    state.pending_session = None;
    state.reconnect_timer.stop();
    state.adapter.hello_rejected(reason);
    let error = match reason {
        HelloRejectReason::NoCommonVersion | HelloRejectReason::RequiredFeatureUnsupported => {
            ErrorType::VersionMismatch
        }
        HelloRejectReason::PermissionDenied => ErrorType::PermissionDenied,
        HelloRejectReason::SessionBusy => ErrorType::SessionBusy,
    };
    record_protocol(
        &state,
        Operation::ProtocolNegotiate,
        RunOutcome::Error,
        Some(error),
        Some(session),
        None,
    );
    let timer = state
        .core
        .transition(ApplicationInput::Lifecycle(Lifecycle::SessionInvalidated))
        .map_err(|error| format!("terminal invalidation: {error:?}"))?
        .effects
        .get(0);
    if let Some(timer) = timer {
        state
            .core
            .transition(ApplicationInput::availability(
                timer.id,
                AvailabilityInput::TimerArmCompleted(TimerArmCompleted {
                    effect_id: timer.id.local,
                    result: Err(TimerArmError::Unavailable),
                }),
            ))
            .map_err(|error| format!("terminal stop: {error:?}"))?;
    }
    apply_view(&state.ui, state.core.view());
    Ok(())
}

fn start_protocol(
    state: &ProtocolRuntime,
    operation_kind: Operation,
    session_context: Option<[u8; 16]>,
    operation_context: Option<[u8; 16]>,
) {
    let Some(key) = protocol_diagnostic_key(operation_kind, session_context, operation_context)
    else {
        return;
    };
    let Ok(run_context) = new_context_id() else {
        return;
    };
    let span = ProtocolDiagnosticSpan {
        run_id: format!("run-{}", encode_context(run_context)),
        created_unix_ms: now_unix_ms(),
        started_at: Instant::now(),
        operation_kind,
        session_context,
        operation_context,
    };
    let partial = protocol_run(
        &span,
        operation_kind,
        RunOutcome::Error,
        None,
        session_context,
        operation_context,
        false,
    );
    if let Some(publisher) = &state.protocol_diagnostics {
        if publisher.try_send(partial).is_err() {
            state.diagnostic_dropped.store(true, Ordering::Release);
            return;
        }
        if let Ok(mut spans) = state.diagnostic_spans.lock() {
            spans.insert(key, span);
        }
    }
}

fn record_protocol(
    state: &ProtocolRuntime,
    operation_kind: Operation,
    outcome: RunOutcome,
    error_type: Option<ErrorType>,
    session_context: Option<[u8; 16]>,
    operation_context: Option<[u8; 16]>,
) {
    let key = protocol_diagnostic_key(operation_kind, session_context, operation_context);
    let span = key
        .as_ref()
        .and_then(|key| state.diagnostic_spans.lock().ok()?.remove(key));
    let Some(span) = span else { return };
    let run = protocol_run(
        &span,
        operation_kind,
        outcome,
        error_type,
        session_context,
        operation_context,
        true,
    );
    if let Some(publisher) = &state.protocol_diagnostics
        && publisher.try_send(run).is_err()
    {
        state.diagnostic_dropped.store(true, Ordering::Release);
    }
}

fn drain_protocol_spans(
    state: &ProtocolRuntime,
    outcome: RunOutcome,
    error_type: Option<ErrorType>,
) {
    let spans = state
        .diagnostic_spans
        .lock()
        .map(|mut spans| spans.drain().map(|(_, span)| span).collect::<Vec<_>>())
        .unwrap_or_default();
    for span in spans {
        let run = protocol_run(
            &span,
            span.operation_kind,
            outcome,
            error_type,
            span.session_context,
            span.operation_context,
            true,
        );
        if let Some(publisher) = &state.protocol_diagnostics
            && publisher.try_send(run).is_err()
        {
            state.diagnostic_dropped.store(true, Ordering::Release);
        }
    }
}

fn protocol_diagnostic_key(
    operation_kind: Operation,
    session: Option<[u8; 16]>,
    operation: Option<[u8; 16]>,
) -> Option<String> {
    operation.or(session).map(encode_context).or_else(|| {
        (operation_kind == Operation::PairingPersist).then(|| "protocol-pairing".into())
    })
}

fn protocol_span_active(
    state: &ProtocolRuntime,
    operation_kind: Operation,
    session: Option<[u8; 16]>,
    operation: Option<[u8; 16]>,
) -> bool {
    protocol_diagnostic_key(operation_kind, session, operation).is_some_and(|key| {
        state
            .diagnostic_spans
            .lock()
            .is_ok_and(|spans| spans.contains_key(&key))
    })
}

#[allow(clippy::too_many_arguments)]
fn protocol_run(
    span: &ProtocolDiagnosticSpan,
    operation_kind: Operation,
    outcome: RunOutcome,
    error_type: Option<ErrorType>,
    session_context: Option<[u8; 16]>,
    operation_context: Option<[u8; 16]>,
    terminal: bool,
) -> DiagnosticRun {
    let terminal_status = match outcome {
        RunOutcome::Success => OperationStatus::Success,
        RunOutcome::Error => OperationStatus::Error,
        RunOutcome::Cancel => OperationStatus::Cancel,
        RunOutcome::Timeout => OperationStatus::Timeout,
    };
    let operations: &[Operation] = match operation_kind {
        Operation::ProtocolNegotiate => &[
            Operation::TransportConnect,
            Operation::NoiseHandshake,
            Operation::ProtocolNegotiate,
        ],
        Operation::AvailabilityRead => {
            &[Operation::AvailabilityRead, Operation::PresenterApplyView]
        }
        _ => std::slice::from_ref(&operation_kind),
    };
    let duration =
        terminal.then(|| u32::try_from(span.started_at.elapsed().as_millis()).unwrap_or(u32::MAX));
    let records = operations
        .iter()
        .enumerate()
        .map(|(index, operation)| {
            let last = index + 1 == operations.len();
            SemanticRecord {
                operation: *operation,
                operation_id: u16::try_from(index + 1).unwrap_or(u16::MAX),
                parent_operation_id: (index != 0).then_some(1),
                status: if terminal {
                    if last {
                        terminal_status
                    } else {
                        OperationStatus::Success
                    }
                } else {
                    OperationStatus::InProgress
                },
                error_type: (terminal && last).then_some(error_type).flatten(),
                effect_id: None,
                virtual_time_ms: 0,
                end_virtual_time_ms: duration.map_or(0, u64::from),
                duration_ms: duration,
                render_width: None,
                render_height: None,
                value: None,
            }
        })
        .collect();
    let session_context_id = session_context.map(encode_context);
    let operation_context_id = operation_context.map(encode_context);
    DiagnosticRun {
        schema_version: 1,
        resource: resource_identity_for(
            "deskkin-simulator",
            env!("CARGO_PKG_VERSION"),
            ResourceRole::DeviceSimulator,
        ),
        run_id: span.run_id.clone(),
        scenario_run_id: operation_context_id
            .clone()
            .or_else(|| session_context_id.clone())
            .unwrap_or_else(|| span.run_id.clone()),
        transaction_id: None,
        session_context_id,
        operation_context_id,
        protocol_major: Some(1),
        selected_features: Some(deskkin_protocol::AVAILABILITY_READ_V1.0),
        granted_permissions: Some(deskkin_protocol::AVAILABILITY_READ_PERMISSION.0),
        outcome,
        completeness: if terminal {
            Completeness::Complete
        } else {
            Completeness::Partial
        },
        health: RecordingHealth::Healthy,
        terminal,
        missing_reason: None,
        owner: None,
        retained: false,
        created_unix_ms: span.created_unix_ms,
        records,
    }
}

fn encode_context(value: [u8; 16]) -> String {
    let mut encoded = String::with_capacity(32);
    for byte in value {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn arm_protocol_refresh(
    runtime: &Rc<RefCell<ProtocolRuntime>>,
    effect: Effect,
    delay_ms: u32,
) -> Result<(), String> {
    let weak = Rc::downgrade(runtime);
    runtime.borrow().refresh_timer.start(
        TimerMode::SingleShot,
        Duration::from_millis(u64::from(delay_ms)),
        move || protocol_refresh_due(&weak, effect),
    );
    let running = runtime.borrow().refresh_timer.running();
    let result = if running {
        Ok(())
    } else {
        Err(TimerArmError::Unavailable)
    };
    let mut state = runtime.borrow_mut();
    let transition = state
        .core
        .transition(ApplicationInput::availability(
            effect.id,
            AvailabilityInput::TimerArmCompleted(TimerArmCompleted {
                effect_id: effect.id.local,
                result,
            }),
        ))
        .map_err(|error| format!("timer arm completion: {error:?}"))?;
    apply_view(&state.ui, transition.view);
    Ok(())
}

fn protocol_refresh_due(weak: &Weak<RefCell<ProtocolRuntime>>, timer: Effect) {
    let Some(runtime) = weak.upgrade() else {
        return;
    };
    let next = runtime
        .borrow_mut()
        .core
        .transition(ApplicationInput::availability(
            timer.id,
            AvailabilityInput::RefreshDue(RefreshDue {
                effect_id: timer.id.local,
            }),
        ))
        .ok()
        .and_then(|transition| transition.effects.get(0));
    if let Some(effect) = next {
        let _ = dispatch_protocol_effect(&runtime, effect);
    }
}

fn dispatch_effect(runtime: &Rc<RefCell<NativeRuntime>>, effect: Effect) -> Result<(), String> {
    match effect.request {
        EffectRequest::Availability(availability::EffectRequest::ReadAvailability) => {
            let started_ms = runtime.borrow().logical_time_ms;
            let run_id = new_run_id("refresh");
            {
                let state = runtime.borrow();
                let _ = state.recorder.publish(in_progress_run(
                    run_id.clone(),
                    state.session_run_id.clone(),
                    effect.id.local.get(),
                    started_ms,
                ));
            }
            runtime.borrow_mut().active_read = Some(ActiveRead {
                effect,
                run_id,
                started_ms,
                started_at: Instant::now(),
            });
            let weak = Rc::downgrade(runtime);
            runtime.borrow().read_timer.start(
                TimerMode::SingleShot,
                Duration::from_millis(250),
                move || complete_native_read(&weak),
            );
            let weak = Rc::downgrade(runtime);
            runtime.borrow().read_timeout_timer.start(
                TimerMode::SingleShot,
                Duration::from_secs(2),
                move || timeout_native_read(&weak),
            );
            Ok(())
        }
        EffectRequest::Availability(availability::EffectRequest::ArmRefreshTimer { delay_ms }) => {
            arm_native_timer(runtime, effect, delay_ms)
        }
        EffectRequest::SyntheticNotice(_) => Ok(()),
    }
}

fn complete_native_read(weak: &Weak<RefCell<NativeRuntime>>) {
    let Some(runtime) = weak.upgrade() else {
        return;
    };
    let result = {
        let mut state = runtime.borrow_mut();
        if state.active_read.is_none() {
            return;
        }
        let result = match state.read_index % 3 {
            0 => Ok(Availability::Available),
            1 => Ok(Availability::Unavailable),
            _ => Err(ReadError::Unavailable),
        };
        state.read_index += 1;
        result
    };
    finish_native_read(&runtime, result, None, 250);
}

fn timeout_native_read(weak: &Weak<RefCell<NativeRuntime>>) {
    let Some(runtime) = weak.upgrade() else {
        return;
    };
    finish_native_read(
        &runtime,
        Err(ReadError::Unavailable),
        Some(RunOutcome::Timeout),
        2_000,
    );
}

fn finish_native_read(
    runtime: &Rc<RefCell<NativeRuntime>>,
    result: Result<Availability, ReadError>,
    outcome_override: Option<RunOutcome>,
    elapsed_ms: u64,
) {
    let (effect, next_effect) = {
        let mut state = runtime.borrow_mut();
        let Some(active) = state.active_read.take() else {
            return;
        };
        state.read_timer.stop();
        state.read_timeout_timer.stop();
        state.logical_time_ms = active.started_ms.saturating_add(elapsed_ms);
        let Ok(transition) = state.core.transition(ApplicationInput::availability(
            active.effect.id,
            AvailabilityInput::ReadCompleted(ReadCompleted {
                effect_id: active.effect.id.local,
                result,
            }),
        )) else {
            return;
        };
        apply_view(&state.ui, transition.view);
        (active, transition.effects.get(0))
    };
    if let Some(timer) = next_effect {
        if arm_native_timer(runtime, timer, 5_000).is_err() {
            let mut state = runtime.borrow_mut();
            let _ = state.core.transition(ApplicationInput::availability(
                timer.id,
                AvailabilityInput::TimerArmCompleted(TimerArmCompleted {
                    effect_id: timer.id.local,
                    result: Err(TimerArmError::Unavailable),
                }),
            ));
            apply_view(&state.ui, state.core.view());
            publish_native_refresh(&state, &effect, timer, result, false, outcome_override);
        } else {
            publish_native_refresh(
                &runtime.borrow(),
                &effect,
                timer,
                result,
                true,
                outcome_override,
            );
        }
    }
}

fn arm_native_timer(
    runtime: &Rc<RefCell<NativeRuntime>>,
    effect: Effect,
    delay_ms: u32,
) -> Result<(), String> {
    let weak = Rc::downgrade(runtime);
    runtime.borrow().refresh_timer.start(
        TimerMode::SingleShot,
        Duration::from_millis(u64::from(delay_ms)),
        move || refresh_due(&weak, effect),
    );
    if !runtime.borrow().refresh_timer.running() {
        return Err("Slint refresh timer did not start".into());
    }
    runtime
        .borrow_mut()
        .core
        .transition(ApplicationInput::availability(
            effect.id,
            AvailabilityInput::TimerArmCompleted(TimerArmCompleted {
                effect_id: effect.id.local,
                result: Ok(()),
            }),
        ))
        .map_err(|error| format!("timer arm completion: {error:?}"))?;
    Ok(())
}

fn refresh_due(weak: &Weak<RefCell<NativeRuntime>>, timer: Effect) {
    let Some(runtime) = weak.upgrade() else {
        return;
    };
    let next_effect = {
        let mut state = runtime.borrow_mut();
        state.logical_time_ms = state.logical_time_ms.saturating_add(5_000);
        state
            .core
            .transition(ApplicationInput::availability(
                timer.id,
                AvailabilityInput::RefreshDue(RefreshDue {
                    effect_id: timer.id.local,
                }),
            ))
            .ok()
            .and_then(|transition| transition.effects.get(0))
    };
    if let Some(effect) = next_effect {
        let _ = dispatch_effect(&runtime, effect);
    }
}

fn publish_native_refresh(
    state: &NativeRuntime,
    active: &ActiveRead,
    timer: Effect,
    result: Result<Availability, ReadError>,
    arm_succeeded: bool,
    outcome_override: Option<RunOutcome>,
) {
    let (read_outcome, read_value, view_value) = match result {
        Ok(Availability::Available) => (
            RunOutcome::Success,
            ClosedValue::Available,
            ClosedValue::Available,
        ),
        Ok(Availability::Unavailable) => (
            RunOutcome::Success,
            ClosedValue::Unavailable,
            ClosedValue::Unavailable,
        ),
        Err(ReadError::Unavailable) => (
            RunOutcome::Error,
            outcome_override.map_or(ClosedValue::ReadFailed, |outcome| match outcome {
                RunOutcome::Timeout => ClosedValue::Timeout,
                RunOutcome::Cancel => ClosedValue::Cancel,
                RunOutcome::Error => ClosedValue::Error,
                RunOutcome::Success => ClosedValue::Success,
            }),
            ClosedValue::Unknown,
        ),
    };
    let outcome = if arm_succeeded {
        outcome_override.unwrap_or(read_outcome)
    } else {
        RunOutcome::Error
    };
    let records = native_refresh_records(
        active.started_ms,
        state.logical_time_ms,
        active.effect,
        timer,
        read_value,
        view_value,
        arm_succeeded,
    );
    let _ = state.recorder.publish(DiagnosticRun {
        schema_version: 1,
        resource: resource_identity(),
        run_id: active.run_id.clone(),
        scenario_run_id: state.session_run_id.clone(),
        transaction_id: None,
        session_context_id: None,
        operation_context_id: None,
        protocol_major: None,
        selected_features: None,
        granted_permissions: None,
        outcome,
        completeness: Completeness::Complete,
        health: RecordingHealth::Healthy,
        terminal: true,
        missing_reason: None,
        owner: None,
        retained: false,
        created_unix_ms: now_unix_ms(),
        records,
    });
}

fn native_refresh_records(
    started_ms: u64,
    completed_ms: u64,
    read: Effect,
    timer: Effect,
    read_value: ClosedValue,
    view_value: ClosedValue,
    arm_succeeded: bool,
) -> Vec<SemanticRecord> {
    let mut records = vec![
        native_record(
            Operation::StatusRefresh,
            None,
            started_ms,
            ClosedValue::Started,
        ),
        native_record(
            Operation::EffectReadStatus,
            Some(read.id.local.get()),
            completed_ms,
            read_value,
        ),
        native_record(
            Operation::CoreTransition,
            Some(read.id.local.get()),
            completed_ms,
            view_value,
        ),
        SemanticRecord {
            operation: Operation::PresenterApplyView,
            operation_id: 0,
            parent_operation_id: None,
            status: crate::diagnostics::OperationStatus::Success,
            error_type: None,
            effect_id: None,
            virtual_time_ms: completed_ms,
            end_virtual_time_ms: completed_ms,
            duration_ms: None,
            render_width: Some(320),
            render_height: Some(240),
            value: Some(view_value),
        },
        SemanticRecord {
            operation: Operation::EffectArmRefreshTimer,
            operation_id: 0,
            parent_operation_id: None,
            status: crate::diagnostics::OperationStatus::Success,
            error_type: None,
            effect_id: Some(timer.id.local.get()),
            virtual_time_ms: completed_ms,
            end_virtual_time_ms: completed_ms,
            duration_ms: Some(5_000),
            render_width: None,
            render_height: None,
            value: Some(if arm_succeeded {
                ClosedValue::Armed
            } else {
                ClosedValue::ArmFailed
            }),
        },
    ];
    if !arm_succeeded {
        records.push(native_record(
            Operation::CoreTransition,
            Some(timer.id.local.get()),
            completed_ms,
            ClosedValue::Unknown,
        ));
        records.push(SemanticRecord {
            operation: Operation::PresenterApplyView,
            operation_id: 0,
            parent_operation_id: None,
            status: crate::diagnostics::OperationStatus::Success,
            error_type: None,
            effect_id: None,
            virtual_time_ms: completed_ms,
            end_virtual_time_ms: completed_ms,
            duration_ms: None,
            render_width: Some(320),
            render_height: Some(240),
            value: Some(ClosedValue::Unknown),
        });
    }
    let elapsed = u32::try_from(completed_ms.saturating_sub(started_ms)).ok();
    for record in &mut records {
        if matches!(
            record.operation,
            Operation::StatusRefresh | Operation::EffectReadStatus
        ) {
            record.duration_ms = elapsed;
        }
    }
    finalize_operation_records(
        &mut records,
        if arm_succeeded {
            match read_value {
                ClosedValue::Available | ClosedValue::Unavailable => RunOutcome::Success,
                _ => RunOutcome::Error,
            }
        } else {
            RunOutcome::Error
        },
    );
    records
}

fn publish_interrupted_refresh(
    recorder: &Recorder,
    session_run_id: &str,
    active: ActiveRead,
    completed_ms: u64,
    outcome: RunOutcome,
) -> crate::diagnostics::Publication {
    let value = match outcome {
        RunOutcome::Cancel => ClosedValue::Cancel,
        RunOutcome::Timeout => ClosedValue::Timeout,
        RunOutcome::Success => ClosedValue::Success,
        RunOutcome::Error => ClosedValue::Error,
    };
    recorder.publish(DiagnosticRun {
        schema_version: 1,
        resource: resource_identity(),
        run_id: active.run_id,
        scenario_run_id: session_run_id.into(),
        transaction_id: None,
        session_context_id: None,
        operation_context_id: None,
        protocol_major: None,
        selected_features: None,
        granted_permissions: None,
        outcome,
        completeness: Completeness::Complete,
        health: RecordingHealth::Healthy,
        terminal: true,
        missing_reason: None,
        owner: None,
        retained: false,
        created_unix_ms: now_unix_ms(),
        records: interrupted_records(active.started_ms, completed_ms, active.effect, value),
    })
}

fn interrupted_records(
    started_ms: u64,
    completed_ms: u64,
    read: Effect,
    value: ClosedValue,
) -> Vec<SemanticRecord> {
    let mut records = vec![
        native_record(
            Operation::StatusRefresh,
            None,
            started_ms,
            ClosedValue::Started,
        ),
        native_record(
            Operation::EffectReadStatus,
            Some(read.id.local.get()),
            completed_ms,
            value,
        ),
    ];
    let elapsed = u32::try_from(completed_ms.saturating_sub(started_ms)).ok();
    for record in &mut records {
        record.duration_ms = elapsed;
    }
    let outcome = match value {
        ClosedValue::Cancel => RunOutcome::Cancel,
        ClosedValue::Timeout => RunOutcome::Timeout,
        _ => RunOutcome::Error,
    };
    finalize_operation_records(&mut records, outcome);
    records
}

fn native_record(
    operation: Operation,
    effect_id: Option<u64>,
    virtual_time_ms: u64,
    value: ClosedValue,
) -> SemanticRecord {
    SemanticRecord {
        operation,
        operation_id: 0,
        parent_operation_id: None,
        status: crate::diagnostics::OperationStatus::Success,
        error_type: None,
        effect_id,
        virtual_time_ms,
        end_virtual_time_ms: virtual_time_ms,
        duration_ms: None,
        render_width: None,
        render_height: None,
        value: Some(value),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};

    use super::*;

    #[test]
    fn durable_pairing_with_terminal_hello_reject_does_not_reconnect() {
        let mut adapter = ProtocolAdapter::new();
        assert!(!apply_pairing_outcome(
            &mut adapter,
            Err(SessionError::Incompatible),
        ));
        assert!(adapter.terminal_reason().is_some());

        let mut adapter = ProtocolAdapter::new();
        assert!(!apply_pairing_outcome(
            &mut adapter,
            Err(SessionError::AuthorizationDenied),
        ));
        assert!(adapter.terminal_reason().is_some());
    }

    #[test]
    fn network_worker_serializes_authenticated_connect_read_and_close() {
        let base = std::env::temp_dir().join(new_run_id("network-worker"));
        let host = IdentityStore::new_for_role(base.join("host/identity"), ResourceRole::Host);
        let simulator = IdentityStore::new_for_role(
            base.join("simulator/identity"),
            ResourceRole::DeviceSimulator,
        );
        host.init().unwrap();
        simulator.init().unwrap();
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let host_store = host.clone();
        let host_thread = thread::spawn(move || {
            deskkin_desktop_host::pair_responder(&listener, &host_store, |_, _| true).unwrap();
            deskkin_desktop_host::serve_one(
                &listener,
                &host_store,
                deskkin_protocol::AvailabilityResult::Available,
            )
            .unwrap();
        });
        deskkin_desktop_host::pair_initiator(address, &simulator, [1; 16], |_, _| true).unwrap();

        let (commands, command_receiver) = std::sync::mpsc::sync_channel(8);
        let (control, control_receiver) = std::sync::mpsc::channel();
        let (event_sender, events) = std::sync::mpsc::channel();
        let worker = thread::spawn(move || {
            run_network_worker(
                address,
                &simulator,
                &command_receiver,
                &control_receiver,
                &event_sender,
            );
        });
        commands
            .send(NetworkCommand::Connect { session: [2; 16] })
            .unwrap();
        let NetworkEvent::Connected { session, result } =
            events.recv_timeout(Duration::from_secs(3)).unwrap()
        else {
            panic!("expected connected event");
        };
        assert_eq!(session, [2; 16]);
        assert!(result.is_ok());
        commands
            .send(NetworkCommand::Read {
                session: [2; 16],
                request_id: 1,
                operation: [3; 16],
            })
            .unwrap();
        let NetworkEvent::ReadCompleted {
            session,
            request_id,
            operation,
            result,
            ..
        } = events.recv_timeout(Duration::from_secs(3)).unwrap()
        else {
            panic!("expected read event");
        };
        assert_eq!(session, [2; 16]);
        assert_eq!(request_id, 1);
        assert_eq!(operation, [3; 16]);
        assert_eq!(
            result.unwrap(),
            deskkin_protocol::AvailabilityResult::Available
        );
        control.send(NetworkControl::Close).unwrap();
        assert!(matches!(
            events.recv_timeout(Duration::from_secs(3)).unwrap(),
            NetworkEvent::Closed
        ));
        control.send(NetworkControl::Shutdown).unwrap();
        assert!(matches!(
            events.recv_timeout(Duration::from_secs(3)).unwrap(),
            NetworkEvent::WorkerStopped
        ));
        worker.join().unwrap();
        host_thread.join().unwrap();
    }

    fn effects() -> (Effect, Effect) {
        let mut application = Application::new();
        let read = application
            .transition(ApplicationInput::Lifecycle(Lifecycle::Start))
            .unwrap()
            .effects
            .get(0)
            .unwrap();
        let timer = application
            .transition(ApplicationInput::availability(
                read.id,
                AvailabilityInput::ReadCompleted(ReadCompleted {
                    effect_id: read.id.local,
                    result: Ok(Availability::Available),
                }),
            ))
            .unwrap()
            .effects
            .get(0)
            .unwrap();
        (read, timer)
    }

    #[test]
    fn arm_failure_records_terminal_unknown_view() {
        let (read, timer) = effects();
        let records = native_refresh_records(
            0,
            250,
            read,
            timer,
            ClosedValue::Available,
            ClosedValue::Available,
            false,
        );
        assert!(records.iter().any(|record| {
            record.operation == Operation::EffectArmRefreshTimer
                && record.value == Some(ClosedValue::ArmFailed)
        }));
        assert!(records.iter().any(|record| {
            record.operation == Operation::PresenterApplyView
                && record.value == Some(ClosedValue::Unknown)
        }));
    }

    #[test]
    fn read_failure_records_failed_effect_and_unknown_view() {
        let (read, timer) = effects();
        let records = native_refresh_records(
            0,
            250,
            read,
            timer,
            ClosedValue::ReadFailed,
            ClosedValue::Unknown,
            true,
        );
        assert!(records.iter().any(|record| {
            record.operation == Operation::EffectReadStatus
                && record.value == Some(ClosedValue::ReadFailed)
        }));
        assert!(records.iter().any(|record| {
            record.operation == Operation::PresenterApplyView
                && record.value == Some(ClosedValue::Unknown)
        }));
    }

    #[test]
    fn cancel_and_timeout_terminal_records_are_distinct() {
        let (read, _) = effects();
        let cancel = interrupted_records(0, 125, read, ClosedValue::Cancel);
        let timeout = interrupted_records(0, 2_000, read, ClosedValue::Timeout);
        assert_ne!(cancel, timeout);
        assert_eq!(cancel.last().unwrap().value, Some(ClosedValue::Cancel));
        assert_eq!(timeout.last().unwrap().value, Some(ClosedValue::Timeout));
    }

    #[test]
    fn cancel_and_timeout_publish_correlated_terminal_runs() {
        let (read, _) = effects();
        for (outcome, completed_ms, expected) in [
            (RunOutcome::Cancel, 125, ClosedValue::Cancel),
            (RunOutcome::Timeout, 2_000, ClosedValue::Timeout),
        ] {
            let root = std::env::temp_dir().join(new_run_id("runtime-publication"));
            let recorder = Recorder::at_root(root.clone());
            let active = ActiveRead {
                effect: read,
                run_id: new_run_id("refresh-test"),
                started_ms: 0,
                started_at: Instant::now(),
            };
            let publication = publish_interrupted_refresh(
                &recorder,
                "native-session",
                active,
                completed_ms,
                outcome,
            );
            assert_eq!(publication.completeness, Completeness::Complete);
            assert_eq!(publication.health, RecordingHealth::Healthy);
            assert!(publication.stored);
            let saved: DiagnosticRun = serde_json::from_slice(
                &fs::read(root.join(format!("diagnostics/{}.json", publication.run_id))).unwrap(),
            )
            .unwrap();
            assert_eq!(saved.scenario_run_id, "native-session");
            assert_eq!(saved.outcome, outcome);
            assert_eq!(saved.resource, resource_identity());
            assert_eq!(saved.records[0].virtual_time_ms, 0);
            assert_eq!(
                saved.records[0].duration_ms,
                u32::try_from(completed_ms).ok()
            );
            assert_eq!(saved.records[1].virtual_time_ms, 0);
            assert_eq!(saved.records[1].end_virtual_time_ms, completed_ms);
            assert_eq!(saved.records[1].value, Some(expected));
        }
    }
}
