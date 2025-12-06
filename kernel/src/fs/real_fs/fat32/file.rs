//! FAT32 File Operations

use super::Fat32Fs;
use crate::fs::{FsError, FsResult};
use alloc::sync::Arc;
use alloc::vec::Vec;

/// File Reader
pub struct Fat32FileReader;

impl Fat32FileReader {
    /// Lit un fichier complet
    pub fn read_file(fs: &Arc<Fat32Fs>, first_cluster: u32, size: u32) -> FsResult<Vec<u8>> {
        if first_cluster == 0 {
            // Empty file
            return Ok(Vec::new());
        }
        
        let data = fs.read_cluster_chain(first_cluster)?;
        
        // Tronque à la taille réelle
        let size = size as usize;
        if size <= data.len() {
            Ok(data[..size].to_vec())
        } else {
            Ok(data)
        }
    }
    
    /// Lit une partie d'un fichier (offset + length)
    pub fn read_partial(fs: &Arc<Fat32Fs>, 
                       first_cluster: u32, 
                       offset: u64, 
                       length: usize) -> FsResult<Vec<u8>> {
        if first_cluster == 0 {
            return Ok(Vec::new());
        }
        
        let cluster_size = fs.sectors_per_cluster as usize * fs.bytes_per_sector as usize;
        let start_cluster_idx = offset as usize / cluster_size;
        let end_cluster_idx = (offset as usize + length + cluster_size - 1) / cluster_size;
        
        // TODO: Optimize pour ne lire que les clusters nécessaires
        let data = fs.read_cluster_chain(first_cluster)?;
        
        let start = offset as usize;
        let end = (start + length).min(data.len());
        
        if start >= data.len() {
            return Ok(Vec::new());
        }
        
        Ok(data[start..end].to_vec())
    }
}
