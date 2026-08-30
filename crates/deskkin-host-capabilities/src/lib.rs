#![forbid(unsafe_code)]

use core::num::NonZeroU64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityId {
    AvailabilityRead,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectorId {
    DeterministicAvailability,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Lifecycle {
    Start,
    Stop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AvailabilityValue {
    Available,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityFailure {
    ReadFailed,
    ConnectorUnavailable,
    Timeout,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityRequest {
    ReadAvailability,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityResult {
    Availability(Result<AvailabilityValue, CapabilityFailure>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeterministicAvailability {
    Available,
    Unavailable,
    ReadFailed,
    ConnectorUnavailable,
    Timeout,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapabilityEffectId {
    pub capability: CapabilityId,
    pub connector: ConnectorId,
    pub local: NonZeroU64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectorRequest {
    ReadAvailability,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RoutedRequest {
    pub id: CapabilityEffectId,
    pub request: ConnectorRequest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectorResult {
    Availability(Result<AvailabilityValue, CapabilityFailure>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectorCompletion {
    pub id: CapabilityEffectId,
    pub result: ConnectorResult,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Invocation {
    pub routed: RoutedRequest,
    pub result: CapabilityResult,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostCapabilityError {
    AlreadyStarted,
    NotStarted,
    Busy,
    UnexpectedCompletion,
    EffectIdentityMismatch,
    EffectIdentityExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RuntimeState {
    Stopped,
    Ready,
    Waiting { id: CapabilityEffectId },
}

/// The single owner of connector lifecycle and in-flight effect identity.
///
/// The owner is intentionally neither cloneable nor copyable so one completion
/// cannot be accepted by diverging copies of the same runtime state.
///
/// ```compile_fail
/// use deskkin_host_capabilities::{DeterministicAvailability, HostCapabilities};
/// let owner = HostCapabilities::new(DeterministicAvailability::Available);
/// let moved = owner;
/// let _ = (owner, moved);
/// ```
#[derive(Debug, Eq, PartialEq)]
pub struct HostCapabilities {
    state: RuntimeState,
    availability: DeterministicAvailability,
    next_effect_id: u64,
}

impl HostCapabilities {
    #[must_use]
    pub const fn new(availability: DeterministicAvailability) -> Self {
        Self {
            state: RuntimeState::Stopped,
            availability,
            next_effect_id: 0,
        }
    }

    /// Applies connector lifecycle without publishing a partial state change.
    ///
    /// # Errors
    ///
    /// Returns a closed lifecycle error when the requested transition is invalid.
    pub fn lifecycle(&mut self, lifecycle: Lifecycle) -> Result<(), HostCapabilityError> {
        match (self.state, lifecycle) {
            (RuntimeState::Stopped, Lifecycle::Start) => self.state = RuntimeState::Ready,
            (RuntimeState::Stopped, Lifecycle::Stop) => {
                return Err(HostCapabilityError::NotStarted);
            }
            (RuntimeState::Ready | RuntimeState::Waiting { .. }, Lifecycle::Start) => {
                return Err(HostCapabilityError::AlreadyStarted);
            }
            (RuntimeState::Ready | RuntimeState::Waiting { .. }, Lifecycle::Stop) => {
                self.state = RuntimeState::Stopped;
            }
        }
        Ok(())
    }

    /// Routes one semantic capability request to the statically registered connector.
    ///
    /// # Errors
    ///
    /// Returns a closed lifecycle, capacity, or identity error without mutation.
    pub fn route(
        &mut self,
        request: CapabilityRequest,
    ) -> Result<RoutedRequest, HostCapabilityError> {
        if matches!(self.state, RuntimeState::Stopped) {
            return Err(HostCapabilityError::NotStarted);
        }
        if matches!(self.state, RuntimeState::Waiting { .. }) {
            return Err(HostCapabilityError::Busy);
        }
        let next = self
            .next_effect_id
            .checked_add(1)
            .and_then(NonZeroU64::new)
            .ok_or(HostCapabilityError::EffectIdentityExhausted)?;
        let (capability, connector, request) = match request {
            CapabilityRequest::ReadAvailability => (
                CapabilityId::AvailabilityRead,
                ConnectorId::DeterministicAvailability,
                ConnectorRequest::ReadAvailability,
            ),
        };
        let routed = RoutedRequest {
            id: CapabilityEffectId {
                capability,
                connector,
                local: next,
            },
            request,
        };
        self.next_effect_id = next.get();
        self.state = RuntimeState::Waiting { id: routed.id };
        Ok(routed)
    }

    #[must_use]
    pub const fn execute(&self, request: RoutedRequest) -> ConnectorCompletion {
        let result = match (request.request, self.availability) {
            (ConnectorRequest::ReadAvailability, DeterministicAvailability::Available) => {
                Ok(AvailabilityValue::Available)
            }
            (ConnectorRequest::ReadAvailability, DeterministicAvailability::Unavailable) => {
                Ok(AvailabilityValue::Unavailable)
            }
            (ConnectorRequest::ReadAvailability, DeterministicAvailability::ReadFailed) => {
                Err(CapabilityFailure::ReadFailed)
            }
            (
                ConnectorRequest::ReadAvailability,
                DeterministicAvailability::ConnectorUnavailable,
            ) => Err(CapabilityFailure::ConnectorUnavailable),
            (ConnectorRequest::ReadAvailability, DeterministicAvailability::Timeout) => {
                Err(CapabilityFailure::Timeout)
            }
            (ConnectorRequest::ReadAvailability, DeterministicAvailability::Cancelled) => {
                Err(CapabilityFailure::Cancelled)
            }
        };
        ConnectorCompletion {
            id: request.id,
            result: ConnectorResult::Availability(result),
        }
    }

    /// Accepts only the exact completion for the currently routed request.
    ///
    /// # Errors
    ///
    /// Returns a closed state or identity error without mutation.
    pub fn complete(
        &mut self,
        completion: ConnectorCompletion,
    ) -> Result<CapabilityResult, HostCapabilityError> {
        let RuntimeState::Waiting { id } = self.state else {
            return Err(HostCapabilityError::UnexpectedCompletion);
        };
        if completion.id != id {
            return Err(HostCapabilityError::EffectIdentityMismatch);
        }
        let result = match completion.result {
            ConnectorResult::Availability(result) => CapabilityResult::Availability(result),
        };
        self.state = RuntimeState::Ready;
        Ok(result)
    }

    /// Runs the deterministic connector through the same route/complete path.
    ///
    /// # Errors
    ///
    /// Returns the corresponding routing or completion error.
    pub fn invoke(
        &mut self,
        request: CapabilityRequest,
    ) -> Result<Invocation, HostCapabilityError> {
        let routed = self.route(request)?;
        let completion = self.execute(routed);
        let result = self.complete(completion)?;
        Ok(Invocation { routed, result })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running(outcome: DeterministicAvailability) -> HostCapabilities {
        let mut capabilities = HostCapabilities::new(outcome);
        capabilities.lifecycle(Lifecycle::Start).unwrap();
        capabilities
    }

    #[test]
    fn registry_routes_and_maps_every_closed_outcome() {
        for (configured, expected) in [
            (
                DeterministicAvailability::Available,
                Ok(AvailabilityValue::Available),
            ),
            (
                DeterministicAvailability::Unavailable,
                Ok(AvailabilityValue::Unavailable),
            ),
            (
                DeterministicAvailability::ReadFailed,
                Err(CapabilityFailure::ReadFailed),
            ),
            (
                DeterministicAvailability::ConnectorUnavailable,
                Err(CapabilityFailure::ConnectorUnavailable),
            ),
            (
                DeterministicAvailability::Timeout,
                Err(CapabilityFailure::Timeout),
            ),
            (
                DeterministicAvailability::Cancelled,
                Err(CapabilityFailure::Cancelled),
            ),
        ] {
            let invocation = running(configured)
                .invoke(CapabilityRequest::ReadAvailability)
                .unwrap();
            assert_eq!(
                invocation.routed.id.capability,
                CapabilityId::AvailabilityRead
            );
            assert_eq!(
                invocation.routed.id.connector,
                ConnectorId::DeterministicAvailability
            );
            assert_eq!(invocation.result, CapabilityResult::Availability(expected));
        }
    }

    #[test]
    fn stale_mismatched_and_post_stop_completions_are_transactional() {
        let mut capabilities = running(DeterministicAvailability::Available);
        let routed = capabilities
            .route(CapabilityRequest::ReadAvailability)
            .unwrap();
        let before_state = capabilities.state;
        let before_next_effect_id = capabilities.next_effect_id;
        let mut wrong = capabilities.execute(routed);
        wrong.id.local = NonZeroU64::new(routed.id.local.get() + 1).unwrap();
        assert_eq!(
            capabilities.complete(wrong),
            Err(HostCapabilityError::EffectIdentityMismatch)
        );
        assert_eq!(capabilities.state, before_state);
        assert_eq!(capabilities.next_effect_id, before_next_effect_id);

        let completion = capabilities.execute(routed);
        capabilities.complete(completion).unwrap();
        let ready_state = capabilities.state;
        let ready_next_effect_id = capabilities.next_effect_id;
        assert_eq!(
            capabilities.complete(completion),
            Err(HostCapabilityError::UnexpectedCompletion)
        );
        assert_eq!(capabilities.state, ready_state);
        assert_eq!(capabilities.next_effect_id, ready_next_effect_id);
        capabilities.lifecycle(Lifecycle::Stop).unwrap();
        assert_eq!(
            capabilities.complete(completion),
            Err(HostCapabilityError::UnexpectedCompletion)
        );
    }

    #[test]
    fn busy_shutdown_and_identity_exhaustion_are_closed() {
        let mut capabilities = running(DeterministicAvailability::Available);
        capabilities
            .route(CapabilityRequest::ReadAvailability)
            .unwrap();
        assert_eq!(
            capabilities.route(CapabilityRequest::ReadAvailability),
            Err(HostCapabilityError::Busy)
        );
        capabilities.lifecycle(Lifecycle::Stop).unwrap();
        assert_eq!(
            capabilities.route(CapabilityRequest::ReadAvailability),
            Err(HostCapabilityError::NotStarted)
        );

        let mut exhausted = HostCapabilities {
            state: RuntimeState::Ready,
            availability: DeterministicAvailability::Available,
            next_effect_id: u64::MAX,
        };
        let before_state = exhausted.state;
        let before_next_effect_id = exhausted.next_effect_id;
        assert_eq!(
            exhausted.route(CapabilityRequest::ReadAvailability),
            Err(HostCapabilityError::EffectIdentityExhausted)
        );
        assert_eq!(exhausted.state, before_state);
        assert_eq!(exhausted.next_effect_id, before_next_effect_id);
    }
}
