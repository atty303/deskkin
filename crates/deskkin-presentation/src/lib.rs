#![no_std]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::missing_errors_doc,
    clippy::similar_names
)]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

include!(concat!(env!("OUT_DIR"), "/trig_table.rs"));

mod blit;
pub mod demo_world;
pub use blit::{Blitter, ScalarBlitter};

pub const TURN_UNITS: i64 = 65_536;
pub const VIEWPORT_WIDTH: i32 = 320;
pub const VIEWPORT_HEIGHT: i32 = 240;
pub const FOCAL_LENGTH: i32 = 160;

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct WorldUnit(i32);

impl WorldUnit {
    pub const ZERO: Self = Self(0);
    pub const ONE: Self = Self(1 << 16);

    #[must_use]
    pub const fn from_bits(bits: i32) -> Self {
        Self(bits)
    }

    #[must_use]
    pub const fn from_int(value: i16) -> Self {
        Self((value as i32) << 16)
    }

    #[must_use]
    pub const fn bits(self) -> i32 {
        self.0
    }

    #[must_use]
    pub const fn ratio(numerator: i32, denominator: i32) -> Self {
        Self((((numerator as i64) << 16) / denominator as i64) as i32)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct UnwrappedAngle(i64);

impl UnwrappedAngle {
    pub const ZERO: Self = Self(0);

    #[must_use]
    pub const fn from_units(units: i64) -> Self {
        Self(units)
    }

    #[must_use]
    pub const fn from_degrees(degrees: i64) -> Self {
        Self(degrees * TURN_UNITS / 360)
    }

    #[must_use]
    pub const fn units(self) -> i64 {
        self.0
    }

    #[must_use]
    pub const fn wrapping_add(self, rhs: Self) -> Self {
        Self(self.0.wrapping_add(rhs.0))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CylindricalPose {
    pub radius: WorldUnit,
    pub azimuth: UnwrappedAngle,
    pub height: WorldUnit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CameraPose {
    pub radius: WorldUnit,
    pub observed_azimuth: UnwrappedAngle,
    pub height: WorldUnit,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct BillboardId(pub u16);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextureId(pub u16);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextureFilter {
    Nearest,
    Bilinear,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Billboard {
    pub id: BillboardId,
    pub pose: CylindricalPose,
    pub world_height: WorldUnit,
    pub texture_id: TextureId,
    pub filter: TextureFilter,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceSize {
    pub width: u16,
    pub height: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScreenRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectedBillboard {
    pub id: BillboardId,
    pub screen_rect: ScreenRect,
    pub depth: WorldUnit,
    pub source: TextureId,
    pub filter: TextureFilter,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectionCull {
    InvalidRadius,
    NearPlane,
    OutsideViewport,
    InvalidSource,
}

fn signed_turn(angle: i64) -> i64 {
    let wrapped = angle.rem_euclid(TURN_UNITS);
    if wrapped > TURN_UNITS / 2 {
        wrapped - TURN_UNITS
    } else {
        wrapped
    }
}

fn interpolated_sin_q15(angle: i64) -> i32 {
    let wrapped = angle.rem_euclid(TURN_UNITS) as usize;
    let index = wrapped >> 6;
    let fraction = (wrapped & 63) as i32;
    let first = i32::from(SIN_Q15[index]);
    let second = i32::from(SIN_Q15[(index + 1) & 1023]);
    first + ((second - first) * fraction + 32) / 64
}

fn sin_q15(angle: i64) -> i32 {
    let signed = signed_turn(angle);
    if signed < 0 {
        -interpolated_sin_q15(-signed)
    } else {
        interpolated_sin_q15(signed)
    }
}

fn cos_q15(angle: i64) -> i32 {
    interpolated_sin_q15(TURN_UNITS / 4 + signed_turn(angle).abs())
}

/// Projects one camera-facing billboard into the fixed 320x240 viewport.
///
/// The camera looks toward the cylinder center. World and depth values use
/// signed Q16.16; angles use 65,536 units per unwrapped turn.
pub fn project_billboard(
    billboard: Billboard,
    source: SourceSize,
    camera: CameraPose,
) -> Result<ProjectedBillboard, ProjectionCull> {
    if source.width == 0 || source.height == 0 {
        return Err(ProjectionCull::InvalidSource);
    }
    let (center_x, center_y, depth) = project_anchor(billboard.pose, camera)?;
    let projected_height = ((i64::from(billboard.world_height.bits()) * i64::from(FOCAL_LENGTH))
        / i64::from(depth)) as i32;
    if projected_height <= 0 {
        return Err(ProjectionCull::InvalidSource);
    }
    let projected_width =
        ((i64::from(projected_height) * i64::from(source.width)) / i64::from(source.height)) as i32;
    let rect = ScreenRect {
        x: center_x - projected_width / 2,
        y: center_y - projected_height / 2,
        width: projected_width,
        height: projected_height,
    };
    if rect.x >= VIEWPORT_WIDTH
        || rect.y >= VIEWPORT_HEIGHT
        || rect.x.saturating_add(rect.width) <= 0
        || rect.y.saturating_add(rect.height) <= 0
    {
        return Err(ProjectionCull::OutsideViewport);
    }
    Ok(ProjectedBillboard {
        id: billboard.id,
        screen_rect: rect,
        depth: WorldUnit::from_bits(depth),
        source: billboard.texture_id,
        filter: billboard.filter,
    })
}

fn project_anchor(
    pose: CylindricalPose,
    camera: CameraPose,
) -> Result<(i32, i32, i32), ProjectionCull> {
    if pose.radius < WorldUnit::ZERO || pose.radius > WorldUnit::ratio(32, 10) {
        return Err(ProjectionCull::InvalidRadius);
    }
    let delta = pose.azimuth.units() - camera.observed_azimuth.units();
    let x = ((i64::from(pose.radius.bits()) * i64::from(sin_q15(delta))) >> 15) as i32;
    let radial_depth = ((i64::from(pose.radius.bits()) * i64::from(cos_q15(delta))) >> 15) as i32;
    let depth = camera.radius.bits().saturating_sub(radial_depth);
    if depth < WorldUnit::ratio(1, 4).bits() {
        return Err(ProjectionCull::NearPlane);
    }
    let center_x =
        VIEWPORT_WIDTH / 2 + ((i64::from(x) * i64::from(FOCAL_LENGTH)) / i64::from(depth)) as i32;
    let vertical = camera.height.bits().saturating_sub(pose.height.bits());
    let center_y = VIEWPORT_HEIGHT / 2
        + ((i64::from(vertical) * i64::from(FOCAL_LENGTH)) / i64::from(depth)) as i32;
    Ok((center_x, center_y, depth))
}

/// A fixed-pixel sprite at a bottom-center world anchor. LODs are ordered near to far;
/// each LOD texture is drawn at its native size, without continuous scaling.
#[derive(Clone, Copy, Debug)]
pub struct Particle {
    pub id: BillboardId,
    pub pose: CylindricalPose,
    pub lods: [ParticleLod; 3],
}

#[derive(Clone, Copy, Debug)]
pub struct ParticleLod {
    pub texture: TextureId,
    pub size: SourceSize,
    pub max_depth: WorldUnit,
}

pub fn project_particle(
    particle: Particle,
    camera: CameraPose,
) -> Result<ProjectedBillboard, ProjectionCull> {
    let (x, y, depth) = project_anchor(particle.pose, camera)?;
    let lod = particle
        .lods
        .iter()
        .find(|lod| depth <= lod.max_depth.bits())
        .ok_or(ProjectionCull::OutsideViewport)?;
    if lod.size.width == 0 || lod.size.height == 0 {
        return Err(ProjectionCull::InvalidSource);
    }
    let rect = ScreenRect {
        x: x - i32::from(lod.size.width) / 2,
        y: y - i32::from(lod.size.height),
        width: i32::from(lod.size.width),
        height: i32::from(lod.size.height),
    };
    if rect.x >= VIEWPORT_WIDTH
        || rect.y >= VIEWPORT_HEIGHT
        || rect.x.saturating_add(rect.width) <= 0
        || rect.y.saturating_add(rect.height) <= 0
    {
        return Err(ProjectionCull::OutsideViewport);
    }
    Ok(ProjectedBillboard {
        id: particle.id,
        screen_rect: rect,
        depth: WorldUnit::from_bits(depth),
        source: lod.texture,
        filter: TextureFilter::Nearest,
    })
}

pub fn sort_far_to_near(projected: &mut [ProjectedBillboard]) {
    projected.sort_unstable_by(|left, right| {
        right
            .depth
            .cmp(&left.depth)
            .then_with(|| left.id.cmp(&right.id))
    });
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TouchYawAdapter {
    target: UnwrappedAngle,
    last_x: Option<i16>,
}

impl TouchYawAdapter {
    #[must_use]
    pub const fn new(target: UnwrappedAngle) -> Self {
        Self {
            target,
            last_x: None,
        }
    }

    #[must_use]
    pub const fn target(self) -> UnwrappedAngle {
        self.target
    }

    pub fn sample(&mut self, x: i16, pressed: bool) -> UnwrappedAngle {
        if !pressed {
            self.last_x = None;
            return self.target;
        }
        if let Some(previous) = self.last_x {
            self.target.0 += i64::from(i32::from(x) - i32::from(previous)) * TURN_UNITS
                / i64::from(VIEWPORT_WIDTH);
        }
        self.last_x = Some(x);
        self.target
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RateLimitedObservedYaw {
    observed: UnwrappedAngle,
}

impl RateLimitedObservedYaw {
    #[must_use]
    pub const fn new(observed: UnwrappedAngle) -> Self {
        Self { observed }
    }

    #[must_use]
    pub const fn observed(self) -> UnwrappedAngle {
        self.observed
    }

    pub fn advance(&mut self, target: UnwrappedAngle, elapsed_ms: u32) -> UnwrappedAngle {
        let difference = target.units().saturating_sub(self.observed.units());
        let maximum = (TURN_UNITS / 2)
            .saturating_mul(i64::from(elapsed_ms))
            .saturating_div(1_000);
        self.observed.0 = self
            .observed
            .0
            .saturating_add(difference.clamp(-maximum, maximum));
        self.observed
    }
}

mod bands;
pub use bands::{BandTarget, PreparedBoard, PreparedScene};

mod occlusion;
pub use occlusion::{
    Background, Coverage, Mask8, Occlusion, RasterPhase, SceneBillboard, SceneStats, ScreenTile,
    build_opaque_mask, raster_scene, raster_scene_observed, raster_scene_with_blitter,
};

#[derive(Clone, Copy)]
pub struct Texture<'a> {
    pub size: SourceSize,
    pub pixels: &'a [u16],
    pub coverage: Coverage<'a>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextureRegion {
    pub source_x: u16,
    pub source_y: u16,
    pub width: u16,
    pub height: u16,
    pub stride: u16,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RasterStats {
    pub nearest_samples: u32,
    pub bilinear_samples: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RasterError {
    InvalidFramebuffer,
    InvalidTexture,
    InvalidMask,
    InvalidOrder,
    InsufficientScratch,
}

pub fn raster_billboard(
    framebuffer: &mut [u16],
    stride: usize,
    projected: ProjectedBillboard,
    texture: Texture<'_>,
) -> Result<RasterStats, RasterError> {
    let region = full_texture_region(texture.size);
    raster_billboard_ordered(framebuffer, stride, projected, texture, region, false)
}

fn validate_texture(texture: Texture<'_>, region: TextureRegion) -> Result<(), RasterError> {
    let right = usize::from(region.source_x) + usize::from(region.width);
    let bottom = usize::from(region.source_y) + usize::from(region.height);
    let required = bottom.saturating_sub(1) * usize::from(region.stride) + right;
    if region.width == 0
        || region.height == 0
        || right > usize::from(region.stride)
        || right > usize::from(texture.size.width)
        || bottom > usize::from(texture.size.height)
        || region.stride != texture.size.width
        || texture.pixels.len() < required
        || (texture.coverage.is_alpha() && texture.coverage.alpha().len() < required)
    {
        return Err(RasterError::InvalidTexture);
    }
    texture.coverage.validate(texture.size)
}

/// Renders into an RGB565 framebuffer whose words are stored in wire byte order.
pub fn raster_billboard_be(
    framebuffer: &mut [u16],
    stride: usize,
    projected: ProjectedBillboard,
    texture: Texture<'_>,
) -> Result<RasterStats, RasterError> {
    let region = full_texture_region(texture.size);
    raster_billboard_ordered(framebuffer, stride, projected, texture, region, true)
}

pub fn raster_billboard_region_be(
    framebuffer: &mut [u16],
    stride: usize,
    projected: ProjectedBillboard,
    texture: Texture<'_>,
    region: TextureRegion,
) -> Result<RasterStats, RasterError> {
    raster_billboard_ordered(framebuffer, stride, projected, texture, region, true)
}

fn raster_billboard_ordered(
    framebuffer: &mut [u16],
    stride: usize,
    projected: ProjectedBillboard,
    texture: Texture<'_>,
    region: TextureRegion,
    big_endian: bool,
) -> Result<RasterStats, RasterError> {
    raster_billboard_masked(
        framebuffer,
        stride,
        projected,
        texture,
        region,
        big_endian,
        (None, &mut |_| {}, &mut ScalarBlitter),
    )
}

fn raster_billboard_masked(
    framebuffer: &mut [u16],
    stride: usize,
    projected: ProjectedBillboard,
    texture: Texture<'_>,
    region: TextureRegion,
    big_endian: bool,
    observation: (
        Option<(&Occlusion<'_>, usize)>,
        &mut impl FnMut(RasterPhase),
        &mut impl Blitter,
    ),
) -> Result<RasterStats, RasterError> {
    let (mask, observer, blitter) = observation;
    if stride < VIEWPORT_WIDTH as usize || framebuffer.len() < stride * VIEWPORT_HEIGHT as usize {
        return Err(RasterError::InvalidFramebuffer);
    }
    validate_texture(texture, region)?;
    if projected.screen_rect.width <= 0 || projected.screen_rect.height <= 0 {
        return Err(RasterError::InvalidTexture);
    }
    let left = projected.screen_rect.x.max(0);
    let top = projected.screen_rect.y.max(0);
    let right = projected
        .screen_rect
        .x
        .saturating_add(projected.screen_rect.width)
        .min(VIEWPORT_WIDTH);
    let bottom = projected
        .screen_rect
        .y
        .saturating_add(projected.screen_rect.height)
        .min(VIEWPORT_HEIGHT);
    let rows = top as usize..bottom as usize;
    let columns = left as usize..right as usize;
    if left >= right
        || top >= bottom
        || mask.is_some_and(|(map, index)| !map.has_visible_span(rows, columns, index))
    {
        return Ok(RasterStats::default());
    }
    if projected.filter == TextureFilter::Nearest
        && projected.screen_rect.width == i32::from(region.width)
        && projected.screen_rect.height == i32::from(region.height)
    {
        observer(RasterPhase::Pixels);
        let nearest_samples = raster_native(
            &mut BandTarget::new(framebuffer, stride, 0, VIEWPORT_HEIGHT as usize)?,
            projected,
            texture,
            region,
            big_endian,
            (mask, blitter),
        );
        observer(RasterPhase::Setup);
        return Ok(RasterStats {
            nearest_samples,
            bilinear_samples: 0,
        });
    }
    let mut x = AxisStepper::new(
        region.width,
        projected.screen_rect.width,
        i64::from(left) - i64::from(projected.screen_rect.x),
    );
    let mut columns = [ColumnSample::default(); VIEWPORT_WIDTH as usize];
    let columns = &mut columns[..(right - left) as usize];
    for column in columns.iter_mut() {
        let coordinate = x.take();
        let index = (coordinate >> 16) as u16;
        *column = ColumnSample {
            first: region.source_x + index,
            second: region.source_x + (index + 1).min(region.width - 1),
            fraction: coordinate & 0xffff,
        };
    }
    let y = AxisStepper::new(
        region.height,
        projected.screen_rect.height,
        i64::from(top) - i64::from(projected.screen_rect.y),
    );
    let rows = RasterRows {
        origin: 0,
        stride,
        left: left as usize,
        top: top as usize,
        bottom: bottom as usize,
        columns,
        y,
        region,
    };
    observer(RasterPhase::Pixels);
    let samples = rows.dispatch_visible(
        framebuffer,
        texture,
        projected.filter,
        big_endian,
        (mask, blitter),
    );
    observer(RasterPhase::Setup);
    Ok(match projected.filter {
        TextureFilter::Nearest => RasterStats {
            nearest_samples: samples,
            bilinear_samples: 0,
        },
        TextureFilter::Bilinear => RasterStats {
            nearest_samples: 0,
            bilinear_samples: samples,
        },
    })
}

fn raster_native(
    target: &mut BandTarget<'_>,
    projected: ProjectedBillboard,
    texture: Texture<'_>,
    region: TextureRegion,
    big_endian: bool,
    context: (Option<(&Occlusion<'_>, usize)>, &mut impl Blitter),
) -> u32 {
    let (mask, blitter) = context;
    let rect = projected.screen_rect;
    let left = rect.x.max(0) as usize;
    let right = rect.x.saturating_add(rect.width).min(VIEWPORT_WIDTH) as usize;
    let top = (rect.y.max(0) as usize).max(target.y);
    let bottom = (rect.y.saturating_add(rect.height).min(VIEWPORT_HEIGHT) as usize)
        .min(target.y + target.rows);
    let alpha = texture
        .coverage
        .is_alpha()
        .then(|| texture.coverage.alpha());
    let mut samples = 0;
    let mut row = top;
    while row < bottom {
        let row_end = mask.map_or(bottom, |(map, _)| map.row_end(row, bottom));
        let mut x = left;
        while x < right {
            let span = mask.map_or(Some((x, right)), |(map, index)| {
                map.visible_span(row, x, right, index)
            });
            let Some((start, end)) = span else { break };
            let source_x = usize::from(region.source_x) + (start as i32 - rect.x) as usize;
            for y in row..row_end {
                let source = (usize::from(region.source_y) + (y as i32 - rect.y) as usize)
                    * usize::from(region.stride)
                    + source_x;
                blitter.blit_from(
                    &mut target.pixels[(y - target.y) * target.stride + start
                        ..(y - target.y) * target.stride + end],
                    texture.pixels,
                    source,
                    alpha,
                    big_endian,
                );
            }
            samples += ((end - start) * (row_end - row)) as u32;
            x = end;
        }
        row = row_end;
    }
    samples
}

// Carrying the division remainder preserves floor(offset * source / destination)
// exactly, including clipped starts and non-integral scale factors.
#[derive(Clone, Copy)]
struct AxisStepper {
    coordinate: u32,
    step: u32,
    remainder: u32,
    remainder_step: u32,
    denominator: u32,
}

impl AxisStepper {
    fn new(source: u16, destination: i32, offset: i64) -> Self {
        let numerator = u64::from(source) << 16;
        let denominator = destination as u64;
        let start = offset as u64 * numerator;
        Self {
            coordinate: (start / denominator) as u32,
            step: (numerator / denominator) as u32,
            remainder: (start % denominator) as u32,
            remainder_step: (numerator % denominator) as u32,
            denominator: denominator as u32,
        }
    }

    fn take(&mut self) -> u32 {
        let coordinate = self.coordinate;
        self.coordinate += self.step;
        self.remainder += self.remainder_step;
        if self.remainder >= self.denominator {
            self.remainder -= self.denominator;
            self.coordinate += 1;
        }
        coordinate
    }
}

/// Caller-owned reusable horizontal sampler storage. Fields are prepared by the renderer.
#[derive(Clone, Copy, Default)]
pub struct ColumnSample {
    first: u16,
    second: u16,
    fraction: u32,
}

#[repr(align(16))]
struct SampleRow<T>(T);

struct RasterRows<'a> {
    origin: usize,
    stride: usize,
    left: usize,
    top: usize,
    bottom: usize,
    columns: &'a [ColumnSample],
    y: AxisStepper,
    region: TextureRegion,
}

impl RasterRows<'_> {
    fn dispatch_visible(
        self,
        framebuffer: &mut [u16],
        texture: Texture<'_>,
        filter: TextureFilter,
        big_endian: bool,
        context: (Option<(&Occlusion<'_>, usize)>, &mut impl Blitter),
    ) -> u32 {
        let (mask, blitter) = context;
        let total = (self.columns.len() * (self.bottom - self.top)) as u32;
        if let Some((map, index)) = mask {
            let mut samples = 0;
            let mut y = self.y;
            let mut row = self.top;
            while row < self.bottom {
                let row_end = map.row_end(row, self.bottom);
                let mut x = self.left;
                while let Some((start, end)) =
                    map.visible_span(row, x, self.left + self.columns.len(), index)
                {
                    RasterRows {
                        origin: self.origin,
                        stride: self.stride,
                        left: start,
                        top: row,
                        bottom: row_end,
                        columns: &self.columns[start - self.left..end - self.left],
                        y,
                        region: self.region,
                    }
                    .dispatch(
                        framebuffer,
                        texture,
                        filter,
                        big_endian,
                        blitter,
                    );
                    samples += ((end - start) * (row_end - row)) as u32;
                    x = end;
                }
                for _ in row..row_end {
                    y.take();
                }
                row = row_end;
            }
            samples
        } else {
            self.dispatch(framebuffer, texture, filter, big_endian, blitter);
            total
        }
    }

    fn dispatch(
        self,
        framebuffer: &mut [u16],
        texture: Texture<'_>,
        filter: TextureFilter,
        big_endian: bool,
        blitter: &mut impl Blitter,
    ) {
        match (filter, texture.coverage.is_alpha(), big_endian) {
            (TextureFilter::Nearest, false, false) => {
                self.draw::<false, false, false>(framebuffer, texture, blitter);
            }
            (TextureFilter::Nearest, false, true) => {
                self.draw::<false, false, true>(framebuffer, texture, blitter);
            }
            (TextureFilter::Nearest, true, false) => {
                self.draw::<false, true, false>(framebuffer, texture, blitter);
            }
            (TextureFilter::Nearest, true, true) => {
                self.draw::<false, true, true>(framebuffer, texture, blitter);
            }
            (TextureFilter::Bilinear, false, false) => {
                self.draw::<true, false, false>(framebuffer, texture, blitter);
            }
            (TextureFilter::Bilinear, false, true) => {
                self.draw::<true, false, true>(framebuffer, texture, blitter);
            }
            (TextureFilter::Bilinear, true, false) => {
                self.draw::<true, true, false>(framebuffer, texture, blitter);
            }
            (TextureFilter::Bilinear, true, true) => {
                self.draw::<true, true, true>(framebuffer, texture, blitter);
            }
        }
    }

    fn draw<const BILINEAR: bool, const ALPHA: bool, const BIG_ENDIAN: bool>(
        mut self,
        framebuffer: &mut [u16],
        texture: Texture<'_>,
        blitter: &mut impl Blitter,
    ) {
        let mut colors = SampleRow([0u16; VIEWPORT_WIDTH as usize]);
        let mut alphas = SampleRow([0u8; VIEWPORT_WIDTH as usize]);
        for destination_y in self.top..self.bottom {
            let coordinate = self.y.take();
            let sy = (coordinate >> 16) as usize;
            let first_row =
                (usize::from(self.region.source_y) + sy) * usize::from(self.region.stride);
            let second_row = (usize::from(self.region.source_y)
                + (sy + 1).min(usize::from(self.region.height) - 1))
                * usize::from(self.region.stride);
            let fraction_y = coordinate & 0xffff;
            let start = (destination_y - self.origin) * self.stride + self.left;
            for (index, column) in self.columns.iter().enumerate() {
                let source_index = first_row + usize::from(column.first);
                // Bilinear+A8 retains the existing constant-alpha convention.
                let alpha = if ALPHA {
                    texture.coverage.alpha()[if BILINEAR { 0 } else { source_index }]
                } else {
                    255
                };
                alphas.0[index] = alpha;
                let color = if BILINEAR {
                    let first = interpolate_rgb565(
                        texture.pixels[source_index],
                        texture.pixels[first_row + usize::from(column.second)],
                        column.fraction,
                    );
                    let second = interpolate_rgb565(
                        texture.pixels[second_row + usize::from(column.first)],
                        texture.pixels[second_row + usize::from(column.second)],
                        column.fraction,
                    );
                    interpolate_rgb565(first, second, fraction_y)
                } else {
                    texture.pixels[source_index]
                };
                colors.0[index] = color;
            }
            blitter.blit_from(
                &mut framebuffer[start..start + self.columns.len()],
                &colors.0,
                0,
                ALPHA.then_some(&alphas.0),
                BIG_ENDIAN,
            );
        }
    }
}

const fn full_texture_region(size: SourceSize) -> TextureRegion {
    TextureRegion {
        source_x: 0,
        source_y: 0,
        width: size.width,
        height: size.height,
        stride: size.width,
    }
}

fn interpolate_rgb565(first: u16, second: u16, fraction: u32) -> u16 {
    let inverse = 65_536 - fraction;
    let r = ((u32::from((first >> 11) & 0x1f) * inverse
        + u32::from((second >> 11) & 0x1f) * fraction
        + 32_768)
        >> 16) as u16;
    let g = ((u32::from((first >> 5) & 0x3f) * inverse
        + u32::from((second >> 5) & 0x3f) * fraction
        + 32_768)
        >> 16) as u16;
    let b = ((u32::from(first & 0x1f) * inverse + u32::from(second & 0x1f) * fraction + 32_768)
        >> 16) as u16;
    (r << 11) | (g << 5) | b
}

fn blend_rgb565(background: u16, foreground: u16, alpha: u8) -> u16 {
    let fraction = u32::from(alpha) * 257;
    interpolate_rgb565(background, foreground, fraction)
}

#[cfg(test)]
mod raster_tests;

/// Presentation-only animation states supported by the embedded Pet skin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PetAnimationState {
    Idle,
    MoveRight,
    MoveLeft,
    Attend,
}

impl PetAnimationState {
    #[must_use]
    pub const fn loop_index(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::MoveRight => 1,
            Self::MoveLeft => 2,
            Self::Attend => 3,
        }
    }

    #[must_use]
    pub const fn frame_count(self) -> u8 {
        match self {
            Self::Idle | Self::Attend => 6,
            Self::MoveRight | Self::MoveLeft => 8,
        }
    }

    #[must_use]
    pub const fn frame_period_ms(self) -> u32 {
        match self {
            Self::Idle | Self::Attend => 100,
            Self::MoveRight | Self::MoveLeft => 50,
        }
    }
}

/// One frame in a normalized Pet animation loop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PetFrame {
    pub state: PetAnimationState,
    pub index: u8,
}

/// Deterministic, allocation-free Pet animation state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PetAnimator {
    state: PetAnimationState,
    index: u8,
    elapsed_in_frame_ms: u32,
}

impl Default for PetAnimator {
    fn default() -> Self {
        Self::new()
    }
}

impl PetAnimator {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: PetAnimationState::Idle,
            index: 0,
            elapsed_in_frame_ms: 0,
        }
    }

    #[must_use]
    pub const fn state(self) -> PetAnimationState {
        self.state
    }

    #[must_use]
    pub const fn frame(self) -> PetFrame {
        PetFrame {
            state: self.state,
            index: self.index,
        }
    }

    #[must_use]
    pub fn set_state(&mut self, state: PetAnimationState) -> PetFrame {
        if self.state != state {
            self.state = state;
            self.index = 0;
            self.elapsed_in_frame_ms = 0;
        }
        self.frame()
    }

    #[must_use]
    pub fn advance(&mut self, elapsed_ms: u32) -> PetFrame {
        let period = u64::from(self.state.frame_period_ms());
        let total = u64::from(self.elapsed_in_frame_ms) + u64::from(elapsed_ms);
        let steps = total / period;
        self.elapsed_in_frame_ms = u32::try_from(total % period).unwrap_or_default();
        self.index =
            u8::try_from((u64::from(self.index) + steps) % u64::from(self.state.frame_count()))
                .unwrap_or_default();
        self.frame()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAMERA: CameraPose = CameraPose {
        radius: WorldUnit::from_int(4),
        observed_azimuth: UnwrappedAngle::ZERO,
        height: WorldUnit::ZERO,
    };

    fn billboard(id: u16, radius: WorldUnit, azimuth: UnwrappedAngle) -> Billboard {
        Billboard {
            id: BillboardId(id),
            pose: CylindricalPose {
                radius,
                azimuth,
                height: WorldUnit::ZERO,
            },
            world_height: WorldUnit::ONE,
            texture_id: TextureId(id),
            filter: TextureFilter::Nearest,
        }
    }

    fn project(value: Billboard, camera: CameraPose) -> ProjectedBillboard {
        project_billboard(
            value,
            SourceSize {
                width: 32,
                height: 32,
            },
            camera,
        )
        .unwrap()
    }

    #[test]
    fn integer_turns_do_not_change_projection_and_seam_is_continuous() {
        let base = billboard(1, WorldUnit::from_int(2), UnwrappedAngle::from_degrees(1));
        let extra_turn = Billboard {
            pose: CylindricalPose {
                azimuth: base
                    .pose
                    .azimuth
                    .wrapping_add(UnwrappedAngle::from_units(TURN_UNITS * 23)),
                ..base.pose
            },
            ..base
        };
        assert_eq!(project(base, CAMERA), project(extra_turn, CAMERA));
        let before = project(
            billboard(1, WorldUnit::from_int(2), UnwrappedAngle::from_degrees(359)),
            CAMERA,
        );
        let after = project(base, CAMERA);
        assert!((before.screen_rect.x - after.screen_rect.x).abs() <= 4);
    }

    #[test]
    fn positive_and_negative_azimuth_are_mirrored() {
        let left = project(
            billboard(1, WorldUnit::from_int(2), UnwrappedAngle::from_degrees(-20)),
            CAMERA,
        );
        let right = project(
            billboard(1, WorldUnit::from_int(2), UnwrappedAngle::from_degrees(20)),
            CAMERA,
        );
        assert_eq!(left.depth, right.depth);
        assert_eq!(left.screen_rect.width, right.screen_rect.width);
        assert!(
            (left.screen_rect.x + right.screen_rect.x + left.screen_rect.width - 320).abs() <= 1
        );
    }

    #[test]
    fn observed_yaw_is_the_only_camera_angle() {
        let entity = billboard(1, WorldUnit::from_int(2), UnwrappedAngle::ZERO);
        let centered = project(entity, CAMERA);
        let turned = project(
            entity,
            CameraPose {
                observed_azimuth: UnwrappedAngle::from_degrees(20),
                ..CAMERA
            },
        );
        assert_ne!(centered.screen_rect.x, turned.screen_rect.x);
    }

    #[test]
    fn painter_order_is_depth_then_identifier() {
        let mut values = [
            project(
                billboard(3, WorldUnit::from_int(1), UnwrappedAngle::ZERO),
                CAMERA,
            ),
            project(
                billboard(2, WorldUnit::from_int(2), UnwrappedAngle::ZERO),
                CAMERA,
            ),
            project(
                billboard(1, WorldUnit::from_int(2), UnwrappedAngle::ZERO),
                CAMERA,
            ),
        ];
        sort_far_to_near(&mut values);
        assert_eq!(
            values.map(|value| value.id),
            [BillboardId(3), BillboardId(1), BillboardId(2)]
        );
    }

    #[test]
    fn touch_is_unwrapped_and_rate_limiter_does_not_take_shortest_path() {
        let mut touch = TouchYawAdapter::new(UnwrappedAngle::ZERO);
        touch.sample(0, true);
        assert_eq!(
            touch.sample(320, true),
            UnwrappedAngle::from_units(TURN_UNITS)
        );
        touch.sample(0, false);
        touch.sample(0, true);
        assert_eq!(
            touch.sample(320, true),
            UnwrappedAngle::from_units(TURN_UNITS * 2)
        );

        let mut observed = RateLimitedObservedYaw::new(UnwrappedAngle::ZERO);
        assert_eq!(
            observed.advance(touch.target(), 1_000),
            UnwrappedAngle::from_units(TURN_UNITS / 2)
        );
        assert_eq!(
            observed.advance(touch.target(), 3_000),
            UnwrappedAngle::from_units(TURN_UNITS * 2)
        );

        let mut extreme = TouchYawAdapter::new(UnwrappedAngle::ZERO);
        extreme.sample(i16::MIN, true);
        assert!(extreme.sample(i16::MAX, true).units() > 0);
    }

    #[test]
    fn nearest_alpha_and_bilinear_are_clipped_and_deterministic() {
        let mut framebuffer = std::vec![0x001f; 320 * 240];
        let nearest = ProjectedBillboard {
            id: BillboardId(1),
            screen_rect: ScreenRect {
                x: -1,
                y: -1,
                width: 2,
                height: 2,
            },
            depth: WorldUnit::ONE,
            source: TextureId(1),
            filter: TextureFilter::Nearest,
        };
        let stats = raster_billboard(
            &mut framebuffer,
            320,
            nearest,
            Texture {
                size: SourceSize {
                    width: 1,
                    height: 1,
                },
                pixels: &[0xf800],
                coverage: Coverage::Alpha8 {
                    alpha: &[255],
                    opaque_blocks: Mask8::new(
                        SourceSize {
                            width: 1,
                            height: 1,
                        },
                        &[1],
                    )
                    .unwrap(),
                },
            },
        )
        .unwrap();
        assert_eq!(stats.nearest_samples, 1);
        assert_eq!(framebuffer[0], 0xf800);

        let bilinear_board = ProjectedBillboard {
            screen_rect: ScreenRect {
                x: 1,
                y: 0,
                width: 1,
                height: 1,
            },
            filter: TextureFilter::Bilinear,
            ..nearest
        };
        let stats = raster_billboard(
            &mut framebuffer,
            320,
            bilinear_board,
            Texture {
                size: SourceSize {
                    width: 2,
                    height: 2,
                },
                pixels: &[0, 0xffff, 0xffff, 0],
                coverage: Coverage::Opaque,
            },
        )
        .unwrap();
        assert_eq!(stats.bilinear_samples, 1);
    }

    #[test]
    fn states_map_to_closed_loop_indices_and_frame_counts() {
        assert_eq!(PetAnimationState::Idle.loop_index(), 0);
        assert_eq!(PetAnimationState::MoveRight.loop_index(), 1);
        assert_eq!(PetAnimationState::MoveLeft.loop_index(), 2);
        assert_eq!(PetAnimationState::Attend.loop_index(), 3);
        assert_eq!(PetAnimationState::Idle.frame_count(), 6);
        assert_eq!(PetAnimationState::MoveRight.frame_count(), 8);
        assert_eq!(PetAnimationState::MoveLeft.frame_count(), 8);
        assert_eq!(PetAnimationState::Attend.frame_count(), 6);
    }

    #[test]
    fn movement_advances_at_twenty_frames_per_second_and_wraps() {
        let mut animator = PetAnimator::new();
        assert_eq!(
            animator.set_state(PetAnimationState::MoveRight),
            PetFrame {
                state: PetAnimationState::MoveRight,
                index: 0,
            }
        );
        assert_eq!(animator.advance(49).index, 0);
        assert_eq!(animator.advance(1).index, 1);
        assert_eq!(animator.advance(350).index, 0);
    }

    #[test]
    fn ambient_states_advance_at_ten_frames_per_second() {
        let mut animator = PetAnimator::new();
        assert_eq!(animator.advance(99).index, 0);
        assert_eq!(animator.advance(1).index, 1);
        assert_eq!(
            animator.set_state(PetAnimationState::Attend),
            PetFrame {
                state: PetAnimationState::Attend,
                index: 0,
            }
        );
        assert_eq!(animator.advance(600).index, 0);
    }

    #[test]
    fn changing_state_resets_frame_and_partial_time() {
        let mut animator = PetAnimator::new();
        assert_eq!(animator.advance(150).index, 1);
        assert_eq!(
            animator.set_state(PetAnimationState::MoveLeft),
            PetFrame {
                state: PetAnimationState::MoveLeft,
                index: 0,
            }
        );
        assert_eq!(animator.advance(49).index, 0);
        assert_eq!(animator.advance(1).index, 1);
    }

    #[test]
    fn large_elapsed_values_remain_bounded() {
        let mut animator = PetAnimator::new();
        assert_eq!(
            animator.set_state(PetAnimationState::MoveLeft),
            PetFrame {
                state: PetAnimationState::MoveLeft,
                index: 0,
            }
        );
        let frame = animator.advance(u32::MAX);
        assert_eq!(frame.state, PetAnimationState::MoveLeft);
        assert!(frame.index < PetAnimationState::MoveLeft.frame_count());
    }
}
