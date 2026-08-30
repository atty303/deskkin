use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::net::SocketAddr;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use deskkin_protocol::AvailabilityResult;
use local_run_recorder::{
    Completeness, DiagnosticRun, ErrorType, Operation, OperationStatus, Recorder, RecordingHealth,
    RecordingMode, ResourceRole, RunOutcome, SemanticRecord, now_unix_ms, resource_identity_for,
};
use serde::{Deserialize, Serialize};

use crate::owner_control::{
    OwnerCommand, OwnerLaunchMetadata, OwnerResponse, call_owner_control, discover_owner_info,
    try_acquire_owner_lock,
};
use crate::{
    HostExit, HostRuntimeError, SessionError, is_exact_private_lan_address, new_control_id,
    run_profile_host_runtime,
};

const PROFILE_LIMIT: usize = 32;
const PROFILE_BYTES_MAX: u64 = 4 * 1024;
const PROFILE_SCHEMA_VERSION: u8 = 1;
const STATE_ROOT: &str = ".deskkin";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BindMode {
    Loopback,
    PrivateLan,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BindProfile {
    pub mode: BindMode,
    pub address: SocketAddr,
}

impl<'de> Deserialize<'de> for BindProfile {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawBindProfile {
            mode: BindMode,
            address: String,
        }

        let raw = RawBindProfile::deserialize(deserializer)?;
        if raw.address.len() > 64 {
            return Err(serde::de::Error::custom("address exceeds limit"));
        }
        let address = raw
            .address
            .parse::<SocketAddr>()
            .map_err(serde::de::Error::custom)?;
        if address.to_string() != raw.address {
            return Err(serde::de::Error::custom("address is not canonical"));
        }
        Ok(Self {
            mode: raw.mode,
            address,
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileAvailability {
    Available,
    Unavailable,
    ReadFailed,
}

impl From<ProfileAvailability> for AvailabilityResult {
    fn from(value: ProfileAvailability) -> Self {
        match value {
            ProfileAvailability::Available => Self::Available,
            ProfileAvailability::Unavailable => Self::Unavailable,
            ProfileAvailability::ReadFailed => Self::ReadFailed,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileRecording {
    On,
    Off,
}

impl From<ProfileRecording> for RecordingMode {
    fn from(value: ProfileRecording) -> Self {
        match value {
            ProfileRecording::On => Self::On,
            ProfileRecording::Off => Self::Off,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PhysicalProfile {
    pub schema_version: u8,
    pub role_root: String,
    pub bind: BindProfile,
    pub availability: ProfileAvailability,
    pub recording: ProfileRecording,
}

impl PhysicalProfile {
    pub fn new(
        role_root: String,
        mode: BindMode,
        address: SocketAddr,
        availability: ProfileAvailability,
        recording: ProfileRecording,
    ) -> Result<Self, ProfileError> {
        let profile = Self {
            schema_version: PROFILE_SCHEMA_VERSION,
            role_root,
            bind: BindProfile { mode, address },
            availability,
            recording,
        };
        profile.validate()?;
        Ok(profile)
    }

    fn validate(&self) -> Result<(), ProfileError> {
        if self.schema_version != PROFILE_SCHEMA_VERSION {
            return Err(ProfileError::SchemaInvalid);
        }
        validate_role_root(&self.role_root)?;
        if self.bind.address.port() == 0 {
            return Err(ProfileError::AddressInvalid);
        }
        match self.bind.mode {
            BindMode::Loopback if self.bind.address.ip().is_loopback() => Ok(()),
            BindMode::PrivateLan if is_exact_private_lan_address(self.bind.address) => Ok(()),
            _ => Err(ProfileError::AddressInvalid),
        }
    }

    fn launch_metadata(&self, name: &str) -> OwnerLaunchMetadata {
        OwnerLaunchMetadata {
            profile_name: name.to_owned(),
            role_root: self.role_root.clone(),
            bind_mode: match self.bind.mode {
                BindMode::Loopback => "loopback",
                BindMode::PrivateLan => "private_lan",
            }
            .into(),
            bind_address: self.bind.address.to_string(),
            availability: match self.availability {
                ProfileAvailability::Available => "available",
                ProfileAvailability::Unavailable => "unavailable",
                ProfileAvailability::ReadFailed => "read_failed",
            }
            .into(),
            recording: match self.recording {
                ProfileRecording::On => "on",
                ProfileRecording::Off => "off",
            }
            .into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileState {
    Stopped,
    Running,
    ProfileMismatch,
    OwnerUnknown,
}

impl fmt::Display for ProfileState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Stopped => "stopped",
            Self::Running => "running",
            Self::ProfileMismatch => "profile_mismatch",
            Self::OwnerUnknown => "owner_unknown",
        })
    }
}

#[derive(Debug)]
pub enum ProfileError {
    NameInvalid,
    SchemaInvalid,
    AddressInvalid,
    StoreUnsafe,
    NotFound,
    LimitExceeded,
    OwnerBusy,
    ProfileMismatch,
    StaleGeneration,
    OwnerUnknown,
    PublicationUnknown,
    ShutdownRejected,
    ShutdownTimeout,
    Runtime(SessionError),
    Io,
}

impl fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NameInvalid => "profile name is invalid",
            Self::SchemaInvalid => "profile schema is invalid",
            Self::AddressInvalid => "profile address is invalid",
            Self::StoreUnsafe => "profile store is unsafe",
            Self::NotFound => "profile not found",
            Self::LimitExceeded => "profile limit exceeded",
            Self::OwnerBusy => "profile role has a live owner",
            Self::ProfileMismatch => "running owner does not match profile",
            Self::StaleGeneration => "owner generation changed before shutdown",
            Self::OwnerUnknown => "owner state is unknown",
            Self::PublicationUnknown => "profile publication is unknown",
            Self::ShutdownRejected => "owner rejected shutdown",
            Self::ShutdownTimeout => "owner shutdown timed out",
            Self::Runtime(_) => "profile host runtime failed",
            Self::Io => "profile operation failed",
        })
    }
}

impl std::error::Error for ProfileError {}

pub struct ProfileStore {
    state_root: PathBuf,
}

impl ProfileStore {
    #[must_use]
    pub fn at(state_root: PathBuf) -> Self {
        Self { state_root }
    }

    #[must_use]
    pub fn local() -> Self {
        Self::at(PathBuf::from(STATE_ROOT))
    }

    pub fn list(&self) -> Result<Vec<String>, ProfileError> {
        let _guard = self.lock()?;
        self.list_locked()
    }

    pub fn show(&self, name: &str) -> Result<String, ProfileError> {
        let _guard = self.lock()?;
        let profile = self.load_locked(name)?;
        let mut bytes = serde_json::to_vec_pretty(&profile).map_err(|_| ProfileError::Io)?;
        bytes.push(b'\n');
        String::from_utf8(bytes).map_err(|_| ProfileError::Io)
    }

    pub fn set(&self, name: &str, profile: &PhysicalProfile) -> Result<(), ProfileError> {
        self.set_inner(name, profile, PublicationFault::None)
    }

    fn set_inner(
        &self,
        name: &str,
        profile: &PhysicalProfile,
        fault: PublicationFault,
    ) -> Result<(), ProfileError> {
        validate_profile_name(name)?;
        profile.validate()?;
        let _guard = self.lock()?;
        let profiles = self.prepare_profiles()?;
        cleanup_temporary(&profiles)?;
        let existing = match self.load_locked(name) {
            Ok(profile) => Some(profile),
            Err(ProfileError::NotFound) => None,
            Err(error) => return Err(error),
        };
        if existing.is_none() && self.list_locked()?.len() >= PROFILE_LIMIT {
            return Err(ProfileError::LimitExceeded);
        }
        if let Some(existing) = &existing {
            self.refuse_live_owner(existing)?;
        }
        self.refuse_live_owner(profile)?;
        let mut bytes = serde_json::to_vec_pretty(profile).map_err(|_| ProfileError::Io)?;
        bytes.push(b'\n');
        if bytes.len() as u64 > PROFILE_BYTES_MAX {
            return Err(ProfileError::SchemaInvalid);
        }
        let temporary = profiles.join(format!(".tmp-{name}"));
        reject_existing_path(&temporary)?;
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|_| ProfileError::Io)?;
        if file
            .write_all(&bytes)
            .and_then(|()| file.sync_all())
            .is_err()
        {
            let _ = fs::remove_file(&temporary);
            return Err(ProfileError::Io);
        }
        if fault == PublicationFault::BeforeRename {
            let _ = fs::remove_file(&temporary);
            return Err(ProfileError::Io);
        }
        let destination = profile_path(&profiles, name)?;
        if fs::rename(&temporary, &destination).is_err() {
            let _ = fs::remove_file(&temporary);
            return Err(ProfileError::Io);
        }
        if fault == PublicationFault::AfterRename {
            return Err(ProfileError::PublicationUnknown);
        }
        if sync_directory(&profiles).is_err()
            || !matches!(self.load_locked(name), Ok(stored) if stored == *profile)
        {
            return Err(ProfileError::PublicationUnknown);
        }
        Ok(())
    }

    pub fn delete(&self, name: &str) -> Result<(), ProfileError> {
        let _guard = self.lock()?;
        let profile = self.load_locked(name)?;
        self.refuse_live_owner(&profile)?;
        let profiles = self.prepare_profiles()?;
        let path = profile_path(&profiles, name)?;
        validate_profile_file(&path)?;
        fs::remove_file(path).map_err(|_| ProfileError::Io)?;
        sync_directory(&profiles).map_err(|_| ProfileError::PublicationUnknown)
    }

    pub fn status(&self, name: &str) -> Result<ProfileState, ProfileError> {
        let _guard = self.lock()?;
        let profile = self.load_for_operation(name, Operation::ProfileStatus)?;
        let role_root = self.resolve_for_operation(&profile, Operation::ProfileStatus)?;
        let expected = profile.launch_metadata(name);
        let state = match discover_owner_info(&role_root.join("control")) {
            Ok(None) => ProfileState::Stopped,
            Ok(Some(info)) if info.launch.as_ref() == Some(&expected) => ProfileState::Running,
            Ok(Some(_)) => ProfileState::ProfileMismatch,
            Err(_) => ProfileState::OwnerUnknown,
        };
        if role_root.exists() {
            let (outcome, error_type) = match state {
                ProfileState::Running | ProfileState::Stopped => (RunOutcome::Success, None),
                ProfileState::ProfileMismatch => {
                    (RunOutcome::Error, Some(ErrorType::ProfileMismatch))
                }
                ProfileState::OwnerUnknown => (RunOutcome::Error, Some(ErrorType::OwnerLost)),
            };
            record_single_operation(
                &role_root,
                profile.recording.into(),
                Operation::ProfileStatus,
                outcome,
                error_type,
            );
        }
        Ok(state)
    }

    pub fn stop(&self, name: &str) -> Result<(), ProfileError> {
        let _guard = self.lock()?;
        let profile = self.load_for_operation(name, Operation::ProfileStop)?;
        let role_root = self.resolve_for_operation(&profile, Operation::ProfileStop)?;
        let control = role_root.join("control");
        let expected = profile.launch_metadata(name);
        let result = (|| {
            let info = discover_owner_info(&control)
                .map_err(|_| ProfileError::OwnerUnknown)?
                .ok_or(ProfileError::OwnerUnknown)?;
            if info.launch.as_ref() != Some(&expected) {
                return Err(ProfileError::ProfileMismatch);
            }
            let response = call_owner_control(
                &control,
                &OwnerCommand::Shutdown {
                    owner_generation: info.owner_generation,
                },
            )
            .map_err(|_| ProfileError::ShutdownRejected)?;
            classify_shutdown_response(&response)?;
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match try_acquire_owner_lock(&control) {
                    Ok(Some(_)) if !control.join("owner.sock").exists() => break,
                    Ok(_) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Ok(_) => return Err(ProfileError::ShutdownTimeout),
                    Err(_) => return Err(ProfileError::OwnerUnknown),
                }
            }
            Ok(())
        })();
        let (outcome, error_type) = match &result {
            Ok(()) => (RunOutcome::Success, None),
            Err(error) => (RunOutcome::Error, Some(profile_error_type(error))),
        };
        record_single_operation(
            &role_root,
            profile.recording.into(),
            Operation::ProfileStop,
            outcome,
            error_type,
        );
        result
    }

    pub fn run(&self, name: &str) -> Result<(), ProfileError> {
        let guard = self.lock()?;
        let profile = self.load_for_operation(name, Operation::ProfileResolve)?;
        let role_root = self.resolve_for_operation(&profile, Operation::ProfileResolve)?;
        let metadata = profile.launch_metadata(name);
        let lifecycle = LifecycleRun::start(&role_root, profile.recording.into());
        let context = lifecycle.as_ref().map(|run| run.run_id.clone());
        let result = run_profile_host_runtime(
            profile.bind.address,
            &role_root,
            profile.bind.mode,
            profile.availability.into(),
            profile.recording.into(),
            metadata,
            Some(guard),
            context,
        );
        if let Some(lifecycle) = lifecycle {
            lifecycle.finish(result.as_ref().copied(), result.as_ref().err().copied());
        }
        result
            .map(|_| ())
            .map_err(|failure| ProfileError::Runtime(failure.error))
    }

    fn lock(&self) -> Result<File, ProfileError> {
        prepare_private_directory(&self.state_root)?;
        let control = self.state_root.join("profile-control");
        prepare_private_directory(&control)?;
        let path = control.join("operation.lock");
        reject_symlink_if_exists(&path)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .map_err(|_| ProfileError::Io)?;
        file.lock().map_err(|_| ProfileError::Io)?;
        Ok(file)
    }

    fn load_for_operation(
        &self,
        name: &str,
        operation: Operation,
    ) -> Result<PhysicalProfile, ProfileError> {
        match self.load_locked(name) {
            Ok(profile) => Ok(profile),
            Err(error) => {
                record_single_operation(
                    &self.state_root.join("profile-control"),
                    RecordingMode::On,
                    operation,
                    RunOutcome::Error,
                    Some(profile_error_type(&error)),
                );
                Err(error)
            }
        }
    }

    fn prepare_profiles(&self) -> Result<PathBuf, ProfileError> {
        let profiles = self.state_root.join("profiles");
        prepare_private_directory(&profiles)?;
        Ok(profiles)
    }

    fn list_locked(&self) -> Result<Vec<String>, ProfileError> {
        let profiles = self.prepare_profiles()?;
        cleanup_temporary(&profiles)?;
        let mut names = Vec::new();
        for entry in fs::read_dir(profiles).map_err(|_| ProfileError::Io)? {
            let entry = entry.map_err(|_| ProfileError::Io)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| ProfileError::StoreUnsafe)?;
            let Some(name) = name.strip_suffix(".json") else {
                return Err(ProfileError::StoreUnsafe);
            };
            validate_profile_name(name)?;
            validate_profile_file(&entry.path())?;
            self.load_locked(name)?;
            names.push(name.to_owned());
        }
        if names.len() > PROFILE_LIMIT {
            return Err(ProfileError::LimitExceeded);
        }
        names.sort();
        Ok(names)
    }

    fn load_locked(&self, name: &str) -> Result<PhysicalProfile, ProfileError> {
        let profiles = self.prepare_profiles()?;
        let path = profile_path(&profiles, name)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(ProfileError::NotFound);
            }
            Err(_) => return Err(ProfileError::Io),
        };
        validate_profile_metadata(&metadata)?;
        let bytes = fs::read(path).map_err(|_| ProfileError::Io)?;
        let mut deserializer = serde_json::Deserializer::from_slice(&bytes);
        let profile = PhysicalProfile::deserialize(&mut deserializer)
            .map_err(|_| ProfileError::SchemaInvalid)?;
        deserializer
            .end()
            .map_err(|_| ProfileError::SchemaInvalid)?;
        profile.validate()?;
        Ok(profile)
    }

    fn resolve_role_root(&self, profile: &PhysicalProfile) -> Result<PathBuf, ProfileError> {
        validate_role_root(&profile.role_root)?;
        reject_symlink_components(&self.state_root)?;
        let role_root = self.state_root.join(&profile.role_root);
        reject_symlink_components(&role_root)?;
        Ok(role_root)
    }

    fn resolve_for_operation(
        &self,
        profile: &PhysicalProfile,
        operation: Operation,
    ) -> Result<PathBuf, ProfileError> {
        self.resolve_role_root(profile).inspect_err(|error| {
            record_single_operation(
                &self.state_root.join("profile-control"),
                RecordingMode::On,
                operation,
                RunOutcome::Error,
                Some(profile_error_type(error)),
            );
        })
    }

    fn refuse_live_owner(&self, profile: &PhysicalProfile) -> Result<(), ProfileError> {
        let control = self.resolve_role_root(profile)?.join("control");
        match fs::symlink_metadata(&control) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(ProfileError::OwnerUnknown),
        }
        match discover_owner_info(&control) {
            Ok(Some(_)) => Err(ProfileError::OwnerBusy),
            Ok(None) => Ok(()),
            Err(_) => Err(ProfileError::OwnerUnknown),
        }
    }
}

