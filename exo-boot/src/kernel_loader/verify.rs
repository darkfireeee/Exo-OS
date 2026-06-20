//! verify.rs — Vérification de signature kernel (Ed25519) **fail-closed**.
//!
//! RÈGLE BOOT-02 : signature vérifiée AVANT tout chargement.
//!
//! Ce module est un **adaptateur mince** sur la crate partagée [`exo_verity`] :
//! le format de signature et la logique de vérification sont définis une seule
//! fois et utilisés à l'identique par l'outil de signature (`tools/kernel_signer`)
//! et par le bootloader → aucune divergence possible.
//!
//! # Ce qui a changé (FIX-DEEP-CRYPTO) vs l'ancien code
//! - **Plus de stub fail-open** : l'ancien chemin « sans secure-boot » renvoyait
//!   `Ok`/`true` pour n'importe quel kernel → « signature valide » était un
//!   mensonge. Désormais le verdict est honnête ([`KernelVerdict`]).
//! - **Crypto toujours compilée** : Ed25519+SHA-512 ne sont plus derrière une
//!   feature qui les éteint. Seule la *politique* (refuser vs avertir) est
//!   configurable.
//! - **Vraie clé** : la clé publique embarquée est générée par `kernel_signer`
//!   (`signing_key.rs`), pas un vecteur de test. Garde de compilation ci-dessous.
//! - **`verify_strict`** (dans `exo_verity`) : anti-clé-faible + anti-malléabilité.
//! - **Altéré ≠ non signé** : une image *altérée* est refusée **même en dev**.

use super::signing_key::KERNEL_SIGNING_PUBLIC_KEY;
pub use exo_verity::KernelVerdict;

/// Garde de COMPILATION : le bootloader ne compile pas si la clé embarquée est
/// nulle ou un vecteur de test connu. La « fausse sécurité » ne peut pas revenir.
const _: () = assert!(
    exo_verity::key_is_usable(&KERNEL_SIGNING_PUBLIC_KEY),
    "KERNEL_SIGNING_PUBLIC_KEY inexploitable (nulle/vecteur de test) — \
     régénérez : cargo run -p exo-kernel-signer -- keygen --force"
);

/// Vérifie l'image kernel contre la clé embarquée. Ne panique pas, n'alloue pas.
///
/// Fonctionne pour les deux chemins :
/// - **UEFI** : `image` = le fichier signé exact (footer en toute fin) → vérif
///   directe.
/// - **BIOS** : `image` = la fenêtre shadow (64 MiB, footer PAS en fin de
///   tampon) → on localise le footer juste après la fin réelle du fichier ELF
///   (calculée depuis les en-têtes), puis on vérifie la tranche correspondante.
pub fn verify_kernel(image: &[u8]) -> KernelVerdict {
    // 1. Cas exact (image == fichier signé) : footer aux 256 derniers octets.
    let v = exo_verity::verify_image(image, &KERNEL_SIGNING_PUBLIC_KEY);
    // Verified / Tampered / NoVerifierKey sont définitifs ; seul Unsigned (pas de
    // marqueur à la fin du tampon) justifie de chercher plus loin (cas BIOS).
    if v != KernelVerdict::Unsigned {
        return v;
    }
    // 2. Cas tampon large (BIOS) : footer juste après la fin du fichier ELF.
    if let Some(signed_end) = elf_signed_end(image) {
        if signed_end <= image.len() {
            return exo_verity::verify_image(&image[..signed_end], &KERNEL_SIGNING_PUBLIC_KEY);
        }
    }
    KernelVerdict::Unsigned
}

/// Fin du fichier signé dans un tampon = fin réelle de l'ELF64 + footer (256 o).
/// Lit les en-têtes ELF64 (programme + sections) pour déterminer l'octet de fin
/// du fichier. Bornes vérifiées ; `None` si l'en-tête ELF est invalide/tronqué.
fn elf_signed_end(buf: &[u8]) -> Option<usize> {
    elf_file_end(buf)?.checked_add(exo_verity::SIG_FOOTER_SIZE)
}

fn rd_u16(b: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(off)?, *b.get(off + 1)?]))
}
fn rd_u64(b: &[u8], off: usize) -> Option<u64> {
    let s = b.get(off..off + 8)?;
    Some(u64::from_le_bytes([
        s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
    ]))
}

