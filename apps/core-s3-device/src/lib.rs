// SPDX-License-Identifier: GPL-3.0-only

#![no_std]

extern crate alloc;

use alloc::{boxed::Box, rc::Rc};
use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use core::{
    cell::RefCell, ffi::c_int, marker::PhantomData, ops::Range, ptr::NonNull, time::Duration,
};
use deskkin_presentation::PetAnimator;
use embassy_time::Instant;
use slint::platform::software_renderer::{
    LineBufferProvider, MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel,
};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize};
use snow::params::{CipherChoice, DHChoice, HashChoice};
use snow::resolvers::{CryptoResolver, DefaultResolver};
use snow::types::{Cipher, Dh, Hash, Random};
use static_cell::StaticCell;
use zephyr::embassy::Executor;
use zeroize::Zeroize;

slint::include_modules!();

const NOISE_PATTERN: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
static EXECUTOR: StaticCell<Executor> = StaticCell::new();
static UI_ACTION: AtomicU8 = AtomicU8::new(0);
static UI_SAS: AtomicU32 = AtomicU32::new(u32::MAX);
static UI_VIEW: AtomicU8 = AtomicU8::new(0);
static UI_SHELL: AtomicU8 = AtomicU8::new(0);
static UI_FRAME_DIGEST: AtomicU32 = AtomicU32::new(0);
static VALID_RESULT: AtomicU8 = AtomicU8::new(0);
static RUN_ATTEMPT: AtomicU32 = AtomicU32::new(0);
static RESULT_ATTEMPT: AtomicU32 = AtomicU32::new(0);
static FRAME_ATTEMPT: AtomicU32 = AtomicU32::new(0);
static LAST_STAGE: AtomicU8 = AtomicU8::new(0);
static LAST_ERROR: AtomicU8 = AtomicU8::new(0);
static BOOT_STAGE: AtomicU8 = AtomicU8::new(BootStage::Starting as u8);
static BOOT_ERROR: AtomicU8 = AtomicU8::new(BootError::None as u8);
static APPLICATION_RUNNING: AtomicU8 = AtomicU8::new(1);
static PET_BENCHMARK_STATE: AtomicU8 = AtomicU8::new(0);
static PET_BENCHMARK_DURATION_MS: AtomicU32 = AtomicU32::new(0);
static PET_BENCHMARK_UPDATE_REQUESTS: AtomicU32 = AtomicU32::new(0);
static PET_BENCHMARK_COMPLETED_FRAMES: AtomicU32 = AtomicU32::new(0);
static PET_BENCHMARK_RENDER_TOTAL_US: AtomicU32 = AtomicU32::new(0);
static PET_BENCHMARK_TRANSFER_TOTAL_US: AtomicU32 = AtomicU32::new(0);
static PET_BENCHMARK_RENDER_MAX_US: AtomicU32 = AtomicU32::new(0);
static PET_BENCHMARK_TRANSFER_MAX_US: AtomicU32 = AtomicU32::new(0);
static PET_BENCHMARK_WITHIN_BUDGET: AtomicU32 = AtomicU32::new(0);
static PET_BENCHMARK_STALLS: AtomicU32 = AtomicU32::new(0);
static PET_BENCHMARK_DEADLINE_MISSES: AtomicU32 = AtomicU32::new(0);
static PET_BENCHMARK_MAX_CONSECUTIVE_MISSES: AtomicU32 = AtomicU32::new(0);
static PET_BENCHMARK_DIRTY_LINES: AtomicU32 = AtomicU32::new(0);
static PET_BENCHMARK_TRANSFERRED_BYTES: AtomicU32 = AtomicU32::new(0);
static PET_BENCHMARK_DIGEST_UPDATES: AtomicU32 = AtomicU32::new(0);
static PET_BENCHMARK_TRANSFER_FAILURES: AtomicU32 = AtomicU32::new(0);
static PET_BENCHMARK_ALLOCATION_FAILURES: AtomicU8 = AtomicU8::new(0);
static SESSION_CONTEXT: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];
static OPERATION_CONTEXT: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];
static OWNER_GENERATION_HIGH: AtomicU32 = AtomicU32::new(0);
static OWNER_GENERATION_LOW: AtomicU32 = AtomicU32::new(0);
static PEER_STATE: AtomicU8 = AtomicU8::new(deskkin_core_s3::PeerState::Unpaired as u8);
static CONFIG_PRESENT: AtomicU8 = AtomicU8::new(0);
static PEER_ID: [AtomicU32; 8] = [const { AtomicU32::new(0) }; 8];
const WIDTH: usize = 320;
const HEIGHT: usize = 240;

unsafe extern "C" {
    fn deskkin_boot_trace(stage: u8, error: u8);
    fn deskkin_csrand(output: *mut u8, length: usize) -> c_int;
    fn deskkin_start_service_worker() -> c_int;
    fn deskkin_service_take_command(output: *mut u8, capacity: usize) -> c_int;
    fn deskkin_service_publish_completion(input: *const u8, length: usize) -> c_int;
    fn deskkin_nvs_read(record_id: u16, output: *mut u8, capacity: usize) -> c_int;
    fn deskkin_nvs_write_readback(record_id: u16, input: *const u8, length: usize) -> c_int;
    fn deskkin_nvs_delete(record_id: u16) -> c_int;
    fn deskkin_framebuffer_alloc() -> *mut u16;
    fn deskkin_staging_alloc() -> *mut u16;
    fn deskkin_allocation_failures() -> u8;
    fn deskkin_display_write(
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        pitch: u16,
        pixels: *const u16,
    ) -> c_int;
    fn deskkin_display_enable() -> c_int;
    fn deskkin_take_touch(x: *mut i32, y: *mut i32) -> bool;
    fn deskkin_wifi_disconnect() -> c_int;
    fn deskkin_wifi_associate(
        ssid: *const u8,
        ssid_length: u8,
        psk: *const u8,
        psk_length: u8,
    ) -> c_int;
    fn deskkin_wait_dhcp() -> c_int;
    fn deskkin_tcp_connect(host: *const u8, port: u16) -> c_int;
    fn deskkin_tcp_set_timeout(descriptor: c_int, timeout_ms: u32) -> c_int;
    fn deskkin_tcp_read(descriptor: c_int, output: *mut u8, length: usize) -> c_int;
    fn deskkin_tcp_write(descriptor: c_int, input: *const u8, length: usize) -> c_int;
    fn deskkin_tcp_close(descriptor: c_int) -> c_int;
    fn deskkin_sleep_ms(delay_ms: u32);
    fn deskkin_uptime_ms() -> u64;
}

const IDENTITY_A: u16 = 0x100;
const IDENTITY_B: u16 = 0x101;
const IDENTITY_INTENT: u16 = 0x102;
const CONFIG_A: u16 = 0x200;
const CONFIG_B: u16 = 0x201;
const CONFIG_INTENT: u16 = 0x202;

#[derive(Clone, Copy)]
enum ServiceStatus {
    Success = 0,
    Invalid = 1,
    AlreadyInitialized = 2,
    Missing = 3,
    StoreFailed = 4,
    PeerMismatch = 5,
    Busy = 6,
}

#[derive(Clone, Copy)]
#[repr(u8)]
enum BootStage {
    Starting = 1,
    NoiseResolverReady = 4,
    ServiceWorkerReady = 5,
    UiPlatformReady = 6,
    UiComponentReady = 7,
    FramebufferReady = 8,
    FirstFrameReady = 9,
}

#[derive(Clone, Copy)]
#[repr(u8)]
enum BootError {
    None = 0,
    NoiseResolver = 2,
    ServiceWorker = 3,
    UiPlatform = 4,
    UiComponent = 5,
    Framebuffer = 6,
    DisplayTransfer = 7,
    DisplayEnable = 8,
}

#[derive(Clone, Copy, Eq, PartialEq)]
#[repr(u8)]
enum PetBenchmarkState {
    Idle = 0,
    Active = 1,
    Complete = 2,
    Failed = 3,
}

fn set_boot_stage(stage: BootStage) {
    BOOT_STAGE.store(stage as u8, Ordering::Release);
    unsafe { deskkin_boot_trace(stage as u8, BootError::None as u8) };
}

fn fail_boot(error: BootError) {
    BOOT_ERROR.store(error as u8, Ordering::Release);
    unsafe { deskkin_boot_trace(BOOT_STAGE.load(Ordering::Acquire), error as u8) };
}

fn reset_pet_benchmark() {
    for value in [
        &PET_BENCHMARK_DURATION_MS,
        &PET_BENCHMARK_UPDATE_REQUESTS,
        &PET_BENCHMARK_COMPLETED_FRAMES,
        &PET_BENCHMARK_RENDER_TOTAL_US,
        &PET_BENCHMARK_TRANSFER_TOTAL_US,
        &PET_BENCHMARK_RENDER_MAX_US,
        &PET_BENCHMARK_TRANSFER_MAX_US,
        &PET_BENCHMARK_WITHIN_BUDGET,
        &PET_BENCHMARK_STALLS,
        &PET_BENCHMARK_DEADLINE_MISSES,
        &PET_BENCHMARK_MAX_CONSECUTIVE_MISSES,
        &PET_BENCHMARK_DIRTY_LINES,
        &PET_BENCHMARK_TRANSFERRED_BYTES,
        &PET_BENCHMARK_DIGEST_UPDATES,
        &PET_BENCHMARK_TRANSFER_FAILURES,
    ] {
        value.store(0, Ordering::Relaxed);
    }
    PET_BENCHMARK_ALLOCATION_FAILURES
        .store(unsafe { deskkin_allocation_failures() }, Ordering::Relaxed);
    PET_BENCHMARK_STATE.store(PetBenchmarkState::Active as u8, Ordering::Release);
}

fn publish_pet_benchmark(summary: &deskkin_core_s3::PetBenchmarkSummary, state: PetBenchmarkState) {
    PET_BENCHMARK_DURATION_MS.store(summary.duration_ms, Ordering::Relaxed);
    PET_BENCHMARK_UPDATE_REQUESTS.store(summary.update_requests, Ordering::Relaxed);
    PET_BENCHMARK_COMPLETED_FRAMES.store(summary.completed_frames, Ordering::Relaxed);
    PET_BENCHMARK_RENDER_TOTAL_US.store(summary.render_total_us, Ordering::Relaxed);
    PET_BENCHMARK_TRANSFER_TOTAL_US.store(summary.transfer_total_us, Ordering::Relaxed);
    PET_BENCHMARK_RENDER_MAX_US.store(summary.render_max_us, Ordering::Relaxed);
    PET_BENCHMARK_TRANSFER_MAX_US.store(summary.transfer_max_us, Ordering::Relaxed);
    PET_BENCHMARK_WITHIN_BUDGET.store(summary.frames_within_budget, Ordering::Relaxed);
    PET_BENCHMARK_STALLS.store(u32::from(summary.stalls), Ordering::Relaxed);
    PET_BENCHMARK_DEADLINE_MISSES.store(u32::from(summary.deadline_misses), Ordering::Relaxed);
    PET_BENCHMARK_MAX_CONSECUTIVE_MISSES
        .store(u32::from(summary.max_consecutive_misses), Ordering::Relaxed);
    PET_BENCHMARK_DIRTY_LINES.store(summary.dirty_lines, Ordering::Relaxed);
    PET_BENCHMARK_TRANSFERRED_BYTES.store(summary.transferred_bytes, Ordering::Relaxed);
    PET_BENCHMARK_DIGEST_UPDATES.store(summary.digest_updates, Ordering::Relaxed);
    PET_BENCHMARK_TRANSFER_FAILURES.store(u32::from(summary.transfer_failures), Ordering::Relaxed);
    PET_BENCHMARK_ALLOCATION_FAILURES.store(summary.allocation_failures, Ordering::Relaxed);
    PET_BENCHMARK_STATE.store(state as u8, Ordering::Release);
}

