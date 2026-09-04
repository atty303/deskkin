use deskkin_application::{ApplicationViews, availability, synthetic_notice};
use deskkin_presentation::{
    BillboardId, CameraPose, Coverage, Mask8, Occlusion, ProjectedBillboard,
    RateLimitedObservedYaw, SceneBillboard, ScreenTile, SourceSize, Texture, TextureFilter,
    TextureId, TextureRegion, UnwrappedAngle, WorldUnit, build_opaque_mask,
    demo_world::{self, DemoCamera, DemoMotion},
    raster_scene, sort_far_to_near,
};
use slint::{ComponentHandle, Image, Rgb8Pixel, SharedPixelBuffer};

use crate::StatusWindow;

const WIDTH: usize = 320;
const HEIGHT: usize = 240;

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
    coverage: OwnedCoverage,
}

enum OwnedCoverage {
    Opaque,
    Alpha8 {
        alpha: Vec<u8>,
        opaque_blocks: Vec<u8>,
    },
}

impl OwnedTexture {
    fn with_alpha(size: SourceSize, pixels: Vec<u16>, alpha: Vec<u8>) -> Self {
        let mut opaque_blocks = vec![0; Mask8::bytes_for(size)];
        build_opaque_mask(size, &alpha, &mut opaque_blocks)
            .expect("generated alpha texture dimensions");
        Self {
            size,
            pixels,
            coverage: OwnedCoverage::Alpha8 {
                alpha,
                opaque_blocks,
            },
        }
    }

    fn borrow(&self) -> Texture<'_> {
        Texture {
            size: self.size,
            pixels: &self.pixels,
            coverage: match &self.coverage {
                OwnedCoverage::Opaque => Coverage::Opaque,
                OwnedCoverage::Alpha8 {
                    alpha,
                    opaque_blocks,
                } => Coverage::Alpha8 {
                    alpha,
                    opaque_blocks: Mask8::new(self.size, opaque_blocks)
                        .expect("generated mask dimensions"),
                },
            },
        }
    }
}

pub(crate) struct WorldScene {
    touch: DemoCamera,
    observed: RateLimitedObservedYaw,
    motion: DemoMotion,
    availability_cache: [Option<OwnedTexture>; 3],
    notice_cache: Option<OwnedTexture>,
    demo_cache: [Option<OwnedTexture>; 3],
    character: Vec<OwnedTexture>,
    character_frame: usize,
    character_frame_elapsed_ms: u32,
    decorations: [OwnedTexture; demo_world::DECORATION_TEXTURE_COUNT],
    framebuffer: Vec<u16>,
    cutoffs: Vec<u16>,
    metrics: WorldMetrics,
}

