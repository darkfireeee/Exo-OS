//! Script de build Cargo pour le kernel Exo-OS.
//!
//! Rôle :
//! 1. Indiquer au linker d'utiliser `linker.ld` (chemin absolu depuis CARGO_MANIFEST_DIR)
//! 2. Invalider le cache si le script de linker change
//! 3. Définir des constantes de build accessibles via `env!()`

fn main() {
    // ── Linker script ─────────────────────────────────────────────────────────
    // Fournit le chemin absolu du script pour que rust-lld le trouve quel que
    // soit le répertoire de travail au moment du linkage.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR non défini");
    let linker_script = format!("{}/linker.ld", manifest_dir);

    // Passe le chemin absolu au linker
    println!("cargo:rustc-link-arg=-T{}", linker_script);
    println!("cargo:rustc-link-arg=--no-pie");

    // Reconstruire si le linker script ou ce fichier changent
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", linker_script);

    // ── Constantes de build ───────────────────────────────────────────────────
    println!(
        "cargo:rustc-env=EXOOS_BUILD_PROFILE={}",
        std::env::var("PROFILE").unwrap_or_else(|_| "unknown".to_string())
    );
}
