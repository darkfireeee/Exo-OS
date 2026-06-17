//! partition_scan — résolution **réelle** de la partition ExoFS via GPT.
//!
//! Avant ce module, le kernel supposait que le périphérique bloc *entier* était
//! le volume ExoFS (superblock lu au LBA 0). C'est faux dès qu'un disque est
//! partitionné GPT (ESP + ExoFS ROOT + ExoFS DATA) : le superblock se trouve au
//! début de la **partition ROOT**, pas au LBA 0 du disque.
//!
//! Ce module utilise le parseur GPT/MBR partagé (`exo-partition`, équivalent de
//! redox-os/partitionlib) pour localiser la partition ExoFS ROOT par son
//! **type-GUID**, puis enveloppe le périphérique global dans un
//! [`PartitionOffsetDevice`] qui décale **toute** l'I/O ExoFS vers le LBA de
//! début de la partition. Ainsi le reste d'ExoFS continue de raisonner en
//! « LBA 0 = début du volume » sans aucune modification.
//!
//! ## Additif — zéro régression
//! - Disque **brut** (image mkfs actuelle, superblock au LBA 0, pas de table) →
//!   `scan` échoue / pas de GPT → **aucun décalage**, comportement inchangé.
//! - Table **MBR legacy** (pas de GPT protecteur) → on n'interprète pas, base 0.
//! - GPT **sans** partition ExoFS ROOT → base 0.
//! - GPT **avec** partition ExoFS ROOT à `start_lba > 0` → décalage appliqué.
//!
//! Toute erreur de parsing (signature, CRC, garde anti-OOM) est traitée comme
//! « pas de partition » : on retombe sur le comportement LBA 0. La promesse est
//! donc tenue sans jamais risquer de casser un volume existant.

extern crate alloc;

use crate::fs::exofs::core::ExofsResult;
use crate::fs::exofs::recovery::boot_recovery::BlockDevice;
use crate::fs::exofs::storage::virtio_adapter;
use alloc::sync::Arc;
use exo_partition::{BlockReader, PartError, Scheme};

/// Partition ExoFS résolue : LBA de début (sur le disque physique) + taille.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedPartition {
    /// LBA de début de la partition sur le périphérique physique.
    pub base_lba: u64,
    /// Nombre de secteurs de la partition.
    pub sectors: u64,
}

/// Adaptateur : expose un `Arc<dyn BlockDevice>` kernel au parseur `exo-partition`.
///
/// `exo-partition` lit toujours des blocs de taille `block_size()` (header GPT,
/// puis chaque bloc de la table d'entrées) — exactement ce que `read_block`
/// attend (`buf.len() == block_size`).
struct ArcDiskReader {
    disk: Arc<dyn BlockDevice>,
    block_size: usize,
    num_blocks: u64,
}

impl BlockReader for ArcDiskReader {
    fn block_size(&self) -> usize {
        self.block_size
    }
    fn num_blocks(&self) -> u64 {
        self.num_blocks
    }
    fn read_lba(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), PartError> {
        self.disk.read_block(lba, buf).map_err(|_| PartError::Io)
    }
}

/// Périphérique bloc « vue partition » : décale chaque LBA de `base_lba` et
/// borne la capacité à `sectors`. Transparent pour tout le reste d'ExoFS.
pub struct PartitionOffsetDevice {
    inner: Arc<dyn BlockDevice>,
    base_lba: u64,
    sectors: u64,
}

impl PartitionOffsetDevice {
    pub fn new(inner: Arc<dyn BlockDevice>, base_lba: u64, sectors: u64) -> Self {
        Self {
            inner,
            base_lba,
            sectors,
        }
    }

    #[inline]
    fn map_lba(&self, lba: u64) -> ExofsResult<u64> {
        // Borne stricte : on refuse toute I/O au-delà de la partition (évite de
        // lire/écrire dans une partition voisine si une couche supérieure se
        // trompe de LBA).
        if lba >= self.sectors {
            return Err(crate::fs::exofs::core::ExofsError::IoError);
        }
        self.base_lba
            .checked_add(lba)
            .ok_or(crate::fs::exofs::core::ExofsError::IoError)
    }
}

impl BlockDevice for PartitionOffsetDevice {
    fn read_block(&self, lba: u64, buf: &mut [u8]) -> ExofsResult<()> {
        let phys = self.map_lba(lba)?;
        self.inner.read_block(phys, buf)
    }

    fn write_block(&self, lba: u64, buf: &[u8]) -> ExofsResult<()> {
        let phys = self.map_lba(lba)?;
        self.inner.write_block(phys, buf)
    }

    fn block_size(&self) -> u32 {
        self.inner.block_size()
    }

    fn total_blocks(&self) -> u64 {
        // Capacité = taille de la partition, PAS du disque entier.
        self.sectors
    }

    fn flush(&self) -> ExofsResult<()> {
        self.inner.flush()
    }
}

