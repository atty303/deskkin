// SPDX-License-Identifier: GPL-3.0-only

#![no_std]

extern crate alloc;

use alloc::{boxed::Box, rc::Rc, vec::Vec};
use core::{
    cell::{Cell, RefCell},
    ffi::{c_char, c_int},
    marker::PhantomData,
    ops::Range,
    ptr::NonNull,
    time::Duration,
};
use embassy_time::{Instant, Timer};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::platform::software_renderer::{
    LineBufferProvider, MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel,
};
use static_cell::StaticCell;
use zephyr::{embassy::Executor, printkln};

slint::include_modules!();

const WIDTH: usize = 320;
const HEIGHT: usize = 240;
const FRAMES: u32 = 1_800;
const WARMUP_FRAMES: u32 = 60;
const FRAME_PERIOD_US: u64 = 33_333;
const MAX_DIRTY_PIXELS: u32 = 19_200;
const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

static EXECUTOR: StaticCell<Executor> = StaticCell::new();

unsafe extern "C" {
    fn deskkin_wait_command(run_id: *mut c_char) -> c_int;
    fn deskkin_devices_ready() -> bool;
    fn deskkin_print_boot(run_id: *const c_char, mode: *const c_char);
    fn deskkin_print_idle(run_id: *const c_char);
    fn deskkin_framebuffer_alloc() -> *mut u16;
    fn deskkin_staging_alloc() -> *mut u16;
    fn deskkin_now_cycles() -> u32;
    fn deskkin_elapsed_us(started: u32) -> u32;
    fn deskkin_display_write(
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        pitch: u16,
        pixels: *const u16,
        duration_us: *mut u64,
    ) -> c_int;
    fn deskkin_display_enable() -> c_int;
    fn deskkin_inject_touch() -> c_int;
    fn deskkin_take_touch(x: *mut i32, y: *mut i32) -> bool;
}

struct GatePlatform {
    window: Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>,
}

impl Platform for GatePlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        self.window.replace(Some(window.clone()));
        Ok(window)
    }

    fn duration_since_start(&self) -> Duration {
        Instant::now().duration_since(Instant::from_secs(0)).into()
    }
}

#[derive(Clone, Copy)]
struct DirtyRange {
    start: u16,
    end: u16,
}

impl DirtyRange {
    const EMPTY: Self = Self { start: 0, end: 0 };

    fn pixels(self) -> u32 {
        u32::from(self.end - self.start)
    }
}

struct Framebuffer {
    pointer: NonNull<u16>,
    staging: NonNull<u16>,
}

impl Framebuffer {
    fn new() -> Option<Self> {
        Some(Self {
            pointer: NonNull::new(unsafe { deskkin_framebuffer_alloc() })?,
            staging: NonNull::new(unsafe { deskkin_staging_alloc() })?,
        })
    }

    fn line_pointer(&self, line: usize, column: usize) -> *const u16 {
        unsafe { self.pointer.as_ptr().add(line * WIDTH + column) }
    }
}

struct UiOwner {
    component: GateWindow,
    window: Rc<MinimalSoftwareWindow>,
    activations: Rc<Cell<u32>>,
    _single_threaded: PhantomData<Rc<()>>,
}

impl UiOwner {
    fn new(window_state: Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>) -> Self {
        let component = GateWindow::new().expect("the Gate 1E component must instantiate");
        let window = window_state
            .borrow()
            .clone()
            .expect("Slint must request exactly one Gate 1E window");
        window.set_size(PhysicalSize::new(WIDTH as u32, HEIGHT as u32));
        let activations = Rc::new(Cell::new(0));
        let callback_activations = activations.clone();
        component.on_activated(move || {
            callback_activations.set(callback_activations.get() + 1);
        });
        component.show().expect("the Gate 1E component must show");
        Self {
            component,
            window,
            activations,
            _single_threaded: PhantomData,
        }
    }

    fn set_frame(&self, eye_offset: i32, mouth_open: bool) {
        self.component.set_eye_offset(eye_offset);
        self.component.set_mouth_open(mouth_open);
    }

    fn dispatch_touch(&self, x: i32, y: i32) {
        let position = LogicalPosition::new(x as f32, y as f32);
        self.window.dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        self.window.dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
    }
}

struct Capture<'a> {
    line: [Rgb565Pixel; WIDTH],
    ranges: &'a mut [DirtyRange; HEIGHT],
    framebuffer: &'a Framebuffer,
    frame_hash: &'a mut u64,
}

