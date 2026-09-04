// SPDX-License-Identifier: GPL-3.0-only

#![no_std]

extern crate alloc;
extern crate zephyr;

use alloc::{boxed::Box, rc::Rc, vec, vec::Vec};
use core::{cell::RefCell, ffi::c_int, time::Duration};
use deskkin_presentation::{
    BandTarget, BillboardId, CameraPose, ColumnSample, Coverage, Mask8, Occlusion,
    PetAnimationState, PetAnimator, PreparedBoard, PreparedScene, ProjectedBillboard, RasterPhase,
    SceneBillboard, ScreenRect, ScreenTile, SourceSize, Texture, TextureFilter, TextureId,
    TextureRegion, UnwrappedAngle, WorldUnit, build_opaque_mask,
    demo_world::{self, DemoMotion},
    sort_far_to_near,
};
use qoi::{Channels, Decoder};
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType, Rgb565Pixel, TargetPixel,
};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, Image, LogicalPosition, PhysicalSize, Rgba8Pixel, SharedPixelBuffer};

slint::include_modules!();

mod background;
mod band_buffer;
mod blit;
mod buffer_ownership;
mod texture_storage;

use band_buffer::{BandCompletion, Framebuffer, ShellBands};
use texture_storage::{Alpha, Colors, row_stride};

const WIDTH: usize = 320;
const HEIGHT: usize = 240;
const BUFFER_COUNT: usize = 2;
const BAND_ROWS: usize = 32;
const BAND_PIXELS: usize = WIDTH * BAND_ROWS;
const PET_FRAME_WIDTH: u32 = 144;
const PET_FRAME_HEIGHT: u32 = 156;

struct LoopAsset {
    bytes: &'static [u8],
    state: PetAnimationState,
}

impl LoopAsset {
    const fn for_state(state: PetAnimationState) -> Self {
        let bytes: &'static [u8] = match state {
            PetAnimationState::Idle => {
                include_bytes!("../../../../assets/pets/koyori/idle.qoi")
            }
            PetAnimationState::MoveRight => {
                include_bytes!("../../../../assets/pets/koyori/move-right.qoi")
            }
            PetAnimationState::MoveLeft => {
                include_bytes!("../../../../assets/pets/koyori/move-left.qoi")
            }
            PetAnimationState::Attend => {
                include_bytes!("../../../../assets/pets/koyori/attend.qoi")
            }
        };
        Self { bytes, state }
    }

    const fn width(&self) -> u32 {
        PET_FRAME_WIDTH * self.state.frame_count() as u32
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct WorldSnapshot {
    magic: u32,
    generation: u32,
    observed_yaw: i64,
    sas: u32,
    schema: u8,
    shell: u8,
    availability: u8,
    notice: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TouchSample {
    publication: u32,
    generation: u32,
    x: i16,
    y: i16,
    pressed: u8,
    schema: u8,
    reserved: [u8; 2],
}

unsafe extern "C" {
    fn deskkin_renderer_entry_probe();
    fn deskkin_framebuffer_alloc(index: u8) -> *mut u16;
    fn deskkin_display_submit(buffer_index: u8, y: u16, rows: u16) -> c_int;
    fn deskkin_display_take_completion(completion: *mut BandCompletion) -> c_int;
    fn deskkin_display_enable() -> c_int;
    fn deskkin_renderer_observe(stage: u8, fault: u8, render_us: u32, transfer_us: u32);
    fn deskkin_renderer_progress(stage: u8);
    fn deskkin_uptime_us() -> u64;
    fn deskkin_yield();
    fn deskkin_world_snapshot(output: *mut WorldSnapshot) -> c_int;
    fn deskkin_touch_read(
        after_generation: u32,
        output: *mut TouchSample,
        drop_count: *mut u32,
    ) -> c_int;
    fn deskkin_publish_target_yaw(value: i64);
    fn deskkin_publish_ui_command(command: u8);
    fn deskkin_world_observe(
        generation: u32,
        input_generation: u32,
        touch_drops: u32,
        cache_hits: u16,
        cache_misses: u16,
        cache_failures: u16,
        visible: u8,
        culled: u8,
        nearest: u32,
        bilinear: u32,
        projection_us: u32,
        sort_us: u32,
        texture_us: u32,
        raster_us: u32,
    );
    fn deskkin_deadline_missed();
    fn deskkin_shell_observe(shell: u8, property_matches: u8);
    fn deskkin_raster_profile(values: *const u32);
    fn deskkin_blit_cycles() -> u32;
}

#[repr(u8)]
enum RendererProgress {
    Loop = 1,
    Snapshot = 2,
    Touch = 3,
    Texture = 4,
    Buffer = 5,
    Raster = 6,
    Submit = 7,
    Pacing = 8,
}

struct DevicePlatform {
    windows: Rc<RefCell<Vec<Rc<MinimalSoftwareWindow>>>>,
}

impl Platform for DevicePlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        self.windows.borrow_mut().push(window.clone());
        Ok(window)
    }

    fn duration_since_start(&self) -> Duration {
        Duration::from_micros(unsafe { deskkin_uptime_us() })
    }
}

