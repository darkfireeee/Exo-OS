//! Linux DRM Compatibility Layer
//!
//! Provides compatibility shims for using GPL-2.0 Linux drivers
//! in Exo-OS. This allows wrapping existing, well-tested Linux
//! drivers without rewriting them from scratch.
//!
//! ## Architecture
//!
//! ```text
//! Linux Driver (GPL-2.0)
//!         ↓
//! DRM Compatibility Layer (this module)
//!         ↓
//! Exo-OS Driver Framework
//! ```
//!
//! ## Supported Abstractions
//!
//! - `struct device` - Generic device representation
//! - `struct driver` - Driver instance
//! - `struct pci_dev` - PCI device specifics
//! - DMA API - dma_alloc_coherent, etc.
//! - IRQ API - request_irq, free_irq
//! - Power Management - pm_ops
//!
//! ## License
//!
//! GPL-2.0 required for this module due to Linux driver compatibility.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use crate::memory::PhysAddr;

/// Device type abstraction (mimics Linux's struct device)
#[repr(C)]
pub struct Device {
    /// Device name
    pub name: String,
    
    /// Parent device (if any)
    pub parent: Option<Box<Device>>,
    
    /// Driver attached to this device
    pub driver: Option<*const Driver>,
    
    /// Private driver data
    pub driver_data: *mut u8,
    
    /// Bus type (PCI, USB, etc.)
    pub bus: Option<&'static Bus>,
    
    /// Power management state
    pub power_state: PowerState,
    
    /// DMA mask (supported address bits)
    pub dma_mask: u64,
    
    /// Device resources (memory, IRQ, etc.)
    pub resources: Vec<Resource>,
}

/// Driver abstraction (mimics Linux's struct driver)
#[repr(C)]
pub struct Driver {
    /// Driver name
    pub name: String,
    
    /// Bus this driver supports
    pub bus: &'static Bus,
    
    /// Probe function (called when device matches)
    pub probe: Option<fn(&mut Device) -> Result<(), i32>>,
    
    /// Remove function (called on device removal)
    pub remove: Option<fn(&mut Device)>,
    
    /// Shutdown function
    pub shutdown: Option<fn(&mut Device)>,
    
    /// Power management operations
    pub pm: Option<&'static PowerManagementOps>,
}

/// Bus type (PCI, USB, etc.)
#[repr(C)]
pub struct Bus {
    /// Bus name
    pub name: &'static str,
    
    /// Match function (device vs driver)
    pub match_fn: fn(&Device, &Driver) -> bool,
    
    /// Uevent function
    pub uevent: Option<fn(&Device) -> String>,
}

/// Power states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PowerState {
    D0 = 0, // Fully on
    D1 = 1, // Light sleep
    D2 = 2, // Deeper sleep
    D3Hot = 3, // Off, hot-pluggable
    D3Cold = 4, // Off, power removed
}

/// Power management operations
#[repr(C)]
pub struct PowerManagementOps {
    /// Suspend callback
    pub suspend: Option<fn(&mut Device) -> Result<(), i32>>,
    
    /// Resume callback
    pub resume: Option<fn(&mut Device) -> Result<(), i32>>,
    
    /// Runtime suspend
    pub runtime_suspend: Option<fn(&mut Device) -> Result<(), i32>>,
    
    /// Runtime resume
    pub runtime_resume: Option<fn(&mut Device) -> Result<(), i32>>,
}

/// Resource types (memory, I/O, IRQ, DMA)
#[derive(Debug, Clone)]
pub enum Resource {
    /// Memory-mapped I/O region
    Memory {
        start: PhysAddr,
        size: usize,
        name: String,
    },
    
    /// I/O port range
    Io {
        start: u16,
        end: u16,
        name: String,
    },
    
    /// Interrupt line
    Irq {
        number: u32,
        flags: u32,
        name: String,
    },
    
    /// DMA channel
    Dma {
        channel: u32,
    },
}

impl Device {
    /// Create new device
    pub fn new(name: String) -> Self {
        Self {
            name,
            parent: None,
            driver: None,
            driver_data: core::ptr::null_mut(),
            bus: None,
            power_state: PowerState::D0,
            dma_mask: 0xFFFF_FFFF_FFFF_FFFF, // 64-bit default
            resources: Vec::new(),
        }
    }
    
