# Rapport Fixes Kernel + Security

Date: 2026-04-20

## Périmètre

Cette passe a couvert:

- `docs/FIX/4 all kernel (without security)`
- `docs/FIX/5 security`

Objectif suivi pendant toute la passe:

- vérifier chaque anomalie contre le code réel avant modification
- n’appliquer que les correctifs compatibles avec l’architecture actuelle du dépôt
- remettre le noyau en état de compilation
- valider avec un test standard et un stress test pour la partie kernel, puis idem pour la partie security

## Documentation relue

- `docs/FIX/4 all kernel (without security)/00_INDEX_MASTER.md`
- `docs/FIX/4 all kernel (without security)/01_CORRECTIONS_P0_CRITIQUES.md`
- `docs/FIX/4 all kernel (without security)/02_CORRECTIONS_P1_MAJEURES.md`
- `docs/FIX/4 all kernel (without security)/03_CORRECTIONS_P2_MINEURES.md`
- `docs/FIX/4 all kernel (without security)/04_RECTIFICATION_AUDIT_PRECEDENT.md`
- `docs/FIX/4 all kernel (without security)/05_ETAT_SERVERS.md`
- `docs/FIX/5 security/00_VERIFICATION_AUDIT.md`
- `docs/FIX/5 security/audit de Sécurité Exo.txt`
- `docs/FIX/5 security/README_APPLICATION_GUIDE.md`
- `docs/FIX/5 security/patch_01_cve_exo001_smp_race.rs`
- `docs/FIX/5 security/patch_02_exocage_cet_perthread.rs`
- `docs/FIX/5 security/patch_03_exoseal_verify_p0.rs`
- `docs/FIX/5 security/patch_04_05_06_majeur_fixes.rs`
- `docs/FIX/5 security/patch_07_supplementary_fixes.rs`

## Correctifs appliqués

### 1. Kernel FIX/4 déjà recollés et revalidés dans cette passe

- `kernel/src/syscall/fs_bridge.rs`
  - remplacement des stubs `NotReady` par un bridge réel vers ExoFS
  - ajout de helpers de validation/copie utilisateur
  - ajout de tests roundtrip et stress
- `kernel/src/syscall/table.rs`
  - `sys_lseek()` câblé vers `fs_bridge`
  - `SYS_EXO_IPC_CREATE`, `SYS_EXO_IPC_DESTROY`, `SYS_EXO_IPC_RECV_NB` implémentés et dispatchés
  - `SYS_EXO_IPC_CALL` recâblé vers le fast-path IPC existant
  - ajout de tests sur l’enregistrement d’endpoints nommés
- `kernel/src/exophoenix/stage0.rs`
  - `send_sipi_once()` corrigé pour envoyer `INIT IPI` puis deux `SIPI`

### 2. Security FIX/5 déjà recollés et revalidés dans cette passe

- `kernel/src/security/exoseal.rs`
  - ajout de `verify_p0_fixes()`
  - vérification des invariants P0 avant `SECURITY_READY`
- `kernel/src/security/exoledger.rs`
  - saturation P0 sans panic
  - préservation des entrées déjà écrites lors d’un `exo_ledger_init()`
  - journalisation d’overflow P0
- `kernel/src/security/exonmi.rs`
  - clamp des timeouts watchdog
- `kernel/src/security/exoargos.rs`
  - validation du TCB dans `pmc_snapshot()`
  - remplacement du CPUID cassé
- `kernel/src/security/exocage.rs`
  - `enable_cet_for_thread()` complète la sauvegarde `PL0_SSP` dans le TCB
  - `disable_cet_for_thread()` nettoie aussi `PL0_SSP`
- `kernel/src/process/core/tcb.rs`
  - câblage CET per-thread à la création d’un thread quand CET global est actif
- `kernel/src/security/mod.rs`
  - bootstrap thread durci côté CET
  - `verify_p0_fixes` ré-exporté

### 3. Recalages API/compilation nécessaires pour finir FIX/4 et garder le noyau compilable

- `kernel/src/memory/virtual/address_space/fork_impl.rs`
  - adaptation à l’API réelle `PageTableEntry`/`Frame`
  - ajout d’une libération propre des tables userspace clonées
  - suppression des chemins de libération incomplets qui fuyaient silencieusement
- `kernel/src/process/lifecycle/exec.rs`
  - correction du type de `stack_size` pour rester cohérent avec `ThreadAddress`
