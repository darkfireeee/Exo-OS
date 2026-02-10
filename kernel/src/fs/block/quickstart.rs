//! Quick Start Guide for Block Device Layer
//!
//! This file provides quick examples to get started with the block device layer.

#![allow(dead_code)]

use crate::fs::block::*;
use crate::fs::FsResult;
use alloc::sync::Arc;
use spin::RwLock;

/// Quick Start Example 1: Basic RAM Disk
///
/// Creates a RAM disk, reads and writes data.
pub fn quick_start_ramdisk() -> FsResult<()> {
    // Create a 16MB RAM disk with 512-byte blocks
    let device = device::RamDisk::new("ram0".into(), 16 * 1024 * 1024, 512);

    // Write some data
    let write_buffer = [0xAB; 512];
    device.write().write(0, &write_buffer)?;

    // Read it back
    let mut read_buffer = [0u8; 512];
    device.read().read(0, &mut read_buffer)?;

    assert_eq!(write_buffer, read_buffer);
    log::info!("RAM Disk test passed!");

    Ok(())
}

/// Quick Start Example 2: Using the Global Registry
pub fn quick_start_registry() -> FsResult<()> {
    // Create and register a device
    let device = create_test_ramdisk("disk0", 32);

    // Later, retrieve it by name
    if let Some(dev) = get_device("disk0") {
        log::info!("Device found: {} MB", dev.read().size() / 1024 / 1024);
    }

    // List all devices
    for info in list_devices() {
        log::info!("Device: {}, Size: {} MB, Type: {:?}",
                   info.name,
                   info.size / 1024 / 1024,
                   info.device_type);
    }

    Ok(())
}

/// Quick Start Example 3: I/O Scheduler
pub fn quick_start_scheduler() -> FsResult<()> {
    let device = create_test_ramdisk("disk1", 64);

    // Wrap with Deadline scheduler (best for interactive workloads)
    let scheduled = scheduler::ScheduledDevice::new(
        device,
        scheduler::SchedulerType::Deadline,
    );

    // Create a high-priority read request
    let request = scheduler::IoRequest::new(
        scheduler::IoOperation::Read,
        0,      // LBA
        8,      // Count (8 blocks = 4KB)
    ).with_priority(0)  // Highest priority
     .with_deadline(1_000_000);  // 1ms deadline

    // Submit request
    scheduled.read().submit(request)?;

    // Process all pending requests
    while scheduled.write().process_next()? {
        // Processing...
    }

    log::info!("Queue depth: {}", scheduled.read().queue_depth());

    Ok(())
}

/// Quick Start Example 4: I/O Statistics
pub fn quick_start_stats() {
    let stats = stats::IoStats::new();

    // Simulate some I/O operations
    for i in 0..100 {
        let latency = 50_000 + (i * 1000); // 50-150us
        stats.record_read(4096, latency, i * 8, true);
    }

    // Get current statistics
    let snapshot = stats.snapshot();

    log::info!("=== I/O Statistics ===");
    log::info!("Total reads: {}", snapshot.reads);
    log::info!("Bytes read: {} KB", snapshot.bytes_read / 1024);
    log::info!("Avg latency: {} us", snapshot.avg_read_latency_ns / 1000);
    log::info!("Max latency: {} us", snapshot.max_read_latency_ns / 1000);
    log::info!("Read IOPS: {}", snapshot.read_iops);
    log::info!("Throughput: {} MB/s", snapshot.read_throughput_bps / 1_000_000);
}

