// ipc/shared_memory/descriptor.rs — Descripteur de région SHM pour Exo-OS
//
// ShmDescriptor est la structure principale décrivant une région de mémoire
// partagée IPC : identifiant, taille, propriétaire, permissions, liste des
// pages physiques associées.
//
// Contraintes :
//   - Taille maximale : SHM_MAX_PAGES * PAGE_SIZE (256 × 4 KiB = 1 MiB)
//   - Pas d'allocation dynamique — liste de pages inline (MAX_SHM_PAGES_PER_DESC)
//   - Toutes les pages doivent avoir le flag NO_COW

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use core::mem::MaybeUninit;

use crate::ipc::core::types::{IpcError, ProcessId};
use crate::ipc::core::constants::SHM_POOL_PAGES;
use crate::ipc::shared_memory::page::{PhysAddr, PageFlags, PAGE_SIZE};
use crate::ipc::shared_memory::pool::{shm_page_alloc, shm_page_free, shm_page_phys};

// ---------------------------------------------------------------------------
// Identifiants SHM
// ---------------------------------------------------------------------------

/// Identifiant unique d'une région SHM
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct ShmId(u32);

impl ShmId {
    pub const INVALID: Self = Self(0);

    pub fn new(v: u32) -> Option<Self> {
        if v != 0 { Some(Self(v)) } else { None }
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

/// Compteur pour générer des ShmId uniques
static SHM_ID_COUNTER: AtomicU32 = AtomicU32::new(1);

pub fn alloc_shm_id() -> ShmId {
    let v = SHM_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    ShmId(v)
}

// ---------------------------------------------------------------------------
// Permissions SHM
// ---------------------------------------------------------------------------

/// Permissions d'accès à une région SHM
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShmPermissions(pub u8);

impl ShmPermissions {
    pub const READ: Self = Self(1 << 0);
    pub const WRITE: Self = Self(1 << 1);
    pub const READ_WRITE: Self = Self(Self::READ.0 | Self::WRITE.0);

    pub fn can_read(self) -> bool {
        (self.0 & Self::READ.0) != 0
    }

    pub fn can_write(self) -> bool {
        (self.0 & Self::WRITE.0) != 0
    }
}

// ---------------------------------------------------------------------------
// Nombre maximal de pages par descripteur
// ---------------------------------------------------------------------------

/// Nombre maximal de pages physiques par région SHM (limite inline)
pub const MAX_SHM_PAGES_PER_DESC: usize = 64; // 64 × 4 KiB = 256 KiB max

// ---------------------------------------------------------------------------
// État d'une région SHM
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ShmState {
    /// Non initialisée
    Uninitialized = 0,
    /// Active et accessible
    Active = 1,
    /// En cours de fermeture (plus de nouveaux mappings autorisés)
    Closing = 2,
    /// Détruite
    Destroyed = 3,
}

impl ShmState {
    fn from_u32(v: u32) -> Self {
        match v {
            1 => Self::Active,
            2 => Self::Closing,
            3 => Self::Destroyed,
            _ => Self::Uninitialized,
        }
    }
}

// ---------------------------------------------------------------------------
// ShmDescriptor — structure principale
// ---------------------------------------------------------------------------

/// Descripteur d'une région de mémoire partagée IPC.
///
/// Contient l'ensemble des métadonnées et la liste des pages physiques
/// composant la région.
#[repr(C, align(64))]
pub struct ShmDescriptor {
    /// Identifiant unique de la région
    pub id: ShmId,
    /// Processus propriétaire
    pub owner: ProcessId,
    /// État courant
    pub state: AtomicU32,
    /// Permissions déclarées à la création
    pub permissions: u8,
    _perm_pad: [u8; 3],
    /// Nombre de pages allouées
    pub page_count: AtomicU32,
    /// Taille totale en octets (page_count * PAGE_SIZE)
    pub size_bytes: u64,
    /// Compteur de mappings actifs (référencements par des processus)
    pub mapping_count: AtomicU32,
    /// Timestamp de création (ns depuis boot)
    pub created_at: AtomicU64,
    /// Timestamp du dernier accès
    pub last_access: AtomicU64,
    /// Nombre d'accès totaux
    pub access_count: AtomicU64,
    /// Indices des pages dans le pool SHM (MAX_SHM_PAGES_PER_DESC entmax)
    pub pages: [AtomicU32; MAX_SHM_PAGES_PER_DESC],
    _pad: [u8; 8],
}

// SAFETY: ShmId(u32) + ProcessId(u32) sont Copy ; AtomicU32/AtomicU64 sont Sync
unsafe impl Sync for ShmDescriptor {}
unsafe impl Send for ShmDescriptor {}

impl ShmDescriptor {
    /// Crée un descripteur non-initialisé
    pub const fn new_uninit() -> Self {
        const INIT_ATOMIC: AtomicU32 = AtomicU32::new(u32::MAX);
        Self {
            id: ShmId::INVALID,
            owner: ProcessId(0),
            state: AtomicU32::new(ShmState::Uninitialized as u32),
            permissions: 0,
            _perm_pad: [0u8; 3],
            page_count: AtomicU32::new(0),
            size_bytes: 0,
            mapping_count: AtomicU32::new(0),
            created_at: AtomicU64::new(0),
            last_access: AtomicU64::new(0),
            access_count: AtomicU64::new(0),
            pages: [INIT_ATOMIC; MAX_SHM_PAGES_PER_DESC],
            _pad: [0u8; 8],
        }
    }

    pub fn state(&self) -> ShmState {
        ShmState::from_u32(self.state.load(Ordering::Acquire))
    }

    pub fn is_active(&self) -> bool {
        self.state() == ShmState::Active
    }

    pub fn page_count(&self) -> usize {
        self.page_count.load(Ordering::Acquire) as usize
    }

    /// Retourne l'adresse physique de la page `i` de la région.
    pub fn page_phys(&self, i: usize) -> Option<PhysAddr> {
        if i >= self.page_count() {
            return None;
        }
        let idx = self.pages[i].load(Ordering::Relaxed) as usize;
        shm_page_phys(idx)
    }

    /// Retourne l'index pool de la page `i`.
    pub fn page_pool_idx(&self, i: usize) -> Option<usize> {
        if i >= self.page_count() {
            return None;
        }
        let idx = self.pages[i].load(Ordering::Relaxed);
        if idx == u32::MAX { None } else { Some(idx as usize) }
    }

    /// Incrémente le compteur de mappings actifs.
    pub fn add_mapping(&self) {
        self.mapping_count.fetch_add(1, Ordering::Relaxed);
        self.access_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Décrémente le compteur de mappings actifs.
    /// Retourne `true` si plus aucun mapping actif.
    pub fn remove_mapping(&self) -> bool {
        let prev = self.mapping_count.fetch_sub(1, Ordering::AcqRel);
        prev == 1
    }

    pub fn mapping_count(&self) -> u32 {
        self.mapping_count.load(Ordering::Relaxed)
    }

    /// Libère toutes les pages allouées et passe à l'état Destroyed.
    pub fn destroy(&self) {
        self.state.store(ShmState::Destroyed as u32, Ordering::Release);
        let n = self.page_count();
        for i in 0..n {
            let pool_idx = self.pages[i].load(Ordering::Relaxed) as usize;
            if pool_idx < crate::ipc::core::constants::SHM_POOL_PAGES {
                shm_page_free(pool_idx);
                self.pages[i].store(u32::MAX, Ordering::Relaxed);
            }
        }
        self.page_count.store(0, Ordering::Release);
    }
}

// ---------------------------------------------------------------------------
// Allocateur de répertoire de descripteurs
// ---------------------------------------------------------------------------

/// Nombre maximal de régions SHM pouvant coexister simultanément
pub const MAX_SHM_REGIONS: usize = 1024;

/// Entrée dans le répertoire des descripteurs SHM
pub struct ShmDescEntry {
    pub desc: MaybeUninit<ShmDescriptor>,
    pub used: bool,
}

impl ShmDescEntry {
    pub const fn new() -> Self {
        Self {
            desc: MaybeUninit::uninit(),
            used: false,
        }
    }
}

/// Répertoire global des descripteurs SHM (protégé par SpinLock)
pub struct ShmDescDirectory {
    entries: [ShmDescEntry; MAX_SHM_REGIONS],
    count: usize,
}

// SAFETY: accès protégé par SpinLock dans la table globale
unsafe impl Send for ShmDescDirectory {}

impl ShmDescDirectory {
    pub const fn new() -> Self {
        const INIT: ShmDescEntry = ShmDescEntry::new();
        Self {
            entries: [INIT; MAX_SHM_REGIONS],
            count: 0,
        }
    }

    /// Alloue un nouveau descripteur et retourne son index.
    pub fn alloc(&mut self, owner: ProcessId, perms: ShmPermissions, n_pages: usize)
        -> Result<usize, IpcError>
    {
        if n_pages == 0 || n_pages > MAX_SHM_PAGES_PER_DESC {
            return Err(IpcError::InvalidArgument);
        }

        // Chercher un slot libre
        for i in 0..MAX_SHM_REGIONS {
            if !self.entries[i].used {
                let mut desc = ShmDescriptor::new_uninit();
                desc.id = alloc_shm_id();
                desc.owner = owner;
                desc.permissions = perms.0;

                // Allouer les pages depuis le pool
                let mut allocated = 0usize;
                for j in 0..n_pages {
                    match shm_page_alloc() {
                        Some(page_idx) => {
                            desc.pages[j].store(page_idx as u32, Ordering::Relaxed);
                            allocated += 1;
                        }
                        None => {
                            // Annuler les allocations déjà faites
                            for k in 0..allocated {
                                let idx = desc.pages[k].load(Ordering::Relaxed) as usize;
                                shm_page_free(idx);
                            }
                            return Err(IpcError::OutOfResources);
                        }
                    }
                }

                desc.page_count.store(n_pages as u32, Ordering::Relaxed);
                desc.size_bytes = (n_pages * PAGE_SIZE) as u64;
                desc.state.store(ShmState::Active as u32, Ordering::Release);

                self.entries[i].desc.write(desc);
                self.entries[i].used = true;
                self.count += 1;
                return Ok(i);
            }
        }
        Err(IpcError::OutOfResources)
    }

    pub fn free(&mut self, idx: usize) -> bool {
        if idx < MAX_SHM_REGIONS && self.entries[idx].used {
            // SAFETY: used est true → desc est initialisé
            unsafe { self.entries[idx].desc.assume_init_ref() }.destroy();
            // SAFETY: desc initialisé (used true); used → false immédiatement après, empêche double-drop.
            unsafe { self.entries[idx].desc.assume_init_drop() };
            self.entries[idx].used = false;
            self.count -= 1;
            true
        } else {
            false
        }
    }

    pub unsafe fn get(&self, idx: usize) -> Option<&ShmDescriptor> {
        if idx < MAX_SHM_REGIONS && self.entries[idx].used {
            Some(self.entries[idx].desc.assume_init_ref())
        } else {
            None
        }
    }

    pub fn count(&self) -> usize {
        self.count
    }
}

use crate::scheduler::sync::spinlock::SpinLock;

pub static SHM_DESC_DIR: SpinLock<ShmDescDirectory> =
    SpinLock::new(ShmDescDirectory::new());

// ---------------------------------------------------------------------------
// API publique de haut niveau
// ---------------------------------------------------------------------------

/// Crée une nouvelle région SHM de `n_pages` pages.
/// Retourne l'index dans le répertoire.
pub fn shm_create(owner: ProcessId, perms: ShmPermissions, n_pages: usize) -> Result<usize, IpcError> {
    let mut dir = SHM_DESC_DIR.lock();
    dir.alloc(owner, perms, n_pages)
}

/// Retourne l'ID SHM associé à l'index `idx`.
pub fn shm_get_id(idx: usize) -> Option<ShmId> {
    let dir = SHM_DESC_DIR.lock();
    let desc = unsafe { dir.get(idx) }?;
    Some(desc.id)
}

/// Retourne la taille en octets de la région `idx`.
pub fn shm_get_size(idx: usize) -> Option<u64> {
    let dir = SHM_DESC_DIR.lock();
    let desc = unsafe { dir.get(idx) }?;
    Some(desc.size_bytes)
}

/// Détruit la région SHM `idx` (libère toutes ses pages).
pub fn shm_destroy(idx: usize) -> Result<(), IpcError> {
    let mut dir = SHM_DESC_DIR.lock();
    if !dir.free(idx) {
        return Err(IpcError::InvalidHandle);
    }
    Ok(())
}

/// Retourne le nombre de régions SHM actives.
pub fn shm_region_count() -> usize {
    SHM_DESC_DIR.lock().count()
}
