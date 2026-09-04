use super::{
    Blitter, ProjectedBillboard, RasterError, RasterStats, ScalarBlitter, ScreenRect, SourceSize,
    Texture, TextureFilter, TextureRegion, VIEWPORT_HEIGHT, VIEWPORT_WIDTH,
    raster_billboard_masked, validate_texture,
};

const TILE: usize = 8;

#[cfg(test)]
#[path = "occlusion_tests.rs"]
mod tests;

/// One bit per 8x8 source block, packed row-major, least significant bit first.
/// A set bit guarantees alpha=255 for every valid texel, including edge blocks.
#[derive(Clone, Copy, Debug)]
pub struct Mask8<'a> {
    size: SourceSize,
    bits: &'a [u8],
}

impl<'a> Mask8<'a> {
    #[must_use]
    pub const fn bytes_for(size: SourceSize) -> usize {
        ((size.width as usize).div_ceil(TILE) * (size.height as usize).div_ceil(TILE)).div_ceil(8)
    }

    /// `bits` must have been built from the unchanged alpha plane for `size`.
    pub fn new(size: SourceSize, bits: &'a [u8]) -> Result<Self, RasterError> {
        if size.width == 0 || size.height == 0 || bits.len() != Self::bytes_for(size) {
            return Err(RasterError::InvalidMask);
        }
        Ok(Self { size, bits })
    }

    fn covers(self, left: usize, top: usize, right: usize, bottom: usize) -> bool {
        let stride = usize::from(self.size.width).div_ceil(TILE);
        for y in top / TILE..=bottom / TILE {
            for x in left / TILE..=right / TILE {
                let bit = y * stride + x;
                if self.bits[bit / 8] & (1 << (bit % 8)) == 0 {
                    return false;
                }
            }
        }
        true
    }
}

