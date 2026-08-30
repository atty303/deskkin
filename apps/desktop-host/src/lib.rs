#![forbid(unsafe_code)]
#![allow(clippy::missing_errors_doc)]

mod identity_actor;
mod owner_control;
pub mod profile;

pub use identity_actor::IdentityActor;
pub use owner_control::{
    OwnerCommand, OwnerEvent, OwnerInfo, OwnerLaunchMetadata, OwnerPairingTask, OwnerResponse,
    acquire_owner_lock, call_owner_control, discover_owner, discover_owner_info, owner_is_alive,
    query_command_result, run_owner_control, run_owner_control_with_events,
    run_owner_control_with_events_scoped, try_acquire_owner_lock,
};

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use deskkin_protocol::{
    APPLICATION_FRAME_MAX, AvailabilityResult, HelloRejectReason, Message, PRELUDE,
    PairingDecision, authentication_string, decode_frame_length, encode_frame_length,
};
use local_run_recorder::{
    Completeness, DiagnosticRun, Operation, OperationStatus, Publication, Recorder,
    RecordingHealth, RecordingMode, ResourceRole, RunOutcome, SemanticRecord, now_unix_ms,
    resource_identity_for,
};
use serde::{Deserialize, Serialize};
use snow::resolvers::{CryptoResolver, DefaultResolver};
use zeroize::{Zeroize, Zeroizing};

pub const NOISE_PATTERN: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";

pub fn new_control_id() -> Result<String, SessionError> {
    Ok(hex(&new_context_id()?))
}

pub fn new_context_id() -> Result<[u8; 16], SessionError> {
    let keypair = snow::Builder::new(NOISE_PATTERN.parse().map_err(|_| SessionError::Noise)?)
        .generate_keypair()
        .map_err(|_| SessionError::Noise)?;
    let private = Zeroizing::new(keypair.private);
    let mut value = [0; 16];
    value.copy_from_slice(&private[..16]);
    Ok(value)
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DiagnosticKind {
    ProtocolPairing,
    ProtocolSession,
    AvailabilityRead,
    IdentityInit,
    IdentityUnpair,
}

#[derive(Clone, Serialize)]
pub(crate) struct DiagnosticRecord {
    schema_version: u8,
    kind: DiagnosticKind,
    outcome: &'static str,
    transaction_id: Option<String>,
    session_context_id: Option<String>,
    operation_context_id: Option<String>,
    protocol_major: Option<u8>,
    selected_features: Option<[u8; 8]>,
    granted_permissions: Option<[u8; 8]>,
    error_type: Option<local_run_recorder::ErrorType>,
    run_id: Option<String>,
    created_unix_ms: Option<u64>,
    duration_ms: Option<u32>,
}

#[derive(Clone)]
enum DiagnosticEvent {
    Start {
        run_id: String,
        created_unix_ms: u64,
        kind: DiagnosticKind,
        scenario_context_id: Option<String>,
    },
    Finish {
        record: DiagnosticRecord,
        scenario_context_id: Option<String>,
    },
}

pub(crate) struct DiagnosticSpan {
    publisher: Arc<DiagnosticPublisher>,
    run_id: String,
    created_unix_ms: u64,
    started_at: std::time::Instant,
}

impl DiagnosticSpan {
    pub(crate) fn finish(self, mut record: DiagnosticRecord) {
        record.run_id = Some(self.run_id);
        record.created_unix_ms = Some(self.created_unix_ms);
        record.duration_ms =
            Some(u32::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u32::MAX));
        self.publisher.finish(record);
    }
}

impl std::fmt::Debug for DiagnosticSpan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DiagnosticSpan")
            .finish_non_exhaustive()
    }
}

struct FailureDiagnostic {
    span: Option<DiagnosticSpan>,
    record: Option<DiagnosticRecord>,
    kind: DiagnosticKind,
}

impl FailureDiagnostic {
    fn identity(store: &IdentityStore, kind: DiagnosticKind) -> Self {
        Self {
            span: store.start_diagnostic(kind),
            record: Some(DiagnosticRecord::identity(kind, "error")),
            kind,
        }
    }
    fn complete(&mut self) {
        self.record = None;
        if let Some(span) = self.span.take() {
            span.finish(DiagnosticRecord::identity(self.kind, "success"));
        }
    }
    fn error(&mut self, error_type: local_run_recorder::ErrorType) {
        if let Some(record) = self.record.as_mut() {
            record.error_type = Some(error_type);
        }
    }
}

impl Drop for FailureDiagnostic {
    fn drop(&mut self) {
        if let (Some(span), Some(record)) = (self.span.take(), self.record.take()) {
            span.finish(record);
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state", deny_unknown_fields)]
pub enum PeerState {
    Unpaired,
    Pending {
        remote_public_key: String,
        pairing_transaction_id: String,
    },
    Committing {
        remote_public_key: String,
        pairing_transaction_id: String,
    },
    Paired {
        remote_public_key: String,
        pairing_transaction_id: String,
    },
    Revoking {
        remote_public_key: String,
        previous_pairing_transaction_id: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    schema_major: u8,
    local_private_key: String,
    local_public_key: String,
    generation: u64,
    peer: PeerState,
}

impl Drop for Identity {
    fn drop(&mut self) {
        self.local_private_key.zeroize();
    }
}

#[derive(Clone, Debug)]
pub struct IdentityStore {
    root: PathBuf,
    role: ResourceRole,
    recording: RecordingMode,
    session_write_gate: Arc<Mutex<()>>,
    diagnostic_publisher: Arc<DiagnosticPublisher>,
    availability_diagnostics: Arc<Mutex<std::collections::HashMap<[u8; 16], DiagnosticSpan>>>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreError {
    Missing,
    AlreadyInitialized,
    Invalid,
    NotPrivate,
    UnknownEntry,
    PeerMismatch,
    GenerationExhausted,
    LockTimeout,
    RevokedRecoveryRequired,
    Io,
}

impl IdentityStore {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        let diagnostic_publisher =
            diagnostic_publisher(&root, ResourceRole::Host, RecordingMode::On, None);
        Self {
            root,
            role: ResourceRole::Host,
            recording: RecordingMode::On,
            session_write_gate: Arc::new(Mutex::new(())),
            diagnostic_publisher,
            availability_diagnostics: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }
    #[must_use]
    pub fn new_for_role(root: PathBuf, role: ResourceRole) -> Self {
        let diagnostic_publisher = diagnostic_publisher(&root, role, RecordingMode::On, None);
        Self {
            root,
            role,
            recording: RecordingMode::On,
            session_write_gate: Arc::new(Mutex::new(())),
            diagnostic_publisher,
            availability_diagnostics: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }
    #[must_use]
    pub fn with_recording(mut self, recording: RecordingMode) -> Self {
        self.recording = recording;
        self.diagnostic_publisher = diagnostic_publisher(&self.root, self.role, recording, None);
        self.availability_diagnostics = Arc::new(Mutex::new(std::collections::HashMap::new()));
        self
    }
    #[must_use]
    pub(crate) fn with_scenario_context(mut self, context: Option<String>) -> Self {
        self.diagnostic_publisher =
            diagnostic_publisher(&self.root, self.role, self.recording, context);
        self.availability_diagnostics = Arc::new(Mutex::new(std::collections::HashMap::new()));
        self
    }
    pub(crate) fn start_diagnostic(&self, kind: DiagnosticKind) -> Option<DiagnosticSpan> {
        self.diagnostic_publisher.start(kind)
    }
    fn start_availability_diagnostic(&self, operation: [u8; 16]) {
        if let Some(span) = self.start_diagnostic(DiagnosticKind::AvailabilityRead)
            && let Ok(mut spans) = self.availability_diagnostics.lock()
        {
            spans.insert(operation, span);
        }
    }
    fn finish_availability_diagnostic(&self, operation: [u8; 16], record: DiagnosticRecord) {
        if let Ok(mut spans) = self.availability_diagnostics.lock()
            && let Some(span) = spans.remove(&operation)
        {
            span.finish(record);
        }
    }
    fn finish_all_availability_diagnostics(&self, session: [u8; 16], error: SessionError) {
        let spans = self
            .availability_diagnostics
            .lock()
            .map(|mut spans| spans.drain().collect::<Vec<_>>())
            .unwrap_or_default();
        for (operation, span) in spans {
            span.finish(DiagnosticRecord::availability_failure(
                session, operation, error,
            ));
        }
    }
    pub fn init(&self) -> Result<String, StoreError> {
        let mut diagnostic = FailureDiagnostic::identity(self, DiagnosticKind::IdentityInit);
        let result = self.init_inner();
        if result.is_ok() {
            diagnostic.complete();
        }
        result
    }
    pub(crate) fn init_for_owner(&self) -> Result<String, StoreError> {
        self.init_inner()
    }
    fn init_inner(&self) -> Result<String, StoreError> {
        self.prepare_root(true)?;
        let canonical = self.root.join("identity-v1.json");
        if canonical.exists() {
            self.read()?;
            return Err(StoreError::AlreadyInitialized);
        }
        let params = NOISE_PATTERN.parse().map_err(|_| StoreError::Invalid)?;
        let builder = snow::Builder::new(params);
        let pair = builder
            .generate_keypair()
            .map_err(|_| StoreError::Invalid)?;
        let identity = Identity {
            schema_major: 1,
            local_private_key: hex(&pair.private),
            local_public_key: hex(&pair.public),
            generation: 0,
            peer: PeerState::Unpaired,
        };
        self.publish(&identity)?;
        Ok(identity.local_public_key.clone())
    }
    pub fn public_key(&self) -> Result<String, StoreError> {
        Ok(self.read()?.local_public_key.clone())
    }
    pub fn peer(&self) -> Result<PeerState, StoreError> {
        Ok(self.read()?.peer.clone())
    }
    pub fn private_key(&self) -> Result<Zeroizing<Vec<u8>>, StoreError> {
        decode_hex(&self.read()?.local_private_key).map(Zeroizing::new)
    }
    fn paired_generation(&self) -> Result<u64, StoreError> {
        let identity = self.read()?;
        if matches!(identity.peer, PeerState::Paired { .. }) {
            Ok(identity.generation)
        } else {
            Err(StoreError::PeerMismatch)
        }
    }
    pub fn with_paired_generation<T>(
        &self,
        generation: u64,
        apply: impl FnOnce() -> T,
    ) -> Result<T, SessionError> {
        let _gate = self
            .session_write_gate
            .lock()
            .map_err(|_| SessionError::Identity)?;
        if self
            .paired_generation()
            .map_err(|_| SessionError::Identity)?
            != generation
        {
            return Err(SessionError::Identity);
        }
        Ok(apply())
    }
    fn set_peer(&self, peer: PeerState) -> Result<(), StoreError> {
        validate_peer(&peer)?;
        self.prepare_root(false)?;
        let lock = self.lock_file()?;
        lock_identity(&lock)?;
        let mut identity = self.read_unlocked()?;
        if !valid_peer_transition(&identity.peer, &peer) {
            return Err(StoreError::Invalid);
        }
        identity.peer = peer;
        self.publish_locked(&identity)
    }
    pub fn unpair(&self, remote: &str) -> Result<(), StoreError> {
        self.unpair_with_hook(remote, || || true)
    }
    fn unpair_with_hook<J>(
        &self,
        remote: &str,
        revoked: impl FnOnce() -> J,
    ) -> Result<(), StoreError>
    where
        J: FnOnce() -> bool,
    {
        let mut diagnostic = FailureDiagnostic::identity(self, DiagnosticKind::IdentityUnpair);
        let result = self.unpair_with_hook_inner(remote, revoked, &mut diagnostic);
        if result.is_ok() {
            diagnostic.complete();
        }
        result
    }
    pub(crate) fn unpair_with_hook_for_owner<J>(
        &self,
        remote: &str,
        revoked: impl FnOnce() -> J,
    ) -> Result<(), StoreError>
    where
        J: FnOnce() -> bool,
    {
        let mut diagnostic = FailureDiagnostic {
            span: None,
            record: None,
            kind: DiagnosticKind::IdentityUnpair,
        };
        self.unpair_with_hook_inner(remote, revoked, &mut diagnostic)
    }
    fn unpair_with_hook_inner<J>(
        &self,
        remote: &str,
        revoked: impl FnOnce() -> J,
        diagnostic: &mut FailureDiagnostic,
    ) -> Result<(), StoreError>
    where
        J: FnOnce() -> bool,
    {
        self.prepare_root(false)?;
        let lock = self.lock_file()?;
        lock_identity(&lock)?;
        let mut identity = self.read_unlocked()?;
        let actual = match &identity.peer {
            PeerState::Unpaired => return Err(StoreError::PeerMismatch),
            PeerState::Pending {
                remote_public_key, ..
            }
            | PeerState::Committing {
                remote_public_key, ..
            }
            | PeerState::Paired {
                remote_public_key, ..
            }
            | PeerState::Revoking {
                remote_public_key, ..
            } => remote_public_key,
        };
        if actual != remote {
            return Err(StoreError::PeerMismatch);
        }
        let recovering = matches!(identity.peer, PeerState::Revoking { .. });
        if !recovering {
            identity.generation = identity
                .generation
                .checked_add(1)
                .ok_or(StoreError::GenerationExhausted)?;
        }
        let tx = match &identity.peer {
            PeerState::Pending {
                pairing_transaction_id,
                ..
            }
            | PeerState::Committing {
                pairing_transaction_id,
                ..
            }
            | PeerState::Paired {
                pairing_transaction_id,
                ..
            } => pairing_transaction_id.clone(),
            PeerState::Revoking {
                previous_pairing_transaction_id,
                ..
            } => previous_pairing_transaction_id.clone(),
            PeerState::Unpaired => unreachable!(),
        };
        if !recovering {
            identity.peer = PeerState::Revoking {
                remote_public_key: remote.into(),
                previous_pairing_transaction_id: tx,
            };
            let renamed = std::cell::Cell::new(false);
            let join = std::cell::RefCell::new(None);
            let session_write = self.session_write_gate.lock().map_err(|_| StoreError::Io)?;
            let result = self.publish_locked_with_rename_hook(&identity, || {
                renamed.set(true);
                *join.borrow_mut() = Some(revoked());
            });
            if let Err(error) = result {
                return if renamed.get() {
                    diagnostic.error(local_run_recorder::ErrorType::RevocationRecoveryRequired);
                    Err(StoreError::RevokedRecoveryRequired)
                } else {
                    Err(error)
                };
            }
            drop(session_write);
            if !join.into_inner().is_some_and(|join| join()) {
                diagnostic.error(local_run_recorder::ErrorType::RevocationRecoveryRequired);
                return Err(StoreError::RevokedRecoveryRequired);
            }
        }
        identity.peer = PeerState::Unpaired;
        if self.publish_locked(&identity).is_err() {
            diagnostic.error(local_run_recorder::ErrorType::RevocationRecoveryRequired);
            return Err(StoreError::RevokedRecoveryRequired);
        }
        Ok(())
    }
    fn prepare_root(&self, allow_virgin: bool) -> Result<(), StoreError> {
        if !self.root.exists() {
            if !allow_virgin {
                return Err(StoreError::Missing);
            }
            create_private_tree(&self.root)?;
        }
        let m = fs::symlink_metadata(&self.root).map_err(|_| StoreError::Io)?;
        if m.file_type().is_symlink() || !m.is_dir() || m.permissions().mode() & 0o077 != 0 {
            return Err(StoreError::NotPrivate);
        }
        for entry in fs::read_dir(&self.root).map_err(|_| StoreError::Io)? {
            let entry = entry.map_err(|_| StoreError::Io)?;
            let name = entry.file_name();
            if name != ".identity.lock" && name != ".identity.tmp" && name != "identity-v1.json" {
                return Err(StoreError::UnknownEntry);
            }
            let metadata = fs::symlink_metadata(entry.path()).map_err(|_| StoreError::Io)?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(StoreError::Invalid);
            }
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(StoreError::NotPrivate);
            }
        }
        Ok(())
    }
    fn read(&self) -> Result<Identity, StoreError> {
        self.prepare_root(false)?;
        let lock = self.lock_file()?;
        lock_identity(&lock)?;
        let value = self.read_unlocked()?;
        let temporary = self.root.join(".identity.tmp");
        if temporary.exists() {
            let metadata = fs::symlink_metadata(&temporary).map_err(|_| StoreError::Io)?;
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || metadata.permissions().mode() & 0o077 != 0
            {
                return Err(StoreError::Invalid);
            }
            fs::remove_file(temporary).map_err(|_| StoreError::Io)?;
            File::open(&self.root)
                .and_then(|file| file.sync_all())
                .map_err(|_| StoreError::Io)?;
        }
        Ok(value)
    }
    fn read_unlocked(&self) -> Result<Identity, StoreError> {
        self.prepare_root(false)?;
        let bytes = Zeroizing::new(fs::read(self.root.join("identity-v1.json")).map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                StoreError::Missing
            } else {
                StoreError::Io
            }
        })?);
        validate_identity_json_shape(&bytes)?;
        let value: Identity = serde_json::from_slice(&bytes).map_err(|_| StoreError::Invalid)?;
        if value.schema_major != 1 {
            return Err(StoreError::Invalid);
        }
        validate_hex(&value.local_private_key, 32)?;
        validate_hex(&value.local_public_key, 32)?;
        let private = Zeroizing::new(decode_hex(&value.local_private_key)?);
        let mut dh = DefaultResolver
            .resolve_dh(&snow::params::DHChoice::Curve25519)
            .ok_or(StoreError::Invalid)?;
        dh.set(&private);
        let public = decode_hex(&value.local_public_key)?;
        if !constant_time_eq(dh.pubkey(), &public) {
            return Err(StoreError::Invalid);
        }
        validate_peer(&value.peer)?;
        Ok(value)
    }
    fn publish(&self, value: &Identity) -> Result<(), StoreError> {
        self.prepare_root(true)?;
        let lock = self.lock_file()?;
        lock_identity(&lock)?;
        if self.root.join("identity-v1.json").exists() {
            self.read_unlocked()?;
            return Err(StoreError::AlreadyInitialized);
        }
        self.publish_locked(value)
    }
    fn lock_file(&self) -> Result<File, StoreError> {
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(self.root.join(".identity.lock"))
            .map_err(|_| StoreError::Io)
    }
    fn publish_locked(&self, value: &Identity) -> Result<(), StoreError> {
        self.publish_locked_with_rename_hook(value, || {})
    }
    fn publish_locked_with_rename_hook(
        &self,
        value: &Identity,
        renamed: impl FnOnce(),
    ) -> Result<(), StoreError> {
        let bytes = Zeroizing::new(serde_json::to_vec(value).map_err(|_| StoreError::Invalid)?);
        let tmp = self.root.join(".identity.tmp");
        if tmp.exists() {
            let metadata = fs::symlink_metadata(&tmp).map_err(|_| StoreError::Io)?;
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || metadata.permissions().mode() & 0o077 != 0
            {
                return Err(StoreError::Invalid);
            }
            fs::remove_file(&tmp).map_err(|_| StoreError::Io)?;
        }
        let mut f = OpenOptions::new()
            .create_new(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|_| StoreError::Io)?;
        f.write_all(&bytes).map_err(|_| StoreError::Io)?;
        f.sync_all().map_err(|_| StoreError::Io)?;
        fs::rename(tmp, self.root.join("identity-v1.json")).map_err(|_| StoreError::Io)?;
        renamed();
        File::open(&self.root)
            .and_then(|f| f.sync_all())
            .map_err(|_| StoreError::Io)
    }
    fn record(&self, record: &DiagnosticRecord) {
        self.diagnostic_publisher.publish(record.clone());
    }
}

fn validate_identity_json_shape(bytes: &[u8]) -> Result<(), StoreError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| StoreError::Invalid)?;
    let object = value.as_object().ok_or(StoreError::Invalid)?;
    let expected = [
        "schema_major",
        "local_private_key",
        "local_public_key",
        "generation",
        "peer",
    ];
    if object.len() != expected.len() || expected.iter().any(|key| !object.contains_key(*key)) {
        return Err(StoreError::Invalid);
    }
    let peer = object
        .get("peer")
        .and_then(serde_json::Value::as_object)
        .ok_or(StoreError::Invalid)?;
    let state = peer
        .get("state")
        .and_then(serde_json::Value::as_str)
        .ok_or(StoreError::Invalid)?;
    let expected: &[&str] = match state {
        "unpaired" => &["state"],
        "pending" | "committing" | "paired" => {
            &["state", "remote_public_key", "pairing_transaction_id"]
        }
        "revoking" => &[
            "state",
            "remote_public_key",
            "previous_pairing_transaction_id",
        ],
        _ => return Err(StoreError::Invalid),
    };
    if peer.len() != expected.len() || expected.iter().any(|key| !peer.contains_key(*key)) {
        return Err(StoreError::Invalid);
    }
    Ok(())
}

