// SPDX-License-Identifier: MIT

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use core::ffi::CStr;
use core::sync::atomic::{AtomicU32, Ordering};
use zephyr::printkln;

const EXPECTED_VALUE: u32 = 42;
static ATOMIC_EVIDENCE: AtomicU32 = AtomicU32::new(0);

extern "C" {
    fn deskkin_c_multiply(left: u32, right: u32) -> u32;
    fn deskkin_c_to_rust_check() -> u32;
    fn deskkin_c_idle();
    fn deskkin_firmware_digest() -> *const core::ffi::c_char;
    fn deskkin_interrupt_state_probe() -> u32;
    fn deskkin_wait_command(run_id: *mut u8) -> i32;
}

#[no_mangle]
extern "C" fn deskkin_rust_add(left: u32, right: u32) -> u32 {
    left + right
}

#[no_mangle]
extern "C" fn rust_main() {
    let digest = unsafe { CStr::from_ptr(deskkin_firmware_digest()) }
        .to_str()
        .expect("firmware digest is valid ASCII");
    loop {
        let mut run_id = [0_u8; 37];
        let action = unsafe { deskkin_wait_command(run_id.as_mut_ptr()) };
        if action < 0 {
            panic!("gate1c console unavailable");
        }
        let run_id = core::str::from_utf8(&run_id[..36]).expect("validated run id");
        if action == 1 {
            printkln!(
                "DESKKIN_GATE_EVENT schema=1 event=idle run_id={} firmware_digest={}",
                run_id,
                digest
            );
            continue;
        }

        printkln!(
            "DESKKIN_GATE_EVENT schema=1 event=boot run_id={} board={} firmware_digest={}",
            run_id,
            zephyr::kconfig::CONFIG_BOARD,
            digest
        );

        let rust_to_c = unsafe { deskkin_c_multiply(6, 7) };
        let c_to_rust = unsafe { deskkin_c_to_rust_check() };
        assert_eq!(rust_to_c, EXPECTED_VALUE);
        assert_eq!(c_to_rust, EXPECTED_VALUE);
        printkln!(
            "DESKKIN_GATE_EVENT schema=1 event=abi run_id={} c_to_rust={} rust_to_c={}",
            run_id,
            c_to_rust,
            rust_to_c
        );

        assert_eq!(unsafe { deskkin_interrupt_state_probe() }, 1);
        critical_section::with(|_| {
            assert_eq!(unsafe { deskkin_interrupt_state_probe() }, 0);
            critical_section::with(|_| {
                assert_eq!(unsafe { deskkin_interrupt_state_probe() }, 0);
                ATOMIC_EVIDENCE.store(EXPECTED_VALUE, Ordering::SeqCst);
            });
            assert_eq!(unsafe { deskkin_interrupt_state_probe() }, 0);
        });
        assert_eq!(unsafe { deskkin_interrupt_state_probe() }, 1);
        assert_eq!(ATOMIC_EVIDENCE.load(Ordering::SeqCst), EXPECTED_VALUE);
        printkln!(
            "DESKKIN_GATE_EVENT schema=1 event=atomic run_id={} value=42 nesting=ok restoration=ok",
            run_id
        );

        let allocation = Box::new(EXPECTED_VALUE);
        assert_eq!(*allocation, EXPECTED_VALUE);
        drop(allocation);
        printkln!(
            "DESKKIN_GATE_EVENT schema=1 event=allocation run_id={} value=42 freed=ok",
            run_id
        );

        if zephyr::kconfig::CONFIG_DESKKIN_GATE_MODE == 2 {
            printkln!(
                "DESKKIN_GATE_EVENT schema=1 event=panic_trigger run_id={} reason=deliberate",
                run_id
            );
            panic!("deskkin gate1c deliberate panic");
        }

        printkln!("DESKKIN_GATE_RESULT schema=1 run_id={} result=pass", run_id);
        printkln!(
            "DESKKIN_GATE_EVENT schema=1 event=idle run_id={} firmware_digest={}",
            run_id,
            digest
        );
        unsafe { deskkin_c_idle() };
    }
}
