// kernel/src/memory/virtual/vma/tree.rs
//
// Arbre AVL des VMAs — structure d'index par adresse virtuelle.
// Permet de trouver la VMA contenant une adresse en O(log n).
// Taille maximale : MAX_VMAS_PER_PROCESS VMAs par processus.
// Couche 0 — aucune dépendance externe sauf `spin`.

use core::ptr::NonNull;
use crate::memory::core::VirtAddr;
use super::descriptor::VmaDescriptor;

// ─────────────────────────────────────────────────────────────────────────────
// ARBRE AVL DE VMAS
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre maximum de VMAs par processus (limite kernel).
pub const MAX_VMAS_PER_PROCESS: usize = 65536;

/// Arbre AVL des VMAs d'un espace d'adressage.
///
/// Les noeuds sont des pointeurs vers des VmaDescriptor alloués par le slab.
/// L'arbre ne possède PAS les descripteurs — c'est le VmaAllocator qui les gère.
pub struct VmaTree {
    root:    *mut VmaDescriptor,
    count:   usize,
}

// SAFETY: VmaTree est protégé par le verrou de l'address space parent.
unsafe impl Send for VmaTree {}
unsafe impl Sync for VmaTree {}

impl VmaTree {
    pub const fn new() -> Self {
        VmaTree { root: core::ptr::null_mut(), count: 0 }
    }

    /// Nombre de VMAs dans l'arbre.
    pub fn len(&self) -> usize { self.count }
    pub fn is_empty(&self) -> bool { self.count == 0 }

    /// Cherche la VMA contenant `addr`.
    ///
    /// Retourne un pointeur vers le VmaDescriptor ou `None`.
    pub fn find(&self, addr: VirtAddr) -> Option<&VmaDescriptor> {
        // SAFETY: Tous les nœuds de l'arbre sont des VmaDescriptor valides.
        unsafe { Self::find_node(self.root, addr) }
    }

    /// Cherche la VMA contenant `addr` (mutable).
    pub fn find_mut(&mut self, addr: VirtAddr) -> Option<&mut VmaDescriptor> {
        // SAFETY: Accès exclusif (verrou parent).
        unsafe { Self::find_node_mut(self.root, addr) }
    }

    /// Insère un nouveau descripteur VMA dans l'arbre.
    ///
    /// Retourne `false` si une VMA existante chevauche la plage.
    ///
    /// SAFETY: `vma` doit pointer sur un VmaDescriptor valide et non encore
    ///         inséré dans un autre arbre.
    pub unsafe fn insert(&mut self, vma: *mut VmaDescriptor) -> bool {
        if self.count >= MAX_VMAS_PER_PROCESS { return false; }
        if self.root.is_null() {
            self.root  = vma;
            self.count = 1;
            return true;
        }
        if Self::insert_node(&mut self.root, vma) {
            self.count += 1;
            true
        } else {
            false
        }
    }

    /// Retire un descripteur VMA de l'arbre (par adresse de départ).
    ///
    /// Retourne le pointeur vers le descripteur retiré, ou `None`.
    pub fn remove(&mut self, start: VirtAddr) -> Option<*mut VmaDescriptor> {
        // SAFETY: Accès exclusif.
        let removed = unsafe { Self::remove_node(&mut self.root, start) };
        if removed.is_some() { self.count = self.count.saturating_sub(1); }
        removed
    }

    /// Trouve la VMA qui précède immédiatement `addr` (le plus grand `end` <= `addr`).
    pub fn find_prev(&self, addr: VirtAddr) -> Option<&VmaDescriptor> {
        unsafe { Self::find_prev_node(self.root, addr) }
    }

