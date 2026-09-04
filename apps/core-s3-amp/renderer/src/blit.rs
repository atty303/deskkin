// SPDX-License-Identifier: GPL-3.0-only

use deskkin_presentation::{Blitter, ScalarBlitter};

#[repr(align(16))]
struct Aligned<T>(T);
extern "C" {
    fn deskkin_blit_cycles() -> u32;
    fn deskkin_copy_vectors(dst: *mut u16, src: *const u16, vectors: usize, wire: u32);
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
        let alpha = alpha.map(|a| &a[start..end]);
        let started = unsafe { deskkin_blit_cycles() };
        let kind = usize::from(alpha.is_some());
        if let Some(alpha) = alpha {
            ScalarBlitter.blit(dst, src, Some(alpha), wire);
        } else {
            let (prefix, bulk) = vector_span(
                dst.as_ptr() as usize,
                source.as_ptr() as usize,
                source.len(),
                start,
                dst.len(),
            );
            ScalarBlitter.blit(&mut dst[..prefix], &src[..prefix], None, wire);
            for offset in (prefix..prefix + bulk).step_by(32) {
                let len = (prefix + bulk - offset).min(32);
                unsafe {
                    deskkin_copy_vectors(
                        dst[offset..].as_mut_ptr(),
                        source.as_ptr().add(start + offset),
                        len / 8,
                        u32::from(wire),
                    );
                }
            }
            ScalarBlitter.blit(&mut dst[prefix + bulk..], &src[prefix + bulk..], None, wire);
        }
        self.cycles[kind] =
            self.cycles[kind].wrapping_add(unsafe { deskkin_blit_cycles() }.wrapping_sub(started));
        self.pixels[kind] += dst.len() as u32;
    }
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

#[inline(never)]
pub fn self_test() -> bool {
    let mut src = Aligned([0u16; 336]);
    let mut dst = Aligned([0u16; 352]);
    let alpha: [u8; 336] = core::array::from_fn(|i| i as u8);
    for (i, pixel) in src.0.iter_mut().enumerate() {
        *pixel = (i as u16).wrapping_mul(31337);
    }
    for source_offset in 0..8 {
        for destination_offset in 0..8 {
            for len in 0..=320 {
                for (wire, has_alpha, padded) in [
                    (false, false, false),
                    (true, false, false),
                    (false, true, false),
                    (true, true, false),
                    (false, false, true),
                    (true, false, true),
                    (false, true, true),
                    (true, true, true),
                ] {
                    dst.0.fill(0xa55a);
                    let start = 8 + destination_offset;
                    if padded {
                        PieBlitter::default().blit_from(
                            &mut dst.0[start..start + len],
                            &src.0,
                            source_offset,
                            has_alpha.then_some(&alpha),
                            wire,
                        );
                    } else {
                        PieBlitter::default().blit(
                            &mut dst.0[start..start + len],
                            &src.0[source_offset..source_offset + len],
                            has_alpha.then_some(&alpha[source_offset..source_offset + len]),
                            wire,
                        );
                    }
                    for (i, &pixel) in dst.0.iter().enumerate() {
                        let expected = if (start..start + len).contains(&i) {
                            let si = source_offset + i - start;
                            let c = src.0[si];
                            let c = if has_alpha {
                                let mut d = [0xa55a];
                                ScalarBlitter.blit(&mut d, &[c], Some(&[alpha[si]]), wire);
                                if wire { u16::from_be(d[0]) } else { d[0] }
                            } else {
                                c
                            };
                            if wire { c.to_be() } else { c }
                        } else {
                            0xa55a
                        };
                        if pixel != expected {
                            return false;
                        }
                    }
                }
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::vector_span;

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
