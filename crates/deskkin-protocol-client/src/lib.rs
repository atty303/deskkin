#![no_std]
#![allow(clippy::missing_errors_doc)]

use application_core::{
    Availability, AvailabilityInvalidated, Core, Input, ReadCompleted, ReadError, State,
};
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
    CoreRejected,
    ActiveRead,
    Stopped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalReason {
    Incompatible,
    AuthorizationDenied,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtocolAdapter {
    state: ConnectionState,
    session: Option<ContextId>,
    request_id: u32,
    active: Option<(ContextId, u32, application_core::EffectId)>,
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
        core: &Core,
        operation: ContextId,
    ) -> Result<u32, ProtocolAdapterError> {
        if self.state != ConnectionState::Authenticated {
            return Err(ProtocolAdapterError::NotAuthenticated);
        }
        if self.active.is_some() {
            return Err(ProtocolAdapterError::ActiveRead);
        }
        let State::Reading { effect_id } = core.state() else {
            return Err(ProtocolAdapterError::NoActiveRead);
        };
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
        core: &mut Core,
        session: ContextId,
        request_id: u32,
        operation: ContextId,
        result: AvailabilityResult,
    ) -> Result<Option<application_core::Effect>, ProtocolAdapterError> {
        if self.state != ConnectionState::Authenticated || self.session != Some(session) {
            return Err(ProtocolAdapterError::SessionMismatch);
        }
        let Some((expected_operation, expected_request, effect_id)) = self.active else {
            return Err(ProtocolAdapterError::NoActiveRead);
        };
        if expected_request != request_id || expected_operation != operation {
            return Err(ProtocolAdapterError::RequestMismatch);
        }
        let result = match result {
            AvailabilityResult::Available => Ok(Availability::Available),
            AvailabilityResult::Unavailable => Ok(Availability::Unavailable),
            AvailabilityResult::ReadFailed => Err(ReadError::Unavailable),
        };
        let next = core
            .transition(Input::ReadCompleted(ReadCompleted { effect_id, result }))
            .map_err(|_| ProtocolAdapterError::CoreRejected)?
            .effect;
        self.active = None;
        self.backoff_index = 0;
        Ok(next)
    }

    pub fn disconnected(
        &mut self,
        core: &mut Core,
    ) -> Result<Option<application_core::Effect>, ProtocolAdapterError> {
        if self.state == ConnectionState::Stopped {
            return Ok(None);
        }
        self.state = ConnectionState::Backoff;
        self.session = None;
        self.active = None;
        self.terminal_reason = None;
        core.transition(Input::AvailabilityInvalidated(
            AvailabilityInvalidated::SourceUnavailable,
        ))
        .map(|transition| transition.effect)
        .map_err(|_| ProtocolAdapterError::CoreRejected)
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
    use application_core::{Command, StatusView, TimerArmCompleted};

    #[test]
    fn maps_all_results_and_rejects_stale_session() {
        for (wire, view) in [
            (AvailabilityResult::Available, StatusView::Available),
            (AvailabilityResult::Unavailable, StatusView::Unavailable),
            (AvailabilityResult::ReadFailed, StatusView::Unknown),
        ] {
            let mut core = Core::new();
            core.transition(Input::Command(Command::Start)).unwrap();
            let mut adapter = ProtocolAdapter::new();
            adapter.authenticated([1; 16]);
            let request = adapter.begin_read(&core, [2; 16]).unwrap();
            adapter
                .result(&mut core, [1; 16], request, [2; 16], wire)
                .unwrap();
            assert_eq!(core.view(), view);
            assert_eq!(
                adapter.result(&mut core, [9; 16], request, [2; 16], wire),
                Err(ProtocolAdapterError::SessionMismatch)
            );
        }
    }

    #[test]
    fn disconnect_invalidates_waiting_without_changing_timer() {
        let mut core = Core::new();
        let read = core
            .transition(Input::Command(Command::Start))
            .unwrap()
            .effect
            .unwrap();
        let timer = core
            .transition(Input::ReadCompleted(ReadCompleted {
                effect_id: read.id,
                result: Ok(Availability::Available),
            }))
            .unwrap()
            .effect
            .unwrap();
        core.transition(Input::TimerArmCompleted(TimerArmCompleted {
            effect_id: timer.id,
            result: Ok(()),
        }))
        .unwrap();
        let state = core.state();
        let mut adapter = ProtocolAdapter::new();
        adapter.authenticated([1; 16]);
        adapter.disconnected(&mut core).unwrap();
        assert_eq!(core.view(), StatusView::Unknown);
        assert_eq!(core.state(), state);
        assert_eq!(adapter.state(), ConnectionState::Backoff);
    }

    #[test]
    fn reconnect_series_resets_only_after_valid_result() {
        let mut adapter = ProtocolAdapter::new();
        assert_eq!(adapter.connection_failed(), Ok(250));
        assert_eq!(adapter.connection_failed(), Ok(500));
        adapter.authenticated([1; 16]);
        let mut core = Core::new();
        core.transition(Input::Command(Command::Start)).unwrap();
        let request = adapter.begin_read(&core, [2; 16]).unwrap();
        adapter
            .result(
                &mut core,
                [1; 16],
                request,
                [2; 16],
                AvailabilityResult::Available,
            )
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
