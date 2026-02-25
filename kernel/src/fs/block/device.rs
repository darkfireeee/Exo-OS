// kernel/src/fs/block/device.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// BLOCK DEVICE — Abstraction des périphériques bloc (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// `BlockDevice` trait : interface unifiée pour tout périphérique de stockage
// (NVMe, AHCI, VirtioBlk…).
//
// Architecture :
//   • `BlockDevice` trait avec submit_bio, flush, discard, get_info.
//   • `BlockDevInfo` : capacité, taille de bloc, limites I/O.
//   • `BlockDevRegistry` : table globale des block devices enregistrés
//     (indexed par majeur/mineur).
//   • Les drivers du dossier `drivers/storage/` implémentent `BlockDevice`.
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::core::types::{DevId, FsError, FsResult};
use crate::fs::block::bio::Bio;
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// BlockDevInfo
// ─────────────────────────────────────────────────────────────────────────────

/// Informations statiques d'un block device.
#[derive(Clone, Debug)]
pub struct BlockDevInfo {
    /// Taille de secteur logique (en général 512 ou 4096).
    pub logical_block_size:  u32,
    /// Taille de secteur physique.
    pub physical_block_size: u32,
    /// Nombre de secteurs logiques.
    pub sector_count:        u64,
    /// Taille maximale d'une requête (en bytes).
    pub max_request_size:    u64,
    /// Alignement minimum optimal (ex: raid stripe, erase block).
    pub optimal_io_align:    u32,
    /// Supporte DISCARD (TRIM/UNMAP).
    pub supports_discard:    bool,
    /// Supporte FUA (Force Unit Access).
    pub supports_fua:        bool,
    /// Read-only.
    pub read_only:           bool,
    /// Nom du device (ex: "nvme0n1").
    pub name:                [u8; 32],
}

impl BlockDevInfo {
    pub fn sector_size(&self) -> u64 {
        self.logical_block_size as u64
    }
    pub fn total_bytes(&self) -> u64 {
        self.sector_count * self.sector_size()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BlockDevice trait
// ─────────────────────────────────────────────────────────────────────────────

/// Interface unifiée pour un périphérique de stockage bloc.
pub trait BlockDevice: Send + Sync {
    /// DevId (majeur × 256 + mineur).
    fn dev_id(&self) -> DevId;

    /// Informations statiques du device.
    fn info(&self) -> &BlockDevInfo;

    /// Soumet une Bio. Peut être synchrone ou enregistrer un callback.
    fn submit_bio(&self, bio: Bio) -> FsResult<()>;

    /// Flush du cache d'écriture interne.
    fn flush(&self) -> FsResult<()>;

    /// DISCARD (TRIM) une plage de secteurs.
    fn discard(&self, sector: u64, num_sectors: u64) -> FsResult<()> {
        let _ = (sector, num_sectors);
        Err(FsError::NotSupported)
    }

    /// Statistiques cumulées du device.
    fn stats(&self) -> BlockDevStats;
}

// ─────────────────────────────────────────────────────────────────────────────
// BlockDevStats
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default)]
pub struct BlockDevStats {
    pub reads_completed:  u64,
    pub writes_completed: u64,
    pub read_bytes:       u64,
    pub write_bytes:      u64,
    pub io_errors:        u64,
    pub read_ticks_ns:    u64,
    pub write_ticks_ns:   u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// BlockDevRegistry
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée dans la registry.
struct DevEntry {
    dev_id: DevId,
    dev:    Arc<dyn BlockDevice>,
}

pub struct BlockDevRegistry {
    devices: SpinLock<Vec<DevEntry>>,
}

impl BlockDevRegistry {
    pub const fn new() -> Self {
        Self { devices: SpinLock::new(Vec::new()) }
    }

    /// Enregistre un device.
    pub fn register(&self, dev: Arc<dyn BlockDevice>) -> FsResult<()> {
        let id = dev.dev_id();
        let mut devices = self.devices.lock();
        if devices.iter().any(|e| e.dev_id == id) {
            return Err(FsError::Exists);
        }
        devices.push(DevEntry { dev_id: id, dev });
        DEV_STATS.registered.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Désenregistre un device.
    pub fn unregister(&self, dev_id: DevId) {
        let mut devices = self.devices.lock();
        let before = devices.len();
        devices.retain(|e| e.dev_id != dev_id);
        if devices.len() < before {
            DEV_STATS.registered.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Recherche un device par DevId.
    pub fn get(&self, dev_id: DevId) -> Option<Arc<dyn BlockDevice>> {
        let devices = self.devices.lock();
        devices.iter()
            .find(|e| e.dev_id == dev_id)
            .map(|e| e.dev.clone())
    }

    pub fn count(&self) -> usize {
        self.devices.lock().len()
    }
}

pub static BLOCK_DEV_REGISTRY: BlockDevRegistry = BlockDevRegistry::new();

// ─────────────────────────────────────────────────────────────────────────────
// DevRegistryStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct DevRegistryStats {
    pub registered: AtomicU32,
    pub lookups:    AtomicU64,
    pub not_found:  AtomicU64,
}

impl DevRegistryStats {
    pub const fn new() -> Self {
        Self {
            registered: AtomicU32::new(0),
            lookups:    AtomicU64::new(0),
            not_found:  AtomicU64::new(0),
        }
    }
}

pub static DEV_STATS: DevRegistryStats = DevRegistryStats::new();
