use std::{env, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../vendors");
    for tree in [
        "smoltcp-upstream",
        "tokio-upstream",
        "hyper-upstream",
        "axum-upstream",
        "rustls-upstream",
        "hickory-dns-upstream",
        "dhcp4r-upstream",
    ] {
        let path = root.join(tree);
        if path.is_dir() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}