#[repr(transparent)]
#[derive(Clone, Copy, Default)]
struct Rgb565BePixel(u16);

impl TargetPixel for Rgb565BePixel {
    fn blend(&mut self, color: PremultipliedRgbaColor) {
        let mut native = Rgb565Pixel(u16::from_be(self.0));
        native.blend(color);
        self.0 = native.0.to_be();
    }

    fn from_rgb(red: u8, green: u8, blue: u8) -> Self {
        let native = <Rgb565Pixel as TargetPixel>::from_rgb(red, green, blue);
        Self(native.0.to_be())
    }
}

#[repr(u8)]
#[derive(Clone, Copy)]
enum RendererFault {
    None = 0,
    Platform = 1,
    Component = 2,
    Window = 3,
    Show = 4,
    Framebuffer = 5,
    Completion = 6,
    Ownership = 7,
    RenderSkipped = 8,
    Submit = 9,
    DisplayEnable = 10,
    QoiHeader = 14,
    QoiMetadata = 15,
    QoiDecode = 16,
    SharedSnapshot = 17,
    BackgroundCheck = 18,
    BlitCheck = 19,
}

#[repr(u8)]
#[derive(Clone, Copy)]
enum RendererStage {
    Rendering = 2,
    Presented = 4,
    Failed = 5,
    AssetLoading = 6,
    AssetReady = 7,
}

fn elapsed_us(start: u64, end: u64) -> u32 {
    end.saturating_sub(start).try_into().unwrap_or(u32::MAX)
}

struct DecodedLoop {
    image: Image,
    pixels: Colors,
    alpha: Alpha,
    opaque_blocks: Vec<u8>,
    stride: u16,
}

fn decode_loop(asset: LoopAsset) -> Result<DecodedLoop, RendererFault> {
    let decoder = Decoder::new(asset.bytes).map_err(|_| RendererFault::QoiHeader)?;
    let header = *decoder.header();
    if header.width != asset.width()
        || header.height != PET_FRAME_HEIGHT
        || header.channels != Channels::Rgba
    {
        return Err(RendererFault::QoiMetadata);
    }
    let mut pixels = SharedPixelBuffer::<Rgba8Pixel>::new(header.width, header.height);
    decoder
        .with_channels(Channels::Rgba)
        .decode_to_buf(pixels.make_mut_bytes())
        .map_err(|_| RendererFault::QoiDecode)?;
    let width = u16::try_from(header.width).map_err(|_| RendererFault::QoiMetadata)?;
    let stride = row_stride(width);
    let length = usize::from(stride) * header.height as usize;
    let sample = |i: usize| {
        let x = i % usize::from(stride);
        if x < usize::from(width) {
            pixels.as_slice()[i / usize::from(stride) * usize::from(width) + x]
        } else {
            Rgba8Pixel::default()
        }
    };
    let rgb565 = Colors::from_fn(length, |i| {
        let pixel = sample(i);
        (u16::from(pixel.r >> 3) << 11) | (u16::from(pixel.g >> 2) << 5) | u16::from(pixel.b >> 3)
    });
    let alpha = Alpha::from_fn(length, |i| sample(i).a);
    Ok(DecodedLoop {
        image: Image::from_rgba8(pixels),
        pixels: rgb565,
        opaque_blocks: alpha_mask(
            SourceSize {
                width: stride,
                height: header.height as u16,
            },
            &alpha,
        ),
        alpha,
        stride,
    })
}

fn replace_loop(
    component: &RendererWindow,
    decoded: &mut Option<DecodedLoop>,
    state: PetAnimationState,
) -> Result<(), RendererFault> {
    unsafe {
        deskkin_renderer_observe(
            RendererStage::AssetLoading as u8,
            RendererFault::None as u8,
            0,
            0,
        )
    };
    component.set_pet_atlas(Image::default());
    let next = decode_loop(LoopAsset::for_state(state))?;
    component.set_pet_atlas(next.image.clone());
    *decoded = Some(next);
    component.set_pet_frame_index(0);
    unsafe {
        deskkin_renderer_observe(
            RendererStage::AssetReady as u8,
            RendererFault::None as u8,
            0,
            0,
        )
    };
    Ok(())
}