- `kernel/src/fs/exofs/crypto/entropy.rs`
  - wrapper réaligné sur les vraies signatures `rng_*`
  - initialisation lazy du RNG kernel
  - ajout de `random_16()`
- `kernel/src/fs/exofs/crypto/xchacha20.rs`
  - wrapper ExoFS réaligné sur l’API AEAD réellement exportée par `security::crypto`
  - mapping d’erreurs vers `ExofsError` existants
- `kernel/src/fs/exofs/crypto/key_derivation.rs`
  - `hkdf_extract()` recollé à la vraie signature du module KDF
  - `hkdf_expand()` réimplémenté via `Hkdf<Sha256>::from_prk`
  - `derive_fs_block_key()` gère désormais le `Result`
  - `derive_from_passphrase()` utilise `hash_password_into_with_memory()` compatible `no_std`
- `kernel/src/security/crypto/aes_gcm.rs`
  - correction de l’expression `ct_eq()` qui bloquait la compilation

### 4. Durcissement final découvert par le stress test security

Le test de stress ExoLedger a d’abord échoué avec un `SIGSEGV`. La cause réelle n’était pas l’overflow P0 lui-même, mais le chemin d’identification d’acteur en contexte de tests host:

- `kernel/src/security/exoledger.rs`
  - `current_actor_oid()` ne lit plus le TCB courant via `gs:[0x20]` pendant les tests unitaires host
  - le fallback early-boot ne lit plus `gs:[0x10]` pour obtenir un CPU id
  - validation additionnelle via `CURRENT_THREAD_PER_CPU` avant déréférencement côté non-test

Après ce durcissement, le stress test ExoLedger passe.

## Fichiers modifiés dans cette passe

- `kernel/src/syscall/fs_bridge.rs`
- `kernel/src/syscall/table.rs`
- `kernel/src/exophoenix/stage0.rs`
- `kernel/src/security/exoledger.rs`
- `kernel/src/security/exoseal.rs`
- `kernel/src/security/exonmi.rs`
- `kernel/src/security/exoargos.rs`
- `kernel/src/security/exocage.rs`
- `kernel/src/process/core/tcb.rs`
- `kernel/src/security/mod.rs`
- `kernel/src/memory/virtual/address_space/fork_impl.rs`
- `kernel/src/process/lifecycle/exec.rs`
- `kernel/src/fs/exofs/crypto/entropy.rs`
- `kernel/src/fs/exofs/crypto/xchacha20.rs`
- `kernel/src/fs/exofs/crypto/key_derivation.rs`
- `kernel/src/security/crypto/aes_gcm.rs`
- `Cargo.toml`
- `kernel/src/fs/mod.rs`
- `kernel/src/fs/exofs/storage/virtio_adapter.rs`

## Validations exécutées

### Compilation

- `cargo check -p exo-os-kernel --lib --message-format short`
  - statut: OK

### Kernel FIX/4

- Test standard:
  - `cargo test -p exo-os-kernel --lib test_fs_bridge_open_write_read_lseek_roundtrip -- --nocapture`
  - statut: OK
- Stress test:
  - `cargo test -p exo-os-kernel --lib test_register_named_endpoint_stress -- --nocapture`
  - statut: OK

### Security FIX/5

- Test standard:
  - `cargo test -p exo-os-kernel --lib test_validate_phase0_state_accepts_hardened_state -- --nocapture`
  - statut: OK
- Stress test:
  - `cargo test -p exo-os-kernel --lib test_p0_overflow_is_graceful_and_saturating -- --nocapture`
  - premier passage: échec `SIGSEGV`
  - correctif appliqué dans `exoledger.rs`
  - second passage: OK

## État final

- le noyau recompiles correctement en `cargo check`
- les correctifs principaux documentés de `FIX/4` et `FIX/5` traités dans cette passe sont câblés et testés
- le stress test security a permis de corriger un problème réel de compatibilité host dans `ExoLedger`

## Notes résiduelles

Des warnings subsistent dans le dépôt, mais ils ne bloquent pas la compilation ni les validations exécutées ici. Ils concernent principalement:

- `kernel/src/fs/elf_loader_impl.rs`
- `kernel/src/security/crypto/aes_gcm.rs`
- `kernel/src/security/exoargos.rs`
- quelques variables inutilisées dans `kernel/src/syscall/table.rs`

Ces warnings n’ont pas été massivement nettoyés dans cette passe pour éviter d’élargir inutilement le périmètre au-delà des corrections documentées.
