// kernel/src/memory/virtual/vma/tree.rs
//
// AVL tree of VMAs — indexing structure by virtual address.
// Allows finding the VMA containing an address in O(log n).
// Maximum size: MAX_VMAS_PER_PROCESS VMAs per process.
// Layer 0 — no external dependency except `spin`.

use super::cow::mark_vma_cow;
use super::descriptor::{VmaDescriptor, VmaFlags};
use crate::memory::core::VirtAddr;
use alloc::alloc::{alloc, Layout};
use alloc::boxed::Box;

// ─────────────────────────────────────────────────────────────────────────────
// VMA AVL TREE
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum number of VMAs per process (kernel limit).
pub const MAX_VMAS_PER_PROCESS: usize = 65536;

/// AVL tree of VMAs for an address space.
///
/// Nodes are pointers to VmaDescriptors allocated on the kernel heap.
/// An inserted VMA belongs to the tree until `remove()` or `Drop`.
pub struct VmaTree {
    root: *mut VmaDescriptor,
    count: usize,
}

// SAFETY: VmaTree is protected by the parent address space lock.
unsafe impl Send for VmaTree {}
unsafe impl Sync for VmaTree {}

impl Default for VmaTree {
    fn default() -> Self {
        Self::new()
    }
}

impl VmaTree {
    pub const fn new() -> Self {
        VmaTree {
            root: core::ptr::null_mut(),
            count: 0,
        }
    }

