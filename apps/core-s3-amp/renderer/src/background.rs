// SPDX-License-Identifier: GPL-3.0-only

use deskkin_presentation::{Background, demo_world};

pub struct PieBackground;

#[repr(align(16))]
struct Aligned<T>(T);

extern "C" {
    fn deskkin_background_pie(destination: *mut u16, pattern: *const u16, vectors: usize);
}

impl Background for PieBackground {
    fn row(&mut self, y: usize) -> [u16; 4] {
        demo_world::background_row(y, demo_world::HORIZON)
    }

    fn fill(&mut self, pixels: &mut [u16], colors: [u16; 4]) {
        let prefix = pixels.as_ptr().align_offset(16).min(pixels.len());
        let (head, rest) = pixels.split_at_mut(prefix);
        for (index, pixel) in head.iter_mut().enumerate() {
            *pixel = colors[index % 4];
        }
        let pattern = Aligned(core::array::from_fn::<_, 8, _>(|i| {
            colors[(prefix + i) % 4]
        }));
        let bulk = rest.len() / 8 * 8;
        // Bound each IRQ-locked call to one screen row, including for callers
        // outside the scene renderer. Both pointers are aligned and in bounds.
        for span in rest[..bulk].chunks_mut(320) {
            unsafe {
                deskkin_background_pie(span.as_mut_ptr(), pattern.0.as_ptr(), span.len() / 8)
            };
        }
        for (index, pixel) in rest[bulk..].iter_mut().enumerate() {
            *pixel = colors[(prefix + index) % 4];
        }
    }
}

// Exercise the actual assembly before presenting frames. Guard words detect
// over-stores; every halfword alignment, length and dither phase is checked.
#[inline(never)]
pub fn self_test() -> bool {
    let mut buffer = Aligned([0_u16; 344]);
    for offset in 0..8 {
        for len in 0..=320 {
            buffer.0.fill(0xa55a);
            let colors =
                core::array::from_fn(|i| [0x1234, 0xabcd, 0x55aa, 0xf00d][(offset + i) % 4]);
            let start = 8 + offset;
            PieBackground.fill(&mut buffer.0[start..start + len], colors);
            for (index, &pixel) in buffer.0.iter().enumerate() {
                let expected = if (start..start + len).contains(&index) {
                    colors[(index - start) % 4]
                } else {
                    0xa55a
                };
                if pixel != expected {
                    return false;
                }
            }
        }
    }
    true
}
