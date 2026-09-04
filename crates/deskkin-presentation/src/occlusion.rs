use super::{
    ProjectedBillboard, RasterError, RasterStats, ScreenRect, SourceSize, Texture, TextureFilter,
    TextureRegion, VIEWPORT_HEIGHT, VIEWPORT_WIDTH, raster_billboard_clipped, validate_texture,
};

const TILE: usize = 8;
const COLUMNS: usize = VIEWPORT_WIDTH as usize / TILE;

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
    projected: ProjectedBillboard,
    texture: Texture<'a>,
    region: TextureRegion,
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

    fn intersects(self, tile: ScreenRect) -> bool {
        let r = self.projected.screen_rect;
        r.x < tile.x + tile.width
            && r.y < tile.y + tile.height
            && r.x.saturating_add(r.width) > tile.x
            && r.y.saturating_add(r.height) > tile.y
    }

    fn covers(self, tile: ScreenRect) -> bool {
        let r = self.projected.screen_rect;
        if r.x > tile.x
            || r.y > tile.y
            || r.x.saturating_add(r.width) < tile.x + tile.width
            || r.y.saturating_add(r.height) < tile.y + tile.height
        {
            return false;
        }
        match self.texture.coverage {
            Coverage::Opaque => true,
            Coverage::Alpha8 { opaque_blocks, .. } => {
                // Alpha textures use nearest sampling in the world. Do not
                // infer bilinear coverage from a nearest sampling footprint.
                if self.projected.filter != TextureFilter::Nearest {
                    return false;
                }
                let map = |pixel: i32, origin: i32, source: u16, destination: i32| {
                    ((i64::from(pixel) - i64::from(origin)) * i64::from(source)
                        / i64::from(destination)) as usize
                };
                let x = usize::from(self.region.source_x);
                let y = usize::from(self.region.source_y);
                opaque_blocks.covers(
                    x + map(tile.x, r.x, self.region.width, r.width),
                    y + map(tile.y, r.y, self.region.height, r.height),
                    x + map(tile.x + tile.width - 1, r.x, self.region.width, r.width),
                    y + map(tile.y + tile.height - 1, r.y, self.region.height, r.height),
                )
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SceneStats {
    pub raster: RasterStats,
    pub skipped_background_pixels: u32,
    pub opaque_tiles: u32,
}

/// `boards` must be in far-to-near order, breaking depth ties by increasing ID.
/// `background_row` returns four repeating native RGB565 pixels for a screen row.
/// Adjacent eligible tiles are drawn as spans; no per-entity visibility buffers.
pub fn raster_scene(
    framebuffer: &mut [u16],
    stride: usize,
    boards: &[SceneBillboard<'_>],
    mut background_row: impl FnMut(usize) -> [u16; 4],
    wire: bool,
) -> Result<SceneStats, RasterError> {
    if stride < VIEWPORT_WIDTH as usize
        || stride
            .checked_mul(VIEWPORT_HEIGHT as usize)
            .is_none_or(|n| framebuffer.len() < n)
    {
        return Err(RasterError::InvalidFramebuffer);
    }
    if boards.windows(2).any(|pair| {
        let a = pair[0].projected;
        let b = pair[1].projected;
        a.depth < b.depth || (a.depth == b.depth && a.id > b.id)
    }) {
        return Err(RasterError::InvalidOrder);
    }
    let mut stats = SceneStats::default();
    for y in (0..VIEWPORT_HEIGHT as usize).step_by(TILE) {
        let mut cutoffs = [None; COLUMNS];
        for (column, cutoff) in cutoffs.iter_mut().enumerate() {
            let tile = ScreenRect {
                x: (column * TILE) as i32,
                y: y as i32,
                width: TILE as i32,
                height: TILE as i32,
            };
            *cutoff = boards.iter().rposition(|board| board.covers(tile));
            if cutoff.is_some() {
                stats.opaque_tiles += 1;
                stats.skipped_background_pixels += (TILE * TILE) as u32;
            }
        }
        for row in y..y + TILE {
            let colors = background_row(row).map(|c| if wire { c.to_be() } else { c });
            for (column, cutoff) in cutoffs.iter().enumerate() {
                if cutoff.is_none() {
                    for (i, pixel) in framebuffer
                        [row * stride + column * TILE..row * stride + (column + 1) * TILE]
                        .iter_mut()
                        .enumerate()
                    {
                        *pixel = colors[i % 4];
                    }
                }
            }
        }
        for (index, board) in boards.iter().copied().enumerate() {
            let mut column = 0;
            while column < COLUMNS {
                let tile = ScreenRect {
                    x: (column * TILE) as i32,
                    y: y as i32,
                    width: TILE as i32,
                    height: TILE as i32,
                };
                if cutoffs[column].is_some_and(|cutoff| index < cutoff) || !board.intersects(tile) {
                    column += 1;
                    continue;
                }
                let opaque = cutoffs[column] == Some(index);
                let first = column;
                column += 1;
                while column < COLUMNS
                    && cutoffs[column].is_none_or(|cutoff| index >= cutoff)
                    && (cutoffs[column] == Some(index)) == opaque
                {
                    column += 1;
                }
                let mut texture = board.texture;
                if opaque {
                    texture.coverage = Coverage::Opaque;
                }
                let result = raster_billboard_clipped(
                    framebuffer,
                    stride,
                    board.projected,
                    texture,
                    board.region,
                    wire,
                    ScreenRect {
                        x: (first * TILE) as i32,
                        y: y as i32,
                        width: ((column - first) * TILE) as i32,
                        height: TILE as i32,
                    },
                )?;
                stats.raster.nearest_samples = stats
                    .raster
                    .nearest_samples
                    .saturating_add(result.nearest_samples);
                stats.raster.bilinear_samples = stats
                    .raster
                    .bilinear_samples
                    .saturating_add(result.bilinear_samples);
            }
        }
    }
    Ok(stats)
}