impl WorldScene {
    pub(crate) fn new() -> Self {
        Self {
            touch: DemoCamera::new(UnwrappedAngle::ZERO),
            observed: RateLimitedObservedYaw::new(UnwrappedAngle::ZERO),
            motion: DemoMotion::default(),
            availability_cache: [None, None, None],
            notice_cache: None,
            demo_cache: [None, None, None],
            character: character_texture(),
            character_frame: 0,
            character_frame_elapsed_ms: 0,
            decorations: decoration_textures(),
            framebuffer: vec![0; WIDTH * HEIGHT],
            cutoffs: vec![0; ScreenTile::Eight.cells()],
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

        let camera = CameraPose {
            radius: WorldUnit::from_int(4),
            observed_azimuth: self.observed.observed(),
            height: WorldUnit::ZERO,
        };
        let entities = demo_world::projected_entities(
            self.motion,
            views.availability.map(|surface| {
                TextureId(10 + u16::try_from(availability_index(surface)).unwrap_or(0))
            }),
            views.synthetic_notice.is_some(),
            camera,
        );

        let mut projected = [empty_projected(); demo_world::CAPACITY];
        let mut count = 0;
        self.metrics.culled = 0;
        for projected_entity in entities {
            match projected_entity {
                Ok(value) => {
                    projected[count] = value;
                    count += 1;
                }
                Err(_) => self.metrics.culled = self.metrics.culled.saturating_add(1),
            }
        }
        sort_far_to_near(&mut projected[..count]);
        self.metrics.visible = u16::try_from(count).unwrap_or(u16::MAX);
        let mut framebuffer = core::mem::take(&mut self.framebuffer);
        let mut cutoffs = core::mem::take(&mut self.cutoffs);
        let mut occlusion =
            Occlusion::new(ScreenTile::Eight, &mut cutoffs).expect("screen tile storage");
        let ground = demo_world::HORIZON;
        let draw = (|| {
            let resolve = |value: ProjectedBillboard| {
                let texture = self.texture(value.source, views)?.borrow();
                SceneBillboard::new(
                    value,
                    texture,
                    TextureRegion {
                        source_x: 0,
                        source_y: 0,
                        width: texture.size.width,
                        height: texture.size.height,
                        stride: texture.size.width,
                    },
                )
                .map_err(|error| format!("world texture: {error:?}"))
            };
            if count == 0 {
                return raster_scene(
                    &mut framebuffer,
                    WIDTH,
                    &[],
                    |y| demo_world::background_row(y, ground),
                    false,
                    &mut occlusion,
                )
                .map_err(|error| format!("world raster: {error:?}"));
            }
            let mut scene = [resolve(projected[0])?; demo_world::CAPACITY];
            for (slot, value) in scene.iter_mut().zip(&projected[..count]) {
                *slot = resolve(*value)?;
            }
            raster_scene(
                &mut framebuffer,
                WIDTH,
                &scene[..count],
                |y| demo_world::background_row(y, ground),
                false,
                &mut occlusion,
            )
            .map_err(|error| format!("world raster: {error:?}"))
        })();
        self.framebuffer = framebuffer;
        self.cutoffs = cutoffs;
        let stats = draw?;
        self.metrics.nearest_samples = stats.raster.nearest_samples;
        self.metrics.bilinear_samples = stats.raster.bilinear_samples;
        ui.set_world_frame(rgb565_image(&self.framebuffer));
        ui.set_world_mode(true);
        self.metrics.pose_generation = self.metrics.pose_generation.wrapping_add(1).max(1);
        Ok(())
    }

    fn advance(&mut self, elapsed_ms: u32) {
        self.touch.advance(elapsed_ms);
        self.observed.advance(self.touch.target(), elapsed_ms);
        self.motion.advance(elapsed_ms);
        self.character_frame_elapsed_ms =
            self.character_frame_elapsed_ms.saturating_add(elapsed_ms);
        while self.character_frame_elapsed_ms >= 50 {
            self.character_frame_elapsed_ms -= 50;
            self.character_frame = (self.character_frame + 1) % self.character.len();
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
                    Some(capture_billboard(ui, false, -1).inspect_err(|_error| {
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
                self.notice_cache =
                    Some(capture_billboard(ui, true, -1).inspect_err(|_error| {
                        self.metrics.cache_failures = self.metrics.cache_failures.saturating_add(1);
                    })?);
                self.metrics.view_generation = self.metrics.view_generation.wrapping_add(1).max(1);
            } else {
                self.metrics.cache_hits = self.metrics.cache_hits.saturating_add(1);
            }
        }
        for (index, needed) in [
            views.availability.is_none(),
            views.synthetic_notice.is_none(),
            true,
        ]
        .into_iter()
        .enumerate()
        {
            if !needed {
                continue;
            }
            if self.demo_cache[index].is_none() {
                self.metrics.cache_misses = self.metrics.cache_misses.saturating_add(1);
                self.demo_cache[index] = Some(
                    capture_billboard(ui, false, i32::try_from(index).unwrap_or(0)).inspect_err(
                        |_| {
                            self.metrics.cache_failures =
                                self.metrics.cache_failures.saturating_add(1);
                        },
                    )?,
                );
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
            30..=33 | 50..=67 => {
                Ok(&self.decorations[usize::from(if id.0 < 50 { id.0 - 30 } else { id.0 - 46 })])
            }
            40..=42 => self.demo_cache[usize::from(id.0 - 40)]
                .as_ref()
                .ok_or_else(|| "demo texture cache missing".into()),
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

fn capture_billboard(ui: &StatusWindow, notice: bool, demo: i32) -> Result<OwnedTexture, String> {
    let size = if demo == 2 {
        demo_world::PORTRAIT_CARD
    } else {
        demo_world::LANDSCAPE_CARD
    };
    ui.set_capture_width(i32::from(size.width));
    ui.set_capture_height(i32::from(size.height));
    ui.set_world_mode(false);
    ui.set_capture_notice(notice);
    ui.set_capture_demo(demo);
    ui.set_capture_mode(true);
    let snapshot = ui
        .window()
        .take_snapshot()
        .map_err(|error| error.to_string());
    ui.set_capture_mode(false);
    ui.set_world_mode(true);
    let snapshot = snapshot?;
    let mut pixels = Vec::with_capacity(usize::from(size.width) * usize::from(size.height));
    for y in 0..usize::from(size.height) {
        for x in 0..usize::from(size.width) {
            let pixel = snapshot.as_slice()[y * WIDTH + x];
            pixels.push(rgb565(pixel.r, pixel.g, pixel.b));
        }
    }
    Ok(OwnedTexture {
        size,
        pixels,
        coverage: OwnedCoverage::Opaque,
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
            OwnedTexture::with_alpha(
                SourceSize {
                    width: 144,
                    height: 156,
                },
                pixels,
                alpha,
            )
        })
        .collect()
}

fn decoration_textures() -> [OwnedTexture; demo_world::DECORATION_TEXTURE_COUNT] {
    core::array::from_fn(|index| {
        generated_alpha_texture(demo_world::decoration_size(index), |x, y| {
            demo_world::decoration_pixel(index, x, y)
        })
    })
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
    OwnedTexture::with_alpha(size, pixels, alpha)
}

fn rgb565_image(framebuffer: &[u16]) -> Image {
    let mut output = SharedPixelBuffer::<Rgb8Pixel>::new(320, 240);
    for (destination, source) in output.make_mut_slice().iter_mut().zip(framebuffer) {
        destination.r = u8::try_from(((source >> 11) & 0x1f) * 255 / 31).unwrap_or(255);
        destination.g = u8::try_from(((source >> 5) & 0x3f) * 255 / 63).unwrap_or(255);
        destination.b = u8::try_from((source & 0x1f) * 255 / 31).unwrap_or(255);
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

    fn assert_painter_matches(scene: &WorldScene, views: ApplicationViews) {
        let camera = CameraPose {
            radius: WorldUnit::from_int(4),
            observed_azimuth: scene.observed.observed(),
            height: WorldUnit::ZERO,
        };
        let mut projected: Vec<_> = demo_world::projected_entities(
            scene.motion,
            views
                .availability
                .map(|surface| TextureId(10 + u16::try_from(availability_index(surface)).unwrap())),
            views.synthetic_notice.is_some(),
            camera,
        )
        .filter_map(Result::ok)
        .collect();
        sort_far_to_near(&mut projected);
        let mut expected = vec![0; WIDTH * HEIGHT];
        demo_world::paint_background(&mut expected, false).unwrap();
        let mut samples = 0;
        for value in projected {
            let texture = scene.texture(value.source, views).unwrap().borrow();
            let stats =
                deskkin_presentation::raster_billboard(&mut expected, WIDTH, value, texture)
                    .unwrap();
            samples += stats.nearest_samples + stats.bilinear_samples;
        }
        assert!(expected == scene.framebuffer);
        assert!(scene.metrics.nearest_samples + scene.metrics.bilinear_samples <= samples);
    }

    #[test]
    fn rgb565_expansion_preserves_every_color() {
        let colors: Vec<u16> = (0..=u16::MAX).collect();
        let image = rgb565_image(&colors).to_rgba8().unwrap();
        for (pixel, &color) in image.as_slice().iter().zip(&colors) {
            assert_eq!(rgb565(pixel.r, pixel.g, pixel.b), color);
        }
    }

    #[test]
    fn headless_world_renders_views_and_reuses_textures_after_drag() {
        use slint::platform::{
            Platform, WindowAdapter,
            software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        };
        use std::rc::Rc;
        struct TestPlatform(Rc<MinimalSoftwareWindow>);
        impl Platform for TestPlatform {
            fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
                Ok(self.0.clone())
            }
        }
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(TestPlatform(window.clone()))).unwrap();
        let ui = StatusWindow::new().unwrap();
        window.set_size(slint::PhysicalSize::new(320, 240));
        ui.show().unwrap();
        ui.set_notice_text("Deskkin notice".into());
        let mut scene = WorldScene::new();
        let mut views = ApplicationViews {
            availability: Some(availability::Surface::Unknown),
            synthetic_notice: Some(synthetic_notice::NoticeKind::CompositionCheck),
        };
        scene.tick(&ui, views, 0).unwrap();
        assert_painter_matches(&scene, views);
        assert_eq!(scene.metrics.cache_misses, 3);
        let portrait = scene.demo_cache[2].as_ref().unwrap();
        assert_eq!(portrait.size, demo_world::PORTRAIT_CARD);
        let width = usize::from(portrait.size.width);
        let height = usize::from(portrait.size.height);
        assert_eq!(portrait.pixels.len(), width * height);
        assert_eq!(
            portrait.pixels[(height - 1) * width + width / 2],
            rgb565(0x4d, 0x66, 0x60)
        );
        assert_eq!(
            usize::from(scene.metrics.visible + scene.metrics.culled),
            demo_world::CAPACITY
        );
        assert!(scene.metrics.nearest_samples > 0 && scene.metrics.bilinear_samples > 0);
        assert!(ui.get_world_mode() && !ui.get_capture_mode());
        let initial = scene.framebuffer.clone();
        scene.touch_sample(0, true);
        scene.touch_sample(320, true);
        scene.tick(&ui, views, 500).unwrap();
        assert_painter_matches(&scene, views);
        assert_ne!(initial, scene.framebuffer);
        assert_eq!(scene.metrics.cache_misses, 3);
        assert_eq!(scene.metrics.cache_hits, 3);
        views.synthetic_notice = None;
        scene.tick(&ui, views, 500).unwrap();
        assert_eq!(
            usize::from(scene.metrics.visible + scene.metrics.culled),
            demo_world::CAPACITY
        );
        assert!(scene.metrics.bilinear_samples > 0);
        assert_eq!(scene.metrics.cache_misses, 4);
        views.availability = None;
        scene.tick(&ui, views, 500).unwrap();
        assert_eq!(scene.metrics.cache_misses, 5);
        let misses = scene.metrics.cache_misses;
        for _ in 0..10 {
            scene.tick(&ui, views, 50).unwrap();
        }
        assert_eq!(scene.metrics.cache_misses, misses);
        for _ in 0..12 {
            scene.tick(&ui, views, 10_000).unwrap();
            assert_painter_matches(&scene, views);
        }
    }

    #[test]
    fn autonomous_motion_and_drag_are_continuous() {
        let mut scene = WorldScene::new();
        let initial_character = scene.motion.character_azimuth();
        scene.touch_sample(0, true);
        scene.touch_sample(320, true);
        scene.advance(1_000);
        assert!(scene.motion.character_azimuth() > initial_character);
        assert_eq!(
            scene.observed.observed(),
            UnwrappedAngle::from_units(65_536 / 2)
        );
        assert!(scene.motion.object_radius() > WorldUnit::from_int(1));
    }
}
