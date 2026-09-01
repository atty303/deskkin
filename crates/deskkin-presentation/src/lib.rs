#![no_std]
#![forbid(unsafe_code)]

/// Presentation-only animation states supported by the embedded Pet skin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PetAnimationState {
    Idle,
    MoveRight,
    MoveLeft,
    Attend,
}

impl PetAnimationState {
    #[must_use]
    pub const fn loop_index(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::MoveRight => 1,
            Self::MoveLeft => 2,
            Self::Attend => 3,
        }
    }

    #[must_use]
    pub const fn frame_count(self) -> u8 {
        match self {
            Self::Idle | Self::Attend => 6,
            Self::MoveRight | Self::MoveLeft => 8,
        }
    }

    #[must_use]
    pub const fn frame_period_ms(self) -> u32 {
        match self {
            Self::Idle | Self::Attend => 100,
            Self::MoveRight | Self::MoveLeft => 50,
        }
    }
}

/// One frame in a normalized Pet animation loop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PetFrame {
    pub state: PetAnimationState,
    pub index: u8,
}

/// Deterministic, allocation-free Pet animation state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PetAnimator {
    state: PetAnimationState,
    index: u8,
    elapsed_in_frame_ms: u32,
}

impl Default for PetAnimator {
    fn default() -> Self {
        Self::new()
    }
}

impl PetAnimator {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: PetAnimationState::Idle,
            index: 0,
            elapsed_in_frame_ms: 0,
        }
    }

    #[must_use]
    pub const fn state(self) -> PetAnimationState {
        self.state
    }

    #[must_use]
    pub const fn frame(self) -> PetFrame {
        PetFrame {
            state: self.state,
            index: self.index,
        }
    }

    #[must_use]
    pub fn set_state(&mut self, state: PetAnimationState) -> PetFrame {
        if self.state != state {
            self.state = state;
            self.index = 0;
            self.elapsed_in_frame_ms = 0;
        }
        self.frame()
    }

    #[must_use]
    pub fn advance(&mut self, elapsed_ms: u32) -> PetFrame {
        let period = u64::from(self.state.frame_period_ms());
        let total = u64::from(self.elapsed_in_frame_ms) + u64::from(elapsed_ms);
        let steps = total / period;
        self.elapsed_in_frame_ms = u32::try_from(total % period).unwrap_or_default();
        self.index =
            u8::try_from((u64::from(self.index) + steps) % u64::from(self.state.frame_count()))
                .unwrap_or_default();
        self.frame()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_map_to_closed_loop_indices_and_frame_counts() {
        assert_eq!(PetAnimationState::Idle.loop_index(), 0);
        assert_eq!(PetAnimationState::MoveRight.loop_index(), 1);
        assert_eq!(PetAnimationState::MoveLeft.loop_index(), 2);
        assert_eq!(PetAnimationState::Attend.loop_index(), 3);
        assert_eq!(PetAnimationState::Idle.frame_count(), 6);
        assert_eq!(PetAnimationState::MoveRight.frame_count(), 8);
        assert_eq!(PetAnimationState::MoveLeft.frame_count(), 8);
        assert_eq!(PetAnimationState::Attend.frame_count(), 6);
    }

    #[test]
    fn movement_advances_at_twenty_frames_per_second_and_wraps() {
        let mut animator = PetAnimator::new();
        assert_eq!(
            animator.set_state(PetAnimationState::MoveRight),
            PetFrame {
                state: PetAnimationState::MoveRight,
                index: 0,
            }
        );
        assert_eq!(animator.advance(49).index, 0);
        assert_eq!(animator.advance(1).index, 1);
        assert_eq!(animator.advance(350).index, 0);
    }

    #[test]
    fn ambient_states_advance_at_ten_frames_per_second() {
        let mut animator = PetAnimator::new();
        assert_eq!(animator.advance(99).index, 0);
        assert_eq!(animator.advance(1).index, 1);
        assert_eq!(
            animator.set_state(PetAnimationState::Attend),
            PetFrame {
                state: PetAnimationState::Attend,
                index: 0,
            }
        );
        assert_eq!(animator.advance(600).index, 0);
    }

    #[test]
    fn changing_state_resets_frame_and_partial_time() {
        let mut animator = PetAnimator::new();
        assert_eq!(animator.advance(150).index, 1);
        assert_eq!(
            animator.set_state(PetAnimationState::MoveLeft),
            PetFrame {
                state: PetAnimationState::MoveLeft,
                index: 0,
            }
        );
        assert_eq!(animator.advance(49).index, 0);
        assert_eq!(animator.advance(1).index, 1);
    }

    #[test]
    fn large_elapsed_values_remain_bounded() {
        let mut animator = PetAnimator::new();
        assert_eq!(
            animator.set_state(PetAnimationState::MoveLeft),
            PetFrame {
                state: PetAnimationState::MoveLeft,
                index: 0,
            }
        );
        let frame = animator.advance(u32::MAX);
        assert_eq!(frame.state, PetAnimationState::MoveLeft);
        assert!(frame.index < PetAnimationState::MoveLeft.frame_count());
    }
}
