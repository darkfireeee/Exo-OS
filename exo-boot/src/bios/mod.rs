//! bios/ — Chemin BIOS legacy pour exo-boot.
//!
//! Ce module gère le démarrage sur machines sans UEFI (legacy BIOS, VMs legacy).
//!
//! Architecture du démarrage BIOS (voir aussi linker/bios.ld) :
//!
//!   [BIOS]
//!     └─→ MBR @ 0x7C00 (mbr.asm, 512 bytes, 16-bit real mode)
//!           └─→ Stage 2 @ 0x1000 (stage2.asm, 32-bit → 64-bit)
//!                 └─→ exoboot_main_bios() dans Rust (main.rs)
//!
//! Modules :
//!   - `vga`  : Sortie texte VGA 80×25 (debug, affichage progression)
//!   - `disk` : Lecture disque via BIOS INT 13h ext. (LBA 48-bit)
//!   - `mbr`  : MBR 512 bytes (stage 1, assembleur pur)
//!   - `stage2`: Stage 2 transitions real→protected→long mode

pub mod disk;
pub mod vga;

// MBR et Stage2 sont des fichiers assembleur (mbr.asm, stage2.asm)
// intégrés à la build via le processus de compilation assembleur.
// Ils ne sont pas des modules Rust mais leur présence est documentée ici.

// ─── Re-export des types fréquemment utilisés ─────────────────────────────────
pub use vga::{Color, VgaWriter};
pub use disk::BiosDisk;

// ─── Entropie BIOS ────────────────────────────────────────────────────────────

/// Collecte 64 octets d'entropie en mode BIOS.
///
/// Ordre de préférence :
///   1. RDSEED (entropie hardware, disponible Haswell+/Excavator+)
///   2. RDRAND (PRNG hardware, disponible Sandy Bridge+/Bulldozer+)
///   3. TSC + diversification heuristique (fallback faible)
///
/// AVERTISSEMENT : Le fallback TSC produit une entropie INSUFFISANTE
/// pour KASLR efficace sur des adversaires contrôlant le timing de démarrage.
/// Il est utilisé uniquement en dernier recours.
pub fn collect_entropy_bios() -> [u8; 64] {
    // Essaie RDSEED en premier (meilleure source d'entropie hardware)
    if let Some(buf) = collect_via_rdseed(64) {
        return buf;
    }
    // Fallback RDRAND
    if let Some(buf) = collect_via_rdrand_bios(64) {
        return buf;
    }
    // Dernier recours : TSC
    collect_via_tsc_bios(64)
}

/// Collecte l'entropie via RDSEED (True Random Number Generator x86).
///
/// RDSEED extrait directement depuis la source d'entropie hardware
/// (contrairement à RDRAND qui est un CSPRNG seedé depuis la source HW).
///
/// La disponibilité de RDSEED est vérifiée via CPUID.
fn collect_via_rdseed(count: usize) -> Option<[u8; 64]> {
    if !cpu_has_rdseed() { return None; }

    let mut buf = [0u8; 64];
    let mut i = 0usize;

    while i < count {
        let to_fill = (count - i).min(8);
        match rdseed_u64() {
            Some(val) => {
                buf[i..i + to_fill].copy_from_slice(&val.to_le_bytes()[..to_fill]);
                i += to_fill;
            }
            None => return None,
        }
    }
    Some(buf)
}

fn rdseed_u64() -> Option<u64> {
    // RDSEED peut être "busy" — on réessaie jusqu'à 512 fois selon la doc Intel
    for _ in 0..512 {
        let (val, carry): (u64, u8);
        // SAFETY : RDSEED disponible si cpu_has_rdseed() == true.
        unsafe {
            core::arch::asm!(
                "rdseed {val}",
                "setc {carry}",
                val   = out(reg) val,
                carry = out(reg_byte) carry,
                options(nomem, nostack)
            );
        }
        if carry != 0 { return Some(val); }
        unsafe { core::arch::asm!("pause", options(nomem, nostack)); }
    }
    None
}

fn cpu_has_rdseed() -> bool {
    // CPUID Leaf 7, Sub-leaf 0, EBX[18] = RDSEED
    let ebx: u32;
    // SAFETY : CPUID est toujours sûr en mode long x86_64.
    // On doit passer par push/pop rbx car LLVM réserve ce registre.
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {ebx:e}, ebx",
            "pop rbx",
            inout("eax") 7u32 => _,
            inout("ecx") 0u32 => _,
            ebx = out(reg) ebx,
            out("edx") _,
            options(nomem, nostack, preserves_flags)
        );
    }
    (ebx >> 18) & 1 == 1
}

fn collect_via_rdrand_bios(count: usize) -> Option<[u8; 64]> {
    // Vérifie RDRAND via CPUID ECX[30]
    let ecx: u32;
    // SAFETY : CPUID sûr.
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
    if (ecx >> 30) & 1 == 0 { return None; }

    let mut buf = [0u8; 64];
    let mut i = 0usize;
    while i < count {
        let (mut val, mut carry): (u64, u8) = (0, 0);
        let mut ok = false;
        for _ in 0..10 {
            // SAFETY : RDRAND disponible si CPUID ECX[30] = 1.
            unsafe {
                core::arch::asm!(
                    "rdrand {0}",
                    "setc {1}",
                    out(reg) val,
                    out(reg_byte) carry,
                    options(nomem, nostack)
                );
            }
            if carry != 0 { ok = true; break; }
            unsafe { core::arch::asm!("pause", options(nomem, nostack)); }
        }
        if !ok { return None; }
        let to_fill = (count - i).min(8);
        buf[i..i + to_fill].copy_from_slice(&val.to_le_bytes()[..to_fill]);
        i += to_fill;
    }
    Some(buf)
}

fn collect_via_tsc_bios(count: usize) -> [u8; 64] {
    let mut buf = [0u8; 64];
    let mut offset = 0usize;
    for chunk_idx in 0..8usize {
        let tsc: u64;
        // SAFETY : RDTSC disponible en mode long x86_64.
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
        let mixed = tsc.wrapping_mul(0x6c62272e07bb0142).wrapping_add(0x62b821756295c58d);
        let bytes = mixed.to_le_bytes();
        let to_copy = 8usize.min(count.saturating_sub(offset));
        if to_copy == 0 { break; }
        buf[offset..offset + to_copy].copy_from_slice(&bytes[..to_copy]);
        offset += to_copy;
        for _ in 0..(chunk_idx + 1) * 200 {
            // SAFETY : pause sûre.
            unsafe { core::arch::asm!("pause", options(nomem, nostack)); }
        }
    }
    buf
}
