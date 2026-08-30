use application_core::{Lifecycle, LocalEffectId, SurfaceClass};

pub const NOTICE_LIFETIME_MS: u32 = 2_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoticeKind {
    CompositionCheck,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Surface {
    pub kind: NoticeKind,
}

impl Surface {
    #[must_use]
    pub const fn class(self) -> SurfaceClass {
        SurfaceClass::Information
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    Show(NoticeKind),
    Clear,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectRequest {
    ArmExpiryTimer { delay_ms: u32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Effect {
    pub id: LocalEffectId,
    pub request: EffectRequest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimerArmError {
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimerArmCompleted {
    pub effect_id: LocalEffectId,
    pub result: Result<(), TimerArmError>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExpiryDue {
    pub effect_id: LocalEffectId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Input {
    Command(Command),
    TimerArmCompleted(TimerArmCompleted),
    ExpiryDue(ExpiryDue),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    Stopped,
    Idle,
    Arming {
        effect_id: LocalEffectId,
        kind: NoticeKind,
    },
    Visible {
        effect_id: LocalEffectId,
        kind: NoticeKind,
    },
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
pub struct SyntheticNoticeFeature {
    state: State,
    next_effect_id: u64,
}

impl Default for SyntheticNoticeFeature {
    fn default() -> Self {
        Self::new()
    }
}

impl SyntheticNoticeFeature {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: State::Stopped,
            next_effect_id: 0,
        }
    }

    #[must_use]
    pub const fn state(&self) -> State {
        self.state
    }

    #[must_use]
    pub const fn surface(&self) -> Option<Surface> {
        match self.state {
            State::Arming { kind, .. } | State::Visible { kind, .. } => Some(Surface { kind }),
            State::Stopped | State::Idle => None,
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
                self.state = State::Idle;
                Ok(None)
            }
            Lifecycle::Start => Err(TransitionError::AlreadyStarted),
            Lifecycle::Stop if matches!(self.state, State::Stopped) => {
                Err(TransitionError::NotStarted)
            }
            Lifecycle::Stop => {
                self.state = State::Stopped;
                Ok(None)
            }
            Lifecycle::SessionInvalidated if matches!(self.state, State::Stopped) => Ok(None),
            Lifecycle::SessionInvalidated => {
                self.state = State::Idle;
                Ok(None)
            }
        }
    }

    fn apply(&mut self, input: Input) -> Result<Option<Effect>, TransitionError> {
        match (self.state, input) {
            (State::Stopped, _) => Err(TransitionError::NotStarted),
            (_, Input::Command(Command::Show(kind))) => {
                let effect = self.allocate(EffectRequest::ArmExpiryTimer {
                    delay_ms: NOTICE_LIFETIME_MS,
                })?;
                self.state = State::Arming {
                    effect_id: effect.id,
                    kind,
                };
                Ok(Some(effect))
            }
            (State::Idle, Input::Command(Command::Clear)) => Ok(None),
            (State::Arming { .. } | State::Visible { .. }, Input::Command(Command::Clear)) => {
                self.state = State::Idle;
                Ok(None)
            }
            (State::Arming { effect_id, kind }, Input::TimerArmCompleted(completion)) => {
                ensure_id(effect_id, completion.effect_id)?;
                self.state = match completion.result {
                    Ok(()) => State::Visible { effect_id, kind },
                    Err(TimerArmError::Unavailable) => State::Idle,
                };
                Ok(None)
            }
            (State::Visible { effect_id, .. }, Input::ExpiryDue(due)) => {
                ensure_id(effect_id, due.effect_id)?;
                self.state = State::Idle;
                Ok(None)
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

    #[test]
    fn replacement_clear_expiry_and_invalidation_are_bounded() {
        let mut feature = SyntheticNoticeFeature::new();
        feature.lifecycle(Lifecycle::Start).unwrap();
        let first = feature
            .transition(Input::Command(Command::Show(NoticeKind::CompositionCheck)))
            .unwrap()
            .effect
            .unwrap();
        let second = feature
            .transition(Input::Command(Command::Show(NoticeKind::CompositionCheck)))
            .unwrap()
            .effect
            .unwrap();
        let before = feature;
        assert!(
            feature
                .transition(Input::TimerArmCompleted(TimerArmCompleted {
                    effect_id: first.id,
                    result: Ok(()),
                }))
                .is_err()
        );
        assert_eq!(feature, before);
        feature
            .transition(Input::TimerArmCompleted(TimerArmCompleted {
                effect_id: second.id,
                result: Ok(()),
            }))
            .unwrap();
        feature.lifecycle(Lifecycle::SessionInvalidated).unwrap();
        assert_eq!(feature.surface(), None);
        assert!(
            feature
                .transition(Input::ExpiryDue(ExpiryDue {
                    effect_id: second.id,
                }))
                .is_err()
        );
        feature.transition(Input::Command(Command::Clear)).unwrap();
    }

    #[test]
    fn exact_expiry_withdraws_surface() {
        let mut feature = SyntheticNoticeFeature::new();
        feature.lifecycle(Lifecycle::Start).unwrap();
        let effect = feature
            .transition(Input::Command(Command::Show(NoticeKind::CompositionCheck)))
            .unwrap()
            .effect
            .unwrap();
        feature
            .transition(Input::TimerArmCompleted(TimerArmCompleted {
                effect_id: effect.id,
                result: Ok(()),
            }))
            .unwrap();
        feature
            .transition(Input::ExpiryDue(ExpiryDue {
                effect_id: effect.id,
            }))
            .unwrap();
        assert_eq!(feature.surface(), None);
    }

    #[test]
    fn explicit_clear_and_timer_failure_withdraw_the_surface() {
        let mut feature = SyntheticNoticeFeature::new();
        feature.lifecycle(Lifecycle::Start).unwrap();
        let effect = feature
            .transition(Input::Command(Command::Show(NoticeKind::CompositionCheck)))
            .unwrap()
            .effect
            .unwrap();
        feature.transition(Input::Command(Command::Clear)).unwrap();
        assert_eq!(feature.surface(), None);
        assert!(
            feature
                .transition(Input::TimerArmCompleted(TimerArmCompleted {
                    effect_id: effect.id,
                    result: Ok(()),
                }))
                .is_err()
        );

        let effect = feature
            .transition(Input::Command(Command::Show(NoticeKind::CompositionCheck)))
            .unwrap()
            .effect
            .unwrap();
        feature
            .transition(Input::TimerArmCompleted(TimerArmCompleted {
                effect_id: effect.id,
                result: Err(TimerArmError::Unavailable),
            }))
            .unwrap();
        assert_eq!(feature.state(), State::Idle);
        assert_eq!(feature.surface(), None);
    }

    #[test]
    fn effect_identity_exhaustion_is_transactional() {
        let mut feature = SyntheticNoticeFeature {
            state: State::Idle,
            next_effect_id: u64::MAX,
        };
        let before = feature;

        assert_eq!(
            feature.transition(Input::Command(Command::Show(NoticeKind::CompositionCheck))),
            Err(TransitionError::EffectIdentityExhausted)
        );
        assert_eq!(feature, before);
    }
}