    /// Itère sur toutes les VMAs dans l'ordre croissant.
    pub fn iter(&self) -> VmaTreeIter<'_> {
        let stack = [core::ptr::null_mut::<VmaDescriptor>(); 64];
        let depth = 0usize;
        VmaTreeIter::new(self.root, stack, depth)
    }

    // ─── helpers de l'arbre AVL ──────────────────────────────────────────────

    unsafe fn find_node<'a>(mut node: *mut VmaDescriptor, addr: VirtAddr) -> Option<&'a VmaDescriptor> {
        while !node.is_null() {
            let n = &*node;
            if addr.as_u64() < n.start.as_u64() {
                node = n.rb_left;
            } else if addr.as_u64() >= n.end.as_u64() {
                node = n.rb_right;
            } else {
                return Some(n);
            }
        }
        None
    }

    unsafe fn find_node_mut<'a>(mut node: *mut VmaDescriptor, addr: VirtAddr) -> Option<&'a mut VmaDescriptor> {
        while !node.is_null() {
            let n = &mut *node;
            if addr.as_u64() < n.start.as_u64() {
                node = n.rb_left;
            } else if addr.as_u64() >= n.end.as_u64() {
                node = n.rb_right;
            } else {
                return Some(&mut *node);
            }
        }
        None
    }

    unsafe fn insert_node(root: &mut *mut VmaDescriptor, vma: *mut VmaDescriptor) -> bool {
        let node = *root;
        if node.is_null() {
            *root = vma;
            (*vma).rb_left   = core::ptr::null_mut();
            (*vma).rb_right  = core::ptr::null_mut();
            (*vma).rb_height = 1;
            return true;
        }
        let n = &mut *node;
        let vma_ref = &*vma;
        // Chevauchement : rejeter
        if vma_ref.start.as_u64() < n.end.as_u64() && vma_ref.end.as_u64() > n.start.as_u64() {
            return false;
        }
        if vma_ref.start.as_u64() < n.start.as_u64() {
            if !Self::insert_node(&mut n.rb_left, vma) { return false; }
        } else {
            if !Self::insert_node(&mut n.rb_right, vma) { return false; }
        }
        n.rb_height = 1 + Self::height(n.rb_left).max(Self::height(n.rb_right));
        Self::rebalance(root);
        true
    }

    unsafe fn remove_node(root: &mut *mut VmaDescriptor, start: VirtAddr) -> Option<*mut VmaDescriptor> {
        if (*root).is_null() { return None; }
        let n = &mut **root;
        if start.as_u64() < n.start.as_u64() {
            let res = Self::remove_node(&mut n.rb_left, start);
            if res.is_some() {
                n.rb_height = 1 + Self::height(n.rb_left).max(Self::height(n.rb_right));
                Self::rebalance(root);
            }
            res
        } else if start.as_u64() > n.start.as_u64() {
            let res = Self::remove_node(&mut n.rb_right, start);
            if res.is_some() {
                n.rb_height = 1 + Self::height(n.rb_left).max(Self::height(n.rb_right));
                Self::rebalance(root);
            }
            res
        } else {
            // Nœud trouvé
            let removed = *root;
            if n.rb_left.is_null() {
                *root = n.rb_right;
            } else if n.rb_right.is_null() {
                *root = n.rb_left;
            } else {
                // Trouver le successeur inorder (min du sous-arbre droit)
                let succ = Self::min_node(n.rb_right);
                // Swap succ et root
                (*succ).rb_left  = n.rb_left;
                (*succ).rb_right = Self::remove_min(n.rb_right);
                (*succ).rb_height = 1 + Self::height((*succ).rb_left).max(Self::height((*succ).rb_right));
                *root = succ;
                Self::rebalance(root);
            }
            (*removed).rb_left   = core::ptr::null_mut();
            (*removed).rb_right  = core::ptr::null_mut();
            (*removed).rb_height = 1;
            Some(removed)
        }
    }

    unsafe fn find_prev_node<'a>(mut node: *mut VmaDescriptor, addr: VirtAddr) -> Option<&'a VmaDescriptor> {
        let mut best: *mut VmaDescriptor = core::ptr::null_mut();
        while !node.is_null() {
            let n = &*node;
            if n.end.as_u64() <= addr.as_u64() {
                best = node;
                node = n.rb_right;
            } else {
                node = n.rb_left;
            }
        }
        if best.is_null() { None } else { Some(&*best) }
    }

    // ─── AVL helpers ─────────────────────────────────────────────────────────

    unsafe fn height(node: *mut VmaDescriptor) -> i32 {
        if node.is_null() { 0 } else { (*node).rb_height }
    }

    unsafe fn balance_factor(node: *mut VmaDescriptor) -> i32 {
        if node.is_null() { return 0; }
        Self::height((*node).rb_left) - Self::height((*node).rb_right)
    }

    unsafe fn update_height(node: *mut VmaDescriptor) {
        if !node.is_null() {
            (*node).rb_height = 1 + Self::height((*node).rb_left).max(Self::height((*node).rb_right));
        }
    }

    unsafe fn rotate_right(y: &mut *mut VmaDescriptor) {
        let y_ptr = *y;
        let x     = (*y_ptr).rb_left;
        (*y_ptr).rb_left      = (*x).rb_right;
        (*x).rb_right         = y_ptr;
        Self::update_height(y_ptr);
        Self::update_height(x);
        *y = x;
    }

    unsafe fn rotate_left(x: &mut *mut VmaDescriptor) {
        let x_ptr = *x;
        let y     = (*x_ptr).rb_right;
        (*x_ptr).rb_right = (*y).rb_left;
        (*y).rb_left      = x_ptr;
        Self::update_height(x_ptr);
        Self::update_height(y);
        *x = y;
    }

    unsafe fn rebalance(root: &mut *mut VmaDescriptor) {
        if root.is_null() { return; }
        let bf = Self::balance_factor(*root);
        if bf > 1 {
            if Self::balance_factor((**root).rb_left) < 0 {
                Self::rotate_left(&mut (**root).rb_left);
            }
            Self::rotate_right(root);
        } else if bf < -1 {
            if Self::balance_factor((**root).rb_right) > 0 {
                Self::rotate_right(&mut (**root).rb_right);
            }
            Self::rotate_left(root);
        }
    }

    unsafe fn min_node(mut node: *mut VmaDescriptor) -> *mut VmaDescriptor {
        while !(*node).rb_left.is_null() { node = (*node).rb_left; }
        node
    }

    unsafe fn remove_min(root: *mut VmaDescriptor) -> *mut VmaDescriptor {
        if (*root).rb_left.is_null() {
            return (*root).rb_right;
        }
        let mut r = root;
        (*r).rb_left  = Self::remove_min((*r).rb_left);
        (*r).rb_height = 1 + Self::height((*r).rb_left).max(Self::height((*r).rb_right));
        let rr = &mut r as *mut *mut VmaDescriptor;
        Self::rebalance(&mut *rr);
        r
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ITÉRATEUR IN-ORDER
// ─────────────────────────────────────────────────────────────────────────────

pub struct VmaTreeIter<'a> {
    stack: [*mut VmaDescriptor; 64],
    depth: usize,
    _marker: core::marker::PhantomData<&'a VmaDescriptor>,
}

