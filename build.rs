//! axe-core (`vendor/axe-core`, MPL-2.0, unmodified) goes into the binary
//! deflated, for `a11y`: its 652 KB of script as about 182 KB.

fn main() {
    let src = "vendor/axe-core/axe.min.js";
    println!("cargo:rerun-if-changed={src}");
    let raw = std::fs::read(src).expect("vendor/axe-core/axe.min.js is missing");
    let packed = miniz_oxide::deflate::compress_to_vec(&raw, 10);
    let out = std::path::Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("axe.min.js.deflate");
    std::fs::write(out, packed).expect("write the deflated axe-core");
}
