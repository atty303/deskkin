// SPDX-License-Identifier: MIT

#![no_std]

extern crate alloc;

use alloc::vec;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::{Duration, Timer};
use log::info;
use static_cell::StaticCell;
use zephyr::{embassy::Executor, printkln};

const EXPECTED_VALUE: u32 = 42;

static EXECUTOR: StaticCell<Executor> = StaticCell::new();
static EVIDENCE: Channel<CriticalSectionRawMutex, u32, 1> = Channel::new();

#[no_mangle]
extern "C" fn rust_main() {
    unsafe {
        zephyr::set_logger().expect("the Gate 1A logger must install once");
    }

    let board = zephyr::kconfig::CONFIG_BOARD;
    let clock_hz = zephyr::kconfig::CONFIG_SYS_CLOCK_HW_CYCLES_PER_SEC;
    let console_ord = zephyr::devicetree::chosen::zephyr_console::ORD;

    printkln!(
        "DESKKIN_GATE_EVENT schema=1 event=boot board={} clock_hz={} console_ord={}",
        board,
        clock_hz,
        console_ord
    );
    info!(
        "DESKKIN_GATE_LOG schema=1 event=logging status=ok board={}",
        board
    );

    let allocated = vec![10_u32, 20, 12];
    let allocation_sum: u32 = allocated.iter().sum();
    assert_eq!(allocation_sum, EXPECTED_VALUE);
    printkln!(
        "DESKKIN_GATE_EVENT schema=1 event=allocation value={}",
        allocation_sum
    );

    if zephyr::kconfig::CONFIG_DESKKIN_GATE_MODE == 2 {
        printkln!(
            "DESKKIN_GATE_EVENT schema=1 event=panic_trigger reason=deliberate"
        );
        panic!("deskkin gate1a deliberate panic");
    }

    let executor = EXECUTOR.init(Executor::new());
    executor.run(|spawner| {
        spawner
            .spawn(produce_evidence())
            .expect("the bounded Gate 1A task arena must fit one producer");
        spawner
            .spawn(verify_evidence())
            .expect("the bounded Gate 1A task arena must fit one verifier");
    });
}

#[embassy_executor::task]
async fn produce_evidence() {
    Timer::after(Duration::from_millis(10)).await;
    EVIDENCE.send(EXPECTED_VALUE).await;
}

#[embassy_executor::task]
async fn verify_evidence() {
    let received = EVIDENCE.receive().await;
    assert_eq!(received, EXPECTED_VALUE);
    printkln!(
        "DESKKIN_GATE_EVENT schema=1 event=async_wakeup value={}",
        received
    );
    printkln!("DESKKIN_GATE_RESULT schema=1 result=pass");

    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}
