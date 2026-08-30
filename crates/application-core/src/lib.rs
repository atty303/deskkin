#![no_std]
#![forbid(unsafe_code)]

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Lifecycle {
    Start,
    Stop,
    SessionInvalidated,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct LocalEffectId(u64);

impl LocalEffectId {
    #[must_use]
    pub const fn new(value: u64) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SurfaceClass {
    Ambient,
    Information,
}

impl SurfaceClass {
    #[must_use]
    pub const fn precedence(self) -> u8 {
        match self {
            Self::Ambient => 0,
            Self::Information => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_effect_identity_is_nonzero() {
        assert_eq!(LocalEffectId::new(0), None);
        assert_eq!(LocalEffectId::new(1).unwrap().get(), 1);
    }

    #[test]
    fn information_precedes_ambient() {
        assert!(SurfaceClass::Information.precedence() > SurfaceClass::Ambient.precedence());
    }
}