impl LineBufferProvider for &mut Capture<'_> {
    type TargetPixel = Rgb565Pixel;

    fn process_line(
        &mut self,
        line: usize,
        range: Range<usize>,
        render_fn: impl FnOnce(&mut [Self::TargetPixel]),
    ) {
        let destination = &mut self.line[range.clone()];
        render_fn(destination);
        unsafe {
            core::ptr::copy_nonoverlapping(
                destination.as_ptr().cast::<u16>(),
                self.framebuffer.pointer.as_ptr().add(line * WIDTH + range.start),
                destination.len(),
            );
        }
        self.ranges[line] = DirtyRange {
            start: range.start as u16,
            end: range.end as u16,
        };
        hash_u32(self.frame_hash, line as u32);
        hash_u32(self.frame_hash, range.start as u32);
        hash_u32(self.frame_hash, range.end as u32);
        for pixel in destination {
            hash_u32(self.frame_hash, u32::from(pixel.0));
        }
    }
}

struct PhaseSummary {
    render_p95_us: u32,
    transfer_p95_us: u32,
    combined_p95_us: u32,
    combined_p99_us: u32,
    touch_p95_us: u32,
    missed_frames: u32,
    max_dirty_pixels: u32,
    post_initial_full_frames: u32,
    touches: u32,
    semantic_hash: u64,
    framebuffer_hash: u64,
}

#[derive(Clone, Copy)]
struct PhaseError {
    error_type: &'static str,
    frame: u32,
}

impl PhaseError {
    const fn new(error_type: &'static str, frame: u32) -> Self {
        Self { error_type, frame }
    }
}

fn hash_u32(hash: &mut u64, value: u32) {
    for byte in value.to_le_bytes() {
        *hash ^= u64::from(byte);
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}

fn percentile(values: &mut [u32], numerator: usize, denominator: usize) -> u32 {
    values.sort_unstable();
    let rank = (numerator * values.len()).div_ceil(denominator);
    values[rank.saturating_sub(1)]
}

fn transfer_dirty(
    framebuffer: &Framebuffer,
    ranges: &[DirtyRange; HEIGHT],
) -> Result<(u32, u32), &'static str> {
    let started = unsafe { deskkin_now_cycles() };
    let mut line = 0;
    let mut total_pixels = 0_u32;
    while line < HEIGHT {
        let range = ranges[line];
        if range.start == range.end {
            line += 1;
            continue;
        }
        let start_line = line;
        line += 1;
        while line < HEIGHT && ranges[line].start == range.start && ranges[line].end == range.end {
            line += 1;
        }
        let height = line - start_line;
        let width = usize::from(range.end - range.start);
        for row in 0..height {
            for column in 0..width {
                unsafe {
                    framebuffer.staging.as_ptr().add(row * width + column).write(
                        framebuffer
                            .line_pointer(start_line + row, usize::from(range.start) + column)
                            .read()
                            .swap_bytes(),
                    );
                }
            }
        }
        let mut duration = 0_u64;
        let result = unsafe {
            deskkin_display_write(
                range.start,
                start_line as u16,
                range.end - range.start,
                height as u16,
                width as u16,
                framebuffer.staging.as_ptr(),
                &mut duration,
            )
        };
        if result != 0 {
            return Err("display_write_failed");
        }
        total_pixels += range.pixels() * height as u32;
    }
    Ok((total_pixels, unsafe { deskkin_elapsed_us(started) }))
}

async fn scheduled_touch(owner: &UiOwner) -> Result<u32, &'static str> {
    let started = unsafe { deskkin_now_cycles() };
    if unsafe { deskkin_inject_touch() } != 0 {
        return Err("touch_inject_failed");
    }
    for _ in 0..20 {
        let mut x = 0;
        let mut y = 0;
        if unsafe { deskkin_take_touch(&mut x, &mut y) } {
            owner.dispatch_touch(x, y);
            return Ok(started);
        }
        Timer::after(embassy_time::Duration::from_millis(1)).await;
    }
    Err("touch_callback_timeout")
}

