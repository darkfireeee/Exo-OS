use alloc::vec::Vec;
use uefi::table::boot::*;

pub struct Ext2Driver {
    volume: FileHandle,
}

impl Ext2Driver {
    pub fn new(volume: FileHandle) -> Self {
        Self { volume }
    }

    pub fn read_inode(&self, inode: u32) -> Vec<u8> {
        // Implementation de la lecture d'inode Ext2
        vec![]
    }

    pub fn read_directory(&self, dir_inode: u32) -> Vec<DirectoryEntry> {
        // Implementation de la lecture de répertoire Ext2
        vec![]
    }
}
