//! ext4 Inode

use super::{Ext4Fs, ExtentHeader};
use crate::fs::core::{Inode as VfsInode, InodeType, InodePermissions, Timestamp};
use crate::fs::{FsError, FsResult};
use alloc::sync::Arc;
use alloc::string::String;
use alloc::vec::Vec;

/// ext4 Inode (raw on-disk structure)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Ext4InodeRaw {
    pub mode: u16,
    pub uid: u16,
    pub size_lo: u32,
    pub atime: u32,
    pub ctime: u32,
    pub mtime: u32,
    pub dtime: u32,
    pub gid: u16,
    pub links_count: u16,
    pub blocks_lo: u32,
    pub flags: u32,
    pub osd1: u32,
    pub block: [u32; 15], // 12 direct + 3 indirect OR extent tree
    pub generation: u32,
    pub file_acl_lo: u32,
    pub size_high: u32,
    pub obso_faddr: u32,
    // Extra fields
    pub blocks_high: u16,
    pub file_acl_high: u16,
    pub uid_high: u16,
    pub gid_high: u16,
    pub checksum_lo: u16,
    pub reserved: u16,
}

/// ext4 Inode flags
pub const EXT4_EXTENTS_FL: u32 = 0x00080000;
pub const EXT4_INLINE_DATA_FL: u32 = 0x10000000;

/// ext4 Inode (in-memory structure)
pub struct Ext4Inode {
    /// Inode number
    ino: u32,
    
    /// Raw inode
    raw: Ext4InodeRaw,
    
    /// Filesystem reference
    fs: Arc<Ext4Fs>,
}

impl Ext4Inode {
    pub fn from_raw(ino: u32, raw: Ext4InodeRaw, fs: Arc<Ext4Fs>) -> Self {
        Self { ino, raw, fs }
    }
    
    /// File size (64-bit)
    pub fn size(&self) -> u64 {
        ((self.raw.size_high as u64) << 32) | (self.raw.size_lo as u64)
    }
    
    /// Use extents?
    pub fn is_extents(&self) -> bool {
        (self.raw.flags & EXT4_EXTENTS_FL) != 0
    }
    
    /// Inline data?
    pub fn is_inline_data(&self) -> bool {
        (self.raw.flags & EXT4_INLINE_DATA_FL) != 0
    }
    
    /// Récupère le type d'inode
    fn inode_type(&self) -> InodeType {
        let fmt = self.raw.mode & 0xF000;
        match fmt {
            0x8000 => InodeType::File,
            0x4000 => InodeType::Directory,
            0xA000 => InodeType::Symlink,
            0x2000 => InodeType::CharDevice,
            0x6000 => InodeType::BlockDevice,
            0x1000 => InodeType::Fifo,
            0xC000 => InodeType::Socket,
            _ => InodeType::File,
        }
    }
    
    /// Lit le fichier via extent tree
    pub fn read_via_extents(&self, offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        // Parse extent header depuis block[0]
        let extent_header = unsafe {
            core::ptr::read_unaligned(self.raw.block.as_ptr() as *const ExtentHeader)
        };
        
        if extent_header.magic != 0xF30A {
            return Err(FsError::InvalidData);
        }
        
        // Parcourir l'extent tree et lire les blocs
        log::trace!("ext4: reading via extent tree for inode {} at offset {}", self.ino, offset);
        
        // Calculer le bloc logique
        let block_size = 4096; // Taille de bloc ext4 standard
        let logical_block = (offset / block_size) as u32;
        
        // Extraire les données de l'extent tree depuis i_block
        let extent_data = unsafe {
            core::slice::from_raw_parts(
                self.raw.block.as_ptr() as *const u8,
                self.raw.block.len() * 4
            )
        };
        
        // Trouver le bloc physique correspondant
        use super::extent::ExtentTree;
        
        if let Some(physical_block) = ExtentTree::logical_to_physical(
            &extent_header,
            extent_data,
            logical_block
        ) {
            log::trace!("ext4: logical block {} maps to physical block {}", 
                       logical_block, physical_block);
            
            // Simulation: retourner des données nulles
            // Dans un vrai système: lire le bloc depuis BlockDevice
            let to_read = buf.len().min(self.size() as usize - offset as usize);
            buf[..to_read].fill(0);
            
            Ok(to_read)
        } else {
            log::warn!("ext4: failed to map logical block {}", logical_block);
            Err(FsError::NotSupported)
        }
    }
}

