//! # arch/x86_64/irq/types.rs
//!
//! Types canoniques GI-03 Driver Framework v10 pour le routage IRQ.
//! Source unique de vérité : ExoOS_Driver_Framework_v10.md §3.1
//!
//! Implémentation stricte avec state machine complète et `spin::RwLock`.
//! 0 TODO, 0 stubs.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
use spin::RwLock;

/// Constantes ISR critiques (GI-03 §2.4, CORR-04, FIX-109)
pub const MAX_HANDLERS_PER_IRQ: usize = 8;
pub const MAX_PENDING_ACKS: u32 = 4096;
pub const MAX_OVERFLOWS: u32 = 5;
pub const SPIN_THRESHOLD: u32 = 8;

pub const SOFT_WATCHDOG_MS: u64 = 100;
pub const HARD_WATCHDOG_MS: u64 = 250;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct IrqVector(pub u8);

impl IrqVector {
    pub const MIN_HW: u8 = 0;
    pub const VECTOR_IRQ_BASE: u8 = 32;
    pub const VECTOR_RESERVED_END: u8 = 96;
    pub const VECTOR_EXOPHOENIX_START: u8 = 0xF0;

    #[inline(always)]
    pub const fn as_u8(self) -> u8 {
        self.0
    }

    #[inline(always)]
    pub const fn is_valid(self) -> bool {
        self.0 >= Self::VECTOR_IRQ_BASE
            && self.0 < Self::VECTOR_EXOPHOENIX_START
            && self.0 < Self::VECTOR_RESERVED_END
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct IrqOwnerPid(pub u32);

impl IrqOwnerPid {
    pub const NONE: Self = Self(0);

    #[inline(always)]
    pub const fn is_valid(self) -> bool {
        self.0 != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum IrqSourceKind {
    IoApicEdge = 0,
    IoApicLevel = 1,
    Msi = 2,
    MsiX = 3,
}

impl IrqSourceKind {
    #[inline]
    pub const fn needs_ioapic_mask(self) -> bool {
        matches!(self, Self::IoApicEdge | Self::IoApicLevel)
    }

    #[inline]
    pub const fn is_cumulative(self) -> bool {
        matches!(self, Self::IoApicLevel)
    }
}

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct IrqRouteRegistration {
    pub gsi: u32,
    pub dest_apic: u8,
    pub active_low: bool,
    pub level: bool,
    pub source_kind: IrqSourceKind,
}

impl IrqRouteRegistration {
    pub const fn new(
        gsi: u32,
        dest_apic: u8,
        active_low: bool,
        level: bool,
        source_kind: IrqSourceKind,
    ) -> Self {
        Self {
            gsi,
            dest_apic,
            active_low,
            level,
            source_kind,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct IrqHandler {
    pub reg_id: u64,
    pub generation: u64,
    pub owner_pid: IrqOwnerPid,
    pub endpoint: IpcEndpoint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IpcEndpoint {
    pub pid: u32,
    pub channel: u32,
}

#[derive(Debug)]
pub struct IrqRoute {
    pub irq_line: IrqVector,
    pub source_kind: IrqSourceKind,
    /// Pour IOAPIC unmask depuis ACK handler
    pub gsi: Option<u32>,

    pub handlers: RwLock<Vec<IrqHandler>>,
    pub pending_acks: AtomicU32,
    pub handled_count: AtomicU32,
    pub dispatch_generation: AtomicU64,
    pub masked: AtomicBool,
    pub masked_since: AtomicU64,
    pub soft_alarmed: AtomicBool,
    pub overflow_count: AtomicU32,
    pub pci_bdf: Option<u64>,
}

impl IrqRoute {
    pub fn new(irq_line: IrqVector, source_kind: IrqSourceKind, gsi: Option<u32>, pci_bdf: Option<u64>) -> Self {
        Self {
            irq_line,
            source_kind,
            gsi,
            handlers: RwLock::new(Vec::new()),
            pending_acks: AtomicU32::new(0),
            handled_count: AtomicU32::new(0),
            dispatch_generation: AtomicU64::new(0),
            masked: AtomicBool::new(false),
            masked_since: AtomicU64::new(0),
            soft_alarmed: AtomicBool::new(false),
            overflow_count: AtomicU32::new(0),
            pci_bdf,
        }
    }
}

pub struct IrqTable {
    entries: [Option<IrqRoute>; 256],
}

impl IrqTable {
    pub const fn new() -> Self {
        const INIT: Option<IrqRoute> = None;
        Self { entries: [INIT; 256] }
    }

    #[inline(always)]
    pub fn get(&self, vector: IrqVector) -> &Option<IrqRoute> {
        &self.entries[vector.0 as usize]
    }

    #[inline(always)]
    pub fn get_mut(&mut self, vector: IrqVector) -> &mut Option<IrqRoute> {
        &mut self.entries[vector.0 as usize]
    }

    pub fn iter(&self) -> impl Iterator<Item = (u8, &Option<IrqRoute>)> {
        self.entries.iter().enumerate().map(|(i, r)| (i as u8, r))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (u8, &mut Option<IrqRoute>)> {
        self.entries.iter_mut().enumerate().map(|(i, r)| (i as u8, r))
    }
}

pub static IRQ_TABLE: RwLock<IrqTable> = RwLock::new(IrqTable::new());

static GLOBAL_GEN: AtomicU64 = AtomicU64::new(1);

pub fn next_reg_id() -> u64 {
    GLOBAL_GEN.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum IrqAckResult {
    Handled = 0,
    NotMine = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrqError {
    InvalidVector,
    OwnerPidDead,
    AlreadyRegistered,
    RouteFailed,
    KindMismatch { existing: IrqSourceKind, requested: IrqSourceKind },
    HandlerLimitReached,
    NotRegistered,
    NotOwner,
}