struct DiagnosticPublisher {
    sender: Mutex<Option<mpsc::SyncSender<DiagnosticEvent>>>,
    join: Mutex<Option<thread::JoinHandle<()>>>,
    dropped: Arc<Mutex<Option<String>>>,
    scenario_context_id: Option<String>,
}

impl std::fmt::Debug for DiagnosticPublisher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DiagnosticPublisher")
            .finish_non_exhaustive()
    }
}

impl DiagnosticPublisher {
    fn publish(self: &Arc<Self>, record: DiagnosticRecord) {
        let Some(span) = self.start(record.kind) else {
            return;
        };
        span.finish(record);
    }

    fn start(self: &Arc<Self>, kind: DiagnosticKind) -> Option<DiagnosticSpan> {
        let run_id = new_context_id()
            .ok()
            .map(|id| format!("run-{}", hex(&id)))?;
        let created_unix_ms = now_unix_ms();
        self.send(DiagnosticEvent::Start {
            run_id: run_id.clone(),
            created_unix_ms,
            kind,
            scenario_context_id: self.scenario_context_id.clone(),
        })?;
        Some(DiagnosticSpan {
            publisher: self.clone(),
            run_id,
            created_unix_ms,
            started_at: std::time::Instant::now(),
        })
    }

    fn finish(&self, record: DiagnosticRecord) {
        let _ = self.send(DiagnosticEvent::Finish {
            record,
            scenario_context_id: self.scenario_context_id.clone(),
        });
    }

    fn send(&self, event: DiagnosticEvent) -> Option<()> {
        let sent = self.sender.lock().ok().is_some_and(|sender| {
            sender
                .as_ref()
                .is_some_and(|sender| sender.try_send(event).is_ok())
        });
        if !sent && let Ok(mut dropped) = self.dropped.lock() {
            *dropped = Some("diagnostic-queue-full".into());
        }
        sent.then_some(())
    }
}

impl Drop for DiagnosticPublisher {
    fn drop(&mut self) {
        if let Ok(sender) = self.sender.get_mut() {
            sender.take();
        }
        if let Ok(join) = self.join.get_mut()
            && let Some(join) = join.take()
        {
            let _ = join.join();
        }
    }
}

fn diagnostic_publisher(
    identity_root: &Path,
    role: ResourceRole,
    recording: RecordingMode,
    scenario_context_id: Option<String>,
) -> Arc<DiagnosticPublisher> {
    let (sender, receiver) = mpsc::sync_channel::<DiagnosticEvent>(32);
    let role_root = identity_root.parent().map(Path::to_path_buf);
    let dropped = Arc::new(Mutex::new(None));
    let worker_dropped = dropped.clone();
    let join = thread::spawn(move || {
        let Some(role_root) = role_root else { return };
        let recorder = Recorder::new(role_root, recording, 16 * 1024 * 1024);
        let mut live_runs = std::collections::HashMap::new();
        while let Ok(event) = receiver.recv() {
            match event {
                DiagnosticEvent::Start {
                    run_id,
                    created_unix_ms,
                    kind,
                    scenario_context_id,
                } => {
                    if let Ok(marker) = recorder.begin_live_run(&run_id) {
                        live_runs.insert(run_id.clone(), marker);
                    }
                    let partial = diagnostic_run(
                        role,
                        DiagnosticRecord::started(kind, run_id, created_unix_ms),
                        false,
                        scenario_context_id,
                    );
                    let _ = recorder.publish(partial);
                }
                DiagnosticEvent::Finish {
                    record,
                    scenario_context_id,
                } => {
                    let run_id = record.run_id.clone();
                    let run = diagnostic_run(role, record, true, scenario_context_id);
                    let _ = recorder.publish(run);
                    if let Some(run_id) = run_id
                        && let Some(marker) = live_runs.remove(&run_id)
                    {
                        let _ = recorder.end_live_run(&run_id, marker);
                    }
                }
            }
            {
                if let Ok(mut dropped) = worker_dropped.lock()
                    && let Some(run_id) = dropped.take()
                {
                    let _ = recorder.publish_health_best_effort(&Publication {
                        run_id,
                        completeness: Completeness::Dropped,
                        health: RecordingHealth::StorageUnavailable,
                        stored: false,
                    });
                }
            }
        }
    });
    Arc::new(DiagnosticPublisher {
        sender: Mutex::new(Some(sender)),
        join: Mutex::new(Some(join)),
        dropped,
        scenario_context_id,
    })
}

fn diagnostic_run(
    role: ResourceRole,
    record: DiagnosticRecord,
    terminal: bool,
    scenario_context_id: Option<String>,
) -> DiagnosticRun {
    let run_id = record
        .run_id
        .clone()
        .unwrap_or_else(|| "diagnostic-missing-id".into());
    let status = if record.outcome == "success" {
        OperationStatus::Success
    } else {
        OperationStatus::Error
    };
    let mut records = protocol_diagnostic_records(record.kind, status, record.error_type);
    if terminal {
        for operation in &mut records {
            operation.duration_ms = record.duration_ms;
        }
    } else {
        for operation in &mut records {
            operation.status = OperationStatus::InProgress;
            operation.error_type = None;
            operation.duration_ms = None;
        }
    }
    DiagnosticRun {
        schema_version: 1,
        resource: resource_identity_for(
            match role {
                ResourceRole::Host => "deskkin-desktop-host",
                ResourceRole::DeviceSimulator => "deskkin-simulator",
                ResourceRole::PhysicalDevice => "deskkin-core-s3-runner",
            },
            env!("CARGO_PKG_VERSION"),
            role,
        ),
        run_id: run_id.clone(),
        scenario_run_id: scenario_context_id.unwrap_or_else(|| {
            record
                .operation_context_id
                .clone()
                .or_else(|| record.session_context_id.clone())
                .or_else(|| record.transaction_id.clone())
                .unwrap_or_else(|| run_id.clone())
        }),
        transaction_id: record.transaction_id,
        session_context_id: record.session_context_id,
        operation_context_id: record.operation_context_id,
        protocol_major: record.protocol_major,
        selected_features: record.selected_features,
        granted_permissions: record.granted_permissions,
        outcome: if record.outcome == "success" {
            RunOutcome::Success
        } else {
            RunOutcome::Error
        },
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
        created_unix_ms: record.created_unix_ms.unwrap_or_else(now_unix_ms),
        records,
    }
}

fn protocol_diagnostic_records(
    kind: DiagnosticKind,
    terminal_status: OperationStatus,
    error_type: Option<local_run_recorder::ErrorType>,
) -> Vec<SemanticRecord> {
    let operations: &[Operation] = match kind {
        DiagnosticKind::ProtocolPairing => &[
            Operation::TransportAccept,
            Operation::NoiseHandshake,
            Operation::PairingConfirm,
            Operation::PairingPersist,
        ],
        DiagnosticKind::ProtocolSession => &[
            Operation::TransportAccept,
            Operation::NoiseHandshake,
            Operation::ProtocolNegotiate,
        ],
        DiagnosticKind::AvailabilityRead => &[
            Operation::AvailabilityRead,
            Operation::TransportFrameRead,
            Operation::TransportFrameWrite,
        ],
        DiagnosticKind::IdentityInit => &[Operation::ControlRoute, Operation::IdentityInit],
        DiagnosticKind::IdentityUnpair => &[Operation::ControlRoute, Operation::IdentityUnpair],
    };
    operations
        .iter()
        .enumerate()
        .map(|(index, operation)| {
            let terminal = index + 1 == operations.len();
            SemanticRecord {
                operation: *operation,
                operation_id: u16::try_from(index + 1).unwrap_or(u16::MAX),
                parent_operation_id: (index != 0).then_some(1),
                status: if terminal {
                    terminal_status
                } else {
                    OperationStatus::Success
                },
                error_type: terminal.then_some(error_type).flatten(),
                effect_id: None,
                virtual_time_ms: 0,
                end_virtual_time_ms: 0,
                duration_ms: Some(0),
                render_width: None,
                render_height: None,
                value: None,
            }
        })
        .collect()
}

fn create_private_tree(path: &Path) -> Result<(), StoreError> {
    let mut missing = Vec::new();
    let mut cursor = path;
    while !cursor.exists() {
        missing.push(cursor.to_path_buf());
        cursor = cursor.parent().ok_or(StoreError::Invalid)?;
    }
    for ancestor in cursor.ancestors() {
        let metadata = fs::symlink_metadata(ancestor).map_err(|_| StoreError::Io)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(StoreError::Invalid);
        }
    }
    for directory in missing.into_iter().rev() {
        match fs::create_dir(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(StoreError::Io),
        }
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .map_err(|_| StoreError::Io)?;
        File::open(&directory)
            .and_then(|file| file.sync_all())
            .map_err(|_| StoreError::Io)?;
        let parent = directory.parent().ok_or(StoreError::Invalid)?;
        File::open(parent)
            .and_then(|file| file.sync_all())
            .map_err(|_| StoreError::Io)?;
    }
    Ok(())
}

fn lock_identity(file: &File) -> Result<(), StoreError> {
    let started = std::time::Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(()),
            Err(std::fs::TryLockError::WouldBlock)
                if started.elapsed() < Duration::from_secs(2) =>
            {
                thread::sleep(Duration::from_millis(10));
            }
            Err(std::fs::TryLockError::WouldBlock) => return Err(StoreError::LockTimeout),
            Err(std::fs::TryLockError::Error(_)) => return Err(StoreError::Io),
        }
    }
}

impl DiagnosticRecord {
    fn started(kind: DiagnosticKind, run_id: String, created_unix_ms: u64) -> Self {
        Self {
            schema_version: 1,
            kind,
            outcome: "error",
            transaction_id: None,
            session_context_id: None,
            operation_context_id: None,
            protocol_major: None,
            selected_features: None,
            granted_permissions: None,
            error_type: None,
            run_id: Some(run_id),
            created_unix_ms: Some(created_unix_ms),
            duration_ms: None,
        }
    }
    pub(crate) fn identity(kind: DiagnosticKind, outcome: &'static str) -> Self {
        Self {
            schema_version: 1,
            kind,
            outcome,
            transaction_id: None,
            session_context_id: None,
            operation_context_id: None,
            protocol_major: None,
            selected_features: None,
            granted_permissions: None,
            error_type: (outcome == "error")
                .then_some(local_run_recorder::ErrorType::IdentityStore),
            run_id: None,
            created_unix_ms: None,
            duration_ms: None,
        }
    }
    fn pairing(transaction: [u8; 16]) -> Self {
        Self {
            schema_version: 1,
            kind: DiagnosticKind::ProtocolPairing,
            outcome: "success",
            transaction_id: Some(hex(&transaction)),
            session_context_id: None,
            operation_context_id: None,
            protocol_major: Some(1),
            selected_features: Some(deskkin_protocol::AVAILABILITY_READ_V1.0),
            granted_permissions: Some(deskkin_protocol::AVAILABILITY_READ_PERMISSION.0),
            error_type: None,
            run_id: None,
            created_unix_ms: None,
            duration_ms: None,
        }
    }
    fn pairing_failure_for(error: SessionError) -> Self {
        let error_type = if matches!(error, SessionError::Timeout) {
            local_run_recorder::ErrorType::HandshakeTimeout
        } else {
            error_type(error)
        };
        Self {
            schema_version: 1,
            kind: DiagnosticKind::ProtocolPairing,
            outcome: "error",
            transaction_id: None,
            session_context_id: None,
            operation_context_id: None,
            protocol_major: Some(1),
            selected_features: None,
            granted_permissions: None,
            error_type: Some(error_type),
            run_id: None,
            created_unix_ms: None,
            duration_ms: None,
        }
    }
    fn pairing_failure_for_transaction(error: SessionError, transaction: Option<[u8; 16]>) -> Self {
        let mut record = Self::pairing_failure_for(error);
        record.transaction_id = transaction.map(|transaction| hex(&transaction));
        record
    }
    fn availability(session: [u8; 16], operation: [u8; 16], result: AvailabilityResult) -> Self {
        let failed = result == AvailabilityResult::ReadFailed;
        Self {
            schema_version: 1,
            kind: DiagnosticKind::AvailabilityRead,
            outcome: if failed { "error" } else { "success" },
            transaction_id: None,
            session_context_id: Some(hex(&session)),
            operation_context_id: Some(hex(&operation)),
            protocol_major: Some(1),
            selected_features: Some(deskkin_protocol::AVAILABILITY_READ_V1.0),
            granted_permissions: Some(deskkin_protocol::AVAILABILITY_READ_PERMISSION.0),
            error_type: failed.then_some(local_run_recorder::ErrorType::ReadUnavailable),
            run_id: None,
            created_unix_ms: None,
            duration_ms: None,
        }
    }
    fn session(session: [u8; 16]) -> Self {
        Self {
            schema_version: 1,
            kind: DiagnosticKind::ProtocolSession,
            outcome: "success",
            transaction_id: None,
            session_context_id: Some(hex(&session)),
            operation_context_id: None,
            protocol_major: Some(1),
            selected_features: Some(deskkin_protocol::AVAILABILITY_READ_V1.0),
            granted_permissions: Some(deskkin_protocol::AVAILABILITY_READ_PERMISSION.0),
            error_type: None,
            run_id: None,
            created_unix_ms: None,
            duration_ms: None,
        }
    }
    fn preauth_capacity() -> Self {
        let mut record = Self::session_failure([0; 16], SessionError::QueueFull);
        record.session_context_id = None;
        record.error_type = Some(local_run_recorder::ErrorType::PreauthCapacity);
        record
    }
    fn pre_session_failure(error: SessionError) -> Self {
        let mut record = Self::session_failure([0; 16], error);
        record.session_context_id = None;
        record
    }
    fn availability_failure(session: [u8; 16], operation: [u8; 16], error: SessionError) -> Self {
        let mut record = Self::session_failure(session, error);
        record.kind = DiagnosticKind::AvailabilityRead;
        record.operation_context_id = Some(hex(&operation));
        record
    }
    fn session_failure(session: [u8; 16], error: SessionError) -> Self {
        Self {
            schema_version: 1,
            kind: DiagnosticKind::ProtocolSession,
            outcome: "error",
            transaction_id: None,
            session_context_id: Some(hex(&session)),
            operation_context_id: None,
            protocol_major: Some(1),
            selected_features: None,
            granted_permissions: None,
            error_type: Some(error_type(error)),
            run_id: None,
            created_unix_ms: None,
            duration_ms: None,
        }
    }
}

const fn error_type(error: SessionError) -> local_run_recorder::ErrorType {
    match error {
        SessionError::PairingTimeout => local_run_recorder::ErrorType::PairingExpired,
        SessionError::Incompatible => local_run_recorder::ErrorType::VersionMismatch,
        SessionError::AuthorizationDenied => local_run_recorder::ErrorType::PermissionDenied,
        SessionError::SessionBusy => local_run_recorder::ErrorType::SessionBusy,
        SessionError::QueueFull => local_run_recorder::ErrorType::QueueFull,
        SessionError::EndOfStream => local_run_recorder::ErrorType::EndOfStream,
        SessionError::FrameOversize => local_run_recorder::ErrorType::FrameOversize,
        SessionError::Timeout => local_run_recorder::ErrorType::RequestTimeout,
        SessionError::Protocol => local_run_recorder::ErrorType::MalformedFrame,
        SessionError::Noise => local_run_recorder::ErrorType::AuthenticationFailed,
        SessionError::Identity => local_run_recorder::ErrorType::IdentityStore,
        SessionError::PairingRejected => local_run_recorder::ErrorType::PairingRejected,
        SessionError::PairingIncomplete => local_run_recorder::ErrorType::PairingIncomplete,
        SessionError::StoreFailed => local_run_recorder::ErrorType::StoreFailed,
        SessionError::NonLoopback | SessionError::NonPrivateLan => {
            local_run_recorder::ErrorType::InvalidAddress
        }
        SessionError::Io => local_run_recorder::ErrorType::ConnectionLost,
    }
}

#[derive(Clone, Copy, Debug)]
pub enum SessionError {
    NonLoopback,
    NonPrivateLan,
    Io,
    Noise,
    Protocol,
    Identity,
    PairingTimeout,
    Incompatible,
    AuthorizationDenied,
    SessionBusy,
    QueueFull,
    EndOfStream,
    FrameOversize,
    Timeout,
    PairingRejected,
    PairingIncomplete,
    StoreFailed,
}
pub fn bind_loopback(address: SocketAddr) -> Result<TcpListener, SessionError> {
    if !address.ip().is_loopback() {
        return Err(SessionError::NonLoopback);
    }
    TcpListener::bind(address).map_err(|_| SessionError::Io)
}

pub const PRIVATE_LAN_PORT: u16 = 39_042;

#[must_use]
pub fn is_exact_private_lan_address(address: SocketAddr) -> bool {
    let SocketAddr::V4(address) = address else {
        return false;
    };
    let octets = address.ip().octets();
    let private = octets[0] == 10
        || (octets[0] == 172 && (16..=31).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 168);
    private && !address.ip().is_unspecified() && address.port() == PRIVATE_LAN_PORT
}

pub fn bind_private_lan(address: SocketAddr) -> Result<TcpListener, SessionError> {
    if !is_exact_private_lan_address(address) {
        return Err(SessionError::NonPrivateLan);
    }
    // Binding is also the authoritative local-assignment check on Linux.
    TcpListener::bind(address).map_err(|_| SessionError::Io)
}