fn encode_pet_benchmark(output: &mut [u8]) {
    output[26] = PET_BENCHMARK_STATE.load(Ordering::Acquire);
    output[27] = PET_BENCHMARK_ALLOCATION_FAILURES.load(Ordering::Relaxed);
    let transfer_failures = PET_BENCHMARK_TRANSFER_FAILURES
        .load(Ordering::Relaxed)
        .try_into()
        .unwrap_or(u16::MAX);
    output[28..30].copy_from_slice(&transfer_failures.to_be_bytes());
    for (range, value) in [
        (30..34, PET_BENCHMARK_DURATION_MS.load(Ordering::Relaxed)),
        (
            34..38,
            PET_BENCHMARK_UPDATE_REQUESTS.load(Ordering::Relaxed),
        ),
        (
            38..42,
            PET_BENCHMARK_COMPLETED_FRAMES.load(Ordering::Relaxed),
        ),
        (
            42..46,
            PET_BENCHMARK_RENDER_TOTAL_US.load(Ordering::Relaxed),
        ),
        (
            46..50,
            PET_BENCHMARK_TRANSFER_TOTAL_US.load(Ordering::Relaxed),
        ),
        (50..54, PET_BENCHMARK_RENDER_MAX_US.load(Ordering::Relaxed)),
        (
            54..58,
            PET_BENCHMARK_TRANSFER_MAX_US.load(Ordering::Relaxed),
        ),
        (58..62, PET_BENCHMARK_WITHIN_BUDGET.load(Ordering::Relaxed)),
        (68..72, PET_BENCHMARK_DIRTY_LINES.load(Ordering::Relaxed)),
        (
            72..76,
            PET_BENCHMARK_TRANSFERRED_BYTES.load(Ordering::Relaxed),
        ),
        (76..80, PET_BENCHMARK_DIGEST_UPDATES.load(Ordering::Relaxed)),
    ] {
        output[range].copy_from_slice(&value.to_be_bytes());
    }
    for (range, value) in [
        (62..64, PET_BENCHMARK_STALLS.load(Ordering::Relaxed)),
        (
            64..66,
            PET_BENCHMARK_DEADLINE_MISSES.load(Ordering::Relaxed),
        ),
        (
            66..68,
            PET_BENCHMARK_MAX_CONSECUTIVE_MISSES.load(Ordering::Relaxed),
        ),
    ] {
        let value = value.try_into().unwrap_or(u16::MAX);
        output[range].copy_from_slice(&value.to_be_bytes());
    }
}

struct ZephyrRandom;

impl Random for ZephyrRandom {
    fn try_fill_bytes(&mut self, destination: &mut [u8]) -> Result<(), snow::Error> {
        let result = unsafe { deskkin_csrand(destination.as_mut_ptr(), destination.len()) };
        (result == 0).then_some(()).ok_or(snow::Error::Rng)
    }
}

struct ZephyrResolver {
    crypto: DefaultResolver,
}

impl ZephyrResolver {
    const fn new() -> Self {
        Self {
            crypto: DefaultResolver,
        }
    }
}

impl CryptoResolver for ZephyrResolver {
    fn resolve_rng(&self) -> Option<Box<dyn Random>> {
        Some(Box::new(ZephyrRandom))
    }

    fn resolve_dh(&self, choice: &DHChoice) -> Option<Box<dyn Dh>> {
        self.crypto.resolve_dh(choice)
    }

    fn resolve_hash(&self, choice: &HashChoice) -> Option<Box<dyn Hash>> {
        self.crypto.resolve_hash(choice)
    }

    fn resolve_cipher(&self, choice: &CipherChoice) -> Option<Box<dyn Cipher>> {
        self.crypto.resolve_cipher(choice)
    }
}

struct DevicePlatform {
    window: Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>,
}

struct Framebuffer {
    pixels: NonNull<u16>,
    staging: NonNull<u16>,
    _single_threaded: PhantomData<Rc<()>>,
}

impl Framebuffer {
    fn new() -> Option<Self> {
        Some(Self {
            pixels: NonNull::new(unsafe { deskkin_framebuffer_alloc() })?,
            staging: NonNull::new(unsafe { deskkin_staging_alloc() })?,
            _single_threaded: PhantomData,
        })
    }

    fn line_pointer(&self, line: usize, column: usize) -> *const u16 {
        unsafe { self.pixels.as_ptr().add(line * WIDTH + column) }
    }

    fn digest(&self) -> u32 {
        let mut digest = 2_166_136_261_u32;
        for index in 0..WIDTH * HEIGHT {
            let pixel = unsafe { self.pixels.as_ptr().add(index).read() };
            digest ^= u32::from(pixel);
            digest = digest.wrapping_mul(16_777_619);
        }
        digest
    }
}

struct Capture<'a> {
    line: [Rgb565Pixel; WIDTH],
    ranges: &'a mut [deskkin_core_s3::DirtyRange; HEIGHT],
    framebuffer: &'a Framebuffer,
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
                self.framebuffer
                    .pixels
                    .as_ptr()
                    .add(line * WIDTH + range.start),
                destination.len(),
            );
        }
        self.ranges[line].include(range.start as u16, range.end as u16);
    }
}

fn transfer_dirty(
    framebuffer: &Framebuffer,
    ranges: &[deskkin_core_s3::DirtyRange; HEIGHT],
) -> Result<(), ()> {
    let mut line = 0;
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
                    framebuffer
                        .staging
                        .as_ptr()
                        .add(row * width + column)
                        .write(
                            framebuffer
                                .line_pointer(start_line + row, usize::from(range.start) + column)
                                .read()
                                .swap_bytes(),
                        );
                }
            }
        }
        if unsafe {
            deskkin_display_write(
                range.start,
                start_line as u16,
                range.end - range.start,
                height as u16,
                width as u16,
                framebuffer.staging.as_ptr(),
            )
        } != 0
        {
            return Err(());
        }
    }
    Ok(())
}

fn dirty_measurement(ranges: &[deskkin_core_s3::DirtyRange; HEIGHT]) -> (u32, u32) {
    ranges.iter().fold((0_u32, 0_u32), |(lines, bytes), range| {
        if range.start == range.end {
            (lines, bytes)
        } else {
            (
                lines.saturating_add(1),
                bytes.saturating_add(u32::from(range.end - range.start).saturating_mul(2)),
            )
        }
    })
}

fn elapsed_us(start: u64, end: u64) -> u32 {
    end.saturating_sub(start).try_into().unwrap_or(u32::MAX)
}

impl Platform for DevicePlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        self.window.replace(Some(window.clone()));
        Ok(window)
    }

    fn duration_since_start(&self) -> Duration {
        Instant::now().duration_since(Instant::from_secs(0)).into()
    }
}

fn prove_noise_resolver() -> Result<(), ()> {
    let parameters = NOISE_PATTERN.parse().map_err(|_| ())?;
    let resolver = Box::new(ZephyrResolver::new());
    let keypair = snow::Builder::with_resolver(parameters, resolver)
        .generate_keypair()
        .map_err(|_| ())?;
    let mut private = zeroize::Zeroizing::new(keypair.private);
    private.zeroize();
    Ok(())
}

fn read_slot(record_id: u16, output: &mut [u8]) -> Result<Option<usize>, ServiceStatus> {
    let length = unsafe { deskkin_nvs_read(record_id, output.as_mut_ptr(), output.len()) };
    if length == 0 {
        return Ok(None);
    }
    if length < 0 {
        return Err(ServiceStatus::StoreFailed);
    }
    let length = usize::try_from(length - 1).map_err(|_| ServiceStatus::StoreFailed)?;
    (length <= output.len())
        .then_some(Some(length))
        .ok_or(ServiceStatus::StoreFailed)
}

fn canonical_record<'a>(
    ids: (u16, u16, u16),
    first: &'a mut [u8],
    second: &'a mut [u8],
) -> Result<Option<deskkin_core_s3::RecordRef<'a>>, ServiceStatus> {
    let mut intent = [0_u8; 8];
    if read_slot(ids.2, &mut intent)?.is_some() {
        return Err(ServiceStatus::StoreFailed);
    }
    let first_length = read_slot(ids.0, first)?;
    let second_length = read_slot(ids.1, second)?;
    let first = first_length.map(|length| &first[..length]);
    let second = second_length.map(|length| &second[..length]);
    deskkin_core_s3::select_slot(first, second).map_err(|_| ServiceStatus::StoreFailed)
}

fn publish_record(
    ids: (u16, u16, u16),
    current_sequence: u64,
    generation: u64,
    state: deskkin_core_s3::RecordState,
    payload: &[u8],
) -> Result<(), ServiceStatus> {
    let sequence = current_sequence
        .checked_add(1)
        .ok_or(ServiceStatus::StoreFailed)?;
    let mut encoded = zeroize::Zeroizing::new([0_u8; deskkin_core_s3::NVS_RECORD_MAX]);
    let length =
        deskkin_core_s3::encode_record(sequence, generation, state, payload, &mut encoded[..])
            .map_err(|_| ServiceStatus::Invalid)?;
    let record_id = if sequence % 2 == 1 { ids.0 } else { ids.1 };
    let intent = sequence.to_be_bytes();
    if unsafe { deskkin_nvs_write_readback(ids.2, intent.as_ptr(), intent.len()) } != 0 {
        return Err(ServiceStatus::StoreFailed);
    }
    let result = unsafe { deskkin_nvs_write_readback(record_id, encoded.as_ptr(), length) };
    if result != 0 {
        return Err(ServiceStatus::StoreFailed);
    }
    let result = unsafe { deskkin_nvs_delete(ids.2) };
    (result == 0)
        .then_some(())
        .ok_or(ServiceStatus::StoreFailed)
}

