//! Art direction and deterministic motion for the bundled night-garden demo.
//! Rendering primitives remain independent of this scene's entity budget.

use crate::{
    Billboard, BillboardId, CylindricalPose, ProjectedBillboard, RasterError, SourceSize,
    TextureFilter, TextureId, TouchYawAdapter, UnwrappedAngle, VIEWPORT_HEIGHT, VIEWPORT_WIDTH,
    WorldUnit, sin_q15,
};

// Separate sky/ground ramps, one four-pixel dither period per row, in flash.
static BACKGROUND_ROWS: [[u16; 4]; VIEWPORT_HEIGHT as usize * 2] = background_rows();
pub const CHARACTER_ID: BillboardId = BillboardId(1);
pub const LANDSCAPE_CARD: SourceSize = SourceSize {
    width: 272,
    height: 124,
};
pub const PORTRAIT_CARD: SourceSize = SourceSize {
    width: 136,
    height: 204,
};

const fn background_rows() -> [[u16; 4]; VIEWPORT_HEIGHT as usize * 2] {
    let stops = [
        (0, [18, 31, 39]),
        (119, [72, 94, 85]),
        (239, [72, 94, 85]),
        (240, [72, 94, 85]),
        (260, [42, 65, 48]),
        (310, [19, 38, 24]),
        (479, [9, 20, 13]),
    ];
    let bayer = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    let mut rows = [[0; 4]; VIEWPORT_HEIGHT as usize * 2];
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

/// Level camera: the ground's vanishing line stays at the viewport center.
pub const HORIZON: usize = VIEWPORT_HEIGHT as usize / 2;

/// Replaces clear for the demo's packed 320x240 framebuffer. The
/// sky and ground meet in a fixed fog band without introducing floor geometry.
/// Wire order matches the `CoreS3` rasterizer; trailing storage is untouched.
pub fn paint_background(pixels: &mut [u16], wire_order: bool) -> Result<(), RasterError> {
    let frame = pixels
        .get_mut(..(VIEWPORT_WIDTH * VIEWPORT_HEIGHT) as usize)
        .ok_or(RasterError::InvalidFramebuffer)?;
    let ground_y = HORIZON;
    for (y, row) in frame.chunks_exact_mut(VIEWPORT_WIDTH as usize).enumerate() {
        let colors =
            background_row(y, ground_y).map(|color| if wire_order { color.to_be() } else { color });
        for span in row.chunks_exact_mut(4) {
            span.copy_from_slice(&colors);
        }
    }
    Ok(())
}

/// Native RGB565 pattern for a viewport row; ground boundary is in 0..=240.
#[must_use]
pub fn background_row(y: usize, ground_y: usize) -> [u16; 4] {
    BACKGROUND_ROWS[if y < ground_y {
        y
    } else {
        VIEWPORT_HEIGHT as usize + y - ground_y
    }]
}

pub const GRASS_COUNT: usize = 176;
pub const BILLBOARD_COUNT: usize = 23;
pub const CAPACITY: usize = BILLBOARD_COUNT + 48 + GRASS_COUNT;
const CAMERA_ORBIT_PERIOD_MS: u64 = 30_000;
pub const DECORATION_TEXTURE_COUNT: usize = 31;
pub const SPRITE_SIZE: SourceSize = SourceSize {
    width: 96,
    height: 96,
};
pub const SPRITE_PIXELS: usize = 96 * 96;
pub const DRONE: TextureId = TextureId(30);
pub const TERRARIUM: TextureId = TextureId(31);
pub const LANTERN: TextureId = TextureId(32);
pub const LIGHT: TextureId = TextureId(33);

/// Unwrapped tour plus direct manipulation; fractional time is retained.
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
        self.orbit = self
            .orbit
            .saturating_add((numerator / CAMERA_ORBIT_PERIOD_MS) as i64);
        self.remainder = (numerator % CAMERA_ORBIT_PERIOD_MS) as u32;
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
            filter: TextureFilter::Nearest,
        },
        source,
    )
}

