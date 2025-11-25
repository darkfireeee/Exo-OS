//! TmpFS - Temporary Filesystem in RAM
//! 
//! Fast volatile storage.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use crate::memory::MemoryResult;

type InodeType = u32;

/// TmpFS instance
pub struct TmpFs {
    inodes: BTreeMap<u64, TmpInode>,
    next_ino: u64,
}

/// TmpFS inode
struct TmpInode {
    ino: u64,
    typ: InodeType,
    name: String,
    data: Vec<u8>,
    size: usize,
}

impl TmpFs {
    pub fn new() -> Self {
        Self {
            inodes: BTreeMap::new(),
            next_ino: 1,
        }
    }
    
    pub fn create_file(&mut self, name: &str) -> MemoryResult<u64> {
        let ino = self.next_ino;
        self.next_ino += 1;
        
        self.inodes.insert(ino, TmpInode {
            ino,
            typ: 1,  // File type
            name: String::from(name),
            data: Vec::new(),
            size: 0,
        });
        
        Ok(ino)
    }
    
    pub fn write(&mut self, ino: u64, data: &[u8]) -> MemoryResult<usize> {
        if let Some(inode) = self.inodes.get_mut(&ino) {
            inode.data = data.to_vec();
            inode.size = data.len();
            Ok(data.len())
        } else {
            Err(crate::memory::MemoryError::NotFound)
        }
    }
    
    pub fn read(&self, ino: u64) -> MemoryResult<&[u8]> {
        if let Some(inode) = self.inodes.get(&ino) {
            Ok(&inode.data)
        } else {
            Err(crate::memory::MemoryError::NotFound)
        }
    }
}
