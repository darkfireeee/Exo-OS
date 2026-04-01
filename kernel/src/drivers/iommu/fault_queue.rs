//! # drivers/iommu/fault_queue.rs
//!
//! Queue IOMMU fault basée sur CAS strong (FIX-104)
//! Source : ExoOS_Driver_Framework_v10.md §3.4 + GI-03_Drivers_IRQ_DMA.md
//!
//! Logique lock-free complète pour autoriser l'appel `push()` depuis ISR sans aucun Mutex.
//! 0 STUB, 0 TODO

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use core::cell::UnsafeCell;
use core::hint::spin_loop;

pub const IOMMU_QUEUE_CAPACITY: u32 = 256;

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct IommuFaultEvent {
    pub device_id: u16,
    pub fault_type: u8,
    pub domain_id: u32,
    pub faulted_addr: u64,
}

// ALIGNEMENT CACHE L1 (64 octets) pour éviter le faux partage (False Sharing) en SMP.
#[repr(C, align(64))]
pub struct FaultQueueSlot {
    pub seq: AtomicU32,
    pub event: UnsafeCell<IommuFaultEvent>,
}

unsafe impl Sync for FaultQueueSlot {}

// Padding ajouté pour aligner la tête et la queue sur des lignes de cache distinctes.
#[repr(C, align(64))]
pub struct IommuFaultQueue {
    pub head: AtomicU32,
    _pad_head: [u8; 60], // Assure que head et tail sont séparés
    pub tail: AtomicU32,
    _pad_tail: [u8; 60],
    pub dropped: AtomicU32,
    pub initialized: AtomicBool,
    pub slots: [FaultQueueSlot; IOMMU_QUEUE_CAPACITY as usize],
}

unsafe impl Sync for IommuFaultQueue {}

impl IommuFaultQueue {
    pub const fn new() -> Self {
        #[allow(clippy::declare_interior_mutable_const)]
        const INIT_SLOT: FaultQueueSlot = FaultQueueSlot {
            seq: AtomicU32::new(0),
            event: UnsafeCell::new(IommuFaultEvent {
                device_id: 0,
                fault_type: 0,
                domain_id: 0,
                faulted_addr: 0,
            }),
        };

        Self {
            head: AtomicU32::new(0),
            _pad_head: [0; 60],
            tail: AtomicU32::new(0),
            _pad_tail: [0; 60],
            dropped: AtomicU32::new(0),
            initialized: AtomicBool::new(false),
            slots: [INIT_SLOT; IOMMU_QUEUE_CAPACITY as usize],
        }
    }

    pub fn init(&self) {
        for (i, slot) in self.slots.iter().enumerate() {
            slot.seq.store(i as u32, Ordering::Release);
        }
        self.initialized.store(true, Ordering::Release);
    }

    pub fn push(&self, event: IommuFaultEvent) -> bool {
        // Mode release: on évite le panic si appelé avant init,
        // on se fie au drop silencieux spécifié.
        if !self.initialized.load(Ordering::Acquire) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }

        let mut pos = self.head.load(Ordering::Relaxed);
        loop {
            let idx = (pos % IOMMU_QUEUE_CAPACITY) as usize;
            let slot = &self.slots[idx];
            let seq = slot.seq.load(Ordering::Acquire);
            let dif = seq as i64 - pos as i64;

            if dif == 0 {
                match self.head.compare_exchange(
                    pos, pos + 1, Ordering::AcqRel, Ordering::Relaxed
                ) {
                    Ok(_) => {
                        unsafe { *slot.event.get() = event; }
                        slot.seq.store(pos + 1, Ordering::Release);
                        return true;
                    }
                    Err(actual) => {
                        pos = actual;
                        spin_loop();
                    }
                }
            } else if dif < 0 {
                // Queue full
                self.dropped.fetch_add(1, Ordering::Relaxed);
                return false;
            } else {
                pos = self.head.load(Ordering::Relaxed);
                spin_loop();
            }
        }
    }

    pub fn pop(&self) -> Option<IommuFaultEvent> {
        let mut pos = self.tail.load(Ordering::Relaxed);
        loop {
            let idx = (pos % IOMMU_QUEUE_CAPACITY) as usize;
            let slot = &self.slots[idx];
            let seq = slot.seq.load(Ordering::Acquire);
            let dif = seq as i64 - (pos + 1) as i64;

            if dif == 0 {
                match self.tail.compare_exchange(
                    pos, pos + 1, Ordering::AcqRel, Ordering::Relaxed
                ) {
                    Ok(_) => {
                        let event = unsafe { *slot.event.get() };
                        slot.seq.store(pos + IOMMU_QUEUE_CAPACITY, Ordering::Release);
                        return Some(event);
                    }
                    Err(actual) => {
                        pos = actual;
                        spin_loop();
                    }
                }
            } else if dif < 0 {
                // Queue empty
                return None;
            } else {
                pos = self.tail.load(Ordering::Relaxed);
                spin_loop();
            }
        }
    }
}

pub static IOMMU_FAULT_QUEUE: IommuFaultQueue = IommuFaultQueue::new();
