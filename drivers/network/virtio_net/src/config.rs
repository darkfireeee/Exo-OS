pub const VIRTIO_MMIO_MAGIC_VALUE: usize = 0x000;
pub const VIRTIO_MMIO_VERSION: usize = 0x004;
pub const VIRTIO_MMIO_DEVICE_ID: usize = 0x008;
pub const VIRTIO_MMIO_VENDOR_ID: usize = 0x00C;
pub const VIRTIO_MMIO_DEVICE_FEATURES: usize = 0x010;
pub const VIRTIO_MMIO_DEVICE_FEATURES_SEL: usize = 0x014;
pub const VIRTIO_MMIO_DRIVER_FEATURES: usize = 0x020;
pub const VIRTIO_MMIO_DRIVER_FEATURES_SEL: usize = 0x024;
pub const VIRTIO_MMIO_QUEUE_SEL: usize = 0x030;
pub const VIRTIO_MMIO_QUEUE_NUM_MAX: usize = 0x034;
pub const VIRTIO_MMIO_QUEUE_NUM: usize = 0x038;
pub const VIRTIO_MMIO_QUEUE_ALIGN: usize = 0x03C;
pub const VIRTIO_MMIO_QUEUE_PFN: usize = 0x040;
pub const VIRTIO_MMIO_QUEUE_READY: usize = 0x044;
pub const VIRTIO_MMIO_QUEUE_NOTIFY: usize = 0x050;
pub const VIRTIO_MMIO_INTERRUPT_STATUS: usize = 0x060;
pub const VIRTIO_MMIO_INTERRUPT_ACK: usize = 0x064;
pub const VIRTIO_MMIO_STATUS: usize = 0x070;
pub const VIRTIO_MMIO_CONFIG: usize = 0x100;

pub const VIRTIO_STATUS_ACKNOWLEDGE: u32 = 1;
pub const VIRTIO_STATUS_DRIVER: u32 = 2;
pub const VIRTIO_STATUS_DRIVER_OK: u32 = 4;
pub const VIRTIO_STATUS_FEATURES_OK: u32 = 8;
pub const VIRTIO_STATUS_FAILED: u32 = 128;

pub const VIRTIO_NET_F_CSUM: u64 = 1u64 << 0;
pub const VIRTIO_NET_F_GUEST_CSUM: u64 = 1u64 << 1;
pub const VIRTIO_NET_F_MAC: u64 = 1u64 << 5;
pub const VIRTIO_NET_F_MRG_RXBUF: u64 = 1u64 << 15;
pub const VIRTIO_NET_F_STATUS: u64 = 1u64 << 16;
pub const VIRTIO_F_VERSION_1: u64 = 1u64 << 32;

pub const VIRTIO_PCI_VENDOR: u16 = 0x1AF4;
pub const VIRTIO_NET_PCI_DEVICE_LEGACY: u16 = 0x1000;
pub const VIRTIO_NET_PCI_DEVICE_MODERN: u16 = 0x1041;

pub const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;
pub const VIRTIO_PCI_CAP_NOTIFY_CFG: u8 = 2;
pub const VIRTIO_PCI_CAP_ISR_CFG: u8 = 3;
pub const VIRTIO_PCI_CAP_DEVICE_CFG: u8 = 4;

pub const VIRTIO_PCI_COMMON_DEVICE_FEATURE_SELECT: usize = 0x00;
pub const VIRTIO_PCI_COMMON_DEVICE_FEATURE: usize = 0x04;
pub const VIRTIO_PCI_COMMON_DRIVER_FEATURE_SELECT: usize = 0x08;
pub const VIRTIO_PCI_COMMON_DRIVER_FEATURE: usize = 0x0C;
pub const VIRTIO_PCI_COMMON_DEVICE_STATUS: usize = 0x14;
pub const VIRTIO_PCI_COMMON_QUEUE_SELECT: usize = 0x16;
pub const VIRTIO_PCI_COMMON_QUEUE_SIZE: usize = 0x18;
pub const VIRTIO_PCI_COMMON_QUEUE_ENABLE: usize = 0x1C;
pub const VIRTIO_PCI_COMMON_QUEUE_NOTIFY_OFF: usize = 0x1E;
pub const VIRTIO_PCI_COMMON_QUEUE_DESC: usize = 0x20;
pub const VIRTIO_PCI_COMMON_QUEUE_DRIVER: usize = 0x28;
pub const VIRTIO_PCI_COMMON_QUEUE_DEVICE: usize = 0x30;

pub const VIRTIO_NET_HDR_SIZE_LEGACY: usize = 10;
pub const VIRTIO_NET_HDR_SIZE_MRG: usize = 12;
pub const VIRTIO_NET_HDR_SIZE_MODERN: usize = 12;
pub const VRING_QUEUE_SIZE: u16 = 256;
pub const PAGE_SIZE: usize = 4096;
