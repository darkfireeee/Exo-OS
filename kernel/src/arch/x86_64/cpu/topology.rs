//! # cpu/topology.rs — CPU Topology (Cores, HT, NUMA)
//!
//! Detects physical system topology via CPUID and ACPI.
//! Structure: Package → Core → Thread (SMT)
//!
//! Supports up to 256 logical CPUs (MAX_CPUS).

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

// ── Constants ────────────────────────────────────────────────────────────────

/// Maximum number of supported logical CPUs
pub const MAX_CPUS: usize = 256;

/// Maximum number of NUMA packages
pub const MAX_PACKAGES: usize = 16;

/// Maximum number of NUMA nodes
pub const MAX_NUMA_NODES: usize = 16;

// ── CPU Identifiers ──────────────────────────────────────────────────────────

/// Logical CPU identifier (0-based, assigned by the OS)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CpuId(pub u32);

/// Physical APIC ID (read from CPUID leaf 1 EBX[31:24] or x2APIC)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ApicId(pub u32);

/// Package ID (physical socket)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PackageId(pub u32);

/// Core ID (within package)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CoreId(pub u32);

/// NUMA Node
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NumaNode(pub u32);

// ── Logical CPU Descriptor ─────────────────────────────────────────────

/// Topological information for a logical CPU
#[derive(Debug, Clone, Copy)]
pub struct CpuDescriptor {
    pub cpu_id: CpuId,
    pub apic_id: ApicId,
    pub package_id: PackageId,
    pub core_id: CoreId,
    pub smt_id: u32, // Thread ID in core (0 or 1 for HT)
    pub numa_node: NumaNode,
    pub online: bool,
}

impl CpuDescriptor {
    const fn zero() -> Self {
        Self {
            cpu_id: CpuId(0),
            apic_id: ApicId(0),
            package_id: PackageId(0),
            core_id: CoreId(0),
            smt_id: 0,
            numa_node: NumaNode(0),
            online: false,
        }
    }
}

// ── Global Topology Table ────────────────────────────────────────────────

/// CPU descriptor table (indexed by cpu_id)
static mut CPU_DESCRIPTORS: [CpuDescriptor; MAX_CPUS] = [CpuDescriptor::zero(); MAX_CPUS];

/// Number of detected logical CPUs (online + offline)
static CPU_COUNT_TOTAL: AtomicU32 = AtomicU32::new(0);

/// Number of online logical CPUs
static CPU_COUNT_ONLINE: AtomicU32 = AtomicU32::new(0);

/// Number of physical packages (sockets)
static PACKAGE_COUNT: AtomicU32 = AtomicU32::new(1);

/// Maximum physical cores per package
#[allow(dead_code)]
static CORES_PER_PACKAGE: AtomicU32 = AtomicU32::new(1);

/// SMT active (Hyper-Threading)
static SMT_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Number of detected NUMA nodes
static NUMA_NODE_COUNT: AtomicU32 = AtomicU32::new(1);

/// Topology initialized
static TOPOLOGY_READY: AtomicBool = AtomicBool::new(false);

// ── Topology Detection via CPUID leaf 0xB / 0x1F ────────────────────────────

/// CPUID topology level (leaf 0xB / 0x1F)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TopoLevel {
    Invalid = 0,
    Smt = 1,
    Core = 2,
    Module = 3,
    Tile = 4,
    Die = 5,
}

impl TopoLevel {
    fn from_u32(val: u32) -> Self {
        match val & 0xFF {
            1 => Self::Smt,
            2 => Self::Core,
            3 => Self::Module,
            4 => Self::Tile,
            5 => Self::Die,
            _ => Self::Invalid,
        }
    }
}

/// Detects topology shift bits from CPUID leaf 0xB or 0x1F
///
/// Returns `(smt_shift, core_shift, package_shift)` — shifts for extracting
/// IDs from APIC ID.
fn detect_topo_shifts(prefer_leaf_1f: bool) -> (u32, u32, u32) {
    let leaf = if prefer_leaf_1f { 0x1F } else { 0x0B };

    // Check if leaf is supported (EBX != 0 for subleaf 0)
    let (_eax0, ebx0, _ecx0, _edx0) = {
        let (eax, ecx, edx): (u32, u32, u32);
        let ebx_r: u64;
        // SAFETY: Unprivileged CPUID; xchg to preserve rbx (reserved by LLVM)
        unsafe {
            core::arch::asm!(
                "xchg {tmp:r}, rbx",
                "cpuid",
                "xchg {tmp:r}, rbx",
                inout("eax") leaf => eax,
                inout("ecx") 0u32 => ecx,
                out("edx") edx,
                tmp = inout(reg) 0u64 => ebx_r,
                options(nostack, nomem)
            );
        }
        (eax, ebx_r as u32, ecx, edx)
    };

    if ebx0 == 0 {
        return (0, 1, 8); // basic fallback
    }

    let mut smt_shift = 0u32;
    let mut core_shift = 0u32;

    for subleaf in 0..8u32 {
        let (eax, ecx): (u32, u32);
        // SAFETY: Unprivileged CPUID; xchg to preserve rbx
        unsafe {
            core::arch::asm!(
                "xchg {tmp:r}, rbx",
                "cpuid",
                "xchg {tmp:r}, rbx",
                inout("eax") leaf    => eax,
                inout("ecx") subleaf => ecx,
                out("edx") _,
                tmp = inout(reg) 0u64 => _,
                options(nostack, nomem)
            );
        }
        let level_type = TopoLevel::from_u32((ecx >> 8) & 0xFF);
        let shift = eax & 0x1F;

        match level_type {
            TopoLevel::Invalid => break,
            TopoLevel::Smt => smt_shift = shift,
            TopoLevel::Core => core_shift = shift,
            _ => {}
        }
    }

    let package_shift = if core_shift > 0 {
        core_shift
    } else {
        smt_shift + 4
    };
    (smt_shift, core_shift, package_shift)
}

