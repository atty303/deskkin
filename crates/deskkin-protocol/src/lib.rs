#![no_std]
#![forbid(unsafe_code)]

pub const PRELUDE: [u8; 6] = *b"DSKN\0\x01";
pub const HANDSHAKE_FRAME_MAX: usize = 1_024;
pub const APPLICATION_FRAME_MAX: usize = 16 * 1_024;
pub const AVAILABILITY_READ_V1: Bits = Bits([1, 0, 0, 0, 0, 0, 0, 0]);
pub const AVAILABILITY_READ_PERMISSION: Bits = Bits([1, 0, 0, 0, 0, 0, 0, 0]);
pub const PROTOCOL_MAJOR_1: Bits = Bits([2, 0, 0, 0, 0, 0, 0, 0]);

pub type ContextId = [u8; 16];
pub type TransactionId = [u8; 16];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Bits(pub [u8; 8]);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairingDecision {
    Confirmed,
    Rejected,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairingCloseReason {
    Rejected,
    Expired,
    Incomplete,
    StoreFailed,
    PairingBusy,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HelloRejectReason {
    NoCommonVersion,
    RequiredFeatureUnsupported,
    SessionBusy,
    PermissionDenied,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AvailabilityResult {
    Available,
    Unavailable,
    ReadFailed,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloseReason {
    Normal,
    Protocol,
    Timeout,
    Cancelled,
    Unpaired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Message {
    PairingBegin {
        transaction: TransactionId,
    },
    PairingDecision {
        transaction: TransactionId,
        decision: PairingDecision,
    },
    PairingPrepared {
        transaction: TransactionId,
    },
    PairingCommit {
        transaction: TransactionId,
    },
    PairingCommitted {
        transaction: TransactionId,
    },
    PairingClose {
        transaction: TransactionId,
        reason: PairingCloseReason,
    },
    PairingComplete {
        transaction: TransactionId,
    },
    Hello {
        session: ContextId,
        protocol_majors: Bits,
        required_features: Bits,
        optional_features: Bits,
        requested_permissions: Bits,
    },
    HelloAck {
        session: ContextId,
        selected_major: u8,
        selected_features: Bits,
        granted_permissions: Bits,
    },
    HelloReject {
        session: ContextId,
        reason: HelloRejectReason,
    },
    ReadAvailability {
        request_id: u32,
        operation: ContextId,
    },
    AvailabilityResult {
        request_id: u32,
        operation: ContextId,
        result: AvailabilityResult,
    },
    Ping,
    Pong,
    Close {
        reason: CloseReason,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodecError {
    BufferTooSmall,
    Empty,
    UnknownTag(u8),
    InvalidValue,
    TrailingBytes,
    Oversize,
}

impl Message {
    /// Encodes one canonical plaintext message into a caller-owned buffer.
    ///
    /// # Errors
    ///
    /// Returns a bounded codec error when the buffer is too small or the
    /// encoded message would exceed the application-frame limit.
    pub fn encode<'a>(&self, output: &'a mut [u8]) -> Result<&'a [u8], CodecError> {
        let size = self.encoded_len();
        if size > APPLICATION_FRAME_MAX {
            return Err(CodecError::Oversize);
        }
        let out = output.get_mut(..size).ok_or(CodecError::BufferTooSmall)?;
        out[0] = self.tag();
        let mut p = 1;
        macro_rules! bytes {
            ($value:expr) => {{
                let v = $value;
                out[p..p + v.len()].copy_from_slice(v);
                p += v.len();
            }};
        }
        macro_rules! byte {
            ($value:expr) => {{
                out[p] = $value;
                p += 1;
            }};
        }
        match self {
            Self::PairingBegin { transaction }
            | Self::PairingPrepared { transaction }
            | Self::PairingCommit { transaction }
            | Self::PairingCommitted { transaction }
            | Self::PairingComplete { transaction } => bytes!(transaction),
            Self::PairingDecision {
                transaction,
                decision,
            } => {
                bytes!(transaction);
                byte!(decision_u8(*decision));
            }
            Self::PairingClose {
                transaction,
                reason,
            } => {
                bytes!(transaction);
                byte!(*reason as u8);
            }
            Self::Hello {
                session,
                protocol_majors,
                required_features,
                optional_features,
                requested_permissions,
            } => {
                bytes!(session);
                bytes!(&protocol_majors.0);
                bytes!(&required_features.0);
                bytes!(&optional_features.0);
                bytes!(&requested_permissions.0);
            }
            Self::HelloAck {
                session,
                selected_major,
                selected_features,
                granted_permissions,
            } => {
                bytes!(session);
                byte!(*selected_major);
                bytes!(&selected_features.0);
                bytes!(&granted_permissions.0);
            }
            Self::HelloReject { session, reason } => {
                bytes!(session);
                byte!(*reason as u8);
            }
            Self::ReadAvailability {
                request_id,
                operation,
            } => {
                bytes!(&request_id.to_be_bytes());
                bytes!(operation);
            }
            Self::AvailabilityResult {
                request_id,
                operation,
                result,
            } => {
                bytes!(&request_id.to_be_bytes());
                bytes!(operation);
                byte!(*result as u8);
            }
            Self::Ping | Self::Pong => {}
            Self::Close { reason } => byte!(*reason as u8),
        }
        debug_assert_eq!(p, size);
        Ok(out)
    }

    /// Decodes exactly one canonical plaintext message.
    ///
    /// # Errors
    ///
    /// Returns a bounded codec error for empty, malformed, unknown, oversized,
    /// or non-exact input.
    #[allow(clippy::too_many_lines)]
    pub fn decode(input: &[u8]) -> Result<Self, CodecError> {
        let (&tag, body) = input.split_first().ok_or(CodecError::Empty)?;
        if input.len() > APPLICATION_FRAME_MAX {
            return Err(CodecError::Oversize);
        }
        macro_rules! exact {
            ($n:expr) => {
                if body.len() != $n {
                    return Err(CodecError::TrailingBytes);
                }
            };
        }
        macro_rules! id {
            ($at:expr) => {{
                let mut v = [0; 16];
                v.copy_from_slice(&body[$at..$at + 16]);
                v
            }};
        }
        macro_rules! bits {
            ($at:expr) => {{
                let mut v = [0; 8];
                v.copy_from_slice(&body[$at..$at + 8]);
                Bits(v)
            }};
        }
        Ok(match tag {
            0x01 => {
                exact!(16);
                Self::PairingBegin {
                    transaction: id!(0),
                }
            }
            0x02 => {
                exact!(17);
                Self::PairingDecision {
                    transaction: id!(0),
                    decision: match body[16] {
                        0 => PairingDecision::Confirmed,
                        1 => PairingDecision::Rejected,
                        _ => return Err(CodecError::InvalidValue),
                    },
                }
            }
            0x03 => {
                exact!(16);
                Self::PairingPrepared {
                    transaction: id!(0),
                }
            }
            0x04 => {
                exact!(16);
                Self::PairingCommit {
                    transaction: id!(0),
                }
            }
            0x05 => {
                exact!(16);
                Self::PairingCommitted {
                    transaction: id!(0),
                }
            }
            0x06 => {
                exact!(17);
                Self::PairingClose {
                    transaction: id!(0),
                    reason: pairing_reason(body[16])?,
                }
            }
            0x07 => {
                exact!(16);
                Self::PairingComplete {
                    transaction: id!(0),
                }
            }
            0x10 => {
                exact!(48);
                Self::Hello {
                    session: id!(0),
                    protocol_majors: bits!(16),
                    required_features: bits!(24),
                    optional_features: bits!(32),
                    requested_permissions: bits!(40),
                }
            }
            0x11 => {
                exact!(33);
                Self::HelloAck {
                    session: id!(0),
                    selected_major: body[16],
                    selected_features: bits!(17),
                    granted_permissions: bits!(25),
                }
            }
            0x12 => {
                exact!(17);
                Self::HelloReject {
                    session: id!(0),
                    reason: hello_reason(body[16])?,
                }
            }
            0x20 => {
                exact!(20);
                Self::ReadAvailability {
                    request_id: u32::from_be_bytes(
                        body[..4].try_into().map_err(|_| CodecError::InvalidValue)?,
                    ),
                    operation: id!(4),
                }
            }
            0x21 => {
                exact!(21);
                Self::AvailabilityResult {
                    request_id: u32::from_be_bytes(
                        body[..4].try_into().map_err(|_| CodecError::InvalidValue)?,
                    ),
                    operation: id!(4),
                    result: availability(body[20])?,
                }
            }
            0x30 => {
                exact!(0);
                Self::Ping
            }
            0x31 => {
                exact!(0);
                Self::Pong
            }
            0x32 => {
                exact!(1);
                Self::Close {
                    reason: close_reason(body[0])?,
                }
            }
            value => return Err(CodecError::UnknownTag(value)),
        })
    }

    const fn tag(&self) -> u8 {
        match self {
            Self::PairingBegin { .. } => 1,
            Self::PairingDecision { .. } => 2,
            Self::PairingPrepared { .. } => 3,
            Self::PairingCommit { .. } => 4,
            Self::PairingCommitted { .. } => 5,
            Self::PairingClose { .. } => 6,
            Self::PairingComplete { .. } => 7,
            Self::Hello { .. } => 0x10,
            Self::HelloAck { .. } => 0x11,
            Self::HelloReject { .. } => 0x12,
            Self::ReadAvailability { .. } => 0x20,
            Self::AvailabilityResult { .. } => 0x21,
            Self::Ping => 0x30,
            Self::Pong => 0x31,
            Self::Close { .. } => 0x32,
        }
    }
    const fn encoded_len(&self) -> usize {
        match self {
            Self::PairingBegin { .. }
            | Self::PairingPrepared { .. }
            | Self::PairingCommit { .. }
            | Self::PairingCommitted { .. }
            | Self::PairingComplete { .. } => 17,
            Self::PairingDecision { .. } | Self::PairingClose { .. } | Self::HelloReject { .. } => {
                18
            }
            Self::Hello { .. } => 49,
            Self::HelloAck { .. } => 34,
            Self::ReadAvailability { .. } => 21,
            Self::AvailabilityResult { .. } => 22,
            Self::Ping | Self::Pong => 1,
            Self::Close { .. } => 2,
        }
    }
}

/// Encodes a bounded application-frame length.
///
/// # Errors
///
/// Returns [`CodecError::Oversize`] when the value exceeds the accepted frame
/// bound or the two-byte representation.
pub fn encode_frame_length(length: usize) -> Result<[u8; 2], CodecError> {
    let value = u16::try_from(length).map_err(|_| CodecError::Oversize)?;
    if length > APPLICATION_FRAME_MAX {
        return Err(CodecError::Oversize);
    }
    Ok(value.to_be_bytes())
}
#[must_use]
pub const fn decode_frame_length(bytes: [u8; 2]) -> usize {
    u16::from_be_bytes(bytes) as usize
}
/// Derives the zero-padded six-digit local authentication string.
///
/// # Errors
///
/// Returns [`CodecError::InvalidValue`] when fewer than four hash bytes are
/// supplied.
pub fn authentication_string(handshake_hash: &[u8]) -> Result<[u8; 6], CodecError> {
    let first: [u8; 4] = handshake_hash
        .get(..4)
        .ok_or(CodecError::InvalidValue)?
        .try_into()
        .map_err(|_| CodecError::InvalidValue)?;
    let mut n = u32::from_be_bytes(first) % 1_000_000;
    let mut out = *b"000000";
    let mut i = 6;
    while i > 0 {
        i -= 1;
        out[i] = b'0' + u8::try_from(n % 10).map_err(|_| CodecError::InvalidValue)?;
        n /= 10;
    }
    Ok(out)
}
const fn decision_u8(v: PairingDecision) -> u8 {
    match v {
        PairingDecision::Confirmed => 0,
        PairingDecision::Rejected => 1,
    }
}
fn pairing_reason(v: u8) -> Result<PairingCloseReason, CodecError> {
    Ok(match v {
        0 => PairingCloseReason::Rejected,
        1 => PairingCloseReason::Expired,
        2 => PairingCloseReason::Incomplete,
        3 => PairingCloseReason::StoreFailed,
        4 => PairingCloseReason::PairingBusy,
        _ => return Err(CodecError::InvalidValue),
    })
}
fn hello_reason(v: u8) -> Result<HelloRejectReason, CodecError> {
    Ok(match v {
        0 => HelloRejectReason::NoCommonVersion,
        1 => HelloRejectReason::RequiredFeatureUnsupported,
        2 => HelloRejectReason::SessionBusy,
        3 => HelloRejectReason::PermissionDenied,
        _ => return Err(CodecError::InvalidValue),
    })
}
fn availability(v: u8) -> Result<AvailabilityResult, CodecError> {
    Ok(match v {
        0 => AvailabilityResult::Available,
        1 => AvailabilityResult::Unavailable,
        2 => AvailabilityResult::ReadFailed,
        _ => return Err(CodecError::InvalidValue),
    })
}
fn close_reason(v: u8) -> Result<CloseReason, CodecError> {
    Ok(match v {
        0 => CloseReason::Normal,
        1 => CloseReason::Protocol,
        2 => CloseReason::Timeout,
        3 => CloseReason::Cancelled,
        4 => CloseReason::Unpaired,
        _ => return Err(CodecError::InvalidValue),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_vectors() {
        let mut b = [0; 64];
        assert_eq!(Message::Ping.encode(&mut b).unwrap(), [0x30]);
        assert_eq!(Message::Pong.encode(&mut b).unwrap(), [0x31]);
        assert_eq!(
            Message::ReadAvailability {
                request_id: 1,
                operation: [0; 16]
            }
            .encode(&mut b)
            .unwrap(),
            [
                0x20, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
            ]
        );
    }
    #[test]
    fn every_message_round_trips_exactly() {
        let messages = [
            Message::PairingBegin {
                transaction: [0; 16],
            },
            Message::PairingDecision {
                transaction: [1; 16],
                decision: PairingDecision::Confirmed,
            },
            Message::PairingPrepared {
                transaction: [2; 16],
            },
            Message::PairingCommit {
                transaction: [3; 16],
            },
            Message::PairingCommitted {
                transaction: [4; 16],
            },
            Message::PairingClose {
                transaction: [5; 16],
                reason: PairingCloseReason::PairingBusy,
            },
            Message::PairingComplete {
                transaction: [6; 16],
            },
            Message::Hello {
                session: [0; 16],
                protocol_majors: PROTOCOL_MAJOR_1,
                required_features: AVAILABILITY_READ_V1,
                optional_features: Bits([0; 8]),
                requested_permissions: AVAILABILITY_READ_PERMISSION,
            },
            Message::HelloAck {
                session: [1; 16],
                selected_major: 1,
                selected_features: AVAILABILITY_READ_V1,
                granted_permissions: AVAILABILITY_READ_PERMISSION,
            },
            Message::HelloReject {
                session: [2; 16],
                reason: HelloRejectReason::PermissionDenied,
            },
            Message::ReadAvailability {
                request_id: u32::MAX,
                operation: [7; 16],
            },
            Message::AvailabilityResult {
                request_id: 7,
                operation: [8; 16],
                result: AvailabilityResult::ReadFailed,
            },
            Message::Ping,
            Message::Pong,
            Message::Close {
                reason: CloseReason::Unpaired,
            },
        ];
        for message in messages {
            let mut b = [0; 64];
            let encoded = message.encode(&mut b).unwrap();
            assert_eq!(Message::decode(encoded), Ok(message));
        }
    }
    #[test]
    fn rejects_unknown_trailing_and_values() {
        assert_eq!(Message::decode(&[0xff]), Err(CodecError::UnknownTag(0xff)));
        assert_eq!(Message::decode(&[0x30, 0]), Err(CodecError::TrailingBytes));
        assert_eq!(Message::decode(&[0x32, 9]), Err(CodecError::InvalidValue));
    }
    #[test]
    fn sas_is_zero_padded() {
        assert_eq!(authentication_string(&[0, 0, 0, 42]).unwrap(), *b"000042");
    }

    #[test]
    fn frame_and_message_size_boundaries_are_exact() {
        assert_eq!(encode_frame_length(APPLICATION_FRAME_MAX), Ok([0x40, 0x00]));
        assert_eq!(
            encode_frame_length(APPLICATION_FRAME_MAX + 1),
            Err(CodecError::Oversize)
        );
        assert_eq!(decode_frame_length([0x12, 0x34]), 0x1234);
        let hello = Message::Hello {
            session: [0; 16],
            protocol_majors: PROTOCOL_MAJOR_1,
            required_features: AVAILABILITY_READ_V1,
            optional_features: Bits([0; 8]),
            requested_permissions: AVAILABILITY_READ_PERMISSION,
        };
        let mut exact = [0; 49];
        assert_eq!(hello.encode(&mut exact).unwrap().len(), exact.len());
        let mut short = [0; 48];
        assert_eq!(hello.encode(&mut short), Err(CodecError::BufferTooSmall));
        assert_eq!(Message::decode(&[]), Err(CodecError::Empty));
        assert_eq!(Message::decode(&[0x20; 20]), Err(CodecError::TrailingBytes));
    }
}