    /// Set driver-private data
    pub fn set_drvdata(&mut self, data: *mut u8) {
        self.driver_data = data;
    }
    
    /// Get driver-private data
    pub fn get_drvdata(&self) -> *mut u8 {
        self.driver_data
    }
    
    /// Add resource to device
    pub fn add_resource(&mut self, resource: Resource) {
        self.resources.push(resource);
    }
    
    /// Get resource by index
    pub fn get_resource(&self, index: usize) -> Option<&Resource> {
        self.resources.get(index)
    }
    
    /// Set DMA mask
    pub fn set_dma_mask(&mut self, mask: u64) -> Result<(), ()> {
        // Validate mask (must be contiguous bits)
        let bits = mask.count_ones();
        if mask != (1u64 << bits) - 1 {
            return Err(());
        }
        
        self.dma_mask = mask;
        Ok(())
    }
}

/// DMA API - Allocate coherent DMA memory
///
/// Allocates memory suitable for DMA operations (physically contiguous,
/// cache-coherent). Returns virtual address and DMA handle (physical address).
pub fn dma_alloc_coherent(
    dev: &Device,
    size: usize,
    dma_handle: &mut PhysAddr,
) -> Result<*mut u8, ()> {
    // Check size against DMA mask
    if size > (1 << dev.dma_mask.trailing_ones()) {
        return Err(());
    }
    
    // Allocate physically contiguous memory
    // TODO: Integrate with memory allocator
    let virt_addr = unsafe {
        alloc::alloc::alloc(
            alloc::alloc::Layout::from_size_align(size, 4096).unwrap()
        )
    };
    
    if virt_addr.is_null() {
        return Err(());
    }
    
    // Get physical address
    // TODO: Translate via page tables
    *dma_handle = PhysAddr::new(virt_addr as u64);
    
    Ok(virt_addr)
}

/// DMA API - Free coherent DMA memory
pub fn dma_free_coherent(
    _dev: &Device,
    size: usize,
    virt_addr: *mut u8,
    _dma_handle: PhysAddr,
) {
    if !virt_addr.is_null() {
        unsafe {
            alloc::alloc::dealloc(
                virt_addr,
                alloc::alloc::Layout::from_size_align(size, 4096).unwrap()
            );
        }
    }
}

/// IRQ flags (mimics Linux)
pub mod irq_flags {
    /// IRQ is shared between devices
    pub const IRQF_SHARED: u32 = 0x0080;
    
    /// IRQ handler is per-CPU
    pub const IRQF_PERCPU: u32 = 0x0100;
    
    /// IRQ cannot be threaded
    pub const IRQF_NO_THREAD: u32 = 0x0200;
}

/// IRQ handler function type
pub type IrqHandler = fn(irq: u32, dev_id: *mut u8) -> IrqReturn;

/// IRQ handler return values
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrqReturn {
    /// IRQ not from this device
    None,
    
    /// IRQ handled successfully
    Handled,
    
    /// IRQ needs threaded handler
    WakeThread,
}

/// Global IRQ registry
static IRQ_REGISTRY: Mutex<Vec<IrqEntry>> = Mutex::new(Vec::new());

/// IRQ entry in registry
struct IrqEntry {
    irq: u32,
    handler: IrqHandler,
    dev_id: *mut u8,
    name: String,
    flags: u32,
}

/// Request IRQ line
pub fn request_irq(
    irq: u32,
    handler: IrqHandler,
    flags: u32,
    name: String,
    dev_id: *mut u8,
) -> Result<(), i32> {
    let mut registry = IRQ_REGISTRY.lock();
    
    // Check if IRQ already registered (unless shared)
    if (flags & irq_flags::IRQF_SHARED) == 0 {
        if registry.iter().any(|e| e.irq == irq) {
            return Err(-16); // -EBUSY
        }
    }
    
    registry.push(IrqEntry {
        irq,
        handler,
        dev_id,
        name,
        flags,
    });
    
    // TODO: Enable IRQ in interrupt controller
    
    Ok(())
}

