//! disk/gpt.rs — Parsing GPT/MBR **réel** côté bootloader (chemin UEFI).
//!
//! Ce module comble le trou historique : avant, le bootloader n'avait AUCUN
//! parseur de table de partitions (le kernel supposait un LBA codé en dur). Il
//! réutilise désormais le **même** parseur que le kernel — la crate partagée
//! `exo-partition` (équivalent de `redox-os/drivers/storage/partitionlib`) — via
//! un adaptateur sur `EFI_BLOCK_IO_PROTOCOL`. Une seule source de vérité GPT/MBR
//! pour tout Exo-OS (bootloader + kernel), donc zéro divergence possible.
//!
//! ## Usage
//! Le chemin UEFI charge le kernel par le protocole fichier FAT de l'ESP (le
//! firmware gère déjà le partitionnement pour CE besoin). Ce module sert à
//! **localiser les partitions ExoFS (ROOT/DATA) par leur type-GUID** afin de
//! transmettre leurs LBA au kernel (BootInfo) — le kernel n'a alors plus à
//! deviner où se trouve son volume. Additif : si rien n'est trouvé, le boot
//! continue normalement (le kernel re-scanne lui-même via `partition_scan`).
//!
//! ## Sécurité
//! Lecture seule (`GetProtocol`, jamais exclusif → pas de conflit avec le driver
//! FAT du firmware). Toute erreur (pas de Block I/O, CRC invalide, pas d'ExoFS)
//! est non fatale et traitée comme « disque non reconnu ».

use uefi::prelude::*;
use uefi::proto::media::block::BlockIO;
use uefi::table::boot::{OpenProtocolAttributes, OpenProtocolParams, SearchType};
use uefi::Identify; // apporte `BlockIO::GUID` (trait Identify) dans le scope

// Re-export du parseur partagé : le bootloader et le kernel manipulent EXACTEMENT
// les mêmes structures GPT/MBR (Guid, PartitionType, scan, …).
pub use exo_partition::guid::{PartitionType, GUID_ESP, GUID_EXOOS_DATA, GUID_EXOOS_ROOT};
pub use exo_partition::{BlockReader, PartError, Partition, PartitionTable, Scheme};

/// Nombre maximal de handles Block I/O qu'on inspecte (garde-fou).
const MAX_BLOCK_HANDLES: usize = 64;

/// Adaptateur : expose un `EFI_BLOCK_IO_PROTOCOL` au parseur `exo-partition`.
pub struct BlockIoReader<'a> {
    io: &'a BlockIO,
    media_id: u32,
    block_size: usize,
    num_blocks: u64,
}

impl<'a> BlockIoReader<'a> {
    /// Construit l'adaptateur à partir d'un protocole Block I/O ouvert.
    pub fn new(io: &'a BlockIO) -> Self {
        let media = io.media();
        Self {
            media_id: media.media_id(),
            block_size: media.block_size() as usize,
            // `last_block` est l'index du dernier LBA → +1 pour le nombre de blocs.
            num_blocks: media.last_block().saturating_add(1),
            io,
        }
    }
}

impl<'a> BlockReader for BlockIoReader<'a> {
    fn block_size(&self) -> usize {
        self.block_size
    }
    fn num_blocks(&self) -> u64 {
        self.num_blocks
    }
    fn read_lba(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), PartError> {
        self.io
            .read_blocks(self.media_id, lba, buf)
            .map_err(|_| PartError::Io)
    }
}

/// Disposition ExoOS découverte sur le disque de boot (LBA absolus disque).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiscoveredLayout {
    pub esp_lba: Option<u64>,
    pub root_lba: Option<u64>,
    pub root_sectors: u64,
    pub data_lba: Option<u64>,
    pub data_sectors: u64,
    pub block_size: u32,
}

impl DiscoveredLayout {
    fn from_table(table: &PartitionTable, block_size: u32) -> Self {
        let mut out = DiscoveredLayout {
            block_size,
            ..Default::default()
        };
        if let Some(esp) = table.esp() {
            out.esp_lba = Some(esp.start_lba);
        }
        if let Some(root) = table.exofs_root() {
            out.root_lba = Some(root.start_lba);
            out.root_sectors = root.sectors;
        }
        if let Some(data) = table.exofs_data() {
            out.data_lba = Some(data.start_lba);
            out.data_sectors = data.sectors;
        }
        out
    }

    /// Vrai si une partition ExoFS ROOT a été localisée.
    pub fn has_exofs_root(&self) -> bool {
        self.root_lba.is_some()
    }
}

/// Scanne les disques physiques (Block I/O non-partition) et retourne la
/// disposition ExoOS du **premier disque GPT contenant une partition ExoFS
/// ROOT**. `None` si aucun (le boot continue alors normalement).
///
/// Lecture seule, non exclusive → ne perturbe pas le driver FAT du firmware.
pub fn scan_boot_disk(bt: &BootServices, image_handle: Handle) -> Option<DiscoveredLayout> {
    let handles = bt
        .locate_handle_buffer(SearchType::ByProtocol(&BlockIO::GUID))
        .ok()?;

    for (i, &handle) in handles.iter().enumerate() {
        if i >= MAX_BLOCK_HANDLES {
            break;
        }
        // Ouverture NON exclusive (GetProtocol) : inspection en lecture seule.
        // SAFETY : on n'installe rien, on ne garde pas le protocole au-delà du
        // scope, agent = image_handle (notre propre image).
        let params = OpenProtocolParams {
            handle,
            agent: image_handle,
            controller: None,
        };
        let bio = match unsafe { bt.open_protocol::<BlockIO>(params, OpenProtocolAttributes::GetProtocol) }
        {
            Ok(b) => b,
            Err(_) => continue,
        };

        let media = bio.media();
        // On ne scanne QUE les disques physiques entiers — pas les partitions
        // logiques (où le GPT du disque n'est pas visible) ni les médias absents.
        if media.is_logical_partition() || !media.is_media_present() {
            continue;
        }
        let block_size = media.block_size();

        let mut reader = BlockIoReader::new(&bio);
        let table = match exo_partition::scan(&mut reader) {
            Ok(t) => t,
            Err(_) => continue,
        };
        if table.scheme != Scheme::Gpt {
            continue;
        }
        let layout = DiscoveredLayout::from_table(&table, block_size);
        if layout.has_exofs_root() {
            return Some(layout);
        }
    }
    None
}