fn identity_init() -> ServiceStatus {
    let mut first = zeroize::Zeroizing::new([0_u8; deskkin_core_s3::NVS_RECORD_MAX]);
    let mut second = zeroize::Zeroizing::new([0_u8; deskkin_core_s3::NVS_RECORD_MAX]);
    let result = match canonical_record(
        (IDENTITY_A, IDENTITY_B, IDENTITY_INTENT),
        &mut first[..],
        &mut second[..],
    ) {
        Ok(None) => {
            let parameters = match NOISE_PATTERN.parse() {
                Ok(parameters) => parameters,
                Err(_) => return ServiceStatus::Invalid,
            };
            let keypair =
                match snow::Builder::with_resolver(parameters, Box::new(ZephyrResolver::new()))
                    .generate_keypair()
                {
                    Ok(keypair) => keypair,
                    Err(_) => return ServiceStatus::StoreFailed,
                };
            let private = zeroize::Zeroizing::new(keypair.private);
            let mut payload = [0_u8; 64];
            payload[..32].copy_from_slice(&private);
            payload[32..].copy_from_slice(&keypair.public);
            let result = publish_record(
                (IDENTITY_A, IDENTITY_B, IDENTITY_INTENT),
                0,
                1,
                deskkin_core_s3::RecordState::Identity(deskkin_core_s3::PeerState::Unpaired),
                &payload,
            );
            payload.zeroize();
            result.map_or_else(
                |error| error,
                |()| {
                    store_generation(1);
                    ServiceStatus::Success
                },
            )
        }
        Ok(Some(_)) => ServiceStatus::AlreadyInitialized,
        Err(error) => error,
    };
    result
}

fn wifi_provision(payload: &[u8]) -> ServiceStatus {
    let Some((&ssid_length, remaining)) = payload.split_first() else {
        return ServiceStatus::Invalid;
    };
    let ssid_length = usize::from(ssid_length);
    if remaining.len() < ssid_length + 1 {
        return ServiceStatus::Invalid;
    }
    let (ssid, remaining) = remaining.split_at(ssid_length);
    let password_length = usize::from(remaining[0]);
    let remaining = &remaining[1..];
    if remaining.len() != password_length + 6 {
        return ServiceStatus::Invalid;
    }
    let (password, tail) = remaining.split_at(password_length);
    let config = deskkin_core_s3::WifiConfig {
        ssid,
        passphrase: password,
        host_ipv4: [tail[0], tail[1], tail[2], tail[3]],
    };
    if config.validate().is_err() || u16::from_be_bytes([tail[4], tail[5]]) != 39_042 {
        return ServiceStatus::Invalid;
    }
    let mut first = zeroize::Zeroizing::new([0_u8; deskkin_core_s3::NVS_RECORD_MAX]);
    let mut second = zeroize::Zeroizing::new([0_u8; deskkin_core_s3::NVS_RECORD_MAX]);
    let current = match canonical_record(
        (CONFIG_A, CONFIG_B, CONFIG_INTENT),
        &mut first[..],
        &mut second[..],
    ) {
        Ok(value) => value.map_or(0, |record| record.publication_sequence),
        Err(error) => return error,
    };
    let result = publish_record(
        (CONFIG_A, CONFIG_B, CONFIG_INTENT),
        current,
        1,
        deskkin_core_s3::RecordState::ConfigPresent,
        payload,
    )
    .map_or_else(|error| error, |()| ServiceStatus::Success);
    if matches!(result, ServiceStatus::Success) {
        CONFIG_PRESENT.store(1, Ordering::Release);
        let _ = unsafe { deskkin_wifi_disconnect() };
    }
    result
}

fn clear_config() -> ServiceStatus {
    let mut first = zeroize::Zeroizing::new([0_u8; deskkin_core_s3::NVS_RECORD_MAX]);
    let mut second = zeroize::Zeroizing::new([0_u8; deskkin_core_s3::NVS_RECORD_MAX]);
    let current = match canonical_record(
        (CONFIG_A, CONFIG_B, CONFIG_INTENT),
        &mut first[..],
        &mut second[..],
    ) {
        Ok(Some(record)) => (record.publication_sequence, record.generation),
        Ok(None) => return ServiceStatus::Missing,
        Err(error) => return error,
    };
    let result = publish_record(
        (CONFIG_A, CONFIG_B, CONFIG_INTENT),
        current.0,
        current.1,
        deskkin_core_s3::RecordState::ConfigCleared,
        &[],
    )
    .map_or_else(|error| error, |()| ServiceStatus::Success);
    if matches!(result, ServiceStatus::Success) {
        CONFIG_PRESENT.store(0, Ordering::Release);
        let _ = unsafe { deskkin_wifi_disconnect() };
    }
    result
}

struct Revocation {
    sequence: u64,
    generation: u64,
    local_identity: zeroize::Zeroizing<[u8; 64]>,
}

fn recover_identity_intent(peer_id: &[u8]) -> Result<Option<ServiceStatus>, ServiceStatus> {
    let mut intent = [0_u8; 8];
    let Some(intent_length) = read_slot(IDENTITY_INTENT, &mut intent)? else {
        return Ok(None);
    };
    if intent_length != intent.len() {
        return Err(ServiceStatus::StoreFailed);
    }
    let sequence = u64::from_be_bytes(intent);
    let mut first = zeroize::Zeroizing::new([0_u8; deskkin_core_s3::NVS_RECORD_MAX]);
    let mut second = zeroize::Zeroizing::new([0_u8; deskkin_core_s3::NVS_RECORD_MAX]);
    let first_length = read_slot(IDENTITY_A, &mut first[..])?;
    let second_length = read_slot(IDENTITY_B, &mut second[..])?;
    let (target, previous) = if sequence % 2 == 1 {
        (
            first_length.map(|length| &first[..length]),
            second_length.map(|length| &second[..length]),
        )
    } else {
        (
            second_length.map(|length| &second[..length]),
            first_length.map(|length| &first[..length]),
        )
    };
    let target = target.ok_or(ServiceStatus::StoreFailed).and_then(|bytes| {
        deskkin_core_s3::decode_record(bytes).map_err(|_| ServiceStatus::StoreFailed)
    })?;
    if target.publication_sequence != sequence {
        return Err(ServiceStatus::StoreFailed);
    }
    let exact_peer = |record: deskkin_core_s3::RecordRef<'_>| {
        matches!(record.state, deskkin_core_s3::RecordState::Identity(_))
            && record.payload.len() >= 96
            && &record.payload[64..96] == peer_id
    };
    let completed = matches!(
        target.state,
        deskkin_core_s3::RecordState::Identity(deskkin_core_s3::PeerState::Unpaired)
    ) && target.payload.len() == 64
        && previous
            .and_then(|bytes| deskkin_core_s3::decode_record(bytes).ok())
            .is_some_and(|record| {
                record.generation == target.generation
                    && matches!(
                        record.state,
                        deskkin_core_s3::RecordState::Identity(
                            deskkin_core_s3::PeerState::Revoking
                        )
                    )
                    && exact_peer(record)
            });
    if !completed && !exact_peer(target) {
        return Err(ServiceStatus::PeerMismatch);
    }
    if unsafe { deskkin_nvs_delete(IDENTITY_INTENT) } != 0 {
        return Err(ServiceStatus::StoreFailed);
    }
    if completed {
        store_generation(target.generation);
        PEER_STATE.store(
            deskkin_core_s3::PeerState::Unpaired as u8,
            Ordering::Release,
        );
        for word in &PEER_ID {
            word.store(0, Ordering::Release);
        }
        return Ok(Some(ServiceStatus::Success));
    }
    Ok(None)
}

enum ActiveControl {
    Shutdown {
        command_id: [u8; 16],
    },
    Unpair {
        command_id: [u8; 16],
        revocation: Revocation,
    },
}

fn begin_identity_unpair(peer_id: &[u8]) -> Result<Revocation, ServiceStatus> {
    if peer_id.len() != 32 {
        return Err(ServiceStatus::Invalid);
    }
    if let Some(status) = recover_identity_intent(peer_id)? {
        return Err(status);
    }
    let mut first = zeroize::Zeroizing::new([0_u8; deskkin_core_s3::NVS_RECORD_MAX]);
    let mut second = zeroize::Zeroizing::new([0_u8; deskkin_core_s3::NVS_RECORD_MAX]);
    let record = match canonical_record(
        (IDENTITY_A, IDENTITY_B, IDENTITY_INTENT),
        &mut first[..],
        &mut second[..],
    ) {
        Ok(Some(record)) => record,
        Ok(None) => return Err(ServiceStatus::Missing),
        Err(error) => return Err(error),
    };
    if !matches!(
        record.state,
        deskkin_core_s3::RecordState::Identity(
            deskkin_core_s3::PeerState::Pending
                | deskkin_core_s3::PeerState::Committing
                | deskkin_core_s3::PeerState::Paired
                | deskkin_core_s3::PeerState::Revoking
        )
    ) || record.payload.len() < 96
        || &record.payload[64..96] != peer_id
    {
        return Err(ServiceStatus::PeerMismatch);
    }
    let sequence = record.publication_sequence;
    let recovering = matches!(
        record.state,
        deskkin_core_s3::RecordState::Identity(deskkin_core_s3::PeerState::Revoking)
    );
    let generation = match if recovering {
        Some(record.generation)
    } else {
        record.generation.checked_add(1)
    } {
        Some(generation) => generation,
        None => return Err(ServiceStatus::StoreFailed),
    };
    let mut local_identity = zeroize::Zeroizing::new([0_u8; 64]);
    local_identity.copy_from_slice(&record.payload[..64]);
    if !recovering {
        if let Err(error) = publish_record(
            (IDENTITY_A, IDENTITY_B, IDENTITY_INTENT),
            sequence,
            generation,
            deskkin_core_s3::RecordState::Identity(deskkin_core_s3::PeerState::Revoking),
            record.payload,
        ) {
            return Err(error);
        }
    }
    store_generation(generation);
    PEER_STATE.store(
        deskkin_core_s3::PeerState::Revoking as u8,
        Ordering::Release,
    );
    UI_VIEW.store(0, Ordering::Release);
    Ok(Revocation {
        sequence: sequence + usize::from(!recovering) as u64,
        generation,
        local_identity,
    })
}

fn finish_identity_unpair(revocation: &Revocation) -> ServiceStatus {
    let result = publish_record(
        (IDENTITY_A, IDENTITY_B, IDENTITY_INTENT),
        revocation.sequence,
        revocation.generation,
        deskkin_core_s3::RecordState::Identity(deskkin_core_s3::PeerState::Unpaired),
        &revocation.local_identity[..],
    )
    .map_or_else(|error| error, |()| ServiceStatus::Success);
    if matches!(result, ServiceStatus::Success) {
        store_generation(revocation.generation);
        PEER_STATE.store(
            deskkin_core_s3::PeerState::Unpaired as u8,
            Ordering::Release,
        );
        for word in &PEER_ID {
            word.store(0, Ordering::Release);
        }
    }
    result
}

fn identity_unpair(peer_id: &[u8]) -> ServiceStatus {
    begin_identity_unpair(peer_id).map_or_else(
        |status| status,
        |revocation| finish_identity_unpair(&revocation),
    )
}

