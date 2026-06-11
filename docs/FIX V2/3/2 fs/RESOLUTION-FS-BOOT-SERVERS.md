# Résolution — ExoFS, hang de boot stage0, audit serveurs

**Base :** `AUDIT-EXOFS-COMPLET-V020.md`, `AUDIT-V020-FS-IPC-SCHED-DATAPATH.md`
**Date :** 2026-06-11 · **Build/test :** WSL · `make test` = 3077 passed / 0 failed.

## 1. ExoFS — branchement du moteur transactionnel

### EXOFS-CORE-1 (P0) — ✅ commit_epoch branché sur le chemin chaud
`commit_epoch` n'était déclenché qu'au démontage. Ajout de
`epoch_commit::commit_current_epoch()` (réutilise `do_commit`, même chemin que
le shutdown) et câblage sur :
- **writeback thread** (`mod.rs:exofs_writeback_dirty`) — remplace la persistance
  brute isolée par un commit transactionnel (journal + EpochRoot/Record + 3 barrières).
- **`sync()`** (`fs_bridge.rs:fs_sync`).
- **`fsync()`** (`fs_bridge.rs:fs_fsync`) — durabilité données + métadonnées.
Atomicité et recovery deviennent réels sur le chemin d'écriture.

### EXOFS-ROB-1 (P1) — ✅ pas de fausse durabilité
`commit_durable_epoch_if_disk` refuse désormais (Err `NvmeFlushFailed`) si un
disque est présent mais le hook de flush NVMe n'est pas enregistré, au lieu
d'exécuter ses barrières en no-op. Le chemin dev-sans-disque reste court-circuité.

### EXOFS-ROB-4 (P1) — ✅ fsync ≠ fdatasync
`fs_fsync` respecte `data_only` : `fsync` scelle un epoch (métadonnées) ;
`fdatasync` force seulement les données du blob.

### EXOFS-CORE-3 (P1) — ⚪ VÉRIFIÉ : déjà satisfait
Le PathIndex EST on-disk (chaque répertoire = un blob clé `blob_id_for_path`,
hash déterministe). `snapshot_blob` retombe sur le disque
(`load_blob_data_if_available`), et le catalogue BlobId→LBA est persisté
inconditionnellement après chaque écriture (`persist_catalog_to_global_disk`,
object_store.rs:586) puis rechargé au boot (`ensure_catalog_loaded`).
create/rename/delete marquent les blobs répertoire dirty → committés via CORE-1.
**L'arborescence survit au reboot.** Aucun changement nécessaire.

### EXOFS-ROB-2 (P1) — ⚪ FAUX POSITIF
Les sites cités (key_storage.rs:927/944/953, blob_cache.rs:916/929) sont tous en
`#[cfg(kani)]` (preuves avec `kani::assume`) ou `#[cfg(test)]`. Les fonctions de
désérialisation production (`key_kind_from_u8`, `slot_state_from_u8`) retournent
déjà `ExofsResult`. Scan complet de `fs/exofs` (hors test) : **0** unwrap/expect.

### EXOFS-CORE-2 / Z1 / Z2 — ⚠️ analysés, NON appliqués (suggestions audit erronées)
- **Z1 (read via `get()→Arc`)** : régresserait. Le cache est PAGINÉ ; `read_at`
  copie seulement la plage, tandis que `get()` matérialise le blob ENTIER. Pour
  une lecture partielle d'un gros fichier, `get()` allouerait tout le fichier.
- **Z1/Z2 (éliminer le Vec intermédiaire)** : le Vec existe pour libérer le verrou
  du cache AVANT `copy_to_user`/`read_user_bytes` (qui peuvent fauter). Copier
  vers/depuis l'espace user en tenant le verrou casserait la sûreté du verrou.
  Le design copiant actuel est intentionnel et correct.
- **CORE-2 (pipeline dédup/compress/crypto)** : changerait le format on-disk
  (compressé/chiffré/checksum), rendant les images ExoFS existantes (format brut,
  boot QEMU validé) illisibles. Nécessite un format versionné + compat lecture —
  effort dédié à part, hors de cette passe pour ne pas casser le boot validé.
  NB : le chiffrement obligatoire des `ObjectKind::Secret` reste un P0 sécurité
  à traiter dans cet effort dédié.

## 2. 🔴 Hang de boot après « SECURITY » — ✅ CORRIGÉ

**Symptôme :** boot s'arrête après `stage ok SECURITY`, jamais de STAGE0/IPC/FS/
shell (e9k.txt). Le byte `'S'` (Stage0 done) n'apparaît pas ⇒ blocage DANS
`stage0_init_all_steps()` (lib.rs:277, Phase 5b).

**Cause :** `stage0_init_all_steps()` (init du sentinel ExoPhoenix Kernel-B) a été
câblé sur le chemin de boot de **Kernel A** (FIX-STAGE0, session précédente). Il
contenait un `loop { hlt }` si `initialize_layout_v7()` (SSR) échoue — bloquant
tout le boot de Kernel A avant IPC/FS. Le README confirme que le boot complet
jusqu'au shell ne listait PAS « STAGE0 » : stage0 sur le boot path n'avait jamais
été validé en QEMU avec disque.