impl<'a> VmaTreeIter<'a> {
    fn new(root: *mut VmaDescriptor, _stack: [*mut VmaDescriptor; 64], _depth: usize) -> Self {
        let mut iter = VmaTreeIter {
            stack:   [core::ptr::null_mut(); 64],
            depth:   0,
            _marker: core::marker::PhantomData,
        };
        // Pousser jusqu'au nœud le plus à gauche
        let mut cur = root;
        while !cur.is_null() && iter.depth < 64 {
            iter.stack[iter.depth] = cur;
            iter.depth += 1;
            // SAFETY: cur est un VmaDescriptor valide.
            cur = unsafe { (*cur).rb_left };
        }
        iter
    }
}

impl<'a> Iterator for VmaTreeIter<'a> {
    type Item = &'a VmaDescriptor;

    fn next(&mut self) -> Option<Self::Item> {
        if self.depth == 0 { return None; }
        self.depth -= 1;
        let node = self.stack[self.depth];
        // SAFETY: node est un VmaDescriptor valide.
        let result = unsafe { &*node };
        // Pousser le sous-arbre droit
        let mut cur = unsafe { (*node).rb_right };
        while !cur.is_null() && self.depth < 64 {
            self.stack[self.depth] = cur;
            self.depth += 1;
            cur = unsafe { (*cur).rb_left };
        }
        Some(result)
    }
}