/// Scanne la table de partitions du disque et retourne la partition ExoFS ROOT
/// (par type-GUID) si — et seulement si — un GPT valide la contient.
///
/// Fonction **pure** (n'utilise pas l'état global) → testable unitairement.
/// Retourne `None` pour : disque brut, MBR legacy, GPT sans ROOT, ou toute
/// erreur de parsing (CRC/signature/garde anti-OOM).
pub fn scan_root(disk: &Arc<dyn BlockDevice>) -> Option<ResolvedPartition> {
    let block_size = disk.block_size() as usize;
    if block_size < 512 {
        return None;
    }
    let num_blocks = disk.total_blocks();
    let mut reader = ArcDiskReader {
        disk: Arc::clone(disk),
        block_size,
        num_blocks,
    };

    // Toute erreur de scan → on retombe sur le comportement LBA 0.
    let table = exo_partition::scan(&mut reader).ok()?;

    // On n'active le décalage QUE pour un vrai GPT (le MBR legacy n'est pas
    // réinterprété, pour ne jamais altérer un disque partitionné autrement).
    if table.scheme != Scheme::Gpt {
        return None;
    }

    let root = table.exofs_root()?;
    if root.start_lba == 0 || root.sectors == 0 {
        return None;
    }
    Some(ResolvedPartition {
        base_lba: root.start_lba,
        sectors: root.sectors,
    })
}

