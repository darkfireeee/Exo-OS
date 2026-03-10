// kernel/src/memory/dma/iommu/domain.rs
//
// Domaines IOMMU — isolation entre périphériques et espaces d'adressage.
// Un domaine regroupe un ensemble de périphériques partageant le même
// espace IOVA et les mêmes tables de pages IOMMU.
//
// COUCHE 0 — aucune dépendance externe.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicBool, Ordering};
use spin::Mutex;

use crate::memory::core::types::PhysAddr;
use crate::memory::dma::core::types::{IommuDomainId, IovaAddr, DmaError};

// ─────────────────────────────────────────────────────────────────────────────
// CONSTANTES
// ─────────────────────────────────────────────────────────────────────────────

pub const MAX_DOMAINS: usize = 256;
pub const MAX_DEVICES_PER_DOMAIN: usize = 64;
/// ID du domaine identité (passthrough — pas de translation).
pub const IDENTITY_DOMAIN_ID: IommuDomainId = IommuDomainId(0);

// ─────────────────────────────────────────────────────────────────────────────
// TYPES DE DOMAINE
// ─────────────────────────────────────────────────────────────────────────────

/// Type de domaine IOMMU.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(u8)]
pub enum DomainType {
    /// Identité : IOVA == PhysAddr (passthrough total).
    Identity    = 0,
    /// Traduction complète via tables de pages IOMMU.
    Translated  = 1,
    /// Traduction partielle (hôte + passthrough sélectif).
    Hybrid      = 2,
    /// Domaine de blocage : tous les accès DMA sont refusés.
    Blocked     = 3,
}

// ─────────────────────────────────────────────────────────────────────────────
// IDENTIFIANT DE PÉRIPHÉRIQUE (BDF)
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant PCI d'un périphérique (Bus:Device:Function).
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(C)]
pub struct PciBdf {
    pub bus:  u8,
    pub dev:  u8,
    pub func: u8,
    _pad: u8,
}

