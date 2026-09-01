// SPDX-License-Identifier: GPL-3.0-only

fn main() {
    println!("cargo:rerun-if-changed=ui/renderer.slint");
    println!("cargo:rerun-if-changed=../../../ui/pet-surface.slint");
    println!("cargo:rerun-if-changed=../../../assets/pets/koyori/idle.qoi");
    println!("cargo:rerun-if-changed=../../../assets/pets/koyori/move-right.qoi");
    println!("cargo:rerun-if-changed=../../../assets/pets/koyori/move-left.qoi");
    println!("cargo:rerun-if-changed=../../../assets/pets/koyori/attend.qoi");
    let configuration = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer)
        .with_scale_factor(1.0);
    slint_build::compile_with_config("ui/renderer.slint", configuration)
        .expect("the AMP renderer UI must compile");
}
