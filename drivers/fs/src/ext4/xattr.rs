// drivers/fs/src/ext4/xattr.rs — Attributs étendus ext4  (exo-os-driver-fs)

extern crate alloc;
use alloc::vec::Vec;

pub const EXT4_XATTR_MAGIC: u32 = 0xEA020000;

pub const XATTR_INDEX_USER:     u8 = 1;
pub const XATTR_INDEX_POSIX_ACL: u8 = 2;
pub const XATTR_INDEX_TRUSTED:  u8 = 4;
pub const XATTR_INDEX_SECURITY: u8 = 6;

#[repr(C, packed)]
pub struct Ext4XattrHeader {
    pub h_magic:     u32,
    pub h_refcount:  u32,
    pub h_blocks:    u32,
    pub h_hash:      u32,
    pub h_checksum:  u32,
    pub h_reserved:  [u32; 3],
}

#[repr(C, packed)]
pub struct Ext4XattrEntry {
    pub e_name_len:    u8,
    pub e_name_index:  u8,
    pub e_value_offs:  u16,
    pub e_value_inum:  u32,
    pub e_value_size:  u32,
    pub e_hash:        u32,
    // e_name : e_name_len octets
}

#[derive(Debug, Clone)]
pub struct XattrItem {
    pub index: u8,
    pub name:  alloc::vec::Vec<u8>,
    pub value: alloc::vec::Vec<u8>,
}

/// Parse les xattrs depuis un bloc de 4096 octets.
pub fn parse_xattrs(block: &[u8]) -> Vec<XattrItem> {
    let mut result = Vec::new();
    if block.len() < 32 {
        return result;
    }
    let hdr = unsafe { &*(block.as_ptr() as *const Ext4XattrHeader) };
    if hdr.h_magic != EXT4_XATTR_MAGIC {
        return result;
    }
    let mut offset = 32usize;
    while offset + 16 <= block.len() {
        let entry = unsafe { &*(block.as_ptr().add(offset) as *const Ext4XattrEntry) };
        if entry.e_name_len == 0 {
            break;
        }
        let name_len = entry.e_name_len as usize;
        let name_start = offset + 16;
        if name_start + name_len > block.len() {
            break;
        }
        let name = block[name_start..name_start + name_len].to_vec();
        let val_off  = entry.e_value_offs as usize;
        let val_size = entry.e_value_size as usize;
        let value = if val_off + val_size <= block.len() {
            block[val_off..val_off + val_size].to_vec()
        } else {
            alloc::vec![]
        };
        result.push(XattrItem { index: entry.e_name_index, name, value });
        offset += 16 + name_len;
        // Alignement sur 4 octets.
        offset = (offset + 3) & !3;
    }
    result
}
