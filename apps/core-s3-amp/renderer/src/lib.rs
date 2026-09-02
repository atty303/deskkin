// SPDX-License-Identifier: GPL-3.0-only

#![no_std]

extern crate alloc;
extern crate zephyr;

use alloc::{boxed::Box, rc::Rc, vec, vec::Vec};
use core::{cell::RefCell, ffi::c_int, marker::PhantomData, ptr::NonNull, time::Duration};
use deskkin_presentation::{
    Billboard, BillboardId, CameraPose, CylindricalPose, PetAnimationState, PetAnimator,
    PixelFormat, ProjectedBillboard, ScreenRect, SourceSize, Texture, TextureFilter, TextureId,
    TextureRegion, TouchYawAdapter, UnwrappedAngle, WorldUnit, project_billboard,
    raster_billboard_be, raster_billboard_region_be, sort_far_to_near,
};
use qoi::{Channels, Decoder};
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType, Rgb565Pixel, TargetPixel,
};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, Image, LogicalPosition, PhysicalSize, Rgba8Pixel, SharedPixelBuffer};

slint::include_modules!();

mod buffer_ownership;

use buffer_ownership::BufferOwnership;

const WIDTH: usize = 320;
const HEIGHT: usize = 240;
const BUFFER_COUNT: usize = 2;
const FRAME_PIXELS: usize = WIDTH * HEIGHT;
const MAX_DIRTY_RECTS: usize = 3;
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
struct DirtyRect {
    x: u16,
    y: u16,
    width: u16,
    height: u16,
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
    fn deskkin_display_submit(
        buffer_index: u8,
        dirty_rects: *const DirtyRect,
        dirty_rect_count: u8,
    ) -> c_int;
    fn deskkin_display_take_completion(buffer_index: *mut u8, duration_us: *mut u32) -> c_int;
    fn deskkin_display_enable() -> c_int;
    fn deskkin_renderer_observe(stage: u8, fault: u8, render_us: u32, transfer_us: u32);
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
}

struct DevicePlatform {
    windows: Rc<RefCell<Vec<Rc<MinimalSoftwareWindow>>>>,
}

impl Platform for DevicePlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let repaint = if self.windows.borrow().is_empty() {
            RepaintBufferType::SwappedBuffers
        } else {
            RepaintBufferType::NewBuffer
        };
        let window = MinimalSoftwareWindow::new(repaint);
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

struct CompletedFrame {
    render_us: u32,
    transfer_us: u32,
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
}

#[repr(u8)]
#[derive(Clone, Copy)]
enum RendererStage {
    Rendering = 2,
    Transferring = 3,
    Presented = 4,
    Failed = 5,
    AssetLoading = 6,
    AssetReady = 7,
}

struct Framebuffer {
    pixels: [NonNull<u16>; BUFFER_COUNT],
    render_us: [u32; BUFFER_COUNT],
    back: usize,
    ownership: BufferOwnership<BUFFER_COUNT>,
    _single_threaded: PhantomData<Rc<()>>,
}

impl Framebuffer {
    fn new() -> Option<Self> {
        Some(Self {
            pixels: [
                NonNull::new(unsafe { deskkin_framebuffer_alloc(0) })?,
                NonNull::new(unsafe { deskkin_framebuffer_alloc(1) })?,
            ],
            render_us: [0; BUFFER_COUNT],
            back: 0,
            ownership: BufferOwnership::new(),
            _single_threaded: PhantomData,
        })
    }

    fn pixels_mut(&mut self, index: usize) -> &mut [Rgb565BePixel] {
        unsafe {
            core::slice::from_raw_parts_mut(
                self.pixels[index].as_ptr().cast::<Rgb565BePixel>(),
                FRAME_PIXELS,
            )
        }
    }

    fn words_mut(&mut self, index: usize) -> &mut [u16] {
        unsafe { core::slice::from_raw_parts_mut(self.pixels[index].as_ptr(), FRAME_PIXELS) }
    }

