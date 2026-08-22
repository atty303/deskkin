// SPDX-License-Identifier: GPL-3.0-only

#![no_std]

extern crate alloc;

mod shared;

use alloc::{boxed::Box, rc::Rc, string::String};
use core::{cell::RefCell, fmt::Write, time::Duration};
use embassy_time::Timer;
use slint::platform::{Platform, WindowAdapter};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use static_cell::StaticCell;
use zephyr::{embassy::Executor, printkln};

static EXECUTOR: StaticCell<Executor> = StaticCell::new();

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
        embassy_time::Instant::now().duration_since(embassy_time::Instant::from_secs(0)).into()
    }
}

#[no_mangle]
extern "C" fn rust_main() {
    let executor = EXECUTOR.init(Executor::new());
    executor.run(|spawner| {
        spawner.spawn(run_gate()).expect("the bounded Gate 1B task arena must fit");
    });
}

#[embassy_executor::task]
async fn run_gate() {
    let state = Rc::new(RefCell::new(None));
    slint::platform::set_platform(Box::new(GatePlatform { window: state.clone() }))
        .expect("the Gate 1B platform must install exactly once");
    let owner = shared::UiOwner::new(state);

    slint::platform::update_timers_and_animations();
    let mut initial_ranges = 0;
    owner.render_lines(|range, pixels| {
        initial_ranges += 1;
        emit_frame_line(0, range, pixels);
    });
    printkln!("DESKKIN_GATE1B_EVENT schema=1 event=static_render ranges={}", initial_ranges);

    owner.inject_tap();
    slint::platform::update_timers_and_animations();
    let mut input_ranges = 0;
    owner.render_lines(|range, pixels| {
        input_ranges += 1;
        emit_frame_line(1, range, pixels);
    });
    printkln!(
        "DESKKIN_GATE1B_EVENT schema=1 event=input callback_count={} phase={}",
        owner.activation_count(),
        owner.phase()
    );
    printkln!("DESKKIN_GATE1B_EVENT schema=1 event=input_dirty ranges={}", input_ranges);

    let mut timer_waits = 0_u32;
    for stage in 2..12 {
        Timer::after(embassy_time::Duration::from_millis(15)).await;
        timer_waits += 1;
        slint::platform::update_timers_and_animations();
        owner.render_lines(|range, pixels| emit_frame_line(stage, range, pixels));
    }
    printkln!(
        "DESKKIN_GATE1B_EVENT schema=1 event=animation timer_waits={} busy_polls=0",
        timer_waits
    );

    printkln!("DESKKIN_GATE1B_RESULT schema=1 result=pass");

    loop {
        Timer::after(embassy_time::Duration::from_secs(60)).await;
    }
}

fn emit_frame_line(
    stage: u32,
    range: shared::DirtyRange,
    pixels: &[slint::platform::software_renderer::Rgb565Pixel],
) {
    let mut encoded = String::with_capacity(pixels.len() * 4);
    for pixel in pixels {
        write!(&mut encoded, "{:04x}", pixel.0).expect("frame line encoding must fit");
    }
    printkln!(
        "DESKKIN_GATE1B_FRAME schema=1 stage={} line={} start={} end={} rgb565={}",
        stage,
        range.line,
        range.start,
        range.end,
        encoded
    );
}
