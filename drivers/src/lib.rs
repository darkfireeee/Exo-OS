//! # exo_os_drivers — Framework universel des pilotes Ring 3 d'Exo-OS
//!
//! Conforme aux spécifications formelles TLA+ :
//! - S24 : Topologie PCI avant IRQ (`SYS_PCI_SET_TOPOLOGY` avant `SYS_IRQ_REGISTER`).
//! - S19/S20 : Unicité atomique BDF/MMIO (`SYS_PCI_CLAIM`).
//! - S39/S40 : Whitelist IOMMU immuable pour DMA.
//! - S21-S23 : Teardown `do_exit` en 8 étapes avec link retraining.
//! - SRV-02 : #![no_std] pur, arithmétique entière, aucun flottant ni SIMD.

#![no_std]
#![allow(dead_code)]

use exo_syscall_abi as syscall;

/// Critères de correspondance de périphérique matériel (par ordre de priorité)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceMatch {
    /// Correspondance exacte Vendor ID / Device ID (Priorité 1)
    Exact { vendor: u16, device: u16 },
    /// Correspondance par classe et sous-classe PCI (Priorité 2)
    ClassSub { class: u8, sub: u8 },
    /// Correspondance par classe PCI seule (Priorité 3 - fallback)
    Class { class: u8 },
}

/// Résultat du sondage (probe) d'un périphérique
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeResult {
    /// Support garanti et optimal
    Yes,
    /// Support générique ou dégradé
    Maybe,
    /// Périphérique non pris en charge
    No,
}

/// Résultat du traitement d'une interruption matérielle (IRQ)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrqResult {
    /// Interruption traitée avec succès par ce pilote
    Handled,
    /// Interruption non destinée à ce pilote (ligne partagée)
    NotMine,
    /// Erreur matérielle ou protocolaire
    Error(DriverError),
}

/// Codes d'erreur standard des pilotes matériels Exo-OS
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverError {
    DeviceNotFound,
    ClaimFailed(i64),
    MmioMapFailed(i64),
    IrqRegisterFailed(i64),
    DmaAllocFailed(i64),
    BusMasterFailed(i64),
    TopologyFailed(i64),
    HardwareTimeout,
    InvalidConfiguration,
    IoError(i64),
}

/// Description matérielle découverte via PCI ou table statique
#[derive(Debug, Clone, Copy)]
pub struct DeviceInfo {
    pub vendor_id: u16,
    pub device_id: u16,
    pub class: u8,
    pub subclass: u8,
    pub bdf_raw: u32,
    pub bar0_phys: u64,
    pub bar0_size: u64,
    pub irq_line: u8,
}

/// Région MMIO mappée dans l'espace virtuel du pilote
#[derive(Debug, Clone, Copy)]
pub struct MmioRegion {
    pub phys_addr: u64,
    pub virt_addr: *mut u8,
    pub size: usize,
}

impl MmioRegion {
    pub const fn empty() -> Self {
        Self {
            phys_addr: 0,
            virt_addr: core::ptr::null_mut(),
            size: 0,
        }
    }

    #[inline]
    pub unsafe fn read8(&self, offset: usize) -> u8 {
        if offset + 1 <= self.size && !self.virt_addr.is_null() {
            core::ptr::read_volatile(self.virt_addr.add(offset) as *const u8)
        } else {
            0
        }
    }

    #[inline]
    pub unsafe fn write8(&self, offset: usize, value: u8) {
        if offset + 1 <= self.size && !self.virt_addr.is_null() {
            core::ptr::write_volatile(self.virt_addr.add(offset) as *mut u8, value);
        }
    }

    #[inline]
    pub unsafe fn read16(&self, offset: usize) -> u16 {
        if offset + 2 <= self.size && !self.virt_addr.is_null() {
            core::ptr::read_volatile(self.virt_addr.add(offset) as *const u16)
        } else {
            0
        }
    }

    #[inline]
    pub unsafe fn write16(&self, offset: usize, value: u16) {
        if offset + 2 <= self.size && !self.virt_addr.is_null() {
            core::ptr::write_volatile(self.virt_addr.add(offset) as *mut u16, value);
        }
    }

