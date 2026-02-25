// drivers/fs/src/fat32/compat.rs
//
// FAT32 — Vérification compatibilité  (exo-os-driver-fs)
// RÈGLE FS-FAT32-01 : Valider BPB + cluster_count.
// RÈGLE FS-FAT32-03 : noexec forcé — jamais exécuter depuis FAT32.

use crate::FsDriverError;
use super::bpb::ParsedBpb;

/// Options de montage FAT32.
/// RÈGLE FS-FAT32-03 : `noexec` est toujours `true`, jamais débrayable.
#[derive(Clone, Debug)]
pub struct Fat32MountOptions {
    pub read_only: bool,
    /// TOUJOURS true — RÈGLE FS-FAT32-03 (MS_NOEXEC immuable).
    pub noexec:    bool,
}

impl Fat32MountOptions {
    pub fn default_secure() -> Self {
        Self { read_only: false, noexec: true }
    }
}

/// Vérifie qu'un volume FAT32 est montable.
pub fn fat32_verify(bpb: &ParsedBpb, opts: &mut Fat32MountOptions) -> Result<(), FsDriverError> {
    if bpb.cluster_count < 65525 {
        return Err(FsDriverError::WrongFsType);
    }
    if bpb.sec_per_cluster == 0 || bpb.num_fats == 0 {
        return Err(FsDriverError::InvalidParameter);
    }
    // RÈGLE FS-FAT32-03 : noexec immuable.
    opts.noexec = true;
    Ok(())
}