/// Parses BSP topology from CPUID
fn parse_bsp_topology() -> CpuDescriptor {
    use super::features::cpu_features_or_none;

    // Read APIC ID (32 bits if x2APIC, 8 bits otherwise)
    let apic_id = if cpu_features_or_none().is_some_and(|features| features.has_x2apic()) {
        let (_eax, _ecx, edx): (u32, u32, u32);
        // SAFETY: CPUID 0xB unprivileged; xchg to preserve rbx
        unsafe {
            core::arch::asm!(
                "xchg {tmp:r}, rbx",
                "cpuid",
                "xchg {tmp:r}, rbx",
                inout("eax") 0x0Bu32 => _eax,
                inout("ecx") 0u32 => _ecx,
                out("edx") edx,
                tmp = inout(reg) 0u64 => _,
                options(nostack, nomem)
            );
        }
        edx
    } else {
        let ebx_r: u64;
        // SAFETY: CPUID 1 unprivileged; xchg to preserve rbx
        unsafe {
            core::arch::asm!(
                "xchg {tmp:r}, rbx",
                "cpuid",
                "xchg {tmp:r}, rbx",
                inout("eax") 1u32 => _,
                inout("ecx") 0u32 => _,
                out("edx") _,
                tmp = inout(reg) 0u64 => ebx_r,
                options(nostack, nomem)
            );
        }
        let ebx = ebx_r as u32;
        (ebx >> 24) & 0xFF
    };

    let prefer_1f = {
        let eax: u32;
        // SAFETY: CPUID 0 unprivileged; xchg to preserve rbx
        unsafe {
            core::arch::asm!(
                "xchg {tmp:r}, rbx",
                "cpuid",
                "xchg {tmp:r}, rbx",
                inout("eax") 0u32 => eax,
                inout("ecx") 0u32 => _,
                out("edx") _,
                tmp = inout(reg) 0u64 => _,
                options(nostack, nomem)
            );
        }
        eax >= 0x1F
    };

    let (smt_shift, core_shift, pkg_shift) = detect_topo_shifts(prefer_1f);

    let smt_id = apic_id & ((1 << smt_shift) - 1);
    let core_id = (apic_id >> smt_shift) & ((1 << (core_shift - smt_shift)) - 1);
    let package_id = apic_id >> pkg_shift;

    CpuDescriptor {
        cpu_id: CpuId(0),
        apic_id: ApicId(apic_id),
        package_id: PackageId(package_id),
        core_id: CoreId(core_id),
        smt_id,
        numa_node: NumaNode(0), // will be updated by ACPI SRAT
        online: true,
    }
}

// ── Initialization ────────────────────────────────────────────────────────────

/// Initializes CPU topology for BSP (CPU 0)
///
/// APs will be added via `register_ap()` when booted.
pub fn init_topology() {
    let bsp = parse_bsp_topology();

    // SAFETY: called once from BSP before booting APs
    unsafe {
        CPU_DESCRIPTORS[0] = bsp;
    }

    CPU_COUNT_TOTAL.store(1, Ordering::Release);
    CPU_COUNT_ONLINE.store(1, Ordering::Release);
    TOPOLOGY_READY.store(true, Ordering::Release);
}

