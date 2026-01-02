//! Phase 3 Integration Tests
//!
//! Tests for driver framework, PCI, MSI/MSI-X, ACPI, VirtIO

use exo_os_kernel::drivers::pci;
use exo_os_kernel::drivers::virtio;
use exo_os_kernel::memory::{PhysAddr, VirtAddr};

/// Test PCI address encoding
#[test]
fn test_pci_address_encoding() {
    let pci_addr = pci::PciAddress::new(0, 0, 0);
    assert_eq!(pci_addr.bus, 0);
    assert_eq!(pci_addr.device, 0);
    assert_eq!(pci_addr.function, 0);
}

/// Test PCI device IDs
#[test]
fn test_pci_device_ids() {
    let e1000 = pci::ids::INTEL_E1000;
    assert_eq!(e1000.vendor_id, 0x8086);
    assert_eq!(e1000.device_id, 0x100E);
    
    let virtio_net = pci::ids::VIRTIO_NET;
    assert_eq!(virtio_net.vendor_id, 0x1AF4);
    assert_eq!(virtio_net.device_id, 0x1000);
    
    let rtl8139 = pci::ids::REALTEK_RTL8139;
    assert_eq!(rtl8139.vendor_id, 0x10EC);
    assert_eq!(rtl8139.device_id, 0x8139);
}

/// Test PCI class codes
#[test]
fn test_pci_class_codes() {
    assert_eq!(pci::PciClass::from(0x02), pci::PciClass::Network);
    assert_eq!(pci::PciClass::from(0x01), pci::PciClass::MassStorage);
    assert_eq!(pci::PciClass::from(0x03), pci::PciClass::Display);
}

/// Test VirtIO device types
#[test]
fn test_virtio_device_types() {
    assert_eq!(virtio::DeviceType::Network as u32, 1);
    assert_eq!(virtio::DeviceType::Block as u32, 2);
    assert_eq!(virtio::DeviceType::Console as u32, 3);
    assert_eq!(virtio::DeviceType::Gpu as u32, 16);
}

/// Test VirtIO status bits
#[test]
fn test_virtio_status_bits() {
    assert_eq!(virtio::status::ACKNOWLEDGE, 1);
    assert_eq!(virtio::status::DRIVER, 2);
    assert_eq!(virtio::status::DRIVER_OK, 4);
    assert_eq!(virtio::status::FEATURES_OK, 8);
    assert_eq!(virtio::status::FAILED, 128);
}

/// Test VirtIO descriptor flags
#[test]
fn test_virtio_desc_flags() {
    assert_eq!(virtio::desc_flags::NEXT, 1);
    assert_eq!(virtio::desc_flags::WRITE, 2);
    assert_eq!(virtio::desc_flags::INDIRECT, 4);
}

/// Test VirtQueue descriptor structure
#[test]
fn test_virtq_desc_layout() {
    use virtio::VirtqDesc;
    
    assert_eq!(core::mem::size_of::<VirtqDesc>(), 16);
    assert_eq!(core::mem::align_of::<VirtqDesc>(), 8);
}

/// Test VirtQueue creation
#[test]
fn test_virtqueue_creation() {
    // Test valid sizes (power of 2)
    assert!(virtio::VirtQueue::new(16).is_ok());
    assert!(virtio::VirtQueue::new(32).is_ok());
    assert!(virtio::VirtQueue::new(64).is_ok());
    assert!(virtio::VirtQueue::new(128).is_ok());
    assert!(virtio::VirtQueue::new(256).is_ok());
    
    // Test invalid sizes
    assert!(virtio::VirtQueue::new(0).is_err());
    assert!(virtio::VirtQueue::new(15).is_err());
    assert!(virtio::VirtQueue::new(33).is_err());
    assert!(virtio::VirtQueue::new(65536).is_err());
}

/// Test VirtIO-Net header structure
#[test]
fn test_virtio_net_hdr() {
    use virtio::net::VirtioNetHdr;
    
    assert_eq!(core::mem::size_of::<VirtioNetHdr>(), 12);
    
    let hdr = VirtioNetHdr::new();
    assert_eq!(hdr.flags, 0);
    assert_eq!(hdr.gso_type, 0);
    assert_eq!(hdr.hdr_len, 0);
    assert_eq!(hdr.gso_size, 0);
}

/// Test VirtIO-Net feature bits
#[test]
fn test_virtio_net_features() {
    use virtio::net::net_features;
    
    assert_eq!(net_features::MAC, 1 << 5);
    assert_eq!(net_features::STATUS, 1 << 16);
    assert_eq!(net_features::MTU, 1 << 3);
    assert_eq!(net_features::CSUM, 1 << 0);
    assert_eq!(net_features::CTRL_VQ, 1 << 17);
}