pub fn pair_responder<F>(
    listener: &TcpListener,
    store: &IdentityStore,
    confirm: F,
) -> Result<[u8; 6], SessionError>
where
    F: FnOnce([u8; 16], [u8; 6]) -> bool + Send + 'static,
{
    pair_responder_with_timeout(listener, store, confirm, Duration::from_mins(1))
}

#[allow(clippy::too_many_lines)]
fn pair_responder_with_timeout<F>(
    listener: &TcpListener,
    store: &IdentityStore,
    confirm: F,
    window: Duration,
) -> Result<[u8; 6], SessionError>
where
    F: FnOnce([u8; 16], [u8; 6]) -> bool + Send + 'static,
{
    let diagnostic = store.start_diagnostic(DiagnosticKind::ProtocolPairing);
    let Ok(peer) = store.peer() else {
        if let Some(diagnostic) = diagnostic {
            diagnostic.finish(DiagnosticRecord::pairing_failure_for(
                SessionError::Identity,
            ));
        }
        return Err(SessionError::Identity);
    };
    if peer != PeerState::Unpaired {
        if let Some(diagnostic) = diagnostic {
            diagnostic.finish(DiagnosticRecord::pairing_failure_for(
                SessionError::Identity,
            ));
        }
        return Err(SessionError::Identity);
    }
    let (stream, _) = match accept_pairing_connection(listener, window) {
        Ok(connection) => connection,
        Err(error) => {
            if let Some(diagnostic) = diagnostic {
                diagnostic.finish(DiagnosticRecord::pairing_failure_for(error));
            }
            return Err(error);
        }
    };
    let _busy_guard = match PairingBusyGuard::start(listener, store) {
        Ok(guard) => guard,
        Err(error) => {
            if let Some(diagnostic) = diagnostic {
                diagnostic.finish(DiagnosticRecord::pairing_failure_for(error));
            }
            return Err(error);
        }
    };
    pair_responder_stream_reserved_with_diagnostic(
        stream,
        store,
        confirm,
        || true,
        std::time::Instant::now() + Duration::from_mins(1),
        diagnostic,
    )
}

#[allow(clippy::too_many_lines)]
fn pair_responder_stream_reserved<F, R>(
    stream: TcpStream,
    store: &IdentityStore,
    confirm: F,
    reserve: R,
    deadline: std::time::Instant,
) -> Result<[u8; 6], SessionError>
where
    F: FnOnce([u8; 16], [u8; 6]) -> bool + Send + 'static,
    R: FnOnce() -> bool,
{
    let diagnostic = store.start_diagnostic(DiagnosticKind::ProtocolPairing);
    pair_responder_stream_reserved_with_diagnostic(
        stream, store, confirm, reserve, deadline, diagnostic,
    )
}

#[allow(clippy::too_many_lines)]
fn pair_responder_stream_reserved_with_diagnostic<F, R>(
    stream: TcpStream,
    store: &IdentityStore,
    confirm: F,
    reserve: R,
    deadline: std::time::Instant,
    diagnostic: Option<DiagnosticSpan>,
) -> Result<[u8; 6], SessionError>
where
    F: FnOnce([u8; 16], [u8; 6]) -> bool + Send + 'static,
    R: FnOnce() -> bool,
{
    let diagnostic_transaction = Mutex::new(None);
    let result = pair_responder_stream_reserved_inner(
        stream,
        store,
        confirm,
        reserve,
        deadline,
        &diagnostic_transaction,
    );
    let transaction = diagnostic_transaction.into_inner().ok().flatten();
    if let Some(diagnostic) = diagnostic {
        let record = if matches!(store.peer(), Ok(PeerState::Paired { .. })) {
            transaction.map_or_else(
                || DiagnosticRecord::pairing_failure_for(SessionError::PairingIncomplete),
                DiagnosticRecord::pairing,
            )
        } else {
            DiagnosticRecord::pairing_failure_for_transaction(
                result.err().unwrap_or(SessionError::PairingIncomplete),
                transaction,
            )
        };
        diagnostic.finish(record);
    }
    result
}

#[allow(clippy::too_many_lines)]
fn pair_responder_stream_reserved_inner<F, R>(
    mut stream: TcpStream,
    store: &IdentityStore,
    confirm: F,
    reserve: R,
    deadline: std::time::Instant,
    diagnostic_transaction: &Mutex<Option<[u8; 16]>>,
) -> Result<[u8; 6], SessionError>
where
    F: FnOnce([u8; 16], [u8; 6]) -> bool + Send + 'static,
    R: FnOnce() -> bool,
{
    let actor = IdentityActor::start(store.clone());
    if store.peer().map_err(|_| SessionError::Identity)? != PeerState::Unpaired {
        return Err(SessionError::Identity);
    }
    configure(&stream)?;
    let mut prelude = [0; 6];
    read_exact_deadline(
        &mut stream,
        &mut prelude,
        std::time::Instant::now() + Duration::from_secs(5),
    )?;
    if prelude != PRELUDE {
        return Err(SessionError::Protocol);
    }
    let private = store.private_key().map_err(|_| SessionError::Identity)?;
    let params = NOISE_PATTERN.parse().map_err(|_| SessionError::Noise)?;
    let mut noise = snow::Builder::new(params)
        .prologue(&PRELUDE)
        .map_err(|_| SessionError::Noise)?
        .local_private_key(&private)
        .map_err(|_| SessionError::Noise)?
        .build_responder()
        .map_err(|_| SessionError::Noise)?;
    handshake_responder(&mut stream, &mut noise)?;
    let sas =
        authentication_string(noise.get_handshake_hash()).map_err(|_| SessionError::Protocol)?;
    let remote = hex(noise.get_remote_static().ok_or(SessionError::Noise)?);
    let mut transport = noise
        .into_transport_mode()
        .map_err(|_| SessionError::Noise)?;
    let Message::PairingBegin { transaction } = read_message(&mut stream, &mut transport)? else {
        return Err(SessionError::Protocol);
    };
    if let Ok(mut diagnostic_transaction) = diagnostic_transaction.lock() {
        *diagnostic_transaction = Some(transaction);
    }
    if std::time::Instant::now() >= deadline {
        return Err(close_pairing(
            &mut stream,
            &mut transport,
            transaction,
            deskkin_protocol::PairingCloseReason::Expired,
            SessionError::PairingTimeout,
        ));
    }
    if !reserve() {
        write_message(
            &mut stream,
            &mut transport,
            &Message::PairingClose {
                transaction,
                reason: deskkin_protocol::PairingCloseReason::PairingBusy,
            },
        )?;
        return Err(SessionError::SessionBusy);
    }
    let local_confirmed = confirm(transaction, sas);
    write_message(
        &mut stream,
        &mut transport,
        &Message::PairingDecision {
            transaction,
            decision: if local_confirmed {
                PairingDecision::Confirmed
            } else {
                PairingDecision::Rejected
            },
        },
    )?;
    if !local_confirmed {
        return Err(SessionError::PairingRejected);
    }
    if std::time::Instant::now() >= deadline {
        return Err(close_pairing(
            &mut stream,
            &mut transport,
            transaction,
            deskkin_protocol::PairingCloseReason::Expired,
            SessionError::PairingTimeout,
        ));
    }
    match read_message(&mut stream, &mut transport)? {
        Message::PairingDecision {
            transaction: t,
            decision: PairingDecision::Confirmed,
        } if t == transaction => {}
        Message::PairingDecision {
            transaction: t,
            decision: PairingDecision::Rejected,
        } if t == transaction => return Err(SessionError::PairingRejected),
        _ => return Err(SessionError::PairingIncomplete),
    }
    match read_message(&mut stream, &mut transport)? {
        Message::PairingPrepared { transaction: t } if t == transaction => {}
        _ => return Err(SessionError::Protocol),
    }
    let tx = hex(&transaction);
    if std::time::Instant::now() >= deadline {
        return Err(close_pairing(
            &mut stream,
            &mut transport,
            transaction,
            deskkin_protocol::PairingCloseReason::Expired,
            SessionError::PairingTimeout,
        ));
    }
    actor
        .set_peer(PeerState::Pending {
            remote_public_key: remote.clone(),
            pairing_transaction_id: tx.clone(),
        })
        .map_err(|_| {
            close_pairing(
                &mut stream,
                &mut transport,
                transaction,
                deskkin_protocol::PairingCloseReason::StoreFailed,
                SessionError::StoreFailed,
            )
        })?;
    write_message(
        &mut stream,
        &mut transport,
        &Message::PairingPrepared { transaction },
    )?;
    match read_message(&mut stream, &mut transport)? {
        Message::PairingCommit { transaction: t } if t == transaction => {}
        _ => return Err(SessionError::Protocol),
    }
    if std::time::Instant::now() >= deadline {
        return Err(close_pairing(
            &mut stream,
            &mut transport,
            transaction,
            deskkin_protocol::PairingCloseReason::Expired,
            SessionError::PairingTimeout,
        ));
    }
    actor
        .set_peer(PeerState::Committing {
            remote_public_key: remote.clone(),
            pairing_transaction_id: tx.clone(),
        })
        .map_err(|_| {
            close_pairing(
                &mut stream,
                &mut transport,
                transaction,
                deskkin_protocol::PairingCloseReason::StoreFailed,
                SessionError::StoreFailed,
            )
        })?;
    write_message(
        &mut stream,
        &mut transport,
        &Message::PairingCommitted { transaction },
    )?;
    match read_message(&mut stream, &mut transport)? {
        Message::PairingCommitted { transaction: t } if t == transaction => {}
        _ => return Err(SessionError::Protocol),
    }
    if std::time::Instant::now() >= deadline {
        return Err(close_pairing(
            &mut stream,
            &mut transport,
            transaction,
            deskkin_protocol::PairingCloseReason::Expired,
            SessionError::PairingTimeout,
        ));
    }
    write_message(
        &mut stream,
        &mut transport,
        &Message::PairingComplete { transaction },
    )?;
    let session_diagnostic = store.start_diagnostic(DiagnosticKind::ProtocolSession);
    let hello = match read_message(&mut stream, &mut transport) {
        Ok(hello) => hello,
        Err(error) => {
            if let Some(diagnostic) = session_diagnostic {
                diagnostic.finish(DiagnosticRecord::pre_session_failure(error));
            }
            return Err(error);
        }
    };
    let Message::Hello { session, .. } = hello else {
        if let Some(diagnostic) = session_diagnostic {
            diagnostic.finish(DiagnosticRecord::pre_session_failure(
                SessionError::Protocol,
            ));
        }
        return Err(SessionError::Protocol);
    };
    let (session, rejection) = match complete_responder_trust(&actor, remote, tx, hello) {
        Ok(completed) => completed,
        Err(error) => {
            if let Some(diagnostic) = session_diagnostic {
                diagnostic.finish(DiagnosticRecord::session_failure(session, error));
            }
            let reason = if matches!(error, SessionError::StoreFailed) {
                deskkin_protocol::PairingCloseReason::StoreFailed
            } else {
                deskkin_protocol::PairingCloseReason::Incomplete
            };
            return Err(close_pairing(
                &mut stream,
                &mut transport,
                transaction,
                reason,
                error,
            ));
        }
    };
    if let Some(reason) = rejection {
        let write_result = write_message(
            &mut stream,
            &mut transport,
            &Message::HelloReject { session, reason },
        );
        let error = rejection_error(reason);
        if let Some(diagnostic) = session_diagnostic {
            diagnostic.finish(DiagnosticRecord::session_failure(session, error));
        }
        write_result?;
        Err(error)
    } else {
        let result = write_hello_ack(&mut stream, &mut transport, session);
        if let Some(diagnostic) = session_diagnostic {
            diagnostic.finish(match result {
                Ok(()) => DiagnosticRecord::session(session),
                Err(error) => DiagnosticRecord::session_failure(session, error),
            });
        }
        result.map(|()| sas)
    }
}

fn complete_responder_trust(
    actor: &IdentityActor,
    remote_public_key: String,
    pairing_transaction_id: String,
    hello: Message,
) -> Result<([u8; 16], Option<HelloRejectReason>), SessionError> {
    let Message::Hello { session, .. } = hello else {
        return Err(SessionError::Protocol);
    };
    let rejection = negotiate_hello(hello).err().map(|(_, reason)| reason);
    actor
        .set_peer(PeerState::Paired {
            remote_public_key,
            pairing_transaction_id,
        })
        .map_err(|_| SessionError::StoreFailed)?;
    Ok((session, rejection))
}

fn close_pairing(
    stream: &mut TcpStream,
    transport: &mut snow::TransportState,
    transaction: [u8; 16],
    reason: deskkin_protocol::PairingCloseReason,
    error: SessionError,
) -> SessionError {
    let _ = write_message(
        stream,
        transport,
        &Message::PairingClose {
            transaction,
            reason,
        },
    );
    error
}

#[allow(clippy::too_many_lines)]
pub fn pair_initiator<F>(
    address: SocketAddr,
    store: &IdentityStore,
    session: [u8; 16],
    confirm: F,
) -> Result<[u8; 6], SessionError>
where
    F: FnOnce([u8; 16], [u8; 6]) -> bool + Send + 'static,
{
    pair_initiator_until(
        address,
        store,
        session,
        confirm,
        std::time::Instant::now() + Duration::from_mins(1),
    )
}

#[allow(clippy::too_many_lines)]
pub fn pair_initiator_until<F>(
    address: SocketAddr,
    store: &IdentityStore,
    session: [u8; 16],
    confirm: F,
    deadline: std::time::Instant,
) -> Result<[u8; 6], SessionError>
where
    F: FnOnce([u8; 16], [u8; 6]) -> bool + Send + 'static,
{
    let diagnostic = store.start_diagnostic(DiagnosticKind::ProtocolPairing);
    let diagnostic_transaction = Mutex::new(None);
    let result = pair_initiator_until_inner(
        address,
        store,
        session,
        confirm,
        deadline,
        &diagnostic_transaction,
    );
    let transaction = diagnostic_transaction.into_inner().ok().flatten();
    if let Some(diagnostic) = diagnostic {
        let record = if matches!(store.peer(), Ok(PeerState::Paired { .. })) {
            transaction.map_or_else(
                || DiagnosticRecord::pairing_failure_for(SessionError::PairingIncomplete),
                DiagnosticRecord::pairing,
            )
        } else {
            DiagnosticRecord::pairing_failure_for_transaction(
                result.err().unwrap_or(SessionError::PairingIncomplete),
                transaction,
            )
        };
        diagnostic.finish(record);
    }
    result
}

#[allow(clippy::too_many_lines)]
fn pair_initiator_until_inner<F>(
    address: SocketAddr,
    store: &IdentityStore,
    session: [u8; 16],
    confirm: F,
    deadline: std::time::Instant,
    diagnostic_transaction: &Mutex<Option<[u8; 16]>>,
) -> Result<[u8; 6], SessionError>
where
    F: FnOnce([u8; 16], [u8; 6]) -> bool + Send + 'static,
{
    let actor = IdentityActor::start(store.clone());
    if !address.ip().is_loopback() {
        return Err(SessionError::NonLoopback);
    }
    if store.peer().map_err(|_| SessionError::Identity)? != PeerState::Unpaired {
        return Err(SessionError::Identity);
    }
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))
        .map_err(|_| SessionError::Io)?;
    configure(&stream)?;
    write_all_deadline(&mut stream, &PRELUDE)?;
    let private = store.private_key().map_err(|_| SessionError::Identity)?;
    let params = NOISE_PATTERN.parse().map_err(|_| SessionError::Noise)?;
    let builder = snow::Builder::new(params);
    let mut noise = builder
        .prologue(&PRELUDE)
        .map_err(|_| SessionError::Noise)?
        .local_private_key(&private)
        .map_err(|_| SessionError::Noise)?
        .build_initiator()
        .map_err(|_| SessionError::Noise)?;
    handshake_initiator(&mut stream, &mut noise)?;
    let sas =
        authentication_string(noise.get_handshake_hash()).map_err(|_| SessionError::Protocol)?;
    let remote = hex(noise.get_remote_static().ok_or(SessionError::Noise)?);
    let mut random = Zeroizing::new(
        snow::Builder::new(NOISE_PATTERN.parse().map_err(|_| SessionError::Noise)?)
            .generate_keypair()
            .map_err(|_| SessionError::Noise)?
            .private,
    );
    let mut transaction = [0; 16];
    transaction.copy_from_slice(&random[..16]);
    random.zeroize();
    if let Ok(mut diagnostic_transaction) = diagnostic_transaction.lock() {
        *diagnostic_transaction = Some(transaction);
    }
    let mut transport = noise
        .into_transport_mode()
        .map_err(|_| SessionError::Noise)?;
    write_message(
        &mut stream,
        &mut transport,
        &Message::PairingBegin { transaction },
    )?;
    let local_confirmed = confirm(transaction, sas);
    let remote_confirmed = match read_message(&mut stream, &mut transport)? {
        Message::PairingDecision {
            transaction: t,
            decision: PairingDecision::Confirmed,
        } if t == transaction => true,
        Message::PairingDecision {
            transaction: t,
            decision: PairingDecision::Rejected,
        } if t == transaction => false,
        Message::PairingClose {
            transaction: t,
            reason: deskkin_protocol::PairingCloseReason::PairingBusy,
        } if t == transaction => return Err(SessionError::SessionBusy),
        Message::PairingClose {
            transaction: t,
            reason: deskkin_protocol::PairingCloseReason::Expired,
        } if t == transaction => return Err(SessionError::PairingTimeout),
        Message::PairingClose {
            transaction: t,
            reason: deskkin_protocol::PairingCloseReason::Rejected,
        } if t == transaction => return Err(SessionError::PairingRejected),
        Message::PairingClose {
            transaction: t,
            reason: deskkin_protocol::PairingCloseReason::StoreFailed,
        } if t == transaction => return Err(SessionError::StoreFailed),
        Message::PairingClose { transaction: t, .. } if t == transaction => {
            return Err(SessionError::PairingIncomplete);
        }
        _ => return Err(SessionError::Protocol),
    };
    write_message(
        &mut stream,
        &mut transport,
        &Message::PairingDecision {
            transaction,
            decision: if local_confirmed {
                PairingDecision::Confirmed
            } else {
                PairingDecision::Rejected
            },
        },
    )?;
    if !remote_confirmed || !local_confirmed {
        return Err(SessionError::PairingRejected);
    }
    if std::time::Instant::now() >= deadline {
        return Err(close_pairing(
            &mut stream,
            &mut transport,
            transaction,
            deskkin_protocol::PairingCloseReason::Expired,
            SessionError::PairingTimeout,
        ));
    }
    let tx = hex(&transaction);
    actor
        .set_peer(PeerState::Pending {
            remote_public_key: remote.clone(),
            pairing_transaction_id: tx.clone(),
        })
        .map_err(|_| {
            close_pairing(
                &mut stream,
                &mut transport,
                transaction,
                deskkin_protocol::PairingCloseReason::StoreFailed,
                SessionError::StoreFailed,
            )
        })?;
    write_message(
        &mut stream,
        &mut transport,
        &Message::PairingPrepared { transaction },
    )?;
    match read_message(&mut stream, &mut transport)? {
        Message::PairingPrepared { transaction: t } if t == transaction => {}
        _ => return Err(SessionError::Protocol),
    }
    if std::time::Instant::now() >= deadline {
        return Err(close_pairing(
            &mut stream,
            &mut transport,
            transaction,
            deskkin_protocol::PairingCloseReason::Expired,
            SessionError::PairingTimeout,
        ));
    }
    write_message(
        &mut stream,
        &mut transport,
        &Message::PairingCommit { transaction },
    )?;
    match read_message(&mut stream, &mut transport)? {
        Message::PairingCommitted { transaction: t } if t == transaction => {}
        _ => return Err(SessionError::Protocol),
    }
    actor
        .set_peer(PeerState::Committing {
            remote_public_key: remote.clone(),
            pairing_transaction_id: tx.clone(),
        })
        .map_err(|_| {
            close_pairing(
                &mut stream,
                &mut transport,
                transaction,
                deskkin_protocol::PairingCloseReason::StoreFailed,
                SessionError::StoreFailed,
            )
        })?;
    write_message(
        &mut stream,
        &mut transport,
        &Message::PairingCommitted { transaction },
    )?;
    match read_message(&mut stream, &mut transport)? {
        Message::PairingComplete { transaction: t } if t == transaction => {}
        _ => return Err(SessionError::Protocol),
    }
    if std::time::Instant::now() >= deadline {
        return Err(close_pairing(
            &mut stream,
            &mut transport,
            transaction,
            deskkin_protocol::PairingCloseReason::Expired,
            SessionError::PairingTimeout,
        ));
    }
    actor
        .set_peer(PeerState::Paired {
            remote_public_key: remote,
            pairing_transaction_id: tx,
        })
        .map_err(|_| {
            close_pairing(
                &mut stream,
                &mut transport,
                transaction,
                deskkin_protocol::PairingCloseReason::StoreFailed,
                SessionError::StoreFailed,
            )
        })?;
    let session_diagnostic = store.start_diagnostic(DiagnosticKind::ProtocolSession);
    let session_result = (|| {
        write_hello(&mut stream, &mut transport, session)?;
        match read_message(&mut stream, &mut transport)? {
            Message::HelloAck {
                session: s,
                selected_major: 1,
                selected_features,
                granted_permissions,
            } if s == session
                && selected_features == deskkin_protocol::AVAILABILITY_READ_V1
                && granted_permissions == deskkin_protocol::AVAILABILITY_READ_PERMISSION =>
            {
                Ok(sas)
            }
            Message::HelloReject { reason, .. } => Err(rejection_error(reason)),
            _ => Err(SessionError::Protocol),
        }
    })();
    if let Some(diagnostic) = session_diagnostic {
        diagnostic.finish(match session_result {
            Ok(_) => DiagnosticRecord::session(session),
            Err(error) => DiagnosticRecord::session_failure(session, error),
        });
    }
    session_result
}