impl PciBdf {
    pub const fn new(bus: u8, dev: u8, func: u8) -> Self {
        PciBdf { bus, dev, func, _pad: 0 }
    }
    /// Encodes BDF as a 16-bit value (bus<<8 | dev<<3 | func).
    pub const fn as_u16(self) -> u16 {
        ((self.bus as u16) << 8) | ((self.dev as u16) << 3) | (self.func as u16)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DOMAINE IOMMU
// ─────────────────────────────────────────────────────────────────────────────

/// Statistiques d'un domaine IOMMU.
pub struct DomainStats {
    pub mappings_created: AtomicU64,
    pub mappings_freed:   AtomicU64,
    pub page_faults:      AtomicU64,
    pub tlb_invalidations: AtomicU64,
}

impl DomainStats {
    const fn new() -> Self {
        DomainStats {
            mappings_created:  AtomicU64::new(0),
            mappings_freed:    AtomicU64::new(0),
            page_faults:       AtomicU64::new(0),
            tlb_invalidations: AtomicU64::new(0),
        }
    }
}

/// Un domaine IOMMU.
#[repr(C, align(64))]
pub struct IommuDomain {
    /// Identifiant unique.
    pub id:           IommuDomainId,
    /// Type de domaine.
    pub domain_type:  DomainType,
    /// Adresse physique de la table de pages IOMMU (PML4 ou équivalent).
    pub page_table:   AtomicU64,
    /// Actif ?
    pub active:       AtomicBool,
    /// Nombre de périphériques attachés.
    pub device_count: AtomicU32,
    /// Périphériques BDF attachés à ce domaine.
    devices: Mutex<[Option<PciBdf>; MAX_DEVICES_PER_DOMAIN]>,
    /// Statistiques.
    pub stats: DomainStats,
    /// Compteur d'allocations IOVA dans ce domaine.
    pub iova_counter: AtomicU64,
    /// Base IOVA de ce domaine.
    pub iova_base: u64,
    /// Taille max de l'espace IOVA.
    pub iova_limit: u64,
    _pad: [u8; 4],
}

impl IommuDomain {
    const fn new_identity() -> Self {
        IommuDomain {
            id:           IDENTITY_DOMAIN_ID,
            domain_type:  DomainType::Identity,
            page_table:   AtomicU64::new(0),
            active:       AtomicBool::new(true),
            device_count: AtomicU32::new(0),
            devices:      Mutex::new([None; MAX_DEVICES_PER_DOMAIN]),
            stats:        DomainStats::new(),
            iova_counter: AtomicU64::new(0),
            iova_base:    0,
            iova_limit:   u64::MAX,
            _pad:         [0u8; 4],
        }
    }

    pub fn new(id: IommuDomainId, domain_type: DomainType, iova_base: u64, iova_limit: u64) -> Self {
        IommuDomain {
            id,
            domain_type,
            page_table:   AtomicU64::new(0),
            active:       AtomicBool::new(false),
            device_count: AtomicU32::new(0),
            devices:      Mutex::new([None; MAX_DEVICES_PER_DOMAIN]),
            stats:        DomainStats::new(),
            iova_counter: AtomicU64::new(iova_base),
            iova_base,
            iova_limit,
            _pad:         [0u8; 4],
        }
    }

    /// Attache un périphérique à ce domaine.
    pub fn attach_device(&self, bdf: PciBdf) -> Result<(), DmaError> {
        let mut devs = self.devices.lock();
        if let Some(slot) = devs.iter_mut().find(|s| s.is_none()) {
            *slot = Some(bdf);
            self.device_count.fetch_add(1, Ordering::Relaxed);
            Ok(())
        } else {
            Err(DmaError::OutOfMemory)
        }
    }

    /// Détache un périphérique.
    pub fn detach_device(&self, bdf: PciBdf) {
        let mut devs = self.devices.lock();
        for slot in devs.iter_mut() {
            if *slot == Some(bdf) {
                *slot = None;
                self.device_count.fetch_sub(1, Ordering::Relaxed);
                return;
            }
        }
    }

    /// Alloue une IOVA dans ce domaine (bump + alignement sur PAGE_SIZE).
    pub fn alloc_iova(&self, size: usize) -> Option<IovaAddr> {
        use crate::memory::core::constants::PAGE_SIZE;
        let size_aligned = ((size + PAGE_SIZE - 1) / PAGE_SIZE) * PAGE_SIZE;
        let old = self.iova_counter.load(Ordering::Relaxed);
        let new = old + size_aligned as u64;
        if new > self.iova_limit { return None; }
        // CAS pour atomicité.
        match self.iova_counter.compare_exchange(old, new, Ordering::AcqRel, Ordering::Relaxed) {
            Ok(_) => Some(IovaAddr::new(old)),
            Err(_) => {
                // Réessai simple — en cas de contention on incrémente directement.
                let base = self.iova_counter.fetch_add(size_aligned as u64, Ordering::Relaxed);
                if base + size_aligned as u64 > self.iova_limit { return None; }
                Some(IovaAddr::new(base))
            }
        }
    }

    /// Retourne l'adresse physique de la table de pages IOMMU.
    pub fn page_table_phys(&self) -> Option<PhysAddr> {
        let p = self.page_table.load(Ordering::Acquire);
        if p == 0 { None } else { Some(PhysAddr::new(p)) }
    }

    /// Définit la table de pages (unsafe — doit être appelé depuis init IOMMU).
    ///
    /// # Safety
    /// `phys` doit pointer une table de pages IOMMU valide et alignée sur PAGE_SIZE.
    pub unsafe fn set_page_table(&self, phys: PhysAddr) {
        self.page_table.store(phys.as_u64(), Ordering::Release);
    }

    pub fn activate(&self) { self.active.store(true, Ordering::Release); }
    pub fn deactivate(&self) { self.active.store(false, Ordering::Release); }
    pub fn is_active(&self) -> bool { self.active.load(Ordering::Acquire) }
}

// ─────────────────────────────────────────────────────────────────────────────
// TABLE GLOBALE DES DOMAINES
// ─────────────────────────────────────────────────────────────────────────────

pub struct IommuDomainTable {
    domains: Mutex<IommuDomainTableInner>,
    count:   AtomicU32,
}

struct IommuDomainTableInner {
    slots:   [Option<IommuDomain>; MAX_DOMAINS],
    next_id: u32,
}

// SAFETY: IommuDomainTable est protégé par Mutex.
unsafe impl Sync for IommuDomainTable {}
unsafe impl Send for IommuDomainTable {}

impl IommuDomainTable {
    const fn new() -> Self {
        IommuDomainTable {
            domains: Mutex::new(IommuDomainTableInner {
                // SAFETY: Option<IommuDomain> = None au niveau bits zéros.
                slots:   unsafe { core::mem::MaybeUninit::zeroed().assume_init() },
                next_id: 1, // 0 = domaine identité réservé
            }),
            count: AtomicU32::new(0),
        }
    }

    /// Initialise le domaine identité.
    pub fn init(&self) {
        let mut inner = self.domains.lock();
        inner.slots[0] = Some(IommuDomain::new_identity());
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    /// Crée un nouveau domaine traduit.
    pub fn create_domain(
        &self,
        domain_type: DomainType,
        iova_base: u64,
        iova_limit: u64,
    ) -> Result<IommuDomainId, DmaError> {
        let mut inner = self.domains.lock();
        if self.count.load(Ordering::Relaxed) >= MAX_DOMAINS as u32 {
            return Err(DmaError::OutOfMemory);
        }
        let id = IommuDomainId(inner.next_id);
        inner.next_id += 1;

        let slot = inner.slots.iter_mut().find(|s| s.is_none())
            .ok_or(DmaError::OutOfMemory)?;
        *slot = Some(IommuDomain::new(id, domain_type, iova_base, iova_limit));
        self.count.fetch_add(1, Ordering::Relaxed);
        Ok(id)
    }

    /// Accès en lecture seule à un domaine.
    pub fn with_domain<T, F: FnOnce(&IommuDomain) -> T>(
        &self, id: IommuDomainId, f: F
    ) -> Option<T> {
        let inner = self.domains.lock();
        for slot in inner.slots.iter() {
            if let Some(ref dom) = slot {
                if dom.id == id { return Some(f(dom)); }
            }
        }
        None
    }

    /// Accès mutable à un domaine (pour initialisation).
    pub fn with_domain_mut<T, F: FnOnce(&mut IommuDomain) -> T>(
        &self, id: IommuDomainId, f: F
    ) -> Option<T> {
        let mut inner = self.domains.lock();
        for slot in inner.slots.iter_mut() {
            if let Some(ref mut dom) = slot {
                if dom.id == id { return Some(f(dom)); }
            }
        }
        None
    }

    pub fn domain_count(&self) -> u32 { self.count.load(Ordering::Relaxed) }
}

pub static IOMMU_DOMAINS: IommuDomainTable = IommuDomainTable::new();
