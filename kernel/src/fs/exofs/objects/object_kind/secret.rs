// kernel/src/fs/exofs/objects/object_kind/secret.rs
//
// Objets Secret — données chiffrées (KIND_SECRET, toujours ENCRYPTED flag).
//
// RÈGLE SEC-03 : BlobId d'un Secret calculé sur le texte CLAIR avant chiffrement.
// RÈGLE SEC-04 : contenu d'un Secret jamais loggué ni inclus dans les stats.

use crate::fs::exofs::core::flags::ObjectFlags;

/// Vrai si les flags d'un objet sont cohérents avec un Secret.
pub fn secret_flags_valid(flags: ObjectFlags) -> bool {
    flags.contains(ObjectFlags::ENCRYPTED)
}
