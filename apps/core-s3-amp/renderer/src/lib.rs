// SPDX-License-Identifier: GPL-3.0-only

#![no_std]

extern crate alloc;
extern crate zephyr;

use alloc::{boxed::Box, rc::Rc};
use core::{cell::RefCell, ffi::c_int, marker::PhantomData, ptr::NonNull, time::Duration};
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType, Rgb565Pixel, TargetPixel,
};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize};

slint::include_modules!();

mod buffer_ownership;

use buffer_ownership::BufferOwnership;

const WIDTH: usize = 320;
const HEIGHT: usize = 240;
const BUFFER_COUNT: usize = 2;
const FRAME_PIXELS: usize = WIDTH * HEIGHT;
const MAX_DIRTY_RECTS: usize = 3;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct DirtyRect {
    x: u16,
    y: u16,
    width: u16,
    height: u16,
}

unsafe extern "C" {
    fn deskkin_framebuffer_alloc(index: u8) -> *mut u16;
    fn deskkin_display_submit(
        buffer_index: u8,
        dirty_rects: *const DirtyRect,
        dirty_rect_count: u8,
    ) -> c_int;
    fn deskkin_display_take_completion(buffer_index: *mut u8, duration_us: *mut u32) -> c_int;
    fn deskkin_display_enable() -> c_int;
    fn deskkin_renderer_boot_stage(stage: u8);
    fn deskkin_renderer_observe(stage: u8, fault: u8, render_us: u32, transfer_us: u32);
    fn deskkin_uptime_us() -> u64;
    fn deskkin_yield();
}

struct DevicePlatform {
    window: Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>,
}

impl Platform for DevicePlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::SwappedBuffers);
        self.window.replace(Some(window.clone()));
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
                    4,
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
                        4,
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
        unsafe { deskkin_renderer_observe(3, RendererFault::None as u8, render_us, 0) };
        Ok(())
    }
}

fn elapsed_us(start: u64, end: u64) -> u32 {
    end.saturating_sub(start).try_into().unwrap_or(u32::MAX)
}

fn render_frame(
    component: &RendererWindow,
    window: &MinimalSoftwareWindow,
    framebuffer: &mut Framebuffer,
    frame: i32,
) -> Result<(), RendererFault> {
    component.set_frame(frame);
    slint::platform::update_timers_and_animations();
    framebuffer.publish_completions()?;
    framebuffer.wait_for_back_buffer()?;
    unsafe { deskkin_renderer_observe(2, RendererFault::None as u8, 0, 0) };
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
    framebuffer.submit(
        elapsed_us(started, unsafe { deskkin_uptime_us() }),
        &dirty_rects[..dirty_rect_count],
    )
}

#[no_mangle]
extern "C" fn rust_main() {
    unsafe { deskkin_renderer_boot_stage(9) };
    let state = Rc::new(RefCell::new(None));
    if slint::platform::set_platform(Box::new(DevicePlatform {
        window: state.clone(),
    }))
    .is_err()
    {
        unsafe { deskkin_renderer_observe(5, RendererFault::Platform as u8, 0, 0) };
        return;
    }
    unsafe { deskkin_renderer_boot_stage(10) };
    let Ok(component) = RendererWindow::new() else {
        unsafe { deskkin_renderer_observe(5, RendererFault::Component as u8, 0, 0) };
        return;
    };
    unsafe { deskkin_renderer_boot_stage(11) };
    let Some(window) = state.borrow().clone() else {
        unsafe { deskkin_renderer_observe(5, RendererFault::Window as u8, 0, 0) };
        return;
    };
    window.set_size(PhysicalSize::new(WIDTH as u32, HEIGHT as u32));
    unsafe { deskkin_renderer_boot_stage(12) };
    if component.show().is_err() {
        unsafe { deskkin_renderer_observe(5, RendererFault::Show as u8, 0, 0) };
        return;
    }
    unsafe { deskkin_renderer_boot_stage(13) };
    let Some(mut framebuffer) = Framebuffer::new() else {
        unsafe { deskkin_renderer_observe(5, RendererFault::Framebuffer as u8, 0, 0) };
        return;
    };
    unsafe { deskkin_renderer_boot_stage(14) };
    let mut frame = 0_i32;
    let mut display_enabled = false;

    loop {
        if let Err(fault) = render_frame(&component, &window, &mut framebuffer, frame) {
            unsafe { deskkin_renderer_observe(5, fault as u8, 0, 0) };
            return;
        }
        if !display_enabled {
            if unsafe { deskkin_display_enable() } != 0 {
                unsafe { deskkin_renderer_observe(5, RendererFault::DisplayEnable as u8, 0, 0) };
                return;
            }
            display_enabled = true;
        }
        frame = frame.wrapping_add(1);
    }
}