struct StoredIdentity {
    sequence: u64,
    generation: u64,
    state: deskkin_core_s3::PeerState,
    payload: [u8; 112],
    payload_length: usize,
}

impl Drop for StoredIdentity {
    fn drop(&mut self) {
        self.payload.zeroize();
    }
}

struct StoredConfig {
    ssid: [u8; 32],
    ssid_length: u8,
    passphrase: [u8; 63],
    passphrase_length: u8,
    host: [u8; 4],
}

impl Drop for StoredConfig {
    fn drop(&mut self) {
        self.ssid.zeroize();
        self.passphrase.zeroize();
    }
}

enum SessionFailure {
    Store,
    Wifi,
    Dhcp,
    Tcp,
    Noise,
    Protocol,
    Rejected,
    Incompatible,
    AuthorizationDenied,
    SessionBusy,
    Cancelled,
    Control(ActiveControl),
}

fn load_identity() -> Result<StoredIdentity, SessionFailure> {
    let mut first = zeroize::Zeroizing::new([0_u8; deskkin_core_s3::NVS_RECORD_MAX]);
    let mut second = zeroize::Zeroizing::new([0_u8; deskkin_core_s3::NVS_RECORD_MAX]);
    let record = canonical_record(
        (IDENTITY_A, IDENTITY_B, IDENTITY_INTENT),
        &mut first[..],
        &mut second[..],
    )
    .map_err(|_| SessionFailure::Store)?
    .ok_or(SessionFailure::Store)?;
    let deskkin_core_s3::RecordState::Identity(state) = record.state else {
        return Err(SessionFailure::Store);
    };
    if !(64..=112).contains(&record.payload.len()) {
        return Err(SessionFailure::Store);
    }
    let mut payload = [0_u8; 112];
    payload[..record.payload.len()].copy_from_slice(record.payload);
    let value = StoredIdentity {
        sequence: record.publication_sequence,
        generation: record.generation,
        state,
        payload,
        payload_length: record.payload.len(),
    };
    Ok(value)
}

fn load_config() -> Result<StoredConfig, SessionFailure> {
    let mut first = zeroize::Zeroizing::new([0_u8; deskkin_core_s3::NVS_RECORD_MAX]);
    let mut second = zeroize::Zeroizing::new([0_u8; deskkin_core_s3::NVS_RECORD_MAX]);
    let record = canonical_record(
        (CONFIG_A, CONFIG_B, CONFIG_INTENT),
        &mut first[..],
        &mut second[..],
    )
    .map_err(|_| SessionFailure::Store)?
    .ok_or(SessionFailure::Store)?;
    if record.state != deskkin_core_s3::RecordState::ConfigPresent {
        return Err(SessionFailure::Store);
    }
    let Some((&ssid_length, rest)) = record.payload.split_first() else {
        return Err(SessionFailure::Store);
    };
    let ssid_length_usize = usize::from(ssid_length);
    if rest.len() < ssid_length_usize + 1 {
        return Err(SessionFailure::Store);
    }
    let (ssid, rest) = rest.split_at(ssid_length_usize);
    let passphrase_length = rest[0];
    let passphrase_length_usize = usize::from(passphrase_length);
    let rest = &rest[1..];
    if rest.len() != passphrase_length_usize + 6 {
        return Err(SessionFailure::Store);
    }
    let (passphrase, tail) = rest.split_at(passphrase_length_usize);
    let host = [tail[0], tail[1], tail[2], tail[3]];
    let config = deskkin_core_s3::WifiConfig {
        ssid,
        passphrase,
        host_ipv4: host,
    };
    if config.validate().is_err() || u16::from_be_bytes([tail[4], tail[5]]) != 39_042 {
        return Err(SessionFailure::Store);
    }
    let mut value = StoredConfig {
        ssid: [0; 32],
        ssid_length,
        passphrase: [0; 63],
        passphrase_length,
        host,
    };
    value.ssid[..ssid_length_usize].copy_from_slice(ssid);
    value.passphrase[..passphrase_length_usize].copy_from_slice(passphrase);
    Ok(value)
}

fn deadline_after(delay_ms: u64) -> u64 {
    unsafe { deskkin_uptime_ms() }.saturating_add(delay_ms)
}

fn check_control(control_frame: &mut [u8]) -> Result<(), SessionFailure> {
    poll_active_control(control_frame)
        .map_or(Ok(()), |control| Err(SessionFailure::Control(control)))
}

fn write_all(
    descriptor: c_int,
    mut bytes: &[u8],
    deadline: u64,
    control_frame: &mut [u8],
) -> Result<(), SessionFailure> {
    while !bytes.is_empty() {
        check_control(control_frame)?;
        if unsafe { deskkin_uptime_ms() } >= deadline {
            return Err(SessionFailure::Tcp);
        }
        let written = unsafe { deskkin_tcp_write(descriptor, bytes.as_ptr(), bytes.len()) };
        if written < 0 {
            continue;
        }
        if written == 0 {
            return Err(SessionFailure::Tcp);
        }
        bytes = &bytes[written as usize..];
    }
    Ok(())
}

fn read_exact(
    descriptor: c_int,
    mut bytes: &mut [u8],
    deadline: u64,
    control_frame: &mut [u8],
) -> Result<(), SessionFailure> {
    while !bytes.is_empty() {
        check_control(control_frame)?;
        if unsafe { deskkin_uptime_ms() } >= deadline {
            return Err(SessionFailure::Tcp);
        }
        let read = unsafe { deskkin_tcp_read(descriptor, bytes.as_mut_ptr(), bytes.len()) };
        if read < 0 {
            continue;
        }
        if read == 0 {
            return Err(SessionFailure::Tcp);
        }
        bytes = &mut bytes[read as usize..];
    }
    Ok(())
}

fn write_frame(
    descriptor: c_int,
    bytes: &[u8],
    deadline: u64,
    control_frame: &mut [u8],
) -> Result<(), SessionFailure> {
    let length =
        deskkin_protocol::encode_frame_length(bytes.len()).map_err(|_| SessionFailure::Protocol)?;
    write_all(descriptor, &length, deadline, control_frame)?;
    write_all(descriptor, bytes, deadline, control_frame)
}

fn read_frame<'a>(
    descriptor: c_int,
    buffer: &'a mut [u8],
    deadline: u64,
    control_frame: &mut [u8],
) -> Result<&'a [u8], SessionFailure> {
    let mut prefix = [0_u8; 2];
    read_exact(descriptor, &mut prefix, deadline, control_frame)?;
    let length = deskkin_protocol::decode_frame_length(prefix);
    if length > buffer.len() {
        return Err(SessionFailure::Protocol);
    }
    read_exact(descriptor, &mut buffer[..length], deadline, control_frame)?;
    Ok(&buffer[..length])
}

fn write_message(
    descriptor: c_int,
    transport: &mut snow::TransportState,
    message: &deskkin_protocol::Message,
    deadline: u64,
    control_frame: &mut [u8],
) -> Result<(), SessionFailure> {
    let mut plain = [0_u8; 64];
    let encoded = message
        .encode(&mut plain)
        .map_err(|_| SessionFailure::Protocol)?;
    let mut encrypted = [0_u8; 128];
    let length = transport
        .write_message(encoded, &mut encrypted)
        .map_err(|_| SessionFailure::Noise)?;
    write_frame(descriptor, &encrypted[..length], deadline, control_frame)
}

fn read_message(
    descriptor: c_int,
    transport: &mut snow::TransportState,
    deadline: u64,
    control_frame: &mut [u8],
) -> Result<deskkin_protocol::Message, SessionFailure> {
    let mut encrypted = [0_u8; 128];
    let input = read_frame(descriptor, &mut encrypted, deadline, control_frame)?;
    let mut plain = [0_u8; 64];
    let length = transport
        .read_message(input, &mut plain)
        .map_err(|_| SessionFailure::Noise)?;
    deskkin_protocol::Message::decode(&plain[..length]).map_err(|_| SessionFailure::Protocol)
}

fn noise_connect(
    descriptor: c_int,
    identity: &StoredIdentity,
) -> Result<snow::HandshakeState, SessionFailure> {
    if unsafe { deskkin_tcp_set_timeout(descriptor, 10) } != 0 {
        return Err(SessionFailure::Tcp);
    }
    let deadline = deadline_after(5_000);
    let mut control_frame = [0_u8; deskkin_core_s3::CONTROL_PAYLOAD_MAX + 28];
    write_all(
        descriptor,
        &deskkin_protocol::PRELUDE,
        deadline,
        &mut control_frame,
    )?;
    let parameters = NOISE_PATTERN.parse().map_err(|_| SessionFailure::Noise)?;
    let mut noise = snow::Builder::with_resolver(parameters, Box::new(ZephyrResolver::new()))
        .prologue(&deskkin_protocol::PRELUDE)
        .map_err(|_| SessionFailure::Noise)?
        .local_private_key(&identity.payload[..32])
        .map_err(|_| SessionFailure::Noise)?
        .build_initiator()
        .map_err(|_| SessionFailure::Noise)?;
    let mut output = [0_u8; deskkin_protocol::HANDSHAKE_FRAME_MAX];
    let length = noise
        .write_message(&[], &mut output)
        .map_err(|_| SessionFailure::Noise)?;
    write_frame(descriptor, &output[..length], deadline, &mut control_frame)?;
    let mut input = [0_u8; deskkin_protocol::HANDSHAKE_FRAME_MAX];
    let incoming = read_frame(descriptor, &mut input, deadline, &mut control_frame)?;
    noise
        .read_message(incoming, &mut output)
        .map_err(|_| SessionFailure::Noise)?;
    let length = noise
        .write_message(&[], &mut output)
        .map_err(|_| SessionFailure::Noise)?;
    write_frame(descriptor, &output[..length], deadline, &mut control_frame)?;
    Ok(noise)
}

fn random_context() -> Result<[u8; 16], SessionFailure> {
    let mut output = [0_u8; 16];
    (unsafe { deskkin_csrand(output.as_mut_ptr(), output.len()) } == 0)
        .then_some(output)
        .ok_or(SessionFailure::Noise)
}

fn store_context(target: &[AtomicU32; 4], value: [u8; 16]) {
    for (word, bytes) in target.iter().zip(value.chunks_exact(4)) {
        word.store(
            u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            Ordering::Release,
        );
    }
}

fn load_context(source: &[AtomicU32; 4], output: &mut [u8]) {
    for (bytes, word) in output.chunks_exact_mut(4).zip(source) {
        bytes.copy_from_slice(&word.load(Ordering::Acquire).to_be_bytes());
    }
}

fn publish_peer(
    identity: &mut StoredIdentity,
    state: deskkin_core_s3::PeerState,
) -> Result<(), SessionFailure> {
    publish_record(
        (IDENTITY_A, IDENTITY_B, IDENTITY_INTENT),
        identity.sequence,
        identity.generation,
        deskkin_core_s3::RecordState::Identity(state),
        &identity.payload[..identity.payload_length],
    )
    .map_err(|_| SessionFailure::Store)?;
    identity.sequence += 1;
    identity.state = state;
    store_generation(identity.generation);
    store_identity_snapshot(identity);
    Ok(())
}