/// Registers an AP upon boot
///
/// Called from AP boot code with the processor's APIC ID.
pub fn register_ap(apic_id: ApicId, numa_node: NumaNode) -> CpuId {
    let idx = CPU_COUNT_TOTAL.fetch_add(1, Ordering::AcqRel) as usize;

    if idx >= MAX_CPUS {
        // Overflow — ignore this CPU
        CPU_COUNT_TOTAL.fetch_sub(1, Ordering::Relaxed);
        crate::arch::x86_64::terminal::debug_write(b"register_ap: trop de CPUs\n");
        crate::arch::x86_64::halt_cpu();
    }

    let (smt_shift, core_shift, pkg_shift) = detect_topo_shifts(false);
    let a = apic_id.0;
    let smt_id = a & ((1 << smt_shift) - 1);
    let core_id = if core_shift > smt_shift {
        (a >> smt_shift) & ((1 << (core_shift - smt_shift)) - 1)
    } else {
        0
    };
    let package_id = a >> pkg_shift;

    let desc = CpuDescriptor {
        cpu_id: CpuId(idx as u32),
        apic_id,
        package_id: PackageId(package_id),
        core_id: CoreId(core_id),
        smt_id,
        numa_node,
        online: true,
    };

    // SAFETY: unique idx (atomic), no data race as each AP registers once
    unsafe {
        CPU_DESCRIPTORS[idx] = desc;
    }
    CPU_COUNT_ONLINE.fetch_add(1, Ordering::AcqRel);

    // Update package count if new
    if package_id + 1 > PACKAGE_COUNT.load(Ordering::Relaxed) {
        PACKAGE_COUNT.store(package_id + 1, Ordering::Relaxed);
    }

    // Detect SMT if smt_id > 0
    if smt_id > 0 {
        SMT_ACTIVE.store(true, Ordering::Relaxed);
    }

    CpuId(idx as u32)
}

// ── Accessors ────────────────────────────────────────────────────────────────

/// Returns the descriptor of a logical CPU
///
/// Returns `None` if `cpu_id` >= CPU count.
pub fn cpu_descriptor(cpu_id: CpuId) -> Option<&'static CpuDescriptor> {
    let idx = cpu_id.0 as usize;
    if idx >= CPU_COUNT_TOTAL.load(Ordering::Relaxed) as usize {
        return None;
    }
    // SAFETY: idx < CPU_COUNT_TOTAL — written before incrementing the counter
    Some(unsafe { &CPU_DESCRIPTORS[idx] })
}

/// Total number of logical CPUs (online + offline)
#[inline(always)]
pub fn cpu_count_total() -> u32 {
    CPU_COUNT_TOTAL.load(Ordering::Relaxed)
}

/// Number of online logical CPUs
#[inline(always)]
pub fn cpu_count_online() -> u32 {
    CPU_COUNT_ONLINE.load(Ordering::Relaxed)
}

/// Number of physical packages
#[inline(always)]
pub fn package_count() -> u32 {
    PACKAGE_COUNT.load(Ordering::Relaxed)
}

/// SMT (Hyper-Threading) active
#[inline(always)]
pub fn smt_active() -> bool {
    SMT_ACTIVE.load(Ordering::Relaxed)
}

/// Number of NUMA nodes
#[inline(always)]
pub fn numa_node_count() -> u32 {
    NUMA_NODE_COUNT.load(Ordering::Relaxed)
}

/// Updates NUMA node count (from ACPI SRAT)
pub fn set_numa_node_count(count: u32) {
    NUMA_NODE_COUNT.store(count, Ordering::Release);
}

/// Updates NUMA node of a CPU (from ACPI SRAT/SLIT)
pub fn update_numa_node(cpu_id: CpuId, node: NumaNode) {
    let idx = cpu_id.0 as usize;
    if idx >= CPU_COUNT_TOTAL.load(Ordering::Relaxed) as usize {
        return;
    }
    // SAFETY: idx within bounds, one-off NUMA update from single-thread boot
    unsafe {
        CPU_DESCRIPTORS[idx].numa_node = node;
    }
}

/// Returns the current logical CPU identifier.
///
/// RDTSCP is not guaranteed under QEMU/TCG. The kernel's stable path is the
/// per-CPU GS slot, maintained by SMP initialization and syscall/IRQ stubs.
#[inline(always)]
pub fn current_cpu_id() -> CpuId {
    CpuId(crate::arch::x86_64::smp::percpu::current_cpu_id())
}

// ── NUMA Distance Matrix ──────────────────────────────────────────────────

/// NUMA distance matrix (index: [from_node][to_node])
/// Value 10 = local, >10 = remote (ACPI SLIT standard)
static mut NUMA_DISTANCE: [[u8; MAX_NUMA_NODES]; MAX_NUMA_NODES] =
    [[10u8; MAX_NUMA_NODES]; MAX_NUMA_NODES];

/// Returns the NUMA distance between two nodes
pub fn numa_distance(from: NumaNode, to: NumaNode) -> u8 {
    let f = from.0 as usize;
    let t = to.0 as usize;
    if f >= MAX_NUMA_NODES || t >= MAX_NUMA_NODES {
        return 255;
    }
    // SAFETY: bounded indices, read-only after init
    unsafe { NUMA_DISTANCE[f][t] }
}

/// Configures the NUMA distance matrix from ACPI SLIT
pub fn set_numa_distance(from: NumaNode, to: NumaNode, dist: u8) {
    let f = from.0 as usize;
    let t = to.0 as usize;
    if f >= MAX_NUMA_NODES || t >= MAX_NUMA_NODES {
        return;
    }
    // SAFETY: called once from BSP boot (ACPI parsing)
    unsafe {
        NUMA_DISTANCE[f][t] = dist;
    }
}
