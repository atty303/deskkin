use crate::{ColumnSample, Coverage, Texture, blend_rgb565, interpolate_rgb565};

/// Source colors are native RGB565; destination words use the requested order.
/// Slices have equal lengths. Implementations must not access outside them.
pub trait Blitter {
    fn cutout(
        &mut self,
        destination: &mut [u16],
        source: &[u16],
        offset: usize,
        bits: &[u8],
        wire: bool,
    ) {
        let end = offset.checked_add(destination.len()).expect("cutout range");
        assert!(end.div_ceil(8) <= bits.len());
        for (index, (dst, &src)) in destination.iter_mut().zip(&source[offset..end]).enumerate() {
            let bit = offset + index;
            if bits[bit / 8] & (1 << (bit % 8)) != 0 {
                *dst = if wire { src.to_be() } else { src };
            }
        }
    }
    /// Samples a validated span directly into its destination without a pixel row copy.
    fn sample(&mut self, destination: &mut [u16], source: SampledSpan<'_>, wire: bool) {
        source.draw(destination, wire, blend_rgb565);
    }

    fn blit(&mut self, destination: &mut [u16], source: &[u16], alpha: Option<&[u8]>, wire: bool);

    /// Draw `destination.len()` pixels starting at `offset`. The entire source
    /// and optional alpha backing slices are readable, including padding;
    /// only the destination slice is writable. The selected range must exist.
    fn blit_from(
        &mut self,
        destination: &mut [u16],
        source: &[u16],
        offset: usize,
        alpha: Option<&[u8]>,
        wire: bool,
    ) {
        let end = offset.checked_add(destination.len()).expect("blit range");
        self.blit(
            destination,
            &source[offset..end],
            alpha.map(|a| &a[offset..end]),
            wire,
        );
    }
}

pub struct ScalarBlitter;

impl Blitter for ScalarBlitter {
    fn blit(&mut self, destination: &mut [u16], source: &[u16], alpha: Option<&[u8]>, wire: bool) {
        assert_eq!(destination.len(), source.len());
        assert!(alpha.is_none_or(|a| a.len() == source.len()));
        for (index, (dst, &src)) in destination.iter_mut().zip(source).enumerate() {
            let a = alpha.map_or(255, |a| a[index]);
            if a == 0 {
                continue;
            }
            let color = if a == 255 {
                src
            } else {
                blend_rgb565(if wire { u16::from_be(*dst) } else { *dst }, src, a)
            };
            *dst = if wire { color.to_be() } else { color };
        }
    }
}

/// A validated source row mapping. Construction is confined to the portable
/// rasterizer after texture, clipping and coordinate validation.
#[derive(Clone, Copy)]
pub struct SampledSpan<'a> {
    pub(crate) texture: Texture<'a>,
    pub(crate) columns: &'a [ColumnSample],
    pub(crate) rows: [usize; 2],
    pub(crate) last_column: u16,
    pub(crate) fraction_y: u16,
    pub(crate) bilinear: bool,
}

pub struct NearestSpan<'a> {
    pub colors: &'a [u16],
    pub alpha: Option<&'a [u8]>,
    pub columns: &'a [ColumnSample],
}

impl<'a> SampledSpan<'a> {
    #[must_use]
    pub fn slice(self, range: core::ops::Range<usize>) -> Self {
        Self {
            columns: &self.columns[range],
            ..self
        }
    }

    #[must_use]
    pub fn has_alpha(self) -> bool {
        self.texture.coverage.is_alpha()
    }

