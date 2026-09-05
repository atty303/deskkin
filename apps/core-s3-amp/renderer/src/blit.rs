// SPDX-License-Identifier: GPL-3.0-only

use deskkin_presentation::{Blitter, ScalarBlitter};

extern "C" {
    static deskkin_alpha_pie_masks: [u16; 32];
    fn deskkin_alpha_pie(
        dst: *mut u16,
        src: *const u16,
        alpha: *const u8,
        vectors: usize,
        wire: u32,
        masks: *const u16,
    );
    fn deskkin_copy_pie(dst: *mut u16, src: *const u16, vectors: usize, wire: u32);
}

#[inline(always)]
pub fn cycles() -> u32 {
    #[cfg(target_arch = "xtensa")]
    {
        let cycles;
        // Keep the default memory effects so measured loads/stores stay between reads.
        unsafe {
            core::arch::asm!("rsr.ccount {cycles}", cycles = out(reg) cycles, options(nostack));
        }
        cycles
    }
    #[cfg(not(target_arch = "xtensa"))]
    unreachable!("CoreS3 cycle counter requires Xtensa")
}

#[derive(Default)]
pub struct PieBlitter {
    cycles: [u32; 2],
    pixels: [u32; 2],
}
impl PieBlitter {
    pub fn profile(&self) -> [u32; 4] {
        [
            self.cycles[0] / 240,
            self.cycles[1] / 240,
            self.pixels[0],
            self.pixels[1],
        ]
    }
}
impl Blitter for PieBlitter {
    fn blit(&mut self, dst: &mut [u16], src: &[u16], alpha: Option<&[u8]>, wire: bool) {
        assert_eq!(dst.len(), src.len());
        assert!(alpha.is_none_or(|a| a.len() == src.len()));
        self.blit_from(dst, src, 0, alpha, wire);
    }

    #[inline(always)]
    fn blit_from(
        &mut self,
        dst: &mut [u16],
        source: &[u16],
        start: usize,
        alpha: Option<&[u8]>,
        wire: bool,
    ) {
        let end = start.checked_add(dst.len()).expect("blit range");
        let src = &source[start..end];
        let alpha_backing = alpha;
        let alpha = alpha.map(|a| &a[start..end]);
        let started = cycles();
        let kind = usize::from(alpha.is_some());
        if let Some(alpha) = alpha {
            let backing = alpha_backing.unwrap();
            let (prefix, bulk) = alpha_span(
                dst.as_ptr() as usize,
                source.as_ptr() as usize,
                source.len(),
                backing.as_ptr() as usize,
                backing.len(),
                start,
                dst.len(),
            );
            if prefix != 0 {
                alpha_scalar(&mut dst[..prefix], &src[..prefix], &alpha[..prefix], wire);
            }
            if bulk != 0 {
                unsafe {
                    deskkin_alpha_pie(
                        dst[prefix..].as_mut_ptr(),
                        source.as_ptr().add(start + prefix),
                        backing.as_ptr().add(start + prefix),
                        bulk / 8,
                        u32::from(wire),
                        core::ptr::addr_of!(deskkin_alpha_pie_masks).cast(),
                    );
                }
            }
            if prefix + bulk < dst.len() {
                alpha_scalar(
                    &mut dst[prefix + bulk..],
                    &src[prefix + bulk..],
                    &alpha[prefix + bulk..],
                    wire,
                );
            }
        } else {
            let (prefix, bulk) = vector_span(
                dst.as_ptr() as usize,
                source.as_ptr() as usize,
                source.len(),
                start,
                dst.len(),
            );
            if prefix != 0 {
                ScalarBlitter.blit(&mut dst[..prefix], &src[..prefix], None, wire);
            }
            if bulk != 0 {
                unsafe {
                    deskkin_copy_pie(
                        dst[prefix..].as_mut_ptr(),
                        source.as_ptr().add(start + prefix),
                        bulk / 8,
                        u32::from(wire),
                    );
                }
            }
            if prefix + bulk < dst.len() {
                ScalarBlitter.blit(&mut dst[prefix + bulk..], &src[prefix + bulk..], None, wire);
            }
        }
        self.cycles[kind] = self.cycles[kind].wrapping_add(cycles().wrapping_sub(started));
        self.pixels[kind] += dst.len() as u32;
    }
}

fn alpha_pixel(dst: u16, src: u16, alpha: u8) -> u16 {
    let weight = i32::from(alpha) + i32::from(alpha >> 7);
    let channel = |shift: u32, mask: u16| {
        let d = i32::from((dst >> shift) & mask);
        let s = i32::from((src >> shift) & mask);
        (d + (((s - d) * weight) >> 8)) as u16
    };
    (channel(11, 31) << 11) | (channel(5, 63) << 5) | channel(0, 31)
}