struct PairingBusyGuard {
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}

impl PairingBusyGuard {
    fn start(listener: &TcpListener, store: &IdentityStore) -> Result<Self, SessionError> {
        let listener = listener.try_clone().map_err(|_| SessionError::Io)?;
        listener
            .set_nonblocking(true)
            .map_err(|_| SessionError::Io)?;
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let store = store.clone();
        let join = thread::spawn(move || {
            let mut peers: Vec<thread::JoinHandle<()>> = Vec::new();
            while !worker_stop.load(Ordering::Acquire) {
                let mut index = 0;
                while index < peers.len() {
                    if peers[index].is_finished() {
                        let peer: thread::JoinHandle<()> = peers.swap_remove(index);
                        let _ = peer.join();
                    } else {
                        index += 1;
                    }
                }
                match listener.accept() {
                    Ok((stream, _)) if peers.len() < 3 => {
                        let store = store.clone();
                        peers.push(thread::spawn(move || {
                            let _ = reject_pairing_busy(stream, &store);
                        }));
                    }
                    Ok((_stream, _)) => {}
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
            for peer in peers {
                let _ = peer.join();
            }
            let _ = listener.set_nonblocking(false);
        });
        Ok(Self {
            stop,
            join: Some(join),
        })
    }
}

impl Drop for PairingBusyGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn reject_pairing_busy(mut stream: TcpStream, store: &IdentityStore) -> Result<(), SessionError> {
    configure(&stream)?;
    let mut prelude = [0; 6];
    read_exact_deadline(
        &mut stream,
        &mut prelude,
        std::time::Instant::now() + Duration::from_secs(5),
    )?;
    if prelude != PRELUDE {
        return Err(SessionError::Protocol);
    }
    let private = store.private_key().map_err(|_| SessionError::Identity)?;
    let mut noise = snow::Builder::new(NOISE_PATTERN.parse().map_err(|_| SessionError::Noise)?)
        .prologue(&PRELUDE)
        .map_err(|_| SessionError::Noise)?
        .local_private_key(&private)
        .map_err(|_| SessionError::Noise)?
        .build_responder()
        .map_err(|_| SessionError::Noise)?;
    handshake_responder(&mut stream, &mut noise)?;
    let mut transport = noise
        .into_transport_mode()
        .map_err(|_| SessionError::Noise)?;
    let Message::PairingBegin { transaction } = read_message(&mut stream, &mut transport)? else {
        return Err(SessionError::Protocol);
    };
    write_message(
        &mut stream,
        &mut transport,
        &Message::PairingClose {
            transaction,
            reason: deskkin_protocol::PairingCloseReason::PairingBusy,
        },
    )
}
pub fn serve_one(
    listener: &TcpListener,
    store: &IdentityStore,
    result: AvailabilityResult,
) -> Result<(), SessionError> {
    let (stream, _) = listener.accept().map_err(|_| SessionError::Io)?;
    serve_stream(stream, store, result)
}

fn serve_stream(
    mut stream: TcpStream,
    store: &IdentityStore,
    result: AvailabilityResult,
) -> Result<(), SessionError> {
    serve_stream_admitted(&mut stream, store, result, None, None)
}

fn serve_stream_admitted(
    stream: &mut TcpStream,
    store: &IdentityStore,
    result: AvailabilityResult,
    admission: Option<&AtomicBool>,
    preauth_release: Option<Box<dyn FnOnce() + Send>>,
) -> Result<(), SessionError> {
    let diagnostic = store.start_diagnostic(DiagnosticKind::ProtocolSession);
    let diagnostic_session = Mutex::new(None);
    let session_result = serve_stream_admitted_inner(
        stream,
        store,
        result,
        admission,
        preauth_release,
        &diagnostic_session,
    );
    let session = diagnostic_session.into_inner().ok().flatten();
    let terminal_record = match (session_result, session) {
        (Ok(()), Some(session)) => (DiagnosticRecord::session(session), Ok(())),
        (Err(error), Some(session)) => (
            DiagnosticRecord::session_failure(session, error),
            Err(error),
        ),
        (Err(error), None) => (DiagnosticRecord::pre_session_failure(error), Err(error)),
        (Ok(()), None) => (
            DiagnosticRecord::pre_session_failure(SessionError::Protocol),
            Err(SessionError::Protocol),
        ),
    };
    if let Some(diagnostic) = diagnostic {
        diagnostic.finish(terminal_record.0);
    }
    terminal_record.1
}

fn serve_stream_admitted_inner(
    stream: &mut TcpStream,
    store: &IdentityStore,
    result: AvailabilityResult,
    admission: Option<&AtomicBool>,
    preauth_release: Option<Box<dyn FnOnce() + Send>>,
    diagnostic_session: &Mutex<Option<[u8; 16]>>,
) -> Result<(), SessionError> {
    configure(stream)?;
    let mut prelude = [0; 6];
    read_exact_deadline(
        stream,
        &mut prelude,
        std::time::Instant::now() + Duration::from_secs(5),
    )?;
    if prelude != PRELUDE {
        return Err(SessionError::Protocol);
    }
    let private = store.private_key().map_err(|_| SessionError::Identity)?;
    let params = NOISE_PATTERN.parse().map_err(|_| SessionError::Noise)?;
    let builder = snow::Builder::new(params);
    let mut noise = builder
        .prologue(&PRELUDE)
        .map_err(|_| SessionError::Noise)?
        .local_private_key(&private)
        .map_err(|_| SessionError::Noise)?
        .build_responder()
        .map_err(|_| SessionError::Noise)?;
    handshake_responder(stream, &mut noise)?;
    let generation = verify_pinned_peer(store, noise.get_remote_static())?;
    let mut transport = noise
        .into_transport_mode()
        .map_err(|_| SessionError::Noise)?;
    let hello = read_message(stream, &mut transport)?;
    let session = match negotiate_hello(hello) {
        Ok(session) => session,
        Err((session, reason)) => {
            write_message(
                stream,
                &mut transport,
                &Message::HelloReject { session, reason },
            )?;
            return Err(SessionError::Protocol);
        }
    };
    if let Ok(mut diagnostic_session) = diagnostic_session.lock() {
        *diagnostic_session = Some(session);
    }
    let _session_admission = if let Some(admission) = admission {
        if admission
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            write_message(
                stream,
                &mut transport,
                &Message::HelloReject {
                    session,
                    reason: HelloRejectReason::SessionBusy,
                },
            )?;
            return Err(SessionError::SessionBusy);
        }
        Some(SessionAdmission(admission))
    } else {
        None
    };
    {
        let _session_admission_gate = store
            .session_write_gate
            .lock()
            .map_err(|_| SessionError::Identity)?;
        if store
            .paired_generation()
            .map_err(|_| SessionError::Identity)?
            != generation
        {
            return Err(SessionError::Identity);
        }
        write_message(
            stream,
            &mut transport,
            &Message::HelloAck {
                session,
                selected_major: 1,
                selected_features: deskkin_protocol::AVAILABILITY_READ_V1,
                granted_permissions: deskkin_protocol::AVAILABILITY_READ_PERMISSION,
            },
        )?;
    }
    if let Some(release) = preauth_release {
        release();
    }
    run_availability_session(stream, store, result, generation, session, transport)
}

fn run_availability_session(
    stream: &mut TcpStream,
    store: &IdentityStore,
    result: AvailabilityResult,
    generation: u64,
    session: [u8; 16],
    transport: snow::TransportState,
) -> Result<(), SessionError> {
    let result =
        run_availability_session_inner(stream, store, result, generation, session, transport);
    store.finish_all_availability_diagnostics(
        session,
        result.as_ref().err().copied().unwrap_or(SessionError::Io),
    );
    result
}

fn run_availability_session_inner(
    stream: &mut TcpStream,
    store: &IdentityStore,
    result: AvailabilityResult,
    generation: u64,
    session: [u8; 16],
    transport: snow::TransportState,
) -> Result<(), SessionError> {
    let transport = Arc::new(Mutex::new(transport));
    let writer = SessionWriter::start(
        stream.try_clone().map_err(|_| SessionError::Io)?,
        transport.clone(),
        store.clone(),
        generation,
        session,
        result,
    );
    let mut last_request_id = 0_u32;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|_| SessionError::Io)?;
    loop {
        match read_message_shared(stream, &transport)? {
            Message::ReadAvailability {
                request_id,
                operation,
            } => {
                if !accept_request_id(&mut last_request_id, request_id) {
                    return Err(SessionError::Protocol);
                }
                if store
                    .paired_generation()
                    .map_err(|_| SessionError::Identity)?
                    != generation
                {
                    return Err(SessionError::Identity);
                }
                store.start_availability_diagnostic(operation);
                let response = Message::AvailabilityResult {
                    request_id,
                    operation,
                    result,
                };
                if let Err(error) = writer.enqueue_application(response) {
                    store.finish_availability_diagnostic(
                        operation,
                        DiagnosticRecord::availability_failure(session, operation, error),
                    );
                    return Err(error);
                }
            }
            Message::Ping => {
                writer.enqueue_control(WriterControl::Pong)?;
            }
            Message::Pong => {}
            Message::Close { reason } => {
                writer.enqueue_control(WriterControl::Close(reason))?;
                return writer.join();
            }
            _ => return Err(SessionError::Protocol),
        }
        if let Some(error) = writer.failure() {
            return Err(error);
        }
    }
}

fn read_message_shared(
    stream: &mut TcpStream,
    state: &Arc<Mutex<snow::TransportState>>,
) -> Result<Message, SessionError> {
    let encrypted = read_frame(stream, APPLICATION_FRAME_MAX)?;
    let mut plain = [0; APPLICATION_FRAME_MAX];
    let n = state
        .lock()
        .map_err(|_| SessionError::Noise)?
        .read_message(&encrypted, &mut plain)
        .map_err(|_| SessionError::Noise)?;
    Message::decode(&plain[..n]).map_err(|_| SessionError::Protocol)
}

fn write_message_shared(
    stream: &mut TcpStream,
    state: &Arc<Mutex<snow::TransportState>>,
    message: &Message,
) -> Result<(), SessionError> {
    let mut plain = [0; APPLICATION_FRAME_MAX];
    let encoded = message
        .encode(&mut plain)
        .map_err(|_| SessionError::Protocol)?;
    let mut encrypted = vec![0; APPLICATION_FRAME_MAX + 16];
    let written = state
        .lock()
        .map_err(|_| SessionError::Noise)?
        .write_message(encoded, &mut encrypted)
        .map_err(|_| SessionError::Noise)?;
    write_frame(stream, &encrypted[..written])
}

fn accept_request_id(last: &mut u32, request_id: u32) -> bool {
    if last.checked_add(1) == Some(request_id) {
        *last = request_id;
        true
    } else {
        false
    }
}

struct SessionWriteQueue {
    close: Option<Message>,
    pong: bool,
    ping: bool,
    application: std::collections::VecDeque<Message>,
}

impl SessionWriteQueue {
    fn new() -> Self {
        Self {
            close: None,
            pong: false,
            ping: false,
            application: std::collections::VecDeque::new(),
        }
    }
    fn enqueue_application(&mut self, message: Message) -> Result<(), SessionError> {
        if self.application.len() >= 8 {
            return Err(SessionError::QueueFull);
        }
        self.application.push_back(message);
        Ok(())
    }
    fn enqueue_close(&mut self, reason: deskkin_protocol::CloseReason) {
        self.close = Some(Message::Close { reason });
    }
    fn enqueue_pong(&mut self) {
        self.pong = true;
    }
    fn enqueue_ping_if_idle(&mut self) {
        if self.close.is_none() && !self.pong && self.application.is_empty() {
            self.ping = true;
        }
    }
    fn pop(&mut self) -> Option<Message> {
        self.close
            .take()
            .or_else(|| {
                self.pong.then(|| {
                    self.pong = false;
                    Message::Pong
                })
            })
            .or_else(|| {
                self.ping.then(|| {
                    self.ping = false;
                    Message::Ping
                })
            })
            .or_else(|| self.application.pop_front())
    }
}

#[derive(Clone, Copy)]
enum WriterControl {
    Close(deskkin_protocol::CloseReason),
    Pong,
    Stop,
}

struct SessionWriter {
    application: mpsc::SyncSender<Message>,
    control: mpsc::SyncSender<WriterControl>,
    failure: mpsc::Receiver<SessionError>,
    join: Mutex<Option<thread::JoinHandle<Result<(), SessionError>>>>,
    close: Arc<Mutex<Option<deskkin_protocol::CloseReason>>>,
}

impl SessionWriter {
    fn start(
        mut stream: TcpStream,
        transport: Arc<Mutex<snow::TransportState>>,
        store: IdentityStore,
        generation: u64,
        session: [u8; 16],
        availability: AvailabilityResult,
    ) -> Self {
        let (application, applications) = mpsc::sync_channel(8);
        let (control, controls) = mpsc::sync_channel(1);
        let (failure_sender, failure) = mpsc::sync_channel(1);
        let close = Arc::new(Mutex::new(None));
        let writer_close = close.clone();
        let cancellation_stream = stream.try_clone().ok();
        let join = thread::spawn(move || {
            let result = run_session_writer(
                &mut stream,
                &transport,
                &store,
                generation,
                session,
                availability,
                &applications,
                &controls,
                &writer_close,
            );
            if let Err(error) = result {
                let _ = failure_sender.try_send(error);
                if let Some(stream) = cancellation_stream {
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                }
            }
            result
        });
        Self {
            application,
            control,
            failure,
            join: Mutex::new(Some(join)),
            close,
        }
    }

    fn enqueue_application(&self, message: Message) -> Result<(), SessionError> {
        self.application
            .try_send(message)
            .map_err(|_| SessionError::QueueFull)
    }

    fn enqueue_control(&self, control: WriterControl) -> Result<(), SessionError> {
        if let WriterControl::Close(reason) = control {
            *self.close.lock().map_err(|_| SessionError::Io)? = Some(reason);
            return Ok(());
        }
        self.control
            .try_send(control)
            .map_err(|_| SessionError::QueueFull)
    }

    fn failure(&self) -> Option<SessionError> {
        self.failure.try_recv().ok()
    }

    fn join(&self) -> Result<(), SessionError> {
        let join = self
            .join
            .lock()
            .map_err(|_| SessionError::Io)?
            .take()
            .ok_or(SessionError::Io)?;
        join.join().map_err(|_| SessionError::Io)?
    }
}

