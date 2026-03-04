//! Tests d'intégration ExoFS — parcours complet write → commit → read.
//!
//! Ces tests vérifient les invariants de bout en bout sans disque réel
//! (simulation mémoire).

// Placeholder — tests d'intégration à compléter avec un backend mémoire.
// Structure prévue :
//   - test_epoch_commit_roundtrip : écriture + commit + relecture
//   - test_blob_dedup_pipeline    : réutilisation BlobId identique
//   - test_recovery_after_crash   : recovery depuis slot A/B/C
