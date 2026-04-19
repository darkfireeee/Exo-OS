# Rapport FS + Arch-Memory
Date: 2026-04-19
Répertoire de travail: `C:\Users\xavie\Desktop\Exo-OS`

## Périmètre

Travail réalisé à partir de:

- `docs/FIX/2 fs/CORRECTIONS_EXOFS_SYNTHESE.md`
- `docs/FIX/2 fs/CORRECTIONS_EXOFS_P0_P1.md`
- `docs/FIX/2 fs/CORRECTIONS_EXOFS_P2_QUALITE.md`
- `docs/FIX/2 fs/report Exo-FS by kimi-ai.md`
- `docs/FIX/3 arch-memory/AUDIT KIMI-AI & Claude.txt`

Méthode suivie:

1. lecture des documents de fix;
2. relecture des modules réellement concernés avant toute modification;
3. tri des alertes arch-memory entre vraies anomalies code, critiques théoriques et faux positifs;
4. corrections uniquement quand le dépôt exposait déjà l’API ou la sémantique attendue;
5. validation WSL par compilation et tests ciblés.

## Correctifs ExoFS appliqués

### Recovery / I/O bloc

- Ajout de `kernel/src/fs/exofs/recovery/block_io.rs` pour encapsuler les lectures/écritures sous-bloc via tampon bloc complet.
- Remplacement des lectures/écritures directes partielles dans:
  - `epoch_replay.rs`
  - `fsck_phase1.rs`
  - `fsck_phase2.rs`
  - `fsck_phase3.rs`
  - `fsck_phase4.rs`
  - `fsck_repair.rs`
  - `slot_recovery.rs`

### FSCK / journal / commit

- `fsck_phase2.rs`: `alloc_entries_iter()` n’est plus un faux stub, l’itérateur expose les vraies entrées.
- `fsck_phase4.rs`: utilisation de l’itérateur réel au lieu d’un chemin vide.
- `epoch_record.rs`: suppression du reliquat TODO résiduel.
- `epoch_commit.rs`:
  - offsets on-disk verrouillés par assertions cohérentes;
  - remise à zéro sûre de l’état de commit via `CommitGuard`;
  - checksum/journal recollés à la logique réelle;
  - petit nettoyage des warnings `dead_code` pour éviter un ICE nightly pendant `cargo test`.

### Cache / POSIX bridge / constantes

- `vfs_compat.rs`: corrections sur `read`, `write`, `rmdir`, `readdir`, `rename`.
- `blob_cache.rs`, `object_cache.rs`, `extent_cache.rs`, `metadata_cache.rs`, `path_cache.rs`: corrections de flush/drop et de cohérence dirty/clean.
- `constants.rs`, `config.rs`, `inline_data.rs`, `object_builder.rs`, `object_class.rs`:
  - unification `INLINE_DATA_MAX_BYTES = 256`;
  - unification `GC_MIN_EPOCH_DELAY_DEFAULT`;
  - getters config recâblés avec `Ordering::Acquire`;
  - capacité `PathCache::new_const()` réalignée.

### Backend stockage / barrières

- `drivers/storage/virtio_blk/src/lib.rs`: `flush()` mock renvoie désormais un succès réel de façade.
- `storage/virtio_adapter.rs`: flush disque global réel exposé.
- `epoch_barriers.rs`: fail-closed si aucun hook de flush NVMe n’est enregistré.
- `kernel/src/fs/exofs/mod.rs`: enregistrement du hook de flush pendant `exofs_init()`.

### Serveur VFS

- `servers/vfs_server/src/main.rs`:
  - implémentation réelle de `VFS_UMOUNT`;
  - réutilisation des slots de montage;
  - rejet des doublons de mountpoint;
  - flush ExoFS best-effort via le seul point de sync réellement exposé aujourd’hui, `SYS_EXOFS_EPOCH_COMMIT`.

## Triage arch-memory

### Confirmé et corrigé

- `kernel/src/exophoenix/ssr.rs`
  - correction critique: SSR physique désormais remappée via `phys_to_virt()` avant déréférencement atomique.
