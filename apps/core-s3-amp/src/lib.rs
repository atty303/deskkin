// SPDX-License-Identifier: MIT

#![no_std]

extern crate zephyr;

unsafe extern "C" {
    fn deskkin_amp_prepare_renderer() -> core::ffi::c_int;
    fn deskkin_install_allocation_failed_probe();
    fn deskkin_amp_supervisor_main();
    fn deskkin_start_control_worker() -> core::ffi::c_int;
}

#[no_mangle]
extern "C" fn rust_main() {
    unsafe { deskkin_install_allocation_failed_probe() };
    if unsafe { deskkin_start_control_worker() } != 0 {
        return;
    }
    let _renderer_ready = unsafe { deskkin_amp_prepare_renderer() } == 0;
    unsafe { deskkin_amp_supervisor_main() };
}

#[no_mangle]
extern "C" fn deskkin_start_service_after_runtime_handoff() -> core::ffi::c_int {
    if deskkin_core_s3_service::start().is_ok() {
        0
    } else {
        -1
    }
}
