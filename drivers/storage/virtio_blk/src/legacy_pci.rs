use core::mem::{align_of, size_of};
use core::ptr::NonNull;

use virtio_drivers::transport::{DeviceStatus, DeviceType, Transport};
use virtio_drivers::{Error, Result};

const DEVICE_FEATURES: u16 = 0;
const DRIVER_FEATURES: u16 = 4;
const QUEUE_PFN: u16 = 8;
const QUEUE_SIZE: u16 = 12;
const QUEUE_SELECT: u16 = 14;
const QUEUE_NOTIFY: u16 = 16;
const DEVICE_STATUS: u16 = 18;
const ISR_STATUS: u16 = 19;
const CONFIG_SPACE: u16 = 20;
const CONFIG_CACHE_BYTES: usize = 64;

#[repr(C, align(4))]
struct ConfigCache {
    bytes: [u8; CONFIG_CACHE_BYTES],
}

impl ConfigCache {
    const fn zeroed() -> Self {
        Self {
            bytes: [0; CONFIG_CACHE_BYTES],
        }
    }

    fn write_u32(&mut self, offset: usize, value: u32) {
        if let Some(dst) = self.bytes.get_mut(offset..offset.saturating_add(4)) {
            dst.copy_from_slice(&value.to_le_bytes());
        }
    }
}

/// VirtIO legacy PCI transport backed by the first I/O BAR of a transitional
/// virtio-blk device.
///
/// This transport follows the legacy PCI register layout and intentionally does
/// not emulate storage. All data path operations go through the VirtIO queue
/// and the host block backend.
pub struct LegacyPciTransport {
    io_base: u16,
    config: ConfigCache,
}

impl LegacyPciTransport {
    pub fn new(io_base: u16) -> Self {
        let mut transport = Self {
            io_base,
            config: ConfigCache::zeroed(),
        };
        transport.refresh_config_cache();
        transport
    }

    fn port(&self, offset: u16) -> u16 {
        self.io_base.wrapping_add(offset)
    }

    fn refresh_config_cache(&mut self) {
        self.config.write_u32(0, unsafe { self.config_read32(0) });
        self.config.write_u32(4, unsafe { self.config_read32(4) });
        self.config.write_u32(20, unsafe { self.config_read32(20) });
    }

    unsafe fn config_read32(&self, offset: u16) -> u32 {
        unsafe { inl(self.port(CONFIG_SPACE + offset)) }
    }
}

impl Transport for LegacyPciTransport {
    fn device_type(&self) -> DeviceType {
        DeviceType::Block
    }

    fn read_device_features(&mut self) -> u64 {
        unsafe { inl(self.port(DEVICE_FEATURES)) as u64 }
    }

    fn write_driver_features(&mut self, driver_features: u64) {
        unsafe {
            outl(self.port(DRIVER_FEATURES), driver_features as u32);
        }
    }

    fn max_queue_size(&mut self, queue: u16) -> u32 {
        unsafe {
            outw(self.port(QUEUE_SELECT), queue);
            inw(self.port(QUEUE_SIZE)) as u32
        }
    }

    fn notify(&mut self, queue: u16) {
        unsafe {
            outw(self.port(QUEUE_NOTIFY), queue);
        }
    }

    fn get_status(&self) -> DeviceStatus {
        DeviceStatus::from_bits_truncate(unsafe { inb(self.port(DEVICE_STATUS)) as u32 })
    }

    fn set_status(&mut self, status: DeviceStatus) {
        unsafe {
            outb(self.port(DEVICE_STATUS), status.bits() as u8);
        }
    }

    fn set_guest_page_size(&mut self, _guest_page_size: u32) {}

    fn requires_legacy_layout(&self) -> bool {
        true
    }

    fn queue_set(
        &mut self,
        queue: u16,
        _size: u32,
        descriptors: virtio_drivers::PhysAddr,
        _driver_area: virtio_drivers::PhysAddr,
        _device_area: virtio_drivers::PhysAddr,
    ) {
        let pfn = descriptors / virtio_drivers::PAGE_SIZE;
        unsafe {
            outw(self.port(QUEUE_SELECT), queue);
            outl(self.port(QUEUE_PFN), pfn as u32);
        }
    }

    fn queue_unset(&mut self, queue: u16) {
        unsafe {
            outw(self.port(QUEUE_SELECT), queue);
            outl(self.port(QUEUE_PFN), 0);
        }
    }

    fn queue_used(&mut self, queue: u16) -> bool {
        unsafe {
            outw(self.port(QUEUE_SELECT), queue);
            inl(self.port(QUEUE_PFN)) != 0
        }
    }

    fn ack_interrupt(&mut self) -> bool {
        unsafe { inb(self.port(ISR_STATUS)) != 0 }
    }

    fn config_space<T: 'static>(&self) -> Result<NonNull<T>> {
        if size_of::<T>() > CONFIG_CACHE_BYTES {
            return Err(Error::ConfigSpaceTooSmall);
        }
        if align_of::<T>() > align_of::<ConfigCache>() {
            return Err(Error::InvalidParam);
        }
        NonNull::new(self.config.bytes.as_ptr() as *mut T).ok_or(Error::ConfigSpaceMissing)
    }
}

impl Drop for LegacyPciTransport {
    fn drop(&mut self) {
        self.set_status(DeviceStatus::empty());
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    unsafe {
        core::arch::asm!(
            "in al, dx",
            in("dx") port,
            out("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }
    value
}

#[cfg(target_arch = "x86_64")]
unsafe fn inw(port: u16) -> u16 {
    let value: u16;
    unsafe {
        core::arch::asm!(
            "in ax, dx",
            in("dx") port,
            out("ax") value,
            options(nomem, nostack, preserves_flags)
        );
    }
    value
}

#[cfg(target_arch = "x86_64")]
unsafe fn inl(port: u16) -> u32 {
    let value: u32;
    unsafe {
        core::arch::asm!(
            "in eax, dx",
            in("dx") port,
            out("eax") value,
            options(nomem, nostack, preserves_flags)
        );
    }
    value
}

#[cfg(target_arch = "x86_64")]
unsafe fn outb(port: u16, value: u8) {
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn outw(port: u16, value: u16) {
    unsafe {
        core::arch::asm!(
            "out dx, ax",
            in("dx") port,
            in("ax") value,
            options(nomem, nostack, preserves_flags)
        );
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn outl(port: u16, value: u32) {
    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") port,
            in("eax") value,
            options(nomem, nostack, preserves_flags)
        );
    }
}

#[cfg(not(target_arch = "x86_64"))]
compile_error!("exo-virtio-blk legacy PCI transport currently requires x86_64 I/O ports");
