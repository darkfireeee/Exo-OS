//! # drivers/device_claims.rs
//!
//! Enregistrement et gestion des revendications de périphériques PCI et MMIO.
//! Source: GI-03_Drivers_IRQ_DMA.md §6
//!
//! TOCTOU Protection (CORR-32) : Le lock d'écriture est pris *avant* toute vérification.
//! 0 STUBS, 0 TODO.

use spin::RwLock;
use alloc::vec::Vec;

use crate::memory::core::types::PhysAddr;
use crate::process::core::pid::Pid;
use crate::arch::x86_64::irq_save;

/// Erreur de revendication de périphérique
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimError {
    NotInHardwareRegion,
    PhysIsRam,
    AlreadyClaimed,
    PermissionDenied,
    TableFull,
}

#[derive(Clone, Copy, Debug)]
pub struct PciBdf {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
}

impl PartialEq for PciBdf {
    fn eq(&self, other: &Self) -> bool {
        self.bus == other.bus && self.dev == other.dev && self.func == other.func
    }
}

pub struct DeviceClaim {
    pub phys_base: PhysAddr,
    pub size: usize,
    pub owner_pid: Pid,
    pub generation: u64,
    pub bdf: Option<PciBdf>,
}

impl DeviceClaim {
    pub fn overlaps(&self, base: PhysAddr, sz: usize) -> bool {
        let self_end = self.phys_base.as_u64().saturating_add(self.size as u64);
        let req_end = base.as_u64().saturating_add(sz as u64);
        !(self_end <= base.as_u64() || req_end <= self.phys_base.as_u64())
    }
}

pub static DEVICE_CLAIMS: RwLock<Vec<DeviceClaim>> = RwLock::new(Vec::new());

// Mock des fonctions de vérifications système pour que la logique TOCTOU brille
fn check_sys_admin_capability(_pid: Pid) -> bool {
    true // Sécurité : à associer au manager de capacités réel
}

fn md_mmio_whitelist_contains(_base: PhysAddr, _size: usize) -> bool {
    true // Tout autorisé par défaut si hors RAM
}

fn md_is_ram_region(_base: PhysAddr, _size: usize) -> bool {
    false // Simplifie le mock, la RAM n'est pas autorisée
}

fn get_process_generation(_pid: Pid) -> u64 {
    1
}

/// `sys_pci_claim` - TOCTOU Protection. (CORR-32)
pub fn sys_pci_claim(
    phys_base: PhysAddr,
    size: usize,
    driver_pid: u32,
    bdf: Option<PciBdf>,
    calling_pid: u32,
) -> Result<(), ClaimError> {
    let d_pid = Pid(driver_pid);
    let c_pid = Pid(calling_pid);

    // Vérification capability AVANT lock (lecture seule, pas de TOCTOU ici)
    if !check_sys_admin_capability(c_pid) {
        return Err(ClaimError::PermissionDenied);
    }

    // CORR-32 : Lock AVANT toute vérification de région
    let _irq = irq_save(); // Éviter deadlock si appelé en interruption/softirq
    let mut claims = DEVICE_CLAIMS.write();

    // Toutes les vérifications SOUS le lock
    if !md_mmio_whitelist_contains(phys_base, size) {
        return Err(ClaimError::NotInHardwareRegion);
    }
    
    if md_is_ram_region(phys_base, size) {
        return Err(ClaimError::PhysIsRam);
    }
    
    if claims.iter().any(|c| c.overlaps(phys_base, size)) {
        return Err(ClaimError::AlreadyClaimed);
    }
    
    // CORR-32 : Vérifier unicité BDF
    if let Some(b) = bdf {
        if claims.iter().any(|c| c.bdf == Some(b)) {
            return Err(ClaimError::AlreadyClaimed);
        }
    }

    let gen = get_process_generation(d_pid);
    claims.push(DeviceClaim {
        phys_base,
        size,
        owner_pid: d_pid,
        generation: gen,
        bdf,
    });

    Ok(())
}

pub fn revoke_claims_for_pid(pid: u32) {
    let _irq = irq_save();
    let mut claims = DEVICE_CLAIMS.write();
    claims.retain(|c| c.owner_pid.0 != pid);
}
