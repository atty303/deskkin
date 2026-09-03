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

pub mod demo_world;

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
    if billboard.pose.radius < WorldUnit::ZERO || billboard.pose.radius > WorldUnit::from_int(3) {
        return Err(ProjectionCull::InvalidRadius);
    }
    if source.width == 0 || source.height == 0 {
        return Err(ProjectionCull::InvalidSource);
    }
    let delta = billboard.pose.azimuth.units() - camera.observed_azimuth.units();
    let x = ((i64::from(billboard.pose.radius.bits()) * i64::from(sin_q15(delta))) >> 15) as i32;
    let radial_depth =
        ((i64::from(billboard.pose.radius.bits()) * i64::from(cos_q15(delta))) >> 15) as i32;
    let depth = camera.radius.bits().saturating_sub(radial_depth);
    if depth < WorldUnit::ratio(1, 4).bits() {
        return Err(ProjectionCull::NearPlane);
    }
    let projected_height = ((i64::from(billboard.world_height.bits()) * i64::from(FOCAL_LENGTH))
        / i64::from(depth)) as i32;
    if projected_height <= 0 {
        return Err(ProjectionCull::InvalidSource);
    }
    let projected_width =
        ((i64::from(projected_height) * i64::from(source.width)) / i64::from(source.height)) as i32;
    let center_x =
        VIEWPORT_WIDTH / 2 + ((i64::from(x) * i64::from(FOCAL_LENGTH)) / i64::from(depth)) as i32;
    let vertical = camera
        .height
        .bits()
        .saturating_sub(billboard.pose.height.bits());
    let center_y = VIEWPORT_HEIGHT / 2
        + ((i64::from(vertical) * i64::from(FOCAL_LENGTH)) / i64::from(depth)) as i32;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PixelFormat {
    OpaqueRgb565,
    Rgb565A8,
}

#[derive(Clone, Copy)]
pub struct Texture<'a> {
    pub size: SourceSize,
    pub pixels: &'a [u16],
    pub alpha: &'a [u8],
    pub format: PixelFormat,
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
    if stride < VIEWPORT_WIDTH as usize || framebuffer.len() < stride * VIEWPORT_HEIGHT as usize {
        return Err(RasterError::InvalidFramebuffer);
    }
    let required = usize::from(
        region
            .source_y
            .saturating_add(region.height)
            .saturating_sub(1),
    )
    .saturating_mul(usize::from(region.stride))
    .saturating_add(usize::from(region.source_x.saturating_add(region.width)));
    if region.width == 0
        || region.height == 0
        || region.stride < region.source_x.saturating_add(region.width)
        || texture.pixels.len() < required
        || (texture.format == PixelFormat::Rgb565A8 && texture.alpha.len() < required)
        || projected.screen_rect.width <= 0
        || projected.screen_rect.height <= 0
    {
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
    let mut stats = RasterStats::default();
    for destination_y in top..bottom {
        for destination_x in left..right {
            let relative_x = destination_x - projected.screen_rect.x;
            let relative_y = destination_y - projected.screen_rect.y;
            let source_x_q16 = i64::from(relative_x) * i64::from(region.width) * 65_536
                / i64::from(projected.screen_rect.width);
            let source_y_q16 = i64::from(relative_y) * i64::from(region.height) * 65_536
                / i64::from(projected.screen_rect.height);
            let (color, source_index) = match projected.filter {
                TextureFilter::Nearest => {
                    stats.nearest_samples = stats.nearest_samples.saturating_add(1);
                    let sx = ((source_x_q16 >> 16) as usize).min(usize::from(region.width) - 1)
                        + usize::from(region.source_x);
                    let sy = ((source_y_q16 >> 16) as usize).min(usize::from(region.height) - 1)
                        + usize::from(region.source_y);
                    let index = sy * usize::from(region.stride) + sx;
                    (texture.pixels[index], index)
                }
                TextureFilter::Bilinear => {
                    stats.bilinear_samples = stats.bilinear_samples.saturating_add(1);
                    (
                        bilinear(texture.pixels, region, source_x_q16, source_y_q16),
                        0,
                    )
                }
            };
            let destination_index = destination_y as usize * stride + destination_x as usize;
            let background = if big_endian {
                u16::from_be(framebuffer[destination_index])
            } else {
                framebuffer[destination_index]
            };
            let output = if texture.format == PixelFormat::Rgb565A8 {
                blend_rgb565(background, color, texture.alpha[source_index])
            } else {
                color
            };
            framebuffer[destination_index] = if big_endian { output.to_be() } else { output };
        }
    }
    Ok(stats)
}

fn bilinear(pixels: &[u16], region: TextureRegion, x_q16: i64, y_q16: i64) -> u16 {
    let stride = usize::from(region.stride);
    let x0 =
        ((x_q16 >> 16) as usize).min(usize::from(region.width) - 1) + usize::from(region.source_x);
    let y0 =
        ((y_q16 >> 16) as usize).min(usize::from(region.height) - 1) + usize::from(region.source_y);
    let x1 = (x0 + 1).min(usize::from(region.source_x + region.width - 1));
    let y1 = (y0 + 1).min(usize::from(region.source_y + region.height - 1));
    let fx = (x_q16 & 0xffff) as u32;
    let fy = (y_q16 & 0xffff) as u32;
    let top = interpolate_rgb565(pixels[y0 * stride + x0], pixels[y0 * stride + x1], fx);
    let bottom = interpolate_rgb565(pixels[y1 * stride + x0], pixels[y1 * stride + x1], fx);
    interpolate_rgb565(top, bottom, fy)
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
                alpha: &[255],
                format: PixelFormat::Rgb565A8,
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
                alpha: &[],
                format: PixelFormat::OpaqueRgb565,
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
