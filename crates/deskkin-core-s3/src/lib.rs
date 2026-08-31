#![no_std]
#![allow(clippy::missing_errors_doc)]

pub const SCHEMA_VERSION: u8 = 1;
pub const NVS_HEADER_LEN: usize = 24;
pub const NVS_PAYLOAD_MAX: usize = 160;
pub const NVS_RECORD_MAX: usize = NVS_HEADER_LEN + NVS_PAYLOAD_MAX;
pub const CONTROL_PAYLOAD_MAX: usize = 160;
pub const APPLICATION_QUEUE_CAPACITY: usize = 4;
pub const RESERVED_CONTROL_CAPACITY: usize = 1;
pub const COMPLETION_QUEUE_CAPACITY: usize = 8;
pub const HOST_PORT: u16 = 39_042;
pub const PET_BENCHMARK_DURATION_MS: u32 = 60_000;
pub const PET_BENCHMARK_FRAME_PERIOD_US: u64 = 50_000;
pub const PET_BENCHMARK_REQUESTS: u32 = 1_200;
pub const PET_BENCHMARK_FRAME_BUDGET_US: u32 = 50_000;
pub const PET_BENCHMARK_STALL_US: u32 = 250_000;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PetBenchmarkSummary {
    pub duration_ms: u32,
    pub update_requests: u32,
    pub completed_frames: u32,
    pub render_total_us: u32,
    pub transfer_total_us: u32,
    pub render_max_us: u32,
    pub transfer_max_us: u32,
    pub frames_within_budget: u32,
    pub stalls: u16,
    pub deadline_misses: u16,
    pub max_consecutive_misses: u16,
    pub transferred_lines: u32,
    pub transferred_bytes: u32,
    pub digest_updates: u32,
    pub allocation_failures: u8,
    pub transfer_failures: u16,
    consecutive_misses: u16,
}

impl PetBenchmarkSummary {
    pub fn request_updates(&mut self, count: u32) {
        self.update_requests = self.update_requests.saturating_add(count);
        self.record_misses(count.saturating_sub(1));
    }

    pub fn complete_frame(
        &mut self,
        render_us: u32,
        transfer_us: u32,
        transferred_lines: u32,
        transferred_bytes: u32,
        digest_changed: bool,
        deadline_missed: bool,
    ) {
        self.completed_frames = self.completed_frames.saturating_add(1);
        self.render_total_us = self.render_total_us.saturating_add(render_us);
        self.transfer_total_us = self.transfer_total_us.saturating_add(transfer_us);
        self.render_max_us = self.render_max_us.max(render_us);
        self.transfer_max_us = self.transfer_max_us.max(transfer_us);
        if render_us.saturating_add(transfer_us) <= PET_BENCHMARK_FRAME_BUDGET_US {
            self.frames_within_budget = self.frames_within_budget.saturating_add(1);
        }
        if render_us.saturating_add(transfer_us) > PET_BENCHMARK_STALL_US {
            self.stalls = self.stalls.saturating_add(1);
        }
        self.transferred_lines = self.transferred_lines.saturating_add(transferred_lines);
        self.transferred_bytes = self.transferred_bytes.saturating_add(transferred_bytes);
        self.digest_updates = self
            .digest_updates
            .saturating_add(u32::from(digest_changed));
        if deadline_missed {
            self.record_misses(1);
        } else {
            self.consecutive_misses = 0;
        }
    }

    pub fn record_transfer_failure(&mut self) {
        self.transfer_failures = self.transfer_failures.saturating_add(1);
    }

