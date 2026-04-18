//! # drivers/device_server_ipc.rs
//!
//! Notifications kernel -> device_server.
//! File bornée, sans allocation, utilisable depuis ISR ou worker.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::scheduler::timer::clock::monotonic_ns;

pub const DEVICE_SERVER_EVENT_CAPACITY: u32 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DeviceServerEventKind {
    DriverStall = 0,
    UnhandledIrq = 1,
    IrqBlacklisted = 2,
    IommuFaultKill = 3,
    IommuLeak = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct DeviceServerEvent {
    pub timestamp_ms: u64,
    pub kind: DeviceServerEventKind,
    pub irq: u8,
    pub _pad0: u16,
    pub pid: u32,
    pub value0: u64,
    pub value1: u64,
}

impl DeviceServerEvent {
    const fn zeroed() -> Self {
        Self {
            timestamp_ms: 0,
            kind: DeviceServerEventKind::DriverStall,
            irq: 0,
            _pad0: 0,
            pid: 0,
            value0: 0,
            value1: 0,
        }
    }

    fn at(kind: DeviceServerEventKind, irq: u8, pid: u32, value0: u64, value1: u64) -> Self {
        Self {
            timestamp_ms: monotonic_ns() / 1_000_000,
            kind,
            irq,
            _pad0: 0,
            pid,
            value0,
            value1,
        }
    }
}

#[repr(C, align(64))]
struct DeviceServerEventSlot {
    seq: AtomicU32,
    event: UnsafeCell<DeviceServerEvent>,
}

unsafe impl Sync for DeviceServerEventSlot {}

#[repr(C, align(64))]
pub struct DeviceServerEventQueue {
    head: AtomicU32,
    _pad_head: [u8; 60],
    tail: AtomicU32,
    _pad_tail: [u8; 60],
    dropped: AtomicU32,
    initialized: AtomicBool,
    slots: [DeviceServerEventSlot; DEVICE_SERVER_EVENT_CAPACITY as usize],
}

unsafe impl Sync for DeviceServerEventQueue {}

impl DeviceServerEventQueue {
    pub const fn new() -> Self {
        #[allow(clippy::declare_interior_mutable_const)]
        const INIT_SLOT: DeviceServerEventSlot = DeviceServerEventSlot {
            seq: AtomicU32::new(0),
            event: UnsafeCell::new(DeviceServerEvent::zeroed()),
        };

        Self {
            head: AtomicU32::new(0),
            _pad_head: [0; 60],
            tail: AtomicU32::new(0),
            _pad_tail: [0; 60],
            dropped: AtomicU32::new(0),
            initialized: AtomicBool::new(false),
            slots: [INIT_SLOT; DEVICE_SERVER_EVENT_CAPACITY as usize],
        }
    }

    pub fn init(&self) {
        self.head.store(0, Ordering::Relaxed);
        self.tail.store(0, Ordering::Relaxed);
        self.dropped.store(0, Ordering::Relaxed);
        for (i, slot) in self.slots.iter().enumerate() {
            slot.seq.store(i as u32, Ordering::Release);
        }
        self.initialized.store(true, Ordering::Release);
    }

    pub fn push(&self, event: DeviceServerEvent) -> bool {
        if !self.initialized.load(Ordering::Acquire) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }

        let mut pos = self.head.load(Ordering::Relaxed);
        loop {
            let idx = (pos % DEVICE_SERVER_EVENT_CAPACITY) as usize;
            let slot = &self.slots[idx];
            let seq = slot.seq.load(Ordering::Acquire);
            let dif = seq as i64 - pos as i64;

            if dif == 0 {
                match self.head.compare_exchange(pos, pos + 1, Ordering::AcqRel, Ordering::Relaxed) {
                    Ok(_) => {
                        unsafe { *slot.event.get() = event; }
                        slot.seq.store(pos + 1, Ordering::Release);
                        return true;
                    }
                    Err(actual) => {
                        pos = actual;
                        core::hint::spin_loop();
                    }
                }
            } else if dif < 0 {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                return false;
            } else {
                pos = self.head.load(Ordering::Relaxed);
                core::hint::spin_loop();
            }
        }
    }

    pub fn pop(&self) -> Option<DeviceServerEvent> {
        let mut pos = self.tail.load(Ordering::Relaxed);
        loop {
            let idx = (pos % DEVICE_SERVER_EVENT_CAPACITY) as usize;
            let slot = &self.slots[idx];
            let seq = slot.seq.load(Ordering::Acquire);
            let dif = seq as i64 - (pos + 1) as i64;

            if dif == 0 {
                match self.tail.compare_exchange(pos, pos + 1, Ordering::AcqRel, Ordering::Relaxed) {
                    Ok(_) => {
                        let event = unsafe { *slot.event.get() };
                        slot.seq.store(pos + DEVICE_SERVER_EVENT_CAPACITY, Ordering::Release);
                        return Some(event);
                    }
                    Err(actual) => {
                        pos = actual;
                        core::hint::spin_loop();
                    }
                }
            } else if dif < 0 {
                return None;
            } else {
                pos = self.tail.load(Ordering::Relaxed);
                core::hint::spin_loop();
            }
        }
    }

    pub fn drain_dropped(&self) -> u32 {
        self.dropped.swap(0, Ordering::AcqRel)
    }
}

pub static DEVICE_SERVER_EVENTS: DeviceServerEventQueue = DeviceServerEventQueue::new();

#[inline]
pub fn init() {
    DEVICE_SERVER_EVENTS.init();
}

#[inline]
pub fn pop_notification() -> Option<DeviceServerEvent> {
    DEVICE_SERVER_EVENTS.pop()
}

#[inline]
pub fn drain_dropped() -> u32 {
    DEVICE_SERVER_EVENTS.drain_dropped()
}

#[inline]
fn notify(event: DeviceServerEvent) {
    let _ = DEVICE_SERVER_EVENTS.push(event);
}

/// Driver non réactif : IRQ non acquittée dans la fenêtre watchdog.
pub fn notify_driver_stall(irq: u8) {
    notify(DeviceServerEvent::at(DeviceServerEventKind::DriverStall, irq, 0, 0, 0));
}

/// IRQ ghost : aucun handler n'a revendiqué l'IRQ level.
pub fn notify_unhandled_irq(irq: u8) {
    notify(DeviceServerEvent::at(DeviceServerEventKind::UnhandledIrq, irq, 0, 0, 0));
}

/// IRQ blacklistée après storms répétés.
pub fn notify_irq_blacklisted(irq: u8) {
    notify(DeviceServerEvent::at(DeviceServerEventKind::IrqBlacklisted, irq, 0, 0, 0));
}

/// Faute IOMMU détectée pour un PID et une IOVA données.
pub fn notify_iommu_fault_kill(pid: u32, iova: u64, reason: u8) {
    notify(DeviceServerEvent::at(
        DeviceServerEventKind::IommuFaultKill,
        0,
        pid,
        iova,
        reason as u64,
    ));
}

/// Fuite de domaine/mapping IOMMU détectée par le noyau.
pub fn notify_iommu_leak(domain_id: u32) {
    notify(DeviceServerEvent::at(
        DeviceServerEventKind::IommuLeak,
        0,
        0,
        domain_id as u64,
        0,
    ));
}
