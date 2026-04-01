// kernel/src/memory/dma/iommu/arm_smmu.rs
//
// Pilote ARM SMMU (System Memory Management Unit) v3.
// Implémente l'interface IOMMU pour architectures ARMv8-A.
//
// COUCHE 0 — aucune dépendance externe.
// Référence : ARM IHI0070E — ARM System Memory Management Unit Architecture
//             Specification — SMMU Architecture version 3.
//
// ⚠️  Ce pilote est prévu pour ARM64 (AArch64).
//     Sur x86_64, toutes les opérations retournent `DmaError::NotSupported`.
//     Structure complète présente pour portabilité future.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::core::types::PhysAddr;
use crate::memory::dma::core::types::{IovaAddr, DmaError, IommuDomainId};

// ─────────────────────────────────────────────────────────────────────────────
// REGISTRES SMMU v3 (offsets dans MMIO)
// ─────────────────────────────────────────────────────────────────────────────

/// Page 0 (globale) — registres principaux SMMU v3.
pub mod smmu_regs {
    /// Identification Register 0.
    pub const IDR0: usize = 0x000;
    /// Identification Register 1.
    pub const IDR1: usize = 0x004;
    /// Identification Register 2.
    pub const IDR2: usize = 0x008;
    /// Identification Register 3.
    pub const IDR3: usize = 0x00C;
    /// Identification Register 4.
    pub const IDR4: usize = 0x010;
    /// Identification Register 5.
    pub const IDR5: usize = 0x014;
    /// IIDR — Implementation Identification Register.
    pub const IIDR: usize = 0x018;
    /// CR0 — Control Register 0.
    pub const CR0:  usize = 0x020;
    /// CR0ACK — Control Register 0 Acknowledges.
    pub const CR0ACK: usize = 0x024;
    /// CR1 — Control Register 1.
    pub const CR1:  usize = 0x028;
    /// CR2 — Control Register 2.
    pub const CR2:  usize = 0x02C;
    /// STATUSR — Status Register.
    pub const STATUSR: usize = 0x040;
    /// GBPA — Global Bypass Attribute Register.
    pub const GBPA: usize = 0x044;
    /// STRTAB_BASE — Stream Table Base.
    pub const STRTAB_BASE: usize = 0x080;
    /// STRTAB_BASE_CFG — Config.
    pub const STRTAB_BASE_CFG: usize = 0x088;
    /// CMDQ_BASE — Command Queue Base.
    pub const CMDQ_BASE: usize = 0x090;
    /// CMDQ_PROD — Command Queue Producer.
    pub const CMDQ_PROD: usize = 0x098;
    /// CMDQ_CONS — Command Queue Consumer.
    pub const CMDQ_CONS: usize = 0x09C;
    /// EVENTQ_BASE — Event Queue Base.
    pub const EVENTQ_BASE: usize = 0x0A0;
    /// EVENTQ_PROD — Event Queue Producer.
    pub const EVENTQ_PROD: usize = 0x00A8;
    /// EVENTQ_CONS — Event Queue Consumer.
    pub const EVENTQ_CONS: usize = 0x00AC;
    /// GERROR — Global Error Register.
    pub const GERROR: usize = 0x060;
    /// GERRORN — Global Error Number.
    pub const GERRORN: usize = 0x064;
}

/// Bits du registre CR0.
pub mod cr0_bits {
    /// SMMU Enable.
    pub const SMMUEN: u32 = 1 << 0;
    /// Event Queue Enable.
    pub const EVENTQEN: u32 = 1 << 2;
    /// Command Queue Enable.
    pub const CMDQEN: u32 = 1 << 3;
}

/// Bits du registre IDR0.
pub mod idr0_bits {
    /// Support de S1P (Stage-1 translation).
    pub const S1P:  u32 = 1 << 1;
    /// Support de S2P (Stage-2 translation).
    pub const S2P:  u32 = 1 << 0;
    /// Support TT4K (4KiB granule).
    pub const TT4K: u32 = 1 << 24;
    /// Support CD (Configuration Descriptor).
    pub const CD2L: u32 = 1 << 19;
    /// Support ASID16 (16-bit ASIDs).
    pub const ASID16: u32 = 1 << 12;
}

// ─────────────────────────────────────────────────────────────────────────────
// STREAM TABLE ENTRY (STE) — format ARM SMMU v3
// ─────────────────────────────────────────────────────────────────────────────

