use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::{DiagnosticKind, DiagnosticRecord, DiagnosticSpan, IdentityActor, PeerState};

const PAYLOAD_MAX: usize = 4 * 1024;
const TABLE_MAX: usize = 16;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", tag = "command", deny_unknown_fields)]
pub enum OwnerCommand {
    OwnerInfo,
    IdentityInit {
        command_id: String,
        owner_generation: String,
    },
    IdentityList,
    Unpair {
        command_id: String,
        owner_generation: String,
        peer_id: String,
    },
    PairingWindowOpen {
        command_id: String,
        owner_generation: String,
    },
    PairStart {
        command_id: String,
        owner_generation: String,
        loopback_address: String,
    },
    PairingDecide {
        command_id: String,
        owner_generation: String,
        parent_command_id: String,
        pairing_transaction_id: String,
        confirmed: bool,
    },
    CommandResult {
        command_id: String,
        owner_generation: String,
    },
    Shutdown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "result")]
pub enum OwnerResponse {
    OwnerInfo {
        owner_generation: String,
    },
    IdentityInitialized,
    IdentityState {
        peer: PeerState,
    },
    Unpaired,
    PairingConfirmationRequired {
        pairing_transaction_id: String,
        authentication_string: String,
    },
    PairingDecisionAccepted,
    PairingWaiting,
    Paired,
    CommandUnknown,
    CommandAccepted,
    CommandPending,
    StaleOwner,
    InvalidRequest,
    OwnerBusy,
    OperationFailed,
    ShutdownAccepted,
}

pub enum OwnerEvent {
    IdentityRevoked {
        joined: mpsc::SyncSender<()>,
    },
    PairingWindowOpen {
        task: OwnerPairingTask,
    },
    PairStart {
        address: std::net::SocketAddr,
        task: OwnerPairingTask,
    },
    RuntimeShutdown {
        joined: mpsc::SyncSender<()>,
    },
}

#[derive(Clone)]
pub struct OwnerPairingTask {
    command_id: String,
    updates: mpsc::Sender<(String, OwnerResponse, Instant)>,
    decisions: Arc<Mutex<mpsc::Receiver<(String, bool)>>>,
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
}

