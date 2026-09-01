// SPDX-License-Identifier: MIT

#![no_std]

extern crate zephyr;

unsafe extern "C" {
    fn deskkin_amp_boot_trace(stage: u8);
    fn deskkin_start_control_worker() -> core::ffi::c_int;
    fn deskkin_amp_service_failed();
    fn deskkin_amp_supervisor_main();
}

#[no_mangle]
extern "C" fn rust_main() {
    unsafe { deskkin_amp_boot_trace(1) };
    let control_ready = unsafe { deskkin_start_control_worker() } == 0;
    unsafe { deskkin_amp_boot_trace(if control_ready { 2 } else { 0x82 }) };
    let service_ready = control_ready && deskkin_core_s3_service::start().is_ok();
    unsafe { deskkin_amp_boot_trace(if service_ready { 3 } else { 0x83 }) };
    if !service_ready {
        unsafe { deskkin_amp_service_failed() };
    }
    unsafe { deskkin_amp_boot_trace(4) };
    unsafe { deskkin_amp_supervisor_main() };
}
