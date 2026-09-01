// SPDX-License-Identifier: GPL-3.0-only

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnershipError {
    InvalidBuffer,
    AlreadyRendering,
    TransferInFlight,
    NotRendering,
    NotInFlight,
}

pub struct BandOwnership<const N: usize> {
    rendering: Option<usize>,
    inflight: [bool; N],
}

impl<const N: usize> BandOwnership<N> {
    pub const fn new() -> Self {
        Self {
            rendering: None,
            inflight: [false; N],
        }
    }

    pub fn begin_render(&mut self, index: usize) -> Result<(), OwnershipError> {
        if index >= N {
            return Err(OwnershipError::InvalidBuffer);
        }
        if self.rendering.is_some() {
            return Err(OwnershipError::AlreadyRendering);
        }
        if self.inflight[index] {
            return Err(OwnershipError::TransferInFlight);
        }
        self.rendering = Some(index);
        Ok(())
    }

    pub fn submit(&mut self, index: usize) -> Result<(), OwnershipError> {
        if index >= N {
            return Err(OwnershipError::InvalidBuffer);
        }
        if self.rendering != Some(index) {
            return Err(OwnershipError::NotRendering);
        }
        self.rendering = None;
        self.inflight[index] = true;
        Ok(())
    }

    pub fn submission_failed(&mut self, index: usize) {
        if index < N {
            self.inflight[index] = false;
        }
    }

    pub fn complete(&mut self, index: usize) -> Result<(), OwnershipError> {
        if index >= N {
            return Err(OwnershipError::InvalidBuffer);
        }
        if !self.inflight[index] {
            return Err(OwnershipError::NotInFlight);
        }
        self.inflight[index] = false;
        Ok(())
    }

    pub fn is_inflight(&self, index: usize) -> bool {
        index < N && self.inflight[index]
    }
}

#[cfg(test)]
mod tests {
    use super::{BandOwnership, OwnershipError};

    #[test]
    fn transfer_must_complete_before_the_same_buffer_is_rendered_again() {
        let mut ownership = BandOwnership::<2>::new();
        ownership.begin_render(0).unwrap();
        ownership.submit(0).unwrap();
        assert_eq!(
            ownership.begin_render(0),
            Err(OwnershipError::TransferInFlight)
        );

        ownership.begin_render(1).unwrap();
        ownership.submit(1).unwrap();
        ownership.complete(0).unwrap();
        ownership.begin_render(0).unwrap();
    }

    #[test]
    fn render_and_transfer_ownership_cannot_overlap() {
        let mut ownership = BandOwnership::<2>::new();
        ownership.begin_render(0).unwrap();
        assert_eq!(
            ownership.begin_render(1),
            Err(OwnershipError::AlreadyRendering)
        );
        assert_eq!(ownership.complete(0), Err(OwnershipError::NotInFlight));
        ownership.submit(0).unwrap();
        assert_eq!(ownership.submit(0), Err(OwnershipError::NotRendering));
    }
}