fn render_frame(
    window: &MinimalSoftwareWindow,
    framebuffer: &mut Framebuffer,
) -> Result<(), RendererFault> {
    slint::platform::update_timers_and_animations();
    framebuffer.begin_frame()?;
    let mut bands = ShellBands {
        framebuffer,
        next_line: 0,
        fault: None,
    };
    let rendered = window.draw_if_needed(|renderer| {
        renderer.render_by_line(&mut bands);
    });
    if let Some(fault) = bands.fault {
        return Err(fault);
    }
    if !rendered || bands.next_line != HEIGHT {
        return Err(RendererFault::RenderSkipped);
    }
    Ok(())
}

struct BillboardTexture {
    size: SourceSize,
    pixels: Colors,
}

fn capture_billboard(
    component: &RendererWindow,
    window: &MinimalSoftwareWindow,
    notice: bool,
    demo: i32,
    text: &str,
    color: slint::Color,
) -> Result<BillboardTexture, RendererFault> {
    let size = if demo == 2 {
        demo_world::PORTRAIT_CARD
    } else {
        demo_world::LANDSCAPE_CARD
    };
    component.set_capture_width(i32::from(size.width));
    component.set_capture_height(i32::from(size.height));
    component.set_capture_notice(notice);
    component.set_capture_demo(demo);
    component.set_capture_status_text(text.into());
    component.set_capture_status_color(color);
    component.set_capture_mode(true);
    window.set_size(PhysicalSize::new(
        u32::from(size.width),
        u32::from(size.height),
    ));
    window.request_redraw();
    let mut pixels = vec![Rgb565Pixel(0); usize::from(size.width) * usize::from(size.height)];
    let rendered = window.draw_if_needed(|renderer| {
        let _ = renderer.render(&mut pixels, usize::from(size.width));
    });
    component.set_capture_mode(false);
    window.set_size(PhysicalSize::new(WIDTH as u32, HEIGHT as u32));
    window.request_redraw();
    if !rendered {
        return Err(RendererFault::RenderSkipped);
    }
    Ok(BillboardTexture {
        size,
        pixels: Colors::from_fn(
            usize::from(row_stride(size.width)) * usize::from(size.height),
            |i| {
                let stride = usize::from(row_stride(size.width));
                let x = i % stride;
                if x < usize::from(size.width) {
                    pixels[i / stride * usize::from(size.width) + x].0
                } else {
                    0
                }
            },
        ),
    })
}

struct DecorationTexture {
    size: SourceSize,
    pixels: Colors,
    alpha: Alpha,
    opaque_blocks: Vec<u8>,
}

struct WorldTextures {
    availability: [Option<BillboardTexture>; 3],
    notice: Option<BillboardTexture>,
    demo: [Option<BillboardTexture>; 3],
    decorations: Vec<DecorationTexture>,
}

struct WorldTelemetry {
    cache_hits: u16,
    cache_misses: u16,
    cache_failures: u16,
    texture_us: u32,
    touch_drops: u32,
}

fn new_world_textures() -> WorldTextures {
    WorldTextures {
        availability: [None, None, None],
        notice: None,
        demo: [None, None, None],
        decorations: (0..demo_world::DECORATION_TEXTURE_COUNT)
            .map(|index| {
                let size = demo_world::decoration_size(index);
                let storage_size = SourceSize {
                    width: row_stride(size.width),
                    height: size.height,
                };
                let length = usize::from(storage_size.width) * usize::from(size.height);
                let sample = |i: usize| {
                    let x = (i % usize::from(storage_size.width)) as u16;
                    let y = (i / usize::from(storage_size.width)) as u16;
                    if x < size.width {
                        demo_world::decoration_pixel(index, x, y)
                    } else {
                        (0, 0)
                    }
                };
                let pixels = Colors::from_fn(length, |i| sample(i).0);
                let alpha = Alpha::from_fn(length, |i| sample(i).1);
                DecorationTexture {
                    size,
                    pixels,
                    opaque_blocks: alpha_mask(storage_size, &alpha),
                    alpha,
                }
            })
            .collect(),
    }
}

