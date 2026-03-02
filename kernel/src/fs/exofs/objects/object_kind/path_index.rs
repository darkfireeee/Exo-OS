// kernel/src/fs/exofs/objects/object_kind/path_index.rs
//
// Objets PathIndex — page d'index de chemins (nœuds de l'arbre de chemins).
// Utilisé par path/path_index.rs.

/// Taille d'une page PathIndex (toujours 4096 octets pour lisibilité disque).
pub const PATH_INDEX_PAGE_SIZE: usize = 4096;

/// Magic d'une page PathIndex : "PIDX".
pub const PATH_INDEX_MAGIC: u32 = 0x50494458;