    /// Number of VMAs in the tree.
    pub fn len(&self) -> usize {
        self.count
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Finds the VMA containing `addr`.
    ///
    /// Returns a pointer to the VmaDescriptor or `None`.
    pub fn find(&self, addr: VirtAddr) -> Option<&VmaDescriptor> {
        // SAFETY: All nodes in the tree are valid VmaDescriptors.
        unsafe { Self::find_node(self.root, addr) }
    }

    /// Finds the VMA containing `addr` (mutable).
    pub fn find_mut(&mut self, addr: VirtAddr) -> Option<&mut VmaDescriptor> {
        // SAFETY: Exclusive access (parent lock).
        unsafe { Self::find_node_mut(self.root, addr) }
    }

    /// Inserts a new VMA descriptor into the tree.
    ///
    /// Returns `false` if an existing VMA overlaps the range.
    ///
    /// # Safety
    /// `vma` must point to a valid VmaDescriptor not already
    /// inserted into another tree.
    pub unsafe fn insert(&mut self, vma: *mut VmaDescriptor) -> bool {
        if self.count >= MAX_VMAS_PER_PROCESS {
            return false;
        }
        if self.root.is_null() {
            self.root = vma;
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

    /// Removes a VMA descriptor from the tree (by start address).
    ///
    /// Returns a pointer to the removed descriptor, or `None`.
    pub fn remove(&mut self, start: VirtAddr) -> Option<*mut VmaDescriptor> {
        // SAFETY: Exclusive access.
        let removed = unsafe { Self::remove_node(&mut self.root, start) };
        if removed.is_some() {
            self.count = self.count.saturating_sub(1);
        }
        removed
    }

    /// Finds the VMA immediately preceding `addr` (largest `end` <= `addr`).
    pub fn find_prev(&self, addr: VirtAddr) -> Option<&VmaDescriptor> {
        // SAFETY: root points into the static pool of VmaDescriptors; no active &mut alias.
        unsafe { Self::find_prev_node(self.root, addr) }
    }

    /// Iterates over all VMAs in ascending order.
    pub fn iter(&self) -> VmaTreeIter<'_> {
        let stack = [core::ptr::null_mut::<VmaDescriptor>(); 64];
        let depth = 0usize;
        VmaTreeIter::new(self.root, stack, depth)
    }

    /// Applies `f` to each VMA under exclusive access.
    pub fn for_each_mut<F>(&mut self, mut f: F)
    where
        F: FnMut(&mut VmaDescriptor),
    {
        // SAFETY: Caller holds `&mut self`, so no other access to nodes.
        unsafe { Self::for_each_mut_node(self.root, &mut f) };
    }

    /// Clones VMA metadata for fork().
    ///
    /// DONTCOPY VMAs are not inherited. Private writable VMAs are
    /// marked COW in the child to remain consistent with shared PTEs.
    pub fn clone_for_fork(&self) -> Option<VmaTree> {
        let mut new_tree = VmaTree::new();
        for vma in self.iter() {
            if vma.flags.contains(VmaFlags::DONTCOPY) {
                continue;
            }

            let mut cloned = vma.clone_metadata();
            mark_vma_cow(&mut cloned);

            let raw = match alloc_vma_descriptor(cloned) {
                Some(raw) => raw,
                None => {
                    new_tree.clear_owned();
                    return None;
                }
            };

            // SAFETY: raw comes from a unique allocation and is not in any tree.
            if !unsafe { new_tree.insert(raw) } {
                free_vma_descriptor(raw);
                new_tree.clear_owned();
                return None;
            }
        }
        Some(new_tree)
    }

    fn clear_owned(&mut self) {
        // SAFETY: `self` owns all nodes still attached to `root`.
        unsafe { drop_subtree(self.root) };
        self.root = core::ptr::null_mut();
        self.count = 0;
    }

    // ─── AVL tree helpers ────────────────────────────────────────────────────

    unsafe fn find_node<'a>(
        mut node: *mut VmaDescriptor,
        addr: VirtAddr,
    ) -> Option<&'a VmaDescriptor> {
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

    unsafe fn find_node_mut<'a>(
        mut node: *mut VmaDescriptor,
        addr: VirtAddr,
    ) -> Option<&'a mut VmaDescriptor> {
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
            (*vma).rb_left = core::ptr::null_mut();
            (*vma).rb_right = core::ptr::null_mut();
            (*vma).rb_height = 1;
            return true;
        }
        let n = &mut *node;
        let vma_ref = &*vma;
        // Overlap: reject
        if vma_ref.start.as_u64() < n.end.as_u64() && vma_ref.end.as_u64() > n.start.as_u64() {
            return false;
        }
        if vma_ref.start.as_u64() < n.start.as_u64() {
            if !Self::insert_node(&mut n.rb_left, vma) {
                return false;
            }
        } else {
            if !Self::insert_node(&mut n.rb_right, vma) {
                return false;
            }
        }
        n.rb_height = 1 + Self::height(n.rb_left).max(Self::height(n.rb_right));
        Self::rebalance(root);
        true
    }

    unsafe fn remove_node(
        root: &mut *mut VmaDescriptor,
        start: VirtAddr,
    ) -> Option<*mut VmaDescriptor> {
        if (*root).is_null() {
            return None;
        }
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
            // Node found
            let removed = *root;
            if n.rb_left.is_null() {
                *root = n.rb_right;
            } else if n.rb_right.is_null() {
                *root = n.rb_left;
            } else {
                // Two children: find inorder successor (min of right subtree)
                let succ = Self::min_node(n.rb_right);
                let right_sub = Self::remove_min(n.rb_right);
                (*succ).rb_left = n.rb_left;
                (*succ).rb_right = right_sub;
                (*succ).rb_height =
                    1 + Self::height((*succ).rb_left).max(Self::height((*succ).rb_right));
                *root = succ;
            }
            if !(*root).is_null() {
                let r = &mut **root;
                r.rb_height = 1 + Self::height(r.rb_left).max(Self::height(r.rb_right));
                Self::rebalance(root);
            }
            (*removed).rb_left = core::ptr::null_mut();
            (*removed).rb_right = core::ptr::null_mut();
            (*removed).rb_height = 1;
            Some(removed)
        }
    }

    unsafe fn find_prev_node<'a>(
        mut node: *mut VmaDescriptor,
        addr: VirtAddr,
    ) -> Option<&'a VmaDescriptor> {
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
        if best.is_null() {
            None
        } else {
            Some(&*best)
        }
    }

    unsafe fn for_each_mut_node<F>(node: *mut VmaDescriptor, f: &mut F)
    where
        F: FnMut(&mut VmaDescriptor),
    {
        if node.is_null() {
            return;
        }
        Self::for_each_mut_node((*node).rb_left, f);
        f(&mut *node);
        Self::for_each_mut_node((*node).rb_right, f);
    }

    // ─── AVL helpers ─────────────────────────────────────────────────────────

    unsafe fn height(node: *mut VmaDescriptor) -> i32 {
        if node.is_null() {
            0
        } else {
            (*node).rb_height
        }
    }

    unsafe fn balance_factor(node: *mut VmaDescriptor) -> i32 {
        if node.is_null() {
            return 0;
        }
        Self::height((*node).rb_left) - Self::height((*node).rb_right)
    }

    unsafe fn update_height(node: *mut VmaDescriptor) {
        if !node.is_null() {
            (*node).rb_height =
                1 + Self::height((*node).rb_left).max(Self::height((*node).rb_right));
        }
    }

    unsafe fn rotate_right(y: &mut *mut VmaDescriptor) {
        let y_ptr = *y;
        let x = (*y_ptr).rb_left;
        (*y_ptr).rb_left = (*x).rb_right;
        (*x).rb_right = y_ptr;
        Self::update_height(y_ptr);
        Self::update_height(x);
        *y = x;
    }

    unsafe fn rotate_left(x: &mut *mut VmaDescriptor) {
        let x_ptr = *x;
        let y = (*x_ptr).rb_right;
        (*x_ptr).rb_right = (*y).rb_left;
        (*y).rb_left = x_ptr;
        Self::update_height(x_ptr);
        Self::update_height(y);
        *x = y;
    }

    unsafe fn rebalance(root: &mut *mut VmaDescriptor) {
        if root.is_null() {
            return;
        }
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
        while !(*node).rb_left.is_null() {
            node = (*node).rb_left;
        }
        node
    }

    unsafe fn remove_min(root: *mut VmaDescriptor) -> *mut VmaDescriptor {
        if (*root).rb_left.is_null() {
            return (*root).rb_right;
        }
        let mut r = root;
        (*r).rb_left = Self::remove_min((*r).rb_left);
        (*r).rb_height = 1 + Self::height((*r).rb_left).max(Self::height((*r).rb_right));
        let rr = &mut r as *mut *mut VmaDescriptor;
        Self::rebalance(&mut *rr);
        r
    }
}

