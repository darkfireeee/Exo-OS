//! FAT32 FAT Table Cache
//!
//! Cache la FAT entière en RAM pour performance maximale

use crate::drivers::block::BlockDevice;
use crate::fs::{FsError, FsResult};
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

/// Entrée FAT
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FatEntry {
    Free,
    Bad,
    EndOfChain,
    Next(u32),
}

/// Cache de la FAT (table entière en RAM)
///
/// ## Performance
/// - Lookup: **O(1)** direct array access
/// - Modification: **O(1)** + dirty tracking
/// - Flush: Batch write de toutes les dirty entries
pub struct FatCache {
    /// FAT entries (index = cluster number)
    entries: Vec<u32>,
    
    /// Dirty bitmap (pour flush optimisé)
    dirty: Vec<bool>,
    
    /// FAT start sector
    fat_start: u64,
    
    /// FAT size en sectors
    fat_size: u64,
    
    /// Bytes per sector
    bytes_per_sector: u16,
}

impl FatCache {
    /// Charge la FAT complète en RAM
    pub fn load(device: &Arc<Mutex<dyn BlockDevice>>, 
                fat_start: u64, 
                fat_size: u64, 
                bytes_per_sector: u16) -> FsResult<Self> {
        let entries_per_sector = bytes_per_sector as usize / 4;
        let total_entries = (fat_size as usize * entries_per_sector) as usize;
        
        let mut entries = Vec::with_capacity(total_entries);
        let mut buffer = alloc::vec![0u8; bytes_per_sector as usize];
        
        let device_lock = device.lock();
        
        for sector_offset in 0..fat_size {
            device_lock.read(fat_start + sector_offset, &mut buffer)
                .map_err(|_| FsError::IoError)?;
            
            for chunk in buffer.chunks_exact(4) {
                let entry = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) & 0x0FFFFFFF;
                entries.push(entry);
            }
        }
        
        let dirty = alloc::vec![false; total_entries];
        
        log::debug!("FAT32: loaded {} FAT entries ({} KB)", 
                   total_entries,
                   total_entries * 4 / 1024);
        
        Ok(Self {
            entries,
            dirty,
            fat_start,
            fat_size,
            bytes_per_sector,
        })
    }
    
    /// Lit une entrée FAT
    #[inline(always)]
    pub fn get_entry(&self, cluster: u32) -> FsResult<FatEntry> {
        if cluster as usize >= self.entries.len() {
            return Err(FsError::InvalidArgument);
        }
        
        let value = self.entries[cluster as usize];
        
        Ok(match value {
            0 => FatEntry::Free,
            0x0FFFFFF7 => FatEntry::Bad,
            0x0FFFFFF8..=0x0FFFFFFF => FatEntry::EndOfChain,
            n => FatEntry::Next(n),
        })
    }
    
    /// Modifie une entrée FAT
    #[inline(always)]
    pub fn set_entry(&mut self, cluster: u32, entry: FatEntry) -> FsResult<()> {
        if cluster as usize >= self.entries.len() {
            return Err(FsError::InvalidArgument);
        }
        
        let value = match entry {
            FatEntry::Free => 0,
            FatEntry::Bad => 0x0FFFFFF7,
            FatEntry::EndOfChain => 0x0FFFFFFF,
            FatEntry::Next(n) => n & 0x0FFFFFFF,
        };
        
        self.entries[cluster as usize] = value;
        self.dirty[cluster as usize] = true;
        
        Ok(())
    }
    
    /// Flush les dirty entries vers le disque
    pub fn flush(&self, device: &Arc<Mutex<dyn BlockDevice>>) -> FsResult<()> {
        let entries_per_sector = self.bytes_per_sector as usize / 4;
        let mut buffer = alloc::vec![0u8; self.bytes_per_sector as usize];
        
        let device_lock = device.lock();
        
        for sector_offset in 0..self.fat_size {
            let sector_start = sector_offset as usize * entries_per_sector;
            let sector_end = sector_start + entries_per_sector;
            
            // Check si ce secteur a des dirty entries
            let has_dirty = self.dirty[sector_start..sector_end].iter().any(|&d| d);
            
            if !has_dirty {
                continue;
            }
            
            // Build buffer
            for (i, entry) in self.entries[sector_start..sector_end].iter().enumerate() {
                let bytes = entry.to_le_bytes();
                let offset = i * 4;
                buffer[offset..offset + 4].copy_from_slice(&bytes);
            }
            
            // Write sector
            device_lock.write(self.fat_start + sector_offset, &buffer)
                .map_err(|_| FsError::IoError)?;
        }
        
        Ok(())
    }
    
    /// Trouve le prochain cluster libre
    pub fn find_free(&self, start: u32) -> Option<u32> {
        for cluster in start as usize..self.entries.len() {
            if self.entries[cluster] == 0 {
                return Some(cluster as u32);
            }
        }
        
        // Wrap around
        for cluster in 2..start as usize {
            if self.entries[cluster] == 0 {
                return Some(cluster as u32);
            }
        }
        
        None
    }
    
    /// Compte les clusters libres
    pub fn count_free(&self) -> u32 {
        self.entries.iter().filter(|&&e| e == 0).count() as u32
    }
}