impl OwnerPairingTask {
    pub fn waiting(&self) {
        let _ = self.updates.send((
            self.command_id.clone(),
            OwnerResponse::PairingWaiting,
            Instant::now(),
        ));
    }
    #[must_use]
    pub fn expired(&self) -> bool {
        Instant::now() >= self.deadline
    }
    #[must_use]
    pub fn deadline(&self) -> Instant {
        self.deadline
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    #[must_use]
    pub fn confirm(&self, transaction: [u8; 16], authentication: [u8; 6]) -> bool {
        let transaction = hex_bytes(&transaction);
        if self.deadline <= Instant::now() {
            return false;
        }
        if self
            .updates
            .send((
                self.command_id.clone(),
                OwnerResponse::PairingConfirmationRequired {
                    pairing_transaction_id: transaction.clone(),
                    authentication_string: String::from_utf8_lossy(&authentication).into_owned(),
                },
                Instant::now(),
            ))
            .is_err()
        {
            return false;
        }
        let Ok(decisions) = self.decisions.lock() else {
            return false;
        };
        loop {
            if self.cancelled.load(Ordering::Acquire) {
                return false;
            }
            let Some(remaining) = self.deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            match decisions.recv_timeout(remaining.min(Duration::from_millis(50))) {
                Ok((received, confirmed)) => {
                    return received == transaction && confirmed;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return false,
            }
        }
    }

    pub fn finish(self, success: bool) {
        let _ = self.updates.send((
            self.command_id,
            if success {
                OwnerResponse::Paired
            } else {
                OwnerResponse::OperationFailed
            },
            Instant::now(),
        ));
    }
}

struct CommandRecord {
    id: String,
    response: Option<OwnerResponse>,
    completed_at: Option<Instant>,
    decision: Option<mpsc::SyncSender<(String, bool)>>,
    decision_results: Vec<(String, OwnerResponse)>,
    diagnostic: Option<(DiagnosticKind, DiagnosticSpan)>,
}

pub fn run_owner_control(
    root: &Path,
    actor: &IdentityActor,
    owner_generation: &str,
) -> std::io::Result<()> {
    run_owner_control_with_events(root, actor, owner_generation, None)
}

pub fn run_owner_control_with_events(
    root: &Path,
    actor: &IdentityActor,
    owner_generation: &str,
    event_sender: Option<mpsc::Sender<OwnerEvent>>,
) -> std::io::Result<()> {
    let _owner_lock = acquire_owner_lock(root)?;
    let socket = root.join("owner.sock");
    if socket.exists() {
        let metadata = fs::symlink_metadata(&socket)?;
        if metadata.file_type().is_symlink() {
            return Err(std::io::Error::other("owner socket symlink"));
        }
        if !metadata.file_type().is_socket() {
            return Err(std::io::Error::other("owner socket path is not a socket"));
        }
        fs::remove_file(&socket)?;
    }
    let listener = UnixListener::bind(&socket)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let (completion_sender, completion_receiver) = mpsc::channel();
    let state = Arc::new(Mutex::new(ControlState {
        table: VecDeque::new(),
        completion_sender,
        completion_receiver,
        workers: Vec::new(),
        event_sender,
    }));
    let shutdown = Arc::new(AtomicBool::new(false));
    let generation = Arc::new(owner_generation.to_owned());
    let mut connections: Vec<JoinHandle<()>> = Vec::new();
    while !shutdown.load(Ordering::Acquire) {
        let mut index = 0;
        while index < connections.len() {
            if connections[index].is_finished() {
                let connection = connections.swap_remove(index);
                let _ = connection.join();
            } else {
                index += 1;
            }
        }
        match listener.accept() {
            Ok((mut stream, _)) if connections.len() < 4 => {
                let state = state.clone();
                let shutdown = shutdown.clone();
                let actor = actor.clone();
                let generation = generation.clone();
                connections.push(std::thread::spawn(move || {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                    let command = read_command(&mut stream);
                    let (response, requests_shutdown) = match command {
                        Ok(command) => handle_command(command, &actor, &generation, &state),
                        Err(()) => (OwnerResponse::InvalidRequest, false),
                    };
                    if write_response(&mut stream, &response).is_ok() && requests_shutdown {
                        shutdown.store(true, Ordering::Release);
                    }
                }));
            }
            Ok((_stream, _)) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(error) => return Err(error),
        }
    }
    for connection in connections {
        let _ = connection.join();
    }
    let mut state = state
        .lock()
        .map_err(|_| std::io::Error::other("control state"))?;
    for worker in state.workers.drain(..) {
        let _ = worker.join();
    }
    apply_completions(&mut state);
    if let Some(event_sender) = &state.event_sender {
        let (joined, acknowledgement) = mpsc::sync_channel(1);
        event_sender
            .send(OwnerEvent::RuntimeShutdown { joined })
            .map_err(|_| std::io::Error::other("runtime shutdown coordinator unavailable"))?;
        acknowledgement
            .recv()
            .map_err(|_| std::io::Error::other("runtime shutdown was not joined"))?;
    }
    actor
        .stop_and_join()
        .map_err(|_| std::io::Error::other("identity actor shutdown failed"))?;
    let _ = fs::remove_file(socket);
    Ok(())
}

struct ControlState {
    table: VecDeque<CommandRecord>,
    completion_sender: mpsc::Sender<(String, OwnerResponse, Instant)>,
    completion_receiver: mpsc::Receiver<(String, OwnerResponse, Instant)>,
    workers: Vec<JoinHandle<()>>,
    event_sender: Option<mpsc::Sender<OwnerEvent>>,
}

#[allow(clippy::too_many_lines)]
fn handle_command(
    command: OwnerCommand,
    actor: &IdentityActor,
    owner_generation: &str,
    state: &Mutex<ControlState>,
) -> (OwnerResponse, bool) {
    let Ok(mut state) = state.lock() else {
        return (OwnerResponse::OperationFailed, false);
    };
    let mut worker = 0;
    while worker < state.workers.len() {
        if state.workers[worker].is_finished() {
            let finished = state.workers.swap_remove(worker);
            let _ = finished.join();
        } else {
            worker += 1;
        }
    }
    apply_completions(&mut state);
    expire_terminal(&mut state.table);
    match command {
        OwnerCommand::OwnerInfo => (
            OwnerResponse::OwnerInfo {
                owner_generation: owner_generation.to_owned(),
            },
            false,
        ),
        OwnerCommand::IdentityList => (
            actor.peer().map_or(OwnerResponse::OperationFailed, |peer| {
                OwnerResponse::IdentityState { peer }
            }),
            false,
        ),
        OwnerCommand::CommandResult {
            command_id,
            owner_generation: generation,
        } if generation == owner_generation => (
            state
                .table
                .iter()
                .find_map(|record| {
                    if record.id == command_id {
                        Some(
                            record
                                .response
                                .clone()
                                .unwrap_or(OwnerResponse::CommandPending),
                        )
                    } else {
                        record.decision_results.iter().find_map(|(id, response)| {
                            (id == &command_id).then(|| response.clone())
                        })
                    }
                })
                .unwrap_or(OwnerResponse::CommandUnknown),
            false,
        ),
        OwnerCommand::Shutdown => (OwnerResponse::ShutdownAccepted, true),
        OwnerCommand::IdentityInit {
            command_id,
            owner_generation: generation,
        } if generation == owner_generation => {
            let sender = state.completion_sender.clone();
            let actor = actor.clone();
            let diagnostic_actor = actor.clone();
            let ControlState { table, workers, .. } = &mut *state;
            accept(
                table,
                workers,
                &sender,
                command_id,
                Some((DiagnosticKind::IdentityInit, &diagnostic_actor)),
                move || {
                    actor
                        .init_for_owner()
                        .map(|_| OwnerResponse::IdentityInitialized)
                },
            )
        }
        OwnerCommand::Unpair {
            command_id,
            owner_generation: generation,
            peer_id,
        } if generation == owner_generation => {
            let sender = state.completion_sender.clone();
            let event_sender = state.event_sender.clone();
            let actor = actor.clone();
            let diagnostic_actor = actor.clone();
            let ControlState { table, workers, .. } = &mut *state;
            accept(
                table,
                workers,
                &sender,
                command_id,
                Some((DiagnosticKind::IdentityUnpair, &diagnostic_actor)),
                move || {
                    actor.unpair_with_revocation(peer_id, move || {
                        if let Some(event_sender) = event_sender {
                            let (joined, acknowledgement) = mpsc::sync_channel(1);
                            let _ = event_sender.send(OwnerEvent::IdentityRevoked { joined });
                            return Box::new(move || {
                                acknowledgement.recv_timeout(Duration::from_secs(2)).is_ok()
                            });
                        }
                        Box::new(|| true)
                    })?;
                    Ok(OwnerResponse::Unpaired)
                },
            )
        }
        OwnerCommand::PairingWindowOpen {
            command_id,
            owner_generation: generation,
        } if generation == owner_generation => {
            accept_pairing_event(&mut state, command_id, |task| {
                OwnerEvent::PairingWindowOpen { task }
            })
        }
        OwnerCommand::PairStart {
            command_id,
            owner_generation: generation,
            loopback_address,
        } if generation == owner_generation => {
            let Ok(address) = loopback_address.parse::<std::net::SocketAddr>() else {
                return (OwnerResponse::InvalidRequest, false);
            };
            if !address.ip().is_loopback() {
                return (OwnerResponse::InvalidRequest, false);
            }
            accept_pairing_event(&mut state, command_id, |task| OwnerEvent::PairStart {
                address,
                task,
            })
        }
        OwnerCommand::PairingDecide {
            command_id,
            owner_generation: generation,
            parent_command_id,
            pairing_transaction_id,
            confirmed,
        } if generation == owner_generation => accept_pairing_decision(
            &mut state,
            command_id,
            &parent_command_id,
            pairing_transaction_id,
            confirmed,
        ),
        OwnerCommand::CommandResult { .. }
        | OwnerCommand::IdentityInit { .. }
        | OwnerCommand::Unpair { .. }
        | OwnerCommand::PairingWindowOpen { .. }
        | OwnerCommand::PairStart { .. }
        | OwnerCommand::PairingDecide { .. } => (OwnerResponse::StaleOwner, false),
    }
}

fn apply_completions(state: &mut ControlState) {
    let completions: Vec<_> = state.completion_receiver.try_iter().collect();
    for (id, response, completed_at) in completions {
        if let Some(record) = state.table.iter_mut().find(|record| record.id == id) {
            let terminal = !matches!(
                response,
                OwnerResponse::PairingConfirmationRequired { .. } | OwnerResponse::PairingWaiting
            );
            record.response = Some(response);
            record.completed_at = terminal.then_some(completed_at);
            if terminal && let Some((kind, diagnostic)) = record.diagnostic.take() {
                let success = matches!(
                    record.response,
                    Some(OwnerResponse::IdentityInitialized | OwnerResponse::Unpaired)
                );
                diagnostic.finish(DiagnosticRecord::identity(
                    kind,
                    if success { "success" } else { "error" },
                ));
            }
        }
    }
}

fn accept_pairing_event(
    state: &mut ControlState,
    command_id: String,
    event: impl FnOnce(OwnerPairingTask) -> OwnerEvent,
) -> (OwnerResponse, bool) {
    if !valid_id(&command_id) {
        return (OwnerResponse::InvalidRequest, false);
    }
    if let Some(record) = state.table.iter().find(|record| record.id == command_id) {
        return (
            record
                .response
                .clone()
                .unwrap_or(OwnerResponse::CommandPending),
            false,
        );
    }
    if state.table.len() >= TABLE_MAX {
        return (OwnerResponse::OwnerBusy, false);
    }
    let Some(event_sender) = state.event_sender.clone() else {
        return (OwnerResponse::OperationFailed, false);
    };
    let (decision, decisions) = mpsc::sync_channel(1);
    let task = OwnerPairingTask {
        command_id: command_id.clone(),
        updates: state.completion_sender.clone(),
        decisions: Arc::new(Mutex::new(decisions)),
        deadline: Instant::now() + Duration::from_mins(1),
        cancelled: Arc::new(AtomicBool::new(false)),
    };
    state.table.push_back(CommandRecord {
        id: command_id,
        response: None,
        completed_at: None,
        decision: Some(decision),
        decision_results: Vec::new(),
        diagnostic: None,
    });
    if event_sender.send(event(task)).is_err() {
        if let Some(record) = state.table.back_mut() {
            record.response = Some(OwnerResponse::OperationFailed);
            record.completed_at = Some(Instant::now());
            record.decision = None;
        }
        return (OwnerResponse::OperationFailed, false);
    }
    (OwnerResponse::CommandAccepted, false)
}

fn accept_pairing_decision(
    state: &mut ControlState,
    command_id: String,
    parent_command_id: &str,
    transaction: String,
    confirmed: bool,
) -> (OwnerResponse, bool) {
    if !valid_id(&command_id) || !valid_id(parent_command_id) || !valid_id(&transaction) {
        return (OwnerResponse::InvalidRequest, false);
    }
    if let Some(response) = state.table.iter().find_map(|record| {
        record
            .decision_results
            .iter()
            .find_map(|(id, response)| (id == &command_id).then(|| response.clone()))
    }) {
        return (response, false);
    }
    let Some(parent) = state
        .table
        .iter_mut()
        .find(|record| record.id == parent_command_id)
    else {
        return (OwnerResponse::InvalidRequest, false);
    };
    if !parent.decision_results.is_empty() {
        return (OwnerResponse::InvalidRequest, false);
    }
    let prompt_matches = matches!(
        &parent.response,
        Some(OwnerResponse::PairingConfirmationRequired {
            pairing_transaction_id,
            ..
        }) if pairing_transaction_id == &transaction
    );
    if !prompt_matches {
        return (OwnerResponse::InvalidRequest, false);
    }
    let accepted = parent
        .decision
        .as_ref()
        .is_some_and(|decision| decision.send((transaction, confirmed)).is_ok());
    let response = if accepted {
        OwnerResponse::PairingDecisionAccepted
    } else {
        OwnerResponse::InvalidRequest
    };
    parent.decision_results.push((command_id, response.clone()));
    (response, false)
}

fn accept(
    operation_table: &mut VecDeque<CommandRecord>,
    workers: &mut Vec<JoinHandle<()>>,
    completion_sender: &mpsc::Sender<(String, OwnerResponse, Instant)>,
    command_id: String,
    diagnostic: Option<(DiagnosticKind, &IdentityActor)>,
    operation: impl FnOnce() -> Result<OwnerResponse, crate::StoreError> + Send + 'static,
) -> (OwnerResponse, bool) {
    if !valid_id(&command_id) {
        return (OwnerResponse::InvalidRequest, false);
    }
    if let Some(record) = operation_table
        .iter()
        .find(|record| record.id == command_id)
    {
        return (
            record
                .response
                .clone()
                .unwrap_or(OwnerResponse::CommandPending),
            false,
        );
    }
    if operation_table.len() >= TABLE_MAX {
        return (OwnerResponse::OwnerBusy, false);
    }
    let diagnostic =
        diagnostic.and_then(|(kind, actor)| actor.start_diagnostic(kind).map(|span| (kind, span)));
    operation_table.push_back(CommandRecord {
        id: command_id.clone(),
        response: None,
        completed_at: None,
        decision: None,
        decision_results: Vec::new(),
        diagnostic,
    });
    let sender = completion_sender.clone();
    workers.push(std::thread::spawn(move || {
        let response = operation().unwrap_or(OwnerResponse::OperationFailed);
        let _ = sender.send((command_id, response, Instant::now()));
    }));
    (OwnerResponse::CommandAccepted, false)
}

fn expire_terminal(table: &mut VecDeque<CommandRecord>) {
    table.retain(|record| {
        record
            .completed_at
            .is_none_or(|completed| completed.elapsed() < Duration::from_mins(10))
    });
}
fn read_command(stream: &mut UnixStream) -> Result<OwnerCommand, ()> {
    let mut prefix = [0; 4];
    read_exact_until(stream, &mut prefix, Instant::now() + Duration::from_secs(2))
        .map_err(|_| ())?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length > PAYLOAD_MAX {
        return Err(());
    }
    let mut payload = vec![0; length];
    read_exact_until(
        stream,
        &mut payload,
        Instant::now() + Duration::from_secs(2),
    )
    .map_err(|_| ())?;
    serde_json::from_slice(&payload).map_err(|_| ())
}
fn write_response(stream: &mut UnixStream, response: &OwnerResponse) -> std::io::Result<()> {
    let payload = serde_json::to_vec(response).map_err(std::io::Error::other)?;
    let length = u32::try_from(payload.len()).map_err(std::io::Error::other)?;
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(&payload);
    write_all_until(stream, &frame, Instant::now() + Duration::from_secs(2))
}
pub fn acquire_owner_lock(root: &Path) -> std::io::Result<File> {
    if !root.exists() {
        fs::create_dir_all(root)?;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    }
    let metadata = fs::symlink_metadata(root)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(std::io::Error::other("control root is not private"));
    }
    let lock = root.join("owner.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(lock)?;
    let started = std::time::Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock)
                if started.elapsed() < Duration::from_secs(2) =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(file)
}

/// Returns whether a runtime currently owns the control lock without waiting.
pub fn owner_is_alive(root: &Path) -> std::io::Result<bool> {
    Ok(try_acquire_owner_lock(root)?.is_none())
}

/// Attempts to acquire the owner lock without waiting.
pub fn try_acquire_owner_lock(root: &Path) -> std::io::Result<Option<File>> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(root.join("owner.lock"))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn call_owner_control(root: &Path, command: &OwnerCommand) -> std::io::Result<OwnerResponse> {
    let mut stream = UnixStream::connect(root.join("owner.sock"))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let payload = serde_json::to_vec(command).map_err(std::io::Error::other)?;
    if payload.len() > PAYLOAD_MAX {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "owner request exceeds limit",
        ));
    }
    let length = u32::try_from(payload.len()).map_err(std::io::Error::other)?;
    write_all_until(
        &mut stream,
        &length.to_be_bytes(),
        Instant::now() + Duration::from_secs(2),
    )?;
    write_all_until(
        &mut stream,
        &payload,
        Instant::now() + Duration::from_secs(2),
    )?;
    let mut prefix = [0; 4];
    read_exact_until(
        &mut stream,
        &mut prefix,
        Instant::now() + Duration::from_secs(2),
    )?;
    let response_length = u32::from_be_bytes(prefix) as usize;
    if response_length > PAYLOAD_MAX {
        return Err(std::io::Error::other("owner response exceeds limit"));
    }
    let mut response = vec![0; response_length];
    read_exact_until(
        &mut stream,
        &mut response,
        Instant::now() + Duration::from_secs(2),
    )?;
    serde_json::from_slice(&response).map_err(std::io::Error::other)
}

