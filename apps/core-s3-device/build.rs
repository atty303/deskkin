// SPDX-License-Identifier: GPL-3.0-only

fn main() {
    println!("cargo:rerun-if-changed=../../ui/status-surface.slint");
    let configuration = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer)
        .with_scale_factor(1.0);
    slint_build::compile_with_config("ui/device.slint", configuration)
        .expect("the Phase 3P CoreS3 UI must compile");
}
