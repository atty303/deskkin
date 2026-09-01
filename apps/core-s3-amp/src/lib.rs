// SPDX-License-Identifier: MIT

#![no_std]

extern crate zephyr;

unsafe extern "C" {
    fn deskkin_start_control_worker() -> core::ffi::c_int;
    fn deskkin_amp_service_failed();
    fn deskkin_amp_supervisor_main();
}

#[no_mangle]
extern "C" fn rust_main() {
    let service_ready = deskkin_core_s3_service::start().is_ok();
    let control_ready = service_ready && unsafe { deskkin_start_control_worker() } == 0;
    if !control_ready {
        unsafe { deskkin_amp_service_failed() };
    }
    unsafe { deskkin_amp_supervisor_main() };
}
