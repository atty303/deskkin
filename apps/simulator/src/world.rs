use deskkin_application::{ApplicationViews, availability, synthetic_notice};
use deskkin_presentation::{
    Billboard, BillboardId, CameraPose, CylindricalPose, PixelFormat, ProjectedBillboard,
    RateLimitedObservedYaw, SourceSize, Texture, TextureFilter, TextureId, TouchYawAdapter,
    UnwrappedAngle, WorldUnit, project_billboard, raster_billboard, sort_far_to_near,
};
use slint::{ComponentHandle, Image, Rgb8Pixel, SharedPixelBuffer};

use crate::StatusWindow;

const WIDTH: usize = 320;
const HEIGHT: usize = 240;
const BACKGROUND: u16 = 0x10c3;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct WorldMetrics {
    pub view_generation: u32,
    pub pose_generation: u32,
    pub input_generation: u32,
    pub cache_hits: u32,
    pub cache_misses: u32,
    pub cache_failures: u32,
    pub visible: u16,
    pub culled: u16,
    pub nearest_samples: u32,
    pub bilinear_samples: u32,
}

struct OwnedTexture {
    size: SourceSize,
    pixels: Vec<u16>,
    alpha: Vec<u8>,
    format: PixelFormat,
}

pub(crate) struct WorldScene {
    touch: TouchYawAdapter,
    observed: RateLimitedObservedYaw,
    character_azimuth: UnwrappedAngle,
    object_radius: WorldUnit,
    object_outward: bool,
    availability_cache: [Option<OwnedTexture>; 3],
    notice_cache: Option<OwnedTexture>,
    character: Vec<OwnedTexture>,
    character_frame: usize,
    character_frame_elapsed_ms: u32,
    object: OwnedTexture,
    framebuffer: Vec<u16>,
    metrics: WorldMetrics,
}

impl WorldScene {
    pub(crate) fn new() -> Self {
        Self {
            touch: TouchYawAdapter::new(UnwrappedAngle::ZERO),
            observed: RateLimitedObservedYaw::new(UnwrappedAngle::ZERO),
            character_azimuth: UnwrappedAngle::ZERO,
            object_radius: WorldUnit::from_int(1),
            object_outward: true,
            availability_cache: [None, None, None],
            notice_cache: None,
            character: character_texture(),
            character_frame: 0,
            character_frame_elapsed_ms: 0,
            object: object_texture(),
            framebuffer: vec![BACKGROUND; WIDTH * HEIGHT],
            metrics: WorldMetrics::default(),
        }
    }