fn ensure_world_textures(
    textures: &mut WorldTextures,
    component: &RendererWindow,
    window: &MinimalSoftwareWindow,
    snapshot: WorldSnapshot,
    telemetry: &mut WorldTelemetry,
) -> Result<(), RendererFault> {
    let started = unsafe { deskkin_uptime_us() };
    if snapshot.availability != 0 {
        let index = usize::from(snapshot.availability.saturating_sub(1).min(2));
        if textures.availability[index].is_none() {
            telemetry.cache_misses = telemetry.cache_misses.saturating_add(1);
            let (text, color) = match index {
                1 => ("Available", slint::Color::from_rgb_u8(0x3d, 0xd6, 0x8c)),
                2 => ("Unavailable", slint::Color::from_rgb_u8(0xf2, 0x5f, 0x5c)),
                _ => ("Unknown", slint::Color::from_rgb_u8(0xf3, 0xb3, 0x3d)),
            };
            match capture_billboard(component, window, false, -1, text, color) {
                Ok(texture) => textures.availability[index] = Some(texture),
                Err(error) => {
                    telemetry.cache_failures = telemetry.cache_failures.saturating_add(1);
                    return Err(error);
                }
            }
        } else {
            telemetry.cache_hits = telemetry.cache_hits.saturating_add(1);
        }
    }
    if snapshot.notice != 0 {
        if textures.notice.is_none() {
            telemetry.cache_misses = telemetry.cache_misses.saturating_add(1);
            match capture_billboard(
                component,
                window,
                true,
                -1,
                "Unknown",
                slint::Color::from_rgb_u8(0xf3, 0xb3, 0x3d),
            ) {
                Ok(texture) => textures.notice = Some(texture),
                Err(error) => {
                    telemetry.cache_failures = telemetry.cache_failures.saturating_add(1);
                    return Err(error);
                }
            }
        } else {
            telemetry.cache_hits = telemetry.cache_hits.saturating_add(1);
        }
    }
    for (index, needed) in [snapshot.availability == 0, snapshot.notice == 0, true]
        .into_iter()
        .enumerate()
    {
        if !needed {
            continue;
        }
        if textures.demo[index].is_none() {
            telemetry.cache_misses = telemetry.cache_misses.saturating_add(1);
            match capture_billboard(
                component,
                window,
                false,
                index as i32,
                "",
                slint::Color::default(),
            ) {
                Ok(texture) => textures.demo[index] = Some(texture),
                Err(error) => {
                    telemetry.cache_failures = telemetry.cache_failures.saturating_add(1);
                    return Err(error);
                }
            }
        } else {
            telemetry.cache_hits = telemetry.cache_hits.saturating_add(1);
        }
    }
    telemetry.texture_us = elapsed_us(started, unsafe { deskkin_uptime_us() });
    Ok(())
}

struct WorldRasterScratch {
    cutoffs: Vec<u16>,
    prepared: Vec<PreparedBoard>,
    columns: Vec<ColumnSample>,
}

struct WorldMotion {
    scene: DemoMotion,
    updated_at_us: u64,
    raster: WorldRasterScratch,
    projected: Vec<ProjectedBillboard>,
}

impl WorldMotion {
    fn new(now_us: u64) -> Self {
        Self {
            scene: DemoMotion::default(),
            updated_at_us: now_us,
            raster: WorldRasterScratch {
                cutoffs: vec![0; ScreenTile::Eight.cells()],
                prepared: vec![PreparedBoard::default(); demo_world::CAPACITY],
                columns: vec![ColumnSample::default(); WIDTH * demo_world::BILLBOARD_COUNT],
            },
            projected: vec![empty_projected(); demo_world::CAPACITY],
        }
    }

    fn advance(&mut self, now_us: u64) {
        let elapsed_ms = now_us.saturating_sub(self.updated_at_us) / 1_000;
        // Preserve sub-millisecond remainder rather than losing it every frame.
        self.updated_at_us = self.updated_at_us.saturating_add(elapsed_ms * 1_000);
        self.scene.advance((elapsed_ms % 120_000) as u32);
    }
}

fn alpha_mask(size: SourceSize, alpha: &[u8]) -> Vec<u8> {
    let mut bits = vec![0; Mask8::bytes_for(size)];
    build_opaque_mask(size, alpha, &mut bits).expect("validated alpha texture dimensions");
    bits
}

