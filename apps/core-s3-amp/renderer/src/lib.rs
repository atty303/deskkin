// SPDX-License-Identifier: GPL-3.0-only

#![no_std]

extern crate alloc;
extern crate zephyr;

use alloc::{boxed::Box, rc::Rc};
use core::{
    cell::RefCell, ffi::c_int, marker::PhantomData, ops::Range, ptr::NonNull, time::Duration,
};
use slint::platform::software_renderer::{
    LineBufferProvider, MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
    Rgb565Pixel, TargetPixel,
};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize};

slint::include_modules!();

mod band_ownership;

use band_ownership::BandOwnership;

const WIDTH: usize = 320;
const HEIGHT: usize = 240;
const DMA_MAX_BYTES: usize = 4092 * 8;
const BYTES_PER_LINE: usize = WIDTH * core::mem::size_of::<u16>();
const BAND_COUNT: usize = 8;
const BAND_LINES: usize = HEIGHT / BAND_COUNT;
const BAND_PIXELS: usize = BAND_LINES * BYTES_PER_LINE / core::mem::size_of::<u16>();
const BUFFER_COUNT: usize = 2;

const _: () = {
    assert!(HEIGHT % BAND_COUNT == 0);
    assert!(BAND_LINES == 30);
    assert!(BAND_PIXELS * core::mem::size_of::<u16>() <= DMA_MAX_BYTES);
};

unsafe extern "C" {
    fn deskkin_framebuffer_alloc(index: u8) -> *mut u16;
    fn deskkin_display_submit(buffer_index: u8, y: u16, line_count: u16) -> c_int;
    fn deskkin_display_take_completion(buffer_index: *mut u8, duration_us: *mut u32) -> c_int;
    fn deskkin_display_enable() -> c_int;
    fn deskkin_renderer_boot_stage(stage: u8);
    fn deskkin_renderer_observe(stage: u8, render_us: u32, transfer_us: u32);
    fn deskkin_uptime_us() -> u64;
    fn deskkin_yield();
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

    fn pixels(&self) -> NonNull<Rgb565BePixel> {
        self.pixels.cast()
    }
}

fn elapsed_us(start: u64, end: u64) -> u32 {
    end.saturating_sub(start).try_into().unwrap_or(u32::MAX)
}

struct BandedLineBuffer {
    pixels: NonNull<Rgb565BePixel>,
    next_line: usize,
    band_start: usize,
    band_lines: usize,
    current_buffer: usize,
    ownership: BandOwnership<BUFFER_COUNT>,
    transfers: usize,
    transfer_us: u32,
    wait_us: u32,
    failed: bool,
}

impl BandedLineBuffer {
    fn take_completion(&mut self) -> bool {
        let mut buffer_index = 0_u8;
        let mut duration_us = 0_u32;
        let result =
            unsafe { deskkin_display_take_completion(&mut buffer_index, &mut duration_us) };
        if result == 0 {
            return false;
        }
        let index = buffer_index as usize;
        if result < 0 || self.ownership.complete(index).is_err() {
            self.failed = true;
            return true;
        }
        self.transfer_us = self.transfer_us.saturating_add(duration_us);
        true
    }

    fn wait_for_buffer(&mut self, index: usize) {
        let started = unsafe { deskkin_uptime_us() };
        while self.ownership.is_inflight(index) && !self.failed {
            if !self.take_completion() {
                unsafe { deskkin_yield() };
            }
        }
        self.wait_us = self
            .wait_us
            .saturating_add(elapsed_us(started, unsafe { deskkin_uptime_us() }));
    }

    fn finish(&mut self) {
        for index in 0..BUFFER_COUNT {
            self.wait_for_buffer(index);
        }
    }
}

impl LineBufferProvider for &mut BandedLineBuffer {
    type TargetPixel = Rgb565BePixel;

    fn process_line(
        &mut self,
        line: usize,
        range: Range<usize>,
        render_fn: impl FnOnce(&mut [Self::TargetPixel]),
    ) {
        if line != self.next_line || range != (0..WIDTH) || self.band_lines >= BAND_LINES {
            self.failed = true;
        }
        if self.band_lines == 0 {
            self.wait_for_buffer(self.current_buffer);
            self.failed |= self.ownership.begin_render(self.current_buffer).is_err();
        }
        if self.failed {
            let mut scratch = [Rgb565BePixel::default(); WIDTH];
            render_fn(&mut scratch[range]);
            self.band_lines += 1;
            self.next_line = line.saturating_add(1);
            if self.band_lines == BAND_LINES || self.next_line == HEIGHT {
                self.band_start = self.next_line;
                self.band_lines = 0;
                self.current_buffer ^= 1;
            }
            return;
        }
        let band_line = self.band_lines.min(BAND_LINES - 1);
        let line_start = self.current_buffer * BAND_PIXELS + band_line * WIDTH;
        // Only the renderer-owned band is borrowed, and the borrow ends before
        // ownership is published to the display thread.
        let line_pixels =
            unsafe { core::slice::from_raw_parts_mut(self.pixels.as_ptr().add(line_start), WIDTH) };
        render_fn(&mut line_pixels[range]);
        self.band_lines += 1;
        self.next_line = line.saturating_add(1);

        if self.band_lines == BAND_LINES || self.next_line == HEIGHT {
            if self.ownership.submit(self.current_buffer).is_err() {
                self.failed = true;
            } else {
                let result = unsafe {
                    deskkin_display_submit(
                        self.current_buffer as u8,
                        self.band_start as u16,
                        self.band_lines as u16,
                    )
                };
                self.failed |= result != 0;
                if result != 0 {
                    self.ownership.submission_failed(self.current_buffer);
                }
                self.transfers += 1;
            }
            self.band_start = self.next_line;
            self.band_lines = 0;
            self.current_buffer ^= 1;
        }
    }
}

fn render_frame(
    component: &RendererWindow,
    window: &MinimalSoftwareWindow,
    framebuffer: &mut Framebuffer,
    frame: i32,
) -> Result<(u32, u32), ()> {
    component.set_frame(frame);
    slint::platform::update_timers_and_animations();
    unsafe { deskkin_renderer_observe(2, 0, 0) };
    let started = unsafe { deskkin_uptime_us() };
    let mut provider = BandedLineBuffer {
        pixels: framebuffer.pixels(),
        next_line: 0,
        band_start: 0,
        band_lines: 0,
        current_buffer: 0,
        ownership: BandOwnership::new(),
        transfers: 0,
        transfer_us: 0,
        wait_us: 0,
        failed: false,
    };
    let rendered = window.draw_if_needed(|renderer| {
        let _ = renderer.render_by_line(&mut provider);
    });
    let render_phase_us = elapsed_us(started, unsafe { deskkin_uptime_us() });
    let render_wait_us = provider.wait_us;
    provider.finish();
    (rendered
        && !provider.failed
        && provider.next_line == HEIGHT
        && provider.transfers == BAND_COUNT)
        .then_some((
            render_phase_us.saturating_sub(render_wait_us),
            provider.transfer_us,
        ))
        .ok_or(())
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
        let Ok((render_us, transfer_us)) =
            render_frame(&component, &window, &mut framebuffer, frame)
        else {
            unsafe { deskkin_renderer_observe(5, 0, 0) };
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