    pub(crate) fn touch_sample(&mut self, x: i16, pressed: bool) {
        let before = self.touch.target();
        let after = self.touch.sample(x, pressed);
        if after != before || !pressed {
            self.metrics.input_generation = self.metrics.input_generation.wrapping_add(1).max(1);
        }
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) fn tick(
        &mut self,
        ui: &StatusWindow,
        views: ApplicationViews,
        elapsed_ms: u32,
    ) -> Result<(), String> {
        self.advance(elapsed_ms);
        self.ensure_textures(ui, views)?;
        self.framebuffer.fill(BACKGROUND);

        let mut entities = [None; 4];
        entities[0] = Some((
            Billboard {
                id: BillboardId(1),
                pose: CylindricalPose {
                    radius: WorldUnit::ratio(22, 10),
                    azimuth: self.character_azimuth,
                    height: WorldUnit::ZERO,
                },
                world_height: WorldUnit::ratio(14, 10),
                texture_id: TextureId(1),
                filter: TextureFilter::Nearest,
            },
            self.character[self.character_frame].size,
        ));
        if let Some(surface) = views.availability {
            entities[1] = Some((
                Billboard {
                    id: BillboardId(2),
                    pose: CylindricalPose {
                        radius: WorldUnit::ratio(12, 10),
                        azimuth: UnwrappedAngle::from_degrees(-15),
                        height: WorldUnit::ZERO,
                    },
                    world_height: WorldUnit::ratio(8, 10),
                    texture_id: TextureId(
                        10 + u16::try_from(availability_index(surface)).unwrap_or(0),
                    ),
                    filter: TextureFilter::Bilinear,
                },
                SourceSize {
                    width: 272,
                    height: 124,
                },
            ));
        }
        if views.synthetic_notice.is_some() {
            entities[2] = Some((
                Billboard {
                    id: BillboardId(3),
                    pose: CylindricalPose {
                        radius: WorldUnit::ratio(18, 10),
                        azimuth: UnwrappedAngle::from_degrees(18),
                        height: WorldUnit::ratio(-7, 10),
                    },
                    world_height: WorldUnit::ratio(8, 10),
                    texture_id: TextureId(20),
                    filter: TextureFilter::Bilinear,
                },
                SourceSize {
                    width: 272,
                    height: 124,
                },
            ));
        }
        entities[3] = Some((
            Billboard {
                id: BillboardId(4),
                pose: CylindricalPose {
                    radius: self.object_radius,
                    azimuth: UnwrappedAngle::from_degrees(30),
                    height: WorldUnit::ratio(8, 10),
                },
                world_height: WorldUnit::ratio(5, 10),
                texture_id: TextureId(2),
                filter: TextureFilter::Nearest,
            },
            self.object.size,
        ));

        let camera = CameraPose {
            radius: WorldUnit::from_int(4),
            observed_azimuth: self.observed.observed(),
            height: WorldUnit::ZERO,
        };
        let mut projected = [empty_projected(); 4];
        let mut count = 0;
        self.metrics.culled = 0;
        for (billboard, source) in entities.into_iter().flatten() {
            match project_billboard(billboard, source, camera) {
                Ok(value) => {
                    projected[count] = value;
                    count += 1;
                }
                Err(_) => self.metrics.culled = self.metrics.culled.saturating_add(1),
            }
        }
        sort_far_to_near(&mut projected[..count]);
        self.metrics.visible = u16::try_from(count).unwrap_or(u16::MAX);
        self.metrics.nearest_samples = 0;
        self.metrics.bilinear_samples = 0;
        let mut framebuffer = core::mem::take(&mut self.framebuffer);
        for value in projected[..count].iter().copied() {
            let texture = match self.texture(value.source, views) {
                Ok(texture) => texture,
                Err(error) => {
                    self.framebuffer = framebuffer;
                    return Err(error);
                }
            };
            let stats = match raster_billboard(
                &mut framebuffer,
                WIDTH,
                value,
                Texture {
                    size: texture.size,
                    pixels: &texture.pixels,
                    alpha: &texture.alpha,
                    format: texture.format,
                },
            ) {
                Ok(stats) => stats,
                Err(error) => {
                    self.framebuffer = framebuffer;
                    return Err(format!("world raster: {error:?}"));
                }
            };
            self.metrics.nearest_samples = self
                .metrics
                .nearest_samples
                .saturating_add(stats.nearest_samples);
            self.metrics.bilinear_samples = self
                .metrics
                .bilinear_samples
                .saturating_add(stats.bilinear_samples);
        }
        self.framebuffer = framebuffer;
        ui.set_world_frame(rgb565_image(&self.framebuffer));
        ui.set_world_mode(true);
        self.metrics.pose_generation = self.metrics.pose_generation.wrapping_add(1).max(1);
        Ok(())
    }

