//! Script de build pour exo-boot.
//!
//! Responsabilités :
//! 1. Sélectionner le linker script selon la cible (UEFI ou BIOS).
//! 2. Invalider le cache Cargo si les linker scripts changent.
//! 3. Exporter les métadonnées de compilation (profil, target).
//! 4. Vérifier la cohérence feature/target pour éviter des builds silencieusement faux.

use std::env;
use std::path::{Path, PathBuf};

fn main() {
    // ── Variables d'environnement de build ─────────────────────────────────────
    let target       = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let target_os    = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let profile      = env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR manquant");

    let linker_dir = PathBuf::from(&manifest_dir).join("linker");

    // ── Sélection du linker script ─────────────────────────────────────────────
    // UEFI : target_os = "uefi"  → lld-link (PE/COFF), pas de script GNU ld
    // BIOS : target_os = "none"  → bios.ld  (ELF plat, adresse 0x7C00 / 0x100000)
    if target_os == "uefi" {
        // lld-link gère automatiquement les headers PE32+.
        // Ne pas passer de script -T ni d'options GNU ld (--gc-sections, -mno-red-zone)
        // car lld-link (flaveur PE) ne les supporte pas.
    } else if target_os == "none" || target == "x86" || target == "x86_64" {
        let script = linker_dir.join("bios.ld");
        assert_exists(&script, "linker/bios.ld");
        cargo_link_arg("-T");
        cargo_link_arg(script.to_str().unwrap());
        // GNU ld uniquement : no-red-zone et gc-sections
        println!("cargo:rustc-link-arg=-mno-red-zone");
        println!("cargo:rustc-link-arg=--gc-sections");
    } else {
        println!("cargo:warning=Target OS '{}' inconnu, aucun linker script appliqué.", target_os);
    }

    // ── Invalidation du cache Cargo ────────────────────────────────────────────
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=linker/uefi.ld");
    println!("cargo:rerun-if-changed=linker/bios.ld");
    println!("cargo:rerun-if-changed=linker/linker.ld");
    println!("cargo:rerun-if-changed=src/main.rs");
    println!("cargo:rerun-if-changed=src/uefi/entry.rs");
    println!("cargo:rerun-if-changed=src/bios/mbr.asm");
    println!("cargo:rerun-if-changed=src/bios/stage2.asm");

    // ── Constantes de build exportées pour le code Rust ───────────────────────
    println!("cargo:rustc-env=EXOBOOT_BUILD_PROFILE={}", profile);
    println!("cargo:rustc-env=EXOBOOT_BUILD_TARGET={}", target_os);
    println!("cargo:rustc-env=EXOBOOT_BUILD_ARCH={}", target);
    println!("cargo:rustc-env=EXOBOOT_BUILD_DATE={}", build_date_iso8601());

    // ── Vérification cohérence feature/target ─────────────────────────────────
    let feature_uefi = env::var("CARGO_FEATURE_UEFI_BOOT").is_ok();
    let feature_bios = env::var("CARGO_FEATURE_BIOS_BOOT").is_ok();

    if feature_uefi && target_os == "none" {
        println!(
            "cargo:warning=Feature 'uefi-boot' activée mais target_os='none' (BIOS). \
             Assurez-vous de cibler x86_64-unknown-uefi pour UEFI."
        );
    }
    if feature_bios && target_os == "uefi" {
        println!(
            "cargo:warning=Feature 'bios-boot' activée mais target_os='uefi'. \
             Assurez-vous de cibler x86_64-unknown-none pour BIOS."
        );
    }
    if !feature_uefi && !feature_bios {
        panic!("Exactement une feature parmi 'uefi-boot' ou 'bios-boot' doit être activée.");
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn cargo_link_arg(arg: &str) {
    println!("cargo:rustc-link-arg={}", arg);
}

fn assert_exists(path: &Path, label: &str) {
    if !path.exists() {
        panic!(
            "Fichier requis introuvable : {} (cherché à {:?})",
            label, path
        );
    }
}

/// Retourne la date de compilation au format ISO 8601 (YYYY-MM-DD).
/// Respecte SOURCE_DATE_EPOCH pour les builds reproductibles.
fn build_date_iso8601() -> String {
    if let Ok(epoch_str) = env::var("SOURCE_DATE_EPOCH") {
        if let Ok(epoch) = epoch_str.parse::<u64>() {
            return epoch_to_iso8601(epoch);
        }
    }
    "2026-02-23".to_string()
}

/// Conversion minimale d'un timestamp Unix en date ISO 8601.
/// Algorithme Euclidian affine — précision : jours. Valide 1970–2099.
fn epoch_to_iso8601(epoch: u64) -> String {
    const SECONDS_PER_DAY: u64 = 86400;
    const DAYS_IN_400_YEARS: u64 = 146097;
    let z = epoch / SECONDS_PER_DAY + 719468;
    let era = z / DAYS_IN_400_YEARS;
    let doe = z - era * DAYS_IN_400_YEARS;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}", y, m, d)
}
