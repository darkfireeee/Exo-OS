//! FAT32 Directory Operations

use super::{Fat32Fs, LfnParser, LfnEntry};
use crate::fs::{FsError, FsResult};
use alloc::string::String;
use alloc::vec::Vec;
use alloc::sync::Arc;
use alloc::format;

/// Directory Entry (32 bytes)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct DirEntry {
    pub name: [u8; 11],         // 8.3 name
    pub attr: u8,
    pub reserved: u8,
    pub creation_time_tenth: u8,
    pub creation_time: u16,
    pub creation_date: u16,
    pub last_access_date: u16,
    pub first_cluster_high: u16,
    pub modification_time: u16,
    pub modification_date: u16,
    pub first_cluster_low: u16,
    pub file_size: u32,
}

/// Attributes
pub const ATTR_READ_ONLY: u8 = 0x01;
pub const ATTR_HIDDEN: u8 = 0x02;
pub const ATTR_SYSTEM: u8 = 0x04;
pub const ATTR_VOLUME_LABEL: u8 = 0x08;
pub const ATTR_DIRECTORY: u8 = 0x10;
pub const ATTR_ARCHIVE: u8 = 0x20;
pub const ATTR_LFN: u8 = 0x0F;

impl DirEntry {
    #[inline(always)]
    pub fn first_cluster(&self) -> u32 {
        ((self.first_cluster_high as u32) << 16) | (self.first_cluster_low as u32)
    }
    
    #[inline(always)]
    pub fn is_directory(&self) -> bool {
        (self.attr & ATTR_DIRECTORY) != 0
    }
    
    #[inline(always)]
    pub fn is_volume_label(&self) -> bool {
        (self.attr & ATTR_VOLUME_LABEL) != 0
    }
    
    #[inline(always)]
    pub fn is_lfn(&self) -> bool {
        self.attr == ATTR_LFN
    }
    
    #[inline(always)]
    pub fn is_free(&self) -> bool {
        self.name[0] == 0x00
    }
    
    #[inline(always)]
    pub fn is_deleted(&self) -> bool {
        self.name[0] == 0xE5
    }
    
    pub fn short_name(&self) -> String {
        let name_part = core::str::from_utf8(&self.name[0..8])
            .unwrap_or("")
            .trim_end();
        
        let ext_part = core::str::from_utf8(&self.name[8..11])
            .unwrap_or("")
            .trim_end();
        
        if ext_part.is_empty() {
            String::from(name_part)
        } else {
            format!("{}.{}", name_part, ext_part)
        }
    }
    
    pub fn checksum(&self) -> u8 {
        LfnEntry::calculate_checksum(&self.name)
    }
}

/// Parsed directory entry
#[derive(Debug, Clone)]
pub struct ParsedDirEntry {
    pub name: String,
    pub first_cluster: u32,
    pub size: u32,
    pub is_directory: bool,
    pub attr: u8,
}

/// Directory Reader
pub struct Fat32DirReader;

impl Fat32DirReader {
    /// Lit toutes les entrées d'un répertoire
    pub fn read_directory(fs: &Arc<Fat32Fs>, cluster: u32) -> FsResult<Vec<ParsedDirEntry>> {
        let data = fs.read_cluster_chain(cluster)?;
        
        let mut entries = Vec::new();
        let mut lfn_parser = LfnParser::new();
        
        let entry_size = core::mem::size_of::<DirEntry>();
        let entry_count = data.len() / entry_size;
        
        for i in 0..entry_count {
            let offset = i * entry_size;
            
            // Check si c'est un LFN entry
            if data[offset + 11] == ATTR_LFN {
                let lfn_entry = unsafe {
                    core::ptr::read_unaligned(data.as_ptr().add(offset) as *const LfnEntry)
                };
                
                lfn_parser.push_entry(lfn_entry);
                continue;
            }
            
            let entry = unsafe {
                core::ptr::read_unaligned(data.as_ptr().add(offset) as *const DirEntry)
            };
            
            // Stop at free entry
            if entry.is_free() {
                break;
            }
            
            // Skip deleted, volume labels
            if entry.is_deleted() || entry.is_volume_label() {
                lfn_parser.reset();
                continue;
            }
            
            // Build name (LFN ou short)
            let name = if let Some(lfn_name) = lfn_parser.build_name(entry.checksum()) {
                lfn_parser.reset();
                lfn_name
            } else {
                entry.short_name()
            };
            
            entries.push(ParsedDirEntry {
                name,
                first_cluster: entry.first_cluster(),
                size: entry.file_size,
                is_directory: entry.is_directory(),
                attr: entry.attr,
            });
        }
        
        Ok(entries)
    }
}