    /// Native source words, optional A8, and validated four-byte column entries.
    /// All column indices are in range of both row slices.
    #[must_use]
    pub fn nearest(self) -> Option<NearestSpan<'a>> {
        if self.bilinear || matches!(self.texture.coverage, Coverage::Cutout { .. }) {
            return None;
        }
        let start = self.rows[0];
        let end = start + usize::from(self.last_column) + 1;
        Some(NearestSpan {
            colors: &self.texture.pixels[start..end],
            alpha: self
                .has_alpha()
                .then(|| &self.texture.coverage.alpha()[start..end]),
            columns: self.columns,
        })
    }

    /// Uses the caller's component arithmetic for both vector tails and scalar paths.
    ///
    /// # Panics
    /// Panics if the destination length differs from this span's column count.
    pub fn draw(self, destination: &mut [u16], wire: bool, blend: impl Fn(u16, u16, u8) -> u16) {
        assert_eq!(destination.len(), self.columns.len());
        match (self.bilinear, self.has_alpha()) {
            (false, false) => self.draw_kind::<false, false>(destination, wire, blend),
            (false, true) => self.draw_kind::<false, true>(destination, wire, blend),
            (true, false) => self.draw_kind::<true, false>(destination, wire, blend),
            (true, true) => self.draw_filtered_alpha(destination, wire),
        }
    }

    fn draw_filtered_alpha(self, destination: &mut [u16], wire: bool) {
        for (dst, column) in destination.iter_mut().zip(self.columns) {
            let x = usize::from(column.first);
            let next = usize::from((column.first + 1).min(self.last_column));
            let indices = [
                self.rows[0] + x,
                self.rows[0] + next,
                self.rows[1] + x,
                self.rows[1] + next,
            ];
            let alpha = indices.map(|i| u32::from(self.texture.coverage.at(i)));
            if alpha == [0; 4] {
                continue;
            }
            let interpolate = |v: [u32; 4]| {
                let lerp = |a: u32, b: u32, f: u16| {
                    (a * (65_536 - u32::from(f)) + b * u32::from(f) + 32_768) >> 16
                };
                lerp(
                    lerp(v[0], v[1], column.fraction),
                    lerp(v[2], v[3], column.fraction),
                    self.fraction_y,
                )
            };
            let coverage = interpolate(alpha);
            let background = if wire { u16::from_be(*dst) } else { *dst };
            let mut color = 0;
            for (shift, mask) in [(11, 31u16), (5, 63), (0, 31)] {
                let premultiplied = core::array::from_fn(|i| {
                    u32::from((self.texture.pixels[indices[i]] >> shift) & mask) * alpha[i]
                });
                let component = ((interpolate(premultiplied)
                    + u32::from((background >> shift) & mask) * (255 - coverage)
                    + 127)
                    / 255)
                    .min(u32::from(mask));
                color |= (component as u16) << shift;
            }
            *dst = if wire { color.to_be() } else { color };
        }
    }

    fn draw_kind<const BILINEAR: bool, const ALPHA: bool>(
        self,
        destination: &mut [u16],
        wire: bool,
        blend: impl Fn(u16, u16, u8) -> u16,
    ) {
        for (dst, column) in destination.iter_mut().zip(self.columns) {
            let x = usize::from(column.first);
            let index = self.rows[0] + x;
            let alpha = if ALPHA {
                self.texture.coverage.at(index)
            } else {
                255
            };
            if alpha == 0 {
                continue;
            }
            let color = if BILINEAR {
                let next = usize::from((column.first + 1).min(self.last_column));
                let top = interpolate_rgb565(
                    self.texture.pixels[index],
                    self.texture.pixels[self.rows[0] + next],
                    u32::from(column.fraction),
                );
                let bottom = interpolate_rgb565(
                    self.texture.pixels[self.rows[1] + x],
                    self.texture.pixels[self.rows[1] + next],
                    u32::from(column.fraction),
                );
                interpolate_rgb565(top, bottom, u32::from(self.fraction_y))
            } else {
                self.texture.pixels[index]
            };
            let color = if alpha == 255 {
                color
            } else {
                blend(if wire { u16::from_be(*dst) } else { *dst }, color, alpha)
            };
            *dst = if wire { color.to_be() } else { color };
        }
    }
}
