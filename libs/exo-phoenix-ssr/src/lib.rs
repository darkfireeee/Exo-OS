//! `exo-phoenix-ssr` — Shared Status Region (SSR) pour ExoPhoenix.
//!
//! La SSR est une région physique de 64 KiB à l'adresse `0x0100_0000`,
//! partagée entre Kernel A et Kernel B pour les handoffs ExoPhoenix.
//!
//! **CORR-02** : `SSR_MAX_CORES_LAYOUT = 256` (et non 64).
//!
//! # Layout simplifié
//! ```text
//! [0x0000] Magic/version   u64
//! [0x0008] handoff_flag    u64     ← Kernel B → A
//! [0x0040] cmd_b2a         [u8;64] ← ring IPI B→A
//! [0x0080] freeze_ack[]    u32×256 ← ACK isolation par cœur   (1 KiO)
//! [0x4000] pmc_snapshot[]  [u8;64]×256 ← snapshots PMC        (16 KiO)
//! [0x8000] log_audit       [u8;16384] ← journal audit          (16 KiO)
//! [0xC000] metrics         [u8;16384] ← métriques agrégées     (16 KiO)
//! [0x10000] --- fin SSR ---
//! ```

#![no_std]

use core::sync::atomic::{AtomicU32, Ordering};

// ─── Adresse + taille ─────────────────────────────────────────────────────────

/// Adresse physique de base de la SSR.
pub const SSR_BASE_PHYS: u64 = 0x0100_0000;

/// Taille de la SSR en octets (64 KiB).
pub const SSR_SIZE: usize = 0x1_0000;

// ─── Magic / version v7 ─────────────────────────────────────────────────────

/// Magic SSR ExoPhoenix (`EXPH` en ASCII).
pub const SSR_MAGIC: u32 = 0x4558_5048;

/// Version majeure du layout SSR. v7 est un changement incompatible avec v6.
pub const SSR_LAYOUT_MAJOR: u16 = 7;

/// Version mineure compatible du layout SSR.
pub const SSR_LAYOUT_MINOR: u16 = 0;

/// Mot de compatibilité stocké à `SSR_MAGIC_OFFSET`.
pub const SSR_MAGIC_VERSION: u64 =
    compose_magic_version(SSR_MAGIC, SSR_LAYOUT_MAJOR, SSR_LAYOUT_MINOR);

#[inline(always)]
pub const fn compose_magic_version(magic: u32, major: u16, minor: u16) -> u64 {
    ((magic as u64) << 32) | ((major as u64) << 16) | minor as u64
}

#[inline(always)]
pub const fn magic_from_magic_version(value: u64) -> u32 {
    (value >> 32) as u32
}

#[inline(always)]
pub const fn major_from_magic_version(value: u64) -> u16 {
    ((value >> 16) & 0xFFFF) as u16
}

#[inline(always)]
pub const fn minor_from_magic_version(value: u64) -> u16 {
    (value & 0xFFFF) as u16
}

/// Vérifie que le mot SSR lu au boot est compatible avec la lib compilée.
#[inline(always)]
pub const fn is_compatible_magic_version(value: u64) -> bool {
    magic_from_magic_version(value) == SSR_MAGIC
        && major_from_magic_version(value) == SSR_LAYOUT_MAJOR
        && minor_from_magic_version(value) <= SSR_LAYOUT_MINOR
}

// ─── Constantes de layout (CORR-02) ──────────────────────────────────────────

/// Nombre maximal de cœurs supportés dans le layout SSR.
/// **CORR-02** : doit être 256, pas 64.
pub const SSR_MAX_CORES_LAYOUT: usize = 256;

/// APIC ID du cœur de contrôle Kernel B (ExoPhoenix).
pub const KERNEL_B_APIC_ID: u32 = 0;

/// Taille d'un snapshot PMC par cœur (octets).
pub const SSR_PMC_SNAPSHOT_SIZE: usize = 64;
/// Taille de la zone journal audit.
pub const SSR_LOG_AUDIT_SIZE: usize = 16 * 1024;
/// Taille de la zone métriques.
pub const SSR_METRICS_SIZE: usize = 16 * 1024;

/// Offset physique contractuel dans l'image Kernel A pour le miroir de liveness.
///
/// Kernel A doit écrire le nonce lu dans `SSR_LIVENESS_NONCE` à
/// `KERNEL_LOAD_PHYS_ADDR + A_LIVENESS_MIRROR_OFFSET`.
pub const A_LIVENESS_MIRROR_OFFSET: u64 = 0x0100;

// ─── États du handoff ────────────────────────────────────────────────────────

pub const HANDOFF_NORMAL: u64 = 0;
pub const HANDOFF_FREEZE_REQ: u64 = 1;
pub const HANDOFF_FREEZE_ACK_ALL: u64 = 2;
pub const HANDOFF_B_ACTIVE: u64 = 3;

// ─── Offsets SSR ─────────────────────────────────────────────────────────────

