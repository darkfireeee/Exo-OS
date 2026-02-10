//! FAT32 Directory Operations
//!
//! Directory entry parsing, Long Filename (LFN) support, and directory traversal.
//! Implements VFAT extensions for long filenames.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::sync::Arc;

use crate::fs::{FsError, FsResult};
use crate::fs::core::types::{InodeType, Timestamp};

use super::{Fat32Fs, Fat32Directory};

/// FAT32 directory entry (32 bytes)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct DirEntry {
    pub name: [u8; 11],
    pub attr: u8,
    pub nt_reserved: u8,
    pub create_time_tenth: u8,
    pub create_time: u16,
    pub create_date: u16,
    pub access_date: u16,
    pub first_cluster_hi: u16,
    pub modify_time: u16,
    pub modify_date: u16,
    pub first_cluster_lo: u16,
    pub file_size: u32,
}

impl DirEntry {
    /// Parse directory entry from bytes
    pub fn from_bytes(bytes: &[u8]) -> Self {
        assert!(bytes.len() >= 32);

        let mut name = [0u8; 11];
        name.copy_from_slice(&bytes[0..11]);

        Self {
            name,
            attr: bytes[11],
            nt_reserved: bytes[12],
            create_time_tenth: bytes[13],
            create_time: u16::from_le_bytes([bytes[14], bytes[15]]),
            create_date: u16::from_le_bytes([bytes[16], bytes[17]]),
            access_date: u16::from_le_bytes([bytes[18], bytes[19]]),
            first_cluster_hi: u16::from_le_bytes([bytes[20], bytes[21]]),
            modify_time: u16::from_le_bytes([bytes[22], bytes[23]]),
            modify_date: u16::from_le_bytes([bytes[24], bytes[25]]),
            first_cluster_lo: u16::from_le_bytes([bytes[26], bytes[27]]),
            file_size: u32::from_le_bytes([bytes[28], bytes[29], bytes[30], bytes[31]]),
        }
    }

    /// Convert to bytes
    pub fn to_bytes(&self) -> [u8; 32] {
        let mut bytes = [0u8; 32];

        bytes[0..11].copy_from_slice(&self.name);
        bytes[11] = self.attr;
        bytes[12] = self.nt_reserved;
        bytes[13] = self.create_time_tenth;
        bytes[14..16].copy_from_slice(&self.create_time.to_le_bytes());
        bytes[16..18].copy_from_slice(&self.create_date.to_le_bytes());
        bytes[18..20].copy_from_slice(&self.access_date.to_le_bytes());
        bytes[20..22].copy_from_slice(&self.first_cluster_hi.to_le_bytes());
        bytes[22..24].copy_from_slice(&self.modify_time.to_le_bytes());
        bytes[24..26].copy_from_slice(&self.modify_date.to_le_bytes());
        bytes[26..28].copy_from_slice(&self.first_cluster_lo.to_le_bytes());
        bytes[28..32].copy_from_slice(&self.file_size.to_le_bytes());

        bytes
    }

    /// Get first cluster number
    pub fn first_cluster(&self) -> u32 {
        ((self.first_cluster_hi as u32) << 16) | (self.first_cluster_lo as u32)
    }

    /// Set first cluster number
    pub fn set_first_cluster(&mut self, cluster: u32) {
        self.first_cluster_hi = (cluster >> 16) as u16;
        self.first_cluster_lo = (cluster & 0xFFFF) as u16;
    }

    /// Check if entry is free
    pub fn is_free(&self) -> bool {
        self.name[0] == 0xE5
    }

    /// Check if entry is end marker
    pub fn is_end(&self) -> bool {
        self.name[0] == 0x00
    }

    /// Check if entry is long filename
    pub fn is_lfn(&self) -> bool {
        self.attr == ATTR_LONG_NAME
    }

