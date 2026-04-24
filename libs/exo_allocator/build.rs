use cc;
use std::path::Path;

fn main() {
    let vendor_path = Path::new("vendor/mimalloc");

    if !vendor_path.exists() {
        panic!("Mimalloc sources not found in vendor/mimalloc");
    }

    // Mimalloc uses "amalgamation" style - only compile static.c which includes all others
    cc::Build::new()
        .include("vendor/mimalloc/include")
        .file("vendor/mimalloc/src/static.c")
        .flag_if_supported("-O3")
        .flag_if_supported("-DMI_SECURE=4")
        .define("MI_STATIC_LIB", None)
        .compile("mimalloc");

    println!("cargo:rerun-if-changed=vendor/mimalloc");
    println!("cargo:rustc-link-lib=static=mimalloc");
}
