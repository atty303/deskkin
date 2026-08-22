// SPDX-License-Identifier: GPL-3.0-only

extern crate alloc;

#[path = "../shared.rs"]
mod shared;

use std::{boxed::Box, cell::RefCell, env, fs, rc::Rc, sync::OnceLock, thread, time::{Duration, Instant}};
use slint::platform::{Platform, WindowAdapter};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};

static START: OnceLock<Instant> = OnceLock::new();

struct HostPlatform {
    window: Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>,
}

impl Platform for HostPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        self.window.replace(Some(window.clone()));
        Ok(window)
    }

    fn duration_since_start(&self) -> Duration {
        START.get_or_init(Instant::now).elapsed()
    }
}

fn main() {
    let output = env::args_os().nth(1).expect("framebuffer output path is required");
    let state = Rc::new(RefCell::new(None));
    slint::platform::set_platform(Box::new(HostPlatform { window: state.clone() }))
        .expect("the host platform must install exactly once");
    let owner = shared::UiOwner::new(state);
    let mut pixels = shared::framebuffer();

    slint::platform::update_timers_and_animations();
    owner.render(&mut pixels);
    owner.inject_tap();
    for _ in 0..10 {
        thread::sleep(Duration::from_millis(15));
        slint::platform::update_timers_and_animations();
        owner.render(&mut pixels);
    }

    let mut bytes = Vec::with_capacity(pixels.len() * 2);
    for pixel in pixels {
        bytes.extend_from_slice(&pixel.0.to_le_bytes());
    }
    fs::write(output, bytes).expect("the host framebuffer must be written");
    println!(
        "DESKKIN_GATE1B_HOST schema=1 callback_count={} phase={} timer_waits=10 busy_polls=0",
        owner.activation_count(),
        owner.phase()
    );
}