fn alpha_scalar(dst: &mut [u16], src: &[u16], alpha: &[u8], wire: bool) {
    for ((d, &s), &a) in dst.iter_mut().zip(src).zip(alpha) {
        if a == 0 {
            continue;
        }
        let color = if a == 255 {
            s
        } else {
            alpha_pixel(if wire { u16::from_be(*d) } else { *d }, s, a)
        };
        *d = if wire { color.to_be() } else { color };
    }
}

fn alpha_span(
    dst: usize,
    source: usize,
    source_len: usize,
    alpha: usize,
    alpha_len: usize,
    start: usize,
    len: usize,
) -> (usize, usize) {
    // With aligned, padded backing ends, every enclosing vector for an
    // already-validated logical span is readable, including clipped starts.
    if (source | source_len * 2 | alpha | alpha_len) & 15 == 0 {
        let prefix = (((16 - (dst & 15)) & 15) / 2).min(len);
        return (prefix, (len - prefix) / 8 * 8);
    }
    let (mut prefix, _) = vector_span(dst, source, source_len, start, len);
    while prefix < len && ((alpha + start + prefix) & !15) < alpha {
        prefix = (prefix + 8).min(len);
    }
    let mut bulk = (len - prefix) / 8 * 8;
    while bulk > 0
        && ((source + 2 * (start + prefix + bulk)).div_ceil(16) * 16 > source + source_len * 2
            || (alpha + start + prefix + bulk).div_ceil(16) * 16 > alpha + alpha_len)
    {
        bulk -= 8;
    }
    (prefix, bulk)
}

fn vector_span(
    dst: usize,
    source: usize,
    source_len: usize,
    start: usize,
    len: usize,
) -> (usize, usize) {
    let mut prefix = ((16 - (dst & 15)) & 15) / 2;
    prefix = prefix.min(len);
    if (source | source_len * 2) & 15 == 0 {
        return (prefix, (len - prefix) / 8 * 8);
    }
    let phase = ((source + (start + prefix) * 2) & 15) / 2;
    // Funnel loads include both enclosing vectors, bounded by the backing
    // allocation rather than the logical image or clip boundary.
    if phase > start + prefix {
        prefix = (prefix + 8).min(len);
    }
    let extra = if phase == 0 { 0 } else { 8 - phase };
    let available = source_len.saturating_sub(start + prefix + extra);
    (prefix, (len - prefix).min(available) / 8 * 8)
}

#[cfg(test)]
mod tests {
    use super::{alpha_pixel, alpha_span, vector_span};

    #[test]
    fn alpha_endpoints_and_component_bounds() {
        for d in 0..64u16 {
            for s in 0..64u16 {
                for a in 0..=255u8 {
                    let pixel = alpha_pixel(d << 5, s << 5, a);
                    assert_eq!(pixel & !0x07e0, 0);
                    assert!((d.min(s)..=d.max(s)).contains(&(pixel >> 5)));
                    if a == 0 {
                        assert_eq!(pixel, d << 5);
                    }
                    if a == 255 {
                        assert_eq!(pixel, s << 5);
                    }
                }
            }
        }
    }

    #[test]
    fn alpha_vector_reads_stay_in_independent_backings() {
        for sp in (0..16).step_by(2) {
            for dp in (0..16).step_by(2) {
                for ap in 0..16 {
                    for len in 0..=320 {
                        for before in [0, 1, 7, 15] {
                            for after in [0, 1, 7, 15] {
                                let size = before + len + after;
                                let (prefix, bulk) =
                                    alpha_span(64 + dp, 64 + sp, size, 64 + ap, size, before, len);
                                assert!(prefix + bulk <= len);
                                assert_eq!(bulk % 8, 0);
                                if bulk > 0 {
                                    assert_eq!((64 + dp + prefix * 2) % 16, 0);
                                    for (base, scale) in [(64 + sp, 2), (64 + ap, 1)] {
                                        let first = base + (before + prefix) * scale;
                                        let end = first + bulk * scale;
                                        assert!(first & !15 >= base);
                                        assert!(end.div_ceil(16) * 16 <= base + size * scale);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn vector_reads_and_writes_stay_in_backing_slices() {
        for src_phase in (0..16).step_by(2) {
            for dst_phase in (0..16).step_by(2) {
                for len in 0..=320 {
                    for before in 0..16 {
                        for after in 0..16 {
                            let base = 64 + src_phase;
                            let (prefix, bulk) = vector_span(
                                64 + dst_phase,
                                base,
                                before + len + after,
                                before,
                                len,
                            );
                            assert!(prefix + bulk <= len);
                            assert_eq!(bulk % 8, 0);
                            if bulk > 0 {
                                assert_eq!((64 + dst_phase + prefix * 2) % 16, 0);
                                let first = base + (before + prefix) * 2;
                                let end = first + bulk * 2;
                                assert!(first & !15 >= base);
                                assert!(end.div_ceil(16) * 16 <= base + (before + len + after) * 2);
                            }
                        }
                    }
                }
            }
        }
    }
}
