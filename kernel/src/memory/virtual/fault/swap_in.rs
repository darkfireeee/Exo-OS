// kernel/src/memory/virtual/fault/swap_in.rs
//
// Swap-in fault handler — recharge une page depuis le stockage de swap.
//
// Format du PTE swap (Exo-OS, x86_64, PRESENT=0, PTE!=0) :
//   Bits [63:12] = numéro de bloc disque (swap block number)
//   Bits [11:8]  = index du périphérique de swap (0..15)
//   Bits  [7:1]  = réservés (zéro)
//   Bit   [0]    = 0 (PRESENT = non-présent → c'est un swap PTE)
//
//   Exo-OS encode cette valeur în la PTE quand une page est swapée.
//   La valeur 0 (PTE entièrement nul) signifie « jamais allouée ».
//
// Couche 0 — aucune dépendance externe sauf `spin`.

use core::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

use crate::memory::core::{VirtAddr, Frame, AllocError, PAGE_SIZE};
use crate::memory::virt::vma::VmaDescriptor;
use super::{FaultContext, FaultResult};
use super::handler::FaultAllocator;

// ─────────────────────────────────────────────────────────────────────────────
// TRAIT SwapInProvider (Couche 0 — injection de dépendance)
// ─────────────────────────────────────────────────────────────────────────────

/// Fournisseur de pages depuis le stockage de swap.
///
/// Implémenté par la couche swap/ (non Couche 0).
/// Enregistré via `register_swap_provider()`.
///
/// # Contrat
/// - `swap_device` : index du périphérique swap (bits [11:8] de la PTE)
/// - `swap_block`  : numéro de bloc dans ce périphérique (bits [63:12])
/// - `dest_frame`  : frame de destination (déjà alloué, non initialisé)
///
/// L'implémentation doit remplir exactement PAGE_SIZE octets dans `dest_frame`.
pub trait SwapInProvider: Sync {
    fn read_swap_page(
        &self,
        swap_device: u8,
        swap_block:  u64,
        dest_frame:  Frame,
    ) -> Result<(), AllocError>;
}

// ─────────────────────────────────────────────────────────────────────────────
// REGISTRE GLOBAL DU PROVIDER
// ─────────────────────────────────────────────────────────────────────────────

/// Pointeur sur le SwapInProvider actif (fat pointer — data + vtable).
static SWAP_PROVIDER_DATA:   AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
static SWAP_PROVIDER_VTABLE: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

/// Enregistre le fournisseur de swap-in global.
///
/// Doit être appelé une seule fois depuis le sous-système swap (après init).
///
/// # Safety
/// - `data_ptr` et `vtable` doivent former un fat pointer valide vers
///   un objet `dyn SwapInProvider` de durée de vie `'static`.
/// - Appel single-threaded.
pub unsafe fn register_swap_provider(data_ptr: *const (), vtable: *const ()) {
    SWAP_PROVIDER_DATA  .store(data_ptr as *mut (), Ordering::Release);
    SWAP_PROVIDER_VTABLE.store(vtable   as *mut (), Ordering::Release);
}

/// Désenregistre le provider de swap (pour les tests ou hot-unplug).
pub fn unregister_swap_provider() {
    SWAP_PROVIDER_DATA  .store(core::ptr::null_mut(), Ordering::Release);
    SWAP_PROVIDER_VTABLE.store(core::ptr::null_mut(), Ordering::Release);
}

/// Retourne `true` si un provider de swap est enregistré.
#[inline]
pub fn swap_provider_present() -> bool {
    !SWAP_PROVIDER_DATA.load(Ordering::Acquire).is_null()
}

// ─────────────────────────────────────────────────────────────────────────────
// STATISTIQUES
// ─────────────────────────────────────────────────────────────────────────────

pub struct SwapInStats {
    /// Nombre de faults de swap-in reçus.
    pub total:           AtomicU64,
    /// Pages rechargées avec succès depuis le swap.
    pub success:         AtomicU64,
    /// Faults où aucun provider n'était enregistré → zero-fill fallback.
    pub no_provider:     AtomicU64,
    /// Erreurs I/O du provider (lecture swap échouée).
    pub io_errors:       AtomicU64,
    /// OOM pendant l'allocation du frame de destination.
    pub oom_count:       AtomicU64,
    /// Erreurs de mapping après lecture réussie.
    pub map_errors:      AtomicU64,
    /// PTE nuls (page jamais swapée, demand paging fallback).
    pub null_pte:        AtomicU64,
}

impl SwapInStats {
    pub const fn new() -> Self {
        SwapInStats {
            total:       AtomicU64::new(0),
            success:     AtomicU64::new(0),
            no_provider: AtomicU64::new(0),
            io_errors:   AtomicU64::new(0),
            oom_count:   AtomicU64::new(0),
            map_errors:  AtomicU64::new(0),
            null_pte:    AtomicU64::new(0),
        }
    }
}

pub static SWAP_IN_STATS: SwapInStats = SwapInStats::new();

// ─────────────────────────────────────────────────────────────────────────────
// DÉCODAGE DU PTE SWAP
// ─────────────────────────────────────────────────────────────────────────────

