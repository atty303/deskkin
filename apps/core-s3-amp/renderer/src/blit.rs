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
        let started = unsafe { deskkin_blit_cycles() };
        let kind = usize::from(alpha.is_some());
        if let Some(alpha) = alpha {
            ScalarBlitter.blit(dst, src, Some(alpha), wire);
        } else {
            let prefix = dst.as_ptr().align_offset(16).min(dst.len());
            ScalarBlitter.blit(&mut dst[..prefix], &src[..prefix], None, wire);
            let bulk = (dst.len() - prefix) / 8 * 8;
            let mut scratch = Aligned([0u16; 32]);
            for offset in (prefix..prefix + bulk).step_by(32) {
                let len = (prefix + bulk - offset).min(32);
                let source = &src[offset..offset + len];
                let ptr = if source.as_ptr().align_offset(16) == 0 {
                    source.as_ptr()
                } else {
                    scratch.0[..len].copy_from_slice(source);
                    scratch.0.as_ptr()
                };
                unsafe {
                    deskkin_copy_vectors(dst[offset..].as_mut_ptr(), ptr, len / 8, u32::from(wire));
                }
            }
            ScalarBlitter.blit(&mut dst[prefix + bulk..], &src[prefix + bulk..], None, wire);
        }
        self.cycles[kind] =
            self.cycles[kind].wrapping_add(unsafe { deskkin_blit_cycles() }.wrapping_sub(started));
        self.pixels[kind] += dst.len() as u32;
    }
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
                for (wire, has_alpha) in
                    [(false, false), (true, false), (false, true), (true, true)]
                {
                    dst.0.fill(0xa55a);
                    let start = 8 + destination_offset;
                    PieBlitter::default().blit(
                        &mut dst.0[start..start + len],
                        &src.0[source_offset..source_offset + len],
                        has_alpha.then_some(&alpha[source_offset..source_offset + len]),
                        wire,
                    );
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