impl VfsInode for Ext4Inode {
    fn ino(&self) -> u64 {
        self.ino as u64
    }
    
    fn inode_type(&self) -> InodeType {
        self.inode_type()
    }
    
    fn size(&self) -> u64 {
        self.size()
    }
    
    fn permissions(&self) -> InodePermissions {
        InodePermissions::new(self.raw.mode & 0o7777)
    }
    
    fn uid(&self) -> u32 {
        ((self.raw.uid_high as u32) << 16) | (self.raw.uid as u32)
    }
    
    fn gid(&self) -> u32 {
        ((self.raw.gid_high as u32) << 16) | (self.raw.gid as u32)
    }
    
    fn nlink(&self) -> u32 {
        self.raw.links_count as u32
    }
    
    fn atime(&self) -> Timestamp {
        Timestamp { sec: self.raw.atime as i64, nsec: 0 }
    }
    
    fn mtime(&self) -> Timestamp {
        Timestamp { sec: self.raw.mtime as i64, nsec: 0 }
    }
    
    fn ctime(&self) -> Timestamp {
        Timestamp { sec: self.raw.ctime as i64, nsec: 0 }
    }
    
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> FsResult<usize> {
        if self.is_extents() {
            self.read_via_extents(offset, buf)
        } else {
            // Lecture via indirect blocks (ancien format ext2/ext3)
            log::debug!("ext4: reading inode {} via indirect blocks", self.ino);
            
            let block_size = 4096;
            let logical_block = (offset / block_size) as u32;
            let block_offset = (offset % block_size) as usize;
            
            // Analyser les pointeurs dans i_block
            // i_block[0..11] = direct blocks
            // i_block[12] = single indirect
            // i_block[13] = double indirect  
            // i_block[14] = triple indirect
            
            let physical_block = if logical_block < 12 {
                // Direct block
                self.raw.block[logical_block as usize] as u64
            } else if logical_block < 12 + 256 {
                // Single indirect
                let indirect_block = self.raw.block[12] as u64;
                log::trace!("ext4: single indirect via block {}", indirect_block);
                // Dans un vrai système: lire indirect_block et extraire le pointeur
                indirect_block + (logical_block - 12) as u64
            } else if logical_block < 12 + 256 + 256 * 256 {
                // Double indirect
                let double_indirect = self.raw.block[13] as u64;
                log::trace!("ext4: double indirect via block {}", double_indirect);
                double_indirect + (logical_block - 12 - 256) as u64
            } else {
                // Triple indirect
                log::trace!("ext4: triple indirect access");
                self.raw.block[14] as u64 + logical_block as u64
            };
            
            log::trace!("ext4: logical block {} maps to physical block {}", 
                       logical_block, physical_block);
            
            // Simulation: retourner des données nulles
            // Dans un vrai système: device.read(physical_block * block_size + block_offset, buf)
            let to_read = buf.len().min((self.size() as usize).saturating_sub(offset as usize));
            buf[..to_read].fill(0);
            
            Ok(to_read)
        }
    }
    
    fn write_at(&mut self, _offset: u64, _buf: &[u8]) -> FsResult<usize> {
        Err(FsError::NotSupported)
    }
    
    fn truncate(&mut self, _size: u64) -> FsResult<()> {
        Err(FsError::NotSupported)
    }
    
    fn list(&self) -> FsResult<Vec<String>> {
        // Directory listing pour ext4
        if self.inode_type != InodeType::Directory {
            return Err(FsError::NotDirectory);
        }
        
        // Dans impl complète:
        // 1. Lire directory entries via read_at()
        // 2. Parser ext4_dir_entry_2 structures
        // 3. Extraire noms (avec support UTF-8)
        // 4. Retourner liste
        
        // Simulation basique
        let mut entries = Vec::new();
        
        // Ajouter entries standard
        entries.push(".".to_string());
        entries.push("..".to_string());
        
        log::trace!("ext4: list directory inode={} -> {} entries (stub)", 
                    self.ino, entries.len());
        
        Ok(entries)
    }
    
    fn lookup(&self, _name: &str) -> FsResult<u64> {
        Err(FsError::NotSupported)
    }
    
    fn create(&mut self, _name: &str, _inode_type: InodeType) -> FsResult<u64> {
        Err(FsError::NotSupported)
    }
    
    fn remove(&mut self, _name: &str) -> FsResult<()> {
        Err(FsError::NotSupported)
    }
}
