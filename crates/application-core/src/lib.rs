#![no_std]
#![forbid(unsafe_code)]

pub const REFRESH_DELAY_MS: u32 = 5_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Availability {
    Available,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusView {
    Unknown,
    Available,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    Start,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AvailabilityInvalidated {
    SourceUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct EffectId(u64);

impl EffectId {
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectRequest {
    ReadAvailability,
    ArmRefreshTimer { delay_ms: u32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Effect {
    pub id: EffectId,
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
    pub effect_id: EffectId,
    pub result: Result<Availability, ReadError>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimerArmCompleted {
    pub effect_id: EffectId,
    pub result: Result<(), TimerArmError>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RefreshDue {
    pub effect_id: EffectId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Input {
    Command(Command),
    ReadCompleted(ReadCompleted),
    TimerArmCompleted(TimerArmCompleted),
    RefreshDue(RefreshDue),
    AvailabilityInvalidated(AvailabilityInvalidated),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    Stopped,
    Reading { effect_id: EffectId },
    ArmingRefresh { effect_id: EffectId },
    Waiting { effect_id: EffectId },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransitionError {
    AlreadyStarted,
    UnexpectedInput,
    EffectIdentityMismatch {
        expected: EffectId,
        actual: EffectId,
    },
    EffectIdentityExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Transition {
    pub state: State,
    pub view: StatusView,
    pub effect: Option<Effect>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Core {
    state: State,
    view: StatusView,
    next_effect_id: u64,
}

impl Default for Core {
    fn default() -> Self {
        Self::new()
    }
}

impl Core {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: State::Stopped,
            view: StatusView::Unknown,
            next_effect_id: 0,
        }
    }

    #[must_use]
    pub const fn state(&self) -> State {
        self.state
    }

    #[must_use]
    pub const fn view(&self) -> StatusView {
        self.view
    }

    /// Applies one typed input without mutating the core when validation fails.
    ///
    /// # Errors
    ///
    /// Returns a typed validation or effect-identity error when the input is not
    /// accepted in the current state.
    pub fn transition(&mut self, input: Input) -> Result<Transition, TransitionError> {
        let mut candidate = *self;
        let effect = candidate.apply(input)?;
        *self = candidate;
        Ok(Transition {
            state: self.state,
            view: self.view,
            effect,
        })
    }

    fn apply(&mut self, input: Input) -> Result<Option<Effect>, TransitionError> {
        match (self.state, input) {
            (State::Stopped, Input::AvailabilityInvalidated(_)) => Ok(None),
            (State::Reading { effect_id }, Input::AvailabilityInvalidated(_)) => {
                self.apply(Input::ReadCompleted(ReadCompleted {
                    effect_id,
                    result: Err(ReadError::Unavailable),
                }))
            }
            (
                State::ArmingRefresh { .. } | State::Waiting { .. },
                Input::AvailabilityInvalidated(_),
            ) => {
                self.view = StatusView::Unknown;
                Ok(None)
            }
            (State::Stopped, Input::Command(Command::Start)) => {
                let effect = self.allocate(EffectRequest::ReadAvailability)?;
                self.state = State::Reading {
                    effect_id: effect.id,
                };
                Ok(Some(effect))
            }
            (_, Input::Command(Command::Start)) => Err(TransitionError::AlreadyStarted),
            (State::Reading { effect_id }, Input::ReadCompleted(completion)) => {
                ensure_id(effect_id, completion.effect_id)?;
                self.view = match completion.result {
                    Ok(Availability::Available) => StatusView::Available,
                    Ok(Availability::Unavailable) => StatusView::Unavailable,
                    Err(ReadError::Unavailable) => StatusView::Unknown,
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
                        self.view = StatusView::Unknown;
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
        self.next_effect_id = next;
        Ok(Effect {
            id: EffectId(next),
            request,
        })
    }
}

fn ensure_id(expected: EffectId, actual: EffectId) -> Result<(), TransitionError> {
    if expected == actual {
        Ok(())
    } else {
        Err(TransitionError::EffectIdentityMismatch { expected, actual })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(core: &mut Core) -> Effect {
        core.transition(Input::Command(Command::Start))
            .unwrap()
            .effect
            .unwrap()
    }

    fn read(core: &mut Core, id: EffectId, result: Result<Availability, ReadError>) -> Effect {
        core.transition(Input::ReadCompleted(ReadCompleted {
            effect_id: id,
            result,
        }))
        .unwrap()
        .effect
        .unwrap()
    }

    #[test]
    fn startup_requests_read() {
        let mut core = Core::new();
        let effect = start(&mut core);
        assert_eq!(effect.request, EffectRequest::ReadAvailability);
        assert_eq!(
            core.state(),
            State::Reading {
                effect_id: effect.id
            }
        );
        assert_eq!(core.view(), StatusView::Unknown);
    }

    #[test]
    fn both_availability_values_are_displayed_and_timer_is_armed() {
        for (availability, view) in [
            (Availability::Available, StatusView::Available),
            (Availability::Unavailable, StatusView::Unavailable),
        ] {
            let mut core = Core::new();
            let read_effect = start(&mut core);
            let timer = read(&mut core, read_effect.id, Ok(availability));
            assert_eq!(core.view(), view);
            assert_eq!(
                timer.request,
                EffectRequest::ArmRefreshTimer { delay_ms: 5_000 }
            );
        }
    }

    #[test]
    fn read_failure_is_unknown_and_retries_after_due() {
        let mut core = Core::new();
        let read_effect = start(&mut core);
        let timer = read(&mut core, read_effect.id, Err(ReadError::Unavailable));
        assert_eq!(core.view(), StatusView::Unknown);
        core.transition(Input::TimerArmCompleted(TimerArmCompleted {
            effect_id: timer.id,
            result: Ok(()),
        }))
        .unwrap();
        let retry = core
            .transition(Input::RefreshDue(RefreshDue {
                effect_id: timer.id,
            }))
            .unwrap()
            .effect
            .unwrap();
        assert_eq!(retry.request, EffectRequest::ReadAvailability);
    }

    #[test]
    fn timer_arm_failure_stops_with_unknown() {
        let mut core = Core::new();
        let read_effect = start(&mut core);
        let timer = read(&mut core, read_effect.id, Ok(Availability::Available));
        core.transition(Input::TimerArmCompleted(TimerArmCompleted {
            effect_id: timer.id,
            result: Err(TimerArmError::Unavailable),
        }))
        .unwrap();
        assert_eq!(core.state(), State::Stopped);
        assert_eq!(core.view(), StatusView::Unknown);
    }

    #[test]
    fn source_loss_completes_active_read_once() {
        let mut core = Core::new();
        let _read_effect = start(&mut core);
        let transition = core
            .transition(Input::AvailabilityInvalidated(
                AvailabilityInvalidated::SourceUnavailable,
            ))
            .unwrap();
        assert_eq!(transition.view, StatusView::Unknown);
        assert_eq!(
            transition.effect.unwrap().request,
            EffectRequest::ArmRefreshTimer { delay_ms: 5_000 }
        );
        assert_eq!(
            core.transition(Input::AvailabilityInvalidated(
                AvailabilityInvalidated::SourceUnavailable,
            )),
            Ok(Transition {
                state: core.state(),
                view: StatusView::Unknown,
                effect: None
            })
        );
    }

    #[test]
    fn source_loss_preserves_waiting_effect_and_stopped_is_ignored() {
        let mut stopped = Core::new();
        let before = stopped;
        stopped
            .transition(Input::AvailabilityInvalidated(
                AvailabilityInvalidated::SourceUnavailable,
            ))
            .unwrap();
        assert_eq!(stopped, before);

        let mut core = Core::new();
        let read_effect = start(&mut core);
        let timer = read(&mut core, read_effect.id, Ok(Availability::Available));
        core.transition(Input::TimerArmCompleted(TimerArmCompleted {
            effect_id: timer.id,
            result: Ok(()),
        }))
        .unwrap();
        let waiting = core.state();
        core.transition(Input::AvailabilityInvalidated(
            AvailabilityInvalidated::SourceUnavailable,
        ))
        .unwrap();
        assert_eq!(core.state(), waiting);
        assert_eq!(core.view(), StatusView::Unknown);
    }

    #[test]
    fn duplicate_start_does_not_mutate() {
        let mut core = Core::new();
        start(&mut core);
        let before = core;
        assert_eq!(
            core.transition(Input::Command(Command::Start)),
            Err(TransitionError::AlreadyStarted)
        );
        assert_eq!(core, before);
    }

    #[test]
    fn stale_identity_does_not_mutate() {
        let mut core = Core::new();
        let effect = start(&mut core);
        let before = core;
        let stale = EffectId(effect.id.get() + 1);
        assert!(matches!(
            core.transition(Input::ReadCompleted(ReadCompleted {
                effect_id: stale,
                result: Ok(Availability::Available),
            })),
            Err(TransitionError::EffectIdentityMismatch { .. })
        ));
        assert_eq!(core, before);
    }

    #[test]
    fn identity_exhaustion_does_not_mutate() {
        let mut core = Core {
            next_effect_id: u64::MAX,
            ..Core::new()
        };
        let before = core;
        assert_eq!(
            core.transition(Input::Command(Command::Start)),
            Err(TransitionError::EffectIdentityExhausted)
        );
        assert_eq!(core, before);
    }
}