fn store_generation(generation: u64) {
    OWNER_GENERATION_HIGH.store((generation >> 32) as u32, Ordering::Release);
    OWNER_GENERATION_LOW.store(generation as u32, Ordering::Release);
}

fn load_generation() -> u64 {
    loop {
        let high = OWNER_GENERATION_HIGH.load(Ordering::Acquire);
        let low = OWNER_GENERATION_LOW.load(Ordering::Acquire);
        if high == OWNER_GENERATION_HIGH.load(Ordering::Acquire) {
            return (u64::from(high) << 32) | u64::from(low);
        }
    }
}

fn store_identity_snapshot(identity: &StoredIdentity) {
    PEER_STATE.store(identity.state as u8, Ordering::Release);
    if let Some(peer) = identity.payload.get(64..96) {
        for (word, bytes) in PEER_ID.iter().zip(peer.chunks_exact(4)) {
            word.store(
                u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
                Ordering::Release,
            );
        }
    } else {
        for word in &PEER_ID {
            word.store(0, Ordering::Release);
        }
    }
}

fn wait_pairing_decision(sas: [u8; 6]) -> Result<bool, SessionFailure> {
    let mut control_frame = [0_u8; deskkin_core_s3::CONTROL_PAYLOAD_MAX + 28];
    let numeric = sas.iter().fold(0_u32, |value, digit| {
        value * 10 + u32::from(digit.saturating_sub(b'0'))
    });
    UI_SAS.store(numeric, Ordering::Release);
    UI_ACTION.store(0, Ordering::Release);
    for _ in 0..6_000 {
        check_control(&mut control_frame)?;
        match UI_ACTION.swap(0, Ordering::AcqRel) {
            2 => {
                UI_SAS.store(u32::MAX, Ordering::Release);
                return Ok(true);
            }
            3 => {
                UI_SAS.store(u32::MAX, Ordering::Release);
                return Ok(false);
            }
            _ => unsafe { deskkin_sleep_ms(10) },
        }
    }
    UI_SAS.store(u32::MAX, Ordering::Release);
    Err(SessionFailure::Cancelled)
}

fn hello(
    descriptor: c_int,
    transport: &mut snow::TransportState,
) -> Result<[u8; 16], SessionFailure> {
    let deadline = deadline_after(5_000);
    let mut control_frame = [0_u8; deskkin_core_s3::CONTROL_PAYLOAD_MAX + 28];
    let session = random_context()?;
    store_context(&SESSION_CONTEXT, session);
    write_message(
        descriptor,
        transport,
        &deskkin_protocol::Message::Hello {
            session,
            protocol_majors: deskkin_protocol::PROTOCOL_MAJOR_1,
            required_features: deskkin_protocol::AVAILABILITY_READ_V1,
            optional_features: deskkin_protocol::Bits([0; 8]),
            requested_permissions: deskkin_protocol::AVAILABILITY_READ_PERMISSION,
        },
        deadline,
        &mut control_frame,
    )?;
    match read_message(descriptor, transport, deadline, &mut control_frame)? {
        deskkin_protocol::Message::HelloAck {
            session: received,
            selected_major: 1,
            selected_features,
            granted_permissions,
        } if received == session
            && selected_features == deskkin_protocol::AVAILABILITY_READ_V1
            && granted_permissions == deskkin_protocol::AVAILABILITY_READ_PERMISSION =>
        {
            Ok(session)
        }
        deskkin_protocol::Message::HelloReject {
            session: received,
            reason,
        } if received == session => Err(match reason {
            deskkin_protocol::HelloRejectReason::NoCommonVersion
            | deskkin_protocol::HelloRejectReason::RequiredFeatureUnsupported => {
                SessionFailure::Incompatible
            }
            deskkin_protocol::HelloRejectReason::PermissionDenied => {
                SessionFailure::AuthorizationDenied
            }
            deskkin_protocol::HelloRejectReason::SessionBusy => SessionFailure::SessionBusy,
        }),
        _ => Err(SessionFailure::Protocol),
    }
}

fn availability_loop(
    descriptor: c_int,
    transport: &mut snow::TransportState,
    session: [u8; 16],
) -> Result<(), SessionFailure> {
    if unsafe { deskkin_tcp_set_timeout(descriptor, 10) } != 0 {
        return Err(SessionFailure::Tcp);
    }
    let mut application = deskkin_application::Application::new();
    let mut read_effect = application
        .transition(deskkin_application::ApplicationInput::Lifecycle(
            deskkin_application::Lifecycle::Start,
        ))
        .map_err(|_| SessionFailure::Protocol)?
        .effects
        .get(0)
        .ok_or(SessionFailure::Protocol)?;
    let mut adapter = deskkin_protocol_client::ProtocolAdapter::new();
    adapter.authenticated(session);
    let mut control_frame = [0_u8; deskkin_core_s3::CONTROL_PAYLOAD_MAX + 28];
    let result = (|| -> Result<(), SessionFailure> {
        loop {
            if let Some(active) = poll_active_control(&mut control_frame) {
                return Err(SessionFailure::Control(active));
            }
            let operation = random_context()?;
            LAST_STAGE.store(6, Ordering::Release);
            store_context(&OPERATION_CONTEXT, operation);
            let request_id = adapter
                .begin_read(read_effect.id.local.get(), operation)
                .map_err(|_| SessionFailure::Protocol)?;
            let deadline = deadline_after(2_000);
            write_message(
                descriptor,
                transport,
                &deskkin_protocol::Message::ReadAvailability {
                    request_id,
                    operation,
                },
                deadline,
                &mut control_frame,
            )?;
            loop {
                match read_message(descriptor, transport, deadline, &mut control_frame)? {
                    deskkin_protocol::Message::AvailabilityResult {
                        request_id: received_id,
                        operation: received_operation,
                        result,
                    } => {
                        if received_id != request_id || received_operation != operation {
                            return Err(SessionFailure::Protocol);
                        }
                        let event = adapter
                            .result(session, received_id, received_operation, result)
                            .map_err(|_| SessionFailure::Protocol)?;
                        let deskkin_protocol_client::ProtocolEvent::AvailabilityCompleted {
                            effect_id,
                            value,
                        } = event
                        else {
                            return Err(SessionFailure::Protocol);
                        };
                        let effect_id = deskkin_application::LocalEffectId::new(effect_id)
                            .ok_or(SessionFailure::Protocol)?;
                        let result = match value {
                            deskkin_protocol_client::AvailabilityValue::Available => {
                                Ok(deskkin_application::availability::Availability::Available)
                            }
                            deskkin_protocol_client::AvailabilityValue::Unavailable => {
                                Ok(deskkin_application::availability::Availability::Unavailable)
                            }
                            deskkin_protocol_client::AvailabilityValue::ReadFailed => {
                                Err(deskkin_application::availability::ReadError::Unavailable)
                            }
                        };
                        let transition = application
                            .transition(deskkin_application::ApplicationInput::availability(
                                read_effect.id,
                                deskkin_application::availability::Input::ReadCompleted(
                                    deskkin_application::availability::ReadCompleted {
                                        effect_id,
                                        result,
                                    },
                                ),
                            ))
                            .map_err(|_| SessionFailure::Protocol)?;
                        let timer = transition.effects.get(0).ok_or(SessionFailure::Protocol)?;
                        VALID_RESULT.store(1, Ordering::Release);
                        RESULT_ATTEMPT
                            .store(RUN_ATTEMPT.load(Ordering::Acquire), Ordering::Release);
                        UI_VIEW.store(
                            match transition.view {
                                deskkin_application::ApplicationView::Availability(
                                    deskkin_application::availability::Surface::Unknown,
                                )
                                | deskkin_application::ApplicationView::Empty
                                | deskkin_application::ApplicationView::SyntheticNotice(_) => 0,
                                deskkin_application::ApplicationView::Availability(
                                    deskkin_application::availability::Surface::Available,
                                ) => 1,
                                deskkin_application::ApplicationView::Availability(
                                    deskkin_application::availability::Surface::Unavailable,
                                ) => 2,
                            },
                            Ordering::Release,
                        );
                        LAST_STAGE.store(7, Ordering::Release);
                        LAST_ERROR.store(0, Ordering::Release);
                        application
                            .transition(deskkin_application::ApplicationInput::availability(
                                timer.id,
                                deskkin_application::availability::Input::TimerArmCompleted(
                                    deskkin_application::availability::TimerArmCompleted {
                                        effect_id: timer.id.local,
                                        result: Ok(()),
                                    },
                                ),
                            ))
                            .map_err(|_| SessionFailure::Protocol)?;
                        for _ in 0..500 {
                            if let Some(active) = poll_active_control(&mut control_frame) {
                                return Err(SessionFailure::Control(active));
                            }
                            let action = UI_ACTION.load(Ordering::Acquire);
                            if action == 5 || action == 6 {
                                let _ = write_message(
                                    descriptor,
                                    transport,
                                    &deskkin_protocol::Message::Close {
                                        reason: if action == 6 {
                                            deskkin_protocol::CloseReason::Unpaired
                                        } else {
                                            deskkin_protocol::CloseReason::Normal
                                        },
                                    },
                                    deadline_after(2_000),
                                    &mut control_frame,
                                );
                                return Ok(());
                            }
                            unsafe { deskkin_sleep_ms(10) };
                        }
                        read_effect = application
                            .transition(deskkin_application::ApplicationInput::availability(
                                timer.id,
                                deskkin_application::availability::Input::RefreshDue(
                                    deskkin_application::availability::RefreshDue {
                                        effect_id: timer.id.local,
                                    },
                                ),
                            ))
                            .map_err(|_| SessionFailure::Protocol)?
                            .effects
                            .get(0)
                            .ok_or(SessionFailure::Protocol)?;
                        break;
                    }
                    deskkin_protocol::Message::Ping => {
                        write_message(
                            descriptor,
                            transport,
                            &deskkin_protocol::Message::Pong,
                            deadline,
                            &mut control_frame,
                        )?;
                    }
                    _ => return Err(SessionFailure::Protocol),
                }
            }
        }
    })();
    if result.is_err() {
        if let Some(deskkin_protocol_client::ProtocolEvent::SessionInvalidated) =
            adapter.disconnected()
        {
            let _ = application.transition(deskkin_application::ApplicationInput::Lifecycle(
                deskkin_application::Lifecycle::SessionInvalidated,
            ));
        }
        UI_VIEW.store(0, Ordering::Release);
    }
    result
}