impl Drop for SessionWriter {
    fn drop(&mut self) {
        let _ = self.control.try_send(WriterControl::Stop);
        if let Ok(join) = self.join.get_mut()
            && let Some(join) = join.take()
        {
            let _ = join.join();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_session_writer(
    stream: &mut TcpStream,
    transport: &Arc<Mutex<snow::TransportState>>,
    store: &IdentityStore,
    generation: u64,
    session: [u8; 16],
    availability: AvailabilityResult,
    applications: &mpsc::Receiver<Message>,
    controls: &mpsc::Receiver<WriterControl>,
    close: &Mutex<Option<deskkin_protocol::CloseReason>>,
) -> Result<(), SessionError> {
    let mut queue = SessionWriteQueue::new();
    let mut last_write = std::time::Instant::now();
    loop {
        if let Some(reason) = close.lock().map_err(|_| SessionError::Io)?.take() {
            queue.enqueue_close(reason);
        }
        while let Ok(control) = controls.try_recv() {
            match control {
                WriterControl::Close(reason) => queue.enqueue_close(reason),
                WriterControl::Pong => queue.enqueue_pong(),
                WriterControl::Stop => return Ok(()),
            }
        }
        while let Ok(message) = applications.try_recv() {
            queue.enqueue_application(message)?;
        }
        let Some(message) = queue.pop() else {
            match applications.recv_timeout(Duration::from_millis(10)) {
                Ok(message) => queue.enqueue_application(message)?,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if last_write.elapsed() >= Duration::from_secs(15) {
                        queue.enqueue_ping_if_idle();
                    }
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            }
            continue;
        };
        let operation = match message {
            Message::AvailabilityResult { operation, .. } => Some(operation),
            _ => None,
        };
        let _gate = store
            .session_write_gate
            .lock()
            .map_err(|_| SessionError::Identity)?;
        if store
            .paired_generation()
            .map_err(|_| SessionError::Identity)?
            != generation
        {
            return Err(SessionError::Identity);
        }
        write_message_shared(stream, transport, &message)?;
        last_write = std::time::Instant::now();
        if let Some(operation) = operation {
            store.finish_availability_diagnostic(
                operation,
                DiagnosticRecord::availability(session, operation, availability),
            );
        }
        if matches!(message, Message::Close { .. }) {
            return Ok(());
        }
    }
}

struct SessionAdmission<'a>(&'a AtomicBool);

impl Drop for SessionAdmission<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub fn run_host_runtime(
    address: SocketAddr,
    role_root: &Path,
    result: AvailabilityResult,
) -> Result<(), SessionError> {
    run_host_runtime_with_recording(address, role_root, result, RecordingMode::On)
}

pub fn run_host_runtime_with_recording(
    address: SocketAddr,
    role_root: &Path,
    result: AvailabilityResult,
    recording: RecordingMode,
) -> Result<(), SessionError> {
    if !address.ip().is_loopback() {
        return Err(SessionError::NonLoopback);
    }
    let startup = profile::managed_startup_barrier(role_root).map_err(|_| SessionError::Io)?;
    run_host_runtime_scoped(address, role_root, result, recording, None, startup, None)
        .map(|_| ())
        .map_err(|failure| failure.error)
}

pub fn run_private_lan_host_runtime_with_recording(
    address: SocketAddr,
    role_root: &Path,
    result: AvailabilityResult,
    recording: RecordingMode,
) -> Result<(), SessionError> {
    if !is_exact_private_lan_address(address) {
        return Err(SessionError::NonPrivateLan);
    }
    let startup = profile::managed_startup_barrier(role_root).map_err(|_| SessionError::Io)?;
    run_host_runtime_scoped(address, role_root, result, recording, None, startup, None)
        .map(|_| ())
        .map_err(|failure| failure.error)
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum HostExit {
    Stopped,
    Interrupted,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct HostRuntimeError {
    pub error: SessionError,
    pub stage: Operation,
}

impl From<SessionError> for HostRuntimeError {
    fn from(error: SessionError) -> Self {
        Self {
            error,
            stage: Operation::HostRuntimeStop,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_profile_host_runtime(
    address: SocketAddr,
    role_root: &Path,
    bind_mode: profile::BindMode,
    result: AvailabilityResult,
    recording: RecordingMode,
    launch: OwnerLaunchMetadata,
    startup: Option<File>,
    scenario_context: Option<String>,
) -> Result<HostExit, HostRuntimeError> {
    match bind_mode {
        profile::BindMode::Loopback if address.ip().is_loopback() => {}
        profile::BindMode::PrivateLan if is_exact_private_lan_address(address) => {}
        profile::BindMode::Loopback => {
            return Err(HostRuntimeError {
                error: SessionError::NonLoopback,
                stage: Operation::HostBind,
            });
        }
        profile::BindMode::PrivateLan => {
            return Err(HostRuntimeError {
                error: SessionError::NonPrivateLan,
                stage: Operation::HostBind,
            });
        }
    }
    run_host_runtime_scoped(
        address,
        role_root,
        result,
        recording,
        Some(launch),
        startup,
        scenario_context,
    )
}

fn run_host_runtime_scoped(
    address: SocketAddr,
    role_root: &Path,
    result: AvailabilityResult,
    recording: RecordingMode,
    launch: Option<OwnerLaunchMetadata>,
    startup: Option<File>,
    scenario_context: Option<String>,
) -> Result<HostExit, HostRuntimeError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|_| HostRuntimeError {
            error: SessionError::Io,
            stage: Operation::HostOwnerAcquire,
        })?;
    let (interrupt, terminate) = {
        let _runtime = runtime.enter();
        (
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).map_err(
                |_| HostRuntimeError {
                    error: SessionError::Io,
                    stage: Operation::HostOwnerAcquire,
                },
            )?,
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).map_err(
                |_| HostRuntimeError {
                    error: SessionError::Io,
                    stage: Operation::HostOwnerAcquire,
                },
            )?,
        )
    };
    let store = IdentityStore::new(role_root.join("identity"))
        .with_recording(recording)
        .with_scenario_context(scenario_context);
    store.peer().map_err(|_| HostRuntimeError {
        error: SessionError::Identity,
        stage: Operation::HostOwnerAcquire,
    })?;
    let actor = IdentityActor::start(store.clone());
    let control_root = role_root.join("control");
    let generation = new_control_id().map_err(|error| HostRuntimeError {
        error,
        stage: Operation::HostOwnerAcquire,
    })?;
    let owner_actor = actor.clone();
    let (done_sender, done_receiver) = mpsc::sync_channel(1);
    let (event_sender, event_receiver) = mpsc::channel();
    let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
    let owner_generation = generation.clone();
    let owner_control_root = control_root.clone();
    let startup_cancel = Arc::new(AtomicBool::new(false));
    let owner_startup_cancel = startup_cancel.clone();
    let owner = thread::spawn(move || {
        let result = owner_control::run_owner_control_for_runtime(
            &control_root,
            &owner_actor,
            &generation,
            owner_control::OwnerControlOptions {
                event_sender: Some(event_sender),
                launch,
                startup: None,
                ready: Some(ready_sender),
                startup_cancel: Some(owner_startup_cancel),
            },
        );
        let _ = done_sender.send(result);
    });
    if ready_receiver.recv_timeout(Duration::from_secs(2)).is_err() {
        startup_cancel.store(true, Ordering::Release);
        owner.join().map_err(|_| HostRuntimeError {
            error: SessionError::Io,
            stage: Operation::HostOwnerRelease,
        })?;
        return Err(HostRuntimeError {
            error: SessionError::Io,
            stage: Operation::HostOwnerAcquire,
        });
    }
    let runtime_result = runtime.block_on(host_accept_loop(HostLoop {
        address,
        store,
        result,
        done_receiver,
        event_receiver,
        owner_control_root: owner_control_root.clone(),
        owner_generation: owner_generation.clone(),
        startup,
        interrupt,
        terminate,
    }));
    if runtime_result.is_err() {
        startup_cancel.store(true, Ordering::Release);
    }
    owner.join().map_err(|_| HostRuntimeError {
        error: SessionError::Io,
        stage: Operation::HostOwnerRelease,
    })?;
    runtime_result
}

struct HostLoop {
    address: SocketAddr,
    store: IdentityStore,
    result: AvailabilityResult,
    done_receiver: mpsc::Receiver<std::io::Result<()>>,
    event_receiver: mpsc::Receiver<OwnerEvent>,
    owner_control_root: PathBuf,
    owner_generation: String,
    startup: Option<File>,
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
}

#[allow(clippy::too_many_lines)]
async fn host_accept_loop(loop_state: HostLoop) -> Result<HostExit, HostRuntimeError> {
    use std::collections::HashMap;
    use std::net::Shutdown;
    use std::sync::Arc;
    use tokio::sync::Semaphore;
    use tokio::task::JoinSet;
    let HostLoop {
        address,
        store,
        result,
        done_receiver,
        event_receiver,
        owner_control_root: control_root,
        owner_generation,
        mut startup,
        mut interrupt,
        mut terminate,
    } = loop_state;
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|_| HostRuntimeError {
            error: SessionError::Io,
            stage: Operation::HostBind,
        })?;
    startup.take();
    let mut listener = Some(listener);
    let mut signal_requested = false;
    let preauth = Arc::new(Semaphore::new(4));
    let authenticated = Arc::new(AtomicBool::new(false));
    let mut tasks = JoinSet::new();
    let (connection_done, connection_completed) = mpsc::channel();
    let mut connections: HashMap<u64, TcpStream> = HashMap::new();
    let mut next_connection_id = 0_u64;
    let mut revocation_ack = None;
    let mut shutdown_ack = None;
    let mut pairing_window: Option<OwnerPairingTask> = None;
    let mut active_pairing_task: Option<OwnerPairingTask> = None;
    // (consumed, reservation waiting to transfer, reserved task finished)
    let pairing_state = Arc::new(Mutex::new((false, false, false)));
    loop {
        while tasks.try_join_next().is_some() {}
        while let Ok(connection_id) = connection_completed.try_recv() {
            connections.remove(&connection_id);
        }
        while let Ok(event) = event_receiver.try_recv() {
            match event {
                OwnerEvent::IdentityRevoked { joined } => {
                    if let Some(task) = &pairing_window {
                        task.cancel();
                    }
                    if let Some(task) = &active_pairing_task {
                        task.cancel();
                    }
                    for stream in connections.values() {
                        let _ = stream.shutdown(Shutdown::Both);
                    }
                    revocation_ack = Some(joined);
                }
                OwnerEvent::PairingWindowOpen { task } => {
                    if pairing_window.is_some()
                        || active_pairing_task.is_some()
                        || store.peer().map_err(|_| SessionError::Identity)? != PeerState::Unpaired
                    {
                        task.finish(false);
                    } else {
                        if let Ok(mut state) = pairing_state.lock() {
                            *state = (false, false, false);
                        }
                        active_pairing_task.take();
                        task.waiting();
                        pairing_window = Some(task);
                    }
                }
                OwnerEvent::PairStart { task, .. } => task.finish(false),
                OwnerEvent::RuntimeShutdown { joined } => {
                    listener.take();
                    if let Some(task) = &pairing_window {
                        task.cancel();
                    }
                    if let Some(task) = &active_pairing_task {
                        task.cancel();
                    }
                    for stream in connections.values() {
                        let _ = stream.shutdown(Shutdown::Both);
                    }
                    shutdown_ack = Some(joined);
                }
            }
        }
        let mut expired_task = None;
        let pairing_is_active = {
            let mut pairing_state = pairing_state.lock().map_err(|_| SessionError::Identity)?;
            if pairing_state.1 && active_pairing_task.is_none() {
                active_pairing_task = pairing_window.take();
                pairing_state.1 = false;
            }
            if pairing_state.2 {
                active_pairing_task.take();
                *pairing_state = (false, false, false);
            }
            if !pairing_state.0
                && pairing_window
                    .as_ref()
                    .is_some_and(OwnerPairingTask::expired)
            {
                expired_task = pairing_window.take();
            }
            pairing_state.0 && !pairing_state.2
        };
        if let Some(task) = expired_task {
            task.finish(false);
        }
        if !pairing_is_active {
            active_pairing_task.take();
        }
        if revocation_ack.is_some()
            && connections.is_empty()
            && let Some(joined) = revocation_ack.take()
        {
            let _ = joined.send(());
        }
        if shutdown_ack.is_some()
            && connections.is_empty()
            && tasks.is_empty()
            && let Some(joined) = shutdown_ack.take()
        {
            let _ = joined.send(());
        }
        if let Ok(owner_result) = done_receiver.try_recv() {
            owner_result.map_err(|_| SessionError::Io)?;
            break;
        }
        let Some(listener) = listener.as_ref() else {
            tokio::time::sleep(Duration::from_millis(1)).await;
            continue;
        };
        let accepted = tokio::select! {
            _ = interrupt.recv(), if !signal_requested => {
                let response = call_owner_control(
                    &control_root,
                    &OwnerCommand::Shutdown {
                        owner_generation: owner_generation.clone(),
                    },
                ).map_err(|_| HostRuntimeError {
                    error: SessionError::Io,
                    stage: Operation::HostRuntimeStop,
                })?;
                if response != OwnerResponse::ShutdownAccepted {
                    return Err(SessionError::Io.into());
                }
                signal_requested = true;
                None
            }
            _ = terminate.recv(), if !signal_requested => {
                let response = call_owner_control(
                    &control_root,
                    &OwnerCommand::Shutdown {
                        owner_generation: owner_generation.clone(),
                    },
                ).map_err(|_| HostRuntimeError {
                    error: SessionError::Io,
                    stage: Operation::HostRuntimeStop,
                })?;
                if response != OwnerResponse::ShutdownAccepted {
                    return Err(SessionError::Io.into());
                }
                signal_requested = true;
                None
            }
            result = tokio::time::timeout(
                Duration::from_millis(10),
                listener.accept(),
            ) => Some(result),
        };
        let Some(accepted) = accepted else { continue };
        match accepted {
            Ok(Ok((stream, _))) => {
                let Ok(preauth_permit) = preauth.clone().try_acquire_owned() else {
                    store.record(&DiagnosticRecord::preauth_capacity());
                    continue;
                };
                let Ok(stream) = stream.into_std() else {
                    continue;
                };
                let _ = stream.set_nonblocking(false);
                if let Some(task) = pairing_window.clone() {
                    let Ok(cancellation_stream) = stream.try_clone() else {
                        task.finish(false);
                        continue;
                    };
                    let connection_id = next_connection_id;
                    next_connection_id = next_connection_id.wrapping_add(1);
                    connections.insert(connection_id, cancellation_stream);
                    let connection_done = connection_done.clone();
                    let store = store.clone();
                    let pairing_state = pairing_state.clone();
                    tasks.spawn(async move {
                        let _ = tokio::task::spawn_blocking(move || {
                            let _preauth = preauth_permit;
                            let confirmation = task.clone();
                            let reserved_this = Arc::new(AtomicBool::new(false));
                            let reservation = reserved_this.clone();
                            let state = pairing_state.clone();
                            let deadline = task.deadline();
                            let result = pair_responder_stream_reserved(
                                stream,
                                &store,
                                move |tx, sas| confirmation.confirm(tx, sas),
                                move || {
                                    let reserved = reserve_pairing_window(&state);
                                    reservation.store(reserved, Ordering::Release);
                                    reserved
                                },
                                deadline,
                            );
                            if reserved_this.load(Ordering::Acquire) {
                                let paired = matches!(store.peer(), Ok(PeerState::Paired { .. }));
                                task.finish(result.is_ok() || paired);
                                if let Ok(mut state) = pairing_state.lock() {
                                    state.2 = true;
                                }
                            }
                        })
                        .await;
                        let _ = connection_done.send(connection_id);
                    });
                    continue;
                }
                if pairing_state.lock().is_ok_and(|state| state.0) {
                    let Ok(cancellation_stream) = stream.try_clone() else {
                        continue;
                    };
                    let connection_id = next_connection_id;
                    next_connection_id = next_connection_id.wrapping_add(1);
                    connections.insert(connection_id, cancellation_stream);
                    let store = store.clone();
                    let connection_done = connection_done.clone();
                    tasks.spawn(async move {
                        let _ = tokio::task::spawn_blocking(move || {
                            let _preauth = preauth_permit;
                            let _ = reject_pairing_busy(stream, &store);
                        })
                        .await;
                        let _ = connection_done.send(connection_id);
                    });
                    continue;
                }
                let Ok(cancellation_stream) = stream.try_clone() else {
                    continue;
                };
                let connection_id = next_connection_id;
                next_connection_id = next_connection_id.wrapping_add(1);
                connections.insert(connection_id, cancellation_stream);
                let authenticated = authenticated.clone();
                let store = store.clone();
                let connection_done = connection_done.clone();
                tasks.spawn(async move {
                    let _ = tokio::task::spawn_blocking(move || {
                        let mut stream = stream;
                        serve_stream_admitted(
                            &mut stream,
                            &store,
                            result,
                            Some(&authenticated),
                            Some(Box::new(move || drop(preauth_permit))),
                        )
                    })
                    .await;
                    let _ = connection_done.send(connection_id);
                });
            }
            Ok(Err(_)) => return Err(SessionError::Io.into()),
            Err(_) => {}
        }
    }
    while tasks.join_next().await.is_some() {}
    Ok(if signal_requested {
        HostExit::Interrupted
    } else {
        HostExit::Stopped
    })
}

fn reserve_pairing_window(state: &Mutex<(bool, bool, bool)>) -> bool {
    state.lock().is_ok_and(|mut state| {
        if state.0 {
            false
        } else {
            state.0 = true;
            state.1 = true;
            true
        }
    })
}

fn negotiate_hello(hello: Message) -> Result<[u8; 16], ([u8; 16], HelloRejectReason)> {
    match hello {
        Message::Hello {
            session,
            protocol_majors,
            required_features,
            optional_features,
            requested_permissions,
        } => {
            if protocol_majors.0[0] & 2 == 0 {
                Err((session, HelloRejectReason::NoCommonVersion))
            } else if required_features
                .0
                .iter()
                .zip(deskkin_protocol::AVAILABILITY_READ_V1.0)
                .any(|(required, supported)| required & !supported != 0)
                || (required_features.0[0] | optional_features.0[0])
                    & deskkin_protocol::AVAILABILITY_READ_V1.0[0]
                    == 0
            {
                Err((session, HelloRejectReason::RequiredFeatureUnsupported))
            } else if requested_permissions.0[0]
                & deskkin_protocol::AVAILABILITY_READ_PERMISSION.0[0]
                == 0
            {
                Err((session, HelloRejectReason::PermissionDenied))
            } else {
                Ok(session)
            }
        }
        _ => Err(([0; 16], HelloRejectReason::RequiredFeatureUnsupported)),
    }
}

fn write_hello(
    stream: &mut TcpStream,
    transport: &mut snow::TransportState,
    session: [u8; 16],
) -> Result<(), SessionError> {
    write_message(
        stream,
        transport,
        &Message::Hello {
            session,
            protocol_majors: deskkin_protocol::PROTOCOL_MAJOR_1,
            required_features: deskkin_protocol::AVAILABILITY_READ_V1,
            optional_features: deskkin_protocol::Bits([0; 8]),
            requested_permissions: deskkin_protocol::AVAILABILITY_READ_PERMISSION,
        },
    )
}

fn write_hello_ack(
    stream: &mut TcpStream,
    transport: &mut snow::TransportState,
    session: [u8; 16],
) -> Result<(), SessionError> {
    write_message(
        stream,
        transport,
        &Message::HelloAck {
            session,
            selected_major: 1,
            selected_features: deskkin_protocol::AVAILABILITY_READ_V1,
            granted_permissions: deskkin_protocol::AVAILABILITY_READ_PERMISSION,
        },
    )
}

pub struct ClientSession {
    stream: TcpStream,
    transport: snow::TransportState,
    next_request_id: u32,
    identity: IdentityStore,
    generation: u64,
    session: [u8; 16],
}

impl ClientSession {
    pub fn connect(
        address: SocketAddr,
        store: &IdentityStore,
        session: [u8; 16],
    ) -> Result<Self, SessionError> {
        let diagnostic = store.start_diagnostic(DiagnosticKind::ProtocolSession);
        let result = Self::connect_inner(address, store, session);
        if let Some(diagnostic) = diagnostic {
            diagnostic.finish(match result.as_ref() {
                Ok(_) => DiagnosticRecord::session(session),
                Err(error) => DiagnosticRecord::session_failure(session, *error),
            });
        }
        result
    }

    /// Connects when the caller owns the complete session diagnostic span.
    ///
    /// The hosted simulator uses this entrypoint so one TCP session has one
    /// diagnostic owner through disconnect and shutdown.
    pub fn connect_with_external_diagnostics(
        address: SocketAddr,
        store: &IdentityStore,
        session: [u8; 16],
    ) -> Result<Self, SessionError> {
        Self::connect_inner(address, store, session)
    }

    fn connect_inner(
        address: SocketAddr,
        store: &IdentityStore,
        session: [u8; 16],
    ) -> Result<Self, SessionError> {
        if !address.ip().is_loopback() {
            return Err(SessionError::NonLoopback);
        }
        let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))
            .map_err(|_| SessionError::Io)?;
        configure(&stream)?;
        write_all_deadline(&mut stream, &PRELUDE)?;
        let private = store.private_key().map_err(|_| SessionError::Identity)?;
        let params = NOISE_PATTERN.parse().map_err(|_| SessionError::Noise)?;
        let builder = snow::Builder::new(params);
        let mut noise = builder
            .prologue(&PRELUDE)
            .map_err(|_| SessionError::Noise)?
            .local_private_key(&private)
            .map_err(|_| SessionError::Noise)?
            .build_initiator()
            .map_err(|_| SessionError::Noise)?;
        handshake_initiator(&mut stream, &mut noise)?;
        let generation = verify_pinned_peer(store, noise.get_remote_static())?;
        let mut transport = noise
            .into_transport_mode()
            .map_err(|_| SessionError::Noise)?;
        write_message(
            &mut stream,
            &mut transport,
            &Message::Hello {
                session,
                protocol_majors: deskkin_protocol::PROTOCOL_MAJOR_1,
                required_features: deskkin_protocol::AVAILABILITY_READ_V1,
                optional_features: deskkin_protocol::Bits([0; 8]),
                requested_permissions: deskkin_protocol::AVAILABILITY_READ_PERMISSION,
            },
        )?;
        match read_message(&mut stream, &mut transport)? {
            Message::HelloAck {
                session: s,
                selected_major: 1,
                selected_features,
                granted_permissions,
            } if s == session
                && selected_features == deskkin_protocol::AVAILABILITY_READ_V1
                && granted_permissions == deskkin_protocol::AVAILABILITY_READ_PERMISSION => {}
            Message::HelloReject {
                session: rejected_session,
                reason,
            } if rejected_session == session => return Err(rejection_error(reason)),
            _ => return Err(SessionError::Protocol),
        }
        Ok(Self {
            stream,
            transport,
            next_request_id: 0,
            identity: store.clone(),
            generation,
            session,
        })
    }

