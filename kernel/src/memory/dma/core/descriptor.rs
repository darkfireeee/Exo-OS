// kernel/src/memory/dma/core/descriptor.rs
//
// Descripteur de transaction DMA — structure principale manipulée par les
// engines et canaux DMA.
//
// COUCHE 0 — aucune dépendance externe.

use crate::memory::core::types::PhysAddr;
use crate::memory::dma::core::types::{
    DmaDirection, DmaError, DmaMapFlags, DmaPriority, DmaTransactionId, DmaTransactionState,
    IovaAddr,
};
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// SCATTER-GATHER
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum d'entrées scatter-gather par transaction.
pub const MAX_SG_ENTRIES: usize = 32;

/// Une entrée scatter-gather : (adresse_physique, longueur).
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct SgEntry {
    /// Adresse physique du fragment.
    pub phys: PhysAddr,
    /// Longueur du fragment en octets.
    pub len: u32,
    /// Offset par rapport à la page de base.
    pub page_offset: u16,
    /// Réservé.
    pub _pad: [u8; 2],
}

impl SgEntry {
    pub const EMPTY: Self = SgEntry {
        phys: PhysAddr::new(0),
        len: 0,
        page_offset: 0,
        _pad: [0u8; 2],
    };

    #[inline]
    pub fn total_bytes(&self) -> usize {
        self.len as usize
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DESCRIPTEUR PRINCIPAL
// ─────────────────────────────────────────────────────────────────────────────

/// Descripteur complet d'une transaction DMA.
/// Taille cible : 256 octets pour tenir dans 4 lignes de cache.
#[repr(C, align(64))]
pub struct DmaDescriptor {
    // ── Identité ──────────────────────────────────────────────────────────────
    /// Identifiant unique de la transaction.
    pub txn_id: DmaTransactionId,
    /// Canal DMA associé.
    pub channel_id: u32,
    /// Thread/processus demandeur (pour le wakeup à la fin).
    pub requester_tid: u64,

    // ── Paramètres du transfert ───────────────────────────────────────────────
    /// Direction du transfert.
    pub direction: DmaDirection,
    /// Priorité.
    pub priority: DmaPriority,
    /// Flags de mapping.
    pub map_flags: DmaMapFlags,
    /// Domaine IOMMU (0 = domaine système).
    pub iommu_domain: u32,

    // ── Source ────────────────────────────────────────────────────────────────
    /// Adresse source (physique) — valide si src_sg_count == 0.
    pub src_phys: PhysAddr,
    /// IOVA source mappée par IOMMU.
    pub src_iova: IovaAddr,
    /// Nombre d'entrées SG source (0 = adresse unique).
    pub src_sg_count: u16,

    // ── Destination ───────────────────────────────────────────────────────────
    /// Adresse destination (physique) — valide si dst_sg_count == 0.
    pub dst_phys: PhysAddr,
    /// IOVA destination mappée par IOMMU.
    pub dst_iova: IovaAddr,
    /// Nombre d'entrées SG destination (0 = adresse unique).
    pub dst_sg_count: u16,

    // ── Taille ────────────────────────────────────────────────────────────────
    /// Taille total du transfert en octets.
    pub transfer_size: usize,
    /// Octets effectivement transférés (mis à jour par l'engine).
    pub bytes_done: AtomicU64,

    // ── État ──────────────────────────────────────────────────────────────────
    /// État courant de la transaction.
    pub state: AtomicU8,
    /// Code d'erreur (valide si state == Error).
    pub error: AtomicU8,
    /// Timestamp de soumission (TSC cycles).
    pub submit_tsc: AtomicU64,
    /// Timestamp de completion (TSC cycles).
    pub done_tsc: AtomicU64,

    // ── Scatter-Gather ────────────────────────────────────────────────────────
    /// Table SG source.
    pub src_sg: [SgEntry; MAX_SG_ENTRIES],
    /// Table SG destination.
    pub dst_sg: [SgEntry; MAX_SG_ENTRIES],

    /// Padding pour aligner à 64.
    _pad: [u8; 6],
}

// Vérification de taille : <= 2048 octets (SG tables incluses, 2×32×16 = 1024 + headers).
const _: () = assert!(core::mem::size_of::<DmaDescriptor>() <= 2048);

impl DmaDescriptor {
    /// Construit un descripteur vide avec l'ID fourni.
    pub fn new(txn_id: DmaTransactionId, channel_id: u32, requester_tid: u64) -> Self {
        DmaDescriptor {
            txn_id,
            channel_id,
            requester_tid,
            direction: DmaDirection::Bidirection,
            priority: DmaPriority::Normal,
            map_flags: DmaMapFlags::NONE,
            iommu_domain: 0,
            src_phys: PhysAddr::new(0),
            src_iova: IovaAddr::zero(),
            src_sg_count: 0,
            dst_phys: PhysAddr::new(0),
            dst_iova: IovaAddr::zero(),
            dst_sg_count: 0,
            transfer_size: 0,
            bytes_done: AtomicU64::new(0),
            state: AtomicU8::new(DmaTransactionState::Pending as u8),
            error: AtomicU8::new(0),
            submit_tsc: AtomicU64::new(0),
            done_tsc: AtomicU64::new(0),
            src_sg: [SgEntry::EMPTY; MAX_SG_ENTRIES],
            dst_sg: [SgEntry::EMPTY; MAX_SG_ENTRIES],
            _pad: [0u8; 6],
        }
    }

    // ── Accesseurs d'état ─────────────────────────────────────────────────────

    #[inline]
    pub fn state(&self) -> DmaTransactionState {
        match self.state.load(Ordering::Acquire) {
            0 => DmaTransactionState::Free,
            1 => DmaTransactionState::Pending,
            2 => DmaTransactionState::Submitted,
            3 => DmaTransactionState::Running,
            4 => DmaTransactionState::Done,
            5 => DmaTransactionState::Error,
            6 => DmaTransactionState::Cancelled,
            _ => DmaTransactionState::Error,
        }
    }

    #[inline]
    pub fn set_state(&self, s: DmaTransactionState) {
        self.state.store(s as u8, Ordering::Release);
    }

    #[inline]
    pub fn set_error(&self, e: DmaError) {
        self.error.store(e as u8, Ordering::Release);
        self.set_state(DmaTransactionState::Error);
    }

    #[inline]
    pub fn mark_done(&self, bytes: usize) {
        self.bytes_done.store(bytes as u64, Ordering::Release);
        self.set_state(DmaTransactionState::Done);
    }

    #[inline]
    pub fn is_done(&self) -> bool {
        matches!(
            self.state(),
            DmaTransactionState::Done | DmaTransactionState::Error | DmaTransactionState::Cancelled
        )
    }

    // ── Configuration SG ─────────────────────────────────────────────────────

    /// Ajoute une entrée SG source.
    pub fn add_src_sg(&mut self, phys: PhysAddr, len: u32) -> bool {
        let idx = self.src_sg_count as usize;
        if idx >= MAX_SG_ENTRIES {
            return false;
        }
        self.src_sg[idx] = SgEntry {
            phys,
            len,
            page_offset: 0,
            _pad: [0; 2],
        };
        self.src_sg_count += 1;
        self.transfer_size += len as usize;
        true
    }

    /// Ajoute une entrée SG destination.
    pub fn add_dst_sg(&mut self, phys: PhysAddr, len: u32) -> bool {
        let idx = self.dst_sg_count as usize;
        if idx >= MAX_SG_ENTRIES {
            return false;
        }
        self.dst_sg[idx] = SgEntry {
            phys,
            len,
            page_offset: 0,
            _pad: [0; 2],
        };
        self.dst_sg_count += 1;
        true
    }

    /// Configure un transfert simple (src unique → dst unique).
    pub fn setup_simple(&mut self, src: PhysAddr, dst: PhysAddr, size: usize, dir: DmaDirection) {
        self.src_phys = src;
        self.dst_phys = dst;
        self.transfer_size = size;
        self.direction = dir;
        self.src_sg_count = 0;
        self.dst_sg_count = 0;
    }

    /// Configure un transfert FILL (memset DMA) : remplit `size` octets à `dst`
    /// avec le motif `value`.
    ///
    /// Convention Exo-OS : `src_phys` encode la valeur de remplissage
    /// (bits [7:0]) pour les moteurs I/OAT/DSA qui utilisent un pattern fill.
    /// `src_sg_count == 0` et `src_iova == 0` indiquent le mode FILL.
    pub fn setup_fill(&mut self, dst: PhysAddr, value: u8, size: usize) {
        // Encoder la valeur de fill dans src_phys.0 (les 8 bits de poids faible).
        self.src_phys = PhysAddr::new(value as u64);
        self.dst_phys = dst;
        self.transfer_size = size;
        self.direction = DmaDirection::ToDevice;
        self.src_sg_count = 0;
        self.dst_sg_count = 0;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TABLE DE TRANSACTION (RING BUFFER DE DESCRIPTEURS LIBRES)
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximum de transactions DMA simultanées gérées par le système.
pub const MAX_DMA_TRANSACTIONS: usize = 512;

/// Table globale des descripteurs DMA réutilisables.
pub struct DmaDescriptorTable {
    slots: spin::Mutex<DmaDescriptorTableInner>,
}

struct DmaDescriptorTableInner {
    free_list: [u16; MAX_DMA_TRANSACTIONS], // indices libres
    free_head: usize,
    free_count: usize,
    /// Stockage des descripteurs (MaybeUninit pour éviter l'initialisation).
    storage: [core::mem::MaybeUninit<DmaDescriptor>; MAX_DMA_TRANSACTIONS],
}

// SAFETY: DmaDescriptorTable est protégé par un spin::Mutex.
unsafe impl Sync for DmaDescriptorTable {}
unsafe impl Send for DmaDescriptorTable {}

impl DmaDescriptorTable {
    const fn new() -> Self {
        DmaDescriptorTable {
            slots: spin::Mutex::new(DmaDescriptorTableInner {
                // Initialise free_list[i] = i.
                free_list: unsafe { core::mem::transmute([0u8; MAX_DMA_TRANSACTIONS * 2]) },
                free_head: 0,
                free_count: 0,
                storage: unsafe { core::mem::MaybeUninit::uninit().assume_init() },
            }),
        }
    }

    /// Initialise la table (appel unique au boot).
    pub fn init(&self) {
        let mut inner = self.slots.lock();
        for i in 0..MAX_DMA_TRANSACTIONS {
            inner.free_list[i] = i as u16;
        }
        inner.free_head = 0;
        inner.free_count = MAX_DMA_TRANSACTIONS;
    }

    /// Alloue un descripteur depuis le pool.
    pub fn alloc_descriptor(
        &self,
        channel_id: u32,
        requester_tid: u64,
    ) -> Option<&mut DmaDescriptor> {
        let mut inner = self.slots.lock();
        if inner.free_count == 0 {
            return None;
        }

        let idx = inner.free_list[inner.free_head] as usize;
        inner.free_head = (inner.free_head + 1) % MAX_DMA_TRANSACTIONS;
        inner.free_count -= 1;

        let txn_id = DmaTransactionId::generate();
        let desc_ptr = inner.storage[idx].as_mut_ptr();
        // SAFETY: Le slot est libre (free_list garantit l'unicité).
        unsafe {
            desc_ptr.write(DmaDescriptor::new(txn_id, channel_id, requester_tid));
            Some(&mut *(desc_ptr))
        }
    }

    /// Libère un descripteur (ne doit être appelé qu'après is_done()).
    ///
    /// # Safety
    /// `idx` doit être l'index du descripteur dans le storage.
    /// Le descripteur doit être dans un état terminal (Done/Error/Cancelled).
    pub unsafe fn free_descriptor(&self, idx: usize) {
        let mut inner = self.slots.lock();
        let tail = (inner.free_head + inner.free_count) % MAX_DMA_TRANSACTIONS;
        inner.free_list[tail] = idx as u16;
        inner.free_count += 1;
    }
}

pub static DMA_DESCRIPTOR_TABLE: DmaDescriptorTable = DmaDescriptorTable::new();