    #[inline]
    pub unsafe fn read32(&self, offset: usize) -> u32 {
        if offset + 4 <= self.size && !self.virt_addr.is_null() {
            core::ptr::read_volatile(self.virt_addr.add(offset) as *const u32)
        } else {
            0
        }
    }

    #[inline]
    pub unsafe fn write32(&self, offset: usize, value: u32) {
        if offset + 4 <= self.size && !self.virt_addr.is_null() {
            core::ptr::write_volatile(self.virt_addr.add(offset) as *mut u32, value);
        }
    }
}

/// Handle de ligne d'interruption enregistrée
#[derive(Debug, Clone, Copy)]
pub struct IrqHandle {
    pub irq_line: u8,
    pub reg_id: u64,
    pub handler_gen: u64,
}

impl IrqHandle {
    pub const fn empty() -> Self {
        Self {
            irq_line: 0,
            reg_id: 0,
            handler_gen: 0,
        }
    }

    /// Acquittement explicite de l'interruption conforme IrqRouting.tla
    pub fn ack(&self, wave_gen: u64, handled: bool) -> Result<(), DriverError> {
        let result_raw = if handled { 0 } else { 1 };
        let rc = unsafe {
            syscall::syscall5(
                syscall::SYS_IRQ_ACK,
                self.irq_line as u64 + 32,
                self.reg_id,
                self.handler_gen,
                wave_gen,
                result_raw,
            )
        };
        if rc < 0 {
            Err(DriverError::IoError(rc))
        } else {
            Ok(())
        }
    }
}

/// Handle de périphérique contenant toutes les ressources allouées
pub struct DeviceHandle {
    pub info: DeviceInfo,
    pub mmio: MmioRegion,
    pub irq: IrqHandle,
    pub pid: u64,
}

impl DeviceHandle {
    /// Initialise et revendique un périphérique PCI en respectant les invariants TLA+
    pub fn claim_and_map(info: DeviceInfo) -> Result<Self, DriverError> {
        let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
        if pid <= 0 {
            return Err(DriverError::ClaimFailed(syscall::EACCES));
        }

        // 1. Revendication atomique BDF / MMIO (TLA+ S19, S20)
        let claim = unsafe {
            syscall::syscall5(
                syscall::SYS_PCI_CLAIM,
                info.bar0_phys,
                info.bar0_size,
                pid as u64,
                info.bdf_raw as u64,
                1,
            )
        };
        if claim < 0 {
            return Err(DriverError::ClaimFailed(claim));
        }

        // 2. Mapping MMIO en mémoire virtuelle de l'espace utilisateur
        let mapped = unsafe { syscall::syscall2(syscall::SYS_MMIO_MAP, info.bar0_phys, info.bar0_size) };
        if mapped < 0 {
            return Err(DriverError::MmioMapFailed(mapped));
        }

        // 3. Activation du PCI Bus Master
        let bm = unsafe { syscall::syscall1(syscall::SYS_PCI_BUS_MASTER, 1) };
        if bm < 0 {
            return Err(DriverError::BusMasterFailed(bm));
        }

        // 4. Invariant TLA+ S24 : Définition de la topologie avant tout enregistrement d'IRQ
        let topo = unsafe {
            syscall::syscall3(
                syscall::SYS_PCI_SET_TOPOLOGY,
                info.bdf_raw as u64,
                0, // Root bridge direct
                1, // has_parent
            )
        };
        if topo < 0 {
            return Err(DriverError::TopologyFailed(topo));
        }

        Ok(Self {
            info,
            mmio: MmioRegion {
                phys_addr: info.bar0_phys,
                virt_addr: mapped as u64 as *mut u8,
                size: info.bar0_size as usize,
            },
            irq: IrqHandle::empty(),
            pid: pid as u64,
        })
    }

    /// Enregistre le handler d'interruption pour ce périphérique
    pub fn register_irq(&mut self, endpoint_id: u64) -> Result<(), DriverError> {
        if self.info.irq_line == 0 || self.info.irq_line == u8::MAX {
            return Err(DriverError::InvalidConfiguration);
        }
        let rc = unsafe {
            syscall::syscall6(
                syscall::SYS_IRQ_REGISTER,
                self.info.irq_line as u64 + 32,
                endpoint_id,
                0,
                1, // IOAPIC_LEVEL
                self.info.bdf_raw as u64,
                1,
            )
        };
        if rc < 0 {
            Err(DriverError::IrqRegisterFailed(rc))
        } else {
            self.irq = IrqHandle {
                irq_line: self.info.irq_line,
                reg_id: rc as u64,
                handler_gen: 1,
            };
            Ok(())
        }
    }
}

