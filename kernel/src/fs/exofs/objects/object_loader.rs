// kernel/src/fs/exofs/objects/object_loader.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ObjectLoader — lecture et reconstruction d'un LogicalObject depuis disque
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE HDR-03 : ObjectHeader.verify() AVANT toute utilisation des données.
// RÈGLE SECURITY-01 : le caller doit vérifier ses droits AVANT d'appeler.

use core::sync::atomic::AtomicU32;
use alloc::sync::Arc;

use crate::fs::exofs::core::{ExofsError, ExofsResult, DiskOffset};
use crate::fs::exofs::storage::object_reader::read_object;
use crate::fs::exofs::objects::logical_object::{LogicalObject, LogicalObjectDisk};
use crate::fs::exofs::objects::object_meta::ObjectMeta;
use crate::fs::exofs::objects::inline_data::InlineData;
use crate::fs::exofs::core::object_kind::ObjectKind;
use crate::fs::exofs::core::object_class::ObjectClass;
use crate::fs::exofs::core::flags::ObjectFlags;
use crate::fs::exofs::core::stats::EXOFS_STATS;
use crate::scheduler::sync::rwlock::RwLock;

// ─────────────────────────────────────────────────────────────────────────────
// Chargement d'un objet depuis disque
// ─────────────────────────────────────────────────────────────────────────────

/// Charge un LogicalObject depuis son offset disque.
///
/// # Protocole
/// 1. Lire ObjectHeader (128B) + LogicalObjectDisk (256B) via read_fn.
/// 2. Vérifier ObjectHeader (magic + checksum) — RÈGLE HDR-03.
/// 3. Reconstruire LogicalObject in-memory.
pub fn load_object(
    disk_offset:    DiskOffset,
    verify_content: bool,
    read_fn:        &dyn Fn(DiskOffset, &mut [u8]) -> ExofsResult<usize>,
) -> ExofsResult<Arc<RwLock<LogicalObject>>> {
    use core::mem::size_of;

    let result = read_object(disk_offset, verify_content, read_fn)?;

    // Les 256 premiers octets du payload = LogicalObjectDisk.
    let disk_needed = size_of::<LogicalObjectDisk>();
    if result.data.len() < disk_needed {
        return Err(ExofsError::CorruptedStructure);
    }

    // SAFETY: LogicalObjectDisk est #[repr(C, packed)], taille 256.
    // Les données ont été lues et vérifiées par read_object (checksum ObjectHeader).
    let lod: LogicalObjectDisk = unsafe {
        core::ptr::read_unaligned(result.data.as_ptr() as *const LogicalObjectDisk)
    };

    let kind = ObjectKind::from_u8({ lod.kind })
        .ok_or(ExofsError::InvalidObjectKind)?;
    let class = match { lod.class } {
        1 => ObjectClass::Class1,
        2 => ObjectClass::Class2,
        _ => return Err(ExofsError::InvalidObjectClass),
    };

    let flags   = ObjectFlags({ lod.flags });
    let is_inline = flags.contains(ObjectFlags::INLINE_DATA);

    // Reconstruction des données inline si applicable.
    let inline_data = if is_inline && result.data.len() > disk_needed {
        let inline_bytes = &result.data[disk_needed..];
        Some(InlineData::from_slice(inline_bytes)?)
    } else {
        None
    };

    let obj = LogicalObject {
        object_id:    crate::fs::exofs::core::ObjectId({ lod.object_id }),
        blob_id:      crate::fs::exofs::core::BlobId({ lod.blob_id }),
        epoch_create: crate::fs::exofs::core::EpochId({ lod.epoch_create }),
        epoch_modify: crate::fs::exofs::core::EpochId({ lod.epoch_modify }),
        disk_offset:  crate::fs::exofs::core::DiskOffset({ lod.blob_offset }),
        data_size:    { lod.data_size },
        flags,
        kind,
        class,
        meta:         ObjectMeta::from_disk(&lod),
        inline_data,
        ref_count:    AtomicU32::new({ lod.ref_count }),
    };

    EXOFS_STATS.inc_objects_read();

    Ok(Arc::new(RwLock::new(obj)))
}