    fn record_misses(&mut self, count: u32) {
        let count = u16::try_from(count).unwrap_or(u16::MAX);
        self.deadline_misses = self.deadline_misses.saturating_add(count);
        self.consecutive_misses = self.consecutive_misses.saturating_add(count);
        self.max_consecutive_misses = self.max_consecutive_misses.max(self.consecutive_misses);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PeerState {
    Unpaired = 0,
    Pending = 1,
    Committing = 2,
    Paired = 3,
    Revoking = 4,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordState {
    Identity(PeerState),
    ConfigPresent,
    ConfigCleared,
}

impl RecordState {
    const fn wire(self) -> u8 {
        match self {
            Self::Identity(state) => state as u8,
            Self::ConfigPresent => 0x10,
            Self::ConfigCleared => 0x11,
        }
    }
}

impl TryFrom<u8> for RecordState {
    type Error = DecodeError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0..=4 => Ok(Self::Identity(PeerState::try_from(value)?)),
            0x10 => Ok(Self::ConfigPresent),
            0x11 => Ok(Self::ConfigCleared),
            _ => Err(DecodeError::UnknownState),
        }
    }
}

impl TryFrom<u8> for PeerState {
    type Error = DecodeError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Unpaired),
            1 => Ok(Self::Pending),
            2 => Ok(Self::Committing),
            3 => Ok(Self::Paired),
            4 => Ok(Self::Revoking),
            _ => Err(DecodeError::UnknownState),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiState {
    SetupRequired,
    ReadyToPair,
    Connecting,
    PairingConfirmation,
    Paired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeError {
    Truncated,
    Oversize,
    UnknownSchema,
    UnknownState,
    InvalidLength,
    InvalidCrc,
    ConflictingSlots,
    InvalidConfig,
    UnknownCommand,
    TrailingBytes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecordRef<'a> {
    pub publication_sequence: u64,
    pub generation: u64,
    pub state: RecordState,
    pub payload: &'a [u8],
}

pub fn encode_record(
    sequence: u64,
    generation: u64,
    state: RecordState,
    payload: &[u8],
    output: &mut [u8],
) -> Result<usize, DecodeError> {
    if payload.len() > NVS_PAYLOAD_MAX || output.len() < NVS_HEADER_LEN + payload.len() {
        return Err(DecodeError::Oversize);
    }
    output[0] = SCHEMA_VERSION;
    output[1] = state.wire();
    let length = u16::try_from(payload.len()).map_err(|_| DecodeError::Oversize)?;
    output[2..4].copy_from_slice(&length.to_be_bytes());
    output[4..12].copy_from_slice(&sequence.to_be_bytes());
    output[12..20].copy_from_slice(&generation.to_be_bytes());
    output[20..24].fill(0);
    output[24..24 + payload.len()].copy_from_slice(payload);
    let crc = crc32(&output[..24 + payload.len()]);
    output[20..24].copy_from_slice(&crc.to_be_bytes());
    Ok(24 + payload.len())
}

pub fn decode_record(input: &[u8]) -> Result<RecordRef<'_>, DecodeError> {
    if input.len() < NVS_HEADER_LEN {
        return Err(DecodeError::Truncated);
    }
    if input[0] != SCHEMA_VERSION {
        return Err(DecodeError::UnknownSchema);
    }
    let state = RecordState::try_from(input[1])?;
    let length = usize::from(u16::from_be_bytes([input[2], input[3]]));
    if length > NVS_PAYLOAD_MAX || input.len() != NVS_HEADER_LEN + length {
        return Err(DecodeError::InvalidLength);
    }
    let expected = u32::from_be_bytes([input[20], input[21], input[22], input[23]]);
    let mut header = [0_u8; NVS_HEADER_LEN];
    header.copy_from_slice(&input[..NVS_HEADER_LEN]);
    header[20..24].fill(0);
    let mut crc = crc32_update(u32::MAX, &header);
    crc = crc32_update(crc, &input[NVS_HEADER_LEN..]);
    if !crc != expected {
        return Err(DecodeError::InvalidCrc);
    }
    Ok(RecordRef {
        publication_sequence: u64::from_be_bytes([
            input[4], input[5], input[6], input[7], input[8], input[9], input[10], input[11],
        ]),
        generation: u64::from_be_bytes([
            input[12], input[13], input[14], input[15], input[16], input[17], input[18], input[19],
        ]),
        state,
        payload: &input[NVS_HEADER_LEN..],
    })
}

pub fn select_slot<'a>(
    first: Option<&'a [u8]>,
    second: Option<&'a [u8]>,
) -> Result<Option<RecordRef<'a>>, DecodeError> {
    let first = first.map(decode_record).transpose()?;
    let second = second.map(decode_record).transpose()?;
    match (first, second) {
        (None, None) => Ok(None),
        (Some(value), None) | (None, Some(value)) => Ok(Some(value)),
        (Some(left), Some(right)) => {
            match left.publication_sequence.cmp(&right.publication_sequence) {
                core::cmp::Ordering::Greater => Ok(Some(left)),
                core::cmp::Ordering::Less => Ok(Some(right)),
                core::cmp::Ordering::Equal
                    if left.generation == right.generation
                        && left.state == right.state
                        && left.payload == right.payload =>
                {
                    Ok(Some(left))
                }
                core::cmp::Ordering::Equal => Err(DecodeError::ConflictingSlots),
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiConfig<'a> {
    pub ssid: &'a [u8],
    pub passphrase: &'a [u8],
    pub host_ipv4: [u8; 4],
}

impl WifiConfig<'_> {
    pub fn validate(&self) -> Result<(), DecodeError> {
        if !(1..=32).contains(&self.ssid.len())
            || !(8..=63).contains(&self.passphrase.len())
            || !self
                .passphrase
                .iter()
                .all(|byte| (0x20..=0x7e).contains(byte))
            || !is_rfc1918(self.host_ipv4)
        {
            return Err(DecodeError::InvalidConfig);
        }
        Ok(())
    }
}

#[must_use]
pub const fn is_rfc1918(value: [u8; 4]) -> bool {
    value[0] == 10
        || (value[0] == 172 && value[1] >= 16 && value[1] <= 31)
        || (value[0] == 192 && value[1] == 168)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ControlCommand {
    IdentityInit = 1,
    IdentityList = 2,
    IdentityUnpair = 3,
    WifiProvision = 4,
    WifiStatus = 5,
    WifiClear = 6,
    Run = 7,
    Status = 8,
    Shutdown = 9,
    PetBenchmarkStart = 10,
    PetBenchmarkStatus = 11,
}

impl TryFrom<u8> for ControlCommand {
    type Error = DecodeError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::IdentityInit),
            2 => Ok(Self::IdentityList),
            3 => Ok(Self::IdentityUnpair),
            4 => Ok(Self::WifiProvision),
            5 => Ok(Self::WifiStatus),
            6 => Ok(Self::WifiClear),
            7 => Ok(Self::Run),
            8 => Ok(Self::Status),
            9 => Ok(Self::Shutdown),
            10 => Ok(Self::PetBenchmarkStart),
            11 => Ok(Self::PetBenchmarkStatus),
            _ => Err(DecodeError::UnknownCommand),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlFrame<'a> {
    pub command: ControlCommand,
    pub command_id: [u8; 16],
    pub owner_generation: u64,
    pub payload: &'a [u8],
}

pub fn decode_control(input: &[u8]) -> Result<ControlFrame<'_>, DecodeError> {
    const HEADER: usize = 28;
    if input.len() < HEADER {
        return Err(DecodeError::Truncated);
    }
    if input[0] != SCHEMA_VERSION {
        return Err(DecodeError::UnknownSchema);
    }
    let command = ControlCommand::try_from(input[1])?;
    let payload_len = usize::from(u16::from_be_bytes([input[26], input[27]]));
    if payload_len > CONTROL_PAYLOAD_MAX {
        return Err(DecodeError::Oversize);
    }
    if input.len() != HEADER + payload_len {
        return Err(DecodeError::TrailingBytes);
    }
    let mut command_id = [0_u8; 16];
    command_id.copy_from_slice(&input[2..18]);
    Ok(ControlFrame {
        command,
        command_id,
        owner_generation: u64::from_be_bytes([
            input[18], input[19], input[20], input[21], input[22], input[23], input[24], input[25],
        ]),
        payload: &input[HEADER..],
    })
}

