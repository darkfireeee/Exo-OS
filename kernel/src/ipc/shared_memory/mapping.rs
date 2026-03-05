// ipc/shared_memory/mapping.rs — Mappage des régions SHM dans les espaces d'adressage
//
// Ce module gère l'association entre une région SHM (ShmDescriptor) et un
// espace d'adressage virtuel d'un processus.
//
// Dans Exo-OS, le memory manager gère les tables de pages. Ce module est une
// couche IPC qui :
//   1. Enregistre les mappings actifs (quelle région → quel processus)
//   2. Valide les permissions à chaque accès
//   3. Délègue le mappage réel au memory manager (interface opaque)
//   4. Applique le flag NO_COW à toutes les pages mappées
//
// Les hooks de mappage (MapPageFn / UnmapPageFn) sont connectés au boot via
// `ipc::ipc_install_vmm_hooks()`. Sans hook installé, shm_map() opère en
// mode simulé (virt = phys) — acceptable en dev/test mono-processus.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use core::mem::MaybeUninit;

use crate::ipc::core::types::{IpcError, ProcessId};
use crate::ipc::shared_memory::descriptor::{ShmDescriptor, ShmId, ShmPermissions, SHM_DESC_DIR};
use crate::ipc::shared_memory::page::{PhysAddr, PageFlags, PAGE_SIZE};
use crate::scheduler::sync::spinlock::SpinLock;

// ---------------------------------------------------------------------------
// Adresse virtuelle
// ---------------------------------------------------------------------------

/// Adresse virtuelle dans l'espace d'un processus
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct VirtAddr(pub u64);

impl VirtAddr {
    pub const NULL: Self = Self(0);

    pub fn is_null(self) -> bool {
        self.0 == 0
    }

    pub fn is_page_aligned(self) -> bool {
        (self.0 & (PAGE_SIZE as u64 - 1)) == 0
    }
}

// ---------------------------------------------------------------------------
// Hook vers le memory manager (interface future)
// ---------------------------------------------------------------------------

/// Type de pointeur de fonction pour mapper une page physique en virtuel.
/// À connecter avec memory::virtual::map_page() lors de l'intégration.
///
/// Arguments : (phys_addr, virt_addr, flags, process_id)
/// Retour     : 0 = ok, non-zéro = erreur
pub type MapPageFn = unsafe fn(phys: u64, virt: u64, flags: u32, pid: u32) -> i32;

/// Hook optionnel de mappage de page (None = mappage simulé)
static MAP_PAGE_HOOK: SpinLock<Option<MapPageFn>> = SpinLock::new(None);

/// Hook optionnel de démappage de page
pub type UnmapPageFn = unsafe fn(virt: u64, pid: u32) -> i32;
static UNMAP_PAGE_HOOK: SpinLock<Option<UnmapPageFn>> = SpinLock::new(None);

/// Enregistre le hook de mappage fourni par le memory manager.
pub fn register_map_hook(f: MapPageFn) {
    *MAP_PAGE_HOOK.lock() = Some(f);
}

/// Enregistre le hook de démappage fourni par le memory manager.
pub fn register_unmap_hook(f: UnmapPageFn) {
    *UNMAP_PAGE_HOOK.lock() = Some(f);
}

// ---------------------------------------------------------------------------
// Enregistrement de mapping
// ---------------------------------------------------------------------------

/// Nombre maximal de mappings actifs simultanément
pub const MAX_SHM_MAPPINGS: usize = 2048;

/// Représente un mapping actif : (région, processus, adresse virtuelle)
#[repr(C, align(64))]
pub struct ShmMapping {
    /// Index de la région SHM dans SHM_DESC_DIR
    pub desc_idx: AtomicU32,
    /// Identifiant du processus qui a demandé le mapping
    pub process_id: AtomicU32,
    /// Adresse virtuelle de début du mapping dans l'espace du processus
    pub virt_base: AtomicU64,
    /// Permissions effectives du mapping (intersection créateur + demandeur)
    pub permissions: AtomicU32,
    /// Nombre de pages mappées
    pub mapped_pages: AtomicU32,
    /// Timestamp de création du mapping
    pub created_at: AtomicU64,
    /// Mapping actif / libéré
    pub active: AtomicU32,
    _pad: [u8; 12],
}

// SAFETY: tous les champs sont atomiques
unsafe impl Sync for ShmMapping {}
unsafe impl Send for ShmMapping {}

