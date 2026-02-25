// drivers/fs/src/fat32/dir_entry.rs
//
// FAT32 — Entrées de répertoire (8.3 + LFN)  (exo-os-driver-fs)
// RÈGLE FS-FAT32-04 : LFN en ordre inversé, UTF-16 → UTF-8.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::mem::size_of;

pub const ATTR_READ_ONLY: u8 = 0x01;
pub const ATTR_HIDDEN:    u8 = 0x02;
pub const ATTR_SYSTEM:    u8 = 0x04;
pub const ATTR_VOLUME_ID: u8 = 0x08;
pub const ATTR_DIRECTORY: u8 = 0x10;
pub const ATTR_ARCHIVE:   u8 = 0x20;
pub const ATTR_LFN:       u8 = 0x0F;
pub const DIR_ENTRY_FREE: u8 = 0xE5;
pub const DIR_ENTRY_EOD:  u8 = 0x00;
pub const LFN_LAST_MASK:  u8 = 0x40;

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Fat32DirEntry {
    pub dir_name:         [u8; 11],
    pub dir_attr:         u8,
    pub dir_nt_res:       u8,
    pub dir_crt_time_tenth: u8,
    pub dir_crt_time:     u16,
    pub dir_crt_date:     u16,
    pub dir_lst_acc_date: u16,
    pub dir_fst_clus_hi:  u16,
    pub dir_wrt_time:     u16,
    pub dir_wrt_date:     u16,
    pub dir_fst_clus_lo:  u16,
    pub dir_file_size:    u32,
}

const _: () = assert!(size_of::<Fat32DirEntry>() == 32);

impl Fat32DirEntry {
    pub fn first_cluster(&self) -> u32 {
        ((self.dir_fst_clus_hi as u32) << 16) | self.dir_fst_clus_lo as u32
    }
    pub fn is_dir(&self) -> bool { self.dir_attr & ATTR_DIRECTORY != 0 }
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct LfnEntry {
    pub order:   u8,
    pub name1:   [u16; 5],
    pub attr:    u8,
    pub lfn_type: u8,
    pub checksum: u8,
    pub name2:   [u16; 6],
    pub fst_clus_lo: u16,
    pub name3:   [u16; 2],
}

const _: () = assert!(size_of::<LfnEntry>() == 32);

/// Checksum du nom court 8.3 pour valider les entrées LFN.
pub fn lfn_checksum(short_name: &[u8; 11]) -> u8 {
    let mut sum: u8 = 0;
    for &b in short_name {
        sum = (sum >> 1) | (sum << 7);
        sum = sum.wrapping_add(b);
    }
    sum
}

#[derive(Clone, Debug)]
pub struct DirEntryParsed {
    pub name:          String,
    pub first_cluster: u32,
    pub file_size:     u32,
    pub is_dir:        bool,
    pub attr:          u8,
}

fn short_name_to_string(raw: &[u8; 11]) -> String {
    let base: Vec<u8> = raw[..8].iter().copied().take_while(|&c| c != b' ').collect();
    let ext:  Vec<u8> = raw[8..11].iter().copied().take_while(|&c| c != b' ').collect();
    let mut s = String::from_utf8_lossy(&base).into_owned();
    if !ext.is_empty() { s.push('.'); s.push_str(&String::from_utf8_lossy(&ext)); }
    s
}

fn utf16_to_utf8(src: &[u16]) -> String {
    char::decode_utf16(src.iter().copied())
        .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// Parse toutes les entrées de répertoire depuis un tampon d'un cluster.
/// RÈGLE FS-FAT32-04 : LFN reconstruits en ordre inversé.
pub fn parse_dir_cluster(buf: &[u8]) -> Vec<DirEntryParsed> {
    let mut out  = Vec::new();
    let mut lfns: Vec<(u8, [u16; 13], u8)> = Vec::new();
    let mut i = 0usize;

    while i + 32 <= buf.len() {
        let first = buf[i];
        if first == DIR_ENTRY_EOD { break; }
        if first == DIR_ENTRY_FREE { lfns.clear(); i += 32; continue; }

        let attr = buf[i + 11];
        if attr == ATTR_LFN {
            let lfn: LfnEntry = unsafe { core::ptr::read_unaligned(buf.as_ptr().add(i) as *const LfnEntry) };
            let order = lfn.order & !LFN_LAST_MASK;
            let mut chars = [0u16; 13];
            chars[..5].copy_from_slice(&lfn.name1);
            chars[5..11].copy_from_slice(&lfn.name2);
            chars[11..13].copy_from_slice(&lfn.name3);
            lfns.push((order, chars, lfn.checksum));
            i += 32; continue;
        }

        let entry: Fat32DirEntry = unsafe { core::ptr::read_unaligned(buf.as_ptr().add(i) as *const Fat32DirEntry) };
        if entry.dir_attr & (ATTR_VOLUME_ID | ATTR_DIRECTORY | ATTR_ARCHIVE) == ATTR_VOLUME_ID {
            lfns.clear(); i += 32; continue;
        }

        let name = if !lfns.is_empty() {
            let cs = lfn_checksum(&entry.dir_name);
            let ok = lfns.iter().all(|(_, _, c)| *c == cs);
            if ok {
                lfns.sort_by_key(|(ord, _, _)| *ord);
                let mut utf16 = Vec::new();
                for (_, chars, _) in &lfns {
                    for &c in chars { if c == 0 || c == 0xFFFF { break; } utf16.push(c); }
                }
                utf16_to_utf8(&utf16)
            } else { short_name_to_string(&entry.dir_name) }
        } else { short_name_to_string(&entry.dir_name) };

        lfns.clear();
        out.push(DirEntryParsed {
            name, first_cluster: entry.first_cluster(),
            file_size: entry.dir_file_size, is_dir: entry.is_dir(), attr: entry.dir_attr,
        });
        i += 32;
    }
    out
}