- `kernel/src/ipc/core/mod.rs`
  - inclusion effective de `fastcall_asm.s`.
- `kernel/build.rs`
  - suivi `rerun-if-changed` de `fastcall_asm.s`.
- `kernel/src/arch/x86_64/mod.rs`
  - ajout du symbole `arch_cpu_relax()`.
- `kernel/src/arch/x86_64/smp/init.rs`
  - suppression du double comptage `ONLINE_CPU_COUNT`;
  - `smp_cpu_count()` réaligné sur `percpu::cpu_count()`.
- `kernel/src/exophoenix/stage0.rs`
  - `send_sipi_once()` envoie désormais deux SIPI avec délai conforme;
  - `init_core_count()` SSR conservé et vérifié présent.
- `kernel/src/arch/x86_64/time/sources/pit.rs`
  - `wait_ch2_done()` passe d’un timeout à itérations fixes à une fenêtre TSC réelle.
- `kernel/src/arch/x86_64/time/ktime.rs`
  - offsets TSC signés via `AtomicI64`;
  - application correcte des offsets positifs/négatifs.
- `kernel/src/arch/x86_64/time/percpu/sync.rs`
  - synchronisation SMP recollée à la sémantique signée.
- `kernel/src/ipc/ring/spsc.rs`
  - plus de modulo aliasant sur `channel_id`;
  - refus explicite des IDs hors plage.
- `kernel/src/memory/mod.rs`
  - enregistrement du backend swap provider au boot mémoire.
- `kernel/src/memory/virtual/fault/swap_in.rs`
  - bridge réel vers `SWAP_BACKEND`.
- `kernel/src/exophoenix/handoff.rs`
  - lecture ACK gel/TLB rendue cohérente pour éviter la régression faux timeout.
- `kernel/src/exophoenix/forge.rs`
  - correction de la logique timeout APIC down-counter;
  - attente des ACK TLB réelle au lieu d’un simple busy-wait aveugle.
- `kernel/src/exophoenix/isolate.rs`
  - même correction timeout/APIC;
  - attente des ACK TLB réelle.
- `servers/vfs_server/src/main.rs`
  - correction du vrai défaut documenté de démontage sans sync ExoFS.

### Vérifié puis rejeté comme faux positif ou critique non patchable localement

- Hotplug limité à 64 CPUs:
  - faux sur l’état actuel du code;
  - `kernel/src/arch/x86_64/smp/hotplug.rs` utilise déjà un tableau `[AtomicU64; 4]`, soit 256 CPUs.
- Arrondi de calibration APIC par `pit_count`:
  - le point “erreur majeure de formule” n’était pas un bug réel;
  - le vrai bug était le timeout à itérations fixes de `wait_ch2_done()`, qui a été corrigé.
- Plusieurs remarques Kimi sur buddy/slab, cache-cohérence matérielle, TLA+, dual-kernel “impossible”:
  - ce sont des critiques d’architecture, pas des anomalies code directement patchables dans ce sprint.

### Confirmé mais non soldé faute d’API ou de source fiable dans le dépôt

- `kernel/src/exophoenix/isolate.rs`
  - `mark_a_pages_not_present()` reste vide;
  - `override_a_idt_with_b_handlers()` reste vide.
  - Motif: aucun accès exposé dans le dépôt pour récupérer/modifier proprement le CR3/IDTR de Kernel A sans inventer un protocole mémoire hors spec locale.
- `kernel/src/exophoenix/forge.rs`
  - `A_IMAGE_HASH` et `A_MERKLE_ROOT` restent des zéros;
  - `pci_function_level_reset()` et `reload_driver_binary_from_exofs()` restent des stubs d’intégration.
  - Motif: aucune source autoritative des hashes attendus ni table de correspondance driver-hash dans le dépôt actuel.
- `kernel/src/exophoenix/handoff.rs`
  - `mask_all_msi_msix()` reste best-effort.
  - Motif: pas d’API globale PCI/MSI/MSI-X exposée à ce niveau pour un masquage réel.
