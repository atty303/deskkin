fn main() {
    println!("cargo:rerun-if-changed=../../ui/status-surface.slint");
    slint_build::compile("ui/status.slint").expect("compile shared status UI");
}
