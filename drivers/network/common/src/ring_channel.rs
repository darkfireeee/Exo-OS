//! ring_channel.rs — Anneaux SPSC lock-free pour NetGrant ExoNet V5
//!
//! Fournit les anneaux d'échange lock-free Single-Producer Single-Consumer
//! pour TxGrantRing et RxGrantRing entre network_server et les démons de pilotes réseau.

use core::sync::atomic::{AtomicU32, Ordering};
use crate::netgrant::NetGrantDesc;

pub const GRANT_RING_DEFAULT_CAPACITY: usize = 128;

/// Anneau circulaire lock-free SPSC borné pour NetGrantDesc
#[repr(C)]
pub struct GrantRing<const CAP: usize> {
    head: AtomicU32,
    tail: AtomicU32,
    slots: [NetGrantDesc; CAP],
}

impl<const CAP: usize> GrantRing<CAP> {
    const _POWER_OF_TWO_CHECK: () = assert!(CAP > 0 && (CAP & (CAP - 1)) == 0, "GrantRing capacity must be a power of two");

    pub const fn new() -> Self {
        Self {
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            slots: [NetGrantDesc {
                slot_id: 0,
                generation_id: 0,
                offset: 0,
                len: 0,
                flags: 0,
                _reserved: 0,
            }; CAP],
        }
    }

    #[inline(always)]
    pub fn capacity(&self) -> usize {
        CAP
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.head.load(Ordering::Acquire) == self.tail.load(Ordering::Acquire)
    }

    #[inline]
    pub fn is_full(&self) -> bool {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        tail.wrapping_sub(head) as usize >= CAP
    }

    #[inline]
    pub fn len(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        tail.wrapping_sub(head) as usize
    }

    /// Enfile un descripteur (côté producteur unique).
    /// Retourne Ok(()) ou Err(desc) si l'anneau est saturé.
    pub fn push(&mut self, desc: NetGrantDesc) -> Result<(), NetGrantDesc> {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Relaxed);

        if tail.wrapping_sub(head) as usize >= CAP {
            return Err(desc);
        }

        let mask = (CAP - 1) as u32;
        let idx = (tail & mask) as usize;
        self.slots[idx] = desc;

        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    /// Défile un descripteur (côté consommateur unique).
    /// Retourne Some(desc) ou None si l'anneau est vide.
    pub fn pop(&mut self) -> Option<NetGrantDesc> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);

        if head == tail {
            return None;
        }

        let mask = (CAP - 1) as u32;
        let idx = (head & mask) as usize;
        let desc = self.slots[idx];

        self.head.store(head.wrapping_add(1), Ordering::Release);
        Some(desc)
    }
}

pub type TxGrantRing = GrantRing<GRANT_RING_DEFAULT_CAPACITY>;
pub type RxGrantRing = GrantRing<GRANT_RING_DEFAULT_CAPACITY>;