/// Trait universel pour les pilotes matériels en espace utilisateur (Ring 3)
pub trait ExoDriver: Send + Sync {
    /// Critères de correspondance supportés par ce pilote
    fn supported_ids() -> &'static [DeviceMatch]
    where
        Self: Sized;

    /// Teste si le périphérique est pris en charge
    fn probe(&self, info: &DeviceInfo) -> ProbeResult;

    /// Initialise et démarre le pilote sur le périphérique
    fn attach(&mut self, handle: DeviceHandle) -> Result<(), DriverError>;

    /// Traitement d'interruption matérielle (DOIT retourner < 5ms)
    fn handle_irq(&mut self, irq: u8) -> IrqResult;

    /// Détache et désalloue les ressources du périphérique
    fn detach(&mut self);

    /// Mise en veille / gestion d'énergie
    fn suspend(&mut self) -> Result<(), DriverError> {
        Ok(())
    }

    /// Sortie de veille
    fn resume(&mut self) -> Result<(), DriverError> {
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Traits spécialisés par classe de périphérique
// ─────────────────────────────────────────────────────────────────────────────

/// Erreur d'accès aux unités de stockage par blocs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveError {
    IoError,
    Timeout,
    InvalidLba,
    OutOfBounds,
    WriteProtected,
    DeviceBusy,
    UnsupportedOperation,
}

/// Interface d'accès par blocs pour les périphériques de stockage (NVMe, AHCI, VirtIO-Blk)
pub trait BlockDevice: Send + Sync {
    fn read_blocks(&self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), DriveError>;
    fn write_blocks(&self, lba: u64, data: &[u8]) -> Result<(), DriveError>;
    fn block_count(&self) -> u64;
    fn block_size(&self) -> u32;
    fn flush(&self) -> Result<(), DriveError>;
}

/// Événement clavier / souris / tactile
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputEvent {
    pub timestamp_ms: u64,
    pub event_type: u16,
    pub code: u16,
    pub value: i32,
}

/// Drapeaux de statut des LEDs clavier
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedFlags(pub u8);

impl LedFlags {
    pub const SCROLL_LOCK: Self = Self(1 << 0);
    pub const NUM_LOCK: Self = Self(1 << 1);
    pub const CAPS_LOCK: Self = Self(1 << 2);
}

/// Interface pour les périphériques d'entrée (PS/2, USB HID)
pub trait InputDevice: Send + Sync {
    fn poll_event(&self) -> Option<InputEvent>;
    fn set_led(&self, led: LedFlags);
}

/// Erreurs de périphériques audio
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioError {
    BufferFull,
    DeviceBusy,
    FormatUnsupported,
    HwError,
}

/// Interface pour les périphériques de sortie audio (Intel HDA)
pub trait AudioDevice: Send + Sync {
    fn play_pcm(&self, data: &[i16], rate: u32, channels: u8) -> Result<(), AudioError>;
    fn stop(&self);
}

/// Erreurs réseau bas niveau
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetError {
    LinkDown,
    TxQueueFull,
    RxQueueEmpty,
    BufferTooSmall,
    IoError,
}

/// Interface générique pour contrôleurs de réseau (NICs)
pub trait NetDevice: Send + Sync {
    fn send(&self, frame: &[u8]) -> Result<(), NetError>;
    fn recv(&self, buf: &mut [u8]) -> Result<usize, NetError>;
    fn mac_addr(&self) -> [u8; 6];
}

/// Classes de périphériques matériel pour hotplug et routage
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceClass {
    Storage,
    Network,
    Display,
    Input,
    Audio,
    Bridge,
    Serial,
    Other(u8),
}

/// Événement de gestionnaire de périphériques (hotplug / unbind)
#[derive(Debug, Clone, Copy)]
pub enum DeviceEvent {
    Attached {
        device_id: u32,
        class: DeviceClass,
        info: DeviceInfo,
    },
    Detached {
        device_id: u32,
    },
}
