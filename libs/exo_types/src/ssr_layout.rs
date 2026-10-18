//! ssr_layout.rs — Shared Status Region (SSR) Layout pour ExoPhoenix (#![no_std])
//!
//! Spécification: docs/Vision v0.2.0/SPEC-EXOPHOENIX-STRATA.md
//!
//! La SSR est une région physique de 64 KiB à l'adresse `0x0100_0000`,
//! partagée entre Kernel A et Kernel B pour les handoffs ExoPhoenix.
//!
//! **CORR-02** : `SSR_MAX_CORES_LAYOUT = 256` (support jusqu'à 256 cœurs).

/// Adresse physique de base de la SSR (0x0100_0000)
pub const SSR_BASE_PHYS: u64 = 0x0100_0000;

/// Taille de la SSR en octets (64 KiB)
pub const SSR_SIZE: usize = 0x1_0000;

/// Magic SSR ExoPhoenix (`EXPH` en ASCII)
pub const SSR_MAGIC: u32 = 0x4558_5048;

/// Version majeure du layout SSR (v7)
pub const SSR_LAYOUT_MAJOR: u16 = 7;

/// Version mineure compatible du layout SSR
pub const SSR_LAYOUT_MINOR: u16 = 0;

/// Nombre maximal de cœurs supportés dans le layout SSR (CORR-02)
pub const SSR_MAX_CORES_LAYOUT: usize = 256;

/// APIC ID du cœur de contrôle Kernel B (ExoPhoenix)
pub const KERNEL_B_APIC_ID: u32 = 0;

/// Taille d'un snapshot PMC par cœur (octets)
pub const SSR_PMC_SNAPSHOT_SIZE: usize = 64;
/// Taille de la zone journal audit (16 KiB)
pub const SSR_LOG_AUDIT_SIZE: usize = 16 * 1024;
/// Taille de la zone métriques (16 KiB)
pub const SSR_METRICS_SIZE: usize = 16 * 1024;

/// État normal d'exécution du kernel
pub const HANDOFF_NORMAL: u64 = 0;
/// Requête de gel (freeze) émise par Kernel B
pub const HANDOFF_FREEZE_REQ: u64 = 1;
/// Acquittement du gel complet par tous les cœurs
pub const HANDOFF_FREEZE_ACK_ALL: u64 = 2;
/// Kernel B actif et aux commandes
pub const HANDOFF_B_ACTIVE: u64 = 3;

/// Mot de compatibilité combiné (magic 32b, major 16b, minor 16b)
#[inline(always)]
pub const fn compose_magic_version(magic: u32, major: u16, minor: u16) -> u64 {
    ((magic as u64) << 32) | ((major as u64) << 16) | (minor as u64)
}

/// En-tête de contrôle du handoff SSR
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SsrHeader {
    /// Mot combiné magic et versions
    pub magic_version: u64,
    /// Drapeau d'état du handoff
    pub handoff_flag: u64,
    /// Numéro d'époque de bascule
    pub epoch_generation: u64,
    /// Nonce de vivacité (liveness)
    pub liveness_nonce: u64,
    /// Identifiant du kernel actif (0 = A, 1 = B)
    pub active_kernel: u32,
    /// Réservé pour alignement
    pub reserved: u32,
    /// Anneau de commandes IPI inter-kernels B vers A
    pub cmd_ring_b2a: [u8; 64],
}

impl SsrHeader {
    /// Initialise un en-tête SSR avec les valeurs par défaut
    pub const fn empty() -> Self {
        Self {
            magic_version: compose_magic_version(SSR_MAGIC, SSR_LAYOUT_MAJOR, SSR_LAYOUT_MINOR),
            handoff_flag: HANDOFF_NORMAL,
            epoch_generation: 0,
            liveness_nonce: 0,
            active_kernel: 0,
            reserved: 0,
            cmd_ring_b2a: [0u8; 64],
        }
    }
}
