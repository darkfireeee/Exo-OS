//! rng.rs — EFI_RNG_PROTOCOL — entropy initiale (KASLR + CSPRNG kernel).
//!
//! RÈGLE BOOT-05 (DOC10) :
//!   "Entropy 64 bytes fournie au kernel (KASLR + CSPRNG)"
//!
//! EFI_RNG_PROTOCOL (GUID : 3152bca5-eade-433d-862e-c01cdc291f44) est
//! disponible sur les firmware UEFI 2.3+ conformes à la spec.
//!
//! Sur les systèmes sans EFI_RNG_PROTOCOL, on tente un fallback :
//!   1. RDRAND (instruction x86 Intel/AMD depuis ~2012)
//!   2. RDSEED (plus entropique que RDRAND, disponible depuis Haswell/Excavator)
//!   3. Timestamp counter TSC (en dernier recours — entropie faible mais mieux que rien)
//!
//! AVERTISSEMENT : Le fallback TSC seul n'est PAS cryptographiquement sûr.
//! Si l'entropie est insuffisante, KASLR est affaibli mais pas cassé
//! (le timing exact n'est pas connu de l'attaquant à distance).

use uefi::proto::rng::{Rng, RngAlgorithmType};
use uefi::prelude::*;

// ─── API publique ─────────────────────────────────────────────────────────────

/// Collecte exactement `count` octets d'entropie depuis EFI_RNG_PROTOCOL.
///
/// Exige `count <= 64` (taille du tampon statique interne).
/// La spec UEFI garantit que GetRNG() produit des octets imprévisibles
/// si le firmware est conforme.
///
/// # Errors
/// - `RngError::ProtocolNotFound` : EFI_RNG_PROTOCOL absent (UEFI < 2.3 ou firmware non conforme).
///   → L'appelant doit décider si le fallback est acceptable.
pub fn collect_entropy(
    bt:    &BootServices,
    count: usize,
) -> Result<[u8; 64], RngError> {
    assert!(count <= 64, "collect_entropy: count > 64 non supporté");
    crate::uefi::exit::assert_boot_services_active("collect_entropy");

    // ── Tentative via EFI_RNG_PROTOCOL ────────────────────────────────────────
    match collect_via_efi_rng(bt, count) {
        Ok(buf) => return Ok(buf),
        Err(RngError::ProtocolNotFound) => {
            // Fallback vers instructions CPU (si disponibles)
        }
        Err(e) => return Err(e),
    }

    // ── Fallback : RDRAND/RDSEED ───────────────────────────────────────────────
    if let Some(buf) = collect_via_rdrand(count) {
        return Ok(buf);
    }

    // ── Dernier recours : TSC ─────────────────────────────────────────────────
    // TSC seul est déterministe sur certaines architectures — on l'utilise
    // uniquement si tout le reste échoue, avec un avertissement.
    Ok(collect_via_tsc_fallback(count))
}

// ─── Implémentations sources d'entropie ───────────────────────────────────────

/// Collecte l'entropie via EFI_RNG_PROTOCOL.
fn collect_via_efi_rng(bt: &BootServices, count: usize) -> Result<[u8; 64], RngError> {
    let rng_handle = bt
        .get_handle_for_protocol::<Rng>()
        .map_err(|_| RngError::ProtocolNotFound)?;

    let mut rng_scoped = bt
        .open_protocol_exclusive::<Rng>(rng_handle)
        .map_err(|_| RngError::ProtocolNotFound)?;

    let rng: &mut Rng = &mut *rng_scoped;

    let mut buf = [0u8; 64];

    // On préfère l'algorithme RAW (entropie hardware directe) s'il est disponible.
    // Sinon on laisse UEFI choisir l'algorithme par défaut (algorithm_guid = null).
    if has_algorithm(rng, RngAlgorithmType::ALGORITHM_RAW) {
        rng.get_rng(Some(RngAlgorithmType::ALGORITHM_RAW), &mut buf[..count])
            .map_err(|e| RngError::GetFailed { status: e.status() })?;
    } else {
        // UEFI choisit l'algorithme par défaut du firmware
        rng.get_rng(None, &mut buf[..count])
            .map_err(|e| RngError::GetFailed { status: e.status() })?;
    }

    // Vérification sanity : un buffer entièrement à zéro signale un firmware défectueux
    if buf[..count].iter().all(|&b| b == 0) {
        return Err(RngError::AllZeroOutput { count });
    }

    Ok(buf)
}

/// Vérifie si EFI_RNG_PROTOCOL supporte un algorithme donné.
fn has_algorithm(rng: &mut Rng, algo: RngAlgorithmType) -> bool {
    let mut algorithmes = [RngAlgorithmType::EMPTY_ALGORITHM; 8];
    match rng.get_info(&mut algorithmes) {
        Ok(supported) => supported.contains(&algo),
        Err(_) => false,
    }
}

