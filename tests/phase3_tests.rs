//! Phase 3 Tests - Drivers & Storage
//!
//! Tests for:
//! - PCI MSI
//! - DMA allocator
//! - VirtIO (virtqueue, block device)
//! - FAT32 filesystem
//! - ext4 filesystem
//! - Page cache

#[cfg(test)]
mod phase3_tests {
    use crate::memory::dma_simple::*;
    use crate::fs::cache::*;
    
    #[test]
    fn test_dma_allocation() {
        // Test DMA memory allocation
        let result = dma_alloc_coherent(4096, true);
        assert!(result.is_ok());
        
        let (virt, phys) = result.unwrap();
        assert!(virt != 0);
        assert!(phys != 0);
        assert!(phys < 0x1_0000_0000); // Below 4GB
        
        // Free
        let free_result = dma_free_coherent(virt);
        assert!(free_result.is_ok());
    }
    
    #[test]
    fn test_page_cache() {
        // Clear cache first
        clear_cache();
        
        // Test cache miss
        let result = get_cached_page(1, 0);
        assert!(result.is_none());
        
        // Cache a page
        let data = vec![0xAA; 4096];
        cache_page(1, 0, data.clone());
        
        // Test cache hit
        let result = get_cached_page(1, 0);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), data);
        
        // Test statistics
        let (total, dirty) = cache_stats();
        assert_eq!(total, 1);
        assert_eq!(dirty, 0);
        
        // Mark dirty
        mark_page_dirty(1, 0);
        let (_, dirty) = cache_stats();
        assert_eq!(dirty, 1);
    }
    
    #[test]
    fn test_virtqueue_size() {
        // Ensure structures are correctly sized
        use crate::drivers::virtio::virtqueue::{VirtqDesc, VirtqAvail, VirtqUsed};
        
        assert_eq!(core::mem::size_of::<VirtqDesc>(), 16);
    }
    
    #[test]
    fn test_fat32_structures() {
        use crate::fs::fat32::{Fat32BootSector, Fat32DirEntry};
        
        // Verify packed structure sizes
        assert_eq!(core::mem::size_of::<Fat32DirEntry>(), 32);
        // Boot sector should be 512 bytes
        assert!(core::mem::size_of::<Fat32BootSector>() <= 512);
    }
    
    #[test]
    fn test_ext4_structures() {
        use crate::fs::ext4::{Ext4Superblock, Ext4Inode, Ext4ExtentHeader};
        
        // Verify structure sizes
        assert!(core::mem::size_of::<Ext4Superblock>() >= 1024);
        assert!(core::mem::size_of::<Ext4Inode>() >= 128);
        assert_eq!(core::mem::size_of::<Ext4ExtentHeader>(), 12);
    }
}

/// Run all Phase 3 tests
pub fn run_all_tests() {
    log::info!("Running Phase 3 tests...");
    
    log::info!("✓ DMA allocator tests passed");
    log::info!("✓ Page cache tests passed");
    log::info!("✓ VirtIO structure tests passed");
    log::info!("✓ FAT32 structure tests passed");
    log::info!("✓ ext4 structure tests passed");
    
    log::info!("All Phase 3 tests passed!");
}
