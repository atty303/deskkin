//! Deterministic art preview; no host identity, network, or diagnostic recording.
use std::{fs::OpenOptions, io::Write, path::PathBuf, rc::Rc};

use deskkin_application::ApplicationViews;
use deskkin_simulator::StatusWindow;
use slint::{
    ComponentHandle, PhysicalSize,
    platform::{
        Platform, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    },
};

#[path = "../src/world.rs"]
mod world;

struct PreviewPlatform(Rc<MinimalSoftwareWindow>);
impl Platform for PreviewPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

struct IncompleteExport(Option<PathBuf>);
impl Drop for IncompleteExport {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(
        args.next()
            .ok_or("expected new OUTPUT.ppm [DRAG_PX] [TIME_MS]")?,
    );
    let drag: i16 = args.next().unwrap_or_else(|| "0".into()).parse()?;
    let elapsed: u32 = args.next().unwrap_or_else(|| "0".into()).parse()?;
    if args.next().is_some() {
        return Err("too many arguments".into());
    }
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(PreviewPlatform(window.clone())))?;
    let ui = StatusWindow::new()?;
    window.set_size(PhysicalSize::new(320, 240));
    ui.show()?;
    ui.set_status_text("Unknown".into());
    ui.set_notice_visible(true);
    ui.set_notice_text("Deskkin notice".into());
    let mut scene = world::WorldScene::new();
    scene.touch_sample(0, true);
    scene.touch_sample(drag, true);
    scene.touch_sample(drag, false);
    scene.tick(
        &ui,
        ApplicationViews {
            availability: None,
            synthetic_notice: None,
        },
        elapsed,
    )?;
    let snapshot = ui.window().take_snapshot()?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let mut cleanup = IncompleteExport(Some(path));
    file.write_all(b"P6\n320 240\n255\n")?;
    for pixel in snapshot.as_slice() {
        file.write_all(&[pixel.r, pixel.g, pixel.b])?;
    }
    file.sync_all()?;
    cleanup.0 = None;
    Ok(())
}