fn pair_session(
    descriptor: c_int,
    mut identity: StoredIdentity,
    noise: snow::HandshakeState,
) -> Result<(), SessionFailure> {
    let mut deadline = deadline_after(5_000);
    let mut control_frame = [0_u8; deskkin_core_s3::CONTROL_PAYLOAD_MAX + 28];
    let sas = deskkin_protocol::authentication_string(noise.get_handshake_hash())
        .map_err(|_| SessionFailure::Protocol)?;
    let remote = noise.get_remote_static().ok_or(SessionFailure::Noise)?;
    if remote.len() != 32 {
        return Err(SessionFailure::Noise);
    }
    identity.payload[64..96].copy_from_slice(remote);
    let transaction = random_context()?;
    identity.payload[96..112].copy_from_slice(&transaction);
    identity.payload_length = 112;
    let mut transport = noise
        .into_transport_mode()
        .map_err(|_| SessionFailure::Noise)?;
    write_message(
        descriptor,
        &mut transport,
        &deskkin_protocol::Message::PairingBegin { transaction },
        deadline,
        &mut control_frame,
    )?;
    let local = wait_pairing_decision(sas)?;
    deadline = deadline_after(5_000);
    let remote = matches!(
        read_message(descriptor, &mut transport, deadline, &mut control_frame)?,
        deskkin_protocol::Message::PairingDecision {
            transaction: received,
            decision: deskkin_protocol::PairingDecision::Confirmed,
        } if received == transaction
    );
    deadline = deadline_after(5_000);
    write_message(
        descriptor,
        &mut transport,
        &deskkin_protocol::Message::PairingDecision {
            transaction,
            decision: if local {
                deskkin_protocol::PairingDecision::Confirmed
            } else {
                deskkin_protocol::PairingDecision::Rejected
            },
        },
        deadline,
        &mut control_frame,
    )?;
    if !local || !remote {
        return Err(SessionFailure::Rejected);
    }
    publish_peer(&mut identity, deskkin_core_s3::PeerState::Pending)?;
    deadline = deadline_after(5_000);
    write_message(
        descriptor,
        &mut transport,
        &deskkin_protocol::Message::PairingPrepared { transaction },
        deadline,
        &mut control_frame,
    )?;
    deadline = deadline_after(5_000);
    if !matches!(read_message(descriptor, &mut transport, deadline, &mut control_frame)?, deskkin_protocol::Message::PairingPrepared { transaction: received } if received == transaction)
    {
        return Err(SessionFailure::Protocol);
    }
    deadline = deadline_after(5_000);
    write_message(
        descriptor,
        &mut transport,
        &deskkin_protocol::Message::PairingCommit { transaction },
        deadline,
        &mut control_frame,
    )?;
    deadline = deadline_after(5_000);
    if !matches!(read_message(descriptor, &mut transport, deadline, &mut control_frame)?, deskkin_protocol::Message::PairingCommitted { transaction: received } if received == transaction)
    {
        return Err(SessionFailure::Protocol);
    }
    publish_peer(&mut identity, deskkin_core_s3::PeerState::Committing)?;
    deadline = deadline_after(5_000);
    write_message(
        descriptor,
        &mut transport,
        &deskkin_protocol::Message::PairingCommitted { transaction },
        deadline,
        &mut control_frame,
    )?;
    deadline = deadline_after(5_000);
    if !matches!(read_message(descriptor, &mut transport, deadline, &mut control_frame)?, deskkin_protocol::Message::PairingComplete { transaction: received } if received == transaction)
    {
        return Err(SessionFailure::Protocol);
    }
    publish_peer(&mut identity, deskkin_core_s3::PeerState::Paired)?;
    let session = hello(descriptor, &mut transport)?;
    UI_SHELL.store(4, Ordering::Release);
    availability_loop(descriptor, &mut transport, session)
}

fn pinned_session(
    descriptor: c_int,
    identity: &StoredIdentity,
    noise: snow::HandshakeState,
) -> Result<(), SessionFailure> {
    let remote = noise.get_remote_static().ok_or(SessionFailure::Noise)?;
    if identity.payload_length != 112 || remote != &identity.payload[64..96] {
        return Err(SessionFailure::Noise);
    }
    let mut transport = noise
        .into_transport_mode()
        .map_err(|_| SessionFailure::Noise)?;
    let session = hello(descriptor, &mut transport)?;
    UI_SHELL.store(4, Ordering::Release);
    availability_loop(descriptor, &mut transport, session)
}

fn connect_once(pair_requested: bool) -> Result<(), SessionFailure> {
    UI_VIEW.store(0, Ordering::Release);
    VALID_RESULT.store(0, Ordering::Release);
    let fallback = match (load_identity(), load_config()) {
        (Ok(identity), Ok(config)) => {
            let fallback = if identity.state == deskkin_core_s3::PeerState::Unpaired {
                1
            } else {
                2
            };
            (identity, config, fallback)
        }
        _ => {
            UI_SHELL.store(0, Ordering::Release);
            return Err(SessionFailure::Store);
        }
    };
    let (identity, mut config, fallback_shell) = fallback;
    UI_SHELL.store(2, Ordering::Release);
    LAST_STAGE.store(1, Ordering::Release);
    LAST_ERROR.store(0, Ordering::Release);
    if unsafe {
        deskkin_wifi_associate(
            config.ssid.as_ptr(),
            config.ssid_length,
            config.passphrase.as_ptr(),
            config.passphrase_length,
        )
    } != 0
    {
        config.passphrase.zeroize();
        UI_SHELL.store(fallback_shell, Ordering::Release);
        return Err(SessionFailure::Wifi);
    }
    LAST_STAGE.store(2, Ordering::Release);
    if unsafe { deskkin_wait_dhcp() } != 0 {
        config.passphrase.zeroize();
        UI_SHELL.store(fallback_shell, Ordering::Release);
        return Err(SessionFailure::Dhcp);
    }
    config.passphrase.zeroize();
    LAST_STAGE.store(3, Ordering::Release);
    let descriptor = unsafe { deskkin_tcp_connect(config.host.as_ptr(), 39_042) };
    if descriptor < 0 {
        UI_SHELL.store(fallback_shell, Ordering::Release);
        return Err(SessionFailure::Tcp);
    }
    let result = (|| {
        LAST_STAGE.store(4, Ordering::Release);
        let noise = noise_connect(descriptor, &identity)?;
        LAST_STAGE.store(5, Ordering::Release);
        match identity.state {
            deskkin_core_s3::PeerState::Unpaired if pair_requested => {
                pair_session(descriptor, identity, noise)
            }
            deskkin_core_s3::PeerState::Paired => pinned_session(descriptor, &identity, noise),
            _ => Err(SessionFailure::Store),
        }
    })();
    let _ = unsafe { deskkin_tcp_close(descriptor) };
    let result = match result {
        Err(SessionFailure::Control(active)) => {
            finish_active_control(active);
            refresh_setup_shell();
            return Ok(());
        }
        other => other,
    };
    if result.is_err() {
        UI_VIEW.store(0, Ordering::Release);
        refresh_setup_shell();
    }
    result
}

#[no_mangle]
extern "C" fn deskkin_rust_set_boot_status(stage: u8, error: u8) {
    BOOT_STAGE.store(stage, Ordering::Release);
    BOOT_ERROR.store(error, Ordering::Release);
}

#[no_mangle]
unsafe extern "C" fn deskkin_rust_control_snapshot(
    input: *const u8,
    input_length: usize,
    output: *mut u8,
) -> usize {
    if input.is_null() || output.is_null() || input_length > 188 {
        return 0;
    }
    let input = unsafe { core::slice::from_raw_parts(input, input_length) };
    let Ok(control) = deskkin_core_s3::decode_control(input) else {
        return 0;
    };
    if !matches!(
        control.command,
        deskkin_core_s3::ControlCommand::Status
            | deskkin_core_s3::ControlCommand::IdentityList
            | deskkin_core_s3::ControlCommand::WifiStatus
            | deskkin_core_s3::ControlCommand::PetBenchmarkStatus
    ) || !control.payload.is_empty()
    {
        return 0;
    }
    let output = unsafe { core::slice::from_raw_parts_mut(output, 80) };
    output.fill(0);
    output[0] = 1;
    output[1] = ServiceStatus::Success as u8;
    output[2..18].copy_from_slice(&control.command_id);
    output[18..26].copy_from_slice(&load_generation().to_be_bytes());
    if control.command == deskkin_core_s3::ControlCommand::IdentityList {
        output[26] = PEER_STATE.load(Ordering::Acquire);
        if output[26] == deskkin_core_s3::PeerState::Unpaired as u8 {
            return 27;
        }
        for (bytes, word) in output[27..59].chunks_exact_mut(4).zip(&PEER_ID) {
            bytes.copy_from_slice(&word.load(Ordering::Acquire).to_be_bytes());
        }
        return 59;
    }
    if control.command == deskkin_core_s3::ControlCommand::WifiStatus {
        output[26] = CONFIG_PRESENT.load(Ordering::Acquire);
        return 27;
    }
    if control.command == deskkin_core_s3::ControlCommand::PetBenchmarkStatus {
        encode_pet_benchmark(output);
        return 80;
    }
    output[26] =
        UI_SHELL.load(Ordering::Acquire) | (VALID_RESULT.load(Ordering::Acquire).min(1) << 7);
    output[27] = UI_VIEW.load(Ordering::Acquire);
    output[28..32].copy_from_slice(&UI_FRAME_DIGEST.load(Ordering::Acquire).to_be_bytes());
    load_context(&SESSION_CONTEXT, &mut output[32..48]);
    load_context(&OPERATION_CONTEXT, &mut output[48..64]);
    output[64..68].copy_from_slice(&RUN_ATTEMPT.load(Ordering::Acquire).to_be_bytes());
    output[68..72].copy_from_slice(&RESULT_ATTEMPT.load(Ordering::Acquire).to_be_bytes());
    output[72..76].copy_from_slice(&FRAME_ATTEMPT.load(Ordering::Acquire).to_be_bytes());
    output[76] = LAST_STAGE.load(Ordering::Acquire);
    output[77] = LAST_ERROR.load(Ordering::Acquire);
    output[78] = BOOT_STAGE.load(Ordering::Acquire);
    output[79] = BOOT_ERROR.load(Ordering::Acquire);
    80
}