fn classify_shutdown_response(response: &OwnerResponse) -> Result<(), ProfileError> {
    match response {
        OwnerResponse::ShutdownAccepted => Ok(()),
        OwnerResponse::StaleOwner => Err(ProfileError::StaleGeneration),
        _ => Err(ProfileError::ShutdownRejected),
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum PublicationFault {
    None,
    BeforeRename,
    AfterRename,
}

pub fn managed_startup_barrier(role_root: &Path) -> Result<Option<File>, ProfileError> {
    let state_root = managed_state_root(role_root);
    state_root
        .map(|root| ProfileStore::at(root).lock())
        .transpose()
}

fn managed_state_root(role_root: &Path) -> Option<PathBuf> {
    let mut root = PathBuf::new();
    for component in role_root.components() {
        root.push(component);
        if component.as_os_str() == STATE_ROOT {
            return Some(root);
        }
    }
    None
}

fn validate_profile_name(name: &str) -> Result<(), ProfileError> {
    if name.is_empty()
        || name.len() > 32
        || name.starts_with('-')
        || name.ends_with('-')
        || name.contains("--")
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(ProfileError::NameInvalid);
    }
    Ok(())
}

fn validate_role_root(value: &str) -> Result<(), ProfileError> {
    let path = Path::new(value);
    if value.is_empty() || path.is_absolute() {
        return Err(ProfileError::SchemaInvalid);
    }
    let components: Vec<_> = path.components().collect();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ProfileError::SchemaInvalid);
    }
    let normalized = components
        .iter()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if normalized != value
        || matches!(
            components[0].as_os_str().to_str(),
            Some("profiles" | "profile-control")
        )
    {
        return Err(ProfileError::SchemaInvalid);
    }
    Ok(())
}

