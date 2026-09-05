const OVERLAY_WIDTH: usize = 56;
const OVERLAY_HEIGHT: usize = 14;
const GLYPH_SCALE: usize = 2;
const GLYPH_ADVANCE: usize = 8;
const LABEL_X: usize = 2;
const DIGITS_X: usize = 30;
const GLYPH_Y: usize = 2;
const UPDATE_INTERVAL_US: u64 = 500_000;
const BACKGROUND: u16 = 0x0000_u16.to_be();
const FOREGROUND: u16 = 0xffff_u16.to_be();

#[derive(Clone, Copy, Default)]
pub(super) struct DebugOverlay {
    period_started_us: Option<u64>,
    frame_intervals: u32,
    fps: u16,
}

impl DebugOverlay {
    pub(super) fn begin_frame(&mut self, now_us: u64) {
        let Some(started_us) = self.period_started_us else {
            self.period_started_us = Some(now_us);
            return;
        };
        self.frame_intervals = self.frame_intervals.saturating_add(1);
        let elapsed_us = now_us.saturating_sub(started_us);
        if elapsed_us < UPDATE_INTERVAL_US {
            return;
        }
        let rounded =
            (u64::from(self.frame_intervals) * 1_000_000 + elapsed_us / 2) / elapsed_us.max(1);
        self.fps = u16::try_from(rounded.min(999)).unwrap_or(999);
        self.period_started_us = Some(now_us);
        self.frame_intervals = 0;
    }

    pub(super) fn draw_band(self, pixels: &mut [u16], stride: usize, y: usize, rows: usize) {
        if y >= OVERLAY_HEIGHT || rows == 0 || stride < OVERLAY_WIDTH {
            return;
        }
        let visible_rows = rows.min(OVERLAY_HEIGHT - y);
        if pixels.len() < stride.saturating_mul(visible_rows) {
            return;
        }
        for row in 0..visible_rows {
            pixels[row * stride..row * stride + OVERLAY_WIDTH].fill(BACKGROUND);
        }
        self.draw_glyph(pixels, stride, y, rows, LABEL_X, GLYPH_F);
        self.draw_glyph(pixels, stride, y, rows, LABEL_X + GLYPH_ADVANCE, GLYPH_P);
        self.draw_glyph(
            pixels,
            stride,
            y,
            rows,
            LABEL_X + GLYPH_ADVANCE * 2,
            GLYPH_S,
        );

        let fps = self.fps.min(999);
        let hundreds = fps / 100;
        let tens = fps / 10 % 10;
        let ones = fps % 10;
        let mut x = DIGITS_X;
        if hundreds != 0 {
            self.draw_glyph(pixels, stride, y, rows, x, DIGITS[usize::from(hundreds)]);
            x += GLYPH_ADVANCE;
        }
        if hundreds != 0 || tens != 0 {
            self.draw_glyph(pixels, stride, y, rows, x, DIGITS[usize::from(tens)]);
            x += GLYPH_ADVANCE;
        }
        self.draw_glyph(pixels, stride, y, rows, x, DIGITS[usize::from(ones)]);
    }

    fn draw_glyph(
        self,
        pixels: &mut [u16],
        stride: usize,
        band_y: usize,
        band_rows: usize,
        x: usize,
        glyph: [u8; 5],
    ) {
        for (glyph_y, bits) in glyph.into_iter().enumerate() {
            for scale_y in 0..GLYPH_SCALE {
                let screen_y = GLYPH_Y + glyph_y * GLYPH_SCALE + scale_y;
                if screen_y < band_y || screen_y >= band_y + band_rows {
                    continue;
                }
                let row = (screen_y - band_y) * stride;
                for column in 0..3 {
                    if bits & (1 << (2 - column)) == 0 {
                        continue;
                    }
                    let start = row + x + column * GLYPH_SCALE;
                    pixels[start..start + GLYPH_SCALE].fill(FOREGROUND);
                }
            }
        }
    }
}

const GLYPH_F: [u8; 5] = [0b111, 0b100, 0b110, 0b100, 0b100];
const GLYPH_P: [u8; 5] = [0b110, 0b101, 0b110, 0b100, 0b100];
const GLYPH_S: [u8; 5] = [0b111, 0b100, 0b111, 0b001, 0b111];
const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b110, 0b001, 0b010, 0b100, 0b111],
    [0b110, 0b001, 0b010, 0b001, 0b110],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b110, 0b001, 0b110],
    [0b011, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b010, 0b010, 0b010],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b110],
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fps_uses_elapsed_frame_intervals_and_rounds() {
        let mut overlay = DebugOverlay::default();
        overlay.begin_frame(10_000);
        for frame in 1..=13 {
            overlay.begin_frame(10_000 + frame * 20_000);
        }
        assert_eq!(overlay.fps, 0);
        for frame in 14..=25 {
            overlay.begin_frame(10_000 + frame * 20_000);
        }
        assert_eq!(overlay.fps, 50);
    }

    #[test]
    fn overlay_is_opaque_clipped_and_preserves_guards() {
        const STRIDE: usize = 64;
        const ROWS: usize = 8;
        const GUARD: usize = 7;
        let mut storage = [0x1234_u16; GUARD + STRIDE * ROWS + GUARD];
        let overlay = DebugOverlay {
            fps: 26,
            ..DebugOverlay::default()
        };
        overlay.draw_band(&mut storage[GUARD..GUARD + STRIDE * ROWS], STRIDE, 0, ROWS);
        assert!(storage[..GUARD].iter().all(|value| *value == 0x1234));
        assert!(
            storage[GUARD + STRIDE * ROWS..]
                .iter()
                .all(|value| *value == 0x1234)
        );
        assert!(
            storage[GUARD..GUARD + STRIDE * ROWS]
                .iter()
                .all(|value| matches!(*value, BACKGROUND | FOREGROUND | 0x1234))
        );
        assert!(
            storage[GUARD..GUARD + OVERLAY_WIDTH]
                .iter()
                .all(|value| *value == BACKGROUND)
        );
    }
}