    fn take_completion(&mut self) -> Result<Option<CompletedFrame>, RendererFault> {
        let mut buffer_index = 0_u8;
        let mut transfer_us = 0_u32;
        let result =
            unsafe { deskkin_display_take_completion(&mut buffer_index, &mut transfer_us) };
        if result == 0 {
            return Ok(None);
        }
        let index = usize::from(buffer_index);
        if result < 0 || self.ownership.complete(index).is_err() {
            return Err(RendererFault::Completion);
        }
        Ok(Some(CompletedFrame {
            render_us: self.render_us[index],
            transfer_us,
        }))
    }

    fn publish_completions(&mut self) -> Result<(), RendererFault> {
        while let Some(frame) = self.take_completion()? {
            unsafe {
                deskkin_renderer_observe(
                    RendererStage::Presented as u8,
                    RendererFault::None as u8,
                    frame.render_us,
                    frame.transfer_us,
                )
            };
        }
        Ok(())
    }

    fn wait_for_back_buffer(&mut self) -> Result<(), RendererFault> {
        while self.ownership.is_inflight(self.back) {
            if let Some(frame) = self.take_completion()? {
                unsafe {
                    deskkin_renderer_observe(
                        RendererStage::Presented as u8,
                        RendererFault::None as u8,
                        frame.render_us,
                        frame.transfer_us,
                    )
                };
            } else {
                unsafe { deskkin_yield() };
            }
        }
        Ok(())
    }

    fn submit(&mut self, render_us: u32, dirty_rects: &[DirtyRect]) -> Result<(), RendererFault> {
        let index = self.back;
        self.render_us[index] = render_us;
        self.ownership
            .submit(index)
            .map_err(|_| RendererFault::Ownership)?;
        if unsafe {
            deskkin_display_submit(
                index as u8,
                dirty_rects.as_ptr(),
                dirty_rects.len().try_into().unwrap_or(u8::MAX),
            )
        } != 0
        {
            self.ownership.submission_failed(index);
            return Err(RendererFault::Submit);
        }
        self.back ^= 1;
        unsafe {
            deskkin_renderer_observe(
                RendererStage::Transferring as u8,
                RendererFault::None as u8,
                render_us,
                0,
            )
        };
        Ok(())
    }
}

fn elapsed_us(start: u64, end: u64) -> u32 {
    end.saturating_sub(start).try_into().unwrap_or(u32::MAX)
}