fn profile_path(profiles: &Path, name: &str) -> Result<PathBuf, ProfileError> {
    validate_profile_name(name)?;
    Ok(profiles.join(format!("{name}.json")))
}

fn validate_profile_file(path: &Path) -> Result<(), ProfileError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ProfileError::Io)?;
    validate_profile_metadata(&metadata)
}

fn validate_profile_metadata(metadata: &fs::Metadata) -> Result<(), ProfileError> {
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.permissions().mode() & 0o777 != 0o600
        || metadata.len() > PROFILE_BYTES_MAX
    {
        return Err(ProfileError::StoreUnsafe);
    }
    Ok(())
}

fn prepare_private_directory(path: &Path) -> Result<(), ProfileError> {
    reject_symlink_if_exists(path)?;
    if !path.exists() {
        fs::create_dir_all(path).map_err(|_| ProfileError::Io)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| ProfileError::Io)?;
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| ProfileError::Io)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(ProfileError::StoreUnsafe);
    }
    Ok(())
}

fn reject_symlink_if_exists(path: &Path) -> Result<(), ProfileError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(ProfileError::StoreUnsafe),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(ProfileError::Io),
    }
}

fn reject_existing_path(path: &Path) -> Result<(), ProfileError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(ProfileError::StoreUnsafe),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(ProfileError::Io),
    }
}

fn cleanup_temporary(profiles: &Path) -> Result<(), ProfileError> {
    for entry in fs::read_dir(profiles).map_err(|_| ProfileError::Io)? {
        let entry = entry.map_err(|_| ProfileError::Io)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| ProfileError::StoreUnsafe)?;
        if let Some(name) = name.strip_prefix(".tmp-") {
            validate_profile_name(name)?;
            validate_profile_file(&entry.path())?;
            fs::remove_file(entry.path()).map_err(|_| ProfileError::Io)?;
        }
    }
    sync_directory(profiles).map_err(|_| ProfileError::Io)
}

