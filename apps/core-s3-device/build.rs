// SPDX-License-Identifier: GPL-3.0-only

fn main() {
    println!("cargo:rerun-if-changed=../../ui/status-surface.slint");
    slint_build::compile("ui/device.slint").expect("the Phase 3P CoreS3 UI must compile");
}