/// Extrait l'index de périphérique swap et le numéro de bloc depuis une PTE brute.
///
/// Format Exo-OS :
///   device  = (pte_raw >> 8) & 0xF      (4 bits → 16 devices max)
///   block   = (pte_raw >> 12) & ((1<<52)-1)  (52 bits de numéro de bloc)
/// Retourne `None` si `pte_raw == 0` (PTE jamais écrite).
#[inline]
fn decode_swap_pte(pte_raw: u64) -> Option<(u8, u64)> {
    if pte_raw == 0 { return None; }
    let device = ((pte_raw >> 8) & 0xF) as u8;
    let block  = (pte_raw >> 12) & ((1u64 << 52) - 1);
    Some((device, block))
}

// ─────────────────────────────────────────────────────────────────────────────
// HANDLER PRINCIPAL
// ─────────────────────────────────────────────────────────────────────────────

/// Traite un swap-in fault.
///
/// 1. Lit la valeur brute de la PTE non-présente (swap entry encodée).
/// 2. Si PTE == 0 → délègue vers demand_paging (page jamais en swap).
/// 3. Décode le swap device et le numéro de bloc.
/// 4. Alloue un frame physique de destination.
/// 5. Appelle `SwapInProvider::read_swap_page()` pour remplir le frame.
/// 6. Mappe le frame dans l'espace d'adressage avec les flags de la VMA.
pub fn handle_swap_in<A: FaultAllocator>(
    ctx:  &FaultContext,
    vma:  &VmaDescriptor,
    alloc: &A,
) -> FaultResult {
    SWAP_IN_STATS.total.fetch_add(1, Ordering::Relaxed);

    let page_addr = VirtAddr::new(ctx.fault_addr.as_u64() & !(PAGE_SIZE as u64 - 1));

    // Lire la PTE brute pour cette page.
    let pte_raw = alloc.read_pte_raw(page_addr);

    // PTE nul → la page n'a jamais été swapée : demand paging.
    let (swap_device, swap_block) = match decode_swap_pte(pte_raw) {
        Some(t) => t,
        None => {
            SWAP_IN_STATS.null_pte.fetch_add(1, Ordering::Relaxed);
            return super::demand_paging::handle_demand_paging(ctx, vma, alloc);
        }
    };

    // Récupérer le provider enregistré.
    let data_ptr = SWAP_PROVIDER_DATA  .load(Ordering::Acquire);
    let vtable   = SWAP_PROVIDER_VTABLE.load(Ordering::Acquire);

    if data_ptr.is_null() || vtable.is_null() {
        // Aucun provider → zero-fill et continuer (sous-optimal mais pas plantant).
        SWAP_IN_STATS.no_provider.fetch_add(1, Ordering::Relaxed);
        let frame = match alloc.alloc_zeroed() {
            Ok(f)  => f,
            Err(_) => {
                SWAP_IN_STATS.oom_count.fetch_add(1, Ordering::Relaxed);
                return FaultResult::Oom { addr: ctx.fault_addr };
            }
        };
        return match alloc.map_page(page_addr, frame, vma.page_flags) {
            Ok(_)  => { vma.record_fault(); FaultResult::Handled }
            Err(_) => {
                alloc.free_frame(frame);
                SWAP_IN_STATS.map_errors.fetch_add(1, Ordering::Relaxed);
                FaultResult::Oom { addr: ctx.fault_addr }
            }
        };
    }

    // Allouer le frame de destination (non-zéro, le provider le remplira).
    let frame = match alloc.alloc_nonzeroed() {
        Ok(f)  => f,
        Err(_) => {
            SWAP_IN_STATS.oom_count.fetch_add(1, Ordering::Relaxed);
            return FaultResult::Oom { addr: ctx.fault_addr };
        }
    };

    // Reconstruire le fat pointer et appeler read_swap_page.
    // SAFETY : fat pointer enregistré par `register_swap_provider`, 'static.
    let fat: (*const (), *const ()) = (data_ptr as *const (), vtable as *const ());
    let provider: &dyn SwapInProvider = unsafe { core::mem::transmute(fat) };

    match provider.read_swap_page(swap_device, swap_block, frame) {
        Ok(()) => {
            // Mapper la page rechargée avec les flags de la VMA.
            match alloc.map_page(page_addr, frame, vma.page_flags) {
                Ok(_) => {
                    SWAP_IN_STATS.success.fetch_add(1, Ordering::Relaxed);
                    vma.record_fault();
                    FaultResult::Handled
                }
                Err(_) => {
                    alloc.free_frame(frame);
                    SWAP_IN_STATS.map_errors.fetch_add(1, Ordering::Relaxed);
                    FaultResult::Oom { addr: ctx.fault_addr }
                }
            }
        }
        Err(_) => {
            // Erreur I/O : libérer le frame et signaler au caller.
            alloc.free_frame(frame);
            SWAP_IN_STATS.io_errors.fetch_add(1, Ordering::Relaxed);
            // Retourner OOM (la page restera swapée, le process sera réessayé ou tué).
            FaultResult::Oom { addr: ctx.fault_addr }
        }
    }
}
