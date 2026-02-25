// drivers/fs/src/fat32/cluster.rs — Cluster helpers  (exo-os-driver-fs)

use super::bpb::ParsedBpb;

/// Retourne le premier secteur d'un cluster.
#[inline]
pub fn cluster_to_sector(cluster: u32, bpb: &ParsedBpb) -> u64 {
    bpb.cluster_to_sector(cluster)
}

/// Vérifie que le numéro de cluster est dans la plage valide.
#[inline]
pub fn cluster_is_valid(cluster: u32, bpb: &ParsedBpb) -> bool {
    cluster >= 2 && cluster < bpb.cluster_count + 2
}
