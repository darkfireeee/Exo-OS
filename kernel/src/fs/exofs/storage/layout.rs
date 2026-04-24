// kernel/src/fs/exofs/storage/layout.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Layout disque ExoFS — offsets fixes, calculs de zones, validation
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE ARITH-01 : checked_add/checked_sub OBLIGATOIRES pour tous les offsets.
// Violation = overflow silencieux → écriture à l'offset 0 → superblock écrasé.
//
// Layout physique du disque :
//   Offset   0       : SuperBlock primaire (4 KB)
//   Offset   4 KB    : EpochSlot A
//   Offset   8 KB    : EpochSlot B
//   Offset  12 KB    : SuperBlock miroir
//   Offset   1 MB    : Heap général (blobs, objets)
//   Offset size-8KB  : EpochSlot C
//   Offset size-4KB  : SuperBlock miroir final
//
// Taille minimale d'un volume ExoFS valide : 2 MB (contrainte hard codée).

use crate::fs::exofs::core::{DiskOffset, ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes du layout
// ─────────────────────────────────────────────────────────────────────────────

/// Magic ExoFS : "EXOF".
pub const EXOFS_MAGIC: u32 = 0x4558_4F46;

/// Taille d'un bloc ExoFS en octets (4 KB).
pub const BLOCK_SIZE: u64 = 4096;

/// Offset du SuperBlock primaire (début du disque).
pub const SB_PRIMARY_OFFSET: u64 = 0;

/// Offset du SuperBlock primaire en octets (alias).
pub const SUPERBLOCK_SIZE: u64 = 4096;

/// Offset du Slot Epoch A.
pub const EPOCH_SLOT_A_OFFSET: u64 = 4 * 1024; // 4 KB

/// Offset du Slot Epoch B.
pub const EPOCH_SLOT_B_OFFSET: u64 = 8 * 1024; // 8 KB

/// Offset du SuperBlock miroir à 12 KB.
pub const SB_MIRROR_12K_OFFSET: u64 = 12 * 1024; // 12 KB

/// Début du heap général (1 MB).
pub const HEAP_START_OFFSET: u64 = 1024 * 1024; // 1 MB

/// Décalage depuis la fin du disque pour l'Epoch Slot C (8 KB avant la fin).
pub const EPOCH_SLOT_C_FROM_END: u64 = 8 * 1024; // 8 KB depuis la fin

/// Décalage depuis la fin du disque pour le SuperBlock miroir final (4 KB avant la fin).
pub const SB_MIRROR_END_FROM_END: u64 = 4 * 1024; // 4 KB depuis la fin

/// Taille d'un Epoch Slot en octets (4 KB).
pub const EPOCH_SLOT_SIZE: u64 = 4096;

/// Taille minimale d'un volume ExoFS valide.
pub const MIN_DISK_SIZE: u64 = 2 * 1024 * 1024; // 2 MB

/// Nombre de miroirs du superblock (primaire + 12KB + fin).
pub const SB_MIRROR_COUNT: usize = 3;

/// Alignement des allocations heap (blocs de 4 KB).
pub const HEAP_ALLOC_ALIGN: u64 = BLOCK_SIZE;

/// Nombre d'ordres buddy (2^0 × BLOCK_SIZE .. 2^MAX_BUDDY_ORDER × BLOCK_SIZE).
pub const MAX_BUDDY_ORDER: u32 = 14; // 2^14 × 4KB = 64 MB max

/// Taille maximale d'un extent alloué.
pub const MAX_EXTENT_SIZE: u64 = BLOCK_SIZE << MAX_BUDDY_ORDER; // 64 MB

// ─────────────────────────────────────────────────────────────────────────────
// DiskZone — description d'une plage disque
// ─────────────────────────────────────────────────────────────────────────────

/// Représente une zone contiguë sur le disque.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DiskZone {
    /// Offset de début (inclusif).
    pub start: DiskOffset,
    /// Longueur en octets.
    pub len: u64,
}

impl DiskZone {
    /// Constructeur vérifié.
    ///
    /// RÈGLE ARITH-01 : calcul de `end` via checked_add.
    pub fn new(start: DiskOffset, len: u64) -> ExofsResult<Self> {
        if len == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        // Vérifie que start + len ne déborde pas.
        start.0.checked_add(len).ok_or(ExofsError::OffsetOverflow)?;
        Ok(Self { start, len })
    }

