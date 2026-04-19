Les fixes de docs/FIX/1 ont été intégrés sur les chemins actifs du dépôt, avec adaptation prudente quand le code réel divergeait de l’audit.

Scheduler/SMP : unification MAX_CPUS et garde explicite dans stolen_time.rs et reclaim.rs, publication cohérente du TCB courant dans switch.rs, assertions de préemption actives en release dans preempt.rs, et affinité 256 bits dans task.rs + affinity.rs sans casser le TCB 256B.
Sécurité kernel : #CP délègue maintenant à ExoCage dans exceptions.rs, ExoSeal verrouille la NIC IOMMU en premier dans exoseal.rs, ExoKairos a été durci dans exokairos.rs, ExoLedger sérialise la chaîne P0 dans exoledger.rs, et CAP-05 a été uniformisé dans verify.rs.
IPC/ExoCordon : j’ai ajouté la politique kernel dans ipc_policy.rs et le contrôle dans table.rs, puis réaligné le router sur la topologie Ring1 réelle du repo dans exocordon.rs. J’ai aussi remplacé le stub AEAD kernel par une implémentation pure Rust XChaCha20+BLAKE3 dans xchacha20_poly1305.rs.
Validation

Test standard WSL : cargo check --workspace --message-format short passe.
Stress test WSL : cargo test --manifest-path servers/phase5-tests/Cargo.toml -- --nocapture passe, 62/62 tests verts, dont les cas ExoCordon mis à jour dans lib.rs.
Point de vérité

Le dépôt est vert sur cette passe, mais il reste des reliquats hors docs/FIX/1, non bloquants ici : warnings de constantes inutilisées dans exocage.rs et exoveil.rs, plus quelques stubs plus anciens hors périmètre immédiat comme l’allocation réelle de shadow stack dans exocage.rs.