fn world_billboard<'a>(
    value: ProjectedBillboard,
    decoded: &'a DecodedLoop,
    frame_index: u8,
    textures: &'a WorldTextures,
) -> Result<SceneBillboard<'a>, RendererFault> {
    let (texture, source_size) = match value.source.0 {
        1 => {
            let size = SourceSize {
                width: decoded.stride,
                height: PET_FRAME_HEIGHT as u16,
            };
            (
                Texture {
                    size,
                    pixels: &decoded.pixels,
                    coverage: Coverage::Alpha8 {
                        alpha: &decoded.alpha,
                        opaque_blocks: Mask8::new(size, &decoded.opaque_blocks)
                            .map_err(|_| RendererFault::RenderSkipped)?,
                    },
                },
                SourceSize {
                    width: PET_FRAME_WIDTH as u16,
                    height: PET_FRAME_HEIGHT as u16,
                },
            )
        }
        30..=33 | 50..=76 => {
            let texture = &textures.decorations[usize::from(if value.source.0 < 50 {
                value.source.0 - 30
            } else {
                value.source.0 - 46
            })];
            (
                Texture {
                    size: SourceSize {
                        width: row_stride(texture.size.width),
                        height: texture.size.height,
                    },
                    pixels: &texture.pixels,
                    coverage: Coverage::Alpha8 {
                        alpha: &texture.alpha,
                        opaque_blocks: Mask8::new(
                            SourceSize {
                                width: row_stride(texture.size.width),
                                height: texture.size.height,
                            },
                            &texture.opaque_blocks,
                        )
                        .map_err(|_| RendererFault::RenderSkipped)?,
                    },
                },
                texture.size,
            )
        }
        10..=12 | 20 | 40..=42 => {
            let texture = match value.source.0 {
                10..=12 => &textures.availability[usize::from(value.source.0 - 10)],
                20 => &textures.notice,
                _ => &textures.demo[usize::from(value.source.0 - 40)],
            }
            .as_ref()
            .ok_or(RendererFault::RenderSkipped)?;
            (
                Texture {
                    size: SourceSize {
                        width: row_stride(texture.size.width),
                        height: texture.size.height,
                    },
                    pixels: &texture.pixels,
                    coverage: Coverage::Opaque,
                },
                texture.size,
            )
        }
        _ => return Err(RendererFault::RenderSkipped),
    };
    let region = if value.source.0 == 1 {
        TextureRegion {
            source_x: u16::from(frame_index) * PET_FRAME_WIDTH as u16,
            source_y: 0,
            width: PET_FRAME_WIDTH as u16,
            height: PET_FRAME_HEIGHT as u16,
            stride: decoded.stride,
        }
    } else {
        TextureRegion {
            source_x: 0,
            source_y: 0,
            width: source_size.width,
            height: source_size.height,
            stride: texture.size.width,
        }
    };
    SceneBillboard::new(value, texture, region).map_err(|_| RendererFault::RenderSkipped)
}

// Keep the scene array off the projection/sort call stack: slice sorting can
// recurse, and its scratch must not overlap this renderer's large raster frame.
#[inline(never)]
fn draw_world_scene(
    framebuffer: &mut Framebuffer,
    projected: &[ProjectedBillboard],
    decoded: &DecodedLoop,
    frame_index: u8,
    textures: &WorldTextures,
    scratch: &mut WorldRasterScratch,
    context: (&mut impl FnMut(RasterPhase), &mut blit::PieBlitter),
) -> Result<deskkin_presentation::SceneStats, RendererFault> {
    let (observer, blitter) = context;
    let mut render = |boards: &[SceneBillboard<'_>]| {
        let mut occlusion = Occlusion::new(ScreenTile::Eight, &mut scratch.cutoffs)
            .map_err(|_| RendererFault::RenderSkipped)?;
        let mut plan = PreparedScene::new(
            boards,
            &mut occlusion,
            &mut scratch.prepared,
            &mut scratch.columns,
            observer,
        )
        .map_err(|_| RendererFault::RenderSkipped)?;
        let mut background = background::PieBackground;
        for y in (0..HEIGHT).step_by(BAND_ROWS) {
            let rows = BAND_ROWS.min(HEIGHT - y);
            framebuffer.begin_band()?;
            let index = framebuffer.back;
            let target = BandTarget::new(framebuffer.words_mut(index), WIDTH, y, rows)
                .map_err(|_| RendererFault::RenderSkipped)?;
            plan.raster_band(target, (&mut background, &mut *blitter), true, observer);
            if y + rows < HEIGHT {
                framebuffer.submit_band(y, rows, None)?;
            }
        }
        Ok(plan.stats())
    };
    if projected.is_empty() {
        render(&[])
    } else {
        let mut scene =
            [world_billboard(projected[0], decoded, frame_index, textures)?; demo_world::CAPACITY];
        for (slot, value) in scene.iter_mut().zip(projected) {
            *slot = world_billboard(*value, decoded, frame_index, textures)?;
        }
        render(&scene[..projected.len()])
    }
}