    /// Check if entry is directory
    pub fn is_directory(&self) -> bool {
        (self.attr & ATTR_DIRECTORY) != 0
    }

    /// Get short name as string
    pub fn short_name(&self) -> String {
        let name_part = core::str::from_utf8(&self.name[0..8])
            .unwrap_or("????????")
            .trim_end();

        let ext_part = core::str::from_utf8(&self.name[8..11])
            .unwrap_or("???")
            .trim_end();

        if ext_part.is_empty() {
            name_part.to_string()
        } else {
            alloc::format!("{}.{}", name_part, ext_part)
        }
    }

    /// Get file type
    pub fn file_type(&self) -> InodeType {
        if self.is_directory() {
            InodeType::Directory
        } else {
            InodeType::File
        }
    }
}

/// Long filename entry
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct LfnEntry {
    pub order: u8,
    pub name1: [u16; 5],
    pub attr: u8,
    pub entry_type: u8,
    pub checksum: u8,
    pub name2: [u16; 6],
    pub first_cluster: u16,
    pub name3: [u16; 2],
}

impl LfnEntry {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        assert!(bytes.len() >= 32);

        let mut name1 = [0u16; 5];
        for i in 0..5 {
            let offset = 1 + i * 2;
            name1[i] = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
        }

        let mut name2 = [0u16; 6];
        for i in 0..6 {
            let offset = 14 + i * 2;
            name2[i] = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
        }

        let mut name3 = [0u16; 2];
        for i in 0..2 {
            let offset = 28 + i * 2;
            name3[i] = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
        }

        Self {
            order: bytes[0],
            name1,
            attr: bytes[11],
            entry_type: bytes[12],
            checksum: bytes[13],
            name2,
            first_cluster: u16::from_le_bytes([bytes[26], bytes[27]]),
            name3,
        }
    }

    /// Extract characters from LFN entry
    pub fn chars(&self) -> Vec<char> {
        let mut chars = Vec::new();

        // Copy arrays from packed struct to avoid unaligned references
        // Use addr_of! to avoid creating intermediate references
        let name1 = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(self.name1)) };
        let name2 = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(self.name2)) };
        let name3 = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(self.name3)) };

        for &c in &name1 {
            if c == 0u16 || c == 0xFFFFu16 {
                break;
            }
            if let Some(ch) = char::from_u32(c as u32) {
                chars.push(ch);
            }
        }

        for &c in &name2 {
            if c == 0u16 || c == 0xFFFFu16 {
                break;
            }
            if let Some(ch) = char::from_u32(c as u32) {
                chars.push(ch);
            }
        }

        for &c in &name3 {
            if c == 0u16 || c == 0xFFFFu16 {
                break;
            }
            if let Some(ch) = char::from_u32(c as u32) {
                chars.push(ch);
            }
        }

        chars
    }

    /// Check if this is the last LFN entry
    pub fn is_last(&self) -> bool {
        (self.order & 0x40) != 0
    }

    /// Get sequence number
    pub fn sequence(&self) -> u8 {
        self.order & 0x1F
    }
}

/// Directory attributes
pub const ATTR_READ_ONLY: u8 = 0x01;
pub const ATTR_HIDDEN: u8 = 0x02;
pub const ATTR_SYSTEM: u8 = 0x04;
pub const ATTR_VOLUME_ID: u8 = 0x08;
pub const ATTR_DIRECTORY: u8 = 0x10;
pub const ATTR_ARCHIVE: u8 = 0x20;
pub const ATTR_LONG_NAME: u8 = ATTR_READ_ONLY | ATTR_HIDDEN | ATTR_SYSTEM | ATTR_VOLUME_ID;

/// Parsed directory entry with long filename support
#[derive(Debug, Clone)]
pub struct Fat32DirEntry {
    pub name: String,
    pub short_name: String,
    pub attr: u8,
    pub first_cluster: u32,
    pub file_size: u32,
    pub create_time: u16,
    pub create_date: u16,
    pub modify_time: u16,
    pub modify_date: u16,
}