/// Octet de fin du fichier ELF64 (max des fins de tables/segments).
fn elf_file_end(b: &[u8]) -> Option<usize> {
    // En-tête ELF64 minimal (64 octets) + magic.
    if b.len() < 64 || &b[0..4] != b"\x7FELF" || b[4] != 2 {
        return None; // pas un ELF64
    }
    let phoff = rd_u64(b, 0x20)?;
    let shoff = rd_u64(b, 0x28)?;
    let phentsize = rd_u16(b, 0x36)? as u64;
    let phnum = rd_u16(b, 0x38)? as u64;
    let shentsize = rd_u16(b, 0x3A)? as u64;
    let shnum = rd_u16(b, 0x3C)? as u64;

    let mut end: u64 = 64; // au moins l'en-tête ELF
    end = end.max(phoff.checked_add(phnum.checked_mul(phentsize)?)?);
    end = end.max(shoff.checked_add(shnum.checked_mul(shentsize)?)?);

    // Fin de chaque segment de programme (p_offset + p_filesz), borné par phnum.
    let phnum_capped = phnum.min(256); // garde-fou anti-en-tête corrompu
    let mut i = 0u64;
    while i < phnum_capped {
        let ph = (phoff + i * phentsize) as usize;
        let p_offset = rd_u64(b, ph + 0x08)?;
        let p_filesz = rd_u64(b, ph + 0x20)?;
        end = end.max(p_offset.checked_add(p_filesz)?);
        i += 1;
    }
    usize::try_from(end).ok()
}

/// Décision de boot dérivée d'un verdict + de la politique d'enforcement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootDecision {
    /// Image vérifiée — continuer.
    Proceed,
    /// Continuer mais avertir (mode dev permissif). Le message est à logger.
    ProceedWithWarning(&'static str),
    /// Refuser le démarrage (fail-closed). Le message explique pourquoi.
    Refuse(&'static str),
}

/// Applique la politique de boot à un verdict.
///
/// - `require_signed`     : `secure_boot_required` de la config exo-boot.cfg.
/// - `uefi_sb_enforcing`  : UEFI Secure Boot actif en mode enforcing (false en BIOS).
///
/// Règles (fail-closed) :
/// - **Verified**       → toujours OK.
/// - **Tampered**       → **toujours refusé** (signal d'attaque, même en dev).
/// - **NoVerifierKey**  → refusé si signature requise/SB enforcing ; sinon averti.
/// - **Unsigned**       → refusé si signature requise/SB enforcing ; sinon averti.
pub fn decide(verdict: KernelVerdict, require_signed: bool, uefi_sb_enforcing: bool) -> BootDecision {
    let strict = require_signed || uefi_sb_enforcing;
    match verdict {
        KernelVerdict::Verified => BootDecision::Proceed,
        KernelVerdict::Tampered => {
            BootDecision::Refuse("image kernel ALTEREE (signature presente mais invalide)")
        }
        KernelVerdict::NoVerifierKey => {
            if strict {
                BootDecision::Refuse("aucune cle de verification exploitable")
            } else {
                BootDecision::ProceedWithWarning(
                    "AUCUNE cle de verification — kernel NON verifie (dev)",
                )
            }
        }
        KernelVerdict::Unsigned => {
            if strict {
                BootDecision::Refuse("kernel NON signe alors qu'une signature est requise")
            } else {
                BootDecision::ProceedWithWarning("kernel NON signe accepte (mode dev permissif)")
            }
        }
    }
}

/// Vérifie + applique la politique. Panique (fail-closed) si refusé. Retourne
/// `Some(avertissement)` à logger en mode dev permissif, `None` si vérifié.
///
/// RÈGLE BOOT-02 : appelé AVANT le chargement, sur les deux chemins (UEFI/BIOS).
#[must_use = "logger l'avertissement de boot le cas échéant"]
pub fn enforce_or_panic(
    image: &[u8],
    require_signed: bool,
    uefi_sb_enforcing: bool,
) -> Option<&'static str> {
    match decide(verify_kernel(image), require_signed, uefi_sb_enforcing) {
        BootDecision::Proceed => None,
        BootDecision::ProceedWithWarning(w) => Some(w),
        BootDecision::Refuse(why) => panic!("BOOT-02 : {why} — demarrage REFUSE"),
    }
}

/// Défense en profondeur : panique **uniquement** si l'image est altérée
/// (signature présente mais invalide). À appeler juste avant de charger les
/// segments — garantit qu'une image altérée n'est JAMAIS chargée, même si une
/// couche supérieure a mal appliqué la politique. Le cas « non signé » est laissé
/// à la politique de `enforce_or_panic` (déjà appliquée en amont).
#[inline]
pub fn refuse_if_tampered(image: &[u8]) {
    if verify_kernel(image).is_tampered() {
        panic!("BOOT-02 : image kernel ALTEREE detectee au chargement — REFUSE");
    }
}