fn render_world(
    framebuffer: &mut Framebuffer,
    decoded: &DecodedLoop,
    frame_index: u8,
    snapshot: WorldSnapshot,
    textures: &WorldTextures,
    motion: &mut WorldMotion,
    input_generation: u32,
    telemetry: &mut WorldTelemetry,
) -> Result<(), RendererFault> {
    framebuffer.begin_frame()?;
    let started = unsafe { deskkin_uptime_us() };
    unsafe { deskkin_renderer_progress(RendererProgress::Raster as u8) };
    motion.advance(started);
    let camera = CameraPose {
        radius: WorldUnit::from_int(4),
        observed_azimuth: UnwrappedAngle::from_units(snapshot.observed_yaw),
        height: WorldUnit::ZERO,
    };
    let billboards = demo_world::projected_entities(
        motion.scene,
        (snapshot.availability != 0).then_some(TextureId(
            10 + u16::from(snapshot.availability.saturating_sub(1)),
        )),
        snapshot.notice != 0,
        camera,
    );
    let projection_started = unsafe { deskkin_uptime_us() };
    let projected = &mut motion.projected;
    let mut count = 0;
    let mut candidates = 0_u8;
    for projected_entity in billboards {
        candidates = candidates.saturating_add(1);
        if let Ok(value) = projected_entity {
            projected[count] = value;
            count += 1;
        }
    }
    let projection_us = elapsed_us(projection_started, unsafe { deskkin_uptime_us() });
    let sort_started = unsafe { deskkin_uptime_us() };
    sort_far_to_near(&mut projected[..count]);
    let sort_us = elapsed_us(sort_started, unsafe { deskkin_uptime_us() });
    let raster_started = unsafe { deskkin_uptime_us() };
    let mut profile = [0_u32; 16];
    let mut blitter = blit::PieBlitter::default();
    let mut phase = RasterPhase::Idle;
    let mut phase_started = unsafe { deskkin_blit_cycles() };
    let mut observer = |next: RasterPhase| {
        let now = unsafe { deskkin_blit_cycles() };
        if phase != RasterPhase::Idle {
            let index = phase as usize;
            profile[index] = profile[index].saturating_add(now.wrapping_sub(phase_started) / 240);
        }
        phase = next;
        phase_started = now;
    };
    let stats = draw_world_scene(
        framebuffer,
        &projected[..count],
        decoded,
        frame_index,
        textures,
        &mut motion.raster,
        (&mut observer, &mut blitter),
    )?;
    profile[4] = stats.coverage_tests;
    profile[5] = stats.scaler_preparations;
    profile[6] = stats.raster.nearest_samples;
    profile[7] = stats.raster.bilinear_samples;
    let blit_profile = blitter.profile();
    profile[8] = profile[3].saturating_sub(blit_profile[0] + blit_profile[1]);
    profile[9..13].copy_from_slice(&blit_profile);
    let nearest_samples = stats.raster.nearest_samples;
    let bilinear_samples = stats.raster.bilinear_samples;
    let raster_us = elapsed_us(raster_started, unsafe { deskkin_uptime_us() });
    unsafe {
        deskkin_world_observe(
            snapshot.generation,
            input_generation,
            telemetry.touch_drops,
            telemetry.cache_hits,
            telemetry.cache_misses,
            telemetry.cache_failures,
            count as u8,
            candidates.saturating_sub(count as u8),
            nearest_samples,
            bilinear_samples,
            projection_us,
            sort_us,
            telemetry.texture_us,
            raster_us,
        )
    };
    let last_band = (HEIGHT - 1) / BAND_ROWS * BAND_ROWS;
    framebuffer.submit_band(last_band, HEIGHT - last_band, Some(profile))
}