    fn advance(&mut self, elapsed_ms: u32) {
        self.observed.advance(self.touch.target(), elapsed_ms);
        let character_step = i64::from(elapsed_ms) * 12 * 65_536 / 360_000;
        self.character_azimuth = self
            .character_azimuth
            .wrapping_add(UnwrappedAngle::from_units(character_step));
        self.character_frame_elapsed_ms =
            self.character_frame_elapsed_ms.saturating_add(elapsed_ms);
        while self.character_frame_elapsed_ms >= 50 {
            self.character_frame_elapsed_ms -= 50;
            self.character_frame = (self.character_frame + 1) % self.character.len();
        }
        let elapsed_ms = i32::try_from(elapsed_ms).unwrap_or(i32::MAX);
        let radial_step = WorldUnit::ratio(elapsed_ms, 4_000).bits();
        let signed_step = if self.object_outward {
            radial_step
        } else {
            -radial_step
        };
        let next = self.object_radius.bits().saturating_add(signed_step);
        let minimum = WorldUnit::from_int(1).bits();
        let maximum = WorldUnit::ratio(25, 10).bits();
        if next >= maximum {
            self.object_radius = WorldUnit::from_bits(maximum);
            self.object_outward = false;
        } else if next <= minimum {
            self.object_radius = WorldUnit::from_bits(minimum);
            self.object_outward = true;
        } else {
            self.object_radius = WorldUnit::from_bits(next);
        }
    }

    fn ensure_textures(
        &mut self,
        ui: &StatusWindow,
        views: ApplicationViews,
    ) -> Result<(), String> {
        if let Some(surface) = views.availability {
            let index = availability_index(surface);
            if self.availability_cache[index].is_none() {
                self.metrics.cache_misses = self.metrics.cache_misses.saturating_add(1);
                self.availability_cache[index] =
                    Some(capture_billboard(ui, false).inspect_err(|_error| {
                        self.metrics.cache_failures = self.metrics.cache_failures.saturating_add(1);
                    })?);
                self.metrics.view_generation = self.metrics.view_generation.wrapping_add(1).max(1);
            } else {
                self.metrics.cache_hits = self.metrics.cache_hits.saturating_add(1);
            }
        }
        if views.synthetic_notice == Some(synthetic_notice::NoticeKind::CompositionCheck) {
            if self.notice_cache.is_none() {
                self.metrics.cache_misses = self.metrics.cache_misses.saturating_add(1);
                self.notice_cache = Some(capture_billboard(ui, true).inspect_err(|_error| {
                    self.metrics.cache_failures = self.metrics.cache_failures.saturating_add(1);
                })?);
                self.metrics.view_generation = self.metrics.view_generation.wrapping_add(1).max(1);
            } else {
                self.metrics.cache_hits = self.metrics.cache_hits.saturating_add(1);
            }
        }
        Ok(())
    }

    fn texture(&self, id: TextureId, views: ApplicationViews) -> Result<&OwnedTexture, String> {
        match id.0 {
            1 => Ok(&self.character[self.character_frame]),
            2 => Ok(&self.object),
            10..=12 => self.availability_cache[(id.0 - 10) as usize]
                .as_ref()
                .ok_or_else(|| "availability texture cache missing".into()),
            20 if views.synthetic_notice.is_some() => self
                .notice_cache
                .as_ref()
                .ok_or_else(|| "notice texture cache missing".into()),
            _ => Err("unknown world texture".into()),
        }
    }
}

fn availability_index(surface: availability::Surface) -> usize {
    match surface {
        availability::Surface::Unknown => 0,
        availability::Surface::Available => 1,
        availability::Surface::Unavailable => 2,
    }
}

fn capture_billboard(ui: &StatusWindow, notice: bool) -> Result<OwnedTexture, String> {
    ui.set_world_mode(false);
    ui.set_capture_notice(notice);
    ui.set_capture_mode(true);
    let snapshot = ui
        .window()
        .take_snapshot()
        .map_err(|error| error.to_string());
    ui.set_capture_mode(false);
    ui.set_world_mode(true);
    let snapshot = snapshot?;
    let mut pixels = Vec::with_capacity(272 * 124);
    for y in 0..124usize {
        for x in 0..272usize {
            let pixel = snapshot.as_slice()[y * WIDTH + x];
            pixels.push(rgb565(pixel.r, pixel.g, pixel.b));
        }
    }
    Ok(OwnedTexture {
        size: SourceSize {
            width: 272,
            height: 124,
        },
        pixels,
        alpha: Vec::new(),
        format: PixelFormat::OpaqueRgb565,
    })
}

