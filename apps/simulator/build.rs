fn main() {
    println!("cargo:rerun-if-changed=../../ui/status-surface.slint");
    println!("cargo:rerun-if-changed=../../ui/pet-surface.slint");
    println!("cargo:rerun-if-changed=../../assets/pets/koyori/atlas.png");
    slint_build::compile("ui/status.slint").expect("compile shared status UI");
}
