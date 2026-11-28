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
/// FIX-16: vérifie cryptographiquement la signature Ed25519 + BLAKE3 de la note EXOSIG.
/// - `SignatureState::Valid`   -> `SignedOk`
/// - `SignatureState::Invalid` -> TOUJOURS `Denied` (même en mode dev : falsification détectée)
/// - `SignatureState::Unsigned` -> `Denied` si `require_signature`, sinon `UnsignedAllowed`
pub fn check_exec_permission(image: &[u8], require_signature: bool) -> ExecCheckResult {
    match detect_signature_note(image) {
        SignatureState::Valid => ExecCheckResult::SignedOk,
        SignatureState::Invalid => {
            loader_log_byte(b'X');
            ExecCheckResult::Denied
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn var13_forged_exosig_is_always_denied_even_in_dev_mode() {
        let forged = b"\x7fELF\x02\x01\x01\0EXOSIG\0\0forged_payload";
        assert_eq!(check_exec_permission(forged, false), ExecCheckResult::Denied);
        assert_eq!(check_exec_permission(forged, true), ExecCheckResult::Denied);
    }

    #[test]
    fn var13_unsigned_denied_when_signature_required() {
        let unsigned = b"\x7fELF\x02\x01\x01\0plain_binary";
        assert_eq!(check_exec_permission(unsigned, true), ExecCheckResult::Denied);
        assert_eq!(
            check_exec_permission(unsigned, false),
            ExecCheckResult::UnsignedAllowed
        );
    }
}