    /// Offset de fin (exclusif).
    ///
    /// RÈGLE ARITH-01 : checked_add.
    #[inline]
    pub fn end(&self) -> ExofsResult<DiskOffset> {
        self.start
            .0
            .checked_add(self.len)
            .ok_or(ExofsError::OffsetOverflow)
            .map(DiskOffset)
    }

    /// Vérifie si un offset se trouve dans cette zone.
    #[inline]
    pub fn contains(&self, offset: DiskOffset) -> bool {
        offset.0 >= self.start.0 && offset.0 < self.start.0.saturating_add(self.len)
    }

    /// Vérifie si deux zones se chevauchent.
    pub fn overlaps(&self, other: &DiskZone) -> bool {
        let self_end = self.start.0.saturating_add(self.len);
        let other_end = other.start.0.saturating_add(other.len);
        self.start.0 < other_end && other.start.0 < self_end
    }

    /// Nombre de blocs de 4 KB couverts par cette zone (arrondi au-dessus).
    pub fn blocks_4k(&self) -> u64 {
        self.len.saturating_add(BLOCK_SIZE - 1) / BLOCK_SIZE
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Offsets statiques (indépendants de la taille du disque)
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne l'offset du SuperBlock primaire.
#[inline]
pub fn superblock_primary() -> DiskOffset {
    DiskOffset(SB_PRIMARY_OFFSET)
}

/// Retourne la zone du SuperBlock primaire.
#[inline]
pub fn superblock_primary_zone() -> DiskZone {
    DiskZone {
        start: DiskOffset(SB_PRIMARY_OFFSET),
        len: SUPERBLOCK_SIZE,
    }
}

/// Retourne l'offset du Slot Epoch A (fixe : 4 KB).
#[inline]
pub fn epoch_slot_a() -> DiskOffset {
    DiskOffset(EPOCH_SLOT_A_OFFSET)
}

/// Retourne la zone du Slot Epoch A.
#[inline]
pub fn epoch_slot_a_zone() -> DiskZone {
    DiskZone {
        start: DiskOffset(EPOCH_SLOT_A_OFFSET),
        len: EPOCH_SLOT_SIZE,
    }
}

/// Retourne l'offset du Slot Epoch B (fixe : 8 KB).
#[inline]
pub fn epoch_slot_b() -> DiskOffset {
    DiskOffset(EPOCH_SLOT_B_OFFSET)
}

/// Retourne la zone du Slot Epoch B.
#[inline]
pub fn epoch_slot_b_zone() -> DiskZone {
    DiskZone {
        start: DiskOffset(EPOCH_SLOT_B_OFFSET),
        len: EPOCH_SLOT_SIZE,
    }
}

/// Retourne l'offset du SuperBlock miroir à 12 KB.
#[inline]
pub fn superblock_mirror_12k() -> DiskOffset {
    DiskOffset(SB_MIRROR_12K_OFFSET)
}

/// Retourne la zone du SuperBlock miroir à 12 KB.
#[inline]
pub fn superblock_mirror_12k_zone() -> DiskZone {
    DiskZone {
        start: DiskOffset(SB_MIRROR_12K_OFFSET),
        len: SUPERBLOCK_SIZE,
    }
}

/// Retourne l'offset du début du heap général (1 MB).
#[inline]
pub fn heap_start() -> DiskOffset {
    DiskOffset(HEAP_START_OFFSET)
}

// ─────────────────────────────────────────────────────────────────────────────
// Offsets dépendants de la taille du disque
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne l'offset du Slot Epoch C (disk_size - 8 KB).
///
/// # Règle ARITH-01
/// Utilise checked_sub pour éviter l'underflow.
#[inline]
pub fn epoch_slot_c(disk_size_bytes: u64) -> ExofsResult<DiskOffset> {
    disk_size_bytes
        .checked_sub(EPOCH_SLOT_C_FROM_END)
        .ok_or(ExofsError::OffsetOverflow)
        .map(DiskOffset)
}

/// Retourne la zone du Slot Epoch C.
pub fn epoch_slot_c_zone(disk_size_bytes: u64) -> ExofsResult<DiskZone> {
    let start = epoch_slot_c(disk_size_bytes)?;
    Ok(DiskZone {
        start,
        len: EPOCH_SLOT_SIZE,
    })
}

/// Retourne l'offset du SuperBlock miroir final (disk_size - 4 KB).
///
/// # Règle ARITH-01
#[inline]
pub fn superblock_mirror_end(disk_size_bytes: u64) -> ExofsResult<DiskOffset> {
    disk_size_bytes
        .checked_sub(SB_MIRROR_END_FROM_END)
        .ok_or(ExofsError::OffsetOverflow)
        .map(DiskOffset)
}

/// Retourne la zone du SuperBlock miroir final.
pub fn superblock_mirror_end_zone(disk_size_bytes: u64) -> ExofsResult<DiskZone> {
    let start = superblock_mirror_end(disk_size_bytes)?;
    Ok(DiskZone {
        start,
        len: SUPERBLOCK_SIZE,
    })
}

/// Retourne la zone heap [HEAP_START, epoch_slot_c).
pub fn heap_zone(disk_size_bytes: u64) -> ExofsResult<DiskZone> {
    let start = heap_start();
    let end = epoch_slot_c(disk_size_bytes)?;
    if end.0 <= start.0 {
        return Err(ExofsError::OffsetOverflow);
    }
    let len = end.0 - start.0;
    Ok(DiskZone { start, len })
}

/// Retourne les 3 offsets miroirs du SuperBlock pour un disque de taille donnée.
pub fn superblock_mirror_offsets(disk_size_bytes: u64) -> ExofsResult<[DiskOffset; 3]> {
    let mirror_end = superblock_mirror_end(disk_size_bytes)?;
    Ok([
        DiskOffset(SB_PRIMARY_OFFSET),
        DiskOffset(SB_MIRROR_12K_OFFSET),
        mirror_end,
    ])
}

// ─────────────────────────────────────────────────────────────────────────────
// Conversions secteur / octet / bloc
// ─────────────────────────────────────────────────────────────────────────────

/// Convertit un offset en numéro de secteur 512 octets (arrondi bas).
#[inline]
pub fn offset_to_sector_512(offset: DiskOffset) -> u64 {
    offset.0 / 512
}

/// Convertit un numéro de secteur 512 octets en offset.
#[inline]
pub fn sector_512_to_offset(sector: u64) -> ExofsResult<DiskOffset> {
    sector
        .checked_mul(512)
        .ok_or(ExofsError::OffsetOverflow)
        .map(DiskOffset)
}

/// Convertit un offset en numéro de bloc 4 KB (arrondi bas).
#[inline]
pub fn offset_to_block_4k(offset: DiskOffset) -> u64 {
    offset.0 / BLOCK_SIZE
}

/// Convertit un numéro de bloc 4 KB en offset.
#[inline]
pub fn block_4k_to_offset(block: u64) -> ExofsResult<DiskOffset> {
    block
        .checked_mul(BLOCK_SIZE)
        .ok_or(ExofsError::OffsetOverflow)
        .map(DiskOffset)
}

/// Calcule l'offset de fin d'une zone {start, len} avec overflow check.
///
/// # Règle ARITH-01
#[inline]
pub fn zone_end(start: DiskOffset, len: u64) -> ExofsResult<DiskOffset> {
    start
        .0
        .checked_add(len)
        .ok_or(ExofsError::OffsetOverflow)
        .map(DiskOffset)
}

/// Vérifie qu'un {offset, len} est compris dans un disque de `disk_size` octets.
///
/// # Règle ARITH-01
pub fn check_bounds(offset: DiskOffset, len: u64, disk_size: u64) -> ExofsResult<()> {
    let end = offset
        .0
        .checked_add(len)
        .ok_or(ExofsError::OffsetOverflow)?;
    if end > disk_size {
        return Err(ExofsError::OffsetOverflow);
    }
    Ok(())
}

/// Calcule l'offset `n` blocs après un offset de base.
///
/// # Règle ARITH-01
#[inline]
pub fn blocks_to_offset(base: DiskOffset, n_blocks: u64) -> ExofsResult<DiskOffset> {
    n_blocks
        .checked_mul(BLOCK_SIZE)
        .and_then(|delta| base.0.checked_add(delta))
        .ok_or(ExofsError::OffsetOverflow)
        .map(DiskOffset)
}

/// Aligne un offset vers le haut sur `align` (doit être puissance de 2).
///
/// # Règle ARITH-01
#[inline]
pub fn align_up(offset: DiskOffset, align: u64) -> ExofsResult<DiskOffset> {
    debug_assert!(align.is_power_of_two(), "align must be power of 2");
    let mask = align.checked_sub(1).ok_or(ExofsError::OffsetOverflow)?;
    let aligned = offset
        .0
        .checked_add(mask)
        .ok_or(ExofsError::OffsetOverflow)?
        & !mask;
    Ok(DiskOffset(aligned))
}

/// Aligne un offset vers le bas sur `align`.
#[inline]
pub fn align_down(offset: DiskOffset, align: u64) -> ExofsResult<DiskOffset> {
    debug_assert!(align.is_power_of_two(), "align must be power of 2");
    let mask = align.checked_sub(1).ok_or(ExofsError::OffsetOverflow)?;
    Ok(DiskOffset(offset.0 & !mask))
}

/// Arrondit `size` au prochain multiple de BLOCK_SIZE (4 KB).
///
/// # Règle ARITH-01 : checked_add.
#[inline]
pub fn round_up_block_size(size: u64) -> ExofsResult<u64> {
    let mask = BLOCK_SIZE
        .checked_sub(1)
        .ok_or(ExofsError::OffsetOverflow)?;
    size.checked_add(mask)
        .ok_or(ExofsError::OffsetOverflow)
        .map(|v| v & !mask)
}

/// Nombre de blocs de 4 KB nécessaires pour contenir `size` octets.
#[inline]
pub fn blocks_for_bytes(size: u64) -> ExofsResult<u64> {
    round_up_block_size(size).map(|rounded| rounded / BLOCK_SIZE)
}

// ─────────────────────────────────────────────────────────────────────────────
// Validation du layout
// ─────────────────────────────────────────────────────────────────────────────

/// Valide qu'un volume de `disk_size_bytes` a un layout ExoFS cohérent.
///
/// Vérifie :
/// - Taille minimale (2 MB)
/// - Les zones de métadonnées ne se chevauchent pas
/// - Le heap est d'au moins 1 bloc
pub fn validate_disk_layout(disk_size_bytes: u64) -> ExofsResult<()> {
    // Taille minimale.
    if disk_size_bytes < MIN_DISK_SIZE {
        return Err(ExofsError::InvalidArgument);
    }

    // L'offset de fin du miroir final doit être ≤ disk_size.
    let mirror_end_off = superblock_mirror_end(disk_size_bytes)?;
    let mirror_end = mirror_end_off
        .0
        .checked_add(SUPERBLOCK_SIZE)
        .ok_or(ExofsError::OffsetOverflow)?;
    if mirror_end > disk_size_bytes {
        return Err(ExofsError::OffsetOverflow);
    }

    // L'Epoch Slot C doit venir avant le SuperBlock miroir final.
    let slot_c = epoch_slot_c(disk_size_bytes)?;
    if slot_c.0 >= mirror_end_off.0 {
        return Err(ExofsError::OffsetOverflow);
    }

    // Le heap doit avoir au moins 1 bloc.
    let hz = heap_zone(disk_size_bytes)?;
    if hz.len < BLOCK_SIZE {
        return Err(ExofsError::InvalidArgument);
    }

    // Le heap ne doit pas chevaucher les métadonnées initiales.
    let meta_end = SB_MIRROR_12K_OFFSET
        .checked_add(SUPERBLOCK_SIZE)
        .ok_or(ExofsError::OffsetOverflow)?;
    if hz.start.0 < meta_end {
        return Err(ExofsError::OffsetOverflow);
    }

    Ok(())
}

/// Retourne le nombre de blocs de 4 KB disponibles dans le heap.
pub fn heap_blocks(disk_size_bytes: u64) -> ExofsResult<u64> {
    heap_zone(disk_size_bytes).and_then(|z| blocks_for_bytes(z.len))
}

// ─────────────────────────────────────────────────────────────────────────────
// LayoutMap — résumé du layout pour un volume donné
// ─────────────────────────────────────────────────────────────────────────────

/// Résumé calculé une fois au montage du volume.
#[derive(Copy, Clone, Debug)]
pub struct LayoutMap {
    pub disk_size: u64,
    pub sb_primary: DiskOffset,
    pub epoch_slot_a: DiskOffset,
    pub epoch_slot_b: DiskOffset,
    pub sb_mirror_12k: DiskOffset,
    pub heap_start: DiskOffset,
    pub heap_end: DiskOffset,
    pub heap_len: u64,
    pub epoch_slot_c: DiskOffset,
    pub sb_mirror_end: DiskOffset,
    pub heap_blocks: u64,
}

impl LayoutMap {
    /// Construit le LayoutMap depuis la taille du disque.
    ///
    /// Retourne une erreur si le layout est invalide.
    pub fn new(disk_size_bytes: u64) -> ExofsResult<Self> {
        validate_disk_layout(disk_size_bytes)?;

        let slot_c = epoch_slot_c(disk_size_bytes)?;
        let sb_end = superblock_mirror_end(disk_size_bytes)?;
        let hz = heap_zone(disk_size_bytes)?;
        let hb = blocks_for_bytes(hz.len)?;

        Ok(Self {
            disk_size: disk_size_bytes,
            sb_primary: DiskOffset(SB_PRIMARY_OFFSET),
            epoch_slot_a: DiskOffset(EPOCH_SLOT_A_OFFSET),
            epoch_slot_b: DiskOffset(EPOCH_SLOT_B_OFFSET),
            sb_mirror_12k: DiskOffset(SB_MIRROR_12K_OFFSET),
            heap_start: hz.start,
            heap_end: DiskOffset(hz.start.0.saturating_add(hz.len)),
            heap_len: hz.len,
            epoch_slot_c: slot_c,
            sb_mirror_end: sb_end,
            heap_blocks: hb,
        })
    }

    /// Les 3 offsets miroirs du SuperBlock.
    pub fn superblock_mirrors(&self) -> [DiskOffset; 3] {
        [self.sb_primary, self.sb_mirror_12k, self.sb_mirror_end]
    }

    /// Retourne le pourcentage d'espace de métadonnées par rapport à la taille totale.
    pub fn metadata_overhead_pct(&self) -> u64 {
        let meta = HEAP_START_OFFSET; // tout ce qui est avant le heap
        if self.disk_size == 0 {
            return 0;
        }
        (meta as u128 * 100 / self.disk_size as u128) as u64
    }

    /// Vérifie qu'un offset est dans la zone heap.
    #[inline]
    pub fn is_in_heap(&self, offset: DiskOffset) -> bool {
        offset.0 >= self.heap_start.0 && offset.0 < self.heap_end.0
    }

    /// Vérifie qu'une zone {offset, len} est entièrement dans le heap.
    pub fn zone_in_heap(&self, offset: DiskOffset, len: u64) -> bool {
        let end = match offset.0.checked_add(len) {
            Some(e) => e,
            None => return false,
        };
        offset.0 >= self.heap_start.0 && end <= self.heap_end.0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests unitaires (cfg(test) — compilés seulement si `--test`)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_layout_2mb_valid() {
        assert!(validate_disk_layout(2 * 1024 * 1024).is_ok());
    }

    #[test]
    fn test_layout_too_small() {
        assert!(validate_disk_layout(1024).is_err());
    }

    #[test]
    fn test_heap_zone() {
        let disk = 64 * 1024 * 1024u64;
        let hz = heap_zone(disk).unwrap();
        assert_eq!(hz.start.0, HEAP_START_OFFSET);
        // La fin doit être à disk - 8 KB (epoch_slot_c).
        assert_eq!(hz.start.0 + hz.len, disk - EPOCH_SLOT_C_FROM_END);
    }

    #[test]
    fn test_align_up_4k() {
        let off = align_up(DiskOffset(4097), 4096).unwrap();
        assert_eq!(off.0, 8192);
    }

    #[test]
    fn test_round_up_block_size() {
        assert_eq!(round_up_block_size(1).unwrap(), 4096);
        assert_eq!(round_up_block_size(4096).unwrap(), 4096);
        assert_eq!(round_up_block_size(4097).unwrap(), 8192);
    }

    #[test]
    fn test_layout_map_new() {
        let lm = LayoutMap::new(64 * 1024 * 1024).unwrap();
        assert!(lm.heap_blocks > 0);
        assert!(lm.is_in_heap(DiskOffset(HEAP_START_OFFSET)));
        assert!(!lm.is_in_heap(DiskOffset(0)));
    }

    #[test]
    fn test_zone_overlaps() {
        let z1 = DiskZone::new(DiskOffset(0), 4096).unwrap();
        let z2 = DiskZone::new(DiskOffset(2048), 4096).unwrap();
        let z3 = DiskZone::new(DiskOffset(8192), 4096).unwrap();
        assert!(z1.overlaps(&z2));
        assert!(!z1.overlaps(&z3));
    }

    #[test]
    fn test_superblock_mirrors() {
        let disk = 32 * 1024 * 1024u64;
        let mirrors = superblock_mirror_offsets(disk).unwrap();
        assert_eq!(mirrors[0].0, 0);
        assert_eq!(mirrors[1].0, SB_MIRROR_12K_OFFSET);
        assert_eq!(mirrors[2].0, disk - SB_MIRROR_END_FROM_END);
    }
}