    pub fn read_availability(
        &mut self,
        operation: [u8; 16],
    ) -> Result<AvailabilityResult, SessionError> {
        let diagnostic = self
            .identity
            .start_diagnostic(DiagnosticKind::AvailabilityRead);
        let result = self.read_availability_inner(operation);
        if let Some(diagnostic) = diagnostic {
            diagnostic.finish(match result.as_ref() {
                Ok(result) => DiagnosticRecord::availability(self.session(), operation, *result),
                Err(error) => {
                    DiagnosticRecord::availability_failure(self.session(), operation, *error)
                }
            });
        }
        result
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    const fn session(&self) -> [u8; 16] {
        self.session
    }

    fn read_availability_inner(
        &mut self,
        operation: [u8; 16],
    ) -> Result<AvailabilityResult, SessionError> {
        if self
            .identity
            .paired_generation()
            .map_err(|_| SessionError::Identity)?
            != self.generation
        {
            return Err(SessionError::Identity);
        }
        let request_id = self
            .next_request_id
            .checked_add(1)
            .ok_or(SessionError::Protocol)?;
        self.next_request_id = request_id;
        write_message(
            &mut self.stream,
            &mut self.transport,
            &Message::ReadAvailability {
                request_id,
                operation,
            },
        )?;
        self.stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .map_err(|_| SessionError::Io)?;
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            match read_message_until(&mut self.stream, &mut self.transport, deadline)? {
                Message::AvailabilityResult {
                    request_id: response_id,
                    operation: o,
                    result,
                } if response_id == request_id && o == operation => {
                    if self
                        .identity
                        .paired_generation()
                        .map_err(|_| SessionError::Identity)?
                        != self.generation
                    {
                        return Err(SessionError::Identity);
                    }
                    return Ok(result);
                }
                Message::Ping => {
                    write_message(&mut self.stream, &mut self.transport, &Message::Pong)?;
                }
                Message::Pong => {}
                _ => return Err(SessionError::Protocol),
            }
        }
    }

    pub fn close(mut self) -> Result<(), SessionError> {
        write_message(
            &mut self.stream,
            &mut self.transport,
            &Message::Close {
                reason: deskkin_protocol::CloseReason::Normal,
            },
        )
    }
}

fn accept_pairing_connection(
    listener: &TcpListener,
    window: Duration,
) -> Result<(TcpStream, SocketAddr), SessionError> {
    let deadline = std::time::Instant::now() + window;
    listener
        .set_nonblocking(true)
        .map_err(|_| SessionError::Io)?;
    let result = loop {
        match listener.accept() {
            Ok(connection) => break Ok(connection),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() >= deadline {
                    break Err(SessionError::PairingTimeout);
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => break Err(SessionError::Io),
        }
    };
    listener
        .set_nonblocking(false)
        .map_err(|_| SessionError::Io)?;
    result
}

fn rejection_error(reason: HelloRejectReason) -> SessionError {
    match reason {
        HelloRejectReason::NoCommonVersion | HelloRejectReason::RequiredFeatureUnsupported => {
            SessionError::Incompatible
        }
        HelloRejectReason::PermissionDenied => SessionError::AuthorizationDenied,
        HelloRejectReason::SessionBusy => SessionError::SessionBusy,
    }
}

pub fn read_once(
    address: SocketAddr,
    store: &IdentityStore,
    session: [u8; 16],
    operation: [u8; 16],
) -> Result<AvailabilityResult, SessionError> {
    let mut client = ClientSession::connect(address, store, session)?;
    let result = client.read_availability(operation)?;
    client.close()?;
    Ok(result)
}

fn configure(stream: &TcpStream) -> Result<(), SessionError> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| SessionError::Io)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|_| SessionError::Io)
}
fn handshake_initiator(
    stream: &mut TcpStream,
    state: &mut snow::HandshakeState,
) -> Result<(), SessionError> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut out = [0; 1024];
    let n = state
        .write_message(&[], &mut out)
        .map_err(|_| SessionError::Noise)?;
    write_frame_until(stream, &out[..n], deadline)?;
    let input = read_frame_until(stream, 1024, deadline)?;
    state
        .read_message(&input, &mut out)
        .map_err(|_| SessionError::Noise)?;
    let n = state
        .write_message(&[], &mut out)
        .map_err(|_| SessionError::Noise)?;
    write_frame_until(stream, &out[..n], deadline)
}
fn handshake_responder(
    stream: &mut TcpStream,
    state: &mut snow::HandshakeState,
) -> Result<(), SessionError> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut out = [0; 1024];
    let input = read_frame_until(stream, 1024, deadline)?;
    state
        .read_message(&input, &mut out)
        .map_err(|_| SessionError::Noise)?;
    let n = state
        .write_message(&[], &mut out)
        .map_err(|_| SessionError::Noise)?;
    write_frame_until(stream, &out[..n], deadline)?;
    let input = read_frame_until(stream, 1024, deadline)?;
    state
        .read_message(&input, &mut out)
        .map_err(|_| SessionError::Noise)?;
    Ok(())
}
fn write_message(
    stream: &mut TcpStream,
    state: &mut snow::TransportState,
    message: &Message,
) -> Result<(), SessionError> {
    let mut plain = [0; 256];
    let encoded = message
        .encode(&mut plain)
        .map_err(|_| SessionError::Protocol)?;
    let mut encrypted = [0; 512];
    let n = state
        .write_message(encoded, &mut encrypted)
        .map_err(|_| SessionError::Noise)?;
    write_frame(stream, &encrypted[..n])
}
fn read_message(
    stream: &mut TcpStream,
    state: &mut snow::TransportState,
) -> Result<Message, SessionError> {
    let encrypted = read_frame(stream, APPLICATION_FRAME_MAX)?;
    let mut plain = [0; APPLICATION_FRAME_MAX];
    let n = state
        .read_message(&encrypted, &mut plain)
        .map_err(|_| SessionError::Noise)?;
    Message::decode(&plain[..n]).map_err(|_| SessionError::Protocol)
}

fn read_message_until(
    stream: &mut TcpStream,
    state: &mut snow::TransportState,
    deadline: std::time::Instant,
) -> Result<Message, SessionError> {
    let encrypted = read_frame_until(stream, APPLICATION_FRAME_MAX, deadline)?;
    let mut plain = [0; APPLICATION_FRAME_MAX];
    let n = state
        .read_message(&encrypted, &mut plain)
        .map_err(|_| SessionError::Noise)?;
    Message::decode(&plain[..n]).map_err(|_| SessionError::Protocol)
}
fn write_frame(stream: &mut TcpStream, value: &[u8]) -> Result<(), SessionError> {
    let length = encode_frame_length(value.len()).map_err(|_| SessionError::Protocol)?;
    let mut frame = Vec::with_capacity(2 + value.len());
    frame.extend_from_slice(&length);
    frame.extend_from_slice(value);
    write_all_deadline(stream, &frame)
}
fn read_frame(stream: &mut TcpStream, max: usize) -> Result<Vec<u8>, SessionError> {
    let timeout = stream
        .read_timeout()
        .map_err(|_| SessionError::Io)?
        .unwrap_or(Duration::from_secs(2));
    read_frame_until(stream, max, std::time::Instant::now() + timeout)
}

fn read_frame_until(
    stream: &mut TcpStream,
    max: usize,
    deadline: std::time::Instant,
) -> Result<Vec<u8>, SessionError> {
    let mut length = [0; 2];
    read_exact_deadline(stream, &mut length, deadline)?;
    let length = decode_frame_length(length);
    if length > max {
        return Err(SessionError::FrameOversize);
    }
    let mut value = vec![0; length];
    read_exact_deadline(stream, &mut value, deadline)?;
    Ok(value)
}

fn read_exact_deadline(
    stream: &mut TcpStream,
    mut buffer: &mut [u8],
    mut deadline: std::time::Instant,
) -> Result<(), SessionError> {
    while !buffer.is_empty() {
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .ok_or(SessionError::Timeout)?;
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|_| SessionError::Io)?;
        match stream.read(buffer) {
            Ok(0) => return Err(SessionError::EndOfStream),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                return Err(SessionError::Timeout);
            }
            Err(_) => return Err(SessionError::Io),
            Ok(read) => {
                buffer = &mut buffer[read..];
                deadline = deadline.min(std::time::Instant::now() + Duration::from_secs(2));
            }
        }
    }
    Ok(())
}

fn write_all_deadline(stream: &mut TcpStream, buffer: &[u8]) -> Result<(), SessionError> {
    let timeout = stream
        .write_timeout()
        .map_err(|_| SessionError::Io)?
        .unwrap_or(Duration::from_secs(2));
    write_all_until(stream, buffer, std::time::Instant::now() + timeout)
}

fn write_frame_until(
    stream: &mut TcpStream,
    value: &[u8],
    deadline: std::time::Instant,
) -> Result<(), SessionError> {
    let length = encode_frame_length(value.len()).map_err(|_| SessionError::Protocol)?;
    let mut frame = Vec::with_capacity(2 + value.len());
    frame.extend_from_slice(&length);
    frame.extend_from_slice(value);
    write_all_until(stream, &frame, deadline)
}

fn write_all_until(
    stream: &mut TcpStream,
    mut buffer: &[u8],
    deadline: std::time::Instant,
) -> Result<(), SessionError> {
    let deadline = deadline.min(std::time::Instant::now() + Duration::from_secs(2));
    while !buffer.is_empty() {
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .ok_or(SessionError::Timeout)?;
        stream
            .set_write_timeout(Some(remaining))
            .map_err(|_| SessionError::Io)?;
        match stream.write(buffer) {
            Ok(0) => return Err(SessionError::EndOfStream),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                return Err(SessionError::Timeout);
            }
            Err(_) => return Err(SessionError::Io),
            Ok(written) => buffer = &buffer[written..],
        }
    }
    Ok(())
}
fn hex(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(char::from(H[usize::from(b >> 4)]));
        out.push(char::from(H[usize::from(b & 15)]));
    }
    out
}
fn validate_hex(value: &str, bytes: usize) -> Result<(), StoreError> {
    if value.len() != bytes * 2
        || !value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(StoreError::Invalid);
    }
    Ok(())
}
fn decode_hex(value: &str) -> Result<Vec<u8>, StoreError> {
    validate_hex(value, value.len() / 2)?;
    (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).map_err(|_| StoreError::Invalid))
        .collect()
}

fn validate_peer(peer: &PeerState) -> Result<(), StoreError> {
    match peer {
        PeerState::Unpaired => Ok(()),
        PeerState::Pending {
            remote_public_key,
            pairing_transaction_id,
        }
        | PeerState::Committing {
            remote_public_key,
            pairing_transaction_id,
        }
        | PeerState::Paired {
            remote_public_key,
            pairing_transaction_id,
        } => {
            validate_hex(remote_public_key, 32)?;
            validate_hex(pairing_transaction_id, 16)
        }
        PeerState::Revoking {
            remote_public_key,
            previous_pairing_transaction_id,
        } => {
            validate_hex(remote_public_key, 32)?;
            validate_hex(previous_pairing_transaction_id, 16)
        }
    }
}