fn handle_control(frame: deskkin_core_s3::ControlFrame<'_>) -> ServiceStatus {
    use deskkin_core_s3::ControlCommand;
    let generation = load_identity().map_or(0, |identity| identity.generation);
    let mutation = matches!(
        frame.command,
        ControlCommand::IdentityInit
            | ControlCommand::IdentityUnpair
            | ControlCommand::WifiProvision
            | ControlCommand::WifiClear
    );
    if mutation && frame.owner_generation != generation {
        return ServiceStatus::Invalid;
    }
    match frame.command {
        ControlCommand::IdentityInit if frame.payload.is_empty() => identity_init(),
        ControlCommand::IdentityList
        | ControlCommand::WifiStatus
        | ControlCommand::Status
        | ControlCommand::PetBenchmarkStatus => ServiceStatus::Success,
        ControlCommand::WifiProvision => wifi_provision(frame.payload),
        ControlCommand::WifiClear if frame.payload.is_empty() => clear_config(),
        ControlCommand::Run if frame.payload.is_empty() => {
            if PET_BENCHMARK_STATE.load(Ordering::Acquire) == PetBenchmarkState::Active as u8 {
                return ServiceStatus::Busy;
            }
            PET_BENCHMARK_STATE.store(PetBenchmarkState::Idle as u8, Ordering::Release);
            APPLICATION_RUNNING.store(1, Ordering::Release);
            let attempt = RUN_ATTEMPT.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
            VALID_RESULT.store(0, Ordering::Release);
            RESULT_ATTEMPT.store(0, Ordering::Release);
            FRAME_ATTEMPT.store(0, Ordering::Release);
            if attempt == 0 {
                return ServiceStatus::Invalid;
            }
            UI_ACTION.store(4, Ordering::Release);
            ServiceStatus::Success
        }
        ControlCommand::Shutdown if frame.payload.is_empty() => {
            APPLICATION_RUNNING.store(0, Ordering::Release);
            UI_ACTION.store(5, Ordering::Release);
            UI_VIEW.store(0, Ordering::Release);
            ServiceStatus::Success
        }
        ControlCommand::PetBenchmarkStart if frame.payload.is_empty() => {
            if APPLICATION_RUNNING.load(Ordering::Acquire) != 0
                || PET_BENCHMARK_STATE.load(Ordering::Acquire) == PetBenchmarkState::Active as u8
            {
                return ServiceStatus::Busy;
            }
            reset_pet_benchmark();
            ServiceStatus::Success
        }
        ControlCommand::IdentityUnpair => identity_unpair(frame.payload),
        _ => ServiceStatus::Invalid,
    }
}

fn refresh_setup_shell() {
    let shell = match (load_identity(), load_config()) {
        (Ok(identity), Ok(_)) if identity.state == deskkin_core_s3::PeerState::Unpaired => 1,
        (Ok(identity), Ok(_)) if identity.state == deskkin_core_s3::PeerState::Paired => 2,
        _ => 0,
    };
    UI_SHELL.store(shell, Ordering::Release);
}

fn publish_control_completion(
    control: Option<deskkin_core_s3::ControlFrame<'_>>,
    status: ServiceStatus,
) {
    let mut completion = zeroize::Zeroizing::new([0_u8; 80]);
    completion[0] = 1;
    completion[1] = status as u8;
    let mut completion_length = 2;
    if let Some(control) = control {
        completion[2..18].copy_from_slice(&control.command_id);
        let identity = load_identity().ok();
        let generation = identity.as_ref().map_or(0, |value| value.generation);
        completion[18..26].copy_from_slice(&generation.to_be_bytes());
        completion_length = 26;
        if control.command == deskkin_core_s3::ControlCommand::IdentityList {
            if let Some(identity) = identity.as_ref() {
                completion[26] = identity.state as u8;
                completion_length = 27;
                if identity.payload_length >= 96
                    && identity.state != deskkin_core_s3::PeerState::Unpaired
                {
                    completion[27..59].copy_from_slice(&identity.payload[64..96]);
                    completion_length = 59;
                }
            }
        } else if control.command == deskkin_core_s3::ControlCommand::Status {
            completion[26] = UI_SHELL.load(Ordering::Acquire)
                | (VALID_RESULT.load(Ordering::Acquire).min(1) << 7);
            completion[27] = UI_VIEW.load(Ordering::Acquire);
            completion[28..32]
                .copy_from_slice(&UI_FRAME_DIGEST.load(Ordering::Acquire).to_be_bytes());
            load_context(&SESSION_CONTEXT, &mut completion[32..48]);
            load_context(&OPERATION_CONTEXT, &mut completion[48..64]);
            completion[64..68].copy_from_slice(&RUN_ATTEMPT.load(Ordering::Acquire).to_be_bytes());
            completion[68..72]
                .copy_from_slice(&RESULT_ATTEMPT.load(Ordering::Acquire).to_be_bytes());
            completion[72..76]
                .copy_from_slice(&FRAME_ATTEMPT.load(Ordering::Acquire).to_be_bytes());
            completion[76] = LAST_STAGE.load(Ordering::Acquire);
            completion[77] = LAST_ERROR.load(Ordering::Acquire);
            completion[78] = BOOT_STAGE.load(Ordering::Acquire);
            completion[79] = BOOT_ERROR.load(Ordering::Acquire);
            completion_length = 80;
        } else if control.command == deskkin_core_s3::ControlCommand::Run {
            completion[26..30].copy_from_slice(&RUN_ATTEMPT.load(Ordering::Acquire).to_be_bytes());
            completion_length = 30;
        } else if control.command == deskkin_core_s3::ControlCommand::PetBenchmarkStatus {
            encode_pet_benchmark(&mut *completion);
            completion_length = 80;
        }
    }
    let _ = unsafe { deskkin_service_publish_completion(completion.as_ptr(), completion_length) };
}

fn publish_basic_completion(command_id: [u8; 16], status: ServiceStatus) {
    let mut completion = zeroize::Zeroizing::new([0_u8; 26]);
    completion[0] = 1;
    completion[1] = status as u8;
    completion[2..18].copy_from_slice(&command_id);
    completion[18..26].copy_from_slice(&load_generation().to_be_bytes());
    let _ = unsafe { deskkin_service_publish_completion(completion.as_ptr(), completion.len()) };
}

fn poll_active_control(frame: &mut [u8]) -> Option<ActiveControl> {
    let length = unsafe { deskkin_service_take_command(frame.as_mut_ptr(), frame.len()) };
    if length <= 0 {
        return None;
    }
    let decoded = deskkin_core_s3::decode_control(&frame[..length as usize]);
    let Some(control) = decoded.ok() else {
        publish_control_completion(None, ServiceStatus::Invalid);
        frame.zeroize();
        return None;
    };
    let result = match control.command {
        deskkin_core_s3::ControlCommand::Shutdown if control.payload.is_empty() => {
            UI_ACTION.store(5, Ordering::Release);
            Some(ActiveControl::Shutdown {
                command_id: control.command_id,
            })
        }
        deskkin_core_s3::ControlCommand::IdentityUnpair => {
            if control.owner_generation != load_generation() {
                publish_control_completion(Some(control), ServiceStatus::Invalid);
                None
            } else {
                match begin_identity_unpair(control.payload) {
                    Ok(revocation) => Some(ActiveControl::Unpair {
                        command_id: control.command_id,
                        revocation,
                    }),
                    Err(status) => {
                        publish_control_completion(Some(control), status);
                        None
                    }
                }
            }
        }
        _ => {
            publish_control_completion(Some(control), ServiceStatus::Busy);
            frame.zeroize();
            return None;
        }
    };
    frame.zeroize();
    result
}

fn finish_active_control(active: ActiveControl) {
    match active {
        ActiveControl::Shutdown { command_id } => {
            publish_basic_completion(command_id, ServiceStatus::Success);
        }
        ActiveControl::Unpair {
            command_id,
            revocation,
        } => {
            let status = finish_identity_unpair(&revocation);
            if matches!(status, ServiceStatus::Success) {
                UI_SHELL.store(1, Ordering::Release);
            }
            publish_basic_completion(command_id, status);
        }
    }
}

#[no_mangle]
extern "C" fn rust_main() {
    if prove_noise_resolver().is_err() {
        fail_boot(BootError::NoiseResolver);
        loop {
            unsafe { deskkin_sleep_ms(1_000) };
        }
    }
    set_boot_stage(BootStage::NoiseResolverReady);
    if unsafe { deskkin_start_service_worker() } != 0 {
        fail_boot(BootError::ServiceWorker);
        loop {
            unsafe { deskkin_sleep_ms(1_000) };
        }
    }
    set_boot_stage(BootStage::ServiceWorkerReady);
    let executor = EXECUTOR.init(Executor::new());
    executor.run(|spawner| {
        if spawner.spawn(run_ui()).is_err() {
            fail_boot(BootError::UiPlatform);
        }
    });
}

#[no_mangle]
extern "C" fn deskkin_rust_service_worker() {
    let mut frame = [0_u8; deskkin_core_s3::CONTROL_PAYLOAD_MAX + 28];
    let mut running = true;
    let mut next_attempt_ms = 0_u64;
    let mut connection = deskkin_protocol_client::ProtocolAdapter::new();
    if let Ok(identity) = load_identity() {
        store_generation(identity.generation);
        store_identity_snapshot(&identity);
    }
    refresh_setup_shell();
    CONFIG_PRESENT.store(u8::from(load_config().is_ok()), Ordering::Release);
    loop {
        let length = unsafe { deskkin_service_take_command(frame.as_mut_ptr(), frame.len()) };
        if length > 0 {
            let decoded = deskkin_core_s3::decode_control(&frame[..length as usize]);
            let status = decoded.map_or(ServiceStatus::Invalid, handle_control);
            publish_control_completion(decoded.ok(), status);
            frame.zeroize();
            if UI_SHELL.load(Ordering::Acquire) < 3 {
                refresh_setup_shell();
            }
        }
        let action = UI_ACTION.swap(0, Ordering::AcqRel);
        if action == 5 {
            running = false;
            APPLICATION_RUNNING.store(0, Ordering::Release);
            connection.stop();
        } else if action == 4 {
            running = true;
            APPLICATION_RUNNING.store(1, Ordering::Release);
            next_attempt_ms = 0;
            connection.restart_after_pairing();
        }
        let pair_requested = action == 1;
        if pair_requested {
            connection.restart_after_pairing();
            next_attempt_ms = 0;
        }
        let now = Instant::now().as_millis();
        let paired = load_identity()
            .is_ok_and(|identity| identity.state == deskkin_core_s3::PeerState::Paired);
        if connection.state() != deskkin_protocol_client::ConnectionState::Stopped
            && PET_BENCHMARK_STATE.load(Ordering::Acquire) == PetBenchmarkState::Idle as u8
            && (pair_requested || running && paired)
            && now >= next_attempt_ms
        {
            connection.connecting();
            match connect_once(pair_requested) {
                Ok(()) => {
                    if VALID_RESULT.swap(0, Ordering::AcqRel) == 1 {
                        connection.authenticated([0; 16]);
                        connection.valid_availability_result();
                    }
                    let delay = connection.connection_failed().unwrap_or(5_000);
                    next_attempt_ms = Instant::now().as_millis().saturating_add(u64::from(delay));
                }
                Err(SessionFailure::Incompatible) => {
                    LAST_ERROR.store(10, Ordering::Release);
                    UI_SHELL.store(4, Ordering::Release);
                    connection.hello_rejected(deskkin_protocol::HelloRejectReason::NoCommonVersion)
                }
                Err(SessionFailure::AuthorizationDenied) => {
                    LAST_ERROR.store(11, Ordering::Release);
                    UI_SHELL.store(4, Ordering::Release);
                    connection.hello_rejected(deskkin_protocol::HelloRejectReason::PermissionDenied)
                }
                Err(SessionFailure::SessionBusy) => {
                    LAST_ERROR.store(8, Ordering::Release);
                    let delay = connection.connection_failed().unwrap_or(5_000);
                    next_attempt_ms = Instant::now().as_millis().saturating_add(u64::from(delay));
                }
                Err(error) => {
                    let error_code = match error {
                        SessionFailure::Store => 1,
                        SessionFailure::Wifi => 2,
                        SessionFailure::Dhcp => 3,
                        SessionFailure::Tcp => 4,
                        SessionFailure::Noise => 5,
                        SessionFailure::Protocol => 6,
                        SessionFailure::Rejected => 7,
                        SessionFailure::Cancelled => 9,
                        SessionFailure::Control(_) => 0,
                        SessionFailure::Incompatible
                        | SessionFailure::AuthorizationDenied
                        | SessionFailure::SessionBusy => 6,
                    };
                    LAST_ERROR.store(error_code, Ordering::Release);
                    let delay = connection.connection_failed().unwrap_or(5_000);
                    next_attempt_ms = Instant::now().as_millis().saturating_add(u64::from(delay));
                }
            }
        }
    }
}

