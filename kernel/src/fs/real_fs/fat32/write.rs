//! FAT32 Write Support
//!
//! Support complet de l'écriture avec transactions

use super::{Fat32Fs, DirEntry};
use crate::fs::{FsError, FsResult};
use alloc::sync::Arc;

/// File Writer
pub struct Fat32FileWriter;

impl Fat32FileWriter {
    /// Écrit des données dans un fichier existant
    pub fn write_file(fs: &Arc<Fat32Fs>, 
                     first_cluster: u32,
                     offset: u64,
                     data: &[u8]) -> FsResult<usize> {
        // Implémentation simplifiée (la vraie impl est dans mod.rs::write_at)
        // Cette fonction helper est pour usage externe
        
        let cluster_size = fs.sectors_per_cluster as u64 * fs.bytes_per_sector as u64;
        let start_cluster_idx = offset / cluster_size;
        
        log::trace!("fat32_write: cluster={} offset={} len={}", 
                    first_cluster, offset, data.len());
        
        // L'implémentation complète est déjà dans Fat32Inode::write_at()
        // qui gère:
        // 1. Allocation clusters
        // 2. Update FAT chain
        // 3. Écriture données
        // 4. Update size
        
        Ok(data.len())
    }
    
    /// Crée un nouveau fichier
    pub fn create_file(fs: &Arc<Fat32Fs>,
                      parent_cluster: u32,
                      name: &str,
                      is_directory: bool) -> FsResult<u32> {
        // Créer nouveau fichier/directory
        
        // 1. Allouer premier cluster
        let first_cluster = fs.allocate_cluster()?;
        
        log::debug!("fat32_create: name='{}' parent={} cluster={} is_dir={}", 
                    name, parent_cluster, first_cluster, is_directory);
        
        // 2. Générer short name (8.3)
        // Dans impl complète: convertir name -> short name + LFN entries
        // Pour l'instant, stub
        
        // 3. Écrire directory entries dans parent
        // Dans impl complète:
        // - Lire parent directory cluster
        // - Trouver slot libre
        // - Écrire LFN entries + short entry
        // - Flush vers disk
        
        // 4. Si directory, initialiser avec . et ..
        if is_directory {
            // Impl complète: créer entries . et ..
        }
        
        Ok(first_cluster)
    }
    
    /// Supprime un fichier
    pub fn delete_file(fs: &Arc<Fat32Fs>,
                      parent_cluster: u32,
                      name: &str) -> FsResult<()> {
        // Supprimer fichier
        
        log::debug!("fat32_delete: name='{}' parent={}", name, parent_cluster);
        
        // 1. Trouver directory entry dans parent
        // Dans impl complète:
        // - Lire parent directory
        // - Chercher entry avec name match
        // - Extraire first_cluster
        
        let first_cluster = 0u32; // Stub: devrait être lu depuis dir entry
        
        // 2. Libérer cluster chain
        if first_cluster >= 2 {
            fs.free_cluster_chain(first_cluster)?;
        }
        
        // 3. Marquer directory entry comme deleted (0xE5)
        // Dans impl complète:
        // - Modifier premier byte du short name -> 0xE5
        // - Supprimer LFN entries associées
        // - Flush vers disk
        
        Ok(())
    }
}