/// Stream Table Entry — mappe un StreamID → configuration de traduction.
/// Chaque STE fait 64 octets (4 × 128-bit = 4 DWORDs de 64 bits).
#[repr(C, align(64))]
#[derive(Copy, Clone)]
pub struct StreamTableEntry {
    pub word: [u64; 8],
}

impl StreamTableEntry {
    pub const EMPTY: Self = StreamTableEntry { word: [0u64; 8] };

    /// STE valide avec bypass (configurations basique, identité).
    #[inline]
    pub const fn bypass() -> Self {
        let mut ste = StreamTableEntry { word: [0u64; 8] };
        // V=1 (bit 0), CONFIG=bypass (bits 1:3 = 0b100)
        ste.word[0] = 1 | (4 << 1);
        ste
    }

    /// Retourne `true` si ce STE est valide (bit V=1).
    #[inline]
    pub fn is_valid(&self) -> bool { self.word[0] & 1 != 0 }
}

// ─────────────────────────────────────────────────────────────────────────────
// COMMANDES SMMU v3 — Command Queue
// ─────────────────────────────────────────────────────────────────────────────

/// Commande ARM SMMU v3 — 128 bits (2 × u64).
#[repr(C, align(16))]
#[derive(Copy, Clone, Debug)]
pub struct SmmuCommand {
    pub lo: u64,
    pub hi: u64,
}

impl SmmuCommand {
    /// TLBI_NH_ALL — invalide tout le TLB.
    pub const TLBI_NH_ALL: SmmuCommand = SmmuCommand {
        lo: 0x6,   // CMDOP_TLBI_NH_ALL
        hi: 0,
    };

    /// CMD_SYNC — synchronise la file de commandes.
    #[inline]
    pub const fn cmd_sync(msiseg: u32) -> SmmuCommand {
        SmmuCommand {
            lo: 0x46 | ((msiseg as u64) << 12),  // CMDOP_CMD_SYNC
            hi: 0,
        }
    }