#[embassy_executor::task]
async fn run_ui() {
    let state = Rc::new(RefCell::new(None));
    if slint::platform::set_platform(Box::new(DevicePlatform {
        window: state.clone(),
    }))
    .is_err()
    {
        fail_boot(BootError::UiPlatform);
        return;
    }
    set_boot_stage(BootStage::UiPlatformReady);
    let Ok(component) = DeviceWindow::new() else {
        fail_boot(BootError::UiComponent);
        return;
    };
    let Some(window) = state.borrow().clone() else {
        fail_boot(BootError::UiComponent);
        return;
    };
    window.set_size(PhysicalSize::new(WIDTH as u32, HEIGHT as u32));
    component.set_shell_state("SetupRequired".into());
    component.set_status_text("Unknown".into());
    component.on_pair(|| UI_ACTION.store(1, Ordering::Release));
    component.on_confirm(|| UI_ACTION.store(2, Ordering::Release));
    component.on_cancel(|| UI_ACTION.store(3, Ordering::Release));
    if component.show().is_err() {
        fail_boot(BootError::UiComponent);
        return;
    }
    set_boot_stage(BootStage::UiComponentReady);
    let Some(framebuffer) = Framebuffer::new() else {
        fail_boot(BootError::Framebuffer);
        return;
    };
    set_boot_stage(BootStage::FramebufferReady);
    let mut first_frame = true;
    let mut pet_animator = PetAnimator::new();
    let mut pet_updated_at_ms = Instant::now().as_millis();
    let mut benchmark_animator = PetAnimator::new();
    let mut benchmark_summary = deskkin_core_s3::PetBenchmarkSummary::default();
    let mut benchmark_started_at_us = 0_u64;
    let mut benchmark_next_deadline_us = 0_u64;
    let mut benchmark_frame_deadline_us = 0_u64;
    let mut benchmark_scheduled_frames = 0_u32;
    let mut benchmark_frame_pending = false;
    let mut benchmark_previous_digest = 0_u32;
    let mut benchmark_was_active = false;
    loop {
        slint::platform::update_timers_and_animations();
        let benchmark_active =
            PET_BENCHMARK_STATE.load(Ordering::Acquire) == PetBenchmarkState::Active as u8;
        component.set_pet_benchmark_active(benchmark_active);
        if benchmark_active {
            let now_us = Instant::now().as_micros();
            if !benchmark_was_active {
                benchmark_summary = deskkin_core_s3::PetBenchmarkSummary::default();
                benchmark_summary.allocation_failures = unsafe { deskkin_allocation_failures() };
                benchmark_started_at_us = now_us;
                benchmark_next_deadline_us = now_us;
                benchmark_scheduled_frames = 0;
                benchmark_frame_pending = false;
                benchmark_previous_digest = UI_FRAME_DIGEST.load(Ordering::Acquire);
                let frame = benchmark_animator
                    .set_state(deskkin_presentation::PetAnimationState::MoveRight);
                component.set_pet_animation_row(i32::from(frame.row));
                component.set_pet_frame_index(i32::from(frame.column));
            }
            if now_us >= benchmark_next_deadline_us
                && benchmark_scheduled_frames < deskkin_core_s3::PET_BENCHMARK_REQUESTS
            {
                let remaining = deskkin_core_s3::PET_BENCHMARK_REQUESTS
                    .saturating_sub(benchmark_scheduled_frames);
                let due = ((now_us.saturating_sub(benchmark_next_deadline_us)
                    / deskkin_core_s3::PET_BENCHMARK_FRAME_PERIOD_US)
                    .saturating_add(1))
                .min(u64::from(remaining));
                let due = u32::try_from(due).unwrap_or(remaining);
                benchmark_summary.request_updates(due);
                let elapsed_frames = if benchmark_scheduled_frames == 0 {
                    due.saturating_sub(1)
                } else {
                    due
                };
                let frame = benchmark_animator.advance(elapsed_frames.saturating_mul(50));
                component.set_pet_animation_row(i32::from(frame.row));
                component.set_pet_frame_index(i32::from(frame.column));
                benchmark_frame_deadline_us = benchmark_next_deadline_us.saturating_add(
                    u64::from(due.saturating_sub(1))
                        .saturating_mul(deskkin_core_s3::PET_BENCHMARK_FRAME_PERIOD_US),
                );
                benchmark_next_deadline_us = benchmark_next_deadline_us.saturating_add(
                    u64::from(due).saturating_mul(deskkin_core_s3::PET_BENCHMARK_FRAME_PERIOD_US),
                );
                benchmark_scheduled_frames = benchmark_scheduled_frames.saturating_add(due);
                benchmark_frame_pending = true;
            }
        } else {
            let now_ms = Instant::now().as_millis();
            let elapsed_ms =
                u32::try_from(now_ms.saturating_sub(pet_updated_at_ms)).unwrap_or(u32::MAX);
            pet_updated_at_ms = now_ms;
            let pet_frame = pet_animator.advance(elapsed_ms);
            component.set_pet_animation_row(i32::from(pet_frame.row));
            component.set_pet_frame_index(i32::from(pet_frame.column));
        }
        benchmark_was_active = benchmark_active;
        let sas = UI_SAS.load(Ordering::Acquire);
        if sas == u32::MAX {
            component.set_authentication_string("".into());
        } else {
            let mut digits = [b'0'; 6];
            let mut value = sas;
            for index in (0..6).rev() {
                digits[index] = b'0' + (value % 10) as u8;
                value /= 10;
            }
            let text = core::str::from_utf8(&digits).expect("digits are UTF-8");
            component.set_authentication_string(text.into());
            UI_SHELL.store(3, Ordering::Release);
        }
        component.set_shell_state(
            match UI_SHELL.load(Ordering::Acquire) {
                1 => "ReadyToPair",
                2 => "Connecting",
                3 => "PairingConfirmation",
                4 => "Paired",
                _ => "SetupRequired",
            }
            .into(),
        );
        match UI_VIEW.load(Ordering::Acquire) {
            1 => {
                component.set_shell_state("Paired".into());
                component.set_status_text("Available".into());
                component.set_status_color(slint::Color::from_rgb_u8(0x36, 0xc9, 0x82));
            }
            2 => {
                component.set_shell_state("Paired".into());
                component.set_status_text("Unavailable".into());
                component.set_status_color(slint::Color::from_rgb_u8(0xf0, 0x5d, 0x5e));
            }
            _ => {
                component.set_status_text("Unknown".into());
                component.set_status_color(slint::Color::from_rgb_u8(0xf3, 0xb3, 0x3d));
            }
        }
        let mut x = 0;
        let mut y = 0;
        if unsafe { deskkin_take_touch(&mut x, &mut y) } {
            let position = LogicalPosition::new(x as f32, y as f32);
            window.dispatch_event(WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            });
            window.dispatch_event(WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Left,
            });
        }
        let mut ranges = [deskkin_core_s3::DirtyRange::EMPTY; HEIGHT];
        let render_started_us = Instant::now().as_micros();
        let rendered = window.draw_if_needed(|renderer| {
            renderer.render_by_line(&mut Capture {
                line: [Rgb565Pixel(0); WIDTH],
                ranges: &mut ranges,
                framebuffer: &framebuffer,
            });
        });
        let render_ended_us = Instant::now().as_micros();
        let changed = ranges.iter().any(|range| range.start != range.end);
        let (dirty_lines, transferred_bytes) = dirty_measurement(&ranges);
        let transfer_started_us = Instant::now().as_micros();
        if transfer_dirty(&framebuffer, &ranges).is_err() {
            if benchmark_active {
                benchmark_summary.record_transfer_failure();
                benchmark_summary.duration_ms =
                    elapsed_us(benchmark_started_at_us, Instant::now().as_micros()) / 1_000;
                publish_pet_benchmark(&benchmark_summary, PetBenchmarkState::Failed);
            }
            fail_boot(BootError::DisplayTransfer);
            return;
        }
        let transfer_ended_us = Instant::now().as_micros();
        if changed {
            let digest = framebuffer.digest();
            UI_FRAME_DIGEST.store(digest, Ordering::Release);
            if benchmark_active && benchmark_frame_pending && rendered {
                benchmark_summary.complete_frame(
                    elapsed_us(render_started_us, render_ended_us),
                    elapsed_us(transfer_started_us, transfer_ended_us),
                    dirty_lines,
                    transferred_bytes,
                    digest != benchmark_previous_digest,
                    transfer_ended_us
                        > benchmark_frame_deadline_us
                            .saturating_add(deskkin_core_s3::PET_BENCHMARK_FRAME_PERIOD_US),
                );
                benchmark_previous_digest = digest;
                benchmark_frame_pending = false;
            }
            if VALID_RESULT.load(Ordering::Acquire) == 1 {
                FRAME_ATTEMPT.store(RESULT_ATTEMPT.load(Ordering::Acquire), Ordering::Release);
                LAST_STAGE.store(8, Ordering::Release);
            }
        }
        if first_frame {
            if unsafe { deskkin_display_enable() } != 0 {
                fail_boot(BootError::DisplayEnable);
                return;
            }
            first_frame = false;
            set_boot_stage(BootStage::FirstFrameReady);
        }
        if benchmark_active
            && Instant::now().as_micros()
                >= benchmark_started_at_us
                    .saturating_add(u64::from(deskkin_core_s3::PET_BENCHMARK_DURATION_MS) * 1_000)
            && benchmark_scheduled_frames == deskkin_core_s3::PET_BENCHMARK_REQUESTS
        {
            benchmark_summary.duration_ms =
                elapsed_us(benchmark_started_at_us, Instant::now().as_micros()) / 1_000;
            publish_pet_benchmark(&benchmark_summary, PetBenchmarkState::Complete);
        }
        embassy_time::Timer::after_millis(10).await;
    }
}