struct DecodedLoop {
    image: Image,
    pixels: Vec<u16>,
    alpha: Vec<u8>,
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
    let mut rgb565 = Vec::with_capacity((header.width * header.height) as usize);
    let mut alpha = Vec::with_capacity(rgb565.capacity());
    for pixel in pixels.as_slice() {
        rgb565.push(
            (u16::from(pixel.r >> 3) << 11)
                | (u16::from(pixel.g >> 2) << 5)
                | u16::from(pixel.b >> 3),
        );
        alpha.push(pixel.a);
    }
    Ok(DecodedLoop {
        image: Image::from_rgba8(pixels),
        pixels: rgb565,
        alpha,
        stride: header
            .width
            .try_into()
            .map_err(|_| RendererFault::QoiMetadata)?,
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
    force_full_transfer: bool,
) -> Result<(), RendererFault> {
    slint::platform::update_timers_and_animations();
    framebuffer.publish_completions()?;
    framebuffer.wait_for_back_buffer()?;
    unsafe {
        deskkin_renderer_observe(
            RendererStage::Rendering as u8,
            RendererFault::None as u8,
            0,
            0,
        )
    };
    let started = unsafe { deskkin_uptime_us() };
    let index = framebuffer.back;
    let mut dirty_rects = [DirtyRect::default(); MAX_DIRTY_RECTS];
    let mut dirty_rect_count = 0_usize;
    framebuffer
        .ownership
        .begin_render(index)
        .map_err(|_| RendererFault::Ownership)?;
    let rendered = window.draw_if_needed(|renderer| {
        let dirty_region = renderer.render(framebuffer.pixels_mut(index), WIDTH);
        let position = dirty_region.bounding_box_origin();
        let size = dirty_region.bounding_box_size();
        if size.width != 0 && size.height != 0 {
            if let (Ok(x), Ok(y), Ok(width), Ok(height)) = (
                position.x.try_into(),
                position.y.try_into(),
                size.width.try_into(),
                size.height.try_into(),
            ) {
                dirty_rects[0] = DirtyRect {
                    x,
                    y,
                    width,
                    height,
                };
                dirty_rect_count = 1;
            }
        }
    });
    if !rendered {
        framebuffer.ownership.cancel_render(index);
        return Err(RendererFault::RenderSkipped);
    }
    if force_full_transfer {
        dirty_rects[0] = DirtyRect {
            x: 0,
            y: 0,
            width: WIDTH as u16,
            height: HEIGHT as u16,
        };
        dirty_rect_count = 1;
    }
    framebuffer.submit(
        elapsed_us(started, unsafe { deskkin_uptime_us() }),
        &dirty_rects[..dirty_rect_count],
    )
}

struct BillboardTexture {
    pixels: Vec<u16>,
}

fn capture_billboard(
    component: &RendererWindow,
    window: &MinimalSoftwareWindow,
    notice: bool,
    text: &str,
    color: slint::Color,
) -> Result<BillboardTexture, RendererFault> {
    component.set_capture_notice(notice);
    component.set_capture_status_text(text.into());
    component.set_capture_status_color(color);
    component.set_capture_mode(true);
    window.set_size(PhysicalSize::new(272, 124));
    window.request_redraw();
    let mut pixels = vec![Rgb565Pixel(0); 272 * 124];
    let rendered = window.draw_if_needed(|renderer| {
        let _ = renderer.render(&mut pixels, 272);
    });
    component.set_capture_mode(false);
    window.set_size(PhysicalSize::new(WIDTH as u32, HEIGHT as u32));
    window.request_redraw();
    if !rendered {
        return Err(RendererFault::RenderSkipped);
    }
    Ok(BillboardTexture {
        pixels: pixels.into_iter().map(|pixel| pixel.0).collect(),
    })
}

struct WorldTextures {
    availability: [Option<BillboardTexture>; 3],
    notice: Option<BillboardTexture>,
    object_pixels: Vec<u16>,
    object_alpha: Vec<u8>,
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
        object_pixels: vec![0x39ec; 32 * 32],
        object_alpha: vec![255; 32 * 32],
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
            match capture_billboard(component, window, false, text, color) {
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
    telemetry.texture_us = elapsed_us(started, unsafe { deskkin_uptime_us() });
    Ok(())
}

struct WorldMotion {
    character_azimuth: UnwrappedAngle,
    object_radius: WorldUnit,
    object_outward: bool,
    updated_at_us: u64,
}

impl WorldMotion {
    fn new(now_us: u64) -> Self {
        Self {
            character_azimuth: UnwrappedAngle::ZERO,
            object_radius: WorldUnit::from_int(1),
            object_outward: true,
            updated_at_us: now_us,
        }
    }

    fn advance(&mut self, now_us: u64) {
        let elapsed_ms = now_us.saturating_sub(self.updated_at_us) / 1_000;
        self.updated_at_us = now_us;
        let elapsed_ms = elapsed_ms.min(u64::from(u32::MAX)) as u32;
        self.character_azimuth = self
            .character_azimuth
            .wrapping_add(UnwrappedAngle::from_units(
                i64::from(elapsed_ms) * 12 * 65_536 / 360_000,
            ));
        let radial_step = WorldUnit::ratio(elapsed_ms as i32, 4_000).bits();
        let next = self
            .object_radius
            .bits()
            .saturating_add(if self.object_outward {
                radial_step
            } else {
                -radial_step
            });
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
    framebuffer.publish_completions()?;
    framebuffer.wait_for_back_buffer()?;
    let index = framebuffer.back;
    framebuffer
        .ownership
        .begin_render(index)
        .map_err(|_| RendererFault::Ownership)?;
    let started = unsafe { deskkin_uptime_us() };
    motion.advance(started);
    let pixels = framebuffer.words_mut(index);
    pixels.fill(0x10c3_u16.to_be());
    let camera = CameraPose {
        radius: WorldUnit::from_int(4),
        observed_azimuth: UnwrappedAngle::from_units(snapshot.observed_yaw),
        height: WorldUnit::ZERO,
    };
    let billboards = [
        Some((
            Billboard {
                id: BillboardId(1),
                pose: CylindricalPose {
                    radius: WorldUnit::ratio(22, 10),
                    azimuth: motion.character_azimuth,
                    height: WorldUnit::ZERO,
                },
                world_height: WorldUnit::ratio(14, 10),
                texture_id: TextureId(1),
                filter: TextureFilter::Nearest,
            },
            SourceSize {
                width: 144,
                height: 156,
            },
        )),
        (snapshot.availability != 0).then_some((
            Billboard {
                id: BillboardId(2),
                pose: CylindricalPose {
                    radius: WorldUnit::ratio(12, 10),
                    azimuth: UnwrappedAngle::from_degrees(-15),
                    height: WorldUnit::ZERO,
                },
                world_height: WorldUnit::ratio(8, 10),
                texture_id: TextureId(10 + u16::from(snapshot.availability.saturating_sub(1))),
                filter: TextureFilter::Bilinear,
            },
            SourceSize {
                width: 272,
                height: 124,
            },
        )),
        (snapshot.notice != 0).then_some((
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
        )),
        Some((
            Billboard {
                id: BillboardId(4),
                pose: CylindricalPose {
                    radius: motion.object_radius,
                    azimuth: UnwrappedAngle::from_degrees(30),
                    height: WorldUnit::ratio(8, 10),
                },
                world_height: WorldUnit::ratio(5, 10),
                texture_id: TextureId(2),
                filter: TextureFilter::Nearest,
            },
            SourceSize {
                width: 32,
                height: 32,
            },
        )),
    ];
    let projection_started = unsafe { deskkin_uptime_us() };
    let mut projected = [empty_projected(); 4];
    let mut count = 0;
    let mut candidates = 0_u8;
    for (billboard, source) in billboards.into_iter().flatten() {
        candidates = candidates.saturating_add(1);
        if let Ok(value) = project_billboard(billboard, source, camera) {
            projected[count] = value;
            count += 1;
        }
    }
    let projection_us = elapsed_us(projection_started, unsafe { deskkin_uptime_us() });
    let sort_started = unsafe { deskkin_uptime_us() };
    sort_far_to_near(&mut projected[..count]);
    let sort_us = elapsed_us(sort_started, unsafe { deskkin_uptime_us() });
    let raster_started = unsafe { deskkin_uptime_us() };
    let mut nearest_samples = 0_u32;
    let mut bilinear_samples = 0_u32;
    for value in projected[..count].iter().copied() {
        let stats = match value.source.0 {
            1 => raster_billboard_region_be(
                pixels,
                WIDTH,
                value,
                Texture {
                    size: SourceSize {
                        width: decoded.stride,
                        height: PET_FRAME_HEIGHT as u16,
                    },
                    pixels: &decoded.pixels,
                    alpha: &decoded.alpha,
                    format: PixelFormat::Rgb565A8,
                },
                TextureRegion {
                    source_x: u16::from(frame_index) * PET_FRAME_WIDTH as u16,
                    source_y: 0,
                    width: PET_FRAME_WIDTH as u16,
                    height: PET_FRAME_HEIGHT as u16,
                    stride: decoded.stride,
                },
            )
            .map_err(|_| RendererFault::RenderSkipped)?,
            2 => raster_billboard_be(
                pixels,
                WIDTH,
                value,
                Texture {
                    size: SourceSize {
                        width: 32,
                        height: 32,
                    },
                    pixels: &textures.object_pixels,
                    alpha: &textures.object_alpha,
                    format: PixelFormat::Rgb565A8,
                },
            )
            .map_err(|_| RendererFault::RenderSkipped)?,
            10..=12 => raster_billboard_be(
                pixels,
                WIDTH,
                value,
                Texture {
                    size: SourceSize {
                        width: 272,
                        height: 124,
                    },
                    pixels: &textures.availability[(value.source.0 - 10) as usize]
                        .as_ref()
                        .ok_or(RendererFault::RenderSkipped)?
                        .pixels,
                    alpha: &[],
                    format: PixelFormat::OpaqueRgb565,
                },
            )
            .map_err(|_| RendererFault::RenderSkipped)?,
            20 => raster_billboard_be(
                pixels,
                WIDTH,
                value,
                Texture {
                    size: SourceSize {
                        width: 272,
                        height: 124,
                    },
                    pixels: &textures
                        .notice
                        .as_ref()
                        .ok_or(RendererFault::RenderSkipped)?
                        .pixels,
                    alpha: &[],
                    format: PixelFormat::OpaqueRgb565,
                },
            )
            .map_err(|_| RendererFault::RenderSkipped)?,
            _ => return Err(RendererFault::RenderSkipped),
        };
        nearest_samples = nearest_samples.saturating_add(stats.nearest_samples);
        bilinear_samples = bilinear_samples.saturating_add(stats.bilinear_samples);
    }
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
    let rect = [DirtyRect {
        x: 0,
        y: 0,
        width: WIDTH as u16,
        height: HEIGHT as u16,
    }];
    framebuffer.submit(elapsed_us(started, unsafe { deskkin_uptime_us() }), &rect)
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
    let mut next_frame_at_us = unsafe { deskkin_uptime_us() }.saturating_add(50_000);
    let mut touch = TouchYawAdapter::new(UnwrappedAngle::ZERO);
    let mut touch_generation = 0_u32;
    let mut ui_pointer_pressed = false;
    let mut world_snapshot = WorldSnapshot::default();
    let mut have_world_snapshot = false;
    let mut rendered_shell = None;
    let mut world_motion = WorldMotion::new(unsafe { deskkin_uptime_us() });

    loop {
        let snapshot_result = unsafe { deskkin_world_snapshot(&mut world_snapshot) };
        if snapshot_result > 0 {
            if !have_world_snapshot {
                touch =
                    TouchYawAdapter::new(UnwrappedAngle::from_units(world_snapshot.observed_yaw));
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
        let shell_changed = have_world_snapshot && rendered_shell != Some(world_snapshot.shell);
        let render_result = if have_world_snapshot && world_snapshot.shell == 4 {
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
            render_frame(&window, &mut framebuffer, shell_changed)
        };
        if let Err(fault) = render_result {
            unsafe { deskkin_renderer_observe(RendererStage::Failed as u8, fault as u8, 0, 0) };
            return;
        }
        if have_world_snapshot {
            rendered_shell = Some(world_snapshot.shell);
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
        let frame_deadline_us = next_frame_at_us;
        let work_completed_us = unsafe { deskkin_uptime_us() };
        while unsafe { deskkin_uptime_us() } < frame_deadline_us {
            if let Err(fault) = framebuffer.publish_completions() {
                unsafe { deskkin_renderer_observe(RendererStage::Failed as u8, fault as u8, 0, 0) };
                return;
            }
            unsafe { deskkin_yield() };
        }
        let frame = animator.advance(50);
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
    }
}