fn valid_peer_transition(current: &PeerState, next: &PeerState) -> bool {
    match (current, next) {
        (PeerState::Unpaired, PeerState::Pending { .. }) => true,
        (
            PeerState::Pending {
                remote_public_key: current_key,
                pairing_transaction_id: current_transaction,
            },
            PeerState::Committing {
                remote_public_key: next_key,
                pairing_transaction_id: next_transaction,
            },
        )
        | (
            PeerState::Committing {
                remote_public_key: current_key,
                pairing_transaction_id: current_transaction,
            },
            PeerState::Paired {
                remote_public_key: next_key,
                pairing_transaction_id: next_transaction,
            },
        ) => current_key == next_key && current_transaction == next_transaction,
        _ => false,
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

fn verify_pinned_peer(
    store: &IdentityStore,
    remote_static: Option<&[u8]>,
) -> Result<u64, SessionError> {
    let remote = remote_static.ok_or(SessionError::Noise)?;
    let identity = store.read().map_err(|_| SessionError::Identity)?;
    let expected = match &identity.peer {
        PeerState::Paired {
            remote_public_key, ..
        } => decode_hex(remote_public_key).map_err(|_| SessionError::Identity)?,
        _ => return Err(SessionError::Identity),
    };
    if constant_time_eq(remote, &expected) {
        Ok(identity.generation)
    } else {
        Err(SessionError::Identity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;
    use std::os::unix::fs::symlink;
    use std::thread;
    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("deskkin-host-{name}-{}", std::process::id()))
    }
    #[test]
    fn identity_is_explicit_private_and_exact_unpair() {
        let root = temp("identity");
        let _ = fs::remove_dir_all(&root);
        let s = IdentityStore::new(root.clone());
        assert_eq!(s.public_key(), Err(StoreError::Missing));
        let public = s.init().unwrap();
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(root.join("identity-v1.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let remote = "11".repeat(32);
        let transaction = "22".repeat(16);
        s.set_peer(PeerState::Pending {
            remote_public_key: remote.clone(),
            pairing_transaction_id: transaction.clone(),
        })
        .unwrap();
        s.set_peer(PeerState::Committing {
            remote_public_key: remote.clone(),
            pairing_transaction_id: transaction.clone(),
        })
        .unwrap();
        s.set_peer(PeerState::Paired {
            remote_public_key: remote,
            pairing_transaction_id: transaction,
        })
        .unwrap();
        assert_eq!(s.unpair(&"33".repeat(32)), Err(StoreError::PeerMismatch));
        s.unpair(&"11".repeat(32)).unwrap();
        assert_eq!(s.peer().unwrap(), PeerState::Unpaired);
        assert_eq!(s.public_key().unwrap(), public);
    }
    #[test]
    fn revoking_publication_recovers_exactly_without_generation_increment() {
        let root = temp("revoking-recovery");
        let _ = fs::remove_dir_all(&root);
        let store = IdentityStore::new(root);
        store.init().unwrap();
        let remote = "44".repeat(32);
        let transaction = "55".repeat(16);
        store
            .set_peer(PeerState::Pending {
                remote_public_key: remote.clone(),
                pairing_transaction_id: transaction.clone(),
            })
            .unwrap();
        store
            .set_peer(PeerState::Committing {
                remote_public_key: remote.clone(),
                pairing_transaction_id: transaction.clone(),
            })
            .unwrap();
        store
            .set_peer(PeerState::Paired {
                remote_public_key: remote.clone(),
                pairing_transaction_id: transaction,
            })
            .unwrap();
        let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = store.unpair_with_hook(&remote, || -> fn() -> bool {
                panic!("after revoking sync")
            });
        }));
        assert!(interrupted.is_err());
        assert!(matches!(store.peer().unwrap(), PeerState::Revoking { .. }));
        let generation = store.read().unwrap().generation;
        store.unpair(&remote).unwrap();
        assert_eq!(store.peer().unwrap(), PeerState::Unpaired);
        assert_eq!(store.read().unwrap().generation, generation);
    }
    #[test]
    fn revoking_parent_sync_failure_invalidates_and_requires_exact_recovery() {
        let root = temp("revoked-recovery-required");
        let _ = fs::remove_dir_all(&root);
        let store = IdentityStore::new(root.clone());
        store.init().unwrap();
        let remote = "45".repeat(32);
        let transaction = "56".repeat(16);
        store
            .set_peer(PeerState::Pending {
                remote_public_key: remote.clone(),
                pairing_transaction_id: transaction.clone(),
            })
            .unwrap();
        store
            .set_peer(PeerState::Committing {
                remote_public_key: remote.clone(),
                pairing_transaction_id: transaction.clone(),
            })
            .unwrap();
        store
            .set_peer(PeerState::Paired {
                remote_public_key: remote.clone(),
                pairing_transaction_id: transaction,
            })
            .unwrap();
        let invalidated = std::cell::Cell::new(false);
        let moved = root.with_extension("during-sync");
        let _ = fs::remove_dir_all(&moved);
        let result = store.unpair_with_hook(&remote, || {
            invalidated.set(true);
            fs::rename(&root, &moved).unwrap();
            || true
        });
        fs::rename(&moved, &root).unwrap();
        assert!(invalidated.get());
        assert_eq!(result, Err(StoreError::RevokedRecoveryRequired));
        assert!(matches!(store.peer().unwrap(), PeerState::Revoking { .. }));
        store.unpair(&remote).unwrap();
        assert_eq!(store.peer().unwrap(), PeerState::Unpaired);
    }
    #[test]
    fn identity_lock_timeout_does_not_enter_publication() {
        let root = temp("identity-lock-timeout");
        let _ = fs::remove_dir_all(&root);
        let store = IdentityStore::new(root.clone());
        store.init().unwrap();
        let lock = store.lock_file().unwrap();
        lock.lock().unwrap();
        assert_eq!(
            store.set_peer(PeerState::Pending {
                remote_public_key: "66".repeat(32),
                pairing_transaction_id: "77".repeat(16),
            }),
            Err(StoreError::LockTimeout)
        );
        assert!(!root.join(".identity.tmp").exists());
        File::unlock(&lock).unwrap();
        assert_eq!(store.peer().unwrap(), PeerState::Unpaired);
    }
    #[test]
    fn real_loopback_noise_availability() {
        let host_root = temp("e2e-host");
        let client_root = temp("e2e-client");
        let _ = fs::remove_dir_all(&host_root);
        let _ = fs::remove_dir_all(&client_root);
        let host = IdentityStore::new(host_root);
        let client = IdentityStore::new(client_root);
        host.init().unwrap();
        client.init().unwrap();
        let pairing_listener = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
        let pairing_address = pairing_listener.local_addr().unwrap();
        let host_for_pairing = host.clone();
        let pairing_join = thread::spawn(move || {
            pair_responder(&pairing_listener, &host_for_pairing, |_, _| true)
        });
        let initiator_sas = pair_initiator(pairing_address, &client, [2; 16], |_, _| true).unwrap();
        let responder_sas = pairing_join.join().unwrap().unwrap();
        assert_eq!(initiator_sas, responder_sas);
        let listener = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = listener.local_addr().unwrap();
        let serving_host = host.clone();
        let join = thread::spawn(move || {
            serve_one(&listener, &serving_host, AvailabilityResult::Available)
        });
        let mut session = ClientSession::connect(address, &client, [3; 16]).unwrap();
        assert_eq!(
            session.read_availability([4; 16]).unwrap(),
            AvailabilityResult::Available
        );
        assert_eq!(
            session.read_availability([5; 16]).unwrap(),
            AvailabilityResult::Available
        );
        session.close().unwrap();
        join.join().unwrap().unwrap();
    }

    #[test]
    fn stalled_recorder_lock_does_not_change_protocol_outcome() {
        let host_role = temp("recorder-stall-host-role");
        let client_role = temp("recorder-stall-client-role");
        let _ = fs::remove_dir_all(&host_role);
        let _ = fs::remove_dir_all(&client_role);
        let host = IdentityStore::new(host_role.join("identity"));
        let client = IdentityStore::new_for_role(
            client_role.join("identity"),
            ResourceRole::DeviceSimulator,
        );
        host.init().unwrap();
        client.init().unwrap();
        let pairing_listener = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
        let pairing_address = pairing_listener.local_addr().unwrap();
        let host_for_pairing = host.clone();
        let pairing_join = thread::spawn(move || {
            pair_responder(&pairing_listener, &host_for_pairing, |_, _| true)
        });
        pair_initiator(pairing_address, &client, [71; 16], |_, _| true).unwrap();
        pairing_join.join().unwrap().unwrap();
        thread::sleep(Duration::from_millis(50));

        let recorder_lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(host_role.join("store.lock"))
            .unwrap();
        File::lock(&recorder_lock).unwrap();
        let listener = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = listener.local_addr().unwrap();
        let serving_host = host.clone();
        let join = thread::spawn(move || {
            serve_one(&listener, &serving_host, AvailabilityResult::Available)
        });
        let started = std::time::Instant::now();
        let mut session = ClientSession::connect(address, &client, [72; 16]).unwrap();
        assert_eq!(
            session.read_availability([73; 16]).unwrap(),
            AvailabilityResult::Available
        );
        session.close().unwrap();
        join.join().unwrap().unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
        File::unlock(&recorder_lock).unwrap();
    }

    #[test]
    fn pinned_host_rejects_a_changed_device_static_key() {
        let host_root = temp("changed-key-host");
        let client_root = temp("changed-key-client");
        let attacker_root = temp("changed-key-attacker");
        let _ = fs::remove_dir_all(&host_root);
        let _ = fs::remove_dir_all(&client_root);
        let _ = fs::remove_dir_all(&attacker_root);
        let host = IdentityStore::new(host_root);
        let client = IdentityStore::new(client_root);
        let attacker = IdentityStore::new(attacker_root);
        let host_public = host.init().unwrap();
        client.init().unwrap();
        attacker.init().unwrap();
        let pairing_listener = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
        let pairing_address = pairing_listener.local_addr().unwrap();
        let host_for_pairing = host.clone();
        let pairing_join = thread::spawn(move || {
            pair_responder(&pairing_listener, &host_for_pairing, |_, _| true)
        });
        pair_initiator(pairing_address, &client, [20; 16], |_, _| true).unwrap();
        pairing_join.join().unwrap().unwrap();
        attacker
            .set_peer(PeerState::Pending {
                remote_public_key: host_public.clone(),
                pairing_transaction_id: "ab".repeat(16),
            })
            .unwrap();
        attacker
            .set_peer(PeerState::Committing {
                remote_public_key: host_public.clone(),
                pairing_transaction_id: "ab".repeat(16),
            })
            .unwrap();
        attacker
            .set_peer(PeerState::Paired {
                remote_public_key: host_public,
                pairing_transaction_id: "ab".repeat(16),
            })
            .unwrap();

        let listener = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = listener.local_addr().unwrap();
        let join =
            thread::spawn(move || serve_one(&listener, &host, AvailabilityResult::Available));
        assert!(ClientSession::connect(address, &attacker, [21; 16]).is_err());
        assert!(matches!(join.join().unwrap(), Err(SessionError::Identity)));
    }
    #[test]
    fn failed_session_publishes_closed_privacy_safe_diagnostic() {
        let role = temp("failed-session-diagnostic");
        let _ = fs::remove_dir_all(&role);
        let store = IdentityStore::new(role.join("identity"));
        store.init().unwrap();
        let probe = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = probe.local_addr().unwrap();
        drop(probe);
        assert!(ClientSession::connect(address, &store, [91; 16]).is_err());
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let run = loop {
            let found = role.join("diagnostics").exists().then(|| {
                fs::read_dir(role.join("diagnostics"))
                    .unwrap()
                    .filter_map(Result::ok)
                    .filter(|entry| {
                        entry
                            .path()
                            .extension()
                            .is_some_and(|value| value == "json")
                    })
                    .filter_map(|entry| fs::read(entry.path()).ok())
                    .filter_map(|bytes| serde_json::from_slice::<DiagnosticRun>(&bytes).ok())
                    .find(|run| {
                        run.terminal
                            && run.records.iter().any(|record| {
                                record.operation == Operation::ProtocolNegotiate
                                    && record.status == OperationStatus::Error
                                    && record.error_type
                                        == Some(local_run_recorder::ErrorType::ConnectionLost)
                            })
                    })
            });
            if let Some(Some(run)) = found {
                break run;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "terminal failed-session diagnostic was not published"
            );
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(run.completeness, Completeness::Complete);
        assert!(run.terminal);
        assert_eq!(run.records[0].operation, Operation::TransportAccept);
        assert_eq!(run.records[0].parent_operation_id, None);
        assert!(run.records[0].duration_ms.is_some());
        assert!(
            run.records[1..]
                .iter()
                .all(|record| record.parent_operation_id == Some(1))
        );
        let encoded = serde_json::to_string(&run).unwrap();
        assert!(!encoded.contains("authentication"));
        assert!(!encoded.contains("local_private_key"));
        assert!(!encoded.contains(&address.to_string()));
        assert!(!encoded.contains("\"pid\""));
        assert!(!encoded.contains("start_ticks"));
    }
    #[test]
    fn diagnostic_span_is_durable_before_terminal_completion() {
        let role = temp("diagnostic-span-lifecycle");
        let _ = fs::remove_dir_all(&role);
        let store = IdentityStore::new(role.join("identity"));
        let span = store
            .start_diagnostic(DiagnosticKind::ProtocolSession)
            .unwrap();
        let run_id = span.run_id.clone();
        let path = role.join(format!("diagnostics/{run_id}.json"));
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !path.exists() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let partial: DiagnosticRun = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(!partial.terminal);
        assert_eq!(partial.completeness, Completeness::Partial);
        assert!(
            partial
                .records
                .iter()
                .all(|record| record.status == OperationStatus::InProgress)
        );

        span.finish(DiagnosticRecord::session([77; 16]));
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let terminal: DiagnosticRun =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            if terminal.terminal {
                assert_eq!(terminal.run_id, run_id);
                assert_eq!(terminal.completeness, Completeness::Complete);
                assert!(
                    terminal
                        .records
                        .iter()
                        .all(|record| record.duration_ms.is_some())
                );
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    fn pairing_reject_and_window_expiry_persist_no_peer() {
        let host_root = temp("pair-reject-host");
        let client_root = temp("pair-reject-client");
        let _ = fs::remove_dir_all(&host_root);
        let _ = fs::remove_dir_all(&client_root);
        let host = IdentityStore::new(host_root);
        let client = IdentityStore::new(client_root);
        host.init().unwrap();
        client.init().unwrap();
        let listener = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = listener.local_addr().unwrap();
        let reject_host = host.clone();
        let server = thread::spawn(move || pair_responder(&listener, &reject_host, |_, _| false));
        assert!(pair_initiator(address, &client, [16; 16], |_, _| true).is_err());
        assert!(server.join().unwrap().is_err());
        assert_eq!(host.peer().unwrap(), PeerState::Unpaired);
        assert_eq!(client.peer().unwrap(), PeerState::Unpaired);

        let listener = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
        assert!(matches!(
            pair_responder_with_timeout(&listener, &host, |_, _| true, Duration::from_millis(5)),
            Err(SessionError::PairingTimeout)
        ));
        assert_eq!(host.peer().unwrap(), PeerState::Unpaired);
    }

    #[test]
    fn pairing_window_deadline_prevents_pending_publication_after_confirmation() {
        let host_root = temp("pair-deadline-host");
        let device_root = temp("pair-deadline-device");
        let _ = fs::remove_dir_all(&host_root);
        let _ = fs::remove_dir_all(&device_root);
        let host = IdentityStore::new(host_root);
        let device = IdentityStore::new_for_role(device_root, ResourceRole::DeviceSimulator);
        host.init().unwrap();
        device.init().unwrap();
        let listener = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = listener.local_addr().unwrap();
        let host_copy = host.clone();
        let responder = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            pair_responder_stream_reserved(
                stream,
                &host_copy,
                |_, _| {
                    thread::sleep(Duration::from_millis(80));
                    true
                },
                || true,
                std::time::Instant::now() + Duration::from_millis(40),
            )
        });
        assert!(pair_initiator(address, &device, [61; 16], |_, _| true).is_err());
        assert!(matches!(
            responder.join().unwrap(),
            Err(SessionError::PairingTimeout)
        ));
        assert_eq!(host.peer().unwrap(), PeerState::Unpaired);
        assert_eq!(device.peer().unwrap(), PeerState::Unpaired);
    }

    #[test]
    fn simultaneous_unknown_peer_receives_pairing_busy() {
        let host_root = temp("pairing-busy-host");
        let first_root = temp("pairing-busy-first");
        let second_root = temp("pairing-busy-second");
        let _ = fs::remove_dir_all(&host_root);
        let _ = fs::remove_dir_all(&first_root);
        let _ = fs::remove_dir_all(&second_root);
        let host = IdentityStore::new(host_root);
        let first = IdentityStore::new(first_root);
        let second = IdentityStore::new(second_root);
        host.init().unwrap();
        first.init().unwrap();
        second.init().unwrap();
        let listener = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = listener.local_addr().unwrap();
        let (confirmation_ready, confirmation_started) = mpsc::sync_channel(1);
        let (release_confirmation, confirmation_release) = mpsc::sync_channel(1);
        let host_join = thread::spawn(move || {
            pair_responder(&listener, &host, move |_, _| {
                confirmation_ready.send(()).unwrap();
                confirmation_release.recv().is_ok()
            })
        });
        let first_join =
            thread::spawn(move || pair_initiator(address, &first, [17; 16], |_, _| true));
        confirmation_started
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        assert!(matches!(
            pair_initiator(address, &second, [18; 16], |_, _| true),
            Err(SessionError::SessionBusy)
        ));
        release_confirmation.send(()).unwrap();
        assert!(first_join.join().unwrap().is_ok());
        assert!(host_join.join().unwrap().is_ok());
    }
    #[test]
    fn unpaired_noise_peer_cannot_start_application_session() {
        let host_root = temp("unpaired-host");
        let client_root = temp("unpaired-client");
        let _ = fs::remove_dir_all(&host_root);
        let _ = fs::remove_dir_all(&client_root);
        let host = IdentityStore::new(host_root);
        let client = IdentityStore::new(client_root);
        host.init().unwrap();
        client.init().unwrap();
        let listener = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = listener.local_addr().unwrap();
        let join =
            thread::spawn(move || serve_one(&listener, &host, AvailabilityResult::Available));
        assert!(read_once(address, &client, [3; 16], [4; 16]).is_err());
        assert!(join.join().unwrap().is_err());
    }
    #[test]
    fn simultaneous_pinned_hello_accepts_one_and_rejects_one_busy() {
        let host_root = temp("busy-host");
        let client_root = temp("busy-client");
        let _ = fs::remove_dir_all(&host_root);
        let _ = fs::remove_dir_all(&client_root);
        let host = IdentityStore::new(host_root);
        let client = IdentityStore::new(client_root);
        host.init().unwrap();
        client.init().unwrap();
        let pairing = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
        let pairing_address = pairing.local_addr().unwrap();
        let host_pair = host.clone();
        let pair_join = thread::spawn(move || pair_responder(&pairing, &host_pair, |_, _| true));
        pair_initiator(pairing_address, &client, [7; 16], |_, _| true).unwrap();
        pair_join.join().unwrap().unwrap();

        let listener = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = listener.local_addr().unwrap();
        let admission = std::sync::Arc::new(AtomicBool::new(false));
        let server_admission = admission.clone();
        let server = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            let first_host = host.clone();
            let first_admission = server_admission.clone();
            let first = thread::spawn(move || {
                serve_stream_admitted(
                    &mut first,
                    &first_host,
                    AvailabilityResult::Available,
                    Some(&first_admission),
                    None,
                )
            });
            while !server_admission.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(1));
            }
            let (mut second, _) = listener.accept().unwrap();
            let second = serve_stream_admitted(
                &mut second,
                &host,
                AvailabilityResult::Available,
                Some(&server_admission),
                None,
            );
            (first, second)
        });
        let first = ClientSession::connect(address, &client, [8; 16]).unwrap();
        assert!(matches!(
            ClientSession::connect(address, &client, [9; 16]),
            Err(SessionError::SessionBusy)
        ));
        first.close().unwrap();
        let (first_server, second_server) = server.join().unwrap();
        first_server.join().unwrap().unwrap();
        assert!(matches!(second_server, Err(SessionError::SessionBusy)));
    }
    #[test]
    fn active_session_cannot_read_after_exact_unpair() {
        let host_root = temp("unpair-active-host");
        let client_root = temp("unpair-active-client");
        let _ = fs::remove_dir_all(&host_root);
        let _ = fs::remove_dir_all(&client_root);
        let host = IdentityStore::new(host_root);
        let client = IdentityStore::new(client_root);
        host.init().unwrap();
        client.init().unwrap();
        let pairing = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
        let pairing_address = pairing.local_addr().unwrap();
        let host_pair = host.clone();
        let pair_join = thread::spawn(move || pair_responder(&pairing, &host_pair, |_, _| true));
        pair_initiator(pairing_address, &client, [10; 16], |_, _| true).unwrap();
        pair_join.join().unwrap().unwrap();
        let remote = match host.peer().unwrap() {
            PeerState::Paired {
                remote_public_key, ..
            } => remote_public_key,
            state => panic!("unexpected peer state: {state:?}"),
        };
        let listener = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = listener.local_addr().unwrap();
        let server_host = host.clone();
        let server = thread::spawn(move || {
            serve_one(&listener, &server_host, AvailabilityResult::Available)
        });
        let mut session = ClientSession::connect(address, &client, [11; 16]).unwrap();
        host.unpair(&remote).unwrap();
        assert!(session.read_availability([12; 16]).is_err());
        assert!(server.join().unwrap().is_err());
    }
    #[test]
    fn owner_unpair_terminates_runtime_session_before_terminal_result() {
        let role_root = temp("owner-unpair-runtime-host");
        let client_root = temp("owner-unpair-runtime-client");
        let _ = fs::remove_dir_all(&role_root);
        let _ = fs::remove_dir_all(&client_root);
        let host = IdentityStore::new(role_root.join("identity"));
        let client = IdentityStore::new(client_root);
        host.init().unwrap();
        client.init().unwrap();
        let pairing = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
        let pairing_address = pairing.local_addr().unwrap();
        let pair_host = host.clone();
        let pair_join = thread::spawn(move || pair_responder(&pairing, &pair_host, |_, _| true));
        pair_initiator(pairing_address, &client, [13; 16], |_, _| true).unwrap();
        pair_join.join().unwrap().unwrap();
        let remote = match host.peer().unwrap() {
            PeerState::Paired {
                remote_public_key, ..
            } => remote_public_key,
            state => panic!("unexpected peer state: {state:?}"),
        };
        let reservation = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = reservation.local_addr().unwrap();
        drop(reservation);
        let server_root = role_root.clone();
        let server = thread::spawn(move || {
            run_host_runtime(address, &server_root, AvailabilityResult::Available)
        });
        let control = role_root.join("control");
        for _ in 0..200 {
            if control.join("owner.sock").exists() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let mut session = (0..200)
            .find_map(|_| {
                ClientSession::connect(address, &client, [14; 16])
                    .inspect_err(|_| thread::sleep(Duration::from_millis(10)))
                    .ok()
            })
            .unwrap();
        let OwnerResponse::OwnerInfo {
            owner_generation, ..
        } = call_owner_control(&control, &OwnerCommand::OwnerInfo).unwrap()
        else {
            panic!("owner info missing")
        };
        let command_id = "1234567890abcdef1234567890abcdef".to_owned();
        assert_eq!(
            call_owner_control(
                &control,
                &OwnerCommand::Unpair {
                    command_id: command_id.clone(),
                    owner_generation: owner_generation.clone(),
                    peer_id: remote,
                },
            )
            .unwrap(),
            OwnerResponse::CommandAccepted
        );
        let terminal = (0..300)
            .find_map(|_| {
                let response = call_owner_control(
                    &control,
                    &OwnerCommand::CommandResult {
                        command_id: command_id.clone(),
                        owner_generation: owner_generation.clone(),
                    },
                )
                .unwrap();
                if matches!(
                    response,
                    OwnerResponse::CommandPending | OwnerResponse::CommandAccepted
                ) {
                    thread::sleep(Duration::from_millis(10));
                    None
                } else {
                    Some(response)
                }
            })
            .unwrap();
        assert_eq!(terminal, OwnerResponse::Unpaired);
        assert!(session.read_availability([15; 16]).is_err());
        assert_eq!(
            call_owner_control(&control, &OwnerCommand::Shutdown { owner_generation },).unwrap(),
            OwnerResponse::ShutdownAccepted
        );
        server.join().unwrap().unwrap();
    }
    #[test]
    fn long_lived_runtime_shutdown_is_owner_controlled_and_joined() {
        let role_root = temp("runtime");
        let _ = fs::remove_dir_all(&role_root);
        IdentityStore::new(role_root.join("identity"))
            .init()
            .unwrap();
        let runtime_root = role_root.clone();
        let join = thread::spawn(move || {
            run_host_runtime(
                "127.0.0.1:0".parse().unwrap(),
                &runtime_root,
                AvailabilityResult::Available,
            )
        });
        let socket = role_root.join("control/owner.sock");
        for _ in 0..100 {
            if socket.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let control = role_root.join("control");
        let generation = discover_owner(&control).unwrap().unwrap();
        assert_eq!(
            call_owner_control(
                &control,
                &OwnerCommand::Shutdown {
                    owner_generation: generation,
                },
            )
            .unwrap(),
            OwnerResponse::ShutdownAccepted
        );
        join.join().unwrap().unwrap();
    }
    #[test]
    #[allow(clippy::too_many_lines)]
    fn live_owners_pair_through_queryable_confirmation_commands() {
        let host_role = temp("live-pair-host-role");
        let simulator_role = temp("live-pair-simulator-role");
        let _ = fs::remove_dir_all(&host_role);
        let _ = fs::remove_dir_all(&simulator_role);
        let host_store = IdentityStore::new(host_role.join("identity"));
        let simulator_store = IdentityStore::new_for_role(
            simulator_role.join("identity"),
            ResourceRole::DeviceSimulator,
        );
        host_store.init().unwrap();
        simulator_store.init().unwrap();
        let probe = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = probe.local_addr().unwrap();
        drop(probe);
        let runtime_role = host_role.clone();
        let host_runtime = thread::spawn(move || {
            run_host_runtime(address, &runtime_role, AvailabilityResult::Available)
        });
        let host_control = host_role.join("control");
        wait_for_owner_socket(&host_control);
        let OwnerResponse::OwnerInfo {
            owner_generation: host_generation,
            ..
        } = call_owner_control(&host_control, &OwnerCommand::OwnerInfo).unwrap()
        else {
            panic!("host owner info missing");
        };
        for _ in 0..200 {
            if TcpStream::connect(address).is_ok() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        thread::sleep(Duration::from_millis(25));

        let simulator_actor = IdentityActor::start(simulator_store.clone());
        let simulator_control = simulator_role.join("control");
        let simulator_generation = "abcdef0123456789abcdef0123456789".to_owned();
        let server_actor = simulator_actor.clone();
        let server_control = simulator_control.clone();
        let server_generation = simulator_generation.clone();
        let (events, event_receiver) = mpsc::channel();
        let simulator_owner = thread::spawn(move || {
            run_owner_control_with_events(
                &server_control,
                &server_actor,
                &server_generation,
                Some(events),
            )
        });
        wait_for_owner_socket(&simulator_control);
        let pair_store = simulator_store.clone();
        let (pair_started, pair_start_seen) = mpsc::sync_channel(1);
        let (pairing_done, pairing_done_seen) = mpsc::sync_channel(1);
        let simulator_pairing = thread::spawn(move || {
            let OwnerEvent::PairStart { address, task } =
                event_receiver.recv_timeout(Duration::from_secs(3)).unwrap()
            else {
                panic!("simulator pair event missing");
            };
            pair_started.send(()).unwrap();
            let confirmation = task.clone();
            let result = pair_initiator(address, &pair_store, [41; 16], move |tx, sas| {
                confirmation.confirm(tx, sas)
            });
            task.finish(
                result.is_ok() || matches!(pair_store.peer(), Ok(PeerState::Paired { .. })),
            );
            pairing_done.send(()).unwrap();
            loop {
                match event_receiver.recv() {
                    Ok(OwnerEvent::RuntimeShutdown { joined }) => {
                        let _ = joined.send(());
                        break;
                    }
                    Ok(OwnerEvent::IdentityRevoked { joined }) => {
                        let _ = joined.send(());
                    }
                    Ok(
                        OwnerEvent::PairStart { task, .. } | OwnerEvent::PairingWindowOpen { task },
                    ) => task.finish(false),
                    Err(_) => break,
                }
            }
        });

        let host_command = "10000000000000000000000000000001";
        assert_eq!(
            call_owner_control(
                &host_control,
                &OwnerCommand::PairingWindowOpen {
                    command_id: host_command.into(),
                    owner_generation: host_generation.clone(),
                },
            )
            .unwrap(),
            OwnerResponse::CommandAccepted
        );
        owner_pairing_waiting(&host_control, host_command, &host_generation);
        let mut malformed = TcpStream::connect(address).unwrap();
        malformed.write_all(b"BADPRE").unwrap();
        drop(malformed);
        thread::sleep(Duration::from_millis(50));
        owner_pairing_waiting(&host_control, host_command, &host_generation);
        let simulator_command = "20000000000000000000000000000002";
        assert_eq!(
            call_owner_control(
                &simulator_control,
                &OwnerCommand::PairStart {
                    command_id: simulator_command.into(),
                    owner_generation: simulator_generation.clone(),
                    loopback_address: address.to_string(),
                },
            )
            .unwrap(),
            OwnerResponse::CommandAccepted
        );
        pair_start_seen
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        thread::sleep(Duration::from_millis(100));
        assert!(
            !simulator_pairing.is_finished(),
            "simulator pairing ended before confirmation"
        );
        let (host_transaction, host_authentication) =
            owner_pairing_prompt(&host_control, host_command, &host_generation);
        let (simulator_transaction, simulator_authentication) =
            owner_pairing_prompt(&simulator_control, simulator_command, &simulator_generation);
        assert_eq!(host_transaction, simulator_transaction);
        assert_eq!(host_authentication, simulator_authentication);
        owner_pairing_decide(
            &host_control,
            "30000000000000000000000000000003",
            &host_generation,
            host_command,
            &host_transaction,
        );
        owner_pairing_decide(
            &simulator_control,
            "40000000000000000000000000000004",
            &simulator_generation,
            simulator_command,
            &simulator_transaction,
        );
        assert_eq!(
            owner_pairing_terminal(&host_control, host_command, &host_generation),
            OwnerResponse::Paired
        );
        assert_eq!(
            owner_pairing_terminal(&simulator_control, simulator_command, &simulator_generation,),
            OwnerResponse::Paired
        );
        pairing_done_seen
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        assert!(matches!(
            host_store.peer().unwrap(),
            PeerState::Paired { .. }
        ));
        assert!(matches!(
            simulator_store.peer().unwrap(),
            PeerState::Paired { .. }
        ));
        let mut pinned_session = (0..200)
            .find_map(|_| {
                ClientSession::connect(address, &simulator_store, [42; 16])
                    .inspect_err(|_| thread::sleep(Duration::from_millis(10)))
                    .ok()
            })
            .expect("paired identity did not establish a pinned session");
        assert_eq!(
            pinned_session.read_availability([43; 16]).unwrap(),
            AvailabilityResult::Available
        );
        pinned_session.close().unwrap();
        for role in [&host_role, &simulator_role] {
            for entry in fs::read_dir(role.join("diagnostics")).unwrap() {
                let bytes = fs::read(entry.unwrap().path()).unwrap();
                assert!(
                    !bytes
                        .windows(host_authentication.len())
                        .any(|window| window == host_authentication.as_bytes())
                );
            }
        }
        assert_eq!(
            call_owner_control(
                &simulator_control,
                &OwnerCommand::Shutdown {
                    owner_generation: simulator_generation,
                },
            )
            .unwrap(),
            OwnerResponse::ShutdownAccepted
        );
        simulator_owner.join().unwrap().unwrap();
        simulator_pairing.join().unwrap();
        assert_eq!(
            call_owner_control(
                &host_control,
                &OwnerCommand::Shutdown {
                    owner_generation: host_generation,
                },
            )
            .unwrap(),
            OwnerResponse::ShutdownAccepted
        );
        host_runtime.join().unwrap().unwrap();
    }

    fn wait_for_owner_socket(control: &Path) {
        for _ in 0..200 {
            if control.join("owner.sock").exists() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("owner socket did not start");
    }

    fn owner_pairing_prompt(control: &Path, command: &str, generation: &str) -> (String, String) {
        let mut last = OwnerResponse::CommandUnknown;
        for _ in 0..300 {
            let response = call_owner_control(
                control,
                &OwnerCommand::CommandResult {
                    command_id: command.into(),
                    owner_generation: generation.into(),
                },
            )
            .unwrap();
            if let OwnerResponse::PairingConfirmationRequired {
                pairing_transaction_id,
                authentication_string,
            } = response
            {
                return (pairing_transaction_id, authentication_string);
            }
            last = response;
            thread::sleep(Duration::from_millis(10));
        }
        panic!("pairing prompt missing: {last:?}");
    }

    fn owner_pairing_waiting(control: &Path, command: &str, generation: &str) {
        for _ in 0..300 {
            let response = call_owner_control(
                control,
                &OwnerCommand::CommandResult {
                    command_id: command.into(),
                    owner_generation: generation.into(),
                },
            )
            .unwrap();
            if response == OwnerResponse::PairingWaiting {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("pairing window did not become active");
    }

    fn owner_pairing_decide(
        control: &Path,
        command: &str,
        generation: &str,
        parent: &str,
        transaction: &str,
    ) {
        assert_eq!(
            call_owner_control(
                control,
                &OwnerCommand::PairingDecide {
                    command_id: command.into(),
                    owner_generation: generation.into(),
                    parent_command_id: parent.into(),
                    pairing_transaction_id: transaction.into(),
                    confirmed: true,
                },
            )
            .unwrap(),
            OwnerResponse::PairingDecisionAccepted
        );
    }

    fn owner_pairing_terminal(control: &Path, command: &str, generation: &str) -> OwnerResponse {
        for _ in 0..300 {
            let response = call_owner_control(
                control,
                &OwnerCommand::CommandResult {
                    command_id: command.into(),
                    owner_generation: generation.into(),
                },
            )
            .unwrap();
            if !matches!(
                response,
                OwnerResponse::CommandPending
                    | OwnerResponse::CommandAccepted
                    | OwnerResponse::PairingWaiting
                    | OwnerResponse::PairingConfirmationRequired { .. }
            ) {
                return response;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("pairing terminal missing");
    }
    #[test]
    fn rejects_non_loopback() {
        assert!(matches!(
            bind_loopback(SocketAddr::new(IpAddr::from([0, 0, 0, 0]), 0)),
            Err(SessionError::NonLoopback)
        ));
    }

    #[test]
    fn private_lan_scope_is_exact_rfc1918_ipv4_on_fixed_port() {
        for address in [
            SocketAddr::from(([10, 0, 0, 1], PRIVATE_LAN_PORT)),
            SocketAddr::from(([172, 16, 0, 1], PRIVATE_LAN_PORT)),
            SocketAddr::from(([172, 31, 255, 254], PRIVATE_LAN_PORT)),
            SocketAddr::from(([192, 168, 1, 1], PRIVATE_LAN_PORT)),
        ] {
            assert!(is_exact_private_lan_address(address));
        }
        for address in [
            SocketAddr::from(([0, 0, 0, 0], PRIVATE_LAN_PORT)),
            SocketAddr::from(([127, 0, 0, 1], PRIVATE_LAN_PORT)),
            SocketAddr::from(([169, 254, 1, 1], PRIVATE_LAN_PORT)),
            SocketAddr::from(([172, 32, 0, 1], PRIVATE_LAN_PORT)),
            SocketAddr::from(([192, 168, 1, 1], 0)),
            "[::1]:39042".parse().unwrap(),
        ] {
            assert!(!is_exact_private_lan_address(address));
        }
    }
    #[test]
    fn negotiation_keeps_features_and_permissions_independent() {
        let hello =
            |protocol_majors, required_features, optional_features, requested_permissions| {
                Message::Hello {
                    session: [7; 16],
                    protocol_majors,
                    required_features,
                    optional_features,
                    requested_permissions,
                }
            };
        assert_eq!(
            negotiate_hello(hello(
                deskkin_protocol::Bits([0; 8]),
                deskkin_protocol::AVAILABILITY_READ_V1,
                deskkin_protocol::Bits([0; 8]),
                deskkin_protocol::AVAILABILITY_READ_PERMISSION,
            )),
            Err(([7; 16], HelloRejectReason::NoCommonVersion))
        );
        assert_eq!(
            negotiate_hello(hello(
                deskkin_protocol::PROTOCOL_MAJOR_1,
                deskkin_protocol::Bits([3, 0, 0, 0, 0, 0, 0, 0]),
                deskkin_protocol::Bits([0; 8]),
                deskkin_protocol::AVAILABILITY_READ_PERMISSION,
            )),
            Err(([7; 16], HelloRejectReason::RequiredFeatureUnsupported))
        );
        assert_eq!(
            negotiate_hello(hello(
                deskkin_protocol::PROTOCOL_MAJOR_1,
                deskkin_protocol::AVAILABILITY_READ_V1,
                deskkin_protocol::Bits([0; 8]),
                deskkin_protocol::Bits([0; 8]),
            )),
            Err(([7; 16], HelloRejectReason::PermissionDenied))
        );
        assert_eq!(
            negotiate_hello(hello(
                deskkin_protocol::PROTOCOL_MAJOR_1,
                deskkin_protocol::Bits([0; 8]),
                deskkin_protocol::AVAILABILITY_READ_V1,
                deskkin_protocol::AVAILABILITY_READ_PERMISSION,
            )),
            Ok([7; 16])
        );
        assert_eq!(
            negotiate_hello(hello(
                deskkin_protocol::PROTOCOL_MAJOR_1,
                deskkin_protocol::AVAILABILITY_READ_V1,
                deskkin_protocol::Bits([0; 8]),
                deskkin_protocol::Bits([0b1000_0001, 0, 0, 0, 0, 0, 0, 0]),
            )),
            Ok([7; 16])
        );
    }
    #[test]
    fn request_ids_start_at_one_and_are_strictly_monotonic() {
        let mut last = 0;
        assert!(!accept_request_id(&mut last, 0));
        assert!(accept_request_id(&mut last, 1));
        assert!(!accept_request_id(&mut last, 1));
        assert!(!accept_request_id(&mut last, 0));
        assert!(!accept_request_id(&mut last, 3));
        assert!(accept_request_id(&mut last, 2));
        assert!(!accept_request_id(&mut last, 1));
    }
    #[test]
    fn pairing_window_reservation_is_single_use_until_explicit_reset() {
        let state = Mutex::new((false, false, false));
        assert!(reserve_pairing_window(&state));
        assert!(!reserve_pairing_window(&state));
        state.lock().unwrap().2 = true;
        assert!(!reserve_pairing_window(&state));
        *state.lock().unwrap() = (false, false, false);
        assert!(reserve_pairing_window(&state));
    }
    #[test]
    fn session_writer_bounds_application_and_prioritizes_reserved_control() {
        let mut writer = SessionWriteQueue::new();
        writer.enqueue_ping_if_idle();
        for request_id in 0..8 {
            writer
                .enqueue_application(Message::AvailabilityResult {
                    request_id,
                    operation: [0; 16],
                    result: AvailabilityResult::Available,
                })
                .unwrap();
        }
        assert!(matches!(
            writer.enqueue_application(Message::AvailabilityResult {
                request_id: 9,
                operation: [0; 16],
                result: AvailabilityResult::Available,
            }),
            Err(SessionError::QueueFull)
        ));
        writer.enqueue_pong();
        writer.enqueue_close(deskkin_protocol::CloseReason::Timeout);
        assert_eq!(
            writer.pop(),
            Some(Message::Close {
                reason: deskkin_protocol::CloseReason::Timeout
            })
        );
        assert_eq!(writer.pop(), Some(Message::Pong));
        assert_eq!(writer.pop(), Some(Message::Ping));
        assert!(matches!(
            writer.pop(),
            Some(Message::AvailabilityResult { .. })
        ));
    }
    #[test]
    fn structurally_valid_rejected_hello_still_completes_responder_trust() {
        let root = temp("pairing-hello-reject");
        let _ = fs::remove_dir_all(&root);
        let store = IdentityStore::new(root);
        store.init().unwrap();
        let remote = "71".repeat(32);
        let transaction = "82".repeat(16);
        store
            .set_peer(PeerState::Pending {
                remote_public_key: remote.clone(),
                pairing_transaction_id: transaction.clone(),
            })
            .unwrap();
        store
            .set_peer(PeerState::Committing {
                remote_public_key: remote.clone(),
                pairing_transaction_id: transaction.clone(),
            })
            .unwrap();
        let actor = IdentityActor::start(store.clone());
        let (session, rejection) = complete_responder_trust(
            &actor,
            remote.clone(),
            transaction.clone(),
            Message::Hello {
                session: [31; 16],
                protocol_majors: deskkin_protocol::Bits([0; 8]),
                required_features: deskkin_protocol::AVAILABILITY_READ_V1,
                optional_features: deskkin_protocol::Bits([0; 8]),
                requested_permissions: deskkin_protocol::AVAILABILITY_READ_PERMISSION,
            },
        )
        .unwrap();
        assert_eq!(session, [31; 16]);
        assert_eq!(rejection, Some(HelloRejectReason::NoCommonVersion));
        assert_eq!(
            store.peer().unwrap(),
            PeerState::Paired {
                remote_public_key: remote,
                pairing_transaction_id: transaction,
            }
        );
    }
    #[test]
    fn identity_rejects_symlink_and_non_private_temporary() {
        for (name, make) in [("symlink", 0_u8), ("mode", 1_u8)] {
            let root = temp(name);
            let outside = temp(&format!("{name}-outside"));
            let _ = fs::remove_dir_all(&root);
            let _ = fs::remove_file(&outside);
            fs::create_dir_all(&root).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            if make == 0 {
                fs::write(&outside, b"keep").unwrap();
                symlink(&outside, root.join(".identity.tmp")).unwrap();
            } else {
                fs::write(root.join(".identity.tmp"), b"residue").unwrap();
                fs::set_permissions(
                    root.join(".identity.tmp"),
                    fs::Permissions::from_mode(0o644),
                )
                .unwrap();
            }
            assert!(IdentityStore::new(root).init().is_err());
            if make == 0 {
                assert_eq!(fs::read(&outside).unwrap(), b"keep");
            }
        }
    }
    #[test]
    fn identity_rejects_unknown_entry_and_inconsistent_public_key() {
        let unknown_root = temp("unknown-entry");
        let _ = fs::remove_dir_all(&unknown_root);
        fs::create_dir_all(&unknown_root).unwrap();
        fs::set_permissions(&unknown_root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(unknown_root.join("unexpected"), b"x").unwrap();
        assert_eq!(
            IdentityStore::new(unknown_root).init(),
            Err(StoreError::UnknownEntry)
        );

        let corrupt_root = temp("inconsistent-key");
        let _ = fs::remove_dir_all(&corrupt_root);
        let store = IdentityStore::new(corrupt_root.clone());
        store.init().unwrap();
        let canonical = corrupt_root.join("identity-v1.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&canonical).unwrap()).unwrap();
        value["local_public_key"] = serde_json::Value::String("00".repeat(32));
        fs::write(&canonical, serde_json::to_vec(&value).unwrap()).unwrap();
        fs::set_permissions(&canonical, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(store.peer(), Err(StoreError::Invalid));

        let variant_root = temp("peer-variant-extra-field");
        let _ = fs::remove_dir_all(&variant_root);
        let variant_store = IdentityStore::new(variant_root.clone());
        variant_store.init().unwrap();
        let canonical = variant_root.join("identity-v1.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&canonical).unwrap()).unwrap();
        value["peer"]["unexpected"] = serde_json::Value::Bool(true);
        fs::write(&canonical, serde_json::to_vec(&value).unwrap()).unwrap();
        fs::set_permissions(&canonical, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(variant_store.peer(), Err(StoreError::Invalid));
    }
}
