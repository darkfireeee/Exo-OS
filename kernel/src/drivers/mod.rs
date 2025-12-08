//! Hardware Drivers - Phase 0 (Minimal)
//!
//! Phase 0: VGA + Serial uniquement
//! Phase 1+: PCI, Network, Block, USB, etc.

// ═══════════════════════════════════════════════════════════
//  PHASE 0 - Drivers essentiels
// ═══════════════════════════════════════════════════════════
pub mod char;   // ✅ Serial + Console
pub mod video;  // ✅ VGA text mode

// ═══════════════════════════════════════════════════════════
//  PHASE 1+ - Drivers désactivés
// ═══════════════════════════════════════════════════════════
// pub mod block;   // ⏸️ Phase 2: ATA/AHCI/NVMe
// pub mod input;   // ⏸️ Phase 2: Keyboard/Mouse
// pub mod net;     // ⏸️ Phase 3: E1000, RTL8139, VirtIO-Net
// pub mod pci;     // ⏸️ Phase 1: PCI bus enumeration
// pub mod usb;     // ⏸️ Phase 3: USB stack
// pub mod virtio;  // ⏸️ Phase 3: VirtIO devices

/// Error type for driver operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverError {
    InitFailed,
    DeviceNotFound,
    IoError,
    NotSupported,
    Timeout,
    InvalidParameter,
    ResourceBusy,
    NoMemory,
}

/// Result type for driver operations.
pub type DriverResult<T> = Result<T, DriverError>;

/// Basic information about a device.
pub struct DeviceInfo {
    pub name: &'static str,
    pub vendor_id: u16,
    pub device_id: u16,
}

/// Trait that all drivers must implement.
pub trait Driver {
    /// Returns the name of the driver.
    fn name(&self) -> &str;

    /// Initializes the driver.
    fn init(&mut self) -> DriverResult<()>;

    /// Probes for the device.
    fn probe(&self) -> DriverResult<DeviceInfo>;
}

/// Initialize hardware drivers (Phase 0: VGA + Serial uniquement)
pub fn init() {
    // Phase 0: VGA et Serial déjà initialisés dans boot
    // Rien à faire ici pour l'instant
    
    // ⏸️ Phase 1+: PCI, Network, Block, etc.
    // log::info!("Initializing hardware drivers...");
    // pci::init();
    // net::e1000::init();
    // net::rtl8139::init();
    // net::virtio_net::init();
}