/// Free IRQ line
pub fn free_irq(irq: u32, dev_id: *mut u8) {
    let mut registry = IRQ_REGISTRY.lock();
    registry.retain(|e| !(e.irq == irq && e.dev_id == dev_id));
    
    // TODO: Disable IRQ if no more handlers
}

/// Handle IRQ (called from interrupt handler)
pub fn handle_irq(irq: u32) -> IrqReturn {
    let registry = IRQ_REGISTRY.lock();
    
    let mut handled = false;
    for entry in registry.iter().filter(|e| e.irq == irq) {
        let ret = (entry.handler)(irq, entry.dev_id);
        if ret == IrqReturn::Handled {
            handled = true;
        }
    }
    
    if handled {
        IrqReturn::Handled
    } else {
        IrqReturn::None
    }
}

/// Module loading support
pub struct Module {
    /// Module name
    pub name: String,
    
    /// Init function
    pub init: fn() -> Result<(), i32>,
    
    /// Exit function
    pub exit: fn(),
    
    /// License (must be GPL for Linux drivers)
    pub license: &'static str,
}

/// Global module registry
static MODULE_REGISTRY: Mutex<Vec<Box<Module>>> = Mutex::new(Vec::new());

/// Register kernel module
pub fn register_module(module: Module) -> Result<(), i32> {
    // Check license
    if !module.license.contains("GPL") {
        crate::logger::warn(&alloc::format!(
            "[DRM] Module {} has non-GPL license: {}",
            module.name, module.license
        ));
        return Err(-1); // -EPERM
    }
    
    // Call init function
    (module.init)?();
    
    // Add to registry
    let mut registry = MODULE_REGISTRY.lock();
    registry.push(Box::new(module));
    
    crate::logger::info(&alloc::format!(
        "[DRM] Registered module: {}",
        module.name
    ));
    
    Ok(())
}

/// Unregister kernel module
pub fn unregister_module(name: &str) {
    let mut registry = MODULE_REGISTRY.lock();
    
    if let Some(pos) = registry.iter().position(|m| m.name == name) {
        let module = registry.remove(pos);
        (module.exit)();
        
        crate::logger::info(&alloc::format!(
            "[DRM] Unregistered module: {}",
            name
        ));
    }
}

/// Export GPL symbol (for dynamic linking)
#[macro_export]
macro_rules! EXPORT_SYMBOL_GPL {
    ($name:ident) => {
        // TODO: Add to symbol table for module loading
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_device_creation() {
        let dev = Device::new(String::from("test-device"));
        assert_eq!(dev.name, "test-device");
        assert_eq!(dev.power_state, PowerState::D0);
        assert_eq!(dev.dma_mask, 0xFFFF_FFFF_FFFF_FFFF);
    }
    
    #[test]
    fn test_device_drvdata() {
        let mut dev = Device::new(String::from("test"));
        let data: u32 = 0x12345678;
        
        dev.set_drvdata(&data as *const u32 as *mut u8);
        let retrieved = dev.get_drvdata() as *const u32;
        
        unsafe {
            assert_eq!(*retrieved, 0x12345678);
        }
    }
    
    #[test]
    fn test_dma_mask() {
        let mut dev = Device::new(String::from("test"));
        
        // Valid masks (contiguous bits)
        assert!(dev.set_dma_mask(0xFFFFFFFF).is_ok()); // 32-bit
        assert!(dev.set_dma_mask(0xFFFF_FFFF_FFFF).is_ok()); // 48-bit
        
        // Invalid mask (non-contiguous)
        assert!(dev.set_dma_mask(0xFF00FF00).is_err());
    }
    
    #[test]
    fn test_resource_management() {
        let mut dev = Device::new(String::from("test"));
        
        dev.add_resource(Resource::Memory {
            start: PhysAddr::new(0x1000),
            size: 4096,
            name: String::from("BAR0"),
        });
        
        dev.add_resource(Resource::Irq {
            number: 11,
            flags: 0,
            name: String::from("MSI-0"),
        });
        
        assert_eq!(dev.resources.len(), 2);
        
        match dev.get_resource(0) {
            Some(Resource::Memory { size, .. }) => assert_eq!(*size, 4096),
            _ => panic!("Wrong resource type"),
        }
    }
}
