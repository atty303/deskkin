// SPDX-License-Identifier: MIT

#![no_std]

use core::sync::atomic::{AtomicU32, Ordering};
use zephyr::printkln;

const EXPECTED_VALUE: u32 = 42;
static ATOMIC_EVIDENCE: AtomicU32 = AtomicU32::new(0);

extern "C" {
    fn deskkin_c_multiply(left: u32, right: u32) -> u32;
    fn deskkin_c_to_rust_check() -> u32;
    fn deskkin_c_idle();
}

#[no_mangle]
extern "C" fn deskkin_rust_add(left: u32, right: u32) -> u32 {
    left + right
}

#[no_mangle]
extern "C" fn rust_main() {
    printkln!(
        "DESKKIN_GATE_EVENT schema=1 event=boot board={} build_id=gate1c-0.1.0",
        zephyr::kconfig::CONFIG_BOARD
    );

    let rust_to_c = unsafe { deskkin_c_multiply(6, 7) };
    let c_to_rust = unsafe { deskkin_c_to_rust_check() };
    assert_eq!(rust_to_c, EXPECTED_VALUE);
    assert_eq!(c_to_rust, EXPECTED_VALUE);
    printkln!(
        "DESKKIN_GATE_EVENT schema=1 event=abi c_to_rust={} rust_to_c={}",
        c_to_rust,
        rust_to_c
    );

    critical_section::with(|_| {
        ATOMIC_EVIDENCE.store(EXPECTED_VALUE, Ordering::SeqCst);
    });
    assert_eq!(ATOMIC_EVIDENCE.load(Ordering::SeqCst), EXPECTED_VALUE);
    printkln!("DESKKIN_GATE_EVENT schema=1 event=atomic value=42");

    if zephyr::kconfig::CONFIG_DESKKIN_GATE_MODE == 2 {
        printkln!("DESKKIN_GATE_EVENT schema=1 event=panic_trigger reason=deliberate");
        panic!("deskkin gate1c deliberate panic");
    }

    printkln!("DESKKIN_GATE_RESULT schema=1 result=pass");
    printkln!("DESKKIN_GATE_EVENT schema=1 event=idle firmware_id=gate1c-0.1.0");
    loop {
        unsafe { deskkin_c_idle() };
    }
}
