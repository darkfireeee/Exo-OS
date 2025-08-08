use alloc::vec::Vec;
use uefi::table::boot::*;
use uefi::proto::media::file::*;

pub struct FatDriver {
    volume: FileHandle,
}

impl FatDriver {
    pub fn new(volume: FileHandle) -> Self {
        Self { volume }
    }

    pub fn read_dir(&self, path: &str) -> Vec<DirectoryEntry> {
        // Implementation de la lecture du répertoire FAT
        vec![]
    }

    pub fn read_file(&self, path: &str) -> Vec<u8> {
        // Implementation de la lecture de fichier FAT
        vec![]
    }
}