/// Quick Start Example 5: Simple RAID 1 (Mirror)
pub fn quick_start_raid1() -> FsResult<()> {
    // Create two identical disks
    let disk1 = device::RamDisk::new("disk1".into(), 128 * 1024 * 1024, 512);
    let disk2 = device::RamDisk::new("disk2".into(), 128 * 1024 * 1024, 512);

    // Create RAID 1 configuration
    let config = raid::RaidConfig::new(
        raid::RaidLevel::Raid1,
        64 * 1024,  // Chunk size (doesn't matter much for RAID 1)
        "mirror0".into(),
    );

    // Build RAID array
    let devices: alloc::vec::Vec<Arc<RwLock<dyn BlockDevice>>> = alloc::vec![
        disk1 as Arc<RwLock<dyn BlockDevice>>,
        disk2 as Arc<RwLock<dyn BlockDevice>>,
    ];

    let raid = raid::RaidArray::new(config, devices)?;

    // Write to RAID (writes to both disks)
    let data = [0x55; 512];
    raid.write().write(0, &data)?;

    // Read from RAID (reads from any disk)
    let mut buffer = [0u8; 512];
    raid.read().read(0, &mut buffer)?;

    assert_eq!(data, buffer);
    log::info!("RAID 1 test passed!");

    Ok(())
}

/// Quick Start Example 6: NVMe Optimization
pub fn quick_start_nvme() -> FsResult<()> {
    let device = create_test_ramdisk("nvme0", 256);

    // Wrap with NVMe optimizer (4 I/O queues, max 2048 queue depth)
    let nvme_dev = nvme::NvmeDevice::new(device, 2048, 4);

    // Get optimizer handle
    let optimizer = nvme_dev.read().optimizer();

    // Configure queue depth
    optimizer.read().set_queue_depth(1024)?;

    // Simulate I/O submission
    let request = scheduler::IoRequest::new(
        scheduler::IoOperation::Read,
        0,
        16,
    );

    nvme_dev.read().submit_io(&request)?;

    // Auto-tune based on latency (simulated 75us average)
    nvme_dev.read().auto_tune(75);

    // Get statistics
    let stats = nvme_dev.read().stats();
    log::info!("NVMe Queue Depth: {}/{}",
               stats.queue_depth,
               stats.max_queue_depth);
    log::info!("Commands in flight: {}", stats.commands_in_flight);

    Ok(())
}

/// Quick Start Example 7: Partition Detection
pub fn quick_start_partitions() -> FsResult<()> {
    // In a real scenario, this would be a physical disk
    let device = create_test_ramdisk("sda", 1024);

    // Detect partitions
    let partition_table = partition::PartitionTable::detect(&*device.read())?;

    match partition_table.table_type {
        partition::PartitionTableType::MBR => {
            log::info!("MBR partition table detected");
        }
        partition::PartitionTableType::GPT => {
            log::info!("GPT partition table detected");
        }
        partition::PartitionTableType::Unknown => {
            log::info!("No partition table found");
        }
    }

    // Enumerate partitions
    for part in partition_table.all() {
        log::info!("Partition {}: {:?}, {} MB",
                   part.number,
                   part.partition_type,
                   (part.size_blocks * 512) / 1024 / 1024);

        if part.is_bootable() {
            log::info!("  -> Bootable");
        }
    }

    Ok(())
}

/// Run all quick start examples
pub fn run_all_quick_starts() {
    log::info!("=== Block Device Layer - Quick Start Examples ===\n");

    if let Err(e) = quick_start_ramdisk() {
        log::error!("Quick start ramdisk failed: {:?}", e);
    }

    if let Err(e) = quick_start_registry() {
        log::error!("Quick start registry failed: {:?}", e);
    }

    if let Err(e) = quick_start_scheduler() {
        log::error!("Quick start scheduler failed: {:?}", e);
    }

    quick_start_stats();

    if let Err(e) = quick_start_raid1() {
        log::error!("Quick start RAID 1 failed: {:?}", e);
    }

    if let Err(e) = quick_start_nvme() {
        log::error!("Quick start NVMe failed: {:?}", e);
    }

    if let Err(e) = quick_start_partitions() {
        log::error!("Quick start partitions failed: {:?}", e);
    }

    log::info!("\n=== All quick start examples completed ===");
}

/// Minimal working example
pub fn minimal_example() -> FsResult<()> {
    // Create device
    let device = device::RamDisk::new("test".into(), 1024 * 1024, 512);

    // Write
    device.write().write(0, &[0x42; 512])?;

    // Read
    let mut buf = [0u8; 512];
    device.read().read(0, &mut buf)?;

    // Verify
    assert_eq!(buf[0], 0x42);

    Ok(())
}