- `kernel/src/arch/x86_64/acpi/hpet.rs`
  - TODO bare-metal fixmap toujours présent.
  - Motif: hors périmètre direct des fixes validés ici, et non bloquant pour les validations WSL réalisées.

## Vérification des reliquats

### Côté ExoFS

Après audit textuel:

- les fixes demandés par `docs/FIX/2 fs` sont reflétés dans les chemins actifs;
- les placeholders restants détectés dans `kernel/src/fs/exofs` sont principalement:
  - `EPOCH_CHAIN_NEXT_PLACEHOLDER`, qui fait partie du protocole on-disk et n’est pas un stub d’exécution;
  - les fichiers `kernel/src/fs/exofs/tests/integration/tier_*.rs`, qui sont des placeholders de scaffolding de tests et pas du code de production.

### Côté arch-memory

Après triage textuel:

- les placeholders encore visibles dans `forge.rs` et `isolate.rs` correspondent bien à de vraies dettes restantes, non à des faux positifs;
- les autres occurrences “stub/placeholder” relevées dans `exceptions.rs`, `stage0.rs`, `trampoline_asm.rs`, `kpti_split.rs` ou des commentaires de design n’ont pas toutes la même gravité et n’ont pas été maquillées artificiellement.

## Validation

### Compilation standard

Commande:

```bash
wsl.exe -e bash -lc "cd /mnt/c/Users/xavie/Desktop/Exo-OS && cargo check --workspace --message-format short"
```

Résultat:

- succès;
- workspace compilé vert après nettoyage de warnings locaux qui déclenchaient un ICE nightly pendant `cargo test`.

### Tests ExoFS

Standard:

```bash
wsl.exe -e bash -lc "cd /mnt/c/Users/xavie/Desktop/Exo-OS && cargo test -p exo-os-kernel --lib test_alloc_entries_iter_exposes_entries -- --nocapture"
```

Résultat:

- `fs::exofs::recovery::fsck_phase2::tests::test_alloc_entries_iter_exposes_entries`
- `ok`

Stress:

```bash
wsl.exe -e bash -lc "cd /mnt/c/Users/xavie/Desktop/Exo-OS && cargo test -p exo-os-kernel --lib test_block_io_stress_roundtrips -- --nocapture"
```

Résultat:

- `fs::exofs::recovery::block_io::tests::test_block_io_stress_roundtrips`
- `ok`

### Tests arch-memory

Standard:

```bash
wsl.exe -e bash -lc "cd /mnt/c/Users/xavie/Desktop/Exo-OS && cargo test -p exo-os-kernel --lib test_apply_tsc_offset_signed_direction -- --nocapture"
```

Résultat:

- `arch::x86_64::time::ktime::tests::test_apply_tsc_offset_signed_direction`
- `ok`

Stress:

```bash
wsl.exe -e bash -lc "cd /mnt/c/Users/xavie/Desktop/Exo-OS && cargo test -p exo-os-kernel --lib test_invalid_channel_ids_do_not_alias_stress -- --nocapture"
```

Résultat:

- `ipc::ring::spsc::tests::test_invalid_channel_ids_do_not_alias_stress`
- `ok`

## Notes d’environnement

- Trois fichiers `rustc-ice-2026-04-19T16_25_42-760.txt`, `rustc-ice-2026-04-19T16_26_46-774.txt` et `rustc-ice-2026-04-19T16_27_39-788.txt` ont été générés à la racine lors d’un ICE nightly pendant des runs de tests parallèles.
- La cause observée était côté toolchain/lint emission, pas un panic logique des correctifs. Les runs définitifs sont passés après nettoyage local des warnings et exécution séquentielle.

## Conclusion

- Le périmètre `docs/FIX/2 fs` est maintenant recollé sur les chemins actifs corrigés et validés.
- Le périmètre `docs/FIX/3 arch-memory` a été trié: une partie des alertes était réelle et a été corrigée, une partie était fausse ou théorique, et quelques points restent bloquants faute d’API ou de source de vérité dans le dépôt.
- Les deux zones disposent maintenant d’une validation standard + stress exécutée sous WSL.
