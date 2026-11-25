// drivers/fs/src/fat32/fat_table.rs
//
// FAT32 — Table FAT  (exo-os-driver-fs)
// RÈGLE FS-FAT32-05 : Écriture toujours sur FAT1 + FAT2.

pub const FAT_MASK:  u32 = 0x0FFF_FFFF;
pub const FAT_FREE:  u32 = 0x0000_0000;
pub const FAT_BAD:   u32 = 0x0FFF_FFF7;
pub const FAT_EOC:   u32 = 0x0FFF_FFF8;

/// Vrai si l'entrée est une fin de chaîne.
#[inline] pub fn is_eoc(entry: u32) -> bool { (entry & FAT_MASK) >= FAT_EOC }
/// Vrai si le cluster est libre.
#[inline] pub fn is_free(entry: u32) -> bool { (entry & FAT_MASK) == FAT_FREE }
/// Vrai si le cluster est défectueux.
#[inline] pub fn is_bad(entry: u32) -> bool  { (entry & FAT_MASK) == FAT_BAD }

/// Calcule l'offset byte d'une entrée FAT pour un cluster.
/// Retourne (byte_offset_depuis_debut_fat).
#[inline]
pub fn fat_entry_byte_offset(cluster: u32) -> u64 {
    cluster as u64 * 4
}

/// Extrait l'entrée FAT32 depuis un tampon `fat_buf` (données d'un secteur FAT).
/// `byte_in_sector` : offset dans le secteur du fat_buf.
pub fn read_entry_from_buf(fat_buf: &[u8], byte_in_sector: usize) -> u32 {
    if byte_in_sector + 4 > fat_buf.len() {
        return FAT_BAD;
    }
    let raw = u32::from_le_bytes([
        fat_buf[byte_in_sector],
        fat_buf[byte_in_sector + 1],
        fat_buf[byte_in_sector + 2],
        fat_buf[byte_in_sector + 3],
    ]);
    raw & FAT_MASK
}

/// Écrit une entrée FAT32 dans un tampon (FAT1 ou FAT2 — appeler deux fois).
/// Conserve les bits 31..28 réservés.
pub fn write_entry_to_buf(fat_buf: &mut [u8], byte_in_sector: usize, value: u32) {
    if byte_in_sector + 4 > fat_buf.len() {
        return;
    }
    let old = u32::from_le_bytes([
        fat_buf[byte_in_sector], fat_buf[byte_in_sector + 1],
        fat_buf[byte_in_sector + 2], fat_buf[byte_in_sector + 3],
    ]);
    let new_val = (old & !FAT_MASK) | (value & FAT_MASK);
    let bytes = new_val.to_le_bytes();
    fat_buf[byte_in_sector..byte_in_sector + 4].copy_from_slice(&bytes);
}