impl ShmMapping {
    pub const fn new_uninit() -> Self {
        Self {
            desc_idx: AtomicU32::new(u32::MAX),
            process_id: AtomicU32::new(0),
            virt_base: AtomicU64::new(0),
            permissions: AtomicU32::new(0),
            mapped_pages: AtomicU32::new(0),
            created_at: AtomicU64::new(0),
            active: AtomicU32::new(0),
            _pad: [0u8; 12],
        }
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire) != 0
    }

    pub fn process_id(&self) -> ProcessId {
        ProcessId(self.process_id.load(Ordering::Relaxed))
    }

    pub fn desc_idx(&self) -> usize {
        self.desc_idx.load(Ordering::Relaxed) as usize
    }

    pub fn virt_base(&self) -> VirtAddr {
        VirtAddr(self.virt_base.load(Ordering::Relaxed))
    }
}

// ---------------------------------------------------------------------------
// Table des mappings actifs
// ---------------------------------------------------------------------------

/// Table globale des mappings SHM actifs
struct ShmMappingTable {
    entries: [ShmMapping; MAX_SHM_MAPPINGS],
    count: usize,
}

// SAFETY: accès protégé par SpinLock
unsafe impl Send for ShmMappingTable {}

impl ShmMappingTable {
    const fn new() -> Self {
        const INIT: ShmMapping = ShmMapping::new_uninit();
        Self {
            entries: [INIT; MAX_SHM_MAPPINGS],
            count: 0,
        }
    }

    fn alloc(&mut self) -> Option<usize> {
        for i in 0..MAX_SHM_MAPPINGS {
            if !self.entries[i].is_active() {
                self.count += 1;
                return Some(i);
            }
        }
        None
    }

    fn free(&mut self, idx: usize) -> bool {
        if idx < MAX_SHM_MAPPINGS && self.entries[idx].is_active() {
            self.entries[idx].active.store(0, Ordering::Release);
            self.count -= 1;
            true
        } else {
            false
        }
    }
}

static SHM_MAPPING_TABLE: SpinLock<ShmMappingTable> =
    SpinLock::new(ShmMappingTable::new());

// ---------------------------------------------------------------------------
// Opération de mappage
// ---------------------------------------------------------------------------

/// Résultat d'une opération de mappage SHM
#[derive(Debug, Clone, Copy)]
pub struct ShmMapResult {
    /// Index du mapping dans la table
    pub mapping_idx: usize,
    /// Adresse virtuelle de début
    pub virt_base: VirtAddr,
    /// Taille mappée en octets
    pub mapped_size: u64,
}

