use application_core::{Lifecycle, LocalEffectId, SurfaceClass};

pub const REFRESH_DELAY_MS: u32 = 5_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Availability {
    Available,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Surface {
    Unknown,
    Available,
    Unavailable,
}

impl Surface {
    #[must_use]
    pub const fn class(self) -> SurfaceClass {
        SurfaceClass::Ambient
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectRequest {
    ReadAvailability,
    ArmRefreshTimer { delay_ms: u32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Effect {
    pub id: LocalEffectId,
    pub request: EffectRequest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadError {
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimerArmError {
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadCompleted {
    pub effect_id: LocalEffectId,
    pub result: Result<Availability, ReadError>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimerArmCompleted {
    pub effect_id: LocalEffectId,
    pub result: Result<(), TimerArmError>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RefreshDue {
    pub effect_id: LocalEffectId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Input {
    ReadCompleted(ReadCompleted),
    TimerArmCompleted(TimerArmCompleted),
    RefreshDue(RefreshDue),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    Stopped,
    Reading { effect_id: LocalEffectId },
    ArmingRefresh { effect_id: LocalEffectId },
    Waiting { effect_id: LocalEffectId },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransitionError {
    AlreadyStarted,
    NotStarted,
    UnexpectedInput,
    EffectIdentityMismatch {
        expected: LocalEffectId,
        actual: LocalEffectId,
    },
    EffectIdentityExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Transition {
    pub state: State,
    pub surface: Option<Surface>,
    pub effect: Option<Effect>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AvailabilityFeature {
    state: State,
    surface: Surface,
    next_effect_id: u64,
}

impl Default for AvailabilityFeature {
    fn default() -> Self {
        Self::new()
    }
}

impl AvailabilityFeature {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: State::Stopped,
            surface: Surface::Unknown,
            next_effect_id: 0,
        }
    }

    #[must_use]
    pub const fn state(&self) -> State {
        self.state
    }

    #[must_use]
    pub const fn surface(&self) -> Option<Surface> {
        if matches!(self.state, State::Stopped) {
            None
        } else {
            Some(self.surface)
        }
    }

    /// Applies one lifecycle event transactionally.
    ///
    /// # Errors
    ///
    /// Returns a closed lifecycle or effect-identity error without mutation.
    pub fn lifecycle(&mut self, lifecycle: Lifecycle) -> Result<Transition, TransitionError> {
        self.publish(|candidate| candidate.apply_lifecycle(lifecycle))
    }

    /// Applies one feature-local input transactionally.
    ///
    /// # Errors
    ///
    /// Returns a closed state or effect-identity error without mutation.
    pub fn transition(&mut self, input: Input) -> Result<Transition, TransitionError> {
        self.publish(|candidate| candidate.apply(input))
    }

    fn publish(
        &mut self,
        apply: impl FnOnce(&mut Self) -> Result<Option<Effect>, TransitionError>,
    ) -> Result<Transition, TransitionError> {
        let mut candidate = *self;
        let effect = apply(&mut candidate)?;
        *self = candidate;
        Ok(Transition {
            state: self.state,
            surface: self.surface(),
            effect,
        })
    }

    fn apply_lifecycle(&mut self, lifecycle: Lifecycle) -> Result<Option<Effect>, TransitionError> {
        match lifecycle {
            Lifecycle::Start if matches!(self.state, State::Stopped) => {
                self.surface = Surface::Unknown;
                let effect = self.allocate(EffectRequest::ReadAvailability)?;
                self.state = State::Reading {
                    effect_id: effect.id,
                };
                Ok(Some(effect))
            }
            Lifecycle::Start => Err(TransitionError::AlreadyStarted),
            Lifecycle::Stop if matches!(self.state, State::Stopped) => {
                Err(TransitionError::NotStarted)
            }
            Lifecycle::Stop => {
                self.state = State::Stopped;
                self.surface = Surface::Unknown;
                Ok(None)
            }
            Lifecycle::SessionInvalidated if matches!(self.state, State::Stopped) => Ok(None),
            Lifecycle::SessionInvalidated => self.invalidate(),
        }
    }

    fn invalidate(&mut self) -> Result<Option<Effect>, TransitionError> {
        match self.state {
            State::Stopped => Ok(None),
            State::Reading { effect_id } => self.apply(Input::ReadCompleted(ReadCompleted {
                effect_id,
                result: Err(ReadError::Unavailable),
            })),
            State::ArmingRefresh { .. } | State::Waiting { .. } => {
                self.surface = Surface::Unknown;
                Ok(None)
            }
        }
    }

    fn apply(&mut self, input: Input) -> Result<Option<Effect>, TransitionError> {
        match (self.state, input) {
            (State::Reading { effect_id }, Input::ReadCompleted(completion)) => {
                ensure_id(effect_id, completion.effect_id)?;
                self.surface = match completion.result {
                    Ok(Availability::Available) => Surface::Available,
                    Ok(Availability::Unavailable) => Surface::Unavailable,
                    Err(ReadError::Unavailable) => Surface::Unknown,
                };
                let effect = self.allocate(EffectRequest::ArmRefreshTimer {
                    delay_ms: REFRESH_DELAY_MS,
                })?;
                self.state = State::ArmingRefresh {
                    effect_id: effect.id,
                };
                Ok(Some(effect))
            }
            (State::ArmingRefresh { effect_id }, Input::TimerArmCompleted(completion)) => {
                ensure_id(effect_id, completion.effect_id)?;
                match completion.result {
                    Ok(()) => self.state = State::Waiting { effect_id },
                    Err(TimerArmError::Unavailable) => {
                        self.state = State::Stopped;
                        self.surface = Surface::Unknown;
                    }
                }
                Ok(None)
            }
            (State::Waiting { effect_id }, Input::RefreshDue(due)) => {
                ensure_id(effect_id, due.effect_id)?;
                let effect = self.allocate(EffectRequest::ReadAvailability)?;
                self.state = State::Reading {
                    effect_id: effect.id,
                };
                Ok(Some(effect))
            }
            _ => Err(TransitionError::UnexpectedInput),
        }
    }

    fn allocate(&mut self, request: EffectRequest) -> Result<Effect, TransitionError> {
        let next = self
            .next_effect_id
            .checked_add(1)
            .ok_or(TransitionError::EffectIdentityExhausted)?;
        let id = LocalEffectId::new(next).ok_or(TransitionError::EffectIdentityExhausted)?;
        self.next_effect_id = next;
        Ok(Effect { id, request })
    }
}

fn ensure_id(expected: LocalEffectId, actual: LocalEffectId) -> Result<(), TransitionError> {
    if expected == actual {
        Ok(())
    } else {
        Err(TransitionError::EffectIdentityMismatch { expected, actual })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(feature: &mut AvailabilityFeature) -> Effect {
        feature.lifecycle(Lifecycle::Start).unwrap().effect.unwrap()
    }

    fn read(
        feature: &mut AvailabilityFeature,
        id: LocalEffectId,
        result: Result<Availability, ReadError>,
    ) -> Effect {
        feature
            .transition(Input::ReadCompleted(ReadCompleted {
                effect_id: id,
                result,
            }))
            .unwrap()
            .effect
            .unwrap()
    }

    #[test]
    fn cycle_failures_invalidation_and_stop_are_preserved() {
        for (result, expected) in [
            (Ok(Availability::Available), Surface::Available),
            (Ok(Availability::Unavailable), Surface::Unavailable),
            (Err(ReadError::Unavailable), Surface::Unknown),
        ] {
            let mut feature = AvailabilityFeature::new();
            let read_effect = start(&mut feature);
            let timer = read(&mut feature, read_effect.id, result);
            assert_eq!(feature.surface(), Some(expected));
            feature
                .transition(Input::TimerArmCompleted(TimerArmCompleted {
                    effect_id: timer.id,
                    result: Ok(()),
                }))
                .unwrap();
            feature.lifecycle(Lifecycle::SessionInvalidated).unwrap();
            assert_eq!(feature.surface(), Some(Surface::Unknown));
            let retry = feature
                .transition(Input::RefreshDue(RefreshDue {
                    effect_id: timer.id,
                }))
                .unwrap()
                .effect
                .unwrap();
            assert_eq!(retry.request, EffectRequest::ReadAvailability);
            feature.lifecycle(Lifecycle::Stop).unwrap();
            assert_eq!(feature.surface(), None);
        }
    }

    #[test]
    fn timer_arm_failure_stops_with_no_surface() {
        let mut feature = AvailabilityFeature::new();
        let read_effect = start(&mut feature);
        let timer = read(&mut feature, read_effect.id, Err(ReadError::Unavailable));
        feature
            .transition(Input::TimerArmCompleted(TimerArmCompleted {
                effect_id: timer.id,
                result: Err(TimerArmError::Unavailable),
            }))
            .unwrap();
        assert_eq!(feature.state(), State::Stopped);
        assert_eq!(feature.surface(), None);
    }

    #[test]
    fn rejection_is_transactional() {
        let mut feature = AvailabilityFeature::new();
        let effect = start(&mut feature);
        let before = feature;
        let wrong = LocalEffectId::new(effect.id.get() + 1).unwrap();
        assert!(matches!(
            feature.transition(Input::ReadCompleted(ReadCompleted {
                effect_id: wrong,
                result: Ok(Availability::Available),
            })),
            Err(TransitionError::EffectIdentityMismatch { .. })
        ));
        assert_eq!(feature, before);
    }

    #[test]
    fn invalidation_preserves_an_existing_refresh_timer() {
        let mut feature = AvailabilityFeature::new();
        let read_effect = start(&mut feature);
        let timer = read(&mut feature, read_effect.id, Ok(Availability::Available));

        feature.lifecycle(Lifecycle::SessionInvalidated).unwrap();
        assert_eq!(feature.surface(), Some(Surface::Unknown));
        assert_eq!(
            feature.state(),
            State::ArmingRefresh {
                effect_id: timer.id
            }
        );

        feature
            .transition(Input::TimerArmCompleted(TimerArmCompleted {
                effect_id: timer.id,
                result: Ok(()),
            }))
            .unwrap();
        assert_eq!(
            feature.state(),
            State::Waiting {
                effect_id: timer.id
            }
        );
    }

    #[test]
    fn invalidation_during_read_converts_it_to_a_refresh_timer() {
        let mut feature = AvailabilityFeature::new();
        start(&mut feature);

        let transition = feature.lifecycle(Lifecycle::SessionInvalidated).unwrap();

        assert_eq!(transition.surface, Some(Surface::Unknown));
        assert_eq!(
            transition.effect.unwrap().request,
            EffectRequest::ArmRefreshTimer {
                delay_ms: REFRESH_DELAY_MS
            }
        );
    }

    #[test]
    fn effect_identity_exhaustion_is_transactional() {
        let mut feature = AvailabilityFeature {
            state: State::Stopped,
            surface: Surface::Unknown,
            next_effect_id: u64::MAX,
        };
        let before = feature;

        assert_eq!(
            feature.lifecycle(Lifecycle::Start),
            Err(TransitionError::EffectIdentityExhausted)
        );
        assert_eq!(feature, before);
    }
}
