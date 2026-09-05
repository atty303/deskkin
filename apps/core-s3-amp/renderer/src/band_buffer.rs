use super::buffer_ownership::BufferOwnership;
use super::{
    BAND_PIXELS, BAND_ROWS, BUFFER_COUNT, HEIGHT, RendererFault, RendererProgress, RendererStage,
    Rgb565BePixel, WIDTH, deskkin_display_submit, deskkin_display_take_completion,
    deskkin_framebuffer_alloc, deskkin_raster_profile, deskkin_renderer_observe,
    deskkin_renderer_progress, deskkin_uptime_us, elapsed_us,
};
use alloc::rc::Rc;
use core::{marker::PhantomData, ptr::NonNull};
use slint::platform::software_renderer::LineBufferProvider;

#[repr(C)]
#[derive(Default)]
pub(super) struct BandCompletion {
    buffer_index: u8,
    result: i8,
    reserved: u16,
    duration_us: u32,
    wait_us: u32,
}

#[derive(Clone, Copy, Default)]
struct PendingFrame {
    final_band: bool,
    render_us: u32,
    profile: Option<[u32; 16]>,
}

pub(super) struct Framebuffer {
    pixels: [NonNull<u16>; BUFFER_COUNT],
    pending: [PendingFrame; BUFFER_COUNT],
    pub(super) back: usize,
    ownership: BufferOwnership<BUFFER_COUNT>,
    started: u64,
    wait_us: u32,
    _single_threaded: PhantomData<Rc<()>>,
}

impl Framebuffer {
    pub(super) fn new() -> Option<Self> {
        Some(Self {
            pixels: [
                NonNull::new(unsafe { deskkin_framebuffer_alloc(0) })?,
                NonNull::new(unsafe { deskkin_framebuffer_alloc(1) })?,
            ],
            pending: [PendingFrame::default(); BUFFER_COUNT],
            back: 0,
            ownership: BufferOwnership::new(),
            started: 0,
            wait_us: 0,
            _single_threaded: PhantomData,
        })
    }

    pub(super) fn words_mut(&mut self, index: usize) -> &mut [u16] {
        unsafe { core::slice::from_raw_parts_mut(self.pixels[index].as_ptr(), BAND_PIXELS) }
    }

    fn take_completion(&mut self, wait: bool) -> Result<bool, RendererFault> {
        let mut completion = BandCompletion::default();
        let result = unsafe { deskkin_display_take_completion(&mut completion, wait) };
        if result == 0 {
            return Ok(false);
        }
        let index = usize::from(completion.buffer_index);
        if result < 0 || self.ownership.complete(index).is_err() {
            return Err(RendererFault::Completion);
        }
        let frame = self.pending[index];
        if frame.final_band {
            if let Some(mut profile) = frame.profile {
                profile[14] = completion.wait_us;
                profile[15] = completion.duration_us;
                unsafe { deskkin_raster_profile(profile.as_ptr()) };
            }
            unsafe {
                deskkin_renderer_observe(
                    RendererStage::Presented as u8,
                    RendererFault::None as u8,
                    frame.render_us,
                    completion.duration_us,
                )
            };
        }
        Ok(true)
    }

    pub(super) fn publish_completions(&mut self) -> Result<(), RendererFault> {
        while self.take_completion(false)? {}
        Ok(())
    }

    pub(super) fn begin_frame(&mut self) -> Result<(), RendererFault> {
        self.publish_completions()?;
        self.started = unsafe { deskkin_uptime_us() };
        self.wait_us = 0;
        unsafe {
            deskkin_renderer_observe(
                RendererStage::Rendering as u8,
                RendererFault::None as u8,
                0,
                0,
            )
        };
        Ok(())
    }

    pub(super) fn begin_band(&mut self) -> Result<(), RendererFault> {
        unsafe { deskkin_renderer_progress(RendererProgress::Buffer as u8) };
        let started = unsafe { deskkin_uptime_us() };
        while self.ownership.is_inflight(self.back) {
            self.take_completion(true)?;
        }
        self.wait_us = self
            .wait_us
            .saturating_add(elapsed_us(started, unsafe { deskkin_uptime_us() }));
        self.ownership
            .begin_render(self.back)
            .map_err(|_| RendererFault::Ownership)?;
        unsafe { deskkin_renderer_progress(RendererProgress::Raster as u8) };
        Ok(())
    }

    pub(super) fn submit_band(
        &mut self,
        y: usize,
        rows: usize,
        mut profile: Option<[u32; 16]>,
    ) -> Result<(), RendererFault> {
        let index = self.back;
        let final_band = y + rows == HEIGHT;
        let render_us =
            elapsed_us(self.started, unsafe { deskkin_uptime_us() }).saturating_sub(self.wait_us);
        if let Some(values) = profile.as_mut() {
            values[13] = self.wait_us;
        }
        self.pending[index] = PendingFrame {
            final_band,
            render_us,
            profile,
        };
        self.ownership
            .submit(index)
            .map_err(|_| RendererFault::Ownership)?;
        unsafe { deskkin_renderer_progress(RendererProgress::Submit as u8) };
        if unsafe { deskkin_display_submit(index as u8, y as u16, rows as u16) } != 0 {
            self.ownership.submission_failed(index);
            return Err(RendererFault::Submit);
        }
        self.back ^= 1;
        Ok(())
    }
}

pub(super) struct ShellBands<'a> {
    pub(super) framebuffer: &'a mut Framebuffer,
    pub(super) next_line: usize,
    pub(super) fault: Option<RendererFault>,
}

impl LineBufferProvider for &mut ShellBands<'_> {
    type TargetPixel = Rgb565BePixel;

    fn process_line(
        &mut self,
        line: usize,
        range: core::ops::Range<usize>,
        render: impl FnOnce(&mut [Rgb565BePixel]),
    ) {
        if self.fault.is_some() {
            return;
        }
        if line != self.next_line || range != (0..WIDTH) || line >= HEIGHT {
            self.fault = Some(RendererFault::RenderSkipped);
            return;
        }
        if line % BAND_ROWS == 0 {
            if let Err(fault) = self.framebuffer.begin_band() {
                self.fault = Some(fault);
                return;
            }
        }
        let index = self.framebuffer.back;
        let start = line % BAND_ROWS * WIDTH;
        let words = &mut self.framebuffer.words_mut(index)[start..start + WIDTH];
        let pixels = unsafe { core::slice::from_raw_parts_mut(words.as_mut_ptr().cast(), WIDTH) };
        render(pixels);
        self.next_line += 1;
        if self.next_line % BAND_ROWS == 0 || self.next_line == HEIGHT {
            let y = line / BAND_ROWS * BAND_ROWS;
            if let Err(fault) = self.framebuffer.submit_band(y, self.next_line - y, None) {
                self.fault = Some(fault);
            }
        }
    }
}