impl Fat32DirEntry {
    pub fn is_directory(&self) -> bool {
        (self.attr & ATTR_DIRECTORY) != 0
    }

    pub fn file_type(&self) -> InodeType {
        if self.is_directory() {
            InodeType::Directory
        } else {
            InodeType::File
        }
    }
}

impl Fat32Fs {
    /// Read directory entries from cluster
    pub fn read_dir(&self, dir: &Fat32Directory) -> FsResult<Vec<Fat32DirEntry>> {
        let data = self.read_cluster_chain(dir.cluster(), None)?;
        parse_directory_entries(&data)
    }

    /// Find entry in directory
    pub fn find_entry(&self, dir: &Fat32Directory, name: &str) -> FsResult<Fat32DirEntry> {
        let entries = self.read_dir(dir)?;

        entries
            .into_iter()
            .find(|e| e.name.eq_ignore_ascii_case(name))
            .ok_or(FsError::NotFound)
    }

    /// Create directory entry
    pub fn create_entry(
        &self,
        dir: &Fat32Directory,
        name: &str,
        attr: u8,
        first_cluster: u32,
    ) -> FsResult<()> {
        // Read directory data
        let mut data = self.read_cluster_chain(dir.cluster(), None)?;

        // Find free slot
        let entry_size = 32;
        let mut offset = 0;

        while offset + entry_size <= data.len() {
            let entry_bytes = &data[offset..offset + entry_size];
            let entry = DirEntry::from_bytes(entry_bytes);

            if entry.is_free() || entry.is_end() {
                // Found free slot - create entry
                let mut new_entry = DirEntry {
                    name: [0x20; 11], // Space-padded
                    attr,
                    nt_reserved: 0,
                    create_time_tenth: 0,
                    create_time: 0,
                    create_date: 0,
                    access_date: 0,
                    first_cluster_hi: (first_cluster >> 16) as u16,
                    modify_time: 0,
                    modify_date: 0,
                    first_cluster_lo: (first_cluster & 0xFFFF) as u16,
                    file_size: 0,
                };

                // Set short name (simplified - no LFN creation yet)
                let short_name = to_short_name(name);
                new_entry.name.copy_from_slice(short_name.as_bytes());

                // Write entry
                let entry_bytes = new_entry.to_bytes();
                data[offset..offset + entry_size].copy_from_slice(&entry_bytes);

                // Write back directory
                self.write_cluster_chain(dir.cluster(), &data)?;

                return Ok(());
            }

            offset += entry_size;
        }

        // No free slot - need to extend directory
        // For now, return error
        Err(FsError::NoSpace)
    }

    /// Delete directory entry
    pub fn delete_entry(&self, dir: &Fat32Directory, name: &str) -> FsResult<()> {
        let mut data = self.read_cluster_chain(dir.cluster(), None)?;
        let entry_size = 32;
        let mut offset = 0;

        while offset + entry_size <= data.len() {
            let entry_bytes = &data[offset..offset + entry_size];
            let entry = DirEntry::from_bytes(entry_bytes);

            if entry.is_end() {
                break;
            }

            if !entry.is_free() && !entry.is_lfn() {
                let entry_name = entry.short_name();
                if entry_name.eq_ignore_ascii_case(name) {
                    // Mark as deleted
                    data[offset] = 0xE5;

                    // Write back
                    self.write_cluster_chain(dir.cluster(), &data)?;

                    return Ok(());
                }
            }

            offset += entry_size;
        }

        Err(FsError::NotFound)
    }