impl Drop for VmaTree {
    fn drop(&mut self) {
        self.clear_owned();
    }
}

fn alloc_vma_descriptor(desc: VmaDescriptor) -> Option<*mut VmaDescriptor> {
    let layout = Layout::new::<VmaDescriptor>();
    let raw = unsafe { alloc(layout) as *mut VmaDescriptor };
    if raw.is_null() {
        return None;
    }
    // SAFETY: `raw` is unique and large enough for VmaDescriptor.
    unsafe {
        raw.write(desc);
    }
    Some(raw)
}

fn free_vma_descriptor(ptr: *mut VmaDescriptor) {
    if ptr.is_null() {
        return;
    }
    // SAFETY: ptr comes from Box::into_raw or alloc_vma_descriptor.
    unsafe {
        drop(Box::from_raw(ptr));
    }
}

unsafe fn drop_subtree(node: *mut VmaDescriptor) {
    if node.is_null() {
        return;
    }
    let left = (*node).rb_left;
    let right = (*node).rb_right;
    drop_subtree(left);
    drop_subtree(right);
    (*node).rb_left = core::ptr::null_mut();
    (*node).rb_right = core::ptr::null_mut();
    (*node).rb_height = 1;
    free_vma_descriptor(node);
}

// ─────────────────────────────────────────────────────────────────────────────
// IN-ORDER ITERATOR
// ─────────────────────────────────────────────────────────────────────────────

pub struct VmaTreeIter<'a> {
    stack: [*mut VmaDescriptor; 64],
    depth: usize,
    _marker: core::marker::PhantomData<&'a VmaDescriptor>,
}

impl<'a> VmaTreeIter<'a> {
    fn new(root: *mut VmaDescriptor, _stack: [*mut VmaDescriptor; 64], _depth: usize) -> Self {
        let mut iter = VmaTreeIter {
            stack: [core::ptr::null_mut(); 64],
            depth: 0,
            _marker: core::marker::PhantomData,
        };
        // Push down to the leftmost node
        let mut cur = root;
        while !cur.is_null() && iter.depth < 64 {
            iter.stack[iter.depth] = cur;
            iter.depth += 1;
            // SAFETY: cur is a valid VmaDescriptor.
            cur = unsafe { (*cur).rb_left };
        }
        iter
    }
}

impl<'a> Iterator for VmaTreeIter<'a> {
    type Item = &'a VmaDescriptor;

    fn next(&mut self) -> Option<Self::Item> {
        if self.depth == 0 {
            return None;
        }
        self.depth -= 1;
        let node = self.stack[self.depth];
        // SAFETY: node is a valid VmaDescriptor.
        let result = unsafe { &*node };
        // Push the right subtree
        let mut cur = unsafe { (*node).rb_right };
        while !cur.is_null() && self.depth < 64 {
            self.stack[self.depth] = cur;
            self.depth += 1;
            cur = unsafe { (*cur).rb_left };
        }
        Some(result)
    }
}
