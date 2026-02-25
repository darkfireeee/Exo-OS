// drivers/fs/src/fat32/bpb.rs
//
// FAT32 — BIOS Parameter Block  (exo-os-driver-fs)
//
// RÈGLE FS-FAT32-01 : Valider et rejeter FAT12/FAT16.

use core::mem::size_of;
use crate::FsDriverError;

/// BPB FAT32 on-disk (512 octets — secteur de boot).
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct BiosParameterBlock {
    pub bs_jmp_boot:        [u8; 3],
    pub bs_oem_name:        [u8; 8],
    pub bpb_bytes_per_sec:  u16,
    pub bpb_sec_per_clus:   u8,
    pub bpb_resvd_sec_cnt:  u16,
    pub bpb_num_fats:       u8,
    pub bpb_root_ent_cnt:   u16,
    pub bpb_tot_sec16:      u16,
    pub bpb_media:          u8,
    pub bpb_fat_sz16:       u16,
    pub bpb_sec_per_trk:    u16,
    pub bpb_num_heads:      u16,
    pub bpb_hidd_sec:       u32,
    pub bpb_tot_sec32:      u32,
    // FAT32 extended :
    pub bpb_fat_sz32:       u32,
    pub bpb_ext_flags:      u16,
    pub bpb_fs_ver:         u16,
    pub bpb_root_clus:      u32,
    pub bpb_fs_info:        u16,
    pub bpb_bk_boot_sec:    u16,
    pub bpb_reserved:       [u8; 12],
    pub bs_drv_num:         u8,
    pub bs_reserved1:       u8,
    pub bs_boot_sig:        u8,
    pub bs_vol_id:          u32,
    pub bs_vol_lab:         [u8; 11],
    pub bs_fil_sys_type:    [u8; 8],
    pub _boot_code:         [u8; 420],
    pub boot_sig_55:        u8,
    pub boot_sig_aa:        u8,
}

const _: () = assert!(size_of::<BiosParameterBlock>() == 512);

/// BPB analysé et validé.
#[derive(Clone, Debug)]
pub struct ParsedBpb {
    pub bytes_per_sec:     u32,
    pub sec_per_cluster:   u32,
    pub resvd_sectors:     u32,
    pub num_fats:          u32,
    pub fat_sz_sectors:    u32,
    pub total_sectors:     u64,
    pub root_cluster:      u32,
    pub data_start:        u32,
    pub cluster_count:     u32,
    pub bytes_per_cluster: u32,
    pub fs_info_sector:    u32,
    pub fat_start:         u32,
    pub fat2_start:        u32,
}

impl ParsedBpb {
    pub fn cluster_to_sector(&self, cluster: u32) -> u64 {
        self.data_start as u64 + (cluster - 2) as u64 * self.sec_per_cluster as u64
    }
}

/// Parse un tampon de 512 octets comme un BPB FAT32.
/// RÈGLE FS-FAT32-01 : rejet FAT12/FAT16.
pub fn parse_bpb(raw: &[u8; 512]) -> Result<ParsedBpb, FsDriverError> {
    // SAFETY: raw est de taille 512 = taille de BiosParameterBlock.
    let bpb: BiosParameterBlock = unsafe {
        core::ptr::read_unaligned(raw.as_ptr() as *const BiosParameterBlock)
    };

    if bpb.boot_sig_55 != 0x55 || bpb.boot_sig_aa != 0xAA {
        return Err(FsDriverError::BadSignature);
    }

    let bps = bpb.bpb_bytes_per_sec as u32;
    if bps != 512 && bps != 1024 && bps != 2048 && bps != 4096 {
        return Err(FsDriverError::InvalidParameter);
    }

    let spc      = bpb.bpb_sec_per_clus as u32;
    let num_fats = bpb.bpb_num_fats as u32;
    let resvd    = bpb.bpb_resvd_sec_cnt as u32;
    let fat_sz   = if bpb.bpb_fat_sz32 != 0 { bpb.bpb_fat_sz32 } else { bpb.bpb_fat_sz16 as u32 };
    let tot_sec  = if bpb.bpb_tot_sec32 != 0 { bpb.bpb_tot_sec32 as u64 } else { bpb.bpb_tot_sec16 as u64 };
    let data_start = resvd + num_fats * fat_sz;
    let data_sectors = if tot_sec > data_start as u64 { tot_sec - data_start as u64 } else { 0 };
    let cluster_count = if spc > 0 { (data_sectors / spc as u64) as u32 } else { 0 };

    if cluster_count < 65525 {
        return Err(FsDriverError::WrongFsType); // FAT12 ou FAT16
    }

    Ok(ParsedBpb {
        bytes_per_sec: bps, sec_per_cluster: spc, resvd_sectors: resvd, num_fats,
        fat_sz_sectors: fat_sz, total_sectors: tot_sec,
        root_cluster: bpb.bpb_root_clus,
        data_start, cluster_count,
        bytes_per_cluster: bps * spc,
        fs_info_sector: bpb.bpb_fs_info as u32,
        fat_start: resvd,
        fat2_start: resvd + fat_sz,
    })
}