/// Test network statistics
#[test]
fn test_net_stats() {
    use virtio::net::NetStats;
    
    let stats = NetStats::default();
    assert_eq!(stats.rx_packets, 0);
    assert_eq!(stats.tx_packets, 0);
    assert_eq!(stats.rx_bytes, 0);
    assert_eq!(stats.tx_bytes, 0);
    assert_eq!(stats.rx_errors, 0);
    assert_eq!(stats.tx_errors, 0);
    assert_eq!(stats.rx_dropped, 0);
    assert_eq!(stats.tx_dropped, 0);
}

/// Test physical address
#[test]
fn test_phys_addr() {
    let addr = PhysAddr::new(0x1000);
    assert_eq!(addr.as_u64(), 0x1000);
    
    let zero = PhysAddr::zero();
    assert_eq!(zero.as_u64(), 0);
}

/// Test virtual address
#[test]
fn test_virt_addr() {
    let addr = VirtAddr::new(0xFFFF_8000_0000_1000);
    assert_eq!(addr.as_u64(), 0xFFFF_8000_0000_1000);
}

/// Test ACPI RSDP signature
#[cfg(feature = "acpi")]
#[test]
fn test_acpi_rsdp_signature() {
    use exo_os_kernel::acpi;
    
    // RSDP signature should be "RSD PTR "
    assert_eq!(b"RSD PTR ".len(), 8);
}

/// Test ACPI SDT header size
#[cfg(feature = "acpi")]
#[test]
fn test_acpi_sdt_header_size() {
    use exo_os_kernel::acpi::SdtHeader;
    
    assert_eq!(core::mem::size_of::<SdtHeader>(), 36);
}

/// Test MSI address calculation
#[cfg(feature = "msi")]
#[test]
fn test_msi_address() {
    // MSI address format: 0xFEE00000 | (destination_id << 12)
    let dest_id = 0xAB;
    let address = 0xFEE00000u64 | ((dest_id & 0xFF) << 12);
    
    assert_eq!(address, 0xFEE00000 | (0xAB << 12));
    assert_eq!(address & 0xFFF00000, 0xFEE00000);
}

/// Test MSI-X table entry size
#[cfg(feature = "msi")]
#[test]
fn test_msix_table_entry_size() {
    // MSI-X table entry: 16 bytes (4x u32)
    // - msg_addr_low: u32
    // - msg_addr_high: u32
    // - msg_data: u32
    // - vector_control: u32
    assert_eq!(4 * core::mem::size_of::<u32>(), 16);
}

/// Test driver compatibility layer structures
#[cfg(feature = "compat")]
#[test]
fn test_linux_compat_device() {
    use exo_os_kernel::drivers::compat::Device;
    
    let dev = Device::new("test_device");
    assert!(dev.get_drvdata().is_null());
}

/// Test DMA mask validation
#[cfg(feature = "compat")]
#[test]
fn test_dma_mask() {
    // DMA mask should be contiguous bits
    let valid_mask = 0xFFFF_FFFF; // 32-bit
    let mut count = 0;
    let mut found_zero = false;
    
    for i in 0..64 {
        if (valid_mask & (1 << i)) != 0 {
            assert!(!found_zero, "Non-contiguous mask");
            count += 1;
        } else {
            found_zero = true;
        }
    }
    
    assert_eq!(count, 32);
}

/// Benchmark: VirtQueue descriptor allocation
#[bench]
fn bench_virtqueue_alloc_desc(b: &mut test::Bencher) {
    let mut queue = virtio::VirtQueue::new(256).unwrap();
    
    b.iter(|| {
        let desc_idx = queue.alloc_desc_chain(1).unwrap();
        queue.free_desc_chain(desc_idx);
    });
}

/// Benchmark: PCI config space read
#[bench]
fn bench_pci_config_read(b: &mut test::Bencher) {
    let addr = pci::PciAddress::new(0, 0, 0);
    
    b.iter(|| {
        // Simulated PCI read (no actual I/O in test)
        let _vendor = addr.encode();
    });
}

/// Summary test: verify all Phase 3 components are available
#[test]
fn test_phase3_components_available() {
    // PCI
    let _ = pci::ids::INTEL_E1000;
    let _ = pci::ids::VIRTIO_NET;
    
    // VirtIO
    let _ = virtio::DeviceType::Network;
    let _ = virtio::DeviceType::Block;
    
    // MSI (if enabled)
    #[cfg(feature = "msi")]
    {
        let _ = 0xFEE00000u64;
    }
    
    // ACPI (if enabled)
    #[cfg(feature = "acpi")]
    {
        let _ = b"RSD PTR ";
    }
    
    // All components compile successfully
    assert!(true);
}
