#![no_std]
#![forbid(unsafe_code)]

pub use application_core::{Lifecycle, LocalEffectId};
pub use application_features::{availability, synthetic_notice};

pub const FEATURE_COUNT: usize = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FeatureId {
    Availability,
    SyntheticNotice,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApplicationEffectId {
    pub feature: FeatureId,
    pub local: LocalEffectId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectRequest {
    Availability(availability::EffectRequest),
    SyntheticNotice(synthetic_notice::EffectRequest),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Effect {
    pub id: ApplicationEffectId,
    pub request: EffectRequest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EffectBatch {
    items: [Option<Effect>; FEATURE_COUNT],
    len: usize,
}

impl EffectBatch {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            items: [None; FEATURE_COUNT],
            len: 0,
        }
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[must_use]
    pub fn get(&self, index: usize) -> Option<Effect> {
        self.items.get(index).copied().flatten()
    }

    fn push(&mut self, effect: Effect) -> Result<(), TransitionError> {
        if self.len == FEATURE_COUNT {
            return Err(TransitionError::EffectBatchExhausted);
        }
        self.items[self.len] = Some(effect);
        self.len += 1;
        Ok(())
    }
}

impl Default for EffectBatch {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApplicationViews {
    pub availability: Option<availability::Surface>,
    pub synthetic_notice: Option<synthetic_notice::NoticeKind>,
}

impl ApplicationViews {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            availability: None,
            synthetic_notice: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplicationInput {
    Lifecycle(Lifecycle),
    AvailabilityEffect {
        id: ApplicationEffectId,
        input: availability::Input,
    },
    SyntheticNoticeCommand(synthetic_notice::Command),
    SyntheticNoticeEffect {
        id: ApplicationEffectId,
        input: synthetic_notice::Input,
    },
}

impl ApplicationInput {
    #[must_use]
    pub const fn availability(id: ApplicationEffectId, input: availability::Input) -> Self {
        Self::AvailabilityEffect { id, input }
    }

    #[must_use]
    pub const fn synthetic_notice_command(command: synthetic_notice::Command) -> Self {
        Self::SyntheticNoticeCommand(command)
    }

    #[must_use]
    pub const fn synthetic_notice_effect(
        id: ApplicationEffectId,
        input: synthetic_notice::Input,
    ) -> Self {
        Self::SyntheticNoticeEffect { id, input }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransitionError {
    AlreadyStarted,
    NotStarted,
    AvailabilityRejected(availability::TransitionError),
    SyntheticNoticeRejected(synthetic_notice::TransitionError),
    EffectFeatureMismatch {
        expected: FeatureId,
        actual: FeatureId,
    },
    EffectIdentityMismatch {
        expected: LocalEffectId,
        actual: LocalEffectId,
    },
    UnexpectedEffectInput,
    EffectBatchExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Transition {
    pub view: ApplicationViews,
    pub effects: EffectBatch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ApplicationState {
    Stopped,
    Running,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Application {
    state: ApplicationState,
    availability: availability::AvailabilityFeature,
    synthetic_notice: synthetic_notice::SyntheticNoticeFeature,
    view: ApplicationViews,
}

impl Default for Application {
    fn default() -> Self {
        Self::new()
    }
}

impl Application {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: ApplicationState::Stopped,
            availability: availability::AvailabilityFeature::new(),
            synthetic_notice: synthetic_notice::SyntheticNoticeFeature::new(),
            view: ApplicationViews::empty(),
        }
    }

    #[must_use]
    pub const fn view(&self) -> ApplicationViews {
        self.view
    }

    /// Applies one routed or broadcast input transactionally.
    ///
    /// # Errors
    ///
    /// Returns a closed lifecycle, feature, identity, or capacity error without
    /// publishing candidate state or surface changes.
    pub fn transition(&mut self, input: ApplicationInput) -> Result<Transition, TransitionError> {
        let mut candidate = *self;
        let effects = candidate.apply(input)?;
        candidate.view = candidate.current_views();
        *self = candidate;
        Ok(Transition {
            view: self.view,
            effects,
        })
    }

    fn apply(&mut self, input: ApplicationInput) -> Result<EffectBatch, TransitionError> {
        match input {
            ApplicationInput::Lifecycle(lifecycle) => self.apply_lifecycle(lifecycle),
            ApplicationInput::AvailabilityEffect { id, input } => {
                if self.state == ApplicationState::Stopped {
                    return Err(TransitionError::NotStarted);
                }
                validate_effect_input(id, FeatureId::Availability, availability_input_id(input))?;
                let transition = self
                    .availability
                    .transition(input)
                    .map_err(TransitionError::AvailabilityRejected)?;
                single_availability_effect(transition.effect)
            }
            ApplicationInput::SyntheticNoticeCommand(command) => {
                if self.state == ApplicationState::Stopped {
                    return Err(TransitionError::NotStarted);
                }
                let transition = self
                    .synthetic_notice
                    .transition(synthetic_notice::Input::Command(command))
                    .map_err(TransitionError::SyntheticNoticeRejected)?;
                single_notice_effect(transition.effect)
            }
            ApplicationInput::SyntheticNoticeEffect { id, input } => {
                if self.state == ApplicationState::Stopped {
                    return Err(TransitionError::NotStarted);
                }
                validate_effect_input(
                    id,
                    FeatureId::SyntheticNotice,
                    synthetic_notice_input_id(input)?,
                )?;
                let transition = self
                    .synthetic_notice
                    .transition(input)
                    .map_err(TransitionError::SyntheticNoticeRejected)?;
                single_notice_effect(transition.effect)
            }
        }
    }

    fn apply_lifecycle(&mut self, lifecycle: Lifecycle) -> Result<EffectBatch, TransitionError> {
        match (self.state, lifecycle) {
            (ApplicationState::Running, Lifecycle::Start) => {
                return Err(TransitionError::AlreadyStarted);
            }
            (ApplicationState::Stopped, Lifecycle::Stop) => {
                return Err(TransitionError::NotStarted);
            }
            _ => {}
        }

        let mut effects = EffectBatch::new();
        let availability = self
            .availability
            .lifecycle(lifecycle)
            .map_err(TransitionError::AvailabilityRejected)?;
        if let Some(effect) = availability.effect {
            effects.push(wrap_availability(effect))?;
        }
        let notice = self
            .synthetic_notice
            .lifecycle(lifecycle)
            .map_err(TransitionError::SyntheticNoticeRejected)?;
        if let Some(effect) = notice.effect {
            effects.push(wrap_notice(effect))?;
        }
        self.state = match lifecycle {
            Lifecycle::Start => ApplicationState::Running,
            Lifecycle::SessionInvalidated => self.state,
            Lifecycle::Stop => ApplicationState::Stopped,
        };
        Ok(effects)
    }

    fn current_views(&self) -> ApplicationViews {
        ApplicationViews {
            availability: self.availability.surface(),
            synthetic_notice: self.synthetic_notice.surface().map(|surface| surface.kind),
        }
    }
}

fn validate_effect_input(
    id: ApplicationEffectId,
    expected_feature: FeatureId,
    input_id: LocalEffectId,
) -> Result<(), TransitionError> {
    if id.feature != expected_feature {
        return Err(TransitionError::EffectFeatureMismatch {
            expected: expected_feature,
            actual: id.feature,
        });
    }
    if id.local != input_id {
        return Err(TransitionError::EffectIdentityMismatch {
            expected: id.local,
            actual: input_id,
        });
    }
    Ok(())
}

const fn availability_input_id(input: availability::Input) -> LocalEffectId {
    match input {
        availability::Input::ReadCompleted(completion) => completion.effect_id,
        availability::Input::TimerArmCompleted(completion) => completion.effect_id,
        availability::Input::RefreshDue(due) => due.effect_id,
    }
}

fn synthetic_notice_input_id(
    input: synthetic_notice::Input,
) -> Result<LocalEffectId, TransitionError> {
    match input {
        synthetic_notice::Input::TimerArmCompleted(completion) => Ok(completion.effect_id),
        synthetic_notice::Input::ExpiryDue(due) => Ok(due.effect_id),
        synthetic_notice::Input::Command(_) => Err(TransitionError::UnexpectedEffectInput),
    }
}

fn single_availability_effect(
    effect: Option<availability::Effect>,
) -> Result<EffectBatch, TransitionError> {
    let mut effects = EffectBatch::new();
    if let Some(effect) = effect {
        effects.push(wrap_availability(effect))?;
    }
    Ok(effects)
}

fn single_notice_effect(
    effect: Option<synthetic_notice::Effect>,
) -> Result<EffectBatch, TransitionError> {
    let mut effects = EffectBatch::new();
    if let Some(effect) = effect {
        effects.push(wrap_notice(effect))?;
    }
    Ok(effects)
}

const fn wrap_availability(effect: availability::Effect) -> Effect {
    Effect {
        id: ApplicationEffectId {
            feature: FeatureId::Availability,
            local: effect.id,
        },
        request: EffectRequest::Availability(effect.request),
    }
}

const fn wrap_notice(effect: synthetic_notice::Effect) -> Effect {
    Effect {
        id: ApplicationEffectId {
            feature: FeatureId::SyntheticNotice,
            local: effect.id,
        },
        request: EffectRequest::SyntheticNotice(effect.request),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(application: &mut Application) -> Effect {
        application
            .transition(ApplicationInput::Lifecycle(Lifecycle::Start))
            .unwrap()
            .effects
            .get(0)
            .unwrap()
    }

    #[test]
    fn registry_start_routes_in_order_and_stop_rejects_late_completion() {
        let mut application = Application::new();
        let read = start(&mut application);
        assert_eq!(read.id.feature, FeatureId::Availability);
        assert_eq!(
            application.view(),
            ApplicationViews {
                availability: Some(availability::Surface::Unknown),
                synthetic_notice: None,
            }
        );
        application
            .transition(ApplicationInput::Lifecycle(Lifecycle::Stop))
            .unwrap();
        let before = application;
        assert!(matches!(
            application.transition(ApplicationInput::availability(
                read.id,
                availability::Input::ReadCompleted(availability::ReadCompleted {
                    effect_id: read.id.local,
                    result: Ok(availability::Availability::Available),
                })
            )),
            Err(TransitionError::NotStarted)
        ));
        assert_eq!(application, before);
    }

    #[test]
    fn namespaced_effects_do_not_collide_and_views_coexist() {
        let mut application = Application::new();
        let read = start(&mut application);
        let notice = application
            .transition(ApplicationInput::synthetic_notice_command(
                synthetic_notice::Command::Show(synthetic_notice::NoticeKind::CompositionCheck),
            ))
            .unwrap()
            .effects
            .get(0)
            .unwrap();
        assert_eq!(read.id.local, notice.id.local);
        assert_ne!(read.id.feature, notice.id.feature);
        assert_eq!(
            application.view(),
            ApplicationViews {
                availability: Some(availability::Surface::Unknown),
                synthetic_notice: Some(synthetic_notice::NoticeKind::CompositionCheck),
            }
        );

        let before = application;
        assert_eq!(
            application.transition(ApplicationInput::availability(
                notice.id,
                availability::Input::ReadCompleted(availability::ReadCompleted {
                    effect_id: notice.id.local,
                    result: Ok(availability::Availability::Available),
                }),
            )),
            Err(TransitionError::EffectFeatureMismatch {
                expected: FeatureId::Availability,
                actual: FeatureId::SyntheticNotice,
            })
        );
        assert_eq!(application, before);
        assert_eq!(
            application.transition(ApplicationInput::synthetic_notice_effect(
                read.id,
                synthetic_notice::Input::TimerArmCompleted(synthetic_notice::TimerArmCompleted {
                    effect_id: read.id.local,
                    result: Ok(()),
                },),
            )),
            Err(TransitionError::EffectFeatureMismatch {
                expected: FeatureId::SyntheticNotice,
                actual: FeatureId::Availability,
            })
        );
        assert_eq!(application, before);
    }

    #[test]
    fn covered_availability_update_is_revealed_on_clear() {
        let mut application = Application::new();
        let first_read = start(&mut application);
        let first_timer = application
            .transition(ApplicationInput::availability(
                first_read.id,
                availability::Input::ReadCompleted(availability::ReadCompleted {
                    effect_id: first_read.id.local,
                    result: Ok(availability::Availability::Available),
                }),
            ))
            .unwrap()
            .effects
            .get(0)
            .unwrap();
        application
            .transition(ApplicationInput::availability(
                first_timer.id,
                availability::Input::TimerArmCompleted(availability::TimerArmCompleted {
                    effect_id: first_timer.id.local,
                    result: Ok(()),
                }),
            ))
            .unwrap();
        application
            .transition(ApplicationInput::synthetic_notice_command(
                synthetic_notice::Command::Show(synthetic_notice::NoticeKind::CompositionCheck),
            ))
            .unwrap();
        let second_read = application
            .transition(ApplicationInput::availability(
                first_timer.id,
                availability::Input::RefreshDue(availability::RefreshDue {
                    effect_id: first_timer.id.local,
                }),
            ))
            .unwrap()
            .effects
            .get(0)
            .unwrap();
        application
            .transition(ApplicationInput::availability(
                second_read.id,
                availability::Input::ReadCompleted(availability::ReadCompleted {
                    effect_id: second_read.id.local,
                    result: Ok(availability::Availability::Unavailable),
                }),
            ))
            .unwrap();
        assert_eq!(
            application.view(),
            ApplicationViews {
                availability: Some(availability::Surface::Unavailable),
                synthetic_notice: Some(synthetic_notice::NoticeKind::CompositionCheck),
            }
        );
        application
            .transition(ApplicationInput::synthetic_notice_command(
                synthetic_notice::Command::Clear,
            ))
            .unwrap();
        assert_eq!(
            application.view(),
            ApplicationViews {
                availability: Some(availability::Surface::Unavailable),
                synthetic_notice: None,
            }
        );
    }

    #[test]
    fn invalidation_clears_notice_and_preserves_availability_schedule() {
        let mut application = Application::new();
        let read = start(&mut application);
        let timer = application
            .transition(ApplicationInput::availability(
                read.id,
                availability::Input::ReadCompleted(availability::ReadCompleted {
                    effect_id: read.id.local,
                    result: Ok(availability::Availability::Available),
                }),
            ))
            .unwrap()
            .effects
            .get(0)
            .unwrap();
        application
            .transition(ApplicationInput::availability(
                timer.id,
                availability::Input::TimerArmCompleted(availability::TimerArmCompleted {
                    effect_id: timer.id.local,
                    result: Ok(()),
                }),
            ))
            .unwrap();
        application
            .transition(ApplicationInput::synthetic_notice_command(
                synthetic_notice::Command::Show(synthetic_notice::NoticeKind::CompositionCheck),
            ))
            .unwrap();
        application
            .transition(ApplicationInput::Lifecycle(Lifecycle::SessionInvalidated))
            .unwrap();
        assert_eq!(
            application.view(),
            ApplicationViews {
                availability: Some(availability::Surface::Unknown),
                synthetic_notice: None,
            }
        );
        let retry = application
            .transition(ApplicationInput::availability(
                timer.id,
                availability::Input::RefreshDue(availability::RefreshDue {
                    effect_id: timer.id.local,
                }),
            ))
            .unwrap()
            .effects
            .get(0)
            .unwrap();
        assert_eq!(retry.id.feature, FeatureId::Availability);
    }

    #[test]
    fn effect_batch_capacity_is_closed() {
        let id = LocalEffectId::new(1).unwrap();
        let effect = Effect {
            id: ApplicationEffectId {
                feature: FeatureId::Availability,
                local: id,
            },
            request: EffectRequest::Availability(availability::EffectRequest::ReadAvailability),
        };
        let mut batch = EffectBatch::new();
        batch.push(effect).unwrap();
        batch.push(effect).unwrap();
        assert_eq!(
            batch.push(effect),
            Err(TransitionError::EffectBatchExhausted)
        );
    }
}