fn board_entity(
    id: u16,
    texture: TextureId,
    azimuth: UnwrappedAngle,
    height: WorldUnit,
    filter: TextureFilter,
) -> (Billboard, SourceSize) {
    let (mut board, source) = entity(
        id,
        texture,
        WorldUnit::ratio(18, 10),
        azimuth,
        height,
        WorldUnit::ONE,
        LANDSCAPE_CARD,
    );
    board.filter = filter;
    (board, source)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BoardFilters {
    pub availability: TextureFilter,
    pub notice: TextureFilter,
}

impl BoardFilters {
    pub const NEAREST: Self = Self {
        availability: TextureFilter::Nearest,
        notice: TextureFilter::Nearest,
    };
}

/// Caller provides scratch capacity to the projector; no scene allocation,
/// global registry, hardware clock, or application-feature dependency is used.
pub fn entities(
    motion: DemoMotion,
    availability: Option<TextureId>,
    notice: bool,
    filters: BoardFilters,
) -> impl Iterator<Item = (Billboard, SourceSize)> {
    let principal = [
        Some(entity(
            CHARACTER_ID.0,
            TextureId(1),
            WorldUnit::ratio(22, 10),
            motion.character_azimuth(),
            WorldUnit::ratio(-4, 10),
            WorldUnit::ratio(12, 10),
            SourceSize {
                width: 144,
                height: 156,
            },
        )),
        Some(board_entity(
            2,
            availability.unwrap_or(TextureId(40)),
            UnwrappedAngle::from_degrees(-45),
            WorldUnit::ONE,
            filters.availability,
        )),
        Some(board_entity(
            3,
            if notice { TextureId(20) } else { TextureId(41) },
            UnwrappedAngle::from_degrees(35),
            WorldUnit::ratio(8, 10),
            filters.notice,
        )),
        Some(entity(
            5,
            TextureId(42),
            WorldUnit::ratio(23, 10),
            UnwrappedAngle::from_degrees(170),
            WorldUnit::ratio(2, 10),
            WorldUnit::ratio(16, 10),
            PORTRAIT_CARD,
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
        (-65, 25, -4, 12, TERRARIUM),
        (70, 24, -4, 12, TERRARIUM),
        (175, 27, -3, 14, TERRARIUM),
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
                    + if texture == LANTERN {
                        motion.bob(12_000, index as i64 * 10_923, 12).bits()
                    } else {
                        0
                    },
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

pub fn particles() -> impl Iterator<Item = crate::Particle> {
    (0..48_u16)
        .map(|index| {
            let cluster = i32::from(index / 6);
            let kind = usize::from(index % 6);
            crate::Particle {
                id: BillboardId(200 + index),
                pose: CylindricalPose {
                    radius: WorldUnit::ratio([19, 27, 23, 29, 21, 26][kind] - cluster % 3, 10),
                    azimuth: UnwrappedAngle::from_degrees(i64::from(
                        cluster * 45 - 174 + [0, 11, -9, 18, -17, 5][kind] + (cluster % 3) * 3,
                    )),
                    height: WorldUnit::from_int(-1),
                },
                lods: core::array::from_fn(|lod| crate::ParticleLod {
                    texture: TextureId(50 + (kind * 3 + lod) as u16),
                    size: detail_lod_size(lod),
                    max_depth: WorldUnit::from_int([2, 4, 8][lod]),
                }),
            }
        })
        .chain(grass_particles())
}

fn grass_particles() -> impl Iterator<Item = crate::Particle> {
    (0..GRASS_COUNT).map(|index| {
        // One continuous radial rank replaces discrete rings. Wide clumps keep
        // angular coverage while independent radii distribute overlap through
        // depth instead of concentrating it on a few rings.
        let radial_rank = (index * 73 + 41) % GRASS_COUNT;
        let radial_span = (GRASS_COUNT - 1) as i64;
        let inward = radial_span - radial_rank as i64;
        let curve = radial_span.pow(2) - inward.pow(2);
        let inner_radius = WorldUnit::ratio(6, 10).bits();
        let outer_radius = WorldUnit::ratio(32, 10).bits();
        let radius = WorldUnit::from_bits(
            inner_radius
                + (i64::from(outer_radius - inner_radius) * curve / radial_span.pow(2)) as i32,
        );
        let jitter = ((index * index * 17 + index * 109 + 19) % 101) as i32;
        let variant = (radial_rank + index / 7) % 3;
        crate::Particle {
            id: BillboardId(300 + index as u16),
            pose: CylindricalPose {
                radius,
                azimuth: UnwrappedAngle::from_units(
                    index as i64 * 25_031 + i64::from(jitter - 50) * 11,
                ),
                height: WorldUnit::from_int(-1),
            },
            lods: core::array::from_fn(|lod| crate::ParticleLod {
                texture: TextureId(68 + (variant * 3 + lod) as u16),
                size: grass_lod_size(lod),
                max_depth: WorldUnit::from_int([2, 4, 8][lod]),
            }),
        }
    })
}

fn grass_lod_size(lod: usize) -> SourceSize {
    let height = [48, 24, 3][lod];
    SourceSize {
        width: height * 3,
        height,
    }
}

fn grass_blade(variant: usize, blade: i32) -> (i32, i32, i32, usize) {
    let mut bits = (blade as u32)
        .wrapping_add((variant as u32 + 1).wrapping_mul(0x9e37_79b9))
        .wrapping_mul(0x045d_9f3b);
    bits ^= bits >> 16;
    let root = (1 + blade * 2 + (bits % 3) as i32 - 1).clamp(1, 142);
    let edge = root.min(144 - root).min(14);
    let height = (9 + ((bits >> 8) % 35) as i32) * edge / 14;
    let lean = ((bits >> 16) % 5) as i32 - 2;
    let shade = 2 + ((bits >> 24) % 3) as usize;
    (root, height, lean, shade)
}

// Dense, low-contrast tufts: multiple blade tips and a broken moss base keep
// repeated patches from reading as solid rectangles. Generated only at startup.
fn grass_pixel(index: usize, x: u16, y: u16) -> (u16, u8) {
    let variant = (index - 22) / 3;
    let block = [1, 2, 16][(index - 22) % 3];
    let mut shade = 0;
    for dy in 0..block {
        for dx in 0..block {
            let px = i32::from(x) * block + dx;
            let py = i32::from(y) * block + dy;
            let foot = 46 - (px / 7 + variant as i32) % 4;
            let rise = foot - py;
            for blade in 0..71 {
                let (root, height, lean, blade_shade) = grass_blade(variant, blade);
                let stem = root + lean * rise / 12;
                if rise > 0
                    && rise <= height
                    && (px == stem || (rise < height / 3 && px == stem + 1))
                {
                    shade = shade.max(blade_shade);
                }
            }
            let base_height = 23 + (px * 13 + variant as i32 * 7) % 7;
            if (2..142).contains(&px) && rise > 0 && rise <= base_height {
                shade = shade.max(1 + ((px / 3 + py / 2) as usize + variant) % 2);
            }
        }
    }
    match shade {
        1 => (0x19e3, 255),
        2 => (0x2224, 255),
        3 => (0x2ac5, 255),
        4 => (0x3b26, 255),
        _ => (0, 0),
    }
}

pub fn projected_entities(
    motion: DemoMotion,
    availability: Option<TextureId>,
    notice: bool,
    filters: BoardFilters,
    camera: crate::CameraPose,
) -> impl Iterator<Item = Result<ProjectedBillboard, crate::ProjectionCull>> {
    entities(motion, availability, notice, filters)
        .map(move |(billboard, source)| crate::project_billboard(billboard, source, camera))
        .chain(particles().map(move |particle| crate::project_particle(particle, camera)))
}

fn detail_lod_size(lod: usize) -> SourceSize {
    let side = [12, 6, 3][lod];
    SourceSize {
        width: side,
        height: side,
    }
}

// Hand-authored pixel silhouettes: mushroom pair, sedge, flowers, mossy cairn,
// crystal cluster and a trail marker. Dots are transparent; all ink is opaque.
// Small binary-alpha sprites avoid soft-glow overdraw and are cached once.
const DETAILS: [[&[u8; 12]; 12]; 6] = [
    [
        b"............",
        b"............",
        b"...rrr......",
        b"..rhrhr.....",
        b".rrrrrrr....",
        b"...ss....rr.",
        b"...ss...rhrh",
        b"...ss...rrrr",
        b"...ss....s..",
        b"..gssg...s..",
        b".ggggggggg..",
        b"............",
    ],
    [
        b"............",
        b".....g......",
        b"..g..g......",
        b"...g.g...g..",
        b"...ghg..g...",
        b".g..hg.g....",
        b"..g.hhgg....",
        b"...ghhg..g..",
        b"...ghhggg...",
        b"....hhg.....",
        b"...ggggg....",
        b"............",
    ],
    [
        b"............",
        b"..r.........",
        b".rhr....h...",
        b"..r....hhh..",
        b"..g.....h...",
        b"..g..r..g...",
        b"..g.rhr.g...",
        b"..gg.r..g...",
        b"...g.g.gg...",
        b"...ggggg....",
        b"..ggggggg...",
        b"............",
    ],
    [
        b"............",
        b"............",
        b"............",
        b".....ss.....",
        b"....shss....",
        b"....ssss....",
        b"...ssssss...",
        b"..shhsssss..",
        b"..sssssssg..",
        b".ggssssssgg.",
        b".gggggggggg.",
        b"............",
    ],
    [
        b"............",
        b".....h......",
        b"....hcs.....",
        b"....hcs.....",
        b"..h.hcs.....",
        b".hcshcs.h...",
        b".hcshcshcs..",
        b".hcshcshcs..",
        b"..cshcshcs..",
        b"...sccscs...",
        b"..ssssssss..",
        b"............",
    ],
    [
        b"............",
        b".....s......",
        b"..sssssss...",
        b"..shhhssss..",
        b"..sssssss...",
        b".....s......",
        b".....s......",
        b".....s......",
        b".....s......",
        b"....gsg.....",
        b"...ggsgg....",
        b"............",
    ],
];

#[must_use]
pub fn decoration_size(index: usize) -> SourceSize {
    match index {
        0..=2 => SPRITE_SIZE,
        3 => SourceSize {
            width: 9,
            height: 9,
        },
        4..=21 => detail_lod_size((index - 4) % 3),
        _ => grass_lod_size((index - 22) % 3),
    }
}

/// Called only during texture creation; index is in `0..DECORATION_TEXTURE_COUNT`.
#[must_use]
pub fn decoration_pixel(index: usize, x: u16, y: u16) -> (u16, u8) {
    match index {
        0..=2 => {
            let rgba = ARTWORK[index];
            let offset = (usize::from(y) * 96 + usize::from(x)) * 4;
            (
                ((u16::from(rgba[offset]) >> 3) << 11)
                    | ((u16::from(rgba[offset + 1]) >> 2) << 5)
                    | (u16::from(rgba[offset + 2]) >> 3),
                rgba[offset + 3],
            )
        }
        3 => light_pixel(i32::from(x), i32::from(y)),
        22.. => grass_pixel(index, x, y),
        _ => {
            let detail = (index - 4) / 3;
            let block = 1 << ((index - 4) % 3);
            // Keep thin stems and petals at distant LODs. Choose the brightest
            // occupied texel in each block once during cache creation.
            let mut ink = b'.';
            for dy in 0..block {
                for dx in 0..block {
                    let candidate =
                        DETAILS[detail][usize::from(y) * block + dy][usize::from(x) * block + dx];
                    if candidate != b'.' && (ink == b'.' || candidate == b'h') {
                        ink = candidate;
                    }
                }
            }
            let color = match ink {
                b'g' => 0x5ba9, // moss
                b'r' => 0xcaad, // coral
                b'h' => 0xef56, // warm ivory
                b's' => 0x7c0e, // stone / stems
                b'c' => 0x65d7, // turquoise
                _ => return (0, 0),
            };
            (color, 255)
        }
    }
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
        let sky = channels(native[119 * VIEWPORT_WIDTH as usize]);
        let ground = channels(native[120 * VIEWPORT_WIDTH as usize]);
        let bottom = channels(native[length - VIEWPORT_WIDTH as usize]);
        assert!(top[2] > top[0]);
        assert!(bottom[1] > bottom[0] && bottom[1] > bottom[2]);
        assert!(sky.iter().zip(ground).all(|(a, b)| a.abs_diff(b) <= 9));
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
        assert_eq!(whole.target().units(), 12 * 65_536);
        split.sample(0, true);
        split.sample(320, true);
        split.sample(320, false);
        split.advance(120_000);
        split.sample(10, true);
        assert_eq!(split.target().units(), 17 * 65_536);
        split.sample(-310, true);
        assert_eq!(split.target().units(), 16 * 65_536);
        whole.advance(u32::MAX);
        assert!(whole.target().units() > 12 * 65_536);
    }

    #[test]
    fn motion_is_chunk_independent_and_periodic() {
        let mut once = DemoMotion::default();
        let mut split = once;
        once.advance(u32::MAX);
        split.advance(u32::MAX - 50);
        split.advance(50);
        assert_eq!(once, split);
        let scene: std::vec::Vec<_> =
            entities(once, Some(TextureId(10)), true, BoardFilters::NEAREST).collect();
        once.advance(120_000);
        assert_eq!(
            scene,
            entities(once, Some(TextureId(10)), true, BoardFilters::NEAREST)
                .collect::<std::vec::Vec<_>>()
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
            let filters = BoardFilters {
                availability: TextureFilter::Bilinear,
                notice: TextureFilter::Bilinear,
            };
            let full: std::vec::Vec<_> =
                entities(motion, Some(TextureId(10)), true, filters).collect();
            let cards: std::vec::Vec<_> = full
                .iter()
                .filter(|(b, _)| b.filter == TextureFilter::Bilinear)
                .collect();
            assert_eq!(cards.len(), 2);
            assert_eq!(
                cards
                    .iter()
                    .filter(|(_, size)| *size == PORTRAIT_CARD)
                    .count(),
                0
            );
            assert_eq!(
                cards
                    .iter()
                    .filter(|(_, size)| *size == LANDSCAPE_CARD)
                    .count(),
                2
            );
            assert_eq!(full.len() + particles().count(), CAPACITY);
            let demo: std::vec::Vec<_> =
                entities(motion, None, false, BoardFilters::NEAREST).collect();
            assert_eq!(demo.len() + particles().count(), CAPACITY);
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
                assert!(matches!(billboard.texture_id.0, 1 | 10 | 20 | 30..=39 | 42));
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
    fn particle_lods_are_native_sized_and_keep_the_ground_anchor() {
        for particle in particles() {
            for (depth, lod) in [(2, 0), (3, 1), (4, 1), (5, 2), (8, 2)] {
                let particle = crate::Particle {
                    pose: CylindricalPose {
                        radius: WorldUnit::ZERO,
                        ..particle.pose
                    },
                    ..particle
                };
                let camera = CameraPose {
                    radius: WorldUnit::from_int(depth),
                    height: WorldUnit::ZERO,
                    observed_azimuth: UnwrappedAngle::ZERO,
                };
                let p = crate::project_particle(particle, camera).unwrap();
                let index = usize::from(p.source.0 - 46);
                let size = decoration_size(index);
                assert_eq!(p.source, particle.lods[lod].texture);
                assert_eq!(p.screen_rect.width, i32::from(size.width));
                assert_eq!(p.screen_rect.height, i32::from(size.height));
                assert_eq!(
                    p.screen_rect.y + p.screen_rect.height,
                    120 + 160 / i32::from(depth)
                );
                assert_eq!(p.filter, TextureFilter::Nearest);
                let pixels: std::vec::Vec<_> = (0..size.height)
                    .flat_map(|y| (0..size.width).map(move |x| decoration_pixel(index, x, y)))
                    .collect();
                assert!(pixels.iter().any(|p| p.1 == 255));
                assert!(pixels.iter().all(|p| p.1 == 0 || p.1 == 255));
            }
        }
    }

    #[test]
    fn grass_breaks_angular_spokes_radial_rings_and_five_blade_repetition() {
        let particles: std::vec::Vec<_> = grass_particles().collect();
        let mut azimuths: std::vec::Vec<_> = particles
            .iter()
            .map(|particle| particle.pose.azimuth.units().rem_euclid(65_536))
            .collect();
        azimuths.sort_unstable();
        let maximum_gap = azimuths
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .chain(core::iter::once(
                azimuths[0] + 65_536 - azimuths[azimuths.len() - 1],
            ))
            .max()
            .unwrap();
        assert_eq!(azimuths.len(), GRASS_COUNT);
        assert!(maximum_gap <= 1_600, "grass angular gap: {maximum_gap}");

        let mut radii: std::vec::Vec<_> = particles
            .iter()
            .map(|particle| particle.pose.radius.bits())
            .collect();
        radii.sort_unstable();
        radii.dedup();
        assert_eq!(radii.len(), GRASS_COUNT);
        assert_eq!(radii[0], WorldUnit::ratio(6, 10).bits());
        assert_eq!(radii[GRASS_COUNT - 1], WorldUnit::ratio(32, 10).bits());
        for particle in particles {
            assert!(!matches!(
                crate::project_particle(
                    particle,
                    CameraPose {
                        radius: WorldUnit::from_int(4),
                        observed_azimuth: UnwrappedAngle::ZERO,
                        height: WorldUnit::ZERO,
                    },
                ),
                Err(crate::ProjectionCull::InvalidRadius)
            ));
        }

        for variant in 0..3 {
            for blade in 7..35 {
                let current = grass_blade(variant, blade);
                let repeated = grass_blade(variant, blade + 5);
                assert_ne!((current.1, current.2), (repeated.1, repeated.2));
            }
        }
    }

    #[test]
    fn small_details_bound_full_turn_raster_work() {
        let mut max_pixels = 0;
        for degrees in 0..360 {
            let camera = CameraPose {
                radius: WorldUnit::from_int(4),
                height: WorldUnit::ZERO,
                observed_azimuth: UnwrappedAngle::from_degrees(degrees),
            };
            let pixels: i32 = particles()
                .filter_map(|particle| crate::project_particle(particle, camera).ok())
                .map(|p| p.screen_rect.width * p.screen_rect.height)
                .sum();
            max_pixels = max_pixels.max(pixels);
        }
        // Counts transparent texels too; clipping and occlusion can only reduce it.
        assert!(max_pixels <= 320_000, "detail pixel budget: {max_pixels}");
        std::println!("maximum detail bounding-box pixels per frame: {max_pixels}");
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
