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

const WIDTH: usize = 320;
const HEIGHT: usize = 240;

unsafe extern "C" {
    fn deskkin_framebuffer_alloc(index: u8) -> *mut u16;
    fn deskkin_display_submit(buffer_index: u8) -> c_int;
    fn deskkin_display_take_completion(
        buffer_index: *mut u8,
        duration_us: *mut u32,
        copy_us: *mut u32,
    ) -> c_int;
    fn deskkin_display_enable() -> c_int;
    fn deskkin_renderer_boot_stage(stage: u8);
    fn deskkin_renderer_observe(stage: u8, render_us: u32, transfer_us: u32);
    fn deskkin_uptime_us() -> u64;
    fn deskkin_sleep_ms(delay_ms: u32);
}

struct DevicePlatform {
    window: Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>,
}

impl Platform for DevicePlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
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

struct Framebuffer {
    pixels: NonNull<u16>,
    _single_threaded: PhantomData<Rc<()>>,
}

impl Framebuffer {
    fn new() -> Option<Self> {
        Some(Self {
            pixels: NonNull::new(unsafe { deskkin_framebuffer_alloc(0) })?,
            _single_threaded: PhantomData,
        })
    }

    fn pixels_mut(&mut self) -> &mut [Rgb565BePixel] {
        unsafe {
            core::slice::from_raw_parts_mut(
                self.pixels.as_ptr().cast::<Rgb565BePixel>(),
                WIDTH * HEIGHT,
            )
        }
    }

    fn back_index(&self) -> u8 {
        0
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
) -> Result<u32, ()> {
    component.set_frame(frame);
    slint::platform::update_timers_and_animations();
    unsafe { deskkin_renderer_observe(2, 0, 0) };
    let started = unsafe { deskkin_uptime_us() };
    let rendered = window.draw_if_needed(|renderer| {
        let _ = renderer.render(framebuffer.pixels_mut(), WIDTH);
    });
    rendered
        .then(|| elapsed_us(started, unsafe { deskkin_uptime_us() }))
        .ok_or(())
}

fn take_completion(expected_buffer: u8) -> Result<u32, ()> {
    loop {
        let mut buffer_index = 0_u8;
        let mut duration_us = 0_u32;
        let mut copy_us = 0_u32;
        let completion = unsafe {
            deskkin_display_take_completion(&mut buffer_index, &mut duration_us, &mut copy_us)
        };
        if completion == 0 {
            unsafe { deskkin_sleep_ms(1) };
            continue;
        }
        return (completion > 0 && buffer_index == expected_buffer)
            .then_some(duration_us)
            .ok_or(());
    }
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
        unsafe { deskkin_renderer_observe(5, 0, 0) };
        return;
    }
    unsafe { deskkin_renderer_boot_stage(10) };
    let Ok(component) = RendererWindow::new() else {
        unsafe { deskkin_renderer_observe(5, 0, 0) };
        return;
    };
    unsafe { deskkin_renderer_boot_stage(11) };
    let Some(window) = state.borrow().clone() else {
        unsafe { deskkin_renderer_observe(5, 0, 0) };
        return;
    };
    window.set_size(PhysicalSize::new(WIDTH as u32, HEIGHT as u32));
    unsafe { deskkin_renderer_boot_stage(12) };
    if component.show().is_err() {
        unsafe { deskkin_renderer_observe(5, 0, 0) };
        return;
    }
    unsafe { deskkin_renderer_boot_stage(13) };
    let Some(mut framebuffer) = Framebuffer::new() else {
        unsafe { deskkin_renderer_observe(5, 0, 0) };
        return;
    };
    unsafe { deskkin_renderer_boot_stage(14) };
    let mut frame = 0_i32;

    let mut first_frame = true;
    loop {
        let Ok(render_us) = render_frame(&component, &window, &mut framebuffer, frame) else {
            unsafe { deskkin_renderer_observe(5, 0, 0) };
            return;
        };
        let buffer = framebuffer.back_index();
        if unsafe { deskkin_display_submit(buffer) } != 0 {
            unsafe { deskkin_renderer_observe(5, render_us, 0) };
            return;
        }
        unsafe { deskkin_renderer_observe(3, render_us, 0) };
        let Ok(transfer_us) = take_completion(buffer) else {
            unsafe { deskkin_renderer_observe(5, render_us, 0) };
            return;
        };
        if first_frame {
            if unsafe { deskkin_display_enable() } != 0 {
                unsafe { deskkin_renderer_observe(5, render_us, transfer_us) };
                return;
            }
            first_frame = false;
        }
        unsafe { deskkin_renderer_observe(4, render_us, transfer_us) };
        frame = frame.wrapping_add(1);
    }
}