/// Mappe la région SHM `desc_idx` dans l'espace d'adressage du processus `pid`.
///
/// `hint_virt` : adresse virtuelle souhaitée (0 = laissé au memory manager).
/// Permissions : intersection des droits de la région et de `requested_perms`.
///
/// # Erreurs
/// - `IpcError::InvalidHandle` — région inexistante ou détruite
/// - `IpcError::PermissionDenied` — permissions insuffisantes
/// - `IpcError::OutOfResources` — table de mappings pleine
/// - `IpcError::MappingFailed` — erreur du memory manager
pub fn shm_map(
    desc_idx: usize,
    pid: ProcessId,
    hint_virt: VirtAddr,
    requested_perms: ShmPermissions,
) -> Result<ShmMapResult, IpcError> {
    // Vérifier que la région existe et est active
    let (n_pages, region_perms, size_bytes) = {
        let dir = SHM_DESC_DIR.lock();
        let desc = unsafe { dir.get(desc_idx) }.ok_or(IpcError::InvalidHandle)?;
        if !desc.is_active() {
            return Err(IpcError::InvalidHandle);
        }
        // Calculer les permissions effectives (intersection)
        let rp = ShmPermissions(requested_perms.0 & desc.permissions);
        let n = desc.page_count();
        let sz = desc.size_bytes;
        desc.add_mapping();
        (n, rp, sz)
    };

    // Vérifier que les permissions demandées sont accordées
    if requested_perms.can_write() && !region_perms.can_write() {
        // Retirer le mapping
        let dir = SHM_DESC_DIR.lock();
        if let Some(desc) = unsafe { dir.get(desc_idx) } {
            desc.remove_mapping();
        }
        return Err(IpcError::PermissionDenied);
    }

    // Allouer un slot de mapping
    let mapping_idx = {
        let mut tbl = SHM_MAPPING_TABLE.lock();
        tbl.alloc().ok_or(IpcError::OutOfResources)?
    };

    // Calculer ou utiliser l'adresse virtuelle
    // Si hint_virt == NULL, on utilise une adresse virtuelle fictive basée sur
    // l'adresse physique de la première page (le memory manager réel placerait
    // correctement dans l'espace d'adressage du processus).
    let virt_base = if hint_virt.is_null() {
        let phys = {
            let dir = SHM_DESC_DIR.lock();
            // SAFETY: desc_idx alloué par alloc() dans SHM_DESC_DIR; verrou tenu pendant l'accès.
            unsafe { dir.get(desc_idx) }
                .and_then(|d| d.page_phys(0))
                .unwrap_or(PhysAddr::NULL)
        };
        // Adresse virtuelle = adresse physique dans l'implémentation stub
        // (sera remplacé par memory::virtual::find_vma() lors de l'intégration)
        VirtAddr(phys.0)
    } else {
        hint_virt
    };

    // Appeler le hook de mappage pour chaque page
    {
        let map_hook = *MAP_PAGE_HOOK.lock();
        if let Some(map_fn) = map_hook {
            let dir = SHM_DESC_DIR.lock();
            if let Some(desc) = unsafe { dir.get(desc_idx) } {
                let mut page_flags = PageFlags::SHM_DEFAULT;
                if region_perms.can_write() {
                    page_flags.insert(PageFlags::WRITE);
                }
                for i in 0..n_pages {
                    if let Some(phys) = desc.page_phys(i) {
                        let virt = virt_base.0 + (i * PAGE_SIZE) as u64;
                        let result = unsafe {
                            map_fn(phys.0, virt, page_flags.0, pid.0)
                        };
                        if result != 0 {
                            // Annuler les mappings précédents
                            if let Some(unmap_fn) = *UNMAP_PAGE_HOOK.lock() {
                                for j in 0..i {
                                    let v = virt_base.0 + (j * PAGE_SIZE) as u64;
                                    // SAFETY: unmap_fn est un hook validé; v est dans la région mappée (j < i < n_pages).
                                    unsafe { unmap_fn(v, pid.0) };
                                }
                            }
                            drop(dir);
                            // Libérer le compte de mapping
                            let dir2 = SHM_DESC_DIR.lock();
                            if let Some(d) = unsafe { dir2.get(desc_idx) } {
                                d.remove_mapping();
                            }
                            SHM_MAPPING_TABLE.lock().free(mapping_idx);
                            return Err(IpcError::MappingFailed);
                        }
                    }
                }
            }
        }
        // Si pas de hook → mappage simulé (dev/test mode)
    }

    // Enregistrer le mapping dans la table
    {
        let tbl = SHM_MAPPING_TABLE.lock();
        let m = &tbl.entries[mapping_idx];
        m.desc_idx.store(desc_idx as u32, Ordering::Relaxed);
        m.process_id.store(pid.0, Ordering::Relaxed);
        m.virt_base.store(virt_base.0, Ordering::Relaxed);
        m.permissions.store(region_perms.0 as u32, Ordering::Relaxed);
        m.mapped_pages.store(n_pages as u32, Ordering::Relaxed);
        m.active.store(1, Ordering::Release);
    }

    Ok(ShmMapResult {
        mapping_idx,
        virt_base,
        mapped_size: size_bytes,
    })
}

/// Démappe la région SHM identifiée par `mapping_idx`.
pub fn shm_unmap(mapping_idx: usize) -> Result<(), IpcError> {
    let (desc_idx, pid, virt_base, n_pages) = {
        let tbl = SHM_MAPPING_TABLE.lock();
        if mapping_idx >= MAX_SHM_MAPPINGS || !tbl.entries[mapping_idx].is_active() {
            return Err(IpcError::InvalidHandle);
        }
        let m = &tbl.entries[mapping_idx];
        (
            m.desc_idx() as usize,
            ProcessId(m.process_id.load(Ordering::Relaxed)),
            VirtAddr(m.virt_base.load(Ordering::Relaxed)),
            m.mapped_pages.load(Ordering::Relaxed) as usize,
        )
    };

    // Appeler le hook de démappage
    if let Some(unmap_fn) = *UNMAP_PAGE_HOOK.lock() {
        for i in 0..n_pages {
            let virt = virt_base.0 + (i * PAGE_SIZE) as u64;
            // SAFETY: virt + pid proviennent d'un mapping valide
            unsafe { unmap_fn(virt, pid.0) };
        }
    }

    // Libérer le slot de mapping
    SHM_MAPPING_TABLE.lock().free(mapping_idx);

    // Décrémenter le compteur de mappings sur la région
    let dir = SHM_DESC_DIR.lock();
    if let Some(desc) = unsafe { dir.get(desc_idx) } {
        desc.remove_mapping();
    }

    Ok(())
}

/// Retourne le nombre de mappings actifs.
pub fn shm_mapping_count() -> usize {
    SHM_MAPPING_TABLE.lock().count
}