fn character_texture() -> Vec<OwnedTexture> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/pets/koyori/move-right.qoi");
    let image = Image::load_from_path(&path)
        .unwrap_or_else(|error| panic!("bundled MoveRight QOI could not be loaded: {error}"));
    let atlas = image
        .to_rgba8()
        .unwrap_or_else(|| panic!("bundled MoveRight QOI is not readable as RGBA8"));
    assert_eq!((atlas.width(), atlas.height()), (1_152, 156));
    (0..8)
        .map(|frame| {
            let mut pixels = Vec::with_capacity(144 * 156);
            let mut alpha = Vec::with_capacity(144 * 156);
            for y in 0..156_usize {
                let row = y * 1_152 + frame * 144;
                for pixel in &atlas.as_slice()[row..row + 144] {
                    pixels.push(rgb565(pixel.r, pixel.g, pixel.b));
                    alpha.push(pixel.a);
                }
            }
            OwnedTexture {
                size: SourceSize {
                    width: 144,
                    height: 156,
                },
                pixels,
                alpha,
                format: PixelFormat::Rgb565A8,
            }
        })
        .collect()
}

fn object_texture() -> OwnedTexture {
    generated_alpha_texture(
        SourceSize {
            width: 32,
            height: 32,
        },
        |x, y| {
            let edge = x < 3 || y < 3 || x > 28 || y > 28;
            (if edge { 0x7d5f } else { 0x39ec }, 255)
        },
    )
}

fn generated_alpha_texture(
    size: SourceSize,
    mut pixel: impl FnMut(u16, u16) -> (u16, u8),
) -> OwnedTexture {
    let mut pixels = Vec::with_capacity(usize::from(size.width) * usize::from(size.height));
    let mut alpha = Vec::with_capacity(pixels.capacity());
    for y in 0..size.height {
        for x in 0..size.width {
            let (color, opacity) = pixel(x, y);
            pixels.push(color);
            alpha.push(opacity);
        }
    }
    OwnedTexture {
        size,
        pixels,
        alpha,
        format: PixelFormat::Rgb565A8,
    }
}

fn rgb565_image(framebuffer: &[u16]) -> Image {
    let mut output = SharedPixelBuffer::<Rgb8Pixel>::new(320, 240);
    for (destination, source) in output.make_mut_slice().iter_mut().zip(framebuffer) {
        destination.r = (((source >> 11) & 0x1f) as u8) * 255 / 31;
        destination.g = (((source >> 5) & 0x3f) as u8) * 255 / 63;
        destination.b = ((source & 0x1f) as u8) * 255 / 31;
    }
    Image::from_rgb8(output)
}

const fn rgb565(red: u8, green: u8, blue: u8) -> u16 {
    ((red as u16 >> 3) << 11) | ((green as u16 >> 2) << 5) | (blue as u16 >> 3)
}

const fn empty_projected() -> ProjectedBillboard {
    ProjectedBillboard {
        id: BillboardId(0),
        screen_rect: deskkin_presentation::ScreenRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        },
        depth: WorldUnit::ZERO,
        source: TextureId(0),
        filter: TextureFilter::Nearest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autonomous_motion_and_drag_are_continuous() {
        let mut scene = WorldScene::new();
        let initial_character = scene.character_azimuth;
        scene.touch_sample(0, true);
        scene.touch_sample(320, true);
        scene.advance(1_000);
        assert!(scene.character_azimuth > initial_character);
        assert_eq!(
            scene.observed.observed(),
            UnwrappedAngle::from_units(65_536 / 2)
        );
        assert!(scene.object_radius > WorldUnit::from_int(1));
    }
}
