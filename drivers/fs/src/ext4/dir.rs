// drivers/fs/src/ext4/dir.rs — Répertoires ext4  (exo-os-driver-fs)

pub const DT_UNKNOWN: u8 = 0;
pub const DT_REG:  u8 = 1;
pub const DT_DIR:  u8 = 2;
pub const DT_LNK:  u8 = 7;

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Ext4DirEntry2 {
    pub inode:     u32,
    pub rec_len:   u16,
    pub name_len:  u8,
    pub file_type: u8,
    // name suit immédiatement (de longueur name_len)
}

/// Résultat d'une recherche dans un bloc de répertoire.
#[derive(Debug, Clone)]
pub struct DirLookupResult {
    pub inode:     u32,
    pub file_type: u8,
}

/// Cherche `target` dans un bloc de 4096 octets de répertoire.
pub fn lookup_in_block(block: &[u8], target: &[u8]) -> Option<DirLookupResult> {
    let mut offset = 0usize;
    while offset + 8 <= block.len() {
        let entry = unsafe { &*(block.as_ptr().add(offset) as *const Ext4DirEntry2) };
        let rec_len = entry.rec_len as usize;
        if rec_len == 0 || offset + rec_len > block.len() {
            break;
        }
        if entry.inode != 0 {
            let name_len = entry.name_len as usize;
            let name_start = offset + 8;
            if name_start + name_len <= block.len() {
                let name = &block[name_start..name_start + name_len];
                if name == target {
                    return Some(DirLookupResult {
                        inode: entry.inode,
                        file_type: entry.file_type,
                    });
                }
            }
        }
        offset += rec_len;
    }
    None
}
