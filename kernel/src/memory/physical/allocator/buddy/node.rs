//! # memory/physical/allocator/buddy/node.rs — Structure des nœuds et niveaux de free-list
//!
//! Nœuds doublement chaînés stockés in-place dans les pages physiques libres
//! et gestion de l'alignement/traduction d'adresses en physmap.

use core::ptr::null_mut;
use core::sync::atomic::AtomicUsize;
use crate::memory::core::{PAGE_SIZE, PhysAddr};

// ─────────────────────────────────────────────────────────────────────────────
// FREE LIST NODE — nœud de liste chaînée dans un buddy bloc libre
// ─────────────────────────────────────────────────────────────────────────────

/// Nœud de liste chaînée embedding dans un bloc libre.
/// Stocké directement dans les premiers octets du bloc (page physique libre).
///
/// SAFETY: Ce nœud est lisible uniquement quand le bloc est LIBRE.
/// Dès qu'un bloc est alloué, son contenu est indéfini pour l'allocateur.
#[repr(C)]
pub struct FreeNode {
    pub next: *mut FreeNode,
    pub prev: *mut FreeNode,
    pub order: u8,
    pub _pad: [u8; 7],
}

impl FreeNode {
    #[inline]
    pub unsafe fn init(ptr: *mut FreeNode, order: u8) {
        (*ptr).next = ptr;
        (*ptr).prev = ptr;
        (*ptr).order = order;
    }

    #[inline]
    pub unsafe fn insert_after(list_head: *mut FreeNode, node: *mut FreeNode) {
        let next = (*list_head).next;
        (*node).prev = list_head;
        (*node).next = next;
        (*next).prev = node;
        (*list_head).next = node;
    }

    #[inline]
    pub unsafe fn remove(node: *mut FreeNode) {
        let prev = (*node).prev;
        let next = (*node).next;
        (*prev).next = next;
        (*next).prev = prev;
        (*node).next = node;
        (*node).prev = node;
    }

    #[inline]
    pub unsafe fn is_empty(list_head: *const FreeNode) -> bool {
        if list_head.is_null() {
            return true;
        }
        (*list_head).next == list_head as *mut FreeNode
    }

    #[inline]
    pub unsafe fn pop(list_head: *mut FreeNode) -> Option<*mut FreeNode> {
        if Self::is_empty(list_head) {
            return None;
        }
        let node = (*list_head).next;
        Self::remove(node);
        Some(node)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BUDDY ORDER LEVEL — une free-list pour un ordre donné
// ─────────────────────────────────────────────────────────────────────────────

/// Tête de free-list pour un ordre donné.
#[repr(C, align(64))]
pub struct BuddyOrderLevel {
    /// Tête sentinelle (nœud vide — next/prev pointent vers elle-même si vide).
    pub head: FreeNode,
    /// Nombre de blocs libres à cet ordre.
    pub count: AtomicUsize,
}

impl BuddyOrderLevel {
    pub const fn new() -> Self {
        BuddyOrderLevel {
            head: FreeNode {
                next: null_mut(),
                prev: null_mut(),
                order: 0,
                _pad: [0; 7],
            },
            count: AtomicUsize::new(0),
        }
    }

    /// Initialise la sentinelle (ne peut pas être fait en const, besoin de pointeur).
    pub unsafe fn init(&mut self) {
        let h = &mut self.head as *mut FreeNode;
        (*h).next = h;
        (*h).prev = h;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// UTILITAIRES INTERNES (bridge physmap direct)
// ─────────────────────────────────────────────────────────────────────────────

/// Traduit une adresse physique en pointeur virtuel pour accéder aux FreeNodes.
/// Utilise la physmap directe (PHYS_MAP_BASE + phys).
#[inline(always)]
pub fn phys_to_virt_buddy(phys: PhysAddr) -> usize {
    use crate::memory::core::layout::PHYS_MAP_BASE;
    PHYS_MAP_BASE.as_usize() + phys.as_usize()
}

/// Traduit un pointeur virtuel (dans la physmap) en adresse physique.
#[inline(always)]
pub fn virt_to_phys_buddy(virt: usize) -> PhysAddr {
    use crate::memory::core::layout::PHYS_MAP_BASE;
    PhysAddr::new((virt - PHYS_MAP_BASE.as_usize()) as u64)
}

/// Remplit une plage de pages de zéros.
///
/// SAFETY: `phys` doit être l'adresse de `2^order` pages allouées exclusivement
/// par l'appelant. La physmap doit être accessible.
#[inline]
pub unsafe fn zero_pages(phys: PhysAddr, order: usize) {
    let size = PAGE_SIZE << order;
    let virt = phys_to_virt_buddy(phys) as *mut u8;
    core::ptr::write_bytes(virt, 0, size);
}
