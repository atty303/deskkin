//! Art direction and deterministic motion for the bundled night-garden demo.
//! Rendering primitives remain independent of this scene's entity budget.

use crate::{
    Billboard, BillboardId, CylindricalPose, RasterError, SourceSize, TextureFilter, TextureId,
    TouchYawAdapter, UnwrappedAngle, VIEWPORT_HEIGHT, VIEWPORT_WIDTH, WorldUnit, sin_q15,
};

// One four-pixel dither period per row: 1,920 flash bytes, not a third framebuffer.
static BACKGROUND_ROWS: [[u16; 4]; VIEWPORT_HEIGHT as usize] = background_rows();

const fn background_rows() -> [[u16; 4]; VIEWPORT_HEIGHT as usize] {
    let stops = [
        (0, [20, 38, 56]),
        (112, [48, 68, 74]),
        (174, [71, 80, 74]),
        (239, [36, 46, 41]),
    ];
    let bayer = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    let mut rows = [[0; 4]; VIEWPORT_HEIGHT as usize];
    let mut y = 0;
    let mut segment = 0;
    while y < rows.len() {
        if y > stops[segment + 1].0 {
            segment += 1;
        }
        let (start_y, start) = stops[segment];
        let (end_y, end) = stops[segment + 1];
        let span = (end_y - start_y) as u32;
        let offset = (y - start_y) as u32;
        let mut x = 0;
        while x < 4 {
            let mut color = 0;
            let mut channel = 0;
            while channel < 3 {
                let bits = if channel == 1 { 6 } else { 5 };
                let shift = [11, 5, 0][channel];
                let value_q8 =
                    (start[channel] * (span - offset) + end[channel] * offset) * 256 / span;
                let quantized = (value_q8 * ((1 << bits) - 1) + bayer[y % 4][x] * 4080) / 65_280;
                color |= (quantized as u16) << shift;
                channel += 1;
            }
            rows[y][x] = color;
            x += 1;
        }
        y += 1;
    }
    rows
}

/// Replaces clear for the demo's packed 320x240 framebuffer. A static vertical
/// gradient suggests ground without geometry or yaw-dependent seams. Wire order
/// matches the `CoreS3` rasterizer; extra trailing storage is left untouched.
pub fn paint_background(pixels: &mut [u16], wire_order: bool) -> Result<(), RasterError> {
    let frame = pixels
        .get_mut(..(VIEWPORT_WIDTH * VIEWPORT_HEIGHT) as usize)
        .ok_or(RasterError::InvalidFramebuffer)?;
    for (row, colors) in frame
        .chunks_exact_mut(VIEWPORT_WIDTH as usize)
        .zip(&BACKGROUND_ROWS)
    {
        let colors = colors.map(|color| if wire_order { color.to_be() } else { color });
        for span in row.chunks_exact_mut(4) {
            span.copy_from_slice(&colors);
        }
    }
    Ok(())
}

pub const CAPACITY: usize = 23;
pub const SPRITE_SIZE: SourceSize = SourceSize {
    width: 96,
    height: 96,
};
pub const SPRITE_PIXELS: usize = 96 * 96;
pub const DRONE: TextureId = TextureId(30);
pub const TERRARIUM: TextureId = TextureId(31);
pub const LANTERN: TextureId = TextureId(32);
pub const LIGHT: TextureId = TextureId(33);

/// Unwrapped slow tour plus direct manipulation; fractional time is retained.
#[derive(Clone, Copy, Debug)]
pub struct DemoCamera {
    touch: TouchYawAdapter,
    orbit: i64,
    remainder: u32,
}

impl DemoCamera {
    #[must_use]
    pub const fn new(initial: UnwrappedAngle) -> Self {
        Self {
            touch: TouchYawAdapter::new(initial),
            orbit: 0,
            remainder: 0,
        }
    }

    pub fn sample(&mut self, x: i16, pressed: bool) -> UnwrappedAngle {
        self.touch.sample(x, pressed);
        self.target()
    }

    pub fn advance(&mut self, elapsed_ms: u32) {
        let numerator = u64::from(elapsed_ms) * 65_536 + u64::from(self.remainder);
        self.orbit = self.orbit.saturating_add((numerator / 120_000) as i64);
        self.remainder = (numerator % 120_000) as u32;
    }

