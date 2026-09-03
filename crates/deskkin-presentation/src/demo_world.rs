//! Art direction and deterministic motion for the bundled night-garden demo.
//! Rendering primitives remain independent of this scene's entity budget.

use crate::{
    Billboard, BillboardId, CylindricalPose, SourceSize, TextureFilter, TextureId, UnwrappedAngle,
    WorldUnit, sin_q15,
};

pub const BACKGROUND: u16 = 0x10c3;
pub const CAPACITY: usize = 22;
pub const SPRITE_SIZE: SourceSize = SourceSize {
    width: 96,
    height: 96,
};
pub const SPRITE_PIXELS: usize = 96 * 96;
pub const DRONE: TextureId = TextureId(30);
pub const TERRARIUM: TextureId = TextureId(31);
pub const LANTERN: TextureId = TextureId(32);
pub const LIGHT: TextureId = TextureId(33);

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
            filter: if matches!(texture.0, 10..=12 | 20) {
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
        availability.map(|texture| {
            entity(
                2,
                texture,
                WorldUnit::ratio(12, 10),
                UnwrappedAngle::from_degrees(-45),
                WorldUnit::ONE,
                WorldUnit::ratio(8, 10),
                board,
            )
        }),
        notice.then(|| {
            entity(
                3,
                TextureId(20),
                WorldUnit::ratio(18, 10),
                UnwrappedAngle::from_degrees(18),
                WorldUnit::ratio(-11, 10),
                WorldUnit::ratio(8, 10),
                board,
            )
        }),
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
            assert_eq!(entities(motion, None, false).count(), CAPACITY - 2);
            for (index, (billboard, source)) in full.iter().enumerate() {
                assert!(
                    !full[..index]
                        .iter()
                        .any(|(other, _)| other.id == billboard.id)
                );
                assert!(matches!(billboard.texture_id.0, 1 | 10 | 20 | 30..=33));
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
