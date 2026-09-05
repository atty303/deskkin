use super::{
    AxisStepper, Background, Blitter, ColumnSample, Occlusion, RasterError, RasterPhase,
    RasterRows, SceneBillboard, SceneStats, TextureFilter, VIEWPORT_HEIGHT, VIEWPORT_WIDTH,
    occlusion::paint_background, raster_native,
};

/// A full-width screen band backed only by its own rows. Screen coordinates stay
/// absolute; writes are translated to local buffer rows. Padding is not written.
pub struct BandTarget<'a> {
    pub(super) pixels: &'a mut [u16],
    pub(super) stride: usize,
    pub(super) y: usize,
    pub(super) rows: usize,
}

impl<'a> BandTarget<'a> {
    pub fn new(
        pixels: &'a mut [u16],
        stride: usize,
        y: usize,
        rows: usize,
    ) -> Result<Self, RasterError> {
        if stride < VIEWPORT_WIDTH as usize
            || rows == 0
            || y >= VIEWPORT_HEIGHT as usize
            || rows > VIEWPORT_HEIGHT as usize - y
            || stride
                .checked_mul(rows)
                .is_none_or(|size| pixels.len() < size)
        {
            return Err(RasterError::InvalidFramebuffer);
        }
        Ok(Self {
            pixels,
            stride,
            y,
            rows,
        })
    }
}

/// Reusable caller-owned per-board metadata. Only `PreparedScene` reads it.
#[derive(Clone, Default)]
pub struct PreparedBoard {
    columns: core::ops::Range<usize>,
    left: usize,
    top: usize,
    bottom: usize,
    visible: bool,
    native: bool,
    masked: bool,
}

/// One immutable scene snapshot and its prepared sampler/coverage data. Scratch
/// is supplied by the caller; preparing or drawing never allocates. Draw each
/// screen row once for frame-wide statistics, in any non-overlapping partition.
pub struct PreparedScene<'a> {
    boards: &'a [SceneBillboard<'a>],
    prepared: &'a [PreparedBoard],
    columns: &'a [ColumnSample],
    occlusion: &'a Occlusion<'a>,
    stats: SceneStats,
}

impl<'a> PreparedScene<'a> {
    pub fn new(
        boards: &'a [SceneBillboard<'a>],
        occlusion: &'a mut Occlusion<'a>,
        prepared: &'a mut [PreparedBoard],
        columns: &'a mut [ColumnSample],
        observer: &mut impl FnMut(RasterPhase),
    ) -> Result<Self, RasterError> {
        if boards.len() > usize::from(u16::MAX)
            || boards.windows(2).any(|pair| {
                let (a, b) = (pair[0].projected, pair[1].projected);
                a.depth < b.depth || (a.depth == b.depth && a.id > b.id)
            })
        {
            return Err(RasterError::InvalidOrder);
        }
        if prepared.len() < boards.len() {
            return Err(RasterError::InsufficientScratch);
        }
        let mut stats = SceneStats::default();
        observer(RasterPhase::Coverage);
        occlusion.prepare(boards, &mut stats);
        observer(RasterPhase::Setup);
        let mut used = 0;
        for (index, (board, entry)) in boards.iter().zip(prepared.iter_mut()).enumerate() {
            let r = board.projected.screen_rect;
            let left = r.x.clamp(0, VIEWPORT_WIDTH) as usize;
            let right = r.x.saturating_add(r.width).clamp(0, VIEWPORT_WIDTH) as usize;
            let top = r.y.clamp(0, VIEWPORT_HEIGHT) as usize;
            let bottom = r.y.saturating_add(r.height).clamp(0, VIEWPORT_HEIGHT) as usize;
            let masked = stats.opaque_tiles != 0 && occlusion.hides(r, index);
            let visible = left < right
                && top < bottom
                && (!masked || occlusion.has_visible_span(top..bottom, left..right, index));
            let native = board.projected.filter == TextureFilter::Nearest
                && r.width == i32::from(board.region.width)
                && r.height == i32::from(board.region.height);
            let count = if visible && !native { right - left } else { 0 };
            if count > columns.len() - used {
                return Err(RasterError::InsufficientScratch);
            }
            *entry = PreparedBoard {
                columns: used..used + count,
                left,
                top,
                bottom,
                visible,
                native,
                masked,
            };
            if count != 0 {
                let mut axis =
                    AxisStepper::new(board.region.width, r.width, left as i64 - i64::from(r.x));
                for column in &mut columns[used..used + count] {
                    let coordinate = axis.take();
                    let first = (coordinate >> 16) as u16;
                    *column = ColumnSample {
                        first: board.region.source_x + first,
                        fraction: coordinate as u16,
                    };
                }
                stats.scaler_preparations += 1;
            }
            used += count;
        }
        observer(RasterPhase::Idle);
        Ok(Self {
            boards,
            prepared: &prepared[..boards.len()],
            columns: &columns[..used],
            occlusion,
            stats,
        })
    }

    pub fn raster_band(
        &mut self,
        mut target: BandTarget<'_>,
        backend: (&mut impl Background, &mut impl Blitter),
        wire: bool,
        observer: &mut impl FnMut(RasterPhase),
    ) {
        let (background, blitter) = backend;
        observer(RasterPhase::Background);
        paint_background(
            &mut target,
            self.occlusion,
            self.stats.opaque_tiles,
            background,
            wire,
        );
        for (index, (board, entry)) in self.boards.iter().zip(self.prepared).enumerate() {
            let top = entry.top.max(target.y);
            let bottom = entry.bottom.min(target.y + target.rows);
            if !entry.visible || top >= bottom {
                continue;
            }
            let mask = entry.masked.then_some((self.occlusion, index));
            observer(RasterPhase::Pixels);
            let samples = if entry.native {
                raster_native(
                    &mut target,
                    board.projected,
                    board.texture,
                    board.region,
                    wire,
                    (mask, blitter),
                )
            } else {
                RasterRows {
                    origin: target.y,
                    stride: target.stride,
                    left: entry.left,
                    top,
                    bottom,
                    columns: &self.columns[entry.columns.clone()],
                    y: AxisStepper::new(
                        board.region.height,
                        board.projected.screen_rect.height,
                        top as i64 - i64::from(board.projected.screen_rect.y),
                    ),
                    region: board.region,
                }
                .dispatch_visible(
                    target.pixels,
                    board.texture,
                    board.projected.filter,
                    wire,
                    (mask, blitter),
                )
            };
            match board.projected.filter {
                TextureFilter::Nearest => self.stats.raster.nearest_samples += samples,
                TextureFilter::Bilinear => self.stats.raster.bilinear_samples += samples,
            }
        }
        observer(RasterPhase::Idle);
    }

    #[must_use]
    pub const fn stats(&self) -> SceneStats {
        self.stats
    }
}
