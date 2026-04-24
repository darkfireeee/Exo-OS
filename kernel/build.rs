// kernel/build.rs — Script de build du noyau Exo-OS
//
// Responsabilités :
//  1. Passe le linker script `linker.ld` à rust-lld via cargo:rustc-link-arg.
//  2. Déclare les fichiers qui, en cas de modification, déclenchent un rebuild.
//  3. Génère les blobs de hash ExoPhoenix (Kernel A) dans OUT_DIR.
//
// Pas besoin du crate `cc` : les fichiers .s sont inclus via `global_asm!(include_str!())`
// directement dans les fichiers Rust concernés (switch.rs pour switch_asm.s).

use std::path::Path;

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(10 + (b - b'a')),
        b'A'..=b'F' => Some(10 + (b - b'A')),
        _ => None,
    }
}

fn parse_hash32_hex(var_name: &str) -> [u8; 32] {
    println!("cargo:rerun-if-env-changed={var_name}");

    let Ok(raw) = std::env::var(var_name) else {
        println!(
            "cargo:warning={var_name} non défini — fallback hash nul (mode dégradé ExoPhoenix)"
        );
        return [0u8; 32];
    };

    let bytes = raw.as_bytes();
    if bytes.len() != 64 {
        println!(
            "cargo:warning={var_name} invalide (len={}) — attendu 64 hex chars, fallback hash nul",
            bytes.len()
        );
        return [0u8; 32];
    }

    let mut out = [0u8; 32];
    let mut i = 0usize;
    while i < 32 {
        let hi = hex_nibble(bytes[i * 2]);
        let lo = hex_nibble(bytes[i * 2 + 1]);
        let (Some(hi), Some(lo)) = (hi, lo) else {
            println!(
                "cargo:warning={var_name} contient des caractères non-hex — fallback hash nul"
            );
            return [0u8; 32];
        };
        out[i] = (hi << 4) | lo;
        i += 1;
    }

    out
}

fn write_hash_blob(path: &Path, bytes: &[u8; 32]) {
    std::fs::write(path, bytes).expect("écriture hash ExoPhoenix OUT_DIR");
}

fn main() {
    // Répertoire du crate (chemin absolu fourni par Cargo)
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let target = std::env::var("TARGET").unwrap_or_default();
    let linker_script = Path::new(&dir)
        .parent()
        .expect("workspace root manquant")
        .join("exo-boot")
        .join("linker")
        .join("linker.ld");
    let linker_script = linker_script
        .to_str()
        .expect("chemin linker non UTF-8")
        .to_owned();

    // Passer le linker script à rust-lld uniquement pour les cibles bare-metal.
    // Le chemin DOIT être absolu : le linker est invoqué depuis un répertoire temporaire.
    if target.contains("none") || target.contains("exo") {
        println!("cargo:rustc-link-arg=-T{linker_script}");
    }

    // Rebuild si le linker script change.
    println!("cargo:rerun-if-changed={linker_script}");

    // Rebuild si un fichier ASM change (les .s sont inclus via include_str!).
    println!("cargo:rerun-if-changed={dir}/src/scheduler/asm/switch_asm.s");
    println!("cargo:rerun-if-changed={dir}/src/scheduler/asm/fast_path.s");
    println!("cargo:rerun-if-changed={dir}/src/ipc/core/fastcall_asm.s");

    // Génération des hashes Kernel A injectés au link pour ExoPhoenix/forge.
    // Variables attendues:
    // - KERNEL_A_IMAGE_HASH  : 64 hex chars
    // - KERNEL_A_MERKLE_ROOT : 64 hex chars
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR manquant");
    let out = std::path::PathBuf::from(out_dir);

    let image_hash = parse_hash32_hex("KERNEL_A_IMAGE_HASH");
    let merkle_root = parse_hash32_hex("KERNEL_A_MERKLE_ROOT");

    write_hash_blob(&out.join("kernel_a_image_hash.bin"), &image_hash);
    write_hash_blob(&out.join("kernel_a_merkle_root.bin"), &merkle_root);
}
