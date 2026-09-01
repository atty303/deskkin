use std::{env, fmt::Write, fs, path::PathBuf};

fn main() {
    let output =
        PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set")).join("trig_table.rs");
    let mut source = String::from("pub(crate) const SIN_Q15: [i16; 1024] = [\n");
    for index in 0..1024 {
        let radians = f64::from(index) * core::f64::consts::TAU / 1024.0;
        #[allow(clippy::cast_possible_truncation)]
        let value = (radians.sin() * 32767.0).round() as i16;
        write!(source, "{value},").expect("writing to a String cannot fail");
        if index % 16 == 15 {
            source.push('\n');
        }
    }
    source.push_str("];\n");
    fs::write(output, source).expect("write generated fixed-point trig table");
}