#[must_use]
pub fn crc32(bytes: &[u8]) -> u32 {
    !crc32_update(u32::MAX, bytes)
}

fn crc32_update(mut crc: u32, bytes: &[u8]) -> u32 {
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    fn record(sequence: u64, state: RecordState, payload: &[u8]) -> std::vec::Vec<u8> {
        let mut output = [0; NVS_RECORD_MAX];
        let length = encode_record(sequence, 7, state, payload, &mut output).unwrap();
        output[..length].to_vec()
    }

    #[test]
    fn record_round_trip_and_fault_boundaries() {
        let encoded = record(3, RecordState::Identity(PeerState::Committing), b"peer");
        let decoded = decode_record(&encoded).unwrap();
        assert_eq!(decoded.publication_sequence, 3);
        assert_eq!(decoded.generation, 7);
        assert_eq!(decoded.state, RecordState::Identity(PeerState::Committing));
        assert_eq!(decoded.payload, b"peer");
        for index in 0..encoded.len() {
            let mut corrupt = encoded.clone();
            corrupt[index] ^= 1;
            assert!(decode_record(&corrupt).is_err(), "byte {index}");
        }
    }

    #[test]
    fn slot_selection_rejects_equal_sequence_conflict() {
        let old = record(1, RecordState::Identity(PeerState::Pending), b"a");
        let current = record(2, RecordState::Identity(PeerState::Paired), b"b");
        assert_eq!(
            select_slot(Some(&old), Some(&current))
                .unwrap()
                .unwrap()
                .payload,
            b"b"
        );
        let conflict = record(2, RecordState::Identity(PeerState::Paired), b"c");
        assert_eq!(
            select_slot(Some(&current), Some(&conflict)),
            Err(DecodeError::ConflictingSlots)
        );
    }

    #[test]
    fn every_publication_boundary_is_closed_and_latest_complete_slot_wins() {
        let old = record(8, RecordState::Identity(PeerState::Paired), &[0x11; 112]);
        let new = record(9, RecordState::Identity(PeerState::Revoking), &[0x22; 112]);
        assert_eq!(
            select_slot(Some(&old), Some(&new)).unwrap().unwrap().state,
            RecordState::Identity(PeerState::Revoking)
        );
        for boundary in 0..new.len() {
            assert!(
                select_slot(Some(&old), Some(&new[..boundary])).is_err(),
                "partial publication boundary {boundary} must fail closed"
            );
        }
        assert_eq!(
            select_slot(Some(&old), None).unwrap().unwrap().state,
            RecordState::Identity(PeerState::Paired)
        );
    }

    #[test]
    fn revocation_recovery_keeps_generation_and_removes_peer_payload() {
        let revoking = record(10, RecordState::Identity(PeerState::Revoking), &[0x44; 112]);
        let decoded = decode_record(&revoking).unwrap();
        let recovered = {
            let mut output = [0; NVS_RECORD_MAX];
            let length = encode_record(
                decoded.publication_sequence + 1,
                decoded.generation,
                RecordState::Identity(PeerState::Unpaired),
                &decoded.payload[..64],
                &mut output,
            )
            .unwrap();
            output[..length].to_vec()
        };
        let selected = select_slot(Some(&revoking), Some(&recovered))
            .unwrap()
            .unwrap();
        assert_eq!(selected.generation, 7);
        assert_eq!(selected.state, RecordState::Identity(PeerState::Unpaired));
        assert_eq!(selected.payload.len(), 64);
    }

    #[test]
    fn config_validation_is_closed() {
        let valid = WifiConfig {
            ssid: b"lab",
            passphrase: b"password",
            host_ipv4: [192, 168, 1, 2],
        };
        assert_eq!(valid.validate(), Ok(()));
        assert!(
            WifiConfig {
                host_ipv4: [8, 8, 8, 8],
                ..valid
            }
            .validate()
            .is_err()
        );
        assert!(
            WifiConfig {
                passphrase: b"short",
                ..valid
            }
            .validate()
            .is_err()
        );
        assert!(
            WifiConfig {
                passphrase: b"password\n",
                ..valid
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn control_decode_rejects_unknown_and_trailing() {
        let mut frame = [0_u8; 28];
        frame[0] = 1;
        frame[1] = ControlCommand::Status as u8;
        assert_eq!(
            decode_control(&frame).unwrap().command,
            ControlCommand::Status
        );
        frame[1] = 99;
        assert_eq!(decode_control(&frame), Err(DecodeError::UnknownCommand));
    }

    #[test]
    fn pet_benchmark_summary_tracks_budget_misses_and_bounded_counters() {
        let mut summary = PetBenchmarkSummary::default();
        summary.request_updates(3);
        summary.complete_frame(20_000, 20_000, 240, 153_600, true, true);
        summary.complete_frame(30_000, 25_000, 120, 76_800, false, false);
        summary.complete_frame(200_000, 60_000, 1, 2, true, false);
        summary.record_transfer_failure();

        assert_eq!(summary.update_requests, 3);
        assert_eq!(summary.completed_frames, 3);
        assert_eq!(summary.frames_within_budget, 1);
        assert_eq!(summary.deadline_misses, 3);
        assert_eq!(summary.max_consecutive_misses, 3);
        assert_eq!(summary.stalls, 1);
        assert_eq!(summary.transferred_lines, 361);
        assert_eq!(summary.transferred_bytes, 230_402);
        assert_eq!(summary.digest_updates, 2);
        assert_eq!(summary.transfer_failures, 1);
        assert_eq!(summary.render_max_us, 200_000);
        assert_eq!(summary.transfer_max_us, 60_000);
    }
}