async fn run_phase(
    window_state: Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>,
    framebuffer: &Framebuffer,
    run_id: &str,
    phase: &str,
    recording: bool,
) -> Result<PhaseSummary, PhaseError> {
    let owner = UiOwner::new(window_state);
    slint::platform::update_timers_and_animations();
    let phase_start = Instant::now();
    let activation_start = owner.activations.get();
    let mut render_samples = Vec::with_capacity((FRAMES - WARMUP_FRAMES) as usize);
    let mut transfer_samples = Vec::with_capacity((FRAMES - WARMUP_FRAMES) as usize);
    let mut combined_samples = Vec::with_capacity((FRAMES - WARMUP_FRAMES) as usize);
    let mut touch_samples = Vec::with_capacity((FRAMES / 30) as usize);
    let mut missed_frames = 0_u32;
    let mut max_dirty_pixels = 0_u32;
    let mut post_initial_full_frames = 0_u32;
    let mut mouth_open = false;
    let mut semantic_hash = FNV_OFFSET;
    let mut framebuffer_hash = FNV_OFFSET;

    for frame in 0..FRAMES {
        if frame != 0 {
            let deadline = phase_start
                + embassy_time::Duration::from_micros(u64::from(frame) * FRAME_PERIOD_US);
            Timer::at(deadline).await;
        }

        let mut touch_started = None;
        if frame % 30 == 0 {
            touch_started = Some(
                scheduled_touch(&owner)
                    .await
                    .map_err(|error_type| PhaseError::new(error_type, frame))?,
            );
            mouth_open = !mouth_open;
        }
        let eye_step = (frame % 20) as i32;
        let eye_offset = if eye_step <= 10 { eye_step - 5 } else { 15 - eye_step };
        owner.set_frame(eye_offset, mouth_open);
        slint::platform::update_timers_and_animations();

        hash_u32(&mut semantic_hash, frame);
        hash_u32(&mut semantic_hash, eye_offset as u32);
        hash_u32(&mut semantic_hash, if mouth_open { 1 } else { 0 });
        hash_u32(
            &mut semantic_hash,
            if touch_started.is_some() { 1 } else { 0 },
        );

        let mut ranges = [DirtyRange::EMPTY; HEIGHT];
        let render_started = unsafe { deskkin_now_cycles() };
        owner.window.draw_if_needed(|renderer| {
            renderer.render_by_line(&mut Capture {
                line: [Rgb565Pixel(0); WIDTH],
                ranges: &mut ranges,
                framebuffer,
                frame_hash: &mut framebuffer_hash,
            });
        });
        let render_us = unsafe { deskkin_elapsed_us(render_started) };
        let (dirty_pixels, transfer_us) = transfer_dirty(framebuffer, &ranges)
            .map_err(|error_type| PhaseError::new(error_type, frame))?;
        if frame == 0 {
            if dirty_pixels != (WIDTH * HEIGHT) as u32 {
                return Err(PhaseError::new("initial_frame_incomplete", frame));
            }
            if unsafe { deskkin_display_enable() } != 0 {
                return Err(PhaseError::new("display_enable_failed", frame));
            }
        } else if dirty_pixels == (WIDTH * HEIGHT) as u32 {
            post_initial_full_frames += 1;
        }
        if frame != 0 && dirty_pixels > MAX_DIRTY_PIXELS {
            return Err(PhaseError::new("dirty_pixel_limit_exceeded", frame));
        }
        if frame != 0 {
            max_dirty_pixels = max_dirty_pixels.max(dirty_pixels);
        }
        let combined_us = render_us.saturating_add(transfer_us);
        let touch_latency_us = touch_started
            .map(|started| unsafe { deskkin_elapsed_us(started) })
            .unwrap_or(0);

        if frame >= WARMUP_FRAMES {
            render_samples.push(render_us);
            transfer_samples.push(transfer_us);
            combined_samples.push(combined_us);
            if touch_started.is_some() {
                touch_samples.push(touch_latency_us);
            }
            let scheduled_deadline_us = u64::from(frame + 1) * FRAME_PERIOD_US;
            let elapsed_us = Instant::now().duration_since(phase_start).as_micros();
            let missed = elapsed_us > scheduled_deadline_us;
            missed_frames += if missed { 1 } else { 0 };
            if recording {
                printkln!(
                    "DESKKIN_GATE1E_FRAME schema=1 run_id={} phase={} frame={} render_us={} transfer_us={} combined_us={} dirty_pixels={} touch_latency_us={} missed={}",
                    run_id,
                    phase,
                    frame,
                    render_us,
                    transfer_us,
                    combined_us,
                    dirty_pixels,
                    touch_latency_us,
                    if missed { "yes" } else { "no" }
                );
            }
        }
    }

    let touches = owner.activations.get() - activation_start;
    if render_samples.len() != (FRAMES - WARMUP_FRAMES) as usize
        || touch_samples.is_empty()
        || touches != FRAMES / 30
    {
        return Err(PhaseError::new("workload_count_mismatch", FRAMES));
    }
    Ok(PhaseSummary {
        render_p95_us: percentile(&mut render_samples, 95, 100),
        transfer_p95_us: percentile(&mut transfer_samples, 95, 100),
        combined_p95_us: percentile(&mut combined_samples, 95, 100),
        combined_p99_us: percentile(&mut combined_samples, 99, 100),
        touch_p95_us: percentile(&mut touch_samples, 95, 100),
        missed_frames,
        max_dirty_pixels,
        post_initial_full_frames,
        touches,
        semantic_hash,
        framebuffer_hash,
    })
}

