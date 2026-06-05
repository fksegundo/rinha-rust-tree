use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=RINHA_NATIVE_SCALE");

    let scale = env::var("RINHA_NATIVE_SCALE")
        .ok()
        .and_then(|value| value.parse::<i16>().ok())
        .unwrap_or(10000);

    if !(1..=11000).contains(&scale) {
        panic!("RINHA_NATIVE_SCALE must be between 1 and 11000, got {scale}");
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
    fs::write(
        out_dir.join("scale.rs"),
        format!("pub const SCALE: i16 = {scale};\n"),
    )
    .expect("failed to write generated scale");
}