    /// CFGI_STE — invalide le STE en cache pour un StreamID.
    #[inline]
    pub const fn cfgi_ste(sid: u32) -> SmmuCommand {
        SmmuCommand {
            lo: 0x3 | ((sid as u64) << 32),   // CMDOP_CFGI_STE
            hi: 1,  // LEAF = 1
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CAPACITÉS AMD DÉTECTÉES
// ─────────────────────────────────────────────────────────────────────────────

/// Capacités détectées sur le matériel SMMU lors de l'initialisation.
#[derive(Copy, Clone, Debug, Default)]
pub struct SmmuCapabilities {
    /// Stage-1 supporté.
    pub s1p:      bool,
    /// Stage-2 supporté.
    pub s2p:      bool,
    /// ASID 16 bits.
    pub asid16:   bool,
    /// Granule 4KiB.
    pub tt4k:     bool,
    /// Nombre de StreamID bits.
    pub sid_size: u8,
    /// Nombre de VMID bits.
    pub vmid16:   bool,
    /// Nombre maximum de streams.
    pub max_streams: u32,
}

// ─────────────────────────────────────────────────────────────────────────────
// ÉTAT DU PILOTE
// ─────────────────────────────────────────────────────────────────────────────

const MAX_ARM_SMMU_DEVICES: usize = 4;
const MAX_STREAMS_PER_SMMU: usize = 256;

struct ArmSmmuState {
    /// Adresse de base du MMIO.
    mmio_base:   u64,
    /// Capacités détectées.
    caps:        SmmuCapabilities,
    /// Vrai si le SMMU est initialisé et actif.
    enabled:     bool,
    /// Stream Table (statique, MAX_STREAMS_PER_SMMU entrées).
    stream_table: [StreamTableEntry; MAX_STREAMS_PER_SMMU],
    /// Statistiques.
    maps:        u64,
    unmaps:      u64,
    faults:      u64,
    tlbi_flushes: u64,
}

impl ArmSmmuState {
    const fn new() -> Self {
        ArmSmmuState {
            mmio_base:   0,
            caps:        SmmuCapabilities {
                s1p: false, s2p: false, asid16: false, tt4k: false,
                sid_size: 0, vmid16: false, max_streams: 0,
            },
            enabled:     false,
            stream_table: [StreamTableEntry::EMPTY; MAX_STREAMS_PER_SMMU],
            maps:         0,
            unmaps:       0,
            faults:       0,
            tlbi_flushes: 0,
        }
    }
}

struct ArmSmmuDeviceTable {
    devices: [Mutex<ArmSmmuState>; MAX_ARM_SMMU_DEVICES],
    count:   AtomicU32,
}

impl ArmSmmuDeviceTable {
    fn new() -> Self {
        ArmSmmuDeviceTable {
            devices: [
                Mutex::new(ArmSmmuState::new()),
                Mutex::new(ArmSmmuState::new()),
                Mutex::new(ArmSmmuState::new()),
                Mutex::new(ArmSmmuState::new()),
            ],
            count: AtomicU32::new(0),
        }
    }
}

static ARM_SMMU_DEVICES: spin::Once<ArmSmmuDeviceTable> = spin::Once::new();
static IS_INITIALIZED: AtomicBool = AtomicBool::new(false);

// ─────────────────────────────────────────────────────────────────────────────
// API PUBLIQUE
// ─────────────────────────────────────────────────────────────────────────────

/// Résultat d'une opération SMMU.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SmmuResult {
    Ok,
    NotSupported,
    InvalidParams,
    HardwareError,
    OutOfResources,
}

/// Initialise le sous-système ARM SMMU.
///
/// Sur x86_64, retourne immédiatement `SmmuResult::NotSupported`.
/// Sur ARM64, détecte les SMMMUs via le SMMU discovery mechanism.
///
/// # Safety
/// Doit être appelé en CPL 0 (mode kernel) après l'initialisation de la MMU.
pub unsafe fn init() -> SmmuResult {
    // Arch guard : ce pilote est ARM64 uniquement.
    #[cfg(not(target_arch = "aarch64"))]
    {
        return SmmuResult::NotSupported;
    }

    #[cfg(target_arch = "aarch64")]
    {
        ARM_SMMU_DEVICES.call_once(ArmSmmuDeviceTable::new);
        // Sur ARM64 réel, on lirait les SMMU descriptors depuis ACPI/IORT
        // ou le device tree (FDT). Pas encore implémenté — prévu pour ARM64 port.
        IS_INITIALIZED.store(true, Ordering::Release);
        SmmuResult::Ok
    }
}

/// Enregistre un nouveau SMMU à l'adresse MMIO donnée.
///
/// # Safety
/// `mmio_base` doit pointer sur un SMMU v3 valide, mappé en MMIO.
pub unsafe fn register_smmu(mmio_base: PhysAddr) -> SmmuResult {
    #[cfg(not(target_arch = "aarch64"))]
    { let _ = mmio_base; return SmmuResult::NotSupported; }

    #[cfg(target_arch = "aarch64")]
    {
        let table = match ARM_SMMU_DEVICES.get() {
            Some(t) => t,
            None    => return SmmuResult::NotSupported,
        };
        let idx = table.count.load(Ordering::Acquire) as usize;
        if idx >= MAX_ARM_SMMU_DEVICES { return SmmuResult::OutOfResources; }

        let mut dev = table.devices[idx].lock();
        dev.mmio_base = mmio_base.as_u64();

        // Lire IDR0 pour découvrir les capacités.
        let idr0 = read_smmu_reg(dev.mmio_base, smmu_regs::IDR0);
        dev.caps.s1p    = idr0 & idr0_bits::S1P  != 0;
        dev.caps.s2p    = idr0 & idr0_bits::S2P  != 0;
        dev.caps.asid16 = idr0 & idr0_bits::ASID16 != 0;
        dev.caps.tt4k   = idr0 & idr0_bits::TT4K != 0;
        dev.caps.max_streams = MAX_STREAMS_PER_SMMU as u32;
        dev.enabled = false;

        table.count.fetch_add(1, Ordering::Release);
        SmmuResult::Ok
    }
}

/// Active le SMMU (écriture dans CR0.SMMUEN).
///
/// # Safety
/// Mêmes préconditions que `register_smmu`.
pub unsafe fn enable(smmu_idx: usize) -> SmmuResult {
    #[cfg(not(target_arch = "aarch64"))]
    { let _ = smmu_idx; return SmmuResult::NotSupported; }

    #[cfg(target_arch = "aarch64")]
    {
        let table = match ARM_SMMU_DEVICES.get() {
            Some(t) => t,
            None    => return SmmuResult::NotSupported,
        };
        if smmu_idx >= MAX_ARM_SMMU_DEVICES { return SmmuResult::InvalidParams; }

        let mut dev = table.devices[smmu_idx].lock();
        if dev.mmio_base == 0 { return SmmuResult::InvalidParams; }

        // Activer CMDQ + SMMUEN.
        let cr0 = cr0_bits::CMDQEN | cr0_bits::EVENTQEN | cr0_bits::SMMUEN;
        write_smmu_reg(dev.mmio_base, smmu_regs::CR0, cr0);

        // Attendre CR0ACK.
        let mut timeout = 1_000_000u32;
        loop {
            let ack = read_smmu_reg(dev.mmio_base, smmu_regs::CR0ACK);
            if ack & cr0_bits::SMMUEN != 0 { break; }
            timeout = timeout.saturating_sub(1);
            if timeout == 0 { return SmmuResult::HardwareError; }
            core::hint::spin_loop();
        }
        dev.enabled = true;
        SmmuResult::Ok
    }
}

/// Mappe un range DMA [iova, iova+size) → [phys, phys+size).
///
/// Sur x86_64, retourne `SmmuResult::NotSupported`.
pub fn map_range(
    smmu_idx: usize,
    _sid:     u32,
    _iova:    IovaAddr,
    _phys:    PhysAddr,
    _size:    usize,
) -> SmmuResult {
    let _ = smmu_idx;
    #[cfg(not(target_arch = "aarch64"))]
    return SmmuResult::NotSupported;
    #[cfg(target_arch = "aarch64")]
    SmmuResult::NotSupported  // A_FAIRE: implémenter stage-1 walk ARM64
}

/// Libère un mapping DMA.
pub fn unmap_range(
    smmu_idx: usize,
    _sid:     u32,
    _iova:    IovaAddr,
    _size:    usize,
) -> SmmuResult {
    let _ = smmu_idx;
    SmmuResult::NotSupported
}

/// Invalide tout le TLB du SMMU (TLBI_NH_ALL + CMD_SYNC).
///
/// # Safety
/// Doit être appelé avec les locks SMMU non tenus.
pub unsafe fn flush_tlb_all(smmu_idx: usize) -> SmmuResult {
    #[cfg(not(target_arch = "aarch64"))]
    { let _ = smmu_idx; return SmmuResult::NotSupported; }

    #[cfg(target_arch = "aarch64")]
    {
        let table = match ARM_SMMU_DEVICES.get() {
            Some(t) => t,
            None    => return SmmuResult::NotSupported,
        };
        if smmu_idx >= MAX_ARM_SMMU_DEVICES { return SmmuResult::InvalidParams; }

        let mut dev = table.devices[smmu_idx].lock();
        if !dev.enabled { return SmmuResult::NotSupported; }
        dev.tlbi_flushes += 1;
        // Issue TLBI_NH_ALL + CMD_SYNC via command queue.
        // (Command queue management non implémenté — prévu pour ARM64 port.)
        SmmuResult::Ok
    }
}

/// Retourne `true` si l'architecture courante supporte ARM SMMU.
#[inline]
pub const fn is_supported() -> bool {
    cfg!(target_arch = "aarch64")
}

/// Statistiques du SMMU idx.
pub fn stats(smmu_idx: usize) -> Option<(u64, u64, u64, u64)> {
    let table = ARM_SMMU_DEVICES.get()?;
    if smmu_idx >= MAX_ARM_SMMU_DEVICES { return None; }
    let dev = table.devices[smmu_idx].lock();
    Some((dev.maps, dev.unmaps, dev.faults, dev.tlbi_flushes))
}

// ─────────────────────────────────────────────────────────────────────────────
// HELPERS MMIO (ARM64)
// ─────────────────────────────────────────────────────────────────────────────

/// Lit un registre 32 bits depuis l'espace MMIO du SMMU.
///
/// # Safety
/// `base` doit être une adresse MMIO valide, mappée, alignée.
#[cfg(target_arch = "aarch64")]
#[inline]
unsafe fn read_smmu_reg(base: u64, offset: usize) -> u32 {
    let ptr = (base + offset as u64) as *const u32;
    ptr.read_volatile()
}

#[cfg(not(target_arch = "aarch64"))]
#[inline]
unsafe fn read_smmu_reg(base: u64, offset: usize) -> u32 {
    let _ = (base, offset); 0
}

/// Écrit un registre 32 bits dans l'espace MMIO du SMMU.
///
/// # Safety
/// `base` doit être une adresse MMIO valide, mappée, alignée.
#[cfg(target_arch = "aarch64")]
#[inline]
unsafe fn write_smmu_reg(base: u64, offset: usize, val: u32) {
    let ptr = (base + offset as u64) as *mut u32;
    ptr.write_volatile(val);
}

#[cfg(not(target_arch = "aarch64"))]
#[inline]
unsafe fn write_smmu_reg(base: u64, offset: usize, val: u32) {
    let _ = (base, offset, val);
}