const fn empty_projected() -> ProjectedBillboard {
    ProjectedBillboard {
        id: BillboardId(0),
        screen_rect: ScreenRect {
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

const fn shell_name(shell: u8) -> &'static str {
    match shell {
        1 => "ReadyToPair",
        2 => "Connecting",
        3 => "PairingConfirmation",
        4 => "Paired",
        _ => "SetupRequired",
    }
}

fn sas_text(sas: u32) -> slint::SharedString {
    if sas == u32::MAX {
        return "".into();
    }
    let mut digits = [b'0'; 6];
    let mut value = sas;
    for index in (0..6).rev() {
        digits[index] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    core::str::from_utf8(&digits).unwrap_or("").into()
}

#[no_mangle]
extern "C" fn rust_main() {
    unsafe { deskkin_renderer_entry_probe() };
    if !background::self_test() {
        unsafe {
            deskkin_renderer_observe(
                RendererStage::Failed as u8,
                RendererFault::BackgroundCheck as u8,
                0,
                0,
            )
        };
        return;
    }
    if !blit::self_test() {
        unsafe {
            deskkin_renderer_observe(
                RendererStage::Failed as u8,
                RendererFault::BlitCheck as u8,
                0,
                0,
            )
        };
        return;
    }
    let state = Rc::new(RefCell::new(Vec::new()));
    if slint::platform::set_platform(Box::new(DevicePlatform {
        windows: state.clone(),
    }))
    .is_err()
    {
        unsafe {
            deskkin_renderer_observe(
                RendererStage::Failed as u8,
                RendererFault::Platform as u8,
                0,
                0,
            )
        };
        return;
    }
    let Ok(component) = RendererWindow::new() else {
        unsafe {
            deskkin_renderer_observe(
                RendererStage::Failed as u8,
                RendererFault::Component as u8,
                0,
                0,
            )
        };
        return;
    };
    let windows = state.borrow();
    let Some(window) = windows.first().cloned() else {
        unsafe {
            deskkin_renderer_observe(
                RendererStage::Failed as u8,
                RendererFault::Window as u8,
                0,
                0,
            )
        };
        return;
    };
    drop(windows);
    window.set_size(PhysicalSize::new(WIDTH as u32, HEIGHT as u32));
    if component.show().is_err() {
        unsafe {
            deskkin_renderer_observe(RendererStage::Failed as u8, RendererFault::Show as u8, 0, 0)
        };
        return;
    }
    let Some(mut framebuffer) = Framebuffer::new() else {
        unsafe {
            deskkin_renderer_observe(
                RendererStage::Failed as u8,
                RendererFault::Framebuffer as u8,
                0,
                0,
            )
        };
        return;
    };
    let mut animator = PetAnimator::new();
    let _ = animator.set_state(PetAnimationState::MoveRight);
    let mut decoded = None;
    if let Err(fault) = replace_loop(&component, &mut decoded, PetAnimationState::MoveRight) {
        unsafe { deskkin_renderer_observe(RendererStage::Failed as u8, fault as u8, 0, 0) };
        return;
    }
    let mut world_textures = new_world_textures();
    let mut world_telemetry = WorldTelemetry {
        cache_hits: 0,
        cache_misses: 0,
        cache_failures: 0,
        texture_us: 0,
        touch_drops: 0,
    };
    component.on_pair(|| unsafe { deskkin_publish_ui_command(1) });
    component.on_confirm(|| unsafe { deskkin_publish_ui_command(2) });
    component.on_cancel(|| unsafe { deskkin_publish_ui_command(3) });
    let mut display_enabled = false;
    let mut animation_updated_us = unsafe { deskkin_uptime_us() };
    let mut next_frame_at_us = animation_updated_us.saturating_add(50_000);
    let mut touch = demo_world::DemoCamera::new(UnwrappedAngle::ZERO);
    let mut camera_updated_us = unsafe { deskkin_uptime_us() };
    let mut touch_generation = 0_u32;
    let mut ui_pointer_pressed = false;
    let mut world_snapshot = WorldSnapshot::default();
    let mut have_world_snapshot = false;
    let mut world_motion = WorldMotion::new(unsafe { deskkin_uptime_us() });

    loop {
        let frame_started_us = unsafe { deskkin_uptime_us() };
        unsafe { deskkin_renderer_progress(RendererProgress::Loop as u8) };
        unsafe { deskkin_renderer_progress(RendererProgress::Snapshot as u8) };
        let snapshot_result = unsafe { deskkin_world_snapshot(&mut world_snapshot) };
        if snapshot_result > 0 {
            if !have_world_snapshot {
                touch = demo_world::DemoCamera::new(UnwrappedAngle::from_units(
                    world_snapshot.observed_yaw,
                ));
            }
            have_world_snapshot = true;
        } else if snapshot_result < 0 && snapshot_result != -11 {
            unsafe {
                deskkin_renderer_observe(
                    RendererStage::Failed as u8,
                    RendererFault::SharedSnapshot as u8,
                    0,
                    0,
                )
            };
            return;
        }
        let camera_now_us = unsafe { deskkin_uptime_us() };
        let elapsed_ms = camera_now_us.saturating_sub(camera_updated_us) / 1000;
        camera_updated_us += elapsed_ms * 1000;
        if have_world_snapshot && world_snapshot.shell == 4 {
            touch.advance(u32::try_from(elapsed_ms).unwrap_or(u32::MAX));
            unsafe { deskkin_publish_target_yaw(touch.target().units()) };
        }
        unsafe { deskkin_renderer_progress(RendererProgress::Touch as u8) };
        loop {
            let mut sample = TouchSample::default();
            let mut drop_count = 0_u32;
            let result =
                unsafe { deskkin_touch_read(touch_generation, &mut sample, &mut drop_count) };
            world_telemetry.touch_drops = drop_count;
            if result < 0 && result != -11 {
                unsafe {
                    deskkin_renderer_observe(
                        RendererStage::Failed as u8,
                        RendererFault::SharedSnapshot as u8,
                        0,
                        0,
                    )
                };
                return;
            }
            if result <= 0 {
                break;
            }
            touch_generation = sample.generation;
            if have_world_snapshot && world_snapshot.shell == 4 {
                ui_pointer_pressed = false;
                let target = touch.sample(sample.x, sample.pressed != 0);
                unsafe { deskkin_publish_target_yaw(target.units()) };
            } else {
                let position = LogicalPosition::new(f32::from(sample.x), f32::from(sample.y));
                let event = if sample.pressed != 0 && !ui_pointer_pressed {
                    ui_pointer_pressed = true;
                    WindowEvent::PointerPressed {
                        position,
                        button: PointerEventButton::Left,
                    }
                } else if sample.pressed != 0 {
                    WindowEvent::PointerMoved { position }
                } else if ui_pointer_pressed {
                    ui_pointer_pressed = false;
                    WindowEvent::PointerReleased {
                        position,
                        button: PointerEventButton::Left,
                    }
                } else {
                    continue;
                };
                window.dispatch_event(event);
            }
        }
        if have_world_snapshot && world_snapshot.shell != 4 {
            component.set_shell_state(shell_name(world_snapshot.shell).into());
            component.set_shell_code(i32::from(world_snapshot.shell));
            component.set_authentication_string(sas_text(world_snapshot.sas));
            unsafe {
                deskkin_shell_observe(
                    world_snapshot.shell,
                    u8::from(
                        component.get_shell_state().as_str() == shell_name(world_snapshot.shell),
                    ),
                )
            };
            window.request_redraw();
        }
        let render_result = if have_world_snapshot && world_snapshot.shell == 4 {
            unsafe { deskkin_shell_observe(4, 0) };
            unsafe { deskkin_renderer_progress(RendererProgress::Texture as u8) };
            if let Err(fault) = ensure_world_textures(
                &mut world_textures,
                &component,
                &window,
                world_snapshot,
                &mut world_telemetry,
            ) {
                unsafe {
                    deskkin_world_observe(
                        world_snapshot.generation,
                        touch_generation,
                        world_telemetry.touch_drops,
                        world_telemetry.cache_hits,
                        world_telemetry.cache_misses,
                        world_telemetry.cache_failures,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        world_telemetry.texture_us,
                        0,
                    )
                };
                unsafe { deskkin_renderer_observe(RendererStage::Failed as u8, fault as u8, 0, 0) };
                return;
            }
            let Some(decoded) = decoded.as_ref() else {
                unsafe {
                    deskkin_renderer_observe(
                        RendererStage::Failed as u8,
                        RendererFault::QoiDecode as u8,
                        0,
                        0,
                    )
                };
                return;
            };
            render_world(
                &mut framebuffer,
                decoded,
                animator.frame().index,
                world_snapshot,
                &world_textures,
                &mut world_motion,
                touch_generation,
                &mut world_telemetry,
            )
        } else {
            render_frame(&window, &mut framebuffer)
        };
        if let Err(fault) = render_result {
            unsafe { deskkin_renderer_observe(RendererStage::Failed as u8, fault as u8, 0, 0) };
            return;
        }
        if have_world_snapshot && world_snapshot.shell != 4 {
            unsafe {
                deskkin_world_observe(
                    world_snapshot.generation,
                    touch_generation,
                    world_telemetry.touch_drops,
                    world_telemetry.cache_hits,
                    world_telemetry.cache_misses,
                    world_telemetry.cache_failures,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    world_telemetry.texture_us,
                    0,
                )
            };
        }
        if !display_enabled {
            if unsafe { deskkin_display_enable() } != 0 {
                unsafe {
                    deskkin_renderer_observe(
                        RendererStage::Failed as u8,
                        RendererFault::DisplayEnable as u8,
                        0,
                        0,
                    )
                };
                return;
            }
            display_enabled = true;
        }
        let world_active = have_world_snapshot && world_snapshot.shell == 4;
        let frame_deadline_us = if world_active {
            frame_started_us.saturating_add(50_000)
        } else {
            next_frame_at_us
        };
        let work_completed_us = unsafe { deskkin_uptime_us() };
        unsafe { deskkin_renderer_progress(RendererProgress::Pacing as u8) };
        while !world_active && unsafe { deskkin_uptime_us() } < frame_deadline_us {
            if let Err(fault) = framebuffer.publish_completions() {
                unsafe { deskkin_renderer_observe(RendererStage::Failed as u8, fault as u8, 0, 0) };
                return;
            }
            unsafe { deskkin_yield() };
        }
        let animation_now_us = unsafe { deskkin_uptime_us() };
        let animation_elapsed_ms = animation_now_us.saturating_sub(animation_updated_us) / 1000;
        animation_updated_us += animation_elapsed_ms * 1000;
        let frame = animator.advance(u32::try_from(animation_elapsed_ms).unwrap_or(u32::MAX));
        component.set_pet_frame_index(i32::from(frame.index));
        if work_completed_us > frame_deadline_us {
            let missed = (work_completed_us - frame_deadline_us) / 50_000 + 1;
            for _ in 0..missed {
                unsafe { deskkin_deadline_missed() };
            }
            next_frame_at_us = frame_deadline_us.saturating_add(missed.saturating_mul(50_000));
        } else {
            next_frame_at_us = frame_deadline_us.saturating_add(50_000);
        }
        if world_active {
            next_frame_at_us = work_completed_us.saturating_add(50_000);
        }
    }
}