/// `[0x0000]` Magic / version SSR (u64).
pub const SSR_MAGIC_OFFSET: usize = 0x0000;
/// `[0x0008]` Handoff flag Kernel B → A (u64 atomique).
pub const SSR_HANDOFF_FLAG_OFFSET: usize = 0x0008;
/// `[0x0010]` Nonce de liveness Kernel B → A (u64 atomique).
pub const SSR_LIVENESS_NONCE_OFFSET: usize = 0x0010;
/// `[0x0018]` Seqlock SSR partagé entre noyaux (u64 atomique).
pub const SSR_SEQLOCK_OFFSET: usize = 0x0018;
/// `[0x0040]` Ring de commandes B → A (64 octets).
pub const SSR_CMD_B2A_OFFSET: usize = 0x0040;
/// `[0x0080]` Freeze ACK par cœur — u32 × 256 = 1 KiO.
pub const SSR_FREEZE_ACK_OFFSET: usize = 0x0080;
/// `[0x4000]` Snapshots PMC par cœur — 64 B × 256 = 16 KiO.
pub const SSR_PMC_OFFSET: usize = 0x4000;
/// `[0x8000]` Journal d'audit (16 KiO).
pub const SSR_LOG_AUDIT_OFFSET: usize = 0x8000;
/// `[0xC000]` Métriques agrégées (16 KiO).
pub const SSR_METRICS_OFFSET: usize = 0xC000;

// ─── Assertions statiques (vérifiées à la compilation) ───────────────────────

const _: () = assert!(
    SSR_SIZE == 0x1_0000,
    "SSR: taille doit être 64 KiO (0x10000)"
);

const _: () = assert!(
    SSR_FREEZE_ACK_OFFSET + SSR_MAX_CORES_LAYOUT * 4 <= SSR_PMC_OFFSET,
    "SSR: zone freeze_ack dépasse la zone PMC"
);

const _: () = assert!(
    SSR_PMC_OFFSET + SSR_MAX_CORES_LAYOUT * SSR_PMC_SNAPSHOT_SIZE <= SSR_LOG_AUDIT_OFFSET,
    "SSR: zone PMC dépasse la zone log_audit"
);

const _: () = assert!(
    SSR_LOG_AUDIT_OFFSET + SSR_LOG_AUDIT_SIZE <= SSR_METRICS_OFFSET,
    "SSR: zone log_audit dépasse la zone métriques"
);

const _: () = assert!(
    SSR_METRICS_OFFSET + SSR_METRICS_SIZE <= SSR_SIZE,
    "SSR: zone métriques doit être dans la SSR"
);

// ─── Nombre de cœurs actifs au runtime ───────────────────────────────────────

/// Nombre de cœurs logiques actifs au runtime (initialisé par stage0).
/// Doit toujours être `≤ SSR_MAX_CORES_LAYOUT`.
pub static MAX_CORES_RUNTIME: AtomicU32 = AtomicU32::new(1);

/// Lit le nombre de cœurs actifs (Relaxed — stable après init).
#[inline(always)]
pub fn active_cores() -> u32 {
    MAX_CORES_RUNTIME.load(Ordering::Relaxed)
}

/// Initialise le nombre de cœurs (appelé UNE SEULE FOIS par stage0).
#[inline(always)]
pub fn init_core_count(n: u32) {
    debug_assert!(
        n as usize <= SSR_MAX_CORES_LAYOUT,
        "nombre de cœurs dépasse SSR_MAX_CORES_LAYOUT"
    );
    MAX_CORES_RUNTIME.store(n, Ordering::Release);
}

// ─── Calcul d'offsets par cœur ───────────────────────────────────────────────

/// Calcule l'offset SSR du freeze ACK pour le cœur `apic_id`.
///
/// `freeze_ack[apic_id]` est un `u32` à l'adresse
/// `SSR_BASE_PHYS + freeze_ack_offset(apic_id)`.
#[inline(always)]
pub const fn freeze_ack_offset(apic_id: u32) -> usize {
    SSR_FREEZE_ACK_OFFSET + apic_id as usize * 4
}

/// Calcule l'offset SSR du snapshot PMC pour le cœur `apic_id`.
///
/// Chaque snapshot fait `SSR_PMC_SNAPSHOT_SIZE` = 64 octets.
#[inline(always)]
pub const fn pmc_snapshot_offset(apic_id: u32) -> usize {
    SSR_PMC_OFFSET + apic_id as usize * SSR_PMC_SNAPSHOT_SIZE
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freeze_ack_bounds() {
        assert_eq!(freeze_ack_offset(0), SSR_FREEZE_ACK_OFFSET);
        let last = freeze_ack_offset(SSR_MAX_CORES_LAYOUT as u32 - 1);
        assert!(
            last + 4 <= SSR_PMC_OFFSET,
            "dernier freeze_ack dépasse la zone PMC"
        );
    }

    #[test]
    fn pmc_snapshot_bounds() {
        assert_eq!(pmc_snapshot_offset(0), SSR_PMC_OFFSET);
        let last = pmc_snapshot_offset(SSR_MAX_CORES_LAYOUT as u32 - 1);
        assert!(
            last + SSR_PMC_SNAPSHOT_SIZE <= SSR_LOG_AUDIT_OFFSET,
            "dernier snapshot PMC dépasse la zone log_audit"
        );
    }

    #[test]
    fn layout_fits_in_ssr() {
        assert!(
            SSR_METRICS_OFFSET + SSR_METRICS_SIZE <= SSR_SIZE,
            "zone métriques + fin dépasse SSR_SIZE"
        );
    }

    #[test]
    fn magic_version_roundtrip() {
        assert_eq!(magic_from_magic_version(SSR_MAGIC_VERSION), SSR_MAGIC);
        assert_eq!(
            major_from_magic_version(SSR_MAGIC_VERSION),
            SSR_LAYOUT_MAJOR
        );
        assert_eq!(
            minor_from_magic_version(SSR_MAGIC_VERSION),
            SSR_LAYOUT_MINOR
        );
        assert!(is_compatible_magic_version(SSR_MAGIC_VERSION));
        assert!(!is_compatible_magic_version(compose_magic_version(
            SSR_MAGIC,
            6,
            0
        )));
    }
}