/// Discovers the live owner generation, cleaning a stale socket only while
/// holding the unowned lock.
pub fn discover_owner(root: &Path) -> std::io::Result<Option<String>> {
    if !root.join("owner.sock").exists() {
        return Ok(None);
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match call_owner_control(root, &OwnerCommand::OwnerInfo) {
            Ok(OwnerResponse::OwnerInfo { owner_generation }) => {
                return Ok(Some(owner_generation));
            }
            Ok(_) => return Err(std::io::Error::other("unexpected owner_info response")),
            Err(error) => {
                if let Some(_owner_lock) = try_acquire_owner_lock(root)? {
                    let _ = fs::remove_file(root.join("owner.sock"));
                    return Ok(None);
                }
                if !owner_is_alive(root)? {
                    return Err(error);
                }
                if Instant::now() >= deadline {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WouldBlock,
                        "owner_busy",
                    ));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

/// Queries one accepted command without replaying it. Transient control
/// connection loss is retried only while the same owner lock remains held.
pub fn query_command_result(
    root: &Path,
    command_id: &str,
    owner_generation: &str,
) -> std::io::Result<OwnerResponse> {
    loop {
        match call_owner_control(
            root,
            &OwnerCommand::CommandResult {
                command_id: command_id.into(),
                owner_generation: owner_generation.into(),
            },
        ) {
            Ok(OwnerResponse::StaleOwner) => {
                return Err(std::io::Error::other("owner_lost_result_unknown"));
            }
            Ok(response) => return Ok(response),
            Err(_) if owner_is_alive(root)? => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return Err(std::io::Error::other("owner_lost_result_unknown")),
        }
    }
}

fn read_exact_until(
    stream: &mut UnixStream,
    mut buffer: &mut [u8],
    deadline: Instant,
) -> std::io::Result<()> {
    while !buffer.is_empty() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::TimedOut, "read deadline"))?;
        stream.set_read_timeout(Some(remaining))?;
        match stream.read(buffer) {
            Ok(0) => return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof)),
            Ok(read) => buffer = &mut buffer[read..],
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn write_all_until(
    stream: &mut UnixStream,
    mut buffer: &[u8],
    deadline: Instant,
) -> std::io::Result<()> {
    while !buffer.is_empty() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::TimedOut, "write deadline"))?;
        stream.set_write_timeout(Some(remaining))?;
        match stream.write(buffer) {
            Ok(0) => return Err(std::io::Error::from(std::io::ErrorKind::WriteZero)),
            Ok(written) => buffer = &buffer[written..],
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
fn valid_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn hex_bytes(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IdentityStore;
    use std::os::unix::net::UnixStream;
    use std::thread;
    #[test]
    fn closed_command_ids_are_exact() {
        assert!(valid_id("00112233445566778899aabbccddeeff"));
        assert!(!valid_id("../00112233445566778899aabbccddeeff"));
    }

    #[test]
    fn absolute_read_deadline_rejects_slow_progress() {
        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        let sender = thread::spawn(move || {
            for byte in [0_u8; 4] {
                let _ = writer.write_all(&[byte]);
                thread::sleep(Duration::from_millis(60));
            }
        });
        let started = Instant::now();
        let mut prefix = [0; 4];
        assert!(
            read_exact_until(
                &mut reader,
                &mut prefix,
                Instant::now() + Duration::from_millis(100),
            )
            .is_err()
        );
        assert!(started.elapsed() < Duration::from_millis(200));
        sender.join().unwrap();
    }

    #[test]
    fn full_command_table_never_evicts_pending_and_shutdown_stays_reserved() {
        let (completion_sender, _completion_receiver) = mpsc::channel();
        let mut table = VecDeque::new();
        let mut workers = Vec::new();
        let mut releases = Vec::new();
        for index in 0..TABLE_MAX {
            let (release, wait) = mpsc::sync_channel(1);
            releases.push(release);
            let id = format!("{index:032x}");
            assert_eq!(
                accept(
                    &mut table,
                    &mut workers,
                    &completion_sender,
                    id,
                    None,
                    move || {
                        let _ = wait.recv();
                        Ok(OwnerResponse::IdentityInitialized)
                    },
                ),
                (OwnerResponse::CommandAccepted, false)
            );
        }
        assert_eq!(table.len(), TABLE_MAX);
        assert_eq!(
            accept(
                &mut table,
                &mut workers,
                &completion_sender,
                "ffffffffffffffffffffffffffffffff".into(),
                None,
                || Ok(OwnerResponse::IdentityInitialized),
            ),
            (OwnerResponse::OwnerBusy, false)
        );
        assert!(table.iter().all(|record| record.response.is_none()));
        for release in releases {
            release.send(()).unwrap();
        }
        for worker in workers {
            worker.join().unwrap();
        }
    }

    #[test]
    fn terminal_results_expire_without_affecting_pending_records() {
        let mut table = VecDeque::from([
            CommandRecord {
                id: "0".repeat(32),
                response: Some(OwnerResponse::Unpaired),
                completed_at: Some(
                    Instant::now()
                        .checked_sub(Duration::from_mins(11))
                        .expect("test duration is representable"),
                ),
                decision: None,
                decision_results: Vec::new(),
                diagnostic: None,
            },
            CommandRecord {
                id: "1".repeat(32),
                response: None,
                completed_at: None,
                decision: None,
                decision_results: Vec::new(),
                diagnostic: None,
            },
        ]);
        expire_terminal(&mut table);
        assert_eq!(table.len(), 1);
        assert_eq!(table[0].id, "1".repeat(32));
    }

    #[test]
    fn pairing_decision_remains_reserved_when_command_table_is_full() {
        let (decision, decisions) = mpsc::sync_channel(1);
        let parent_id = "a".repeat(32);
        let mut table = VecDeque::from([CommandRecord {
            id: parent_id.clone(),
            response: Some(OwnerResponse::PairingConfirmationRequired {
                pairing_transaction_id: "b".repeat(32),
                authentication_string: "123456".into(),
            }),
            completed_at: None,
            decision: Some(decision),
            decision_results: Vec::new(),
            diagnostic: None,
        }]);
        for index in 1..TABLE_MAX {
            table.push_back(CommandRecord {
                id: format!("{index:032x}"),
                response: None,
                completed_at: None,
                decision: None,
                decision_results: Vec::new(),
                diagnostic: None,
            });
        }
        let mut state = ControlState {
            table,
            completion_sender: mpsc::channel().0,
            completion_receiver: mpsc::channel().1,
            workers: Vec::new(),
            event_sender: None,
        };
        let command_id = "c".repeat(32);
        assert_eq!(
            accept_pairing_decision(
                &mut state,
                command_id.clone(),
                &parent_id,
                "b".repeat(32),
                true,
            ),
            (OwnerResponse::PairingDecisionAccepted, false)
        );
        assert_eq!(decisions.recv().unwrap(), ("b".repeat(32), true));
        assert_eq!(state.table.len(), TABLE_MAX);
        assert_eq!(state.table[0].decision_results[0].0, command_id);
    }

    #[test]
    fn pairing_decision_requires_the_published_prompt_and_exact_transaction() {
        let (decision, decisions) = mpsc::sync_channel(1);
        let parent_id = "a".repeat(32);
        let transaction = "b".repeat(32);
        let mut state = ControlState {
            table: VecDeque::from([CommandRecord {
                id: parent_id.clone(),
                response: None,
                completed_at: None,
                decision: Some(decision),
                decision_results: Vec::new(),
                diagnostic: None,
            }]),
            completion_sender: mpsc::channel().0,
            completion_receiver: mpsc::channel().1,
            workers: Vec::new(),
            event_sender: None,
        };

        assert_eq!(
            accept_pairing_decision(
                &mut state,
                "c".repeat(32),
                &parent_id,
                transaction.clone(),
                true,
            ),
            (OwnerResponse::InvalidRequest, false)
        );
        assert!(decisions.try_recv().is_err());

        state.table[0].response = Some(OwnerResponse::PairingConfirmationRequired {
            pairing_transaction_id: transaction.clone(),
            authentication_string: "123456".into(),
        });
        assert_eq!(
            accept_pairing_decision(&mut state, "d".repeat(32), &parent_id, "e".repeat(32), true,),
            (OwnerResponse::InvalidRequest, false)
        );
        assert!(decisions.try_recv().is_err());

        assert_eq!(
            accept_pairing_decision(
                &mut state,
                "f".repeat(32),
                &parent_id,
                transaction.clone(),
                false,
            ),
            (OwnerResponse::PairingDecisionAccepted, false)
        );
        assert_eq!(decisions.recv().unwrap(), (transaction, false));
    }
    #[test]
    fn owner_info_and_reserved_shutdown_use_one_response_each() {
        let base = std::env::temp_dir().join(format!("deskkin-owner-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let actor = IdentityActor::start(IdentityStore::new(base.join("identity")));
        let control = base.join("control");
        let server_actor = actor.clone();
        let server_control = control.clone();
        let generation = "00112233445566778899aabbccddeeff".to_owned();
        let server_generation = generation.clone();
        let join = thread::spawn(move || {
            run_owner_control(&server_control, &server_actor, &server_generation)
        });
        for _ in 0..100 {
            if control.join("owner.sock").exists() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            call(&control, r#"{"command":"owner_info"}"#),
            OwnerResponse::OwnerInfo {
                owner_generation: generation
            }
        );
        assert_eq!(
            call(&control, r#"{"command":"shutdown"}"#),
            OwnerResponse::ShutdownAccepted
        );
        join.join().unwrap().unwrap();
        assert_eq!(actor.peer(), Err(crate::StoreError::Io));
        assert!(!control.join("owner.sock").exists());
    }

    #[test]
    fn stale_owner_socket_is_removed_only_under_acquired_lock() {
        let root = std::env::temp_dir().join(format!(
            "deskkin-stale-owner-{}",
            crate::new_control_id().unwrap()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let socket = root.join("owner.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        drop(listener);
        assert_eq!(discover_owner(&root).unwrap(), None);
        assert!(!socket.exists());
    }
    #[test]
    fn control_connection_capacity_is_four_partial_clients() {
        let base =
            std::env::temp_dir().join(format!("deskkin-owner-capacity-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let actor = IdentityActor::start(IdentityStore::new(base.join("identity")));
        let control = base.join("control");
        let server_actor = actor.clone();
        let server_control = control.clone();
        let join = thread::spawn(move || {
            run_owner_control(
                &server_control,
                &server_actor,
                "11223344556677889900aabbccddeeff",
            )
        });
        for _ in 0..100 {
            if control.join("owner.sock").exists() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let mut partial = Vec::new();
        for _ in 0..4 {
            let mut stream = UnixStream::connect(control.join("owner.sock")).unwrap();
            stream.write_all(&[0]).unwrap();
            partial.push(stream);
        }
        thread::sleep(Duration::from_millis(25));
        let mut overflow = UnixStream::connect(control.join("owner.sock")).unwrap();
        overflow
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let payload = br#"{"command":"owner_info"}"#;
        overflow
            .write_all(&(u32::try_from(payload.len()).unwrap()).to_be_bytes())
            .unwrap();
        overflow.write_all(payload).unwrap();
        let mut prefix = [0; 4];
        assert!(overflow.read_exact(&mut prefix).is_err());
        drop(partial);
        for _ in 0..100 {
            if matches!(
                call_owner_control(&control, &OwnerCommand::Shutdown),
                Ok(OwnerResponse::ShutdownAccepted)
            ) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        join.join().unwrap().unwrap();
    }
    #[test]
    fn accepted_mutation_is_idempotently_queryable() {
        let base =
            std::env::temp_dir().join(format!("deskkin-owner-mutation-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let actor = IdentityActor::start(IdentityStore::new(base.join("identity")));
        let control = base.join("control");
        let server_actor = actor.clone();
        let server_control = control.clone();
        let generation = "00112233445566778899aabbccddeeff".to_owned();
        let server_generation = generation.clone();
        let join = thread::spawn(move || {
            run_owner_control(&server_control, &server_actor, &server_generation)
        });
        for _ in 0..100 {
            if control.join("owner.sock").exists() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let command_id = "ffeeddccbbaa99887766554433221100";
        let request = format!(
            r#"{{"command":"identity_init","command_id":"{command_id}","owner_generation":"{generation}"}}"#
        );
        assert_eq!(call(&control, &request), OwnerResponse::CommandAccepted);
        let query = format!(
            r#"{{"command":"command_result","command_id":"{command_id}","owner_generation":"{generation}"}}"#
        );
        let terminal = (0..100)
            .find_map(|_| {
                let response = call(&control, &query);
                if response == OwnerResponse::CommandPending {
                    thread::sleep(Duration::from_millis(10));
                    None
                } else {
                    Some(response)
                }
            })
            .unwrap();
        assert_eq!(terminal, OwnerResponse::IdentityInitialized);
        assert_eq!(call(&control, &request), terminal);
        assert_eq!(
            call(&control, r#"{"command":"identity_list"}"#),
            OwnerResponse::IdentityState {
                peer: PeerState::Unpaired
            }
        );
        assert_eq!(
            call(&control, r#"{"command":"shutdown"}"#),
            OwnerResponse::ShutdownAccepted
        );
        join.join().unwrap().unwrap();
    }
    #[test]
    fn response_loss_does_not_reexecute_an_accepted_command() {
        let base = std::env::temp_dir().join(format!(
            "deskkin-owner-response-loss-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        let actor = IdentityActor::start(IdentityStore::new(base.join("identity")));
        let control = base.join("control");
        let server_actor = actor.clone();
        let server_control = control.clone();
        let generation = "102132435465768798a9bacbdcedfe0f".to_owned();
        let server_generation = generation.clone();
        let join = thread::spawn(move || {
            run_owner_control(&server_control, &server_actor, &server_generation)
        });
        for _ in 0..100 {
            if control.join("owner.sock").exists() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let command_id = "0123456789abcdeffedcba9876543210";
        let request = format!(
            r#"{{"command":"identity_init","command_id":"{command_id}","owner_generation":"{generation}"}}"#
        );
        send_without_reading_response(&control, &request);
        let query = format!(
            r#"{{"command":"command_result","command_id":"{command_id}","owner_generation":"{generation}"}}"#
        );
        let terminal = (0..100)
            .find_map(|_| {
                let response = call(&control, &query);
                if matches!(
                    response,
                    OwnerResponse::CommandPending | OwnerResponse::CommandUnknown
                ) {
                    thread::sleep(Duration::from_millis(10));
                    None
                } else {
                    Some(response)
                }
            })
            .unwrap();
        assert_eq!(terminal, OwnerResponse::IdentityInitialized);
        assert_eq!(call(&control, &request), terminal);
        assert_eq!(
            call(&control, r#"{"command":"shutdown"}"#),
            OwnerResponse::ShutdownAccepted
        );
        join.join().unwrap().unwrap();
    }

    fn send_without_reading_response(root: &Path, payload: &str) {
        let mut stream = UnixStream::connect(root.join("owner.sock")).unwrap();
        let bytes = payload.as_bytes();
        stream
            .write_all(&(u32::try_from(bytes.len()).unwrap()).to_be_bytes())
            .unwrap();
        stream.write_all(bytes).unwrap();
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }

    fn call(root: &Path, payload: &str) -> OwnerResponse {
        let mut stream = UnixStream::connect(root.join("owner.sock")).unwrap();
        let bytes = payload.as_bytes();
        stream
            .write_all(&(u32::try_from(bytes.len()).unwrap()).to_be_bytes())
            .unwrap();
        stream.write_all(bytes).unwrap();
        let mut prefix = [0; 4];
        stream.read_exact(&mut prefix).unwrap();
        let mut response = vec![0; u32::from_be_bytes(prefix) as usize];
        stream.read_exact(&mut response).unwrap();
        serde_json::from_slice(&response).unwrap()
    }
}