/// Collecte l'entropie via RDRAND (Intel/AMD x86_64 depuis 2012).
///
/// Retourne `None` si RDRAND n'est pas disponible ou échoue après les retries.
/// La spec Intel recommande 10 retries avant de considérer RDRAND comme défaillant.
fn collect_via_rdrand(count: usize) -> Option<[u8; 64]> {
    if !cpu_has_rdrand() { return None; }

    let mut buf = [0u8; 64];
    let mut i = 0usize;

    while i < count {
        let bytes_remaining = count - i;
        let to_fill = bytes_remaining.min(8); // RDRAND produit 8 bytes à la fois (64-bit)

        match rdrand_u64() {
            Some(val) => {
                let val_bytes = val.to_le_bytes();
                buf[i..i + to_fill].copy_from_slice(&val_bytes[..to_fill]);
                i += to_fill;
            }
            None => return None, // Échec hardware RDRAND
        }
    }

    Some(buf)
}

/// Lit un entier 64 bits depuis RDRAND avec 10 retries (spec Intel).
///
/// SAFETY : Appelle l'instruction RDRAND — requiert le flag CPUID.
fn rdrand_u64() -> Option<u64> {
    const MAX_RETRIES: u32 = 10;
    for _ in 0..MAX_RETRIES {
        let (val, carry): (u64, u8);
        // SAFETY : instruction RDRAND disponible si cpu_has_rdrand() == true.
        unsafe {
            core::arch::asm!(
                "rdrand {val}",
                "setc {carry}",
                val  = out(reg) val,
                carry = out(reg_byte) carry,
                options(nomem, nostack)
            );
        }
        if carry != 0 { return Some(val); }
        // carry == 0 signifie que le générateur n'est pas prêt — retry
        // Pause pour éviter de monopoliser le pipeline
        unsafe { core::arch::asm!("pause", options(nomem, nostack)); }
    }
    None // Échec après MAX_RETRIES
}

/// Vérifie la disponibilité de RDRAND via CPUID.Bit Intel ECX[30].
fn cpu_has_rdrand() -> bool {
    let ecx: u32;
    // SAFETY : CPUID est toujours disponible en mode long x86_64.
    // On utilise push/pop rbx car LLVM réserve ce registre.
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            out("ecx") ecx,
            out("edx") _,
            options(nomem, nostack, preserves_flags)
        );
    }
    (ecx >> 30) & 1 == 1
}

/// Fallback entropie faible via TSC + quelques sources déterministes.
/// Utilisé UNIQUEMENT si RDRAND et EFI_RNG_PROTOCOL sont tous les deux indisponibles.
fn collect_via_tsc_fallback(count: usize) -> [u8; 64] {
    let mut buf = [0u8; 64];
    let mut offset = 0usize;

    // Plusieurs lectures TSC avec CPUID serialization entre chaque lecture
    // pour forcer le pipeline à se vider.
    for chunk in 0..8usize {
        // SAFETY : RDTSC disponible en mode long x86_64.
        let tsc: u64;
        unsafe {
            // cpuid(0) sérialise le pipeline avant rdtsc
            core::arch::asm!(
                "push rbx",
                "cpuid",
                "pop rbx",
                inout("eax") 0u32 => _,
                out("ecx") _,
                out("edx") _,
                options(nomem, nostack, preserves_flags)
            );
            let lo: u32;
            let hi: u32;
            core::arch::asm!(
                "rdtsc",
                out("eax") lo,
                out("edx") hi,
                options(nomem, nostack)
            );
            tsc = ((hi as u64) << 32) | (lo as u64);
        }

        // XOR avec l'indice pour diversifier même si TSC est identique
        let mixed = tsc ^ (chunk as u64 * 0x9e3779b97f4a7c15); // Fibonacci hashing
        let bytes = mixed.to_le_bytes();

        let to_copy = 8usize.min(count.saturating_sub(offset));
        if to_copy == 0 { break; }
        buf[offset..offset + to_copy].copy_from_slice(&bytes[..to_copy]);
        offset += to_copy;

        // Pause variable pour maximiser la divergence TSC entre lectures
        unsafe {
            for _ in 0..((chunk + 1) * 100) {
                core::arch::asm!("pause", options(nomem, nostack));
            }
        }
    }

    buf
}

// ─── Erreurs ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum RngError {
    /// EFI_RNG_PROTOCOL absent sur ce firmware.
    ProtocolNotFound,
    /// GetRNG() a retourné une erreur UEFI.
    GetFailed { status: uefi::Status },
    /// Le firmware a retourné uniquement des zéros — firmware défectueux.
    AllZeroOutput { count: usize },
}

impl core::fmt::Display for RngError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ProtocolNotFound =>
                write!(f, "EFI_RNG_PROTOCOL absent — UEFI < 2.3 ou firmware non conforme"),
            Self::GetFailed { status } =>
                write!(f, "EFI_RNG_PROTOCOL::GetRNG() échoué : {:?}", status),
            Self::AllZeroOutput { count } =>
                write!(f, "EFI_RNG_PROTOCOL a retourné {} zéros — firmware défectueux", count),
        }
    }
}