fn print_summary(run_id: &str, phase: &str, summary: &PhaseSummary) {
    printkln!(
        "DESKKIN_GATE1E_SUMMARY schema=1 run_id={} phase={} frames={} samples={} render_p95_us={} transfer_p95_us={} combined_p95_us={} combined_p99_us={} touch_p95_us={} missed_frames={} max_dirty_pixels={} post_initial_full_frames={} touches={} semantic_digest={:016x} framebuffer_digest={:016x}",
        run_id,
        phase,
        FRAMES,
        FRAMES - WARMUP_FRAMES,
        summary.render_p95_us,
        summary.transfer_p95_us,
        summary.combined_p95_us,
        summary.combined_p99_us,
        summary.touch_p95_us,
        summary.missed_frames,
        summary.max_dirty_pixels,
        summary.post_initial_full_frames,
        summary.touches,
        summary.semantic_hash,
        summary.framebuffer_hash,
    );
}

fn print_phase_error(run_id: &str, phase: &str, error: PhaseError) {
    printkln!(
        "DESKKIN_GATE1E_ERROR schema=1 run_id={} phase={} error_type={} frame={}",
        run_id,
        phase,
        error.error_type,
        error.frame,
    );
}

#[no_mangle]
extern "C" fn rust_main() {
    let executor = EXECUTOR.init(Executor::new());
    executor.run(|spawner| {
        spawner
            .spawn(run_gate())
            .expect("the bounded Gate 1E task arena must fit");
    });
}

#[embassy_executor::task]
async fn run_gate() {
    let state = Rc::new(RefCell::new(None));
    slint::platform::set_platform(Box::new(GatePlatform {
        window: state.clone(),
    }))
    .expect("the Gate 1E platform must install exactly once");
    let framebuffer = Framebuffer::new().expect("the bounded external framebuffer must allocate");

    loop {
        let mut run_id_bytes = [0_u8; 37];
        let mode = unsafe { deskkin_wait_command(run_id_bytes.as_mut_ptr()) };
        let length = run_id_bytes.iter().position(|byte| *byte == 0).unwrap_or(36);
        let run_id_raw = &run_id_bytes[..length];
        let run_id = core::str::from_utf8(run_id_raw).unwrap_or("invalid");
        let mode_name: &[u8] = if mode == 2 {
            b"qualification\0"
        } else {
            b"conformance\0"
        };
        unsafe {
            deskkin_print_boot(
                run_id_bytes.as_ptr(),
                mode_name.as_ptr().cast::<c_char>(),
            );
        }
        if !unsafe { deskkin_devices_ready() } {
            printkln!("DESKKIN_GATE_RESULT schema=1 run_id={} result=fail", run_id);
        } else {
            let disabled = run_phase(state.clone(), &framebuffer, run_id, "disabled", false).await;
            match disabled {
                Ok(summary) => {
                    print_summary(run_id, "disabled", &summary);
                    if mode == 2 {
                        match run_phase(state.clone(), &framebuffer, run_id, "enabled", true).await {
                            Ok(enabled) => {
                                print_summary(run_id, "enabled", &enabled);
                                printkln!(
                                    "DESKKIN_GATE_RESULT schema=1 run_id={} result=pass",
                                    run_id
                                );
                            }
                            Err(error) => {
                                print_phase_error(run_id, "enabled", error);
                                printkln!(
                                    "DESKKIN_GATE_RESULT schema=1 run_id={} result=fail",
                                    run_id
                                );
                            }
                        }
                    } else {
                        printkln!(
                            "DESKKIN_GATE_RESULT schema=1 run_id={} result=pass",
                            run_id
                        );
                    }
                }
                Err(error) => {
                    print_phase_error(run_id, "disabled", error);
                    printkln!(
                        "DESKKIN_GATE_RESULT schema=1 run_id={} result=fail",
                        run_id
                    );
                }
            }
        }
        unsafe { deskkin_print_idle(run_id_bytes.as_ptr()) };
    }
}
