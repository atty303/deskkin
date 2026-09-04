// SPDX-License-Identifier: GPL-3.0-only

extern crate alloc;

use alloc::vec::Vec;
use core::{mem::size_of, ops::Deref};

/// Initialized texture storage whose exposed slice starts at a 16-byte boundary.
/// The platform's Rust allocator accepts at most 8-byte layout alignment, so
/// reserve at most 15 spare bytes and generate pixels directly at an aligned
/// offset within an ordinary allocation. No populated pixel buffer is copied.
pub struct Plane<T, const N: usize>(Vec<T>, usize);

impl<T: Default, const N: usize> Plane<T, N> {
    pub fn from_fn(len: usize, pixel: impl FnMut(usize) -> T) -> Self {
        assert_eq!(N * size_of::<T>(), 16);
        assert!(N != 0 && len % N == 0);
        let mut storage: Vec<T> =
            Vec::with_capacity(len.checked_add(N - 1).expect("texture capacity"));
        let start = storage.as_ptr().align_offset(16);
        assert!(start < N);
        storage.resize_with(start, T::default);
        storage.extend((0..len).map(pixel));
        Self(storage, start)
    }
}

impl<T, const N: usize> Deref for Plane<T, N> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.0[self.1..]
    }
}

pub type Colors = Plane<u16, 8>;
pub type Alpha = Plane<u8, 16>;

pub fn row_stride(width: u16) -> u16 {
    width.checked_add(15).expect("texture width fits u16") & !15
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligned_rows_preserve_texels_and_zero_padding() {
        for width in 1..=320 {
            let stride = usize::from(row_stride(width));
            let colors = Colors::from_fn(stride * 7, |i| {
                if i % stride < usize::from(width) {
                    i as u16
                } else {
                    0
                }
            });
            let alpha = Alpha::from_fn(stride * 7, |i| {
                if i % stride < usize::from(width) {
                    i as u8
                } else {
                    0
                }
            });
            for y in 0..7 {
                assert_eq!((&colors[y * stride] as *const u16 as usize) % 16, 0);
                assert_eq!((&alpha[y * stride] as *const u8 as usize) % 16, 0);
                for x in 0..stride {
                    let i = y * stride + x;
                    assert_eq!(colors[i], if x < usize::from(width) { i as u16 } else { 0 });
                    assert_eq!(alpha[i], if x < usize::from(width) { i as u8 } else { 0 });
                }
            }
        }
    }
}
