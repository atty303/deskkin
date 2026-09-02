// SPDX-License-Identifier: MIT

#![no_std]

extern crate zephyr;

unsafe extern "C" {
    fn deskkin_amp_prepare_renderer() -> core::ffi::c_int;
    fn deskkin_start_control_worker() -> core::ffi::c_int;
    fn deskkin_amp_service_failed();
    fn deskkin_amp_supervisor_main();
}

#[no_mangle]
extern "C" fn rust_main() {
    let _renderer_ready = unsafe { deskkin_amp_prepare_renderer() } == 0;
    let control_ready = unsafe { deskkin_start_control_worker() } == 0;
    let service_ready = control_ready && deskkin_core_s3_service::start().is_ok();
    if !service_ready {
        unsafe { deskkin_amp_service_failed() };
    }
    unsafe { deskkin_amp_supervisor_main() };
}
