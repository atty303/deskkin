#![forbid(unsafe_code)]
#![allow(clippy::missing_errors_doc)]

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const MAX_BYTES: u64 = 32 * 1024 * 1024;
const SUCCESS_LIMIT: usize = 10;
const NON_SUCCESS_LIMIT: usize = 20;
static RUN_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordingMode {
    On,
    Off,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    Success,
    Error,
    Cancel,
    Timeout,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Completeness {
    Complete,
    Partial,
    Dropped,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingHealth {
    Healthy,
    StorageUnavailable,
    CapacityExhausted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Publication {
    pub run_id: String,
    pub completeness: Completeness,
    pub health: RecordingHealth,
    pub stored: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    StatusRefresh,
    CoreTransition,
    EffectReadStatus,
    EffectArmRefreshTimer,
    PresenterApplyView,
    TransportConnect,
    TransportAccept,
    TransportFrameRead,
    TransportFrameWrite,
    NoiseHandshake,
    PairingConfirm,
    PairingPersist,
    ProtocolNegotiate,
    AvailabilityRead,
    ControlRoute,
    IdentityInit,
    IdentityUnpair,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SemanticRecord {
    pub operation: Operation,
    pub operation_id: u16,
    pub parent_operation_id: Option<u16>,
    pub status: OperationStatus,
    pub error_type: Option<ErrorType>,
    pub effect_id: Option<u64>,
    pub virtual_time_ms: u64,
    pub end_virtual_time_ms: u64,
    pub duration_ms: Option<u32>,
    pub render_width: Option<u32>,
    pub render_height: Option<u32>,
    pub value: Option<ClosedValue>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    InProgress,
    Success,
    Error,
    Cancel,
    Timeout,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorType {
    ReadUnavailable,
    TimerArmUnavailable,
    Cancelled,
    Timeout,
    Crash,
    ScenarioFailed,
    ConnectionLost,
    VersionMismatch,
    PermissionDenied,
    SessionBusy,
    RequestTimeout,
    QueueFull,
    EndOfStream,
    FrameOversize,
    MalformedFrame,
    HandshakeTimeout,
    AuthenticationFailed,
    PairingExpired,
    IdentityStore,
    PreauthCapacity,
    OwnerLost,
    RevocationRecoveryRequired,
    InvalidAddress,
    PairingRejected,
    PairingIncomplete,
    StoreFailed,
    StoreStalled,
    OwnerBusy,
    LockTimeout,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosedValue {
    Started,
    Available,
    Unavailable,
    Unknown,
    Armed,
    ReadFailed,
    ArmFailed,
    Success,
    Error,
    Cancel,
    Timeout,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiagnosticRun {
    pub schema_version: u8,
    pub resource: ResourceIdentity,
    pub run_id: String,
    pub scenario_run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaction_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_context_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_context_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_major: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_features: Option<[u8; 8]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granted_permissions: Option<[u8; 8]>,
    pub outcome: RunOutcome,
    pub completeness: Completeness,
    pub health: RecordingHealth,
    pub terminal: bool,
    pub missing_reason: Option<MissingReason>,
    pub owner: Option<ProcessOwner>,
    pub retained: bool,
    pub created_unix_ms: u64,
    pub records: Vec<SemanticRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessOwner {
    pid: u32,
    start_ticks: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingReason {
    Crash,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceIdentity {
    pub program: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<ResourceRole>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceRole {
    Host,
    DeviceSimulator,
}

#[must_use]
pub fn resource_identity() -> ResourceIdentity {
    ResourceIdentity {
        program: "deskkin-simulator".into(),
        version: "0.1.0".into(),
        role: None,
    }
}

#[must_use]
pub fn resource_identity_for(program: &str, version: &str, role: ResourceRole) -> ResourceIdentity {
    ResourceIdentity {
        program: program.into(),
        version: version.into(),
        role: Some(role),
    }
}

#[must_use]
pub fn in_progress_run(
    run_id: String,
    scenario_run_id: String,
    effect_id: u64,
    started_ms: u64,
) -> DiagnosticRun {
    DiagnosticRun {
        schema_version: 1,
        resource: resource_identity(),
        run_id,
        scenario_run_id,
        transaction_id: None,
        session_context_id: None,
        operation_context_id: None,
        protocol_major: None,
        selected_features: None,
        granted_permissions: None,
        outcome: RunOutcome::Error,
        completeness: Completeness::Partial,
        health: RecordingHealth::Healthy,
        terminal: false,
        missing_reason: None,
        owner: current_process_owner(),
        retained: false,
        created_unix_ms: now_unix_ms(),
        records: vec![
            SemanticRecord {
                operation: Operation::StatusRefresh,
                operation_id: 1,
                parent_operation_id: None,
                status: OperationStatus::InProgress,
                error_type: None,
                effect_id: None,
                virtual_time_ms: started_ms,
                end_virtual_time_ms: started_ms,
                duration_ms: None,
                render_width: None,
                render_height: None,
                value: Some(ClosedValue::Started),
            },
            SemanticRecord {
                operation: Operation::EffectReadStatus,
                operation_id: 2,
                parent_operation_id: Some(1),
                status: OperationStatus::InProgress,
                error_type: None,
                effect_id: Some(effect_id),
                virtual_time_ms: started_ms,
                end_virtual_time_ms: started_ms,
                duration_ms: None,
                render_width: None,
                render_height: None,
                value: Some(ClosedValue::Started),
            },
        ],
    }
}

#[must_use]
pub fn current_process_owner() -> Option<ProcessOwner> {
    let pid = std::process::id();
    process_start_ticks(pid).map(|start_ticks| ProcessOwner { pid, start_ticks })
}

fn process_start_ticks(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_name = stat.rsplit_once(')')?.1;
    after_name.split_whitespace().nth(19)?.parse().ok()
}

fn owner_is_live(owner: &ProcessOwner) -> bool {
    process_start_ticks(owner.pid) == Some(owner.start_ticks)
}

pub fn finalize_operation_records(records: &mut [SemanticRecord], outcome: RunOutcome) {
    let started_ms = records.first().map_or(0, |record| record.virtual_time_ms);
    let completed_ms = records
        .iter()
        .map(|record| record.virtual_time_ms)
        .max()
        .unwrap_or(started_ms);
    for (index, record) in records.iter_mut().enumerate() {
        record.operation_id = u16::try_from(index + 1).unwrap_or(u16::MAX);
        record.parent_operation_id = (index != 0).then_some(1);
        record.end_virtual_time_ms = record.virtual_time_ms;
        record.status = OperationStatus::Success;
        record.error_type = None;
        match record.value {
            Some(ClosedValue::ReadFailed) => {
                record.status = OperationStatus::Error;
                record.error_type = Some(ErrorType::ReadUnavailable);
            }
            Some(ClosedValue::ArmFailed) => {
                record.status = OperationStatus::Error;
                record.error_type = Some(ErrorType::TimerArmUnavailable);
            }
            Some(ClosedValue::Cancel) => {
                record.status = OperationStatus::Cancel;
                record.error_type = Some(ErrorType::Cancelled);
            }
            Some(ClosedValue::Timeout) => {
                record.status = OperationStatus::Timeout;
                record.error_type = Some(ErrorType::Timeout);
            }
            _ => {}
        }
        if record.operation == Operation::EffectReadStatus {
            record.virtual_time_ms = started_ms;
            record.end_virtual_time_ms = completed_ms;
        }
    }
    let first_error = records.iter().find_map(|record| record.error_type);
    if let Some(root) = records.first_mut() {
        root.end_virtual_time_ms = completed_ms;
        root.status = match outcome {
            RunOutcome::Success => OperationStatus::Success,
            RunOutcome::Error => OperationStatus::Error,
            RunOutcome::Cancel => OperationStatus::Cancel,
            RunOutcome::Timeout => OperationStatus::Timeout,
        };
        root.error_type = match outcome {
            RunOutcome::Success => None,
            RunOutcome::Error => first_error,
            RunOutcome::Cancel => Some(ErrorType::Cancelled),
            RunOutcome::Timeout => Some(ErrorType::Timeout),
        };
    }
}

pub struct Recorder {
    mode: RecordingMode,
    root: PathBuf,
    max_bytes: u64,
    success_limit: usize,
    non_success_limit: usize,
}

impl Recorder {
    #[must_use]
    pub fn at_root(root: PathBuf) -> Self {
        Self {
            mode: RecordingMode::On,
            root,
            max_bytes: MAX_BYTES,
            success_limit: SUCCESS_LIMIT,
            non_success_limit: NON_SUCCESS_LIMIT,
        }
    }

    #[must_use]
    pub fn new(root: PathBuf, mode: RecordingMode, max_bytes: u64) -> Self {
        Self {
            mode,
            root,
            max_bytes,
            success_limit: SUCCESS_LIMIT,
            non_success_limit: NON_SUCCESS_LIMIT,
        }
    }

    pub fn from_environment(mode: RecordingMode) -> Self {
        let root = std::env::var_os("DESKKIN_PHASE2_DIR")
            .map_or_else(|| PathBuf::from(".deskkin/phase2"), PathBuf::from);
        Self {
            mode,
            root,
            max_bytes: MAX_BYTES,
            success_limit: SUCCESS_LIMIT,
            non_success_limit: NON_SUCCESS_LIMIT,
        }
    }

    pub fn begin_live_run(&self, run_id: &str) -> io::Result<File> {
        if !valid_run_id(run_id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid run id",
            ));
        }
        let diagnostics = self.prepare_store()?;
        let marker = diagnostics.join(format!(".live-{run_id}"));
        reject_symlink_if_exists(&marker)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(marker)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        File::lock(&file)?;
        sync_directory(&diagnostics)?;
        Ok(file)
    }

    pub fn end_live_run(&self, run_id: &str, file: File) -> io::Result<()> {
        if !valid_run_id(run_id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid run id",
            ));
        }
        drop(file);
        let diagnostics = self.root.join("diagnostics");
        let marker = diagnostics.join(format!(".live-{run_id}"));
        match fs::remove_file(marker) {
            Ok(()) => sync_directory(&diagnostics),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    #[must_use]
    pub fn publish(&self, mut run: DiagnosticRun) -> Publication {
        if self.mode == RecordingMode::Off {
            return Publication {
                run_id: run.run_id,
                completeness: Completeness::Complete,
                health: RecordingHealth::Healthy,
                stored: false,
            };
        }
        if let Ok(publication) = self.publish_inner(&mut run) {
            publication
        } else {
            let stored = self.run_file_is_stored(&run.run_id);
            let mut publication = Publication {
                run_id: run.run_id,
                completeness: Completeness::Partial,
                health: RecordingHealth::StorageUnavailable,
                stored,
            };
            if !self.publish_health_best_effort(&publication) {
                publication.completeness = Completeness::Dropped;
            }
            publication
        }
    }

    fn run_file_is_stored(&self, run_id: &str) -> bool {
        let path = self.root.join("diagnostics").join(format!("{run_id}.json"));
        fs::symlink_metadata(path)
            .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
    }

    fn publish_inner(&self, run: &mut DiagnosticRun) -> io::Result<Publication> {
        let diagnostics = self.prepare_store()?;
        let lock = self.open_lock()?;
        File::lock(&lock)?;
        let result = (|| {
            cleanup_stale_temps(&diagnostics)?;
            recover_in_progress_locked(&diagnostics)?;
            let bytes = serde_json::to_vec(run).map_err(io::Error::other)?;
            if !self.ensure_capacity_locked(&diagnostics, bytes.len() as u64)? {
                run.completeness = Completeness::Dropped;
                run.health = RecordingHealth::CapacityExhausted;
                let publication = Publication {
                    run_id: run.run_id.clone(),
                    completeness: run.completeness,
                    health: run.health,
                    stored: false,
                };
                self.publish_health_locked(&publication)?;
                return Ok(publication);
            }
            atomic_write(&diagnostics.join(format!("{}.json", run.run_id)), &bytes)?;
            self.enforce_retention_locked(&diagnostics)?;
            Ok(Publication {
                run_id: run.run_id.clone(),
                completeness: run.completeness,
                health: run.health,
                stored: true,
            })
        })();
        File::unlock(&lock)?;
        result
    }

    fn prepare_store(&self) -> io::Result<PathBuf> {
        reject_symlink_if_exists(&self.root)?;
        create_private_dir(&self.root)?;
        let diagnostics = self.root.join("diagnostics");
        reject_symlink_if_exists(&diagnostics)?;
        create_private_dir(&diagnostics)?;
        Ok(diagnostics)
    }

    fn open_lock(&self) -> io::Result<File> {
        reject_symlink_if_exists(&self.root.join("store.lock"))?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(self.root.join("store.lock"))?;
        lock.set_permissions(fs::Permissions::from_mode(0o600))?;
        Ok(lock)
    }

    fn enforce_retention_locked(&self, diagnostics: &Path) -> io::Result<()> {
        let mut success = Vec::new();
        let mut other = Vec::new();
        for entry in private_json_entries(diagnostics)? {
            let bytes = fs::read(&entry)?;
            let Ok(run) = serde_json::from_slice::<DiagnosticRun>(&bytes) else {
                continue;
            };
            if run.retained {
                continue;
            }
            let item = (run.created_unix_ms, entry);
            if run.outcome == RunOutcome::Success {
                success.push(item);
            } else {
                other.push(item);
            }
        }
        success.sort_by_key(|item| item.0);
        other.sort_by_key(|item| item.0);
        remove_oldest(&success, self.success_limit)?;
        remove_oldest(&other, self.non_success_limit)?;
        self.enforce_byte_cap_locked(diagnostics)
    }

    fn enforce_byte_cap_locked(&self, diagnostics: &Path) -> io::Result<()> {
        while directory_bytes(diagnostics)? > self.max_bytes {
            let mut candidates = Vec::new();
            for entry in private_json_entries(diagnostics)? {
                let Ok(run) = serde_json::from_slice::<DiagnosticRun>(&fs::read(&entry)?) else {
                    continue;
                };
                if !run.retained {
                    let priority = u8::from(run.outcome != RunOutcome::Success);
                    candidates.push((priority, run.created_unix_ms, entry));
                }
            }
            candidates.sort_by_key(|item| (item.0, item.1));
            let Some((_, _, oldest)) = candidates.first() else {
                self.publish_health_locked(&Publication {
                    run_id: "retention".into(),
                    completeness: Completeness::Dropped,
                    health: RecordingHealth::CapacityExhausted,
                    stored: false,
                })?;
                break;
            };
            fs::remove_file(oldest)?;
            sync_directory(diagnostics)?;
        }
        Ok(())
    }

    fn ensure_capacity_locked(&self, diagnostics: &Path, required: u64) -> io::Result<bool> {
        while directory_bytes(diagnostics)?.saturating_add(required) > self.max_bytes {
            let mut candidates = Vec::new();
            for entry in private_json_entries(diagnostics)? {
                let Ok(run) = serde_json::from_slice::<DiagnosticRun>(&fs::read(&entry)?) else {
                    continue;
                };
                if !run.retained {
                    let priority = u8::from(run.outcome != RunOutcome::Success);
                    candidates.push((priority, run.created_unix_ms, entry));
                }
            }
            candidates.sort_by_key(|item| (item.0, item.1));
            let Some((_, _, oldest)) = candidates.first() else {
                return Ok(false);
            };
            fs::remove_file(oldest)?;
            sync_directory(diagnostics)?;
        }
        Ok(true)
    }

    fn publish_health_locked(&self, publication: &Publication) -> io::Result<()> {
        let bytes = serde_json::to_vec(&HealthRecord {
            schema_version: 1,
            run_id: publication.run_id.clone(),
            completeness: publication.completeness,
            health: publication.health,
        })
        .map_err(io::Error::other)?;
        atomic_write(&self.root.join("recording-health.json"), &bytes)
    }

    #[must_use]
    pub fn publish_health_best_effort(&self, publication: &Publication) -> bool {
        (|| {
            reject_symlink_if_exists(&self.root)?;
            create_private_dir(&self.root)?;
            let lock = self.open_lock()?;
            File::lock(&lock)?;
            let result = self.publish_health_locked(publication);
            File::unlock(&lock)?;
            result
        })()
        .is_ok()
    }
}

fn recover_in_progress_locked(diagnostics: &Path) -> io::Result<()> {
    for path in private_json_entries(diagnostics)? {
        let bytes = fs::read(&path)?;
        let Ok(mut run) = serde_json::from_slice::<DiagnosticRun>(&bytes) else {
            continue;
        };
        if run.terminal || run.owner.as_ref().is_some_and(owner_is_live) {
            continue;
        }
        let marker = diagnostics.join(format!(".live-{}", run.run_id));
        let stale_marker = if run.owner.is_none() && marker.exists() {
            let file = OpenOptions::new().read(true).write(true).open(&marker)?;
            match file.try_lock() {
                Ok(()) => Some(file),
                Err(std::fs::TryLockError::WouldBlock) => continue,
                Err(std::fs::TryLockError::Error(error)) => return Err(error),
            }
        } else {
            None
        };
        run.terminal = true;
        run.outcome = RunOutcome::Error;
        run.completeness = Completeness::Partial;
        run.missing_reason = Some(MissingReason::Crash);
        run.owner = None;
        for record in &mut run.records {
            if record.status == OperationStatus::InProgress {
                record.status = OperationStatus::Error;
                record.error_type = Some(ErrorType::Crash);
            }
        }
        atomic_write(&path, &serde_json::to_vec(&run).map_err(io::Error::other)?)?;
        if let Some(marker_file) = stale_marker {
            drop(marker_file);
            fs::remove_file(&marker)?;
            sync_directory(diagnostics)?;
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct HealthRecord {
    schema_version: u8,
    run_id: String,
    completeness: Completeness,
    health: RecordingHealth,
}

fn remove_oldest(entries: &[(u64, PathBuf)], keep: usize) -> io::Result<()> {
    for (_, path) in entries.iter().take(entries.len().saturating_sub(keep)) {
        fs::remove_file(path)?;
        if let Some(parent) = path.parent() {
            sync_directory(parent)?;
        }
    }
    Ok(())
}

fn private_json_entries(directory: &Path) -> io::Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    for item in fs::read_dir(directory)? {
        let item = item?;
        let metadata = fs::symlink_metadata(item.path())?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::other("symlink in diagnostic store"));
        }
        if metadata.is_file() && item.path().extension().is_some_and(|ext| ext == "json") {
            result.push(item.path());
        }
    }
    Ok(result)
}

fn directory_bytes(directory: &Path) -> io::Result<u64> {
    fs::read_dir(directory)?.try_fold(0_u64, |total, item| {
        let path = item?.path();
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::other("symlink in diagnostic store"));
        }
        Ok(total.saturating_add(if metadata.is_file() {
            metadata.len()
        } else {
            0
        }))
    })
}

fn create_private_dir(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.permissions().mode() & 0o777 != 0o700 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "existing diagnostic directory must already be mode 0700",
                ));
            }
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(path)?;
            reject_symlink_if_exists(path)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        }
        Err(error) => Err(error),
    }
}

fn reject_symlink_if_exists(path: &Path) -> io::Result<()> {
    for candidate in path.ancestors() {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(io::Error::other("symlink is not allowed"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    reject_symlink_if_exists(path)?;
    let suffix = RUN_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = path.with_extension(format!("tmp-{suffix}"));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        sync_directory(
            path.parent()
                .ok_or_else(|| io::Error::other("missing parent"))?,
        )
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn cleanup_stale_temps(directory: &Path) -> io::Result<()> {
    for item in fs::read_dir(directory)? {
        let item = item?;
        let path = item.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::other("symlink in private store"));
        }
        if metadata.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension.to_string_lossy().starts_with("tmp-"))
        {
            fs::remove_file(path)?;
            sync_directory(directory)?;
        }
    }
    Ok(())
}

pub fn new_run_id(prefix: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = RUN_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{now:032x}-{counter:016x}")
}

#[must_use]
pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

pub fn publish_scenario_result(name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    if !valid_run_id(name) {
        return Err(io::Error::other("invalid scenario name"));
    }
    let recorder = Recorder::from_environment(RecordingMode::On);
    let results = recorder.root.join("results");
    let path = results.join(format!("{name}.json"));
    validate_single_line_utf8_path(&path)?;
    reject_symlink_if_exists(&recorder.root)?;
    create_private_dir(&recorder.root)?;
    reject_symlink_if_exists(&results)?;
    create_private_dir(&results)?;
    let lock = recorder.open_lock()?;
    File::lock(&lock)?;
    cleanup_stale_temps(&results)?;
    let result = atomic_write(&path, bytes).map(|()| path);
    File::unlock(&lock)?;
    result
}

fn validate_single_line_utf8_path(path: &Path) -> io::Result<()> {
    let value = path
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "result path must be UTF-8"))?;
    if value.contains(['\n', '\r']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "result path must be one line",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub enum DiagnosticsCommand {
    List,
    Retain(String),
    Unretain(String),
    Delete(String),
}

/// Runs one exact diagnostic-store administration operation.
///
/// # Errors
///
/// Returns an error when the private store cannot be opened or the exact run
/// cannot be validated, decoded, updated, or removed.
pub fn run_diagnostics_command(command: DiagnosticsCommand) -> Result<String, String> {
    let recorder = Recorder::from_environment(RecordingMode::On);
    run_diagnostics_command_with_recorder(&recorder, command)
}

pub fn run_diagnostics_command_at(
    root: PathBuf,
    command: DiagnosticsCommand,
) -> Result<String, String> {
    let recorder = Recorder::new(root, RecordingMode::On, 16 * 1024 * 1024);
    run_diagnostics_command_with_recorder(&recorder, command)
}

fn run_diagnostics_command_with_recorder(
    recorder: &Recorder,
    command: DiagnosticsCommand,
) -> Result<String, String> {
    let diagnostics = recorder
        .prepare_store()
        .map_err(|error| error.to_string())?;
    let lock = recorder.open_lock().map_err(|error| error.to_string())?;
    File::lock(&lock).map_err(|error| error.to_string())?;
    cleanup_stale_temps(&diagnostics).map_err(|error| error.to_string())?;
    recover_in_progress_locked(&diagnostics).map_err(|error| error.to_string())?;
    let result = match command {
        DiagnosticsCommand::List => list_runs(&diagnostics),
        DiagnosticsCommand::Retain(id) => set_retained(&diagnostics, &id, true),
        DiagnosticsCommand::Unretain(id) => unretain_and_enforce(recorder, &diagnostics, &id),
        DiagnosticsCommand::Delete(id) => delete_run(&diagnostics, &id),
    };
    File::unlock(&lock).map_err(|error| error.to_string())?;
    result
}

fn unretain_and_enforce(
    recorder: &Recorder,
    diagnostics: &Path,
    id: &str,
) -> Result<String, String> {
    let updated = set_retained(diagnostics, id, false)?;
    recorder
        .enforce_retention_locked(diagnostics)
        .map_err(|error| error.to_string())?;
    Ok(updated)
}

fn valid_run_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn resolved_run_path(diagnostics: &Path, id: &str) -> Result<PathBuf, String> {
    if !valid_run_id(id) {
        return Err("run ID must be exact and contain only ASCII letters, digits, or '-'".into());
    }
    let path = diagnostics.join(format!("{id}.json"));
    reject_symlink_if_exists(&path).map_err(|error| error.to_string())?;
    Ok(path)
}

fn list_runs(diagnostics: &Path) -> Result<String, String> {
    let mut runs = Vec::new();
    for path in private_json_entries(diagnostics).map_err(|error| error.to_string())? {
        let bytes = fs::read(path).map_err(|error| error.to_string())?;
        let run: DiagnosticRun =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        runs.push(run);
    }
    runs.sort_by_key(|run| run.created_unix_ms);
    serde_json::to_string(&runs).map_err(|error| error.to_string())
}

fn set_retained(diagnostics: &Path, id: &str, retained: bool) -> Result<String, String> {
    let path = resolved_run_path(diagnostics, id)?;
    let bytes = fs::read(&path).map_err(|error| error.to_string())?;
    let mut run: DiagnosticRun =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    run.retained = retained;
    let bytes = serde_json::to_vec(&run).map_err(|error| error.to_string())?;
    atomic_write(&path, &bytes).map_err(|error| error.to_string())?;
    Ok(id.to_owned())
}

fn delete_run(diagnostics: &Path, id: &str) -> Result<String, String> {
    let path = resolved_run_path(diagnostics, id)?;
    fs::remove_file(path).map_err(|error| error.to_string())?;
    sync_directory(diagnostics).map_err(|error| error.to_string())?;
    Ok(id.to_owned())
}

fn sync_directory(directory: &Path) -> io::Result<()> {
    #[cfg(test)]
    if FAIL_DIRECTORY_SYNC.with(|fail| fail.replace(false)) {
        return Err(io::Error::other("injected directory sync failure"));
    }
    File::open(directory)?.sync_all()
}

#[cfg(test)]
std::thread_local! {
    static FAIL_DIRECTORY_SYNC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::symlink;

    use serde_json::Value;

    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(new_run_id(name))
    }

    fn run(id: &str, outcome: RunOutcome, created: u64) -> DiagnosticRun {
        DiagnosticRun {
            schema_version: 1,
            resource: resource_identity(),
            run_id: id.into(),
            scenario_run_id: "scenario-1".into(),
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
            created_unix_ms: created,
            records: Vec::new(),
        }
    }

    #[test]
    fn recording_off_creates_nothing() {
        let root = temp_root("off");
        let recorder = Recorder {
            mode: RecordingMode::Off,
            root: root.clone(),
            max_bytes: 1,
            success_limit: 0,
            non_success_limit: 0,
        };
        assert_eq!(
            recorder.publish(run("r-1", RunOutcome::Success, 1)).health,
            RecordingHealth::Healthy
        );
        assert!(!root.exists());
    }

    #[test]
    fn phase2_resource_schema_without_role_remains_listable_and_retainable() {
        let root = temp_root("phase2-schema");
        let recorder = Recorder::at_root(root.clone());
        let diagnostics = recorder.prepare_store().unwrap();
        let mut value = serde_json::to_value(run("phase2-run", RunOutcome::Success, 1)).unwrap();
        value["resource"].as_object_mut().unwrap().remove("role");
        atomic_write(
            &diagnostics.join("phase2-run.json"),
            &serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();

        let listed = run_diagnostics_command_at(root.clone(), DiagnosticsCommand::List).unwrap();
        let runs: Vec<DiagnosticRun> = serde_json::from_str(&listed).unwrap();
        assert_eq!(runs[0].resource, resource_identity());
        assert_eq!(
            run_diagnostics_command_at(
                root.clone(),
                DiagnosticsCommand::Retain("phase2-run".into())
            )
            .unwrap(),
            "phase2-run"
        );
        let saved: Value =
            serde_json::from_slice(&fs::read(root.join("diagnostics/phase2-run.json")).unwrap())
                .unwrap();
        assert!(saved["retained"].as_bool().unwrap());
        assert!(saved["resource"].get("role").is_none());
        for phase3_field in [
            "transaction_id",
            "session_context_id",
            "operation_context_id",
            "protocol_major",
            "selected_features",
            "granted_permissions",
        ] {
            assert!(saved.get(phase3_field).is_none());
        }
    }

    #[test]
    fn outcome_retention_and_explicit_pin_are_independent() {
        let root = temp_root("retention");
        let recorder = Recorder {
            mode: RecordingMode::On,
            root: root.clone(),
            max_bytes: MAX_BYTES,
            success_limit: 1,
            non_success_limit: 1,
        };
        let _ = recorder.publish(run("success-1", RunOutcome::Success, 1));
        let mut pinned = run("success-pinned", RunOutcome::Success, 2);
        pinned.retained = true;
        let _ = recorder.publish(pinned);
        let _ = recorder.publish(run("success-2", RunOutcome::Success, 3));
        let _ = recorder.publish(run("error-1", RunOutcome::Error, 4));
        let _ = recorder.publish(run("error-2", RunOutcome::Error, 5));
        let dir = root.join("diagnostics");
        assert!(!dir.join("success-1.json").exists());
        assert!(dir.join("success-pinned.json").exists());
        assert!(dir.join("success-2.json").exists());
        assert!(!dir.join("error-1.json").exists());
        assert!(dir.join("error-2.json").exists());
    }

    #[test]
    fn capacity_exhaustion_is_degraded_not_an_error() {
        let root = temp_root("capacity");
        let recorder = Recorder {
            mode: RecordingMode::On,
            root: root.clone(),
            max_bytes: 1,
            success_limit: 10,
            non_success_limit: 20,
        };
        let publication = recorder.publish(run("r-1", RunOutcome::Error, 1));
        assert_eq!(publication.health, RecordingHealth::CapacityExhausted);
        assert_eq!(publication.completeness, Completeness::Dropped);
        assert_eq!(publication.run_id, "r-1");
        assert!(!publication.stored);
        assert!(!root.join("diagnostics/r-1.json").exists());
    }

    #[test]
    fn capacity_evicts_unretained_success_before_dropping() {
        let root = temp_root("capacity-evict");
        let sample_size = serde_json::to_vec(&run("old", RunOutcome::Success, 1))
            .unwrap()
            .len() as u64;
        let recorder = Recorder {
            mode: RecordingMode::On,
            root: root.clone(),
            max_bytes: sample_size + 64,
            success_limit: 10,
            non_success_limit: 20,
        };
        assert_eq!(
            recorder.publish(run("old", RunOutcome::Success, 1)).health,
            RecordingHealth::Healthy
        );
        assert_eq!(
            recorder.publish(run("new", RunOutcome::Error, 2)).health,
            RecordingHealth::Healthy
        );
        assert!(!root.join("diagnostics/old.json").exists());
        assert!(root.join("diagnostics/new.json").exists());
    }

    #[test]
    fn storage_failure_is_non_interfering_degradation() {
        let root = temp_root("storage-failure");
        fs::write(&root, b"not-a-directory").unwrap();
        let recorder = Recorder {
            mode: RecordingMode::On,
            root,
            max_bytes: MAX_BYTES,
            success_limit: 10,
            non_success_limit: 20,
        };
        let publication = recorder.publish(run("r-1", RunOutcome::Success, 1));
        assert_eq!(publication.health, RecordingHealth::StorageUnavailable);
        assert_eq!(publication.completeness, Completeness::Dropped);
        assert!(!publication.stored);
    }

    #[test]
    fn failed_run_publication_reports_correlated_partial_health() {
        let root = temp_root("partial");
        create_private_dir(&root).unwrap();
        fs::write(root.join("diagnostics"), b"not-a-directory").unwrap();
        let recorder = Recorder {
            mode: RecordingMode::On,
            root: root.clone(),
            max_bytes: MAX_BYTES,
            success_limit: 10,
            non_success_limit: 20,
        };
        let publication = recorder.publish(run("partial-run", RunOutcome::Error, 1));
        assert_eq!(publication.run_id, "partial-run");
        assert_eq!(publication.completeness, Completeness::Partial);
        assert_eq!(publication.health, RecordingHealth::StorageUnavailable);
        let health: Value =
            serde_json::from_slice(&fs::read(root.join("recording-health.json")).unwrap()).unwrap();
        assert_eq!(health["run_id"], "partial-run");
        assert_eq!(health["completeness"], "partial");
    }

    #[test]
    fn symlink_run_is_rejected() {
        let root = temp_root("symlink");
        let recorder = Recorder {
            mode: RecordingMode::On,
            root: root.clone(),
            max_bytes: MAX_BYTES,
            success_limit: 10,
            non_success_limit: 20,
        };
        let diagnostics = recorder.prepare_store().unwrap();
        let target = root.join("outside.json");
        fs::write(&target, b"outside").unwrap();
        symlink(&target, diagnostics.join("linked.json")).unwrap();
        assert!(resolved_run_path(&diagnostics, "linked").is_err());
    }

    #[test]
    fn broken_health_symlink_is_rejected_without_replacement() {
        let root = temp_root("broken-health-link");
        create_private_dir(&root).unwrap();
        fs::write(root.join("diagnostics"), b"blocks directory creation").unwrap();
        let health = root.join("recording-health.json");
        symlink(root.join("missing-target"), &health).unwrap();
        let recorder = Recorder::at_root(root);
        let publication = recorder.publish(run("broken-health", RunOutcome::Error, 1));
        assert_eq!(publication.completeness, Completeness::Dropped);
        assert!(
            fs::symlink_metadata(health)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn in_progress_run_is_recovered_as_partial_crash() {
        let root = temp_root("crash-recovery");
        let recorder = Recorder::at_root(root.clone());
        let mut pending = in_progress_run("pending".into(), "scenario".into(), 1, 0);
        pending.owner = Some(ProcessOwner {
            pid: u32::MAX,
            start_ticks: u64::MAX,
        });
        let _ = recorder.publish(pending);
        let _ = recorder.publish(run("next", RunOutcome::Success, 2));
        let recovered: DiagnosticRun =
            serde_json::from_slice(&fs::read(root.join("diagnostics/pending.json")).unwrap())
                .unwrap();
        assert!(recovered.terminal);
        assert_eq!(recovered.completeness, Completeness::Partial);
        assert_eq!(recovered.missing_reason, Some(MissingReason::Crash));
        assert!(recovered.records.iter().all(|record| {
            record.status == OperationStatus::Error && record.error_type == Some(ErrorType::Crash)
        }));
    }

    #[test]
    fn live_process_marker_is_not_recovered() {
        let root = temp_root("live-owner");
        let recorder = Recorder::at_root(root.clone());
        let mut pending = run("live", RunOutcome::Error, 1);
        pending.terminal = false;
        pending.completeness = Completeness::Partial;
        pending.owner = current_process_owner();
        let _ = recorder.publish(pending);
        let _ = recorder.publish(run("next", RunOutcome::Success, 2));
        let saved: DiagnosticRun =
            serde_json::from_slice(&fs::read(root.join("diagnostics/live.json")).unwrap()).unwrap();
        assert!(!saved.terminal);
        assert_eq!(saved.missing_reason, None);
    }

    #[test]
    fn locked_private_live_marker_replaces_persisted_process_identity() {
        let root = temp_root("private-live-marker");
        let recorder = Recorder::at_root(root.clone());
        let marker = recorder.begin_live_run("private-live").unwrap();
        let mut pending = in_progress_run("private-live".into(), "scenario".into(), 1, 0);
        pending.owner = None;
        assert_eq!(pending.owner, None);
        let _ = recorder.publish(pending);
        let _ = recorder.publish(run("while-live", RunOutcome::Success, 2));
        let path = root.join("diagnostics/private-live.json");
        let live: DiagnosticRun = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(!live.terminal);
        let encoded = serde_json::to_string(&live).unwrap();
        assert!(!encoded.contains("pid"));
        assert!(!encoded.contains("start_ticks"));

        recorder.end_live_run("private-live", marker).unwrap();
        let _ = recorder.publish(run("after-crash", RunOutcome::Success, 3));
        let recovered: DiagnosticRun = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert!(recovered.terminal);
        assert_eq!(recovered.missing_reason, Some(MissingReason::Crash));
    }

    #[test]
    fn publication_reports_existing_run_when_later_store_work_fails() {
        let root = temp_root("stored-truth");
        let recorder = Recorder::at_root(root.clone());
        assert!(recorder.publish(run("same", RunOutcome::Success, 1)).stored);
        symlink(root.join("missing"), root.join("diagnostics/broken.json")).unwrap();
        let publication = recorder.publish(run("same", RunOutcome::Error, 2));
        assert!(publication.stored);
        assert_eq!(publication.health, RecordingHealth::StorageUnavailable);
    }

    #[test]
    fn symlink_store_ancestor_is_rejected() {
        let root = temp_root("symlink-parent");
        let outside = temp_root("outside");
        create_private_dir(&root).unwrap();
        create_private_dir(&outside).unwrap();
        symlink(&outside, root.join("linked")).unwrap();
        let recorder = Recorder {
            mode: RecordingMode::On,
            root: root.join("linked/phase2"),
            max_bytes: MAX_BYTES,
            success_limit: 10,
            non_success_limit: 20,
        };
        assert_eq!(
            recorder.publish(run("r-1", RunOutcome::Success, 1)).health,
            RecordingHealth::StorageUnavailable
        );
        assert!(!outside.join("phase2").exists());
    }

    #[test]
    fn existing_non_private_root_is_rejected_without_chmod() {
        let root = temp_root("existing-mode");
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        let recorder = Recorder {
            mode: RecordingMode::On,
            root: root.clone(),
            max_bytes: MAX_BYTES,
            success_limit: 10,
            non_success_limit: 20,
        };
        let publication = recorder.publish(run("r-1", RunOutcome::Success, 1));
        assert_eq!(publication.health, RecordingHealth::StorageUnavailable);
        assert_eq!(
            fs::metadata(root).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn exact_id_validation_rejects_paths() {
        assert!(!valid_run_id("../all"));
        assert!(!valid_run_id(""));
        assert!(valid_run_id("refresh-0123-abcd"));
    }

    #[test]
    fn retain_unretain_and_exact_delete_update_only_one_run() {
        let root = temp_root("controls");
        let diagnostics = root.join("diagnostics");
        create_private_dir(&diagnostics).unwrap();
        let first = run("first", RunOutcome::Success, 1);
        let second = run("second", RunOutcome::Error, 2);
        atomic_write(
            &diagnostics.join("first.json"),
            &serde_json::to_vec(&first).unwrap(),
        )
        .unwrap();
        atomic_write(
            &diagnostics.join("second.json"),
            &serde_json::to_vec(&second).unwrap(),
        )
        .unwrap();
        set_retained(&diagnostics, "first", true).unwrap();
        let retained: DiagnosticRun =
            serde_json::from_slice(&fs::read(diagnostics.join("first.json")).unwrap()).unwrap();
        assert!(retained.retained);
        set_retained(&diagnostics, "first", false).unwrap();
        delete_run(&diagnostics, "first").unwrap();
        assert!(!diagnostics.join("first.json").exists());
        assert!(diagnostics.join("second.json").exists());
    }

    #[test]
    fn unretain_reapplies_outcome_limit_immediately() {
        let root = temp_root("unretain-limit");
        let diagnostics = root.join("diagnostics");
        create_private_dir(&diagnostics).unwrap();
        let recorder = Recorder {
            mode: RecordingMode::On,
            root,
            max_bytes: MAX_BYTES,
            success_limit: 1,
            non_success_limit: 1,
        };
        for index in 1..=3 {
            let mut item = run(&format!("run-{index}"), RunOutcome::Success, index);
            item.retained = true;
            atomic_write(
                &diagnostics.join(format!("run-{index}.json")),
                &serde_json::to_vec(&item).unwrap(),
            )
            .unwrap();
        }
        for index in 1..=3 {
            unretain_and_enforce(&recorder, &diagnostics, &format!("run-{index}")).unwrap();
        }
        assert_eq!(private_json_entries(&diagnostics).unwrap().len(), 1);
        assert!(diagnostics.join("run-3.json").exists());
    }

    #[test]
    fn stale_atomic_temporary_file_is_recovered_under_lock() {
        let root = temp_root("stale-temp");
        create_private_dir(&root).unwrap();
        let diagnostics = root.join("diagnostics");
        create_private_dir(&diagnostics).unwrap();
        fs::write(diagnostics.join("old.tmp-0"), vec![0_u8; 4096]).unwrap();
        let recorder = Recorder {
            mode: RecordingMode::On,
            root,
            max_bytes: MAX_BYTES,
            success_limit: 10,
            non_success_limit: 20,
        };
        let publication = recorder.publish(run("fresh", RunOutcome::Success, 1));
        assert!(publication.stored);
        assert!(!diagnostics.join("old.tmp-0").exists());
    }

    #[test]
    fn result_path_rejects_multiline_and_non_utf8_values() {
        assert!(validate_single_line_utf8_path(Path::new("line\nbreak")).is_err());
        let non_utf8 = PathBuf::from(OsString::from_vec(vec![b'x', 0xff]));
        assert!(validate_single_line_utf8_path(&non_utf8).is_err());
    }

    #[test]
    fn publication_uses_private_modes_and_leaves_no_temporary_file() {
        let root = temp_root("modes");
        let recorder = Recorder {
            mode: RecordingMode::On,
            root: root.clone(),
            max_bytes: MAX_BYTES,
            success_limit: 10,
            non_success_limit: 20,
        };
        let _ = recorder.publish(run("private", RunOutcome::Success, 1));
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let file = root.join("diagnostics/private.json");
        assert_eq!(
            fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(
            fs::read_dir(root.join("diagnostics"))
                .unwrap()
                .all(|entry| !entry.unwrap().path().to_string_lossy().contains(".tmp-"))
        );
    }

    #[test]
    fn rename_without_parent_sync_is_not_reported_as_success() {
        let root = temp_root("directory-sync");
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let target = root.join("run.json");
        FAIL_DIRECTORY_SYNC.with(|fail| fail.set(true));
        assert!(atomic_write(&target, b"{}").is_err());
        assert!(target.exists(), "rename is the linearization point");
    }

    #[test]
    fn schemas_have_no_free_form_attribute_or_remote_destination() {
        let record = SemanticRecord {
            operation: Operation::PresenterApplyView,
            operation_id: 1,
            parent_operation_id: None,
            status: OperationStatus::Success,
            error_type: None,
            effect_id: None,
            virtual_time_ms: 0,
            end_virtual_time_ms: 0,
            duration_ms: None,
            render_width: Some(320),
            render_height: Some(240),
            value: Some(ClosedValue::Unknown),
        };
        let json = serde_json::to_string(&record).unwrap();
        for forbidden in ["path", "environment", "credential", "remote", "payload"] {
            assert!(!json.contains(forbidden));
        }
    }

    #[test]
    fn all_completion_and_health_classifications_serialize() {
        for outcome in [
            RunOutcome::Success,
            RunOutcome::Error,
            RunOutcome::Cancel,
            RunOutcome::Timeout,
        ] {
            assert!(
                !serde_json::to_vec(&run("r", outcome, 1))
                    .unwrap()
                    .is_empty()
            );
        }
        for completeness in [
            Completeness::Complete,
            Completeness::Partial,
            Completeness::Dropped,
        ] {
            assert!(!serde_json::to_vec(&completeness).unwrap().is_empty());
        }
    }
}