fn reject_symlink_components(path: &Path) -> Result<(), ProfileError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(ProfileError::StoreUnsafe);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(_) => return Err(ProfileError::Io),
        }
    }
    Ok(())
}

fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

struct LifecycleRun {
    recorder: Recorder,
    marker: Option<File>,
    run_id: String,
    created_unix_ms: u64,
    started: Instant,
}

impl LifecycleRun {
    fn start(role_root: &Path, recording: RecordingMode) -> Option<Self> {
        let run_id = format!("profile-{}", new_control_id().ok()?);
        let recorder = Recorder::new(role_root.to_path_buf(), recording, 16 * 1024 * 1024);
        let marker = (recording == RecordingMode::On)
            .then(|| recorder.begin_live_run(&run_id).ok())
            .flatten();
        let created_unix_ms = now_unix_ms();
        let partial = lifecycle_run(
            run_id.clone(),
            created_unix_ms,
            RunOutcome::Error,
            Completeness::Partial,
            None,
            None,
            None,
        );
        let _ = recorder.publish(partial);
        Some(Self {
            recorder,
            marker,
            run_id,
            created_unix_ms,
            started: Instant::now(),
        })
    }

    fn finish(self, exit: Result<HostExit, &HostRuntimeError>, failure: Option<HostRuntimeError>) {
        let outcome = match exit {
            Ok(HostExit::Stopped) => RunOutcome::Success,
            Ok(HostExit::Interrupted) => RunOutcome::Cancel,
            Err(_) => RunOutcome::Error,
        };
        let run = lifecycle_run(
            self.run_id.clone(),
            self.created_unix_ms,
            outcome,
            Completeness::Complete,
            match exit {
                Ok(HostExit::Interrupted) => Some(ErrorType::Cancelled),
                _ => failure.map(|failure| session_error_type(failure.error)),
            },
            Some(u32::try_from(self.started.elapsed().as_millis()).unwrap_or(u32::MAX)),
            failure.map(|failure| failure.stage),
        );
        let _ = self.recorder.publish(run);
        if let Some(marker) = self.marker {
            let _ = self.recorder.end_live_run(&self.run_id, marker);
        }
    }
}

fn lifecycle_run(
    run_id: String,
    created_unix_ms: u64,
    outcome: RunOutcome,
    completeness: Completeness,
    error_type: Option<ErrorType>,
    duration_ms: Option<u32>,
    failed_stage: Option<Operation>,
) -> DiagnosticRun {
    let terminal = completeness == Completeness::Complete;
    let operations = [
        Operation::ProfileResolve,
        Operation::HostOwnerAcquire,
        Operation::HostBind,
        Operation::HostRuntimeStart,
        Operation::HostRuntimeStop,
        Operation::HostOwnerRelease,
    ];
    DiagnosticRun {
        schema_version: 1,
        resource: resource_identity_for(
            "deskkin-desktop-host",
            env!("CARGO_PKG_VERSION"),
            ResourceRole::Host,
        ),
        scenario_run_id: run_id.clone(),
        run_id,
        transaction_id: None,
        session_context_id: None,
        operation_context_id: None,
        protocol_major: None,
        selected_features: None,
        granted_permissions: None,
        outcome,
        completeness,
        health: RecordingHealth::Healthy,
        terminal,
        missing_reason: None,
        owner: None,
        retained: false,
        created_unix_ms,
        records: operations
            .into_iter()
            .enumerate()
            .map(|(index, operation)| SemanticRecord {
                operation,
                operation_id: u16::try_from(index + 1).unwrap_or(u16::MAX),
                parent_operation_id: (index != 0).then_some(1),
                status: lifecycle_operation_status(
                    terminal,
                    outcome,
                    operation,
                    failed_stage,
                    &operations[..index],
                ),
                error_type: (terminal
                    && (failed_stage == Some(operation)
                        || failed_stage.is_none()
                            && ((outcome == RunOutcome::Cancel
                                && operation == Operation::HostRuntimeStop)
                                || (outcome != RunOutcome::Cancel
                                    && index + 1 == operations.len()))))
                .then_some(error_type)
                .flatten(),
                effect_id: None,
                virtual_time_ms: 0,
                end_virtual_time_ms: 0,
                duration_ms: terminal.then_some(duration_ms).flatten(),
                render_width: None,
                render_height: None,
                value: None,
            })
            .collect(),
    }
}

fn lifecycle_operation_status(
    terminal: bool,
    outcome: RunOutcome,
    operation: Operation,
    failed_stage: Option<Operation>,
    preceding: &[Operation],
) -> OperationStatus {
    if !terminal {
        return OperationStatus::InProgress;
    }
    if let Some(stage) = failed_stage {
        if operation == stage {
            return OperationStatus::Error;
        }
        if operation == Operation::HostOwnerRelease
            && preceding.iter().position(|item| *item == stage)
                > preceding
                    .iter()
                    .position(|item| *item == Operation::HostOwnerAcquire)
        {
            return OperationStatus::Success;
        }
        return if preceding.contains(&stage) {
            OperationStatus::Cancel
        } else {
            OperationStatus::Success
        };
    }
    if outcome == RunOutcome::Cancel && operation == Operation::HostRuntimeStop {
        OperationStatus::Cancel
    } else {
        OperationStatus::Success
    }
}

fn record_single_operation(
    role_root: &Path,
    recording: RecordingMode,
    operation: Operation,
    outcome: RunOutcome,
    error_type: Option<ErrorType>,
) {
    let Ok(id) = new_control_id() else { return };
    let run_id = format!("profile-{id}");
    let status = match outcome {
        RunOutcome::Success => OperationStatus::Success,
        RunOutcome::Error => OperationStatus::Error,
        RunOutcome::Cancel => OperationStatus::Cancel,
        RunOutcome::Timeout => OperationStatus::Timeout,
    };
    let recorder = Recorder::new(role_root.to_path_buf(), recording, 16 * 1024 * 1024);
    let _ = recorder.publish(DiagnosticRun {
        schema_version: 1,
        resource: resource_identity_for(
            "deskkin-desktop-host",
            env!("CARGO_PKG_VERSION"),
            ResourceRole::Host,
        ),
        scenario_run_id: run_id.clone(),
        run_id,
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
        records: vec![SemanticRecord {
            operation,
            operation_id: 1,
            parent_operation_id: None,
            status,
            error_type,
            effect_id: None,
            virtual_time_ms: 0,
            end_virtual_time_ms: 0,
            duration_ms: Some(0),
            render_width: None,
            render_height: None,
            value: None,
        }],
    });
}

const fn session_error_type(error: SessionError) -> ErrorType {
    match error {
        SessionError::NonLoopback | SessionError::NonPrivateLan => ErrorType::InvalidAddress,
        SessionError::Identity => ErrorType::IdentityStore,
        SessionError::Timeout => ErrorType::Timeout,
        SessionError::QueueFull => ErrorType::QueueFull,
        _ => ErrorType::ConnectionLost,
    }
}

