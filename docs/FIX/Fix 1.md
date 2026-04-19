Les fixes de docs/FIX/1 ont été intégrés sur les chemins actifs du dépôt, avec adaptation prudente quand le code réel divergeait de l’audit.

Scheduler/SMP : unification MAX_CPUS et garde explicite dans stolen_time.rs et reclaim.rs, publication cohérente du TCB courant dans switch.rs, assertions de préemption actives en release dans preempt.rs, et affinité 256 bits dans task.rs + affinity.rs sans casser le TCB 256B.
Sécurité kernel : #CP délègue maintenant à ExoCage dans exceptions.rs, ExoSeal verrouille la NIC IOMMU en premier dans exoseal.rs, ExoKairos a été durci dans exokairos.rs, ExoLedger sérialise la chaîne P0 dans exoledger.rs, et CAP-05 a été uniformisé dans verify.rs.
IPC/ExoCordon : j’ai ajouté la politique kernel dans ipc_policy.rs et le contrôle dans table.rs, puis réaligné le router sur la topologie Ring1 réelle du repo dans exocordon.rs. J’ai aussi remplacé le stub AEAD kernel par une implémentation pure Rust XChaCha20+BLAKE3 dans xchacha20_poly1305.rs.
Validation

Test standard WSL : cargo check --workspace --message-format short passe.
Stress test WSL : cargo test --manifest-path servers/phase5-tests/Cargo.toml -- --nocapture passe, 62/62 tests verts, dont les cas ExoCordon mis à jour dans lib.rs.
Point de vérité

Le dépôt est vert sur cette passe, mais il reste des reliquats hors docs/FIX/1, non bloquants ici : warnings de constantes inutilisées dans exocage.rs et exoveil.rs, plus quelques stubs plus anciens hors périmètre immédiat comme l’allocation réelle de shadow stack dans exocage.rs.


Parfait — c’est finalisé strictement ✅

Corrections appliquées (strictes)
BUG #1 Hotplug >64 CPUs (hotplug.rs)

Déjà conforme : masque online multi-mots ([AtomicU64; ONLINE_MASK_WORDS], soit 4 mots pour 256 CPUs).
Accès bit/word corrects pour cpu_is_online, set_cpu_online, set_cpu_offline.
BUG #2 Ordering cpu_id (task.rs, migration.rs)

task.rs :
current_cpu() : load(Ordering::Acquire)
assign_cpu() : store(Ordering::Release)
migration.rs : déjà conforme
lecture cpu_id en Acquire
publication cpu_id en Release
BUG #3 vruntime en Relaxed pour comparaisons (runqueue.rs)

Déjà conforme : comparaisons de priorité en Ordering::Acquire dans insert_sorted().
Le Relaxed restant sur min_vruntime est documenté comme approximation CFS (non décision critique).
Commit/push effectué
Le correctif restant (BUG #2 dans task.rs) a été commité et poussé sur main :

commit: bc54d1d6
message: scheduler: strengthen cpu_id ordering to Acquire/Release
Validation build (WSL)
Compilation revalidée en WSL :

cargo check --target x86_64-unknown-none
EXIT: 0
seulement des warnings dead_code préexistants (pas d’erreur).
Checklist finale
 Relire fichiers SMP cibles
 Appliquer correctifs hotplug
 Appliquer ordering cpu_id task/migration
 Appliquer ordering vruntime runqueue
 Compiler en WSL et valider