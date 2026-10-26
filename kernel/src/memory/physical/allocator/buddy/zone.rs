//! # memory/physical/allocator/buddy/zone.rs — Allocateur Buddy par zone mémoire
//!
//! Gère les free-lists par ordre (0..=BUDDY_MAX_ORDER), les fusions (coalescing),
//! les divisions (split), et le bitmap de disponibilité pour une zone physique donnée.

use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use spin::Mutex;

use crate::memory::core::{
    is_aligned, AllocError, AllocFlags, Frame, PhysAddr, ZoneType, BUDDY_MAX_ORDER,
    BUDDY_ORDER_COUNT, PAGE_SIZE,
};
use crate::memory::physical::frame::descriptor::{FrameFlags, FRAME_DESCRIPTORS};
use super::node::{
    phys_to_virt_buddy, virt_to_phys_buddy, zero_pages, BuddyOrderLevel, FreeNode,
};
use super::stats::BuddyZoneStats;

pub struct BuddyZoneInner {
    /// Free-lists par ordre (0..=BUDDY_MAX_ORDER).
    pub orders: [BuddyOrderLevel; BUDDY_ORDER_COUNT],
    /// Bitmap de disponibilité par frame (1 bit par frame).
    pub bitmap_ptr: *mut u64,
    pub bitmap_words: usize,
}

// SAFETY: BuddyZoneInner n'est accessible que sous le spinlock de BuddyZone.
unsafe impl Send for BuddyZoneInner {}

impl Default for BuddyZoneInner {
    fn default() -> Self {
        Self::new()
    }
}

impl BuddyZoneInner {
    pub const fn new() -> Self {
        BuddyZoneInner {
            orders: [const { BuddyOrderLevel::new() }; BUDDY_ORDER_COUNT],
            bitmap_ptr: null_mut(),
            bitmap_words: 0,
        }
    }

    /// Marque le frame `pfn` comme libre dans le bitmap.
    ///
    /// # Safety
    /// `self.bitmap_ptr` doit pointer vers un buffer valide d'au moins `bitmap_words` mots `u64`.
    #[inline(always)]
    pub unsafe fn bitmap_clear(&self, pfn: usize) {
        if self.bitmap_ptr.is_null() {
            return;
        }
        let word = pfn / 64;
        let bit = pfn % 64;
        if word < self.bitmap_words {
            (*self.bitmap_ptr.add(word)) &= !(1u64 << bit);
        }
    }

    /// Marque le frame `pfn` comme alloué dans le bitmap.
    ///
    /// # Safety
    /// `self.bitmap_ptr` doit pointer vers un buffer valide d'au moins `bitmap_words` mots `u64`.
    #[inline(always)]
    pub unsafe fn bitmap_set(&self, pfn: usize) {
        if self.bitmap_ptr.is_null() {
            return;
        }
        let word = pfn / 64;
        let bit = pfn % 64;
        if word < self.bitmap_words {
            (*self.bitmap_ptr.add(word)) |= 1u64 << bit;
        }
    }

    /// Retourne `true` si le frame `pfn` est libre dans le bitmap.
    ///
    /// # Safety
    /// `self.bitmap_ptr` doit être valide pour la lecture si non null.
    #[inline(always)]
    pub unsafe fn bitmap_is_free(&self, pfn: usize) -> bool {
        if self.bitmap_ptr.is_null() {
            return false;
        }
        let word = pfn / 64;
        let bit = pfn % 64;
        if word >= self.bitmap_words {
            return false;
        }
        ((*self.bitmap_ptr.add(word)) & (1u64 << bit)) == 0
    }

    /// Retourne `true` si le frame `pfn` est marqué alloué dans le bitmap.
    ///
    /// # Safety
    /// `self.bitmap_ptr` doit être valide pour la lecture si non null.
    #[inline(always)]
    pub unsafe fn bitmap_is_allocated(&self, pfn: usize) -> bool {
        if self.bitmap_ptr.is_null() {
            return false;
        }
        let word = pfn / 64;
        let bit = pfn % 64;
        if word >= self.bitmap_words {
            return false;
        }
        ((*self.bitmap_ptr.add(word)) & (1u64 << bit)) != 0
    }

    /// Vérifie si une plage de frames est libre dans le bitmap.
    ///
    /// # Safety
    /// `self.bitmap_ptr` doit être valide pour la lecture de la plage si non null.
    #[inline(always)]
    pub unsafe fn bitmap_range_is_free(&self, pfn: usize, count: usize) -> bool {
        (0..count).all(|i| self.bitmap_is_free(pfn + i))
    }

    /// Vérifie si une plage de frames est allouée dans le bitmap.
    ///
    /// # Safety
    /// `self.bitmap_ptr` doit être valide pour la lecture de la plage si non null.
    #[inline(always)]
    pub unsafe fn bitmap_range_is_allocated(&self, pfn: usize, count: usize) -> bool {
        (0..count).all(|i| self.bitmap_is_allocated(pfn + i))
    }
}

#[inline(always)]
pub fn reserve_frame_descriptor(frame: Frame) {
    let desc = FRAME_DESCRIPTORS.get(frame);
    desc.set_flag(FrameFlags::PINNED);
    desc.set_flag(FrameFlags::RESERVED);
    desc.clear_flag(FrameFlags::FREE);
}

#[inline(always)]
pub fn unreserve_frame_descriptor(frame: Frame) {
    let desc = FRAME_DESCRIPTORS.get(frame);
    desc.clear_flag(FrameFlags::PINNED);
    desc.clear_flag(FrameFlags::RESERVED);
}

/// Allocateur buddy pour une zone mémoire donnée.
pub struct BuddyZone {
    pub zone_type: ZoneType,
    pub numa_node: u8,
    pub phys_start: PhysAddr,
    pub phys_end: PhysAddr,
    pub total_frames: usize,
    pub inner: Mutex<BuddyZoneInner>,
    pub free_frames: AtomicUsize,
    pub alloc_count: AtomicUsize,
    pub free_count: AtomicUsize,
    pub fail_count: AtomicUsize,
    pub split_count: AtomicUsize,
    pub merge_count: AtomicUsize,
    pub initialized: AtomicBool,
}

impl BuddyZone {
    pub const fn new_uninit() -> Self {
        BuddyZone {
            zone_type: ZoneType::Normal,
            numa_node: 0,
            phys_start: PhysAddr::new(0),
            phys_end: PhysAddr::new(0),
            total_frames: 0,
            inner: Mutex::new(BuddyZoneInner::new()),
            free_frames: AtomicUsize::new(0),
            alloc_count: AtomicUsize::new(0),
            free_count: AtomicUsize::new(0),
            fail_count: AtomicUsize::new(0),
            split_count: AtomicUsize::new(0),
            merge_count: AtomicUsize::new(0),
            initialized: AtomicBool::new(false),
        }
    }

    /// Initialise la zone buddy.
    ///
    /// # Safety
    /// `bitmap_buf` doit être valide pour écrire `bitmap_words` mots `u64`, et l'appelant
    /// doit détenir un accès exclusif à `self` pendant l'initialisation.
    pub unsafe fn init(
        &self,
        zone_type: ZoneType,
        numa_node: u8,
        phys_start: PhysAddr,
        phys_end: PhysAddr,
        bitmap_buf: *mut u64,
        bitmap_words: usize,
    ) {
        let s = self as *const BuddyZone as *mut BuddyZone;
        (*s).zone_type = zone_type;
        (*s).numa_node = numa_node;
        (*s).phys_start = phys_start;
        (*s).phys_end = phys_end;
        (*s).total_frames = ((phys_end.as_u64() - phys_start.as_u64()) / PAGE_SIZE as u64) as usize;

        let mut inner = self.inner.lock();
        for order in &mut inner.orders {
            order.init();
        }
        inner.bitmap_ptr = bitmap_buf;
        inner.bitmap_words = bitmap_words;

        if !bitmap_buf.is_null() {
            for i in 0..bitmap_words {
                *bitmap_buf.add(i) = u64::MAX;
            }
        }

        drop(inner);
        self.initialized.store(true, Ordering::Release);
    }

    /// Ajoute une plage de frames libres dans la zone buddy.
    ///
    /// # Safety
    /// Les frames de `first_pfn` à `last_pfn` doivent être de la mémoire physique réelle
    /// et libre appartenant à cette zone.
    pub unsafe fn add_free_range(&self, first_pfn: usize, last_pfn: usize) {
        debug_assert!(self.initialized.load(Ordering::Acquire));
        debug_assert!(first_pfn < last_pfn);

        let mut pfn = first_pfn;
        while pfn < last_pfn {
            let remaining = last_pfn - pfn;
            let max_order_by_align = if pfn == 0 {
                BUDDY_MAX_ORDER
            } else {
                pfn.trailing_zeros() as usize
            };
            let max_order_by_count = (usize::BITS - remaining.leading_zeros() - 1) as usize;
            let mut order = max_order_by_align
                .min(max_order_by_count)
                .min(BUDDY_MAX_ORDER);

            loop {
                let count = 1usize << order;
                let still_unclaimed = {
                    let inner = self.inner.lock();
                    unsafe { inner.bitmap_range_is_allocated(pfn, count) }
                };

                if still_unclaimed {
                    let phys =
                        PhysAddr::new(self.phys_start.as_u64() + pfn as u64 * PAGE_SIZE as u64);
                    self.add_free_block(phys, order);
                    pfn += count;
                    break;
                }

                if order == 0 {
                    pfn += 1;
                    break;
                }
                order -= 1;
            }
        }
    }

    unsafe fn add_free_block(&self, phys: PhysAddr, order: usize) {
        if order > BUDDY_MAX_ORDER || !self.contains_aligned_block(phys, order) {
            self.fail_count.fetch_add(1, Ordering::Relaxed);
            return;
        }

        let mut inner = self.inner.lock();
        let pfn = ((phys.as_u64() - self.phys_start.as_u64()) / PAGE_SIZE as u64) as usize;
        let count = 1usize << order;
        if unsafe { !inner.bitmap_range_is_allocated(pfn, count) } {
            return;
        }

        let node_ptr = phys_to_virt_buddy(phys) as *mut FreeNode;
        FreeNode::init(node_ptr, order as u8);
        FreeNode::insert_after(&mut inner.orders[order].head as *mut FreeNode, node_ptr);
        inner.orders[order].count.fetch_add(1, Ordering::Relaxed);

        for i in 0..count {
            inner.bitmap_clear(pfn + i);
        }
        self.mark_descriptors_free(pfn, count);
        self.free_frames.fetch_add(count, Ordering::Relaxed);
    }

    pub fn alloc_pages(&self, order: usize, flags: AllocFlags) -> Result<Frame, AllocError> {
        if order > BUDDY_MAX_ORDER {
            self.fail_count.fetch_add(1, Ordering::Relaxed);
            return Err(AllocError::InvalidParams);
        }

        if !self.initialized.load(Ordering::Acquire) {
            return Err(AllocError::NotInitialized);
        }

        let result = {
            let mut inner = self.inner.lock();
            self.alloc_inner(&mut inner, order, flags)
        };

        match result {
            Ok(phys) => {
                self.free_frames.fetch_sub(1 << order, Ordering::Relaxed);
                self.alloc_count.fetch_add(1, Ordering::Relaxed);
                if flags.contains(AllocFlags::ZEROED) {
                    unsafe {
                        zero_pages(phys, order);
                    }
                }
                let frame = Frame::containing(phys);
                if flags.contains(AllocFlags::DMA) || flags.contains(AllocFlags::DMA32) {
                    FRAME_DESCRIPTORS
                        .get(frame)
                        .set_flag(FrameFlags::DMA_PINNED);
                }
                Ok(frame)
            }
            Err(e) => {
                self.fail_count.fetch_add(1, Ordering::Relaxed);
                Err(e)
            }
        }
    }