**Correctif (FIX-BOOT-STAGE0-HANG) :**
- SSR-fail → `PHOENIX_STATE = Degraded` + **CONTINUE** (plus de halt). Les étapes
  suivantes (IOMMU, APIC, watchdog) ne dépendent pas du layout SSR validé (qui ne
  sert qu'à la résurrection). Le point d'entrée dédié de Kernel B (`stage0_init`)
  garde ses propres gardes d'arrêt.
- Vecteurs ExoPhoenix armés UNIQUEMENT si non dégradé.
- **Instrumentation E9** (préfixe `@`, marqueurs `@1`..`@e`) entre chaque étape de
  stage0 pour pinpointer toute étape encore fautive (à retirer après validation).

**Candidat secondaire instrumenté :** `setup_b_idt_with_stubs()` (étape 4) fait
`load_idt()` sur le BSP de Kernel A. Si le boot s'arrête à `@4` (avant `@5`),
c'est ce `lidt` qui remplace l'IDT — à corriger alors.

**RÉSULTAT (validé en QEMU) :** la cause RÉELLE était que `init_b_tss()` (étape 3)
rechargeait le TR du BSP de Kernel A (`ltr` sur TSS busy ⇒ #GP, RSP0 détourné).
Fix appliqué : `stage0_init_all_steps(kernel_a_boot: bool)` — sur le boot de
Kernel A, on SAUTE les étapes Kernel-B qui clobbent l'état CPU live du BSP
(pile/TSS/IDT/timer APIC/watchdog), en gardant SSR/ACPI/PCI/IOMMU (read-only ou
no-op sans VT-d). Le boot passe désormais **SECURITY → STAGE0 → IPC → FS** (vérifié
en QEMU : `@1@2@5@6@7@8@a@b@c@d@e` → STAGE0 → IPC). Marqueurs E9 conservés (à retirer
après stabilisation).

## 2bis. 🔴 Triple fault dans la première lecture bloc virtio (FS recovery) — EN COURS

Une fois stage0 corrigé, le boot atteint **`exofs_init`** (Phase 7) et **triple-faute**
dans `boot_recovery_sequence` → `SlotRecovery::select_best` → première lecture bloc
virtio (marqueurs `#0#1#2` puis reset CPU, log QEMU `-d int` = boucle `INT=0x08`).

**Diagnostic (vérifié en QEMU) :**
- `init_global_disk` (#1) réussit (négociation + setup queue OK).
- La **première lecture bloc virtio** triple-faute, **où qu'elle survienne** :
  avec `EXO_SKIP_RECOVERY=1` (bypass recovery), le fault se déplace simplement à
  `posix_bridge_init → INODE_EMULATION.ensure_root()` (autre lecture disque).
- Indépendant du mode QEMU (`disable-modern=on,disable-legacy=off` : même fault) ⇒
  pas un mismatch legacy/moderne.
- Buffer correct (`vec![0u8; 4096]` = `EXOFS_BLOCK_SIZE`), DMA alloc valide
  (DMA32|PIN|ZEROED, vaddr = direct map). Le bug est dans le chemin de lecture
  DMA de la crate `virtio-drivers` / interaction HAL, pas dans le code appelant.
- C'est une **régression** (le README bootait jusqu'au shell ⇒ la lecture virtio
  marchait). Les fichiers FS étaient déjà modifiés (M) en début de session.

**Escape hatch :** build avec `EXO_SKIP_RECOVERY=1` (env var, `option_env!`) ⇒ FS
démarre vierge sans recovery — mais bute ensuite sur la même lecture virtio dans
posix_bridge. Donc seul un correctif de la lecture virtio débloque réellement le FS.

### DIAGNOSTIC COMPLET (résolu par instrumentation E9 + #PF-IST)

Le triple fault n'était PAS dans le read virtio lui-même. Chaîne réelle :

1. **Boot stack trop petit (64 KiB)** — `main.rs` `.boot_stack` = `.space 65536`.
   Le chemin profond `kernel_init → exofs_init → init_global_disk/recovery`
   débordait les 64 KiB. La section `.boot_stack` est NOBITS **sans guard page**
   (mémoire basse identity-mappée) ⇒ l'overflow corrompait silencieusement la
   `.bss` adjacente (IDT/TSS/IST/statics), puis le premier fault triple-fautait.
   **Fix : `.space 524288` (512 KiB).** (4 MiB collisionnait la SSR à 16 MiB.)

2. **#PF sans IST (bug IDT)** — `idt.rs` câblait `EXC_PAGE_FAULT` avec IST index
   `0` (pile courante) ALORS QUE la pile IST `page_fault` est allouée dans
   `init_tss_for_cpu` (`tss.ist[IST_PAGE_FAULT]`). Un #PF sur pile débordée ne
   pouvait donc pas empiler sa frame → cascade #DF → triple fault bypassant TOUS
   les handlers (aucun dump #PF/#GP/#DF). **Fix : `IST_PAGE_FAULT as u8 + 1`**
   (comme #DF utilise `IST_DOUBLE_FAULT + 1`).

**Résultat (vérifié QEMU) :** le boot passe désormais **ARCH → … → STAGE0 → IPC →
FS init complet (`#0..#6`) → `seed_kernel_a_image_blob` → ELF loader → fs_bridge →
`@OK:FS`** puis démarre les serveurs Ring1. Énorme progrès depuis le hang SECURITY.

### Blocage résiduel (localisé, non résolu)

Après `@OK:FS`, pendant le démarrage des serveurs, le kernel entre en **boucle de
#PF** (instruction-fetch, `cr2==rip`) à des adresses `PHYS_MAP_BASE + petit_offset`
(~1–2 TiB, direct-map NX). Un **pointeur de code vaut `phys_to_virt(x)`** au lieu
de l'adresse virtuelle de code — phys/virt confusion dans le chemin de création de
process / chargement ELF / context-switch vers init_server. `#PF-IST` a converti le
triple fault en boucle gérée (do_page_fault tourne, préfixe `P`) mais le saut reste
irrésoluble (fetch NX).

**Prochaine étape :** investiguer le setup de l'entrée du premier process Ring1
(`process/lifecycle/exec.rs` / `elf_loader_impl.rs` / context-switch RIP) — un champ
d'entrée/RIP calculé en direct-map au lieu de l'adresse virtuelle ELF. Les marqueurs
E9 (`@`, `#`, `V`, `P`, `#PF cr2=.. rip=..`) et les scripts `tools/justrun.sh` /
`tools/fresh_run_e9.sh` (flags QEMU canoniques) sont en place pour la suite.

### Instrumentation temporaire à retirer après résolution
- `kernel/src/arch/x86_64/exceptions.rs` : `diag_fault_e9`, `FS_REACHED`, marqueurs
  bruts `P`/`G`/`D` dans do_page_fault/do_general_protection/do_double_fault.
- `kernel/src/exophoenix/stage0.rs` : `s0dbg` + marqueurs `@1..@e`.
- `kernel/src/fs/exofs/mod.rs` : `fsdbg` + `#0..#6`.
- `kernel/src/fs/exofs/storage/virtio_adapter.rs` : `vdbg` + `V0..Vd`.
- `kernel/src/fs/exofs/recovery/boot_recovery.rs` : garde `EXO_SKIP_RECOVERY`.
- `kernel/src/lib.rs` : `kdb(b'R')`/`kdb(b'T')`.

### Fixes réels à CONSERVER
- `main.rs` : boot stack 512 KiB (FIX-BOOT-STACK).
- `idt.rs` : #PF sur IST dédié (FIX-PF-IST).
- `stage0.rs`/`lib.rs` : `stage0_init_all_steps(kernel_a_boot)` (FIX-BOOT-STAGE0-HANG).

**Marqueurs E9 de diagnostic en place (à retirer après fix) :**
- stage0 : `@1`SSR `@2`probe `@5`(post TSS/IDT, sautés) `@6`ACPI `@7`PCI `@8`MADT
  `@a`(post APIC, sautés) `@b`IOMMU `@c`FACS `@d`poolR3 `@e`watchdog.
- exofs_init : `#0`entrée `#1`disk `#2`flushbar `#3`recovery `#4`posix `#5`gc `#6`wb.

## 3. Audit cohérence serveurs Ring1 — ✅ état sain

- **Compilation :** `cargo check --workspace --exclude exo-os-kernel` OK (0 erreur).
- **Endpoints cohérents :** init=1, ipc_router=2, vfs=3, crypto=4, memory=5,
  device=6, network=7, scheduler=8, exo_shield=10, input=11, tty=12, virtio=13,
  fb=20. Résolus côté kernel via owner registry (endpoint→PID réel→ServiceClass) ;
  le décalage avec la numérotation ServiceId n'est donc pas un bug.
- **Familles de protocole :** msg_type 0-based (memory/device/scheduler/network/
  init) et 0x1XX (input/tty/fb, syscall_abi) sont deux familles, chacune cohérente.
- **Chemin interactif :** exosh→tty (ep 12), tty→fb (ep 20, via `fb_endpoint_ready`
  + SYS_IPC_LOOKUP), exosh→crypto (ep 4, PID 5) — corrects.
- **Stubs :** 0 `todo!`/`unimplemented!` en production (2 TODO de refacto v0.3.0).
- Les incohérences majeures (exo_shield PID 12→10, ipc_router DAG 5→51 + syscall
  302→300, gardes sécurité memory/scheduler/network/vfs/device) avaient déjà été
  corrigées dans les passes FIX V2/3 précédentes.

**Conclusion :** les serveurs sont cohérents et complets. La cause de « le serveur
FS ne démarre pas » est le hang stage0 (les serveurs ne sont jamais atteints), pas
une incohérence des serveurs eux-mêmes.
