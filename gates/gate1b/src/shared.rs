// SPDX-License-Identifier: GPL-3.0-only

extern crate alloc;

use alloc::rc::Rc;
#[cfg(feature = "host")]
use alloc::{vec, vec::Vec};
use core::{cell::{Cell, RefCell}, marker::PhantomData, ops::Range};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::platform::software_renderer::{
    LineBufferProvider, MinimalSoftwareWindow, Rgb565Pixel,
};

slint::include_modules!();

pub const WIDTH: usize = 320;
pub const HEIGHT: usize = 240;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DirtyRange {
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

pub struct UiOwner {
    component: GateWindow,
    window: Rc<MinimalSoftwareWindow>,
    activations: Rc<Cell<u32>>,
    _single_threaded: PhantomData<Rc<()>>,
}

impl UiOwner {
    pub fn new(window_state: Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>) -> Self {
        let component = GateWindow::new().expect("the Gate 1B component must instantiate");
        let window = window_state.borrow().clone().expect("Slint must request exactly one Gate 1B window");
        window.set_size(PhysicalSize::new(WIDTH as u32, HEIGHT as u32));
        let activations = Rc::new(Cell::new(0));
        let callback_activations = activations.clone();
        let weak = component.as_weak();
        component.on_activated(move || {
            callback_activations.set(callback_activations.get() + 1);
            if let Some(component) = weak.upgrade() {
                component.set_phase(1);
            }
        });
        component.show().expect("the Gate 1B component must show");
        Self {
            component,
            window,
            activations,
            _single_threaded: PhantomData,
        }
    }

    #[cfg(feature = "host")]
    pub fn render(&self, framebuffer: &mut [Rgb565Pixel]) -> Vec<DirtyRange> {
        let mut ranges = Vec::new();
        self.render_lines(|range, pixels| {
            framebuffer[range.line * WIDTH + range.start..range.line * WIDTH + range.end]
                .copy_from_slice(pixels);
            ranges.push(range);
        });
        ranges
    }

    pub fn render_lines(&self, mut process: impl FnMut(DirtyRange, &[Rgb565Pixel])) {
        self.window.draw_if_needed(|renderer| {
            renderer.render_by_line(&mut Capture {
                buffer: [Rgb565Pixel(0); WIDTH],
                process: &mut process,
            });
        });
    }

    pub fn inject_tap(&self) {
        let position = LogicalPosition::new(160.0, 120.0);
        self.window.dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        self.window.dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
    }

    pub fn activation_count(&self) -> u32 {
        self.activations.get()
    }

    pub fn phase(&self) -> i32 {
        self.component.get_phase()
    }
}

#[cfg(feature = "host")]
pub fn framebuffer() -> Vec<Rgb565Pixel> {
    vec![Rgb565Pixel(0); WIDTH * HEIGHT]
}

struct Capture<'a> {
    buffer: [Rgb565Pixel; WIDTH],
    process: &'a mut dyn FnMut(DirtyRange, &[Rgb565Pixel]),
}

impl LineBufferProvider for &mut Capture<'_> {
    type TargetPixel = Rgb565Pixel;

    fn process_line(
        &mut self,
        line: usize,
        range: Range<usize>,
        render_fn: impl FnOnce(&mut [Self::TargetPixel]),
    ) {
        let destination = &mut self.buffer[range.clone()];
        render_fn(destination);
        (self.process)(DirtyRange { line, start: range.start, end: range.end }, destination);
    }
}