    fn alloc_inner(
        &self,
        inner: &mut BuddyZoneInner,
        order: usize,
        _flags: AllocFlags,
    ) -> Result<PhysAddr, AllocError> {
        for current_order in order..=BUDDY_MAX_ORDER {
            unsafe {
                let head = &mut inner.orders[current_order].head as *mut FreeNode;
                match FreeNode::pop(head) {
                    None => continue,
                    Some(node) => {
                        inner.orders[current_order]
                            .count
                            .fetch_sub(1, Ordering::Relaxed);

                        let phys = virt_to_phys_buddy(node as usize);
                        if !self.contains_aligned_block(phys, current_order) {
                            self.fail_count.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                        let popped_pfn = self.phys_to_pfn(phys);
                        if self.descriptors_range_has_reserved(popped_pfn, 1usize << current_order)
                        {
                            self.fail_count.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                        if !inner.bitmap_range_is_free(popped_pfn, 1usize << current_order) {
                            self.fail_count.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                        if !self.descriptors_range_is_free(popped_pfn, 1usize << current_order) {
                            self.fail_count.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }

                        let block_phys = phys;
                        for split_order in (order..current_order).rev() {
                            let buddy_phys = PhysAddr::new(
                                block_phys.as_u64() + (PAGE_SIZE << split_order) as u64,
                            );

                            let buddy_node = phys_to_virt_buddy(buddy_phys) as *mut FreeNode;
                            FreeNode::init(buddy_node, split_order as u8);
                            let buddy_head = &mut inner.orders[split_order].head as *mut FreeNode;
                            FreeNode::insert_after(buddy_head, buddy_node);
                            inner.orders[split_order]
                                .count
                                .fetch_add(1, Ordering::Relaxed);

                            let buddy_pfn = self.phys_to_pfn(buddy_phys);
                            for i in 0..(1usize << split_order) {
                                inner.bitmap_clear(buddy_pfn + i);
                            }

                            self.split_count.fetch_add(1, Ordering::Relaxed);
                        }

                        let pfn = self.phys_to_pfn(block_phys);
                        for i in 0..(1usize << order) {
                            inner.bitmap_set(pfn + i);
                        }
                        self.mark_descriptors_allocated(pfn, 1usize << order);

                        return Ok(block_phys);
                    }
                }
            }
        }

        Err(AllocError::OutOfMemory)
    }

    pub fn free_pages(&self, frame: Frame, order: usize) -> Result<(), AllocError> {
        if order > BUDDY_MAX_ORDER {
            self.fail_count.fetch_add(1, Ordering::Relaxed);
            return Err(AllocError::InvalidParams);
        }

        if !self.initialized.load(Ordering::Acquire) {
            return Err(AllocError::NotInitialized);
        }

        let phys = frame.start_address();
        if !self.contains_block(phys, order) {
            self.fail_count.fetch_add(1, Ordering::Relaxed);
            return Err(AllocError::InvalidParams);
        }
        let zone_relative = phys.as_u64().saturating_sub(self.phys_start.as_u64()) as usize;
        if !is_aligned(zone_relative, PAGE_SIZE << order) {
            self.fail_count.fetch_add(1, Ordering::Relaxed);
            return Err(AllocError::InvalidParams);
        }

        {
            let pfn_check = self.phys_to_pfn(phys);
            let count = 1usize << order;
            for k in 0..count {
                let desc = FRAME_DESCRIPTORS.get(self.pfn_to_frame(pfn_check + k));
                if desc.flags().contains(FrameFlags::RESERVED) {
                    self.fail_count.fetch_add(1, Ordering::Relaxed);
                    return Err(AllocError::InvalidParams);
                }
            }
        }

        {
            let mut inner = self.inner.lock();
            let pfn = self.phys_to_pfn(phys);
            if unsafe { !inner.bitmap_range_is_allocated(pfn, 1usize << order) } {
                self.fail_count.fetch_add(1, Ordering::Relaxed);
                return Err(AllocError::InvalidParams);
            }
            if !self.descriptors_range_is_allocated(pfn, 1usize << order) {
                self.fail_count.fetch_add(1, Ordering::Relaxed);
                return Err(AllocError::InvalidParams);
            }
            self.mark_descriptors_free(pfn, 1usize << order);
            unsafe {
                self.free_inner(&mut inner, phys, order);
            }
        }

        self.free_frames.fetch_add(1 << order, Ordering::Relaxed);
        self.free_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    unsafe fn free_inner(&self, inner: &mut BuddyZoneInner, phys: PhysAddr, order: usize) {
        let mut current_phys = phys;
        let mut current_order = order;

        let pfn = self.phys_to_pfn(current_phys);
        for i in 0..(1usize << order) {
            inner.bitmap_clear(pfn + i);
        }

        while current_order < BUDDY_MAX_ORDER {
            let buddy_phys = self.buddy_of(current_phys, current_order);

            if buddy_phys < self.phys_start || buddy_phys >= self.phys_end {
                break;
            }

            let buddy_pfn = self.phys_to_pfn(buddy_phys);
            let buddy_free =
                (0..(1usize << current_order)).all(|i| inner.bitmap_is_free(buddy_pfn + i));

            if !buddy_free {
                break;
            }

            let buddy_node = phys_to_virt_buddy(buddy_phys) as *mut FreeNode;
            if (*buddy_node).order != current_order as u8 {
                break;
            }
            FreeNode::remove(buddy_node);
            inner.orders[current_order]
                .count
                .fetch_sub(1, Ordering::Relaxed);

            current_phys = if current_phys < buddy_phys {
                current_phys
            } else {
                buddy_phys
            };
            current_order += 1;
            self.merge_count.fetch_add(1, Ordering::Relaxed);
        }

        let node_ptr = phys_to_virt_buddy(current_phys) as *mut FreeNode;
        FreeNode::init(node_ptr, current_order as u8);
        let head = &mut inner.orders[current_order].head as *mut FreeNode;
        FreeNode::insert_after(head, node_ptr);
        inner.orders[current_order]
            .count
            .fetch_add(1, Ordering::Relaxed);
    }

    #[inline(always)]
    fn buddy_of(&self, phys: PhysAddr, order: usize) -> PhysAddr {
        let relative = phys.as_u64() - self.phys_start.as_u64();
        let buddy_relative = relative ^ ((PAGE_SIZE << order) as u64);
        PhysAddr::new(self.phys_start.as_u64() + buddy_relative)
    }

    #[inline(always)]
    fn contains_block(&self, phys: PhysAddr, order: usize) -> bool {
        if order > BUDDY_MAX_ORDER || phys < self.phys_start {
            return false;
        }
        let bytes = (PAGE_SIZE as u64) << order;
        match phys.as_u64().checked_add(bytes) {
            Some(end) => end <= self.phys_end.as_u64(),
            None => false,
        }
    }

    #[inline(always)]
    fn contains_aligned_block(&self, phys: PhysAddr, order: usize) -> bool {
        if !self.contains_block(phys, order) {
            return false;
        }
        let zone_relative = phys.as_u64().saturating_sub(self.phys_start.as_u64()) as usize;
        is_aligned(zone_relative, PAGE_SIZE << order)
    }

    #[inline(always)]
    fn phys_to_pfn(&self, phys: PhysAddr) -> usize {
        ((phys.as_u64() - self.phys_start.as_u64()) / PAGE_SIZE as u64) as usize
    }

    #[inline(always)]
    fn pfn_to_frame(&self, pfn: usize) -> Frame {
        Frame::containing(PhysAddr::new(
            self.phys_start.as_u64() + pfn as u64 * PAGE_SIZE as u64,
        ))
    }

    #[inline(always)]
    fn descriptors_range_is_free(&self, pfn: usize, count: usize) -> bool {
        (0..count).all(|i| {
            let flags = FRAME_DESCRIPTORS.get(self.pfn_to_frame(pfn + i)).flags();
            flags.contains(FrameFlags::FREE) && !flags.contains(FrameFlags::RESERVED)
        })
    }

    #[inline(always)]
    fn descriptors_range_is_allocated(&self, pfn: usize, count: usize) -> bool {
        (0..count).all(|i| {
            !FRAME_DESCRIPTORS
                .get(self.pfn_to_frame(pfn + i))
                .flags()
                .contains(FrameFlags::FREE)
        })
    }

    #[inline(always)]
    fn descriptors_range_has_reserved(&self, pfn: usize, count: usize) -> bool {
        (0..count).any(|i| {
            FRAME_DESCRIPTORS
                .get(self.pfn_to_frame(pfn + i))
                .flags()
                .contains(FrameFlags::RESERVED)
        })
    }

    #[inline(always)]
    fn mark_descriptors_free(&self, pfn: usize, count: usize) {
        for i in 0..count {
            FRAME_DESCRIPTORS
                .get(self.pfn_to_frame(pfn + i))
                .swap_flags(FrameFlags::FREE);
        }
    }

    #[inline(always)]
    fn mark_descriptors_allocated(&self, pfn: usize, count: usize) {
        for i in 0..count {
            FRAME_DESCRIPTORS
                .get(self.pfn_to_frame(pfn + i))
                .clear_flag(FrameFlags::FREE);
        }
    }

    #[inline(always)]
    pub fn free_frames(&self) -> usize {
        self.free_frames.load(Ordering::Relaxed)
    }

    #[inline(always)]
    pub fn total_frames_count(&self) -> usize {
        self.total_frames
    }

    pub fn stats(&self) -> BuddyZoneStats {
        let inner = self.inner.lock();
        let mut free_by_order = [0usize; BUDDY_ORDER_COUNT];
        for (i, order) in inner.orders.iter().enumerate() {
            free_by_order[i] = order.count.load(Ordering::Relaxed);
        }
        BuddyZoneStats {
            zone_type: self.zone_type,
            numa_node: self.numa_node,
            total_frames: self.total_frames,
            free_frames: self.free_frames.load(Ordering::Relaxed),
            alloc_count: self.alloc_count.load(Ordering::Relaxed),
            free_count: self.free_count.load(Ordering::Relaxed),
            fail_count: self.fail_count.load(Ordering::Relaxed),
            split_count: self.split_count.load(Ordering::Relaxed),
            merge_count: self.merge_count.load(Ordering::Relaxed),
            free_by_order,
        }
    }

    #[inline(always)]
    pub fn manages_zone(&self, zone: ZoneType) -> bool {
        self.zone_type == zone
    }

    #[inline(always)]
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }
}
