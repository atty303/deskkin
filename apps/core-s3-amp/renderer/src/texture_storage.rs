// SPDX-License-Identifier: GPL-3.0-only

extern crate alloc;

use alloc::vec::Vec;
use core::{
    mem::size_of,
    ops::{Deref, DerefMut},
};

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

impl<T, const N: usize> DerefMut for Plane<T, N> {
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.0[self.1..]
    }
}

pub type Colors = Plane<u16, 8>;
pub type Alpha = Plane<u8, 16>;

pub struct PreparedCoverage {
    size: deskkin_presentation::SourceSize,
    plane: CoveragePlane,
    opaque_blocks: Vec<u8>,
}

enum CoveragePlane {
    Opaque,
    Cutout(Vec<u8>),
    Alpha8(Alpha),
}

impl PreparedCoverage {
    pub fn new(size: deskkin_presentation::SourceSize, alpha: Alpha) -> Self {
        use deskkin_presentation::{Mask8, build_cutout_mask, build_opaque_mask};
        if alpha.iter().all(|&a| a == 255) {
            return Self {
                size,
                plane: CoveragePlane::Opaque,
                opaque_blocks: Vec::new(),
            };
        }
        let mut opaque_blocks = alloc::vec![0; Mask8::bytes_for(size)];
        build_opaque_mask(size, &alpha, &mut opaque_blocks).expect("generated coverage dimensions");
        let plane = if alpha.iter().all(|&a| a == 0 || a == 255) {
            let mut bits = alloc::vec![0; alpha.len().div_ceil(8)];
            build_cutout_mask(&alpha, &mut bits).expect("binary coverage");
            CoveragePlane::Cutout(bits)
        } else {
            CoveragePlane::Alpha8(alpha)
        };
        Self {
            size,
            plane,
            opaque_blocks,
        }
    }

    pub fn borrow(&self) -> deskkin_presentation::Coverage<'_> {
        use deskkin_presentation::{Coverage, Mask8};
        if matches!(self.plane, CoveragePlane::Opaque) {
            return Coverage::Opaque;
        }
        let opaque_blocks = Mask8::new(self.size, &self.opaque_blocks).expect("prepared coverage");
        match &self.plane {
            CoveragePlane::Opaque => Coverage::Opaque,
            CoveragePlane::Cutout(bits) => Coverage::Cutout {
                bits,
                opaque_blocks,
            },
            CoveragePlane::Alpha8(alpha) => Coverage::Alpha8 {
                alpha,
                opaque_blocks,
            },
        }
    }
}

pub fn row_stride(width: u16) -> u16 {
    width.checked_add(15).expect("texture width fits u16") & !15
}

pub struct MipChain(Vec<MipLevel>);

struct MipLevel {
    size: deskkin_presentation::SourceSize,
    colors: Colors,
    coverage: PreparedCoverage,
}

impl MipLevel {
    fn borrow(
        &self,
    ) -> (
        deskkin_presentation::Texture<'_>,
        deskkin_presentation::TextureRegion,
    ) {
        use deskkin_presentation::{SourceSize, Texture, TextureRegion};
        let stride = row_stride(self.size.width);
        (
            Texture {
                size: SourceSize {
                    width: stride,
                    height: self.size.height,
                },
                pixels: &self.colors,
                coverage: self.coverage.borrow(),
            },
            TextureRegion {
                source_x: 0,
                source_y: 0,
                width: self.size.width,
                height: self.size.height,
                stride,
            },
        )
    }
}

impl MipChain {
    pub fn new(
        base: deskkin_presentation::Texture<'_>,
        region: deskkin_presentation::TextureRegion,
    ) -> Self {
        use deskkin_presentation::{SourceSize, mipmap::downsample};
        let mut levels: Vec<MipLevel> = Vec::new();
        loop {
            let (texture, source) = levels.last().map_or((base, region), MipLevel::borrow);
            if source.width <= 1 && source.height <= 1 {
                break;
            }
            let width = source.width.div_ceil(2);
            let height = source.height.div_ceil(2);
            let stride = row_stride(width);
            let length = usize::from(stride) * usize::from(height);
            let mut colors = Colors::from_fn(length, |_| 0);
            let mut alpha = Alpha::from_fn(length, |_| 0);
            let size = downsample(texture, source, &mut colors, &mut alpha, stride)
                .expect("validated mip source");
            let storage_size = SourceSize {
                width: stride,
                height,
            };
            let coverage = if matches!(texture.coverage, deskkin_presentation::Coverage::Opaque) {
                PreparedCoverage {
                    size: storage_size,
                    plane: CoveragePlane::Opaque,
                    opaque_blocks: Vec::new(),
                }
            } else {
                PreparedCoverage::new(storage_size, alpha)
            };
            levels.push(MipLevel {
                size,
                colors,
                coverage,
            });
        }
        Self(levels)
    }

    pub fn select<'a>(
        &'a self,
        base: deskkin_presentation::Texture<'a>,
        region: deskkin_presentation::TextureRegion,
        rect: deskkin_presentation::ScreenRect,
    ) -> (
        deskkin_presentation::Texture<'a>,
        deskkin_presentation::TextureRegion,
    ) {
        let mut selected = (base, region);
        for level in &self.0 {
            if i32::from(level.size.width) < rect.width
                || i32::from(level.size.height) < rect.height
            {
                break;
            }
            selected = level.borrow();
        }
        selected
    }
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