const fn profile_error_type(error: &ProfileError) -> ErrorType {
    match error {
        ProfileError::SchemaInvalid | ProfileError::NameInvalid => ErrorType::ProfileSchemaInvalid,
        ProfileError::AddressInvalid => ErrorType::InvalidAddress,
        ProfileError::OwnerBusy => ErrorType::OwnerBusy,
        ProfileError::ProfileMismatch => ErrorType::ProfileMismatch,
        ProfileError::StaleGeneration => ErrorType::StaleGeneration,
        ProfileError::ShutdownRejected => ErrorType::ShutdownRejected,
        ProfileError::OwnerUnknown => ErrorType::OwnerLost,
        ProfileError::ShutdownTimeout => ErrorType::Timeout,
        ProfileError::Runtime(error) => session_error_type(*error),
        ProfileError::StoreUnsafe
        | ProfileError::NotFound
        | ProfileError::LimitExceeded
        | ProfileError::PublicationUnknown
        | ProfileError::Io => ErrorType::StoreFailed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::fs::symlink;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::Arc;
    use std::thread;

    use crate::{ClientSession, IdentityStore, bind_loopback, pair_initiator, pair_responder};

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "deskkin-profile-{name}-{}-{}",
            std::process::id(),
            now_unix_ms()
        ))
    }

    fn profile(role: &str) -> PhysicalProfile {
        PhysicalProfile::new(
            role.into(),
            BindMode::Loopback,
            "127.0.0.1:39032".parse().unwrap(),
            ProfileAvailability::Available,
            ProfileRecording::Off,
        )
        .unwrap()
    }

    #[test]
    fn schema_is_closed_and_paths_are_reserved() {
        assert!(classify_shutdown_response(&OwnerResponse::ShutdownAccepted).is_ok());
        assert!(matches!(
            classify_shutdown_response(&OwnerResponse::StaleOwner),
            Err(ProfileError::StaleGeneration)
        ));
        assert!(matches!(
            classify_shutdown_response(&OwnerResponse::OperationFailed),
            Err(ProfileError::ShutdownRejected)
        ));
        for name in ["", "Upper", "-leading", "trailing-", "two--hyphens"] {
            assert!(validate_profile_name(name).is_err());
        }
        assert!(validate_profile_name("a2345678901234567890123456789012").is_ok());
        assert!(validate_profile_name("a23456789012345678901234567890123").is_err());
        for role in ["", "/absolute", "a/../b", "a//b", "profile-control/host"] {
            assert!(validate_role_root(role).is_err());
        }
        assert!(
            PhysicalProfile::new(
                "profiles/host".into(),
                BindMode::Loopback,
                "127.0.0.1:39032".parse().unwrap(),
                ProfileAvailability::Available,
                ProfileRecording::On,
            )
            .is_err()
        );
        assert!(
            PhysicalProfile::new(
                "host".into(),
                BindMode::PrivateLan,
                format!("10.0.0.2:{}", crate::PRIVATE_LAN_PORT + 1)
                    .parse()
                    .unwrap(),
                ProfileAvailability::Available,
                ProfileRecording::On,
            )
            .is_err()
        );
        assert!(serde_json::from_str::<PhysicalProfile>(
            r#"{"schema_version":1,"role_root":"host","bind":{"mode":"loopback","address":"127.0.0.1:39032"},"availability":"available","recording":"on","password":"x"}"#
        )
        .is_err());
        assert!(serde_json::from_str::<PhysicalProfile>(
            r#"{"schema_version":1,"schema_version":1,"role_root":"host","bind":{"mode":"loopback","address":"[::1]:39032"},"availability":"available","recording":"off"}"#
        )
        .is_err());
        assert!(
            PhysicalProfile::new(
                "host".into(),
                BindMode::PrivateLan,
                "192.168.1.2:39042".parse().unwrap(),
                ProfileAvailability::Unavailable,
                ProfileRecording::Off,
            )
            .is_ok()
        );
    }

    #[test]
    fn store_round_trips_sorted_profiles_and_exact_delete() {
        let root = temp("round-trip");
        let _ = fs::remove_dir_all(&root);
        let store = ProfileStore::at(root.clone());
        store.set("zeta", &profile("roles/zeta")).unwrap();
        store.set("alpha", &profile("roles/alpha")).unwrap();
        assert_eq!(store.list().unwrap(), ["alpha", "zeta"]);
        assert_eq!(
            serde_json::from_str::<PhysicalProfile>(&store.show("alpha").unwrap()).unwrap(),
            profile("roles/alpha")
        );
        store.delete("alpha").unwrap();
        assert_eq!(store.list().unwrap(), ["zeta"]);
        assert_eq!(
            fs::symlink_metadata(root.join("profiles/zeta.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn store_enforces_file_count_size_encoding_permissions_and_trailing_input() {
        let root = temp("store-bounds");
        let _ = fs::remove_dir_all(&root);
        let store = ProfileStore::at(root.clone());
        for index in 0..PROFILE_LIMIT {
            let name = format!("p{index:02}");
            store
                .set(&name, &profile(&format!("roles/{name}")))
                .unwrap();
        }
        assert!(matches!(
            store.set("overflow", &profile("roles/overflow")),
            Err(ProfileError::LimitExceeded)
        ));

        let path = root.join("profiles/p00.json");
        fs::write(
            &path,
            vec![b'x'; usize::try_from(PROFILE_BYTES_MAX).unwrap() + 1],
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(matches!(store.show("p00"), Err(ProfileError::StoreUnsafe)));
        fs::write(&path, b"\xff").unwrap();
        assert!(matches!(
            store.show("p00"),
            Err(ProfileError::SchemaInvalid)
        ));
        fs::write(
            &path,
            br#"{"schema_version":1,"role_root":"roles/p00","bind":{"mode":"loopback","address":"127.0.0.1:39032"},"availability":"available","recording":"off"} true"#,
        )
        .unwrap();
        assert!(matches!(
            store.show("p00"),
            Err(ProfileError::SchemaInvalid)
        ));
        fs::write(
            &path,
            br#"{"schema_version":1,"role_root":"roles/p00","bind":{"mode":"loopback","address":"[0:0:0:0:0:0:0:1]:39032"},"availability":"available","recording":"off"}"#,
        )
        .unwrap();
        assert!(matches!(
            store.show("p00"),
            Err(ProfileError::SchemaInvalid)
        ));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(store.show("p00"), Err(ProfileError::StoreUnsafe)));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn publication_faults_are_closed_and_temporary_files_are_recovered() {
        let root = temp("publication-faults");
        let _ = fs::remove_dir_all(&root);
        let store = ProfileStore::at(root.clone());
        let original = profile("roles/original");
        let replacement = profile("roles/replacement");
        store.set("physical", &original).unwrap();

        assert!(matches!(
            store.set_inner("physical", &replacement, PublicationFault::BeforeRename),
            Err(ProfileError::Io)
        ));
        assert_eq!(
            serde_json::from_str::<PhysicalProfile>(&store.show("physical").unwrap()).unwrap(),
            original
        );
        assert!(matches!(
            store.set_inner("physical", &replacement, PublicationFault::AfterRename),
            Err(ProfileError::PublicationUnknown)
        ));
        assert_eq!(
            serde_json::from_str::<PhysicalProfile>(&store.show("physical").unwrap()).unwrap(),
            replacement
        );
        fs::write(root.join("profiles/.tmp-interrupted"), b"partial").unwrap();
        fs::set_permissions(
            root.join("profiles/.tmp-interrupted"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        assert_eq!(store.list().unwrap(), ["physical"]);
        assert!(!root.join("profiles/.tmp-interrupted").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn store_rejects_symlink_and_unknown_entry() {
        let root = temp("unsafe");
        let _ = fs::remove_dir_all(&root);
        let store = ProfileStore::at(root.clone());
        store.set("safe", &profile("roles/safe")).unwrap();
        fs::write(root.join("profiles/unknown"), b"x").unwrap();
        assert!(matches!(store.list(), Err(ProfileError::StoreUnsafe)));
        fs::remove_file(root.join("profiles/unknown")).unwrap();
        let target = root.join("target");
        fs::write(&target, b"{}").unwrap();
        symlink(target, root.join("profiles/link.json")).unwrap();
        assert!(matches!(store.list(), Err(ProfileError::StoreUnsafe)));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn every_role_operation_rejects_a_symlinked_role_root() {
        let base = temp("role-symlink");
        let root = base.join(STATE_ROOT);
        let outside = base.join("outside");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&outside).unwrap();
        let store = ProfileStore::at(root.clone());
        let physical = profile("roles/physical");
        store.set("physical", &physical).unwrap();
        fs::create_dir_all(root.join("roles")).unwrap();
        symlink(&outside, root.join("roles/physical")).unwrap();

        assert!(matches!(
            store.set("physical", &physical),
            Err(ProfileError::StoreUnsafe)
        ));
        assert!(matches!(
            store.delete("physical"),
            Err(ProfileError::StoreUnsafe)
        ));
        assert!(matches!(
            store.status("physical"),
            Err(ProfileError::StoreUnsafe)
        ));
        assert!(matches!(
            store.stop("physical"),
            Err(ProfileError::StoreUnsafe)
        ));
        assert!(matches!(
            store.run("physical"),
            Err(ProfileError::StoreUnsafe)
        ));
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn every_owner_probe_rejects_symlinked_control_and_socket_paths() {
        for kind in ["control", "dangling-control", "socket"] {
            let base = temp(&format!("owner-symlink-{kind}"));
            let root = base.join(STATE_ROOT);
            let role_root = root.join("roles/physical");
            let outside = base.join("outside");
            let _ = fs::remove_dir_all(&base);
            fs::create_dir_all(&outside).unwrap();
            let store = ProfileStore::at(root);
            let physical = profile("roles/physical");
            store.set("physical", &physical).unwrap();
            fs::create_dir_all(&role_root).unwrap();
            fs::set_permissions(&role_root, fs::Permissions::from_mode(0o700)).unwrap();
            if kind.ends_with("control") {
                let target = if kind == "control" {
                    outside.clone()
                } else {
                    outside.join("missing")
                };
                symlink(target, role_root.join("control")).unwrap();
            } else {
                let control = role_root.join("control");
                fs::create_dir(&control).unwrap();
                fs::set_permissions(&control, fs::Permissions::from_mode(0o700)).unwrap();
                let target = outside.join("owner.sock");
                fs::write(&target, b"unchanged").unwrap();
                symlink(&target, control.join("owner.sock")).unwrap();
            }

            assert_eq!(
                store.status("physical").unwrap(),
                ProfileState::OwnerUnknown
            );
            assert!(matches!(
                store.stop("physical"),
                Err(ProfileError::OwnerUnknown)
            ));
            assert!(matches!(
                store.set("physical", &physical),
                Err(ProfileError::OwnerUnknown)
            ));
            assert!(matches!(
                store.delete("physical"),
                Err(ProfileError::OwnerUnknown)
            ));
            if kind == "socket" {
                assert_eq!(fs::read(outside.join("owner.sock")).unwrap(), b"unchanged");
            } else if kind == "control" {
                assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
            }
            fs::remove_dir_all(base).unwrap();
        }
    }

    #[test]
    fn occupied_bind_fails_bounded_and_releases_the_owner() {
        let base = temp("occupied-bind");
        let root = base.join(STATE_ROOT);
        let role = "roles/physical";
        let role_root = root.join(role);
        let _ = fs::remove_dir_all(&base);
        IdentityStore::new(role_root.join("identity"))
            .init()
            .unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let occupied = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = occupied.local_addr().unwrap();
        let physical = PhysicalProfile::new(
            role.into(),
            BindMode::Loopback,
            address,
            ProfileAvailability::Available,
            ProfileRecording::On,
        )
        .unwrap();
        let store = ProfileStore::at(root);
        store.set("physical", &physical).unwrap();

        let started = Instant::now();
        assert!(matches!(
            store.run("physical"),
            Err(ProfileError::Runtime(SessionError::Io))
        ));
        assert!(started.elapsed() < Duration::from_secs(3));
        let control = role_root.join("control");
        assert!(!control.join("owner.sock").exists());
        assert!(try_acquire_owner_lock(&control).unwrap().is_some());

        let runs = fs::read_dir(role_root.join("diagnostics"))
            .unwrap()
            .map(|entry| {
                serde_json::from_slice::<DiagnosticRun>(&fs::read(entry.unwrap().path()).unwrap())
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let lifecycle = runs
            .iter()
            .find(|run| {
                run.completeness == Completeness::Complete
                    && run
                        .records
                        .iter()
                        .any(|record| record.operation == Operation::ProfileResolve)
            })
            .unwrap();
        assert_eq!(lifecycle.outcome, RunOutcome::Error);
        let bind = lifecycle
            .records
            .iter()
            .find(|record| record.operation == Operation::HostBind)
            .unwrap();
        assert_eq!(bind.status, OperationStatus::Error);
        assert!(lifecycle.records.iter().any(|record| {
            record.operation == Operation::HostRuntimeStart
                && record.status == OperationStatus::Cancel
        }));
        assert!(lifecycle.records.iter().any(|record| {
            record.operation == Operation::HostRuntimeStop
                && record.status == OperationStatus::Cancel
        }));
        assert!(lifecycle.records.iter().any(|record| {
            record.operation == Operation::HostOwnerRelease
                && record.status == OperationStatus::Success
        }));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn unresolved_profile_records_only_a_closed_fallback_error() {
        let root = temp("fallback");
        let _ = fs::remove_dir_all(&root);
        let store = ProfileStore::at(root.clone());
        assert!(matches!(
            store.status("missing-private-name"),
            Err(ProfileError::NotFound)
        ));
        let entries = fs::read_dir(root.join("profile-control/diagnostics"))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(entries.len(), 1);
        let bytes = fs::read(entries[0].path()).unwrap();
        let run: DiagnosticRun = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(run.records[0].operation, Operation::ProfileStatus);
        assert_eq!(run.records[0].error_type, Some(ErrorType::StoreFailed));
        assert!(!String::from_utf8_lossy(&bytes).contains("missing-private-name"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn profile_launch_status_authenticated_read_stop_and_relaunch() {
        let root = temp("lifecycle");
        let _ = fs::remove_dir_all(&root);
        let role = "roles/physical";
        let role_root = root.join(role);
        let host = IdentityStore::new(role_root.join("identity"));
        let client = IdentityStore::new(root.join("client/identity"));
        host.init().unwrap();
        client.init().unwrap();

        let pairing_listener = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
        let pairing_address = pairing_listener.local_addr().unwrap();
        let responder_store = host.clone();
        let responder =
            thread::spawn(move || pair_responder(&pairing_listener, &responder_store, |_, _| true));
        pair_initiator(pairing_address, &client, [51; 16], |_, _| true).unwrap();
        responder.join().unwrap().unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();

        let probe = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = probe.local_addr().unwrap();
        drop(probe);
        let profile = PhysicalProfile::new(
            role.into(),
            BindMode::Loopback,
            address,
            ProfileAvailability::Available,
            ProfileRecording::On,
        )
        .unwrap();
        ProfileStore::at(root.clone())
            .set("physical", &profile)
            .unwrap();

        for iteration in 0..2 {
            let runtime_root = root.clone();
            let runtime = thread::spawn(move || ProfileStore::at(runtime_root).run("physical"));
            let store = ProfileStore::at(root.clone());
            for _ in 0..300 {
                if store.status("physical").unwrap() == ProfileState::Running {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(store.status("physical").unwrap(), ProfileState::Running);
            assert!(matches!(
                store.set("physical", &profile),
                Err(ProfileError::OwnerBusy)
            ));
            assert!(matches!(
                store.delete("physical"),
                Err(ProfileError::OwnerBusy)
            ));
            let mut session = (0..200)
                .find_map(|_| {
                    ClientSession::connect(address, &client, [52 + iteration; 16])
                        .inspect_err(|_| thread::sleep(Duration::from_millis(10)))
                        .ok()
                })
                .unwrap();
            assert_eq!(
                session.read_availability([62 + iteration; 16]).unwrap(),
                AvailabilityResult::Available
            );
            session.close().unwrap();
            store.stop("physical").unwrap();
            assert!(std::net::TcpStream::connect(address).is_err());
            runtime.join().unwrap().unwrap();
            assert_eq!(store.status("physical").unwrap(), ProfileState::Stopped);
            assert!(!role_root.join("control/owner.sock").exists());
        }

        let forbidden = [
            root.to_string_lossy().into_owned(),
            address.to_string(),
            "DESKKIN_PROFILE_SECRET_MARKER".into(),
        ];
        let mut runs = Vec::new();
        for entry in fs::read_dir(role_root.join("diagnostics")).unwrap() {
            let bytes = fs::read(entry.unwrap().path()).unwrap();
            let text = String::from_utf8_lossy(&bytes);
            assert!(forbidden.iter().all(|value| !text.contains(value)));
            runs.push(serde_json::from_slice::<DiagnosticRun>(&bytes).unwrap());
        }
        let lifecycle_ids = runs
            .iter()
            .filter(|run| {
                run.records
                    .iter()
                    .any(|record| record.operation == Operation::ProfileResolve)
            })
            .map(|run| run.run_id.as_str())
            .collect::<Vec<_>>();
        assert!(!lifecycle_ids.is_empty());
        assert!(runs.iter().any(|run| {
            run.records
                .iter()
                .any(|record| record.operation == Operation::ProtocolNegotiate)
                && lifecycle_ids.contains(&run.scenario_run_id.as_str())
        }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn profile_stop_refuses_a_low_level_owner() {
        let base = temp("mismatch");
        let root = base.join(STATE_ROOT);
        let _ = fs::remove_dir_all(&base);
        let role = "roles/mismatch";
        let role_root = root.join(role);
        IdentityStore::new(role_root.join("identity"))
            .init()
            .unwrap();
        let diagnostics = role_root.join("diagnostics");
        let probe = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = probe.local_addr().unwrap();
        drop(probe);
        let profile = PhysicalProfile::new(
            role.into(),
            BindMode::Loopback,
            address,
            ProfileAvailability::Available,
            ProfileRecording::On,
        )
        .unwrap();
        let store = ProfileStore::at(root.clone());
        store.set("physical", &profile).unwrap();
        let runtime_role = role_root.clone();
        let runtime = thread::spawn(move || {
            crate::run_host_runtime_with_recording(
                address,
                &runtime_role,
                AvailabilityResult::Available,
                RecordingMode::Off,
            )
        });
        for _ in 0..300 {
            if store.status("physical").unwrap() == ProfileState::ProfileMismatch {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            store.status("physical").unwrap(),
            ProfileState::ProfileMismatch
        );
        assert!(matches!(
            store.stop("physical"),
            Err(ProfileError::ProfileMismatch)
        ));
        let control = role_root.join("control");
        let generation = crate::discover_owner(&control).unwrap().unwrap();
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
        runtime.join().unwrap().unwrap();
        let runs = fs::read_dir(diagnostics)
            .unwrap()
            .map(|entry| {
                serde_json::from_slice::<DiagnosticRun>(&fs::read(entry.unwrap().path()).unwrap())
                    .unwrap()
            })
            .collect::<Vec<_>>();
        for operation in [Operation::ProfileStatus, Operation::ProfileStop] {
            assert!(runs.iter().flat_map(|run| &run.records).any(|record| {
                record.operation == operation
                    && record.status == OperationStatus::Error
                    && record.error_type == Some(ErrorType::ProfileMismatch)
            }));
        }
        fs::remove_dir_all(base).unwrap();
    }

    fn respond(stream: &mut UnixStream, response: &OwnerResponse) {
        let mut prefix = [0; 4];
        stream.read_exact(&mut prefix).unwrap();
        let mut request = vec![0; u32::from_be_bytes(prefix) as usize];
        stream.read_exact(&mut request).unwrap();
        serde_json::from_slice::<OwnerCommand>(&request).unwrap();
        let response = serde_json::to_vec(response).unwrap();
        stream
            .write_all(&(u32::try_from(response.len()).unwrap()).to_be_bytes())
            .unwrap();
        stream.write_all(&response).unwrap();
    }

    #[test]
    fn stop_distinguishes_stale_generation_and_shutdown_timeout() {
        for (name, response, expected) in [
            (
                "stale",
                OwnerResponse::StaleOwner,
                ProfileError::StaleGeneration,
            ),
            (
                "timeout",
                OwnerResponse::ShutdownAccepted,
                ProfileError::ShutdownTimeout,
            ),
        ] {
            let base = temp(name);
            let root = base.join(STATE_ROOT);
            let role = format!("roles/{name}");
            let role_root = root.join(&role);
            let _ = fs::remove_dir_all(&base);
            let physical = PhysicalProfile::new(
                role,
                BindMode::Loopback,
                "127.0.0.1:39032".parse().unwrap(),
                ProfileAvailability::Available,
                ProfileRecording::On,
            )
            .unwrap();
            let store = ProfileStore::at(root);
            store.set("physical", &physical).unwrap();
            fs::create_dir_all(&role_root).unwrap();
            fs::set_permissions(&role_root, fs::Permissions::from_mode(0o700)).unwrap();
            let control = role_root.join("control");
            let owner = crate::acquire_owner_lock(&control).unwrap();
            let socket = control.join("owner.sock");
            let listener = UnixListener::bind(&socket).unwrap();
            fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
            let metadata = physical.launch_metadata("physical");
            let server = thread::spawn(move || {
                let (mut info, _) = listener.accept().unwrap();
                respond(
                    &mut info,
                    &OwnerResponse::OwnerInfo {
                        owner_generation: "102132435465768798a9bacbdcedfe0f".into(),
                        launch: Some(metadata),
                    },
                );
                let (mut shutdown, _) = listener.accept().unwrap();
                respond(&mut shutdown, &response);
            });
            let result = store.stop("physical").unwrap_err();
            assert_eq!(result.to_string(), expected.to_string());
            server.join().unwrap();
            drop(owner);
            let runs = fs::read_dir(role_root.join("diagnostics"))
                .unwrap()
                .map(|entry| {
                    serde_json::from_slice::<DiagnosticRun>(
                        &fs::read(entry.unwrap().path()).unwrap(),
                    )
                    .unwrap()
                })
                .collect::<Vec<_>>();
            let stop = runs
                .iter()
                .find(|run| run.records[0].operation == Operation::ProfileStop)
                .unwrap();
            assert_eq!(stop.outcome, RunOutcome::Error);
            assert_eq!(
                stop.records[0].error_type,
                Some(profile_error_type(&expected))
            );
            fs::remove_dir_all(base).unwrap();
        }
    }

    #[test]
    fn owner_without_control_socket_is_unknown_and_records_closed_errors() {
        let base = temp("owner-not-ready");
        let root = base.join(STATE_ROOT);
        let role = "roles/physical";
        let role_root = root.join(role);
        let _ = fs::remove_dir_all(&base);
        let physical = PhysicalProfile::new(
            role.into(),
            BindMode::Loopback,
            "127.0.0.1:39032".parse().unwrap(),
            ProfileAvailability::Available,
            ProfileRecording::On,
        )
        .unwrap();
        let store = ProfileStore::at(root);
        store.set("physical", &physical).unwrap();
        IdentityStore::new(role_root.join("identity"))
            .init()
            .unwrap();
        let _owner = crate::acquire_owner_lock(&role_root.join("control")).unwrap();

        assert_eq!(
            store.status("physical").unwrap(),
            ProfileState::OwnerUnknown
        );
        assert!(matches!(
            store.stop("physical"),
            Err(ProfileError::OwnerUnknown)
        ));
        let runs = fs::read_dir(role_root.join("diagnostics"))
            .unwrap()
            .map(|entry| {
                serde_json::from_slice::<DiagnosticRun>(&fs::read(entry.unwrap().path()).unwrap())
                    .unwrap()
            })
            .collect::<Vec<_>>();
        for operation in [Operation::ProfileStatus, Operation::ProfileStop] {
            let run = runs
                .iter()
                .find(|run| run.records[0].operation == operation)
                .unwrap();
            assert_eq!(run.outcome, RunOutcome::Error);
            assert_eq!(run.records[0].status, OperationStatus::Error);
            assert_eq!(run.records[0].error_type, Some(ErrorType::OwnerLost));
        }
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn low_level_owner_waits_for_the_profile_store_startup_barrier() {
        let base = temp("startup-barrier");
        let root = base.join(STATE_ROOT);
        let role_root = root.join("roles/host");
        let _ = fs::remove_dir_all(&base);
        IdentityStore::new(role_root.join("identity"))
            .init()
            .unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let probe = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = probe.local_addr().unwrap();
        drop(probe);
        let store = ProfileStore::at(root);
        let barrier = store.lock().unwrap();
        let runtime_role = role_root.clone();
        let runtime = thread::spawn(move || {
            crate::run_host_runtime_with_recording(
                address,
                &runtime_role,
                AvailabilityResult::Available,
                RecordingMode::Off,
            )
        });
        thread::sleep(Duration::from_millis(50));
        assert!(!role_root.join("control/owner.sock").exists());
        drop(barrier);
        let control = role_root.join("control");
        for _ in 0..300 {
            if control.join("owner.sock").exists() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let generation = crate::discover_owner(&control).unwrap().unwrap();
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
        runtime.join().unwrap().unwrap();
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn concurrent_same_and_cross_name_launches_leave_exactly_one_owner() {
        for names in [["physical", "physical"], ["physical", "alternate"]] {
            let base = temp(&format!("concurrent-{}", names[1]));
            let root = base.join(STATE_ROOT);
            let role = "roles/host";
            let role_root = root.join(role);
            let _ = fs::remove_dir_all(&base);
            IdentityStore::new(role_root.join("identity"))
                .init()
                .unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            let probe = bind_loopback("127.0.0.1:0".parse().unwrap()).unwrap();
            let address = probe.local_addr().unwrap();
            drop(probe);
            let physical = PhysicalProfile::new(
                role.into(),
                BindMode::Loopback,
                address,
                ProfileAvailability::Available,
                ProfileRecording::Off,
            )
            .unwrap();
            let store = ProfileStore::at(root.clone());
            for name in names {
                store.set(name, &physical).unwrap();
            }
            let starts = Arc::new(std::sync::Barrier::new(3));
            let mut launches = names
                .map(|name| {
                    let root = root.clone();
                    let starts = starts.clone();
                    thread::spawn(move || {
                        starts.wait();
                        ProfileStore::at(root).run(name)
                    })
                })
                .into_iter()
                .collect::<Vec<_>>();
            starts.wait();
            for _ in 0..400 {
                if launches.iter().any(thread::JoinHandle::is_finished) {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            let failed_index = launches
                .iter()
                .position(thread::JoinHandle::is_finished)
                .unwrap();
            let failed = launches.swap_remove(failed_index).join().unwrap();
            assert!(matches!(
                failed,
                Err(ProfileError::Runtime(SessionError::Io))
            ));
            let running_name = names
                .into_iter()
                .find(|name| store.status(name).unwrap() == ProfileState::Running)
                .unwrap();
            store.stop(running_name).unwrap();
            launches.pop().unwrap().join().unwrap().unwrap();
            assert!(!role_root.join("control/owner.sock").exists());
            fs::remove_dir_all(base).unwrap();
        }
    }
}
