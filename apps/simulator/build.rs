fn main() {
    println!("cargo:rerun-if-changed=../../ui/status-surface.slint");
    println!("cargo:rerun-if-changed=../../ui/pet-surface.slint");
    println!("cargo:rerun-if-changed=../../assets/pets/koyori/idle.qoi");
    println!("cargo:rerun-if-changed=../../assets/pets/koyori/move-right.qoi");
    println!("cargo:rerun-if-changed=../../assets/pets/koyori/move-left.qoi");
    println!("cargo:rerun-if-changed=../../assets/pets/koyori/attend.qoi");
    slint_build::compile("ui/status.slint").expect("compile shared status UI");
}