/// Résout la partition ExoFS ROOT sur le **disque global** et, si trouvée,
/// enveloppe le périphérique global dans un [`PartitionOffsetDevice`] pour que
/// toute l'I/O ExoFS soit décalée vers le début de la partition.
///
/// À appeler **une fois** au montage (après `init_global_disk`, avant le boot
/// recovery). Idempotence assurée par le fait qu'après enveloppement le disque
/// global n'est plus un GPT à LBA 0 (l'offset device borne déjà la vue).
///
/// - `Some(rp)` : partition ExoFS ROOT localisée, décalage installé.
/// - `None`     : pas de GPT / pas de partition ROOT / pas de disque → LBA 0.
pub fn resolve_exofs_partition() -> Option<ResolvedPartition> {
    let disk = virtio_adapter::current_global_disk()?;
    let rp = scan_root(&disk)?;
    let wrapper = Arc::new(PartitionOffsetDevice::new(disk, rp.base_lba, rp.sectors));
    virtio_adapter::replace_global_disk(wrapper);
    Some(rp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;
    use core::sync::atomic::{AtomicU64, Ordering};
    use exo_partition::crc32::crc32;
    use exo_partition::gpt::GPT_SIGNATURE;
    use exo_partition::guid::{Guid, GUID_ESP, GUID_EXOOS_ROOT};
    use exo_partition::mbr::{MBR_SIGNATURE, MBR_TYPE_GPT_PROTECTIVE};
    use spin::Mutex;

    const BS: usize = 512;

    /// Disque brut en mémoire implémentant le `BlockDevice` kernel.
    struct MemDisk {
        data: Mutex<Vec<u8>>,
        reads: AtomicU64,
    }
    impl MemDisk {
        fn new(blocks: usize) -> Self {
            Self {
                data: Mutex::new(vec![0u8; blocks * BS]),
                reads: AtomicU64::new(0),
            }
        }
        fn put(&self, lba: u64, off: usize, bytes: &[u8]) {
            let mut d = self.data.lock();
            let base = lba as usize * BS + off;
            d[base..base + bytes.len()].copy_from_slice(bytes);
        }
    }
    impl BlockDevice for MemDisk {
        fn read_block(&self, lba: u64, buf: &mut [u8]) -> ExofsResult<()> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            let d = self.data.lock();
            let base = lba as usize * BS;
            if base + buf.len() > d.len() {
                return Err(crate::fs::exofs::core::ExofsError::IoError);
            }
            buf.copy_from_slice(&d[base..base + buf.len()]);
            Ok(())
        }
        fn write_block(&self, lba: u64, buf: &[u8]) -> ExofsResult<()> {
            let mut d = self.data.lock();
            let base = lba as usize * BS;
            if base + buf.len() > d.len() {
                return Err(crate::fs::exofs::core::ExofsError::IoError);
            }
            d[base..base + buf.len()].copy_from_slice(buf);
            Ok(())
        }
        fn block_size(&self) -> u32 {
            BS as u32
        }
        fn total_blocks(&self) -> u64 {
            (self.data.lock().len() / BS) as u64
        }
        fn flush(&self) -> ExofsResult<()> {
            Ok(())
        }
    }

    /// Écrit un GPT valide (MBR protecteur + header + table) avec une partition
    /// ESP et une partition ExoFS ROOT à `root_start`/`root_sectors`.
    fn write_gpt(disk: &MemDisk, root_start: u64, root_sectors: u64) {
        // MBR protecteur (LBA 0).
        disk.put(0, 446 + 4, &[MBR_TYPE_GPT_PROTECTIVE]);
        disk.put(0, 446 + 12, &0xFFFF_FFFFu32.to_le_bytes());
        disk.put(0, 510, &MBR_SIGNATURE.to_le_bytes());

        // Table (LBA 2), 4 entrées de 128 o.
        let entry_size = 128usize;
        let num_entries = 4u32;
        let mut table = vec![0u8; entry_size * num_entries as usize];
        let mut mk = |slot: usize, ty: &Guid, start: u64, end: u64| {
            let o = slot * entry_size;
            table[o..o + 16].copy_from_slice(&ty.0);
            table[o + 16..o + 32].copy_from_slice(&[(slot as u8) + 1; 16]);
            table[o + 32..o + 40].copy_from_slice(&start.to_le_bytes());
            table[o + 40..o + 48].copy_from_slice(&end.to_le_bytes());
        };
        mk(0, &GUID_ESP, 2048, 2048 + 1024 - 1);
        mk(
            1,
            &GUID_EXOOS_ROOT,
            root_start,
            root_start + root_sectors - 1,
        );
        let table_crc = crc32(&table);

        // Header (LBA 1).
        let mut hdr = vec![0u8; 92];
        hdr[0..8].copy_from_slice(GPT_SIGNATURE);
        hdr[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        hdr[12..16].copy_from_slice(&92u32.to_le_bytes());
        hdr[24..32].copy_from_slice(&1u64.to_le_bytes());
        hdr[32..40].copy_from_slice(&(disk.total_blocks() - 1).to_le_bytes());
        hdr[40..48].copy_from_slice(&34u64.to_le_bytes());
        hdr[48..56].copy_from_slice(&(disk.total_blocks() - 34).to_le_bytes());
        hdr[56..72].copy_from_slice(&[0xCD; 16]);
        hdr[72..80].copy_from_slice(&2u64.to_le_bytes());
        hdr[80..84].copy_from_slice(&num_entries.to_le_bytes());
        hdr[84..88].copy_from_slice(&(entry_size as u32).to_le_bytes());
        hdr[88..92].copy_from_slice(&table_crc.to_le_bytes());
        let hcrc = crc32(&hdr[..92]);
        hdr[16..20].copy_from_slice(&hcrc.to_le_bytes());

        disk.put(1, 0, &hdr);
        disk.put(2, 0, &table);
    }

    #[test]
    fn scan_root_finds_exofs_partition_on_gpt() {
        let disk = MemDisk::new(4096);
        write_gpt(&disk, 4096, 2048);
        let arc: Arc<dyn BlockDevice> = Arc::new(disk);
        let rp = scan_root(&arc).expect("partition ExoFS ROOT trouvée");
        assert_eq!(rp.base_lba, 4096);
        assert_eq!(rp.sectors, 2048);
    }

    #[test]
    fn scan_root_none_on_raw_disk() {
        // Disque brut (pas de signature MBR) → pas de table → base 0 conservée.
        let disk = MemDisk::new(256);
        let arc: Arc<dyn BlockDevice> = Arc::new(disk);
        assert_eq!(scan_root(&arc), None);
    }

    #[test]
    fn scan_root_none_on_legacy_mbr() {
        // MBR legacy (pas de GPT protecteur) → on n'interprète pas → base 0.
        let disk = MemDisk::new(256);
        disk.put(0, 446, &[0x80]);
        disk.put(0, 446 + 4, &[0x83]); // type Linux, pas 0xEE
        disk.put(0, 446 + 8, &2048u32.to_le_bytes());
        disk.put(0, 446 + 12, &4096u32.to_le_bytes());
        disk.put(0, 510, &MBR_SIGNATURE.to_le_bytes());
        let arc: Arc<dyn BlockDevice> = Arc::new(disk);
        assert_eq!(scan_root(&arc), None);
    }

    #[test]
    fn offset_device_translates_lba() {
        let disk = MemDisk::new(4096);
        // Marqueur au LBA physique 4096 (= LBA 0 de la partition).
        disk.put(4096, 0, b"EXOFS-SUPERBLOCK-AT-PARTITION-START");
        let inner: Arc<dyn BlockDevice> = Arc::new(disk);
        let part = PartitionOffsetDevice::new(Arc::clone(&inner), 4096, 2048);

        // Lecture du LBA 0 de la partition → doit lire le LBA physique 4096.
        let mut buf = vec![0u8; BS];
        part.read_block(0, &mut buf).unwrap();
        assert_eq!(&buf[..35], b"EXOFS-SUPERBLOCK-AT-PARTITION-START");

        // Capacité bornée à la partition.
        assert_eq!(part.total_blocks(), 2048);
        assert_eq!(part.block_size(), BS as u32);

        // I/O au-delà de la partition refusée.
        assert!(part.read_block(2048, &mut buf).is_err());
    }

    #[test]
    fn offset_device_write_roundtrip() {
        let disk = MemDisk::new(4096);
        let inner: Arc<dyn BlockDevice> = Arc::new(disk);
        let part = PartitionOffsetDevice::new(Arc::clone(&inner), 100, 64);

        let payload = {
            let mut v = vec![0u8; BS];
            v[..4].copy_from_slice(b"DATA");
            v
        };
        part.write_block(5, &payload).unwrap();

        // Relecture via le device physique au LBA 105.
        let mut buf = vec![0u8; BS];
        inner.read_block(105, &mut buf).unwrap();
        assert_eq!(&buf[..4], b"DATA");
    }
}