    /// Write cluster chain
    pub(super) fn write_cluster_chain(&self, start_cluster: u32, data: &[u8]) -> FsResult<()> {
        let cluster_size = self.cluster_size();
        let mut offset = 0;
        let mut cluster = start_cluster;

        while offset < data.len() {
            let chunk_size = (data.len() - offset).min(cluster_size);
            let mut cluster_data = alloc::vec![0u8; cluster_size];

            cluster_data[..chunk_size].copy_from_slice(&data[offset..offset + chunk_size]);

            self.write_cluster(cluster, &cluster_data)?;

            offset += chunk_size;

            if offset < data.len() {
                let next = self.read_fat_entry(cluster)?;
                if next >= 0x0FFFFFF8 {
                    return Err(FsError::NoSpace);
                }
                cluster = next;
            }
        }

        Ok(())
    }
}

/// Parse directory entries from raw data
fn parse_directory_entries(data: &[u8]) -> FsResult<Vec<Fat32DirEntry>> {
    let mut entries = Vec::new();
    let mut lfn_chars: Vec<char> = Vec::new();
    let entry_size = 32;
    let mut offset = 0;

    while offset + entry_size <= data.len() {
        let entry_bytes = &data[offset..offset + entry_size];
        let entry = DirEntry::from_bytes(entry_bytes);

        if entry.is_end() {
            break;
        }

        if !entry.is_free() {
            if entry.is_lfn() {
                // Long filename entry
                let lfn = LfnEntry::from_bytes(entry_bytes);
                let mut chars = lfn.chars();

                if lfn.is_last() {
                    lfn_chars = chars;
                } else {
                    chars.extend_from_slice(&lfn_chars);
                    lfn_chars = chars;
                }
            } else {
                // Regular entry
                let short_name = entry.short_name();

                let long_name = if lfn_chars.is_empty() {
                    short_name.clone()
                } else {
                    let name: String = lfn_chars.iter().rev().collect();
                    lfn_chars.clear();
                    name
                };

                // Skip volume ID and "." and ".." in root
                if (entry.attr & ATTR_VOLUME_ID) == 0 {
                    entries.push(Fat32DirEntry {
                        name: long_name,
                        short_name,
                        attr: entry.attr,
                        first_cluster: entry.first_cluster(),
                        file_size: entry.file_size,
                        create_time: entry.create_time,
                        create_date: entry.create_date,
                        modify_time: entry.modify_time,
                        modify_date: entry.modify_date,
                    });
                }
            }
        }

        offset += entry_size;
    }

    Ok(entries)
}

/// Convert long filename to 8.3 short name (simplified)
fn to_short_name(name: &str) -> String {
    let name = name.to_uppercase();
    let parts: Vec<&str> = name.split('.').collect();

    let (base, ext) = if parts.len() > 1 {
        (parts[0], parts[parts.len() - 1])
    } else {
        (name.as_str(), "")
    };

    let base = if base.len() > 8 {
        &base[..8]
    } else {
        base
    };

    let ext = if ext.len() > 3 {
        &ext[..3]
    } else {
        ext
    };

    let mut result = String::with_capacity(11);
    result.push_str(base);

    while result.len() < 8 {
        result.push(' ');
    }

    result.push_str(ext);

    while result.len() < 11 {
        result.push(' ');
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_short_name_conversion() {
        let name = to_short_name("README.TXT");
        assert_eq!(name.len(), 11);
        assert!(name.starts_with("README  "));
    }

    #[test]
    fn test_dir_entry_cluster() {
        let mut entry = DirEntry {
            name: [0; 11],
            attr: 0,
            nt_reserved: 0,
            create_time_tenth: 0,
            create_time: 0,
            create_date: 0,
            access_date: 0,
            first_cluster_hi: 0x1234,
            modify_time: 0,
            modify_date: 0,
            first_cluster_lo: 0x5678,
            file_size: 0,
        };

        assert_eq!(entry.first_cluster(), 0x12345678);

        entry.set_first_cluster(0xABCDEF00);
        assert_eq!(entry.first_cluster_hi, 0xABCD);
        assert_eq!(entry.first_cluster_lo, 0xEF00);
    }
}
