//! # cpu/topology.rs — Topologie CPU (cores, HT, NUMA)
//!
//! Détecte la topologie physique du système via CPUID et ACPI.
//! Structure : Package → Core → Thread (SMT)
//!
//! Supporte jusqu'à 256 CPUs logiques (MAX_CPUS).

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

// ── Constantes ────────────────────────────────────────────────────────────────

/// Nombre maximum de CPUs logiques supportés
pub const MAX_CPUS: usize = 256;

/// Nombre maximum de packages NUMA
pub const MAX_PACKAGES: usize = 16;

/// Nombre maximum de nœuds NUMA
pub const MAX_NUMA_NODES: usize = 16;

// ── Identifiants CPU ──────────────────────────────────────────────────────────

/// Identifiant CPU logique (0-based, attribué par l'OS)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CpuId(pub u32);

/// APIC ID physique (lu depuis CPUID leaf 1 EBX[31:24] ou x2APIC)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ApicId(pub u32);

/// Package ID (socket physique)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PackageId(pub u32);

/// Core ID (dans le package)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CoreId(pub u32);

/// Nœud NUMA
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NumaNode(pub u32);

// ── Descripteur d'un CPU logique ─────────────────────────────────────────────

/// Informations topologiques d'un CPU logique
#[derive(Debug, Clone, Copy)]
pub struct CpuDescriptor {
    pub cpu_id: CpuId,
    pub apic_id: ApicId,
    pub package_id: PackageId,
    pub core_id: CoreId,
    pub smt_id: u32, // Thread ID dans le core (0 ou 1 pour HT)
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

// ── Table globale de topologie ────────────────────────────────────────────────

/// Table des descripteurs CPU (indexée par cpu_id)
static mut CPU_DESCRIPTORS: [CpuDescriptor; MAX_CPUS] = [CpuDescriptor::zero(); MAX_CPUS];

/// Nombre de CPUs logiques détectés (en ligne + hors ligne)
static CPU_COUNT_TOTAL: AtomicU32 = AtomicU32::new(0);

/// Nombre de CPUs logiques en ligne
static CPU_COUNT_ONLINE: AtomicU32 = AtomicU32::new(0);

/// Nombre de packages (sockets) physiques
static PACKAGE_COUNT: AtomicU32 = AtomicU32::new(1);

/// Nombre de cores physiques par package (maximum)
#[allow(dead_code)]
static CORES_PER_PACKAGE: AtomicU32 = AtomicU32::new(1);

/// SMT actif (Hyper-Threading)
static SMT_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Nombre de nœuds NUMA détectés
static NUMA_NODE_COUNT: AtomicU32 = AtomicU32::new(1);

/// Topologie initialisée
static TOPOLOGY_READY: AtomicBool = AtomicBool::new(false);

// ── Détection topologie via CPUID leaf 0xB / 0x1F ────────────────────────────

/// Niveau de topologie CPUID (leaf 0xB / 0x1F)
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

/// Détecte les bits de shift de topologie depuis CPUID leaf 0xB ou 0x1F
///
/// Retourne `(smt_shift, core_shift, package_shift)` — shifts pour extraire
/// les IDs de l'APIC ID.
fn detect_topo_shifts(prefer_leaf_1f: bool) -> (u32, u32, u32) {
    let leaf = if prefer_leaf_1f { 0x1F } else { 0x0B };

    // Vérifier que la leaf est supportée (EBX != 0 pour subleaf 0)
    let (_eax0, ebx0, _ecx0, _edx0) = {
        let (eax, ecx, edx): (u32, u32, u32);
        let ebx_r: u64;
        // SAFETY: CPUID non-privilégié ; xchg pour préserver rbx (réservé par LLVM)
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
        return (0, 1, 8); // fallback basique
    }

    let mut smt_shift = 0u32;
    let mut core_shift = 0u32;

    for subleaf in 0..8u32 {
        let (eax, ecx): (u32, u32);
        // SAFETY: CPUID non-privilégié ; xchg pour préserver rbx
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

/// Parse la topologie du BSP depuis CPUID
fn parse_bsp_topology() -> CpuDescriptor {
    use super::features::CPU_FEATURES;

    // Lire APIC ID (32 bits si x2APIC, 8 bits sinon)
    let apic_id = if CPU_FEATURES.has_x2apic() {
        let (_eax, _ecx, edx): (u32, u32, u32);
        // SAFETY: CPUID 0xB non-privilégié ; xchg pour préserver rbx
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
        // SAFETY: CPUID 1 non-privilégié ; xchg pour préserver rbx
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
        // SAFETY: CPUID 0 non-privilégié ; xchg pour préserver rbx
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
        numa_node: NumaNode(0), // sera mis à jour par ACPI SRAT
        online: true,
    }
}

// ── Initialisation ────────────────────────────────────────────────────────────

/// Initialise la topologie CPU pour le BSP (CPU 0)
///
/// Les APs seront ajoutés via `register_ap()` lors de leur démarrage.
pub fn init_topology() {
    let bsp = parse_bsp_topology();

    // SAFETY: appelé une seule fois depuis le BSP avant démarrage des APs
    unsafe {
        CPU_DESCRIPTORS[0] = bsp;
    }

    CPU_COUNT_TOTAL.store(1, Ordering::Release);
    CPU_COUNT_ONLINE.store(1, Ordering::Release);
    TOPOLOGY_READY.store(true, Ordering::Release);
}

/// Enregistre un AP lors de son démarrage
///
/// Appelé depuis le code de boot AP avec l'APIC ID du processeur.
pub fn register_ap(apic_id: ApicId, numa_node: NumaNode) -> CpuId {
    let idx = CPU_COUNT_TOTAL.fetch_add(1, Ordering::AcqRel) as usize;

    if idx >= MAX_CPUS {
        // Dépassement — ignorer ce CPU
        CPU_COUNT_TOTAL.fetch_sub(1, Ordering::Relaxed);
        panic!("register_ap: trop de CPUs (max {})", MAX_CPUS);
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

    // SAFETY: idx unique (atomique), pas de course car chaque AP s'enregistre une fois
    unsafe {
        CPU_DESCRIPTORS[idx] = desc;
    }
    CPU_COUNT_ONLINE.fetch_add(1, Ordering::AcqRel);

    // Mettre à jour le nombre de packages si nouveau
    if package_id + 1 > PACKAGE_COUNT.load(Ordering::Relaxed) {
        PACKAGE_COUNT.store(package_id + 1, Ordering::Relaxed);
    }

    // Détecter SMT si smt_id > 0
    if smt_id > 0 {
        SMT_ACTIVE.store(true, Ordering::Relaxed);
    }

    CpuId(idx as u32)
}

// ── Accesseurs ────────────────────────────────────────────────────────────────

/// Retourne le descripteur d'un CPU logique
///
/// Retourne `None` si `cpu_id` >= nombre de CPUs.
pub fn cpu_descriptor(cpu_id: CpuId) -> Option<&'static CpuDescriptor> {
    let idx = cpu_id.0 as usize;
    if idx >= CPU_COUNT_TOTAL.load(Ordering::Relaxed) as usize {
        return None;
    }
    // SAFETY: idx < CPU_COUNT_TOTAL — écrit avant d'incrémenter le compteur
    Some(unsafe { &CPU_DESCRIPTORS[idx] })
}

/// Nombre total de CPUs logiques (en ligne + hors ligne)
#[inline(always)]
pub fn cpu_count_total() -> u32 {
    CPU_COUNT_TOTAL.load(Ordering::Relaxed)
}

/// Nombre de CPUs logiques en ligne
#[inline(always)]
pub fn cpu_count_online() -> u32 {
    CPU_COUNT_ONLINE.load(Ordering::Relaxed)
}

/// Nombre de packages physiques
#[inline(always)]
pub fn package_count() -> u32 {
    PACKAGE_COUNT.load(Ordering::Relaxed)
}

/// SMT (Hyper-Threading) actif
#[inline(always)]
pub fn smt_active() -> bool {
    SMT_ACTIVE.load(Ordering::Relaxed)
}

/// Nombre de nœuds NUMA
#[inline(always)]
pub fn numa_node_count() -> u32 {
    NUMA_NODE_COUNT.load(Ordering::Relaxed)
}

/// Met à jour le nombre de nœuds NUMA (depuis ACPI SRAT)
pub fn set_numa_node_count(count: u32) {
    NUMA_NODE_COUNT.store(count, Ordering::Release);
}

/// Met à jour le nœud NUMA d'un CPU (depuis ACPI SRAT/SLIT)
pub fn update_numa_node(cpu_id: CpuId, node: NumaNode) {
    let idx = cpu_id.0 as usize;
    if idx >= CPU_COUNT_TOTAL.load(Ordering::Relaxed) as usize {
        return;
    }
    // SAFETY: idx dans les bornes, update NUMA ponctuel depuis boot single-thread
    unsafe {
        CPU_DESCRIPTORS[idx].numa_node = node;
    }
}

/// Retourne l'identifiant CPU logique courant (via RDTSCP TSC_AUX)
///
/// Nécessite que `init_tsc()` ait configuré `MSR_TSC_AUX` avec le CPU ID.
#[inline(always)]
pub fn current_cpu_id() -> CpuId {
    // SAFETY: RDTSCP configuré au boot — cpu_aux = cpu ID logique
    let (_, aux) = unsafe { super::msr::rdtscp() };
    CpuId(aux)
}

// ── Matrice de distance NUMA ──────────────────────────────────────────────────

/// Matrice de distance NUMA (index: [from_node][to_node])
/// Valeur 10 = local, >10 = remote (ACPI SLIT standard)
static mut NUMA_DISTANCE: [[u8; MAX_NUMA_NODES]; MAX_NUMA_NODES] =
    [[10u8; MAX_NUMA_NODES]; MAX_NUMA_NODES];

/// Retourne la distance NUMA entre deux nœuds
pub fn numa_distance(from: NumaNode, to: NumaNode) -> u8 {
    let f = from.0 as usize;
    let t = to.0 as usize;
    if f >= MAX_NUMA_NODES || t >= MAX_NUMA_NODES {
        return 255;
    }
    // SAFETY: indices bornés, lecture seule après init
    unsafe { NUMA_DISTANCE[f][t] }
}

/// Configure la matrice de distance NUMA depuis ACPI SLIT
pub fn set_numa_distance(from: NumaNode, to: NumaNode, dist: u8) {
    let f = from.0 as usize;
    let t = to.0 as usize;
    if f >= MAX_NUMA_NODES || t >= MAX_NUMA_NODES {
        return;
    }
    // SAFETY: appelé une seule fois depuis boot BSP (parsing ACPI)
    unsafe {
        NUMA_DISTANCE[f][t] = dist;
    }
}
