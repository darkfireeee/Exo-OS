// loader/src/security/capability_check.rs
//
// FIX-APP-09 (Security_Application_Audit §GAP-09) : remplace le stub 4 lignes
// par une vérification réelle de la signature de module via detect_signature_note().
//
// En v0.2.0, le loader détecte la présence d'une note EXOSIG dans le binaire.
// Un binaire non signé peut encore s'exécuter (flag ALLOW_UNSIGNED) mais génère
// un log d'avertissement via le port de debug 0xE9.
// En production, set LOADER_REQUIRE_SIGNATURE=1 dans exo-boot.cfg pour bloquer.

use super::verify_signature::{detect_signature_note, SignatureState};

pub const CAP_EXEC: u64 = 1 << 0;
pub const CAP_EXEC_SIGNED: u64 = 1 << 1; // binaire avec signature Ed25519 valide
pub const CAP_EXEC_UNSIGNED: u64 = 1 << 2; // binaire autorisé sans signature (dev only)

/// Résultat de la vérification d'exécution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecCheckResult {
    /// Binaire signé — exécution autorisée.
    SignedOk,
    /// Binaire non signé — autorisé en mode dev (avertissement émis).
    UnsignedAllowed,
    /// Exécution refusée (mode production strict).
    Denied,
}

#[inline(always)]
fn loader_log_byte(b: u8) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let msg = [b];
        core::arch::asm!(
            "syscall",
            in("rax") 350u64, // SYS_EXO_LOG
            in("rdi") msg.as_ptr() as u64,
            in("rsi") 1u64,
            lateout("rax") _,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = b;
}

/// Vérifie si un binaire ELF peut être exécuté.
///
/// FIX-APP-09 : remplace `may_exec(mask)` par une vérification de la note EXOSIG.
/// La présence de la note indique que le binaire a été signé par la clé de build.
/// La vérification cryptographique complète est faite dans kernel::do_execve()
/// via security::verify_module_signature().
pub fn check_exec_permission(image: &[u8], require_signature: bool) -> ExecCheckResult {
    match detect_signature_note(image) {
        SignatureState::Present => ExecCheckResult::SignedOk,
        SignatureState::Unsigned => {
            if require_signature {
                loader_log_byte(b'U');
                ExecCheckResult::Denied
            } else {
                loader_log_byte(b'u');
                ExecCheckResult::UnsignedAllowed
            }
        }
    }
}

/// Compatibilité avec l'ancienne API (utilisée par le linker ELF du loader).
#[inline(always)]
pub fn may_exec(mask: u64) -> bool {
    mask & CAP_EXEC != 0
}