    #[must_use]
    pub fn target(self) -> UnwrappedAngle {
        UnwrappedAngle::from_units(self.touch.target().units().saturating_add(self.orbit))
    }
}

/// Canonical straight-alpha RGBA8 artwork. Consumers convert once into their
/// owned RGB565+A8 storage, never sample these flash-resident bytes per frame.
pub const ARTWORK: [&[u8; SPRITE_PIXELS * 4]; 3] = [
    include_bytes!("../../../assets/world/night-garden/drone.rgba"),
    include_bytes!("../../../assets/world/night-garden/terrarium.rgba"),
    include_bytes!("../../../assets/world/night-garden/lantern.rgba"),
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DemoMotion {
    phase_ms: u32,
}

impl DemoMotion {
    /// All motion periods divide 120 seconds, bounding arithmetic indefinitely
    /// without a visual discontinuity or dependence on update chunk sizes.
    pub fn advance(&mut self, elapsed_ms: u32) {
        self.phase_ms = ((u64::from(self.phase_ms) + u64::from(elapsed_ms)) % 120_000) as u32;
    }

    #[must_use]
    pub fn character_azimuth(self) -> UnwrappedAngle {
        UnwrappedAngle::from_units(i64::from(self.phase_ms % 30_000) * 65_536 / 30_000)
    }

    #[must_use]
    pub fn object_radius(self) -> WorldUnit {
        let phase = (self.phase_ms % 12_000) as i32;
        let triangle = if phase <= 6_000 {
            phase
        } else {
            12_000 - phase
        };
        WorldUnit::from_bits(WorldUnit::ONE.bits() + WorldUnit::ratio(triangle, 4_000).bits())
    }

    fn bob(self, period_ms: u32, offset: i64, amplitude: i32) -> WorldUnit {
        let angle = i64::from(self.phase_ms % period_ms) * 65_536 / i64::from(period_ms);
        WorldUnit::from_bits(sin_q15(angle + offset) * amplitude / 100)
    }
}

fn entity(
    id: u16,
    texture: TextureId,
    radius: WorldUnit,
    azimuth: UnwrappedAngle,
    height: WorldUnit,
    size: WorldUnit,
    source: SourceSize,
) -> (Billboard, SourceSize) {
    (
        Billboard {
            id: BillboardId(id),
            pose: CylindricalPose {
                radius,
                azimuth,
                height,
            },
            world_height: size,
            texture_id: texture,
            filter: if matches!(texture.0, 10..=12 | 20 | 40..=42) {
                TextureFilter::Bilinear
            } else {
                TextureFilter::Nearest
            },
        },
        source,
    )
}

/// Caller provides scratch capacity to the projector; no scene allocation,
/// global registry, hardware clock, or application-feature dependency is used.
pub fn entities(
    motion: DemoMotion,
    availability: Option<TextureId>,
    notice: bool,
) -> impl Iterator<Item = (Billboard, SourceSize)> {
    let board = SourceSize {
        width: 272,
        height: 124,
    };
    let principal = [
        Some(entity(
            1,
            TextureId(1),
            WorldUnit::ratio(22, 10),
            motion.character_azimuth(),
            WorldUnit::ZERO,
            WorldUnit::ratio(14, 10),
            SourceSize {
                width: 144,
                height: 156,
            },
        )),
        Some(entity(
            2,
            availability.unwrap_or(TextureId(40)),
            WorldUnit::ratio(18, 10),
            UnwrappedAngle::from_degrees(-45),
            WorldUnit::ONE,
            WorldUnit::ONE,
            board,
        )),
        Some(entity(
            3,
            if notice { TextureId(20) } else { TextureId(41) },
            WorldUnit::ratio(18, 10),
            UnwrappedAngle::from_degrees(35),
            WorldUnit::ratio(-11, 10),
            WorldUnit::ONE,
            board,
        )),
        Some(entity(
            5,
            TextureId(42),
            WorldUnit::ratio(23, 10),
            UnwrappedAngle::from_degrees(170),
            WorldUnit::ratio(8, 10),
            WorldUnit::ONE,
            board,
        )),
        Some(entity(
            4,
            DRONE,
            motion.object_radius(),
            UnwrappedAngle::from_degrees(42),
            WorldUnit::from_bits(WorldUnit::ONE.bits() + motion.bob(8_000, 0, 20).bits()),
            WorldUnit::ratio(9, 10),
            SPRITE_SIZE,
        )),
    ];
    // Azimuth, radius, center height and full sprite height, in tenths of a unit.
    let props = [
        (-65, 25, -8, 12, TERRARIUM),
        (70, 24, -10, 13, TERRARIUM),
        (175, 27, -9, 14, TERRARIUM),
        (-110, 23, 16, 6, LANTERN),
        (110, 26, 15, 7, LANTERN),
        (5, 15, 15, 5, LANTERN),
    ]
    .into_iter()
    .enumerate()
    .map(move |(index, (azimuth, radius, height, size, texture))| {
        entity(
            10 + index as u16,
            texture,
            WorldUnit::ratio(radius, 10),
            UnwrappedAngle::from_degrees(azimuth),
            WorldUnit::from_bits(
                WorldUnit::ratio(height, 10).bits()
                    + motion.bob(12_000, index as i64 * 10_923, 12).bits(),
            ),
            WorldUnit::ratio(size, 10),
            SPRITE_SIZE,
        )
    });
    let lights = (0..12_u16).map(move |index| {
        let angle = UnwrappedAngle::from_degrees(i64::from(index) * 30).wrapping_add(
            UnwrappedAngle::from_units(i64::from(motion.phase_ms) * 65_536 / 120_000),
        );
        let height = WorldUnit::ratio(i32::from(index % 4) * 9 - 13, 10);
        entity(
            100 + index,
            LIGHT,
            WorldUnit::ratio(16 + i32::from(index % 3) * 4, 10),
            angle,
            WorldUnit::from_bits(
                height.bits() + motion.bob(10_000, i64::from(index) * 5_461, 28).bits(),
            ),
            WorldUnit::ratio(6 + i32::from(index % 3), 100),
            SourceSize {
                width: 9,
                height: 9,
            },
        )
    });
    principal.into_iter().flatten().chain(props).chain(lights)
}

/// Tiny procedural light, not geometry or a per-pixel lighting pass.
#[must_use]
pub fn light_pixel(x: i32, y: i32) -> (u16, u8) {
    let distance = (x - 4) * (x - 4) + (y - 4) * (y - 4);
    let alpha = match distance {
        0..=1 => 255,
        2..=4 => 160,
        5..=9 => 65,
        10..=16 => 18,
        _ => 0,
    };
    (0xff17, alpha)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CameraPose, project_billboard};

    #[test]
    fn background_covers_frame_preserves_guards_and_matches_wire_order() {
        let length = (VIEWPORT_WIDTH * VIEWPORT_HEIGHT) as usize;
        let mut native = std::vec![0xffff; length + 4];
        let mut wire = native.clone();
        paint_background(&mut native[..length - 1], false).unwrap_err();
        assert!(native.iter().all(|&pixel| pixel == 0xffff));
        paint_background(&mut native, false).unwrap();
        paint_background(&mut wire, true).unwrap();
        assert_eq!(&native[length..], &[0xffff; 4]);
        assert_eq!(&wire[length..], &[0xffff; 4]);
        assert!(
            native[..length]
                .iter()
                .all(|&pixel| pixel != 0xffff && pixel != 0)
        );
        for (native, wire) in native[..length].iter().zip(&wire[..length]) {
            assert_eq!(*native, u16::from_be(*wire));
        }
        let before = native.clone();
        paint_background(&mut native, false).unwrap();
        assert_eq!(native, before);
        for row in native[..length].chunks_exact(VIEWPORT_WIDTH as usize) {
            assert!(row.chunks_exact(4).all(|span| span == &row[..4]));
        }
        let channels = |color: u16| {
            [
                (color >> 11) * 255 / 31,
                ((color >> 5) & 63) * 255 / 63,
                (color & 31) * 255 / 31,
            ]
        };
        let top = channels(native[0]);
        let ground = channels(native[174 * VIEWPORT_WIDTH as usize]);
        let bottom = channels(native[length - VIEWPORT_WIDTH as usize]);
        assert!(top[2] > top[0]);
        assert!(bottom[1] > bottom[0] && bottom[1] > bottom[2]);
        assert!(ground.iter().sum::<u16>() > top.iter().sum::<u16>());
        assert!(ground.iter().sum::<u16>() > bottom.iter().sum::<u16>());
    }

    #[test]
    fn camera_tour_preserves_fractional_time_drag_and_multiple_turns() {
        let mut whole = DemoCamera::new(UnwrappedAngle::ZERO);
        let mut split = whole;
        whole.advance(360_000);
        for _ in 0..360_000 {
            split.advance(1);
        }
        assert_eq!(whole.target(), split.target());
        assert_eq!(whole.target().units(), 3 * 65_536);
        split.sample(0, true);
        split.sample(320, true);
        split.sample(320, false);
        split.advance(120_000);
        split.sample(10, true);
        assert_eq!(split.target().units(), 5 * 65_536);
        split.sample(-310, true);
        assert_eq!(split.target().units(), 4 * 65_536);
        whole.advance(u32::MAX);
        assert!(whole.target().units() > 3 * 65_536);
    }

    #[test]
    fn motion_is_chunk_independent_and_periodic() {
        let mut once = DemoMotion::default();
        let mut split = once;
        once.advance(u32::MAX);
        split.advance(u32::MAX - 50);
        split.advance(50);
        assert_eq!(once, split);
        let scene: std::vec::Vec<_> = entities(once, Some(TextureId(10)), true).collect();
        once.advance(120_000);
        assert_eq!(
            scene,
            entities(once, Some(TextureId(10)), true).collect::<std::vec::Vec<_>>()
        );
        let mut boundary = DemoMotion::default();
        boundary.advance(119_999);
        let before = boundary.object_radius().bits();
        boundary.advance(1);
        assert!((before - boundary.object_radius().bits()).abs() <= 17);
    }

    #[test]
    fn scene_budget_identity_projection_and_semantic_views() {
        let mut motion = DemoMotion::default();
        for _ in 0..120 {
            let full: std::vec::Vec<_> = entities(motion, Some(TextureId(10)), true).collect();
            assert_eq!(full.len(), CAPACITY);
            let demo: std::vec::Vec<_> = entities(motion, None, false).collect();
            assert_eq!(demo.len(), CAPACITY);
            assert_eq!(
                demo.iter()
                    .filter(|(b, _)| matches!(b.texture_id.0, 40..=42))
                    .count(),
                3
            );
            assert!(
                !demo
                    .iter()
                    .any(|(b, _)| matches!(b.texture_id.0, 10..=12 | 20))
            );
            for (index, (billboard, source)) in full.iter().enumerate() {
                assert!(
                    !full[..index]
                        .iter()
                        .any(|(other, _)| other.id == billboard.id)
                );
                assert!(matches!(billboard.texture_id.0, 1 | 10 | 20 | 30..=33 | 42));
                for degrees in (0..360).step_by(30) {
                    let camera = CameraPose {
                        radius: WorldUnit::from_int(4),
                        observed_azimuth: UnwrappedAngle::from_degrees(degrees),
                        height: WorldUnit::ZERO,
                    };
                    assert!(!matches!(
                        project_billboard(*billboard, *source, camera),
                        Err(crate::ProjectionCull::InvalidRadius
                            | crate::ProjectionCull::InvalidSource)
                    ));
                }
            }
            motion.advance(1_000);
        }
    }

    #[test]
    fn artwork_has_transparent_and_near_opaque_regions() {
        for bytes in ARTWORK {
            let mut classes = [0_usize; 3];
            for pixel in bytes.chunks_exact(4) {
                classes[match pixel[3] {
                    0 => 0,
                    250..=255 => 1,
                    _ => 2,
                }] += 1;
            }
            assert!(classes[0] > SPRITE_PIXELS / 10);
            assert!(classes[1] > SPRITE_PIXELS / 10);
            assert!(classes[2] > 0);
        }
    }
}