/// Validates sizes before writing; unused final bits are zero. No allocations.
pub fn build_opaque_mask(
    size: SourceSize,
    alpha: &[u8],
    bits: &mut [u8],
) -> Result<(), RasterError> {
    if size.width == 0
        || size.height == 0
        || alpha.len() != usize::from(size.width) * usize::from(size.height)
        || bits.len() != Mask8::bytes_for(size)
    {
        return Err(RasterError::InvalidMask);
    }
    bits.fill(0);
    let width = usize::from(size.width);
    let height = usize::from(size.height);
    for by in 0..height.div_ceil(TILE) {
        for bx in 0..width.div_ceil(TILE) {
            let opaque = (by * TILE..((by + 1) * TILE).min(height)).all(|y| {
                alpha[y * width + bx * TILE..y * width + ((bx + 1) * TILE).min(width)]
                    .iter()
                    .all(|&a| a == 255)
            });
            if opaque {
                let bit = by * width.div_ceil(TILE) + bx;
                bits[bit / 8] |= 1 << (bit % 8);
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub enum Coverage<'a> {
    Opaque,
    Alpha8 {
        alpha: &'a [u8],
        opaque_blocks: Mask8<'a>,
    },
}

impl<'a> Coverage<'a> {
    pub(super) const fn is_alpha(self) -> bool {
        matches!(self, Self::Alpha8 { .. })
    }
    pub(super) const fn alpha(self) -> &'a [u8] {
        match self {
            Self::Opaque => &[],
            Self::Alpha8 { alpha, .. } => alpha,
        }
    }
    pub(super) fn validate(self, size: SourceSize) -> Result<(), RasterError> {
        if let Self::Alpha8 { opaque_blocks, .. } = self
            && opaque_blocks.size != size
        {
            return Err(RasterError::InvalidMask);
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub struct SceneBillboard<'a> {
    pub(super) projected: ProjectedBillboard,
    pub(super) texture: Texture<'a>,
    pub(super) region: TextureRegion,
}

impl<'a> SceneBillboard<'a> {
    pub fn new(
        projected: ProjectedBillboard,
        texture: Texture<'a>,
        region: TextureRegion,
    ) -> Result<Self, RasterError> {
        validate_texture(texture, region)?;
        if projected.screen_rect.width <= 0 || projected.screen_rect.height <= 0 {
            return Err(RasterError::InvalidTexture);
        }
        Ok(Self {
            projected,
            texture,
            region,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ScreenTile {
    Eight,
    Sixteen,
}

impl ScreenTile {
    #[must_use]
    pub const fn pixels(self) -> usize {
        match self {
            Self::Eight => 8,
            Self::Sixteen => 16,
        }
    }
    #[must_use]
    pub const fn cells(self) -> usize {
        (VIEWPORT_WIDTH as usize / self.pixels()) * (VIEWPORT_HEIGHT as usize / self.pixels())
    }
}

/// Caller-owned reusable storage. Each u16 encodes painter index + 1; zero
/// means no occluder. At most 65,535 boards; scratch must match the tile size.
pub struct Occlusion<'a> {
    tile: ScreenTile,
    cutoffs: &'a mut [u16],
}

impl<'a> Occlusion<'a> {
    pub fn new(tile: ScreenTile, cutoffs: &'a mut [u16]) -> Result<Self, RasterError> {
        if cutoffs.len() != tile.cells() {
            return Err(RasterError::InvalidMask);
        }
        Ok(Self { tile, cutoffs })
    }

    fn columns(&self) -> usize {
        VIEWPORT_WIDTH as usize / self.tile.pixels()
    }

    pub(super) fn row_end(&self, y: usize, end: usize) -> usize {
        ((y / self.tile.pixels() + 1) * self.tile.pixels()).min(end)
    }

    pub(super) fn prepare(&mut self, boards: &[SceneBillboard<'_>], stats: &mut SceneStats) {
        self.cutoffs.fill(0);
        let tile = self.tile.pixels();
        let columns = self.columns();
        for (index, board) in boards.iter().enumerate().rev() {
            let mask = match board.texture.coverage {
                Coverage::Opaque => None,
                Coverage::Alpha8 { opaque_blocks, .. }
                    if board.projected.filter == TextureFilter::Nearest
                        && opaque_blocks.bits.iter().any(|&bits| bits != 0) =>
                {
                    Some(opaque_blocks)
                }
                Coverage::Alpha8 { .. } => continue,
            };
            let r = board.projected.screen_rect;
            let left = (r.x.clamp(0, VIEWPORT_WIDTH) as usize).div_ceil(tile);
            let top = (r.y.clamp(0, VIEWPORT_HEIGHT) as usize).div_ceil(tile);
            let right = r.x.saturating_add(r.width).clamp(0, VIEWPORT_WIDTH) as usize / tile;
            let bottom = r.y.saturating_add(r.height).clamp(0, VIEWPORT_HEIGHT) as usize / tile;
            if left >= right || top >= bottom {
                continue;
            }
            let mut horizontal = [(0, 0); VIEWPORT_WIDTH as usize / 8];
            if mask.is_some() {
                for (x, span) in horizontal.iter_mut().enumerate().take(right).skip(left) {
                    *span = source_span(
                        x * tile,
                        tile,
                        r.x,
                        r.width,
                        board.region.source_x,
                        board.region.width,
                    );
                }
            }
            for y in top..bottom {
                let vertical = if mask.is_some() {
                    source_span(
                        y * tile,
                        tile,
                        r.y,
                        r.height,
                        board.region.source_y,
                        board.region.height,
                    )
                } else {
                    (0, 0)
                };
                for (x, &(first, last)) in horizontal.iter().enumerate().take(right).skip(left) {
                    let cutoff = &mut self.cutoffs[y * columns + x];
                    if *cutoff != 0 {
                        continue;
                    }
                    stats.coverage_tests = stats.coverage_tests.saturating_add(1);
                    if mask.is_none_or(|mask| mask.covers(first, vertical.0, last, vertical.1)) {
                        *cutoff = (index + 1) as u16;
                        stats.opaque_tiles += 1;
                        stats.skipped_background_pixels += (tile * tile) as u32;
                    }
                }
            }
        }
    }

    pub(super) fn visible_span(
        &self,
        y: usize,
        mut x: usize,
        end: usize,
        index: usize,
    ) -> Option<(usize, usize)> {
        let tile = self.tile.pixels();
        let row = y / tile * self.columns();
        while x < end && usize::from(self.cutoffs[row + x / tile]) > index + 1 {
            x = ((x / tile + 1) * tile).min(end);
        }
        if x == end {
            return None;
        }
        let start = x;
        while x < end && usize::from(self.cutoffs[row + x / tile]) <= index + 1 {
            x = ((x / tile + 1) * tile).min(end);
        }
        Some((start, x))
    }

    pub(super) fn has_visible_span(
        &self,
        rows: core::ops::Range<usize>,
        columns: core::ops::Range<usize>,
        index: usize,
    ) -> bool {
        let mut row = rows.start;
        while row < rows.end {
            if self
                .visible_span(row, columns.start, columns.end, index)
                .is_some()
            {
                return true;
            }
            row = self.row_end(row, rows.end);
        }
        false
    }

    pub(super) fn hides(&self, r: ScreenRect, index: usize) -> bool {
        let tile = self.tile.pixels();
        let left = r.x.clamp(0, VIEWPORT_WIDTH) as usize / tile;
        let top = r.y.clamp(0, VIEWPORT_HEIGHT) as usize / tile;
        let right = (r.x.saturating_add(r.width).clamp(0, VIEWPORT_WIDTH) as usize).div_ceil(tile);
        let bottom =
            (r.y.saturating_add(r.height).clamp(0, VIEWPORT_HEIGHT) as usize).div_ceil(tile);
        (top..bottom).any(|y| {
            (left..right).any(|x| usize::from(self.cutoffs[y * self.columns() + x]) > index + 1)
        })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SceneStats {
    pub raster: RasterStats,
    pub skipped_background_pixels: u32,
    pub opaque_tiles: u32,
    pub coverage_tests: u32,
    pub scaler_preparations: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RasterPhase {
    Coverage,
    Background,
    Setup,
    Pixels,
    Idle,
}

/// Repeating native RGB565 row colors and a platform-selectable span writer.
/// `fill` receives colors in the framebuffer byte order and a span starting at
/// pattern phase zero. It must overwrite only that span with the repeated colors.
pub trait Background {
    fn row(&mut self, y: usize) -> [u16; 4];

    fn fill(&mut self, pixels: &mut [u16], colors: [u16; 4]) {
        let mut chunks = pixels.chunks_exact_mut(4);
        for chunk in &mut chunks {
            chunk.copy_from_slice(&colors);
        }
        for (pixel, color) in chunks.into_remainder().iter_mut().zip(colors) {
            *pixel = color;
        }
    }
}

impl<F: FnMut(usize) -> [u16; 4]> Background for F {
    fn row(&mut self, y: usize) -> [u16; 4] {
        self(y)
    }
}

/// Boards must be far-to-near, with increasing ID for equal depths.
/// Coverage is built before drawing; scaler coordinates are prepared once per
/// visible board. Background returns four repeating native RGB565 words.
pub fn raster_scene(
    framebuffer: &mut [u16],
    stride: usize,
    boards: &[SceneBillboard<'_>],
    background_row: impl Background,
    wire: bool,
    occlusion: &mut Occlusion<'_>,
) -> Result<SceneStats, RasterError> {
    raster_scene_observed(
        framebuffer,
        stride,
        boards,
        background_row,
        wire,
        occlusion,
        &mut |_| {},
    )
}

/// Observer changes phase before work; Idle ends the frame. Observers must not
/// change scene state. Times are owned by the caller, not the portable library.
pub fn raster_scene_observed(
    framebuffer: &mut [u16],
    stride: usize,
    boards: &[SceneBillboard<'_>],
    background_row: impl Background,
    wire: bool,
    occlusion: &mut Occlusion<'_>,
    observer: &mut impl FnMut(RasterPhase),
) -> Result<SceneStats, RasterError> {
    raster_scene_with_blitter(
        framebuffer,
        stride,
        boards,
        (background_row, &mut ScalarBlitter),
        wire,
        occlusion,
        observer,
    )
}

/// Render with a caller-owned span blitter; clipping and sampling stay portable.
pub fn raster_scene_with_blitter(
    framebuffer: &mut [u16],
    stride: usize,
    boards: &[SceneBillboard<'_>],
    backend: (impl Background, &mut impl Blitter),
    wire: bool,
    occlusion: &mut Occlusion<'_>,
    observer: &mut impl FnMut(RasterPhase),
) -> Result<SceneStats, RasterError> {
    let (mut background_row, blitter) = backend;
    if stride < VIEWPORT_WIDTH as usize
        || stride
            .checked_mul(VIEWPORT_HEIGHT as usize)
            .is_none_or(|n| framebuffer.len() < n)
    {
        return Err(RasterError::InvalidFramebuffer);
    }
    if boards.len() > usize::from(u16::MAX) {
        return Err(RasterError::InvalidOrder);
    }
    if boards.windows(2).any(|pair| {
        let a = pair[0].projected;
        let b = pair[1].projected;
        a.depth < b.depth || (a.depth == b.depth && a.id > b.id)
    }) {
        return Err(RasterError::InvalidOrder);
    }
    let mut stats = SceneStats::default();
    observer(RasterPhase::Coverage);
    occlusion.prepare(boards, &mut stats);
    observer(RasterPhase::Background);
    paint_background(
        &mut super::BandTarget::new(framebuffer, stride, 0, VIEWPORT_HEIGHT as usize)?,
        occlusion,
        stats.opaque_tiles,
        &mut background_row,
        wire,
    );
    for (index, board) in boards.iter().enumerate() {
        observer(RasterPhase::Setup);
        let mask = (stats.opaque_tiles != 0 && occlusion.hides(board.projected.screen_rect, index))
            .then_some((&*occlusion, index));
        let result = raster_billboard_masked(
            framebuffer,
            stride,
            board.projected,
            board.texture,
            board.region,
            wire,
            (mask, observer, blitter),
        )?;
        let native = board.projected.filter == TextureFilter::Nearest
            && board.projected.screen_rect.width == i32::from(board.region.width)
            && board.projected.screen_rect.height == i32::from(board.region.height);
        if !native && result.nearest_samples + result.bilinear_samples > 0 {
            stats.scaler_preparations += 1;
        }
        stats.raster.nearest_samples = stats
            .raster
            .nearest_samples
            .saturating_add(result.nearest_samples);
        stats.raster.bilinear_samples = stats
            .raster
            .bilinear_samples
            .saturating_add(result.bilinear_samples);
    }
    observer(RasterPhase::Idle);
    Ok(stats)
}

fn source_span(
    pixel: usize,
    tile: usize,
    origin: i32,
    destination: i32,
    offset: u16,
    source: u16,
) -> (usize, usize) {
    let map = |pixel: usize| {
        usize::from(offset)
            + (((pixel as i64 - i64::from(origin)) * i64::from(source)) / i64::from(destination))
                as usize
    };
    (map(pixel), map(pixel + tile - 1))
}

pub(super) fn paint_background(
    target: &mut super::BandTarget<'_>,
    occlusion: &Occlusion<'_>,
    opaque_tiles: u32,
    background_row: &mut impl Background,
    wire: bool,
) {
    let tile = occlusion.tile.pixels();
    let columns = occlusion.columns();
    // At most 20 alternating visible spans in the 40-column 8-pixel grid.
    let mut spans = [[0_u16; 2]; VIEWPORT_WIDTH as usize / 16];
    let mut band = target.y;
    while band < target.y + target.rows {
        let end = occlusion.row_end(band, target.y + target.rows);
        let mut count = 0;
        if opaque_tiles == 0 {
            spans[0] = [0, VIEWPORT_WIDTH as u16];
            count = 1;
        } else {
            let cutoffs = &occlusion.cutoffs[band / tile * columns..][..columns];
            let mut column = 0;
            while column < columns {
                if cutoffs[column] != 0 {
                    column += 1;
                    continue;
                }
                let start = column;
                column += 1;
                while column < columns && cutoffs[column] == 0 {
                    column += 1;
                }
                spans[count] = [(start * tile) as u16, (column * tile) as u16];
                count += 1;
            }
        }
        for y in band..end {
            let colors = background_row
                .row(y)
                .map(|c| if wire { c.to_be() } else { c });
            for &[start, end] in &spans[..count] {
                background_row.fill(
                    &mut target.pixels[(y - target.y) * target.stride + usize::from(start)
                        ..(y - target.y) * target.stride + usize::from(end)],
                    colors,
                );
            }
        }
        band = end;
    }
}
