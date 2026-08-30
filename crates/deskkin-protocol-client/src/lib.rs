#![no_std]
#![allow(clippy::missing_errors_doc)]

use deskkin_protocol::{AvailabilityResult, ContextId, HelloRejectReason};

pub const RECONNECT_DELAYS_MS: [u32; 5] = [250, 500, 1_000, 2_000, 5_000];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    Backoff,
    Connecting,
    Authenticated,
    Stopped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolAdapterError {
    NotAuthenticated,
    NoActiveRead,
    SessionMismatch,
    RequestMismatch,
    ActiveRead,
    Stopped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalReason {
    Incompatible,
    AuthorizationDenied,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AvailabilityValue {
    Available,
    Unavailable,
    ReadFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolEvent {
    AvailabilityCompleted {
        effect_id: u64,
        value: AvailabilityValue,
    },
    SessionInvalidated,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtocolAdapter {
    state: ConnectionState,
    session: Option<ContextId>,
    request_id: u32,
    active: Option<(ContextId, u32, u64)>,
    backoff_index: usize,
    terminal_reason: Option<TerminalReason>,
}

impl Default for ProtocolAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtocolAdapter {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: ConnectionState::Disconnected,
            session: None,
            request_id: 0,
            active: None,
            backoff_index: 0,
            terminal_reason: None,
        }
    }

    #[must_use]
    pub const fn state(&self) -> ConnectionState {
        self.state
    }

    #[must_use]
    pub const fn session_context(&self) -> Option<ContextId> {
        self.session
    }

    pub fn connecting(&mut self) {
        if self.state != ConnectionState::Stopped {
            self.state = ConnectionState::Connecting;
        }
    }

    pub fn authenticated(&mut self, session: ContextId) {
        if self.state != ConnectionState::Stopped {
            self.session = Some(session);
            self.state = ConnectionState::Authenticated;
        }
    }

    /// Records the only event that resets the reconnect backoff series.
    pub fn valid_availability_result(&mut self) {
        if self.state == ConnectionState::Authenticated {
            self.backoff_index = 0;
        }
    }

    pub fn restart_after_pairing(&mut self) {
        self.state = ConnectionState::Disconnected;
        self.session = None;
        self.active = None;
        self.backoff_index = 0;
        self.terminal_reason = None;
    }

    #[must_use]
    pub const fn terminal_reason(&self) -> Option<TerminalReason> {
        self.terminal_reason
    }

    #[must_use]
    pub fn reconnect_delay_ms(&self) -> Option<u32> {
        (self.state == ConnectionState::Backoff)
            .then(|| RECONNECT_DELAYS_MS[self.backoff_index.min(RECONNECT_DELAYS_MS.len() - 1)])
    }

    pub fn connection_failed(&mut self) -> Result<u32, ProtocolAdapterError> {
        if self.state == ConnectionState::Stopped {
            return Err(ProtocolAdapterError::Stopped);
        }
        self.state = ConnectionState::Backoff;
        let delay = RECONNECT_DELAYS_MS[self.backoff_index.min(RECONNECT_DELAYS_MS.len() - 1)];
        self.backoff_index = (self.backoff_index + 1).min(RECONNECT_DELAYS_MS.len() - 1);
        Ok(delay)
    }

    pub fn hello_rejected(&mut self, reason: HelloRejectReason) {
        match reason {
            HelloRejectReason::SessionBusy => {
                let _ = self.connection_failed();
            }
            HelloRejectReason::NoCommonVersion | HelloRejectReason::RequiredFeatureUnsupported => {
                self.state = ConnectionState::Stopped;
                self.terminal_reason = Some(TerminalReason::Incompatible);
            }
            HelloRejectReason::PermissionDenied => {
                self.state = ConnectionState::Stopped;
                self.terminal_reason = Some(TerminalReason::AuthorizationDenied);
            }
        }
    }

    pub fn begin_read(
        &mut self,
        effect_id: u64,
        operation: ContextId,
    ) -> Result<u32, ProtocolAdapterError> {
        if self.state != ConnectionState::Authenticated {
            return Err(ProtocolAdapterError::NotAuthenticated);
        }
        if self.active.is_some() {
            return Err(ProtocolAdapterError::ActiveRead);
        }
        if effect_id == 0 {
            return Err(ProtocolAdapterError::NoActiveRead);
        }
        let next = self
            .request_id
            .checked_add(1)
            .ok_or(ProtocolAdapterError::RequestMismatch)?;
        self.request_id = next;
        self.active = Some((operation, next, effect_id));
        Ok(next)
    }

    pub fn result(
        &mut self,
        session: ContextId,
        request_id: u32,
        operation: ContextId,
        result: AvailabilityResult,
    ) -> Result<ProtocolEvent, ProtocolAdapterError> {
        if self.state != ConnectionState::Authenticated || self.session != Some(session) {
            return Err(ProtocolAdapterError::SessionMismatch);
        }
        let Some((expected_operation, expected_request, effect_id)) = self.active else {
            return Err(ProtocolAdapterError::NoActiveRead);
        };
        if expected_request != request_id || expected_operation != operation {
            return Err(ProtocolAdapterError::RequestMismatch);
        }
        let value = match result {
            AvailabilityResult::Available => AvailabilityValue::Available,
            AvailabilityResult::Unavailable => AvailabilityValue::Unavailable,
            AvailabilityResult::ReadFailed => AvailabilityValue::ReadFailed,
        };
        self.active = None;
        self.backoff_index = 0;
        Ok(ProtocolEvent::AvailabilityCompleted { effect_id, value })
    }

    pub fn disconnected(&mut self) -> Option<ProtocolEvent> {
        if self.state == ConnectionState::Stopped {
            return None;
        }
        self.state = ConnectionState::Backoff;
        self.session = None;
        self.active = None;
        self.terminal_reason = None;
        Some(ProtocolEvent::SessionInvalidated)
    }

    pub fn stop(&mut self) {
        self.state = ConnectionState::Stopped;
        self.session = None;
        self.active = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_all_results_and_rejects_stale_session() {
        for (wire, value) in [
            (AvailabilityResult::Available, AvailabilityValue::Available),
            (
                AvailabilityResult::Unavailable,
                AvailabilityValue::Unavailable,
            ),
            (
                AvailabilityResult::ReadFailed,
                AvailabilityValue::ReadFailed,
            ),
        ] {
            let mut adapter = ProtocolAdapter::new();
            adapter.authenticated([1; 16]);
            let request = adapter.begin_read(7, [2; 16]).unwrap();
            assert_eq!(
                adapter.result([1; 16], request, [2; 16], wire).unwrap(),
                ProtocolEvent::AvailabilityCompleted {
                    effect_id: 7,
                    value,
                }
            );
            assert_eq!(
                adapter.result([9; 16], request, [2; 16], wire),
                Err(ProtocolAdapterError::SessionMismatch)
            );
        }
    }

    #[test]
    fn disconnect_emits_semantic_invalidation() {
        let mut adapter = ProtocolAdapter::new();
        adapter.authenticated([1; 16]);
        assert_eq!(
            adapter.disconnected(),
            Some(ProtocolEvent::SessionInvalidated)
        );
        assert_eq!(adapter.state(), ConnectionState::Backoff);
    }

    #[test]
    fn reconnect_series_resets_only_after_valid_result() {
        let mut adapter = ProtocolAdapter::new();
        assert_eq!(adapter.connection_failed(), Ok(250));
        assert_eq!(adapter.connection_failed(), Ok(500));
        adapter.authenticated([1; 16]);
        let request = adapter.begin_read(1, [2; 16]).unwrap();
        adapter
            .result([1; 16], request, [2; 16], AvailabilityResult::Available)
            .unwrap();
        assert_eq!(adapter.connection_failed(), Ok(250));
        adapter.hello_rejected(HelloRejectReason::PermissionDenied);
        assert_eq!(adapter.state(), ConnectionState::Stopped);
        assert_eq!(
            adapter.terminal_reason(),
            Some(TerminalReason::AuthorizationDenied)
        );
    }

    #[test]
    fn authentication_alone_does_not_reset_backoff() {
        let mut adapter = ProtocolAdapter::new();
        assert_eq!(adapter.connection_failed(), Ok(250));
        assert_eq!(adapter.connection_failed(), Ok(500));
        adapter.authenticated([1; 16]);
        assert_eq!(adapter.connection_failed(), Ok(1_000));
        adapter.authenticated([2; 16]);
        adapter.valid_availability_result();
        assert_eq!(adapter.connection_failed(), Ok(250));
    }

    #[test]
    fn repair_restarts_stopped_adapter() {
        let mut adapter = ProtocolAdapter::new();
        adapter.authenticated([1; 16]);
        adapter.stop();
        adapter.restart_after_pairing();
        assert_eq!(adapter.state(), ConnectionState::Disconnected);
        assert_eq!(adapter.connection_failed(), Ok(250));
    }
}
