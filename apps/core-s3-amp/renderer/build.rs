// SPDX-License-Identifier: GPL-3.0-only

fn main() {
    println!("cargo:rerun-if-changed=ui/renderer.slint");
    let configuration = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer)
        .with_scale_factor(1.0);
    slint_build::compile_with_config("ui/renderer.slint", configuration)
        .expect("the AMP renderer UI must compile");
}
