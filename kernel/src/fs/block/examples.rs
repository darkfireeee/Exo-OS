//! Block Module Examples
//!
//! This file demonstrates the usage of the block device layer.
//! These examples are for documentation purposes.

#![allow(dead_code)]

use super::*;
use alloc::sync::Arc;
use spin::RwLock;

/// Example 1: Create a simple RAM disk
pub fn example_ramdisk() -> crate::fs::FsResult<()> {
    // Create a 64MB RAM disk with 512-byte blocks
    let device = device::RamDisk::new("ramdisk0".into(), 64 * 1024 * 1024, 512);

    // Register the device
    BLOCK_DEVICE_REGISTRY.register(device.clone());

    // Read and write operations
    let mut write_buf = [42u8; 512];
    let mut read_buf = [0u8; 512];

    device.write().write(0, &write_buf)?;
    device.read().read(0, &mut read_buf)?;

    assert_eq!(write_buf, read_buf);

    Ok(())
}

/// Example 2: Partition detection
pub fn example_partition_detection() -> crate::fs::FsResult<()> {
    // Create a device
    let device = device::RamDisk::new("disk0".into(), 1024 * 1024 * 1024, 512);

    // Detect partitions (would normally be on a real disk)
    let partition_table = partition::PartitionTable::detect(&*device.read())?;

    // List all partitions
    for part in partition_table.all() {
        log::info!(
            "Partition {}: type={:?}, start={}, size={} blocks",
            part.number,
            part.partition_type,
            part.start_lba,
            part.size_blocks
        );
    }

    Ok(())
}

/// Example 3: I/O Scheduler
pub fn example_scheduler() -> crate::fs::FsResult<()> {
    // Create a device
    let device = device::RamDisk::new("disk1".into(), 128 * 1024 * 1024, 512);

    // Wrap with a deadline scheduler
    let scheduled = scheduler::ScheduledDevice::new(
        device.clone(),
        scheduler::SchedulerType::Deadline,
    );

    // Submit requests
    let request = scheduler::IoRequest::new(
        scheduler::IoOperation::Read,
        0,
        8,
    ).with_priority(0);

    scheduled.read().submit(request)?;

    // Process requests
    scheduled.write().process_next()?;

    Ok(())
}

/// Example 4: NVMe Optimization
pub fn example_nvme() -> crate::fs::FsResult<()> {
    // Create a device
    let device = device::RamDisk::new("nvme0".into(), 512 * 1024 * 1024, 512);

    // Wrap with NVMe optimizer
    let nvme = nvme::NvmeDevice::new(device, 1024, 4);

    // Get optimizer handle
    let optimizer = nvme.read().optimizer();

    // Set queue depth
    optimizer.read().set_queue_depth(512)?;

    // Get stats
    let stats = optimizer.read().stats();
    log::info!(
        "NVMe Queue Depth: {}, Commands in flight: {}",
        stats.queue_depth,
        stats.commands_in_flight
    );

    Ok(())
}

/// Example 5: I/O Statistics
pub fn example_stats() {
    // Create stats tracker
    let stats = stats::IoStats::new();

    // Simulate some I/O
    stats.record_read(4096, 50_000, 0, true);
    stats.record_write(4096, 80_000, 8, true);

    // Get statistics
    let snapshot = stats.snapshot();
    log::info!(
        "Read throughput: {} MB/s, Write throughput: {} MB/s",
        snapshot.read_throughput_bps / 1_000_000,
        snapshot.write_throughput_bps / 1_000_000
    );

    log::info!(
        "Avg read latency: {} us, Avg write latency: {} us",
        snapshot.avg_read_latency_ns / 1000,
        snapshot.avg_write_latency_ns / 1000
    );
}

/// Example 6: RAID Array
pub fn example_raid() -> crate::fs::FsResult<()> {
    // Create multiple devices
    let dev1 = device::RamDisk::new("disk0".into(), 256 * 1024 * 1024, 512);
    let dev2 = device::RamDisk::new("disk1".into(), 256 * 1024 * 1024, 512);
    let dev3 = device::RamDisk::new("disk2".into(), 256 * 1024 * 1024, 512);

    // Create RAID 5 configuration
    let config = raid::RaidConfig::new(
        raid::RaidLevel::Raid5,
        64 * 1024, // 64KB chunk size
        "raid5_array".into(),
    );

    // Build RAID array
    let devices: alloc::vec::Vec<Arc<RwLock<dyn BlockDevice>>> = alloc::vec![
        dev1 as Arc<RwLock<dyn BlockDevice>>,
        dev2 as Arc<RwLock<dyn BlockDevice>>,
        dev3 as Arc<RwLock<dyn BlockDevice>>,
    ];

    let raid_array = raid::RaidArray::new(config, devices)?;

    // Use RAID array
    let info = raid_array.read().info();
    log::info!(
        "RAID array created: size={} MB, block_size={}",
        info.size / 1024 / 1024,
        info.block_size
    );

    Ok(())
}

/// Example 7: Complete workflow
pub fn example_complete_workflow() -> crate::fs::FsResult<()> {
    // 1. Create a device
    let device = create_test_ramdisk("production_disk", 1024);

    // 2. Detect and enumerate partitions
    let partition_table = partition::PartitionTable::detect(&*device.read())?;

    if partition_table.count() > 0 {
        // 3. Create partition wrapper for first partition
        let part = partition_table.all()[0].clone();
        let part_device = partition::PartitionedDevice::new(device.clone(), part);

        // 4. Wrap with I/O scheduler
        let scheduled = scheduler::ScheduledDevice::new(
            part_device as Arc<RwLock<dyn BlockDevice>>,
            scheduler::SchedulerType::CFQ,
        );

        // 5. Add statistics tracking
        let stats = stats::IoStats::new();

        // 6. Perform I/O operations
        let mut buffer = [0u8; 4096];
        scheduled.read().device.read().read(0, &mut buffer)?;

        stats.record_read(4096, 100_000, 0, true);

        // 7. Check statistics
        let snapshot = stats.snapshot();
        log::info!("IOPS: {}", snapshot.read_iops);
    }

    Ok(())
}

/// Run all examples
pub fn run_all_examples() {
    log::info!("=== Block Device Layer Examples ===");

    if let Err(e) = example_ramdisk() {
        log::error!("Example ramdisk failed: {:?}", e);
    }

    if let Err(e) = example_partition_detection() {
        log::error!("Example partition detection failed: {:?}", e);
    }

    if let Err(e) = example_scheduler() {
        log::error!("Example scheduler failed: {:?}", e);
    }

    if let Err(e) = example_nvme() {
        log::error!("Example NVMe failed: {:?}", e);
    }

    example_stats();

    if let Err(e) = example_raid() {
        log::error!("Example RAID failed: {:?}", e);
    }

    if let Err(e) = example_complete_workflow() {
        log::error!("Example complete workflow failed: {:?}", e);
    }

    log::info!("=== Examples completed ===");
}
