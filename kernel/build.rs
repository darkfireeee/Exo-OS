// kernel/build.rs — Script de build du noyau Exo-OS
//
// Responsabilités :
//  1. Passe le linker script `linker.ld` à rust-lld via cargo:rustc-link-arg.
//  2. Déclare les fichiers qui, en cas de modification, déclenchent un rebuild.
//
// Pas besoin du crate `cc` : les fichiers .s sont inclus via `global_asm!(include_str!())`
// directement dans les fichiers Rust concernés (switch.rs pour switch_asm.s).

fn main() {
    // Répertoire du crate (chemin absolu fourni par Cargo)
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let target = std::env::var("TARGET").unwrap_or_default();

    // Passer le linker script à rust-lld uniquement pour les cibles bare-metal.
    // Le chemin DOIT être absolu : le linker est invoqué depuis un répertoire temporaire.
    if target.contains("none") || target.contains("exo") {
        println!("cargo:rustc-link-arg=-T{dir}/linker.ld");
    }

    // Rebuild si le linker script change.
    println!("cargo:rerun-if-changed={dir}/linker.ld");

    // Rebuild si un fichier ASM change (les .s sont inclus via include_str!).
    println!("cargo:rerun-if-changed={dir}/src/scheduler/asm/switch_asm.s");
    println!("cargo:rerun-if-changed={dir}/src/scheduler/asm/fast_path.s");
    println!("cargo:rerun-if-changed={dir}/src/ipc/core/fastcall_asm.s");
}
