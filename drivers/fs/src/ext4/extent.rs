// drivers/fs/src/ext4/extent.rs — Extent tree ext4  (exo-os-driver-fs)
//
// RÈGLE FS-EXT4-04 : AUCUN reflink, AUCUN CoW ici (ext4 classique uniquement).

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct Ext4ExtentHeader {
    pub eh_magic:  u16,  // 0xF30A
    pub eh_entries: u16,
    pub eh_max:    u16,
    pub eh_depth:  u16,  // 0 = feuille
    pub eh_generation: u32,
}

pub const EXT4_EXT_MAGIC: u16 = 0xF30A;

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct Ext4ExtentIdx {
    pub ei_block:   u32,
    pub ei_leaf_lo: u32,
    pub ei_leaf_hi: u16,
    pub ei_unused:  u16,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
pub struct Ext4Extent {
    pub ee_block:   u32,   // premier bloc logique
    pub ee_len:     u16,   // nb de blocs (bit 15 = préalloué)
    pub ee_start_hi: u16,  // 32..47 bits du bloc physique
    pub ee_start_lo: u32,  // 0..31 bits du bloc physique
}

impl Ext4Extent {
    pub fn physical_block(&self) -> u64 {
        (self.ee_start_lo as u64) | ((self.ee_start_hi as u64) << 32)
    }

    pub fn len(&self) -> u16 {
        self.ee_len & 0x7FFF
    }
}

/// Cherche l'extent couvrant `logical_block` dans le tampon d'un inode.
/// `iblock` : les 15 u32 du champ i_block de l'inode.
/// Retourne (physical_block, nb_blocs_couverts) ou None.
pub fn find_extent(iblock: &[u32; 15], logical_block: u32) -> Option<(u64, u16)> {
    let hdr = unsafe { &*(iblock.as_ptr() as *const Ext4ExtentHeader) };
    if hdr.eh_magic != EXT4_EXT_MAGIC {
        return None;
    }
    search_level(iblock.as_ptr() as *const u8, logical_block, hdr.eh_depth)
}

fn search_level(base: *const u8, logical: u32, depth: u16) -> Option<(u64, u16)> {
    let hdr = unsafe { &*(base as *const Ext4ExtentHeader) };
    if hdr.eh_magic != EXT4_EXT_MAGIC {
        return None;
    }
    if depth == 0 {
        // Feuille : liste d'Ext4Extent
        let entries = base.wrapping_add(12) as *const Ext4Extent;
        for i in 0..hdr.eh_entries as usize {
            let ext = unsafe { &*entries.add(i) };
            let start = ext.ee_block;
            let len   = ext.len() as u32;
            if logical >= start && logical < start + len {
                let offset = logical - start;
                return Some((ext.physical_block() + offset as u64, (len - offset) as u16));
            }
        }
        None
    } else {
        // Nœud interne : liste d'Ext4ExtentIdx (on ne parcourt pas le disque ici)
        None
    }
}
