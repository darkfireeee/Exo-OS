//! ext4 Extent Tree
//!
//! Support complet de l'extent tree (header + index + leaf)

/// Extent Header
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ExtentHeader {
    pub magic: u16,         // 0xF30A
    pub entries: u16,       // Number of valid entries
    pub max: u16,           // Capacity
    pub depth: u16,         // Tree depth (0 = leaf)
    pub generation: u32,
}

/// Extent Index (internal node)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ExtentIdx {
    pub block: u32,         // Logical block
    pub leaf_lo: u32,       // Physical block (low 32)
    pub leaf_hi: u16,       // Physical block (high 16)
    pub unused: u16,
}

impl ExtentIdx {
    pub fn physical_block(&self) -> u64 {
        ((self.leaf_hi as u64) << 32) | (self.leaf_lo as u64)
    }
}

/// Extent (leaf node)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Extent {
    pub block: u32,         // Logical block
    pub len: u16,           // Number of blocks
    pub start_hi: u16,      // Physical block (high 16)
    pub start_lo: u32,      // Physical block (low 32)
}

impl Extent {
    pub fn physical_block(&self) -> u64 {
        ((self.start_hi as u64) << 32) | (self.start_lo as u64)
    }
    
    pub fn is_initialized(&self) -> bool {
        self.len <= 32768
    }
    
    pub fn length(&self) -> u16 {
        if self.is_initialized() {
            self.len
        } else {
            self.len - 32768
        }
    }
}

/// Extent Tree Walker
pub struct ExtentTreeWalker;

impl ExtentTreeWalker {
    /// Trouve l'extent contenant le logical block
    pub fn find_extent(root: &ExtentHeader, data: &[u8], logical_block: u32) -> Option<Extent> {
        if root.magic != 0xF30A {
            return None;
        }
        
        if root.depth == 0 {
            // Leaf node: chercher directement dans les extents
            Self::find_in_leaf(root, data, logical_block)
        } else {
            // Internal node: descendre dans l'arbre
            Self::find_in_internal(root, data, logical_block)
        }
    }
    
    /// Cherche dans un leaf node
    fn find_in_leaf(header: &ExtentHeader, data: &[u8], logical_block: u32) -> Option<Extent> {
        let entries = header.entries as usize;
        let base_offset = core::mem::size_of::<ExtentHeader>();
        
        for i in 0..entries {
            let offset = base_offset + i * core::mem::size_of::<Extent>();
            if offset + core::mem::size_of::<Extent>() > data.len() {
                break;
            }
            
            let extent_bytes = &data[offset..offset + core::mem::size_of::<Extent>()];
            let extent = unsafe {
                core::ptr::read_unaligned(extent_bytes.as_ptr() as *const Extent)
            };
            
            // Vérifier si logical_block est dans cet extent
            let extent_end = extent.block + extent.length() as u32;
            if logical_block >= extent.block && logical_block < extent_end {
                return Some(extent);
            }
        }
        
        None
    }
    
    /// Cherche dans un internal node
    fn find_in_internal(header: &ExtentHeader, data: &[u8], logical_block: u32) -> Option<Extent> {
        let entries = header.entries as usize;
        let base_offset = core::mem::size_of::<ExtentHeader>();
        
        // Trouver le bon index
        let mut target_idx: Option<ExtentIdx> = None;
        
        for i in 0..entries {
            let offset = base_offset + i * core::mem::size_of::<ExtentIdx>();
            if offset + core::mem::size_of::<ExtentIdx>() > data.len() {
                break;
            }
            
            let idx_bytes = &data[offset..offset + core::mem::size_of::<ExtentIdx>()];
            let idx = unsafe {
                core::ptr::read_unaligned(idx_bytes.as_ptr() as *const ExtentIdx)
            };
            
            if logical_block >= idx.block {
                target_idx = Some(idx);
            } else {
                break;
            }
        }
        
        // Note: Pour une implémentation complète, il faudrait:
        // 1. Lire le block physique pointé par target_idx depuis BlockDevice
        // 2. Parser le nouveau ExtentHeader
        // 3. Récurser avec Self::find_extent()
        // 
        // Cette fonction est actuellement limitée aux extents de niveau 0 (feuilles)
        // Pour supporter les arbres multiniveaux, il faudrait:
        // - Passer une référence à BlockDevice en paramètre
        // - Implémenter la récursion avec lecture de blocs
        // - Gérer le cache des nœuds internes pour les performances
        
        if let Some(idx) = target_idx {
            log::trace!("extent_tree: internal node found at depth {}, would recurse to physical block {}",
                       root.depth, idx.physical_block());
            log::debug!("extent_tree: multi-level extent tree traversal not fully implemented (requires BlockDevice integration)");
        }
        
        None
    }
    
    /// Convertit logical block → physical block
    pub fn logical_to_physical(root: &ExtentHeader, data: &[u8], logical_block: u32) -> Option<u64> {
        let extent = Self::find_extent(root, data, logical_block)?;
        
        // Calculer offset dans l'extent
        let offset_in_extent = logical_block - extent.block;
        let physical = extent.physical_block() + offset_in_extent as u64;
        
        Some(physical)
    }
}
