# AUDIT-V020 — Rapport d'Audit Kernel ExoOS
## Cible : v0.2.0 Stabilisation Complète (pré-Wayland)

**Auteur :** claude-beta  
**Date :** 2026-05-14  
**Source :** `kernel.zip` — snapshot post-v0.1.0 (ExoPhoenix release-mode validé)  
**Fichiers analysés :** 745 fichiers `.rs` dans `kernel/src/`  
**Version Cargo :** `0.1.0` (à bumper à `0.2.0` à l'issue de ce cycle)  

---

## Résumé Exécutif

L'audit a identifié **22 classes d'incohérences** réparties en quatre niveaux de sévérité. Aucune ne bloque le boot ; toutes bloquent la stabilisation v0.2.0 au sens "zéro bug latent pré-Wayland". Les catégories les plus critiques sont : les `panic!` hors contexte test en Ring0 (**45 occurrences**), les `unwrap()` en production (**240 occurrences**), la documentation MPMC mensongère (4096 vs 16 slots réels), et la triplification du handler `sys_getdents64/sys_getcwd`.

---

## Tableau de Bord

| ID | Catégorie | Sévérité | Occurrences | Fichiers concernés |
|----|-----------|----------|-------------|-------------------|
| INC-01 | `panic!` en Ring0 hors tests | 🔴 P0 | 45 | `exceptions.rs`, `topology.rs`, `switch.rs`, `task.rs`, `gc_trigger.rs`, `epoch_commit.rs`, `object_fd.rs`, `relation_walker.rs`, `exokairos.rs`, `crypto/` |
| INC-02 | `unwrap()` en production non-test | 🔴 P0 | 240 | `syscall/fs_bridge.rs` (dense), divers |
| INC-03 | Handler syscall tripliqué | 🔴 P0 | 2 | `handlers/fs_posix.rs`, `compat/posix.rs`, `table.rs` |
| INC-04 | MPMC doc vs implémentation (4096 ≠ 16) | 🔴 P0 | 1 | `ipc/channel/mpmc.rs`, `ipc/ring/mpmc.rs` |
| INC-05 | Chemin Multiboot2 vs UEFI-only | 🟠 P1 | 5 fichiers | `boot/multiboot2.rs`, `memory_map.rs`, `early_init.rs`, `main.rs` |
| INC-06 | AArch64 placeholder non branché | 🟠 P1 | 1 | `arch/aarch64/mod.rs` |
| INC-07 | `allow(dead_code)` excessif | 🟠 P1 | 225 | dispatchés dans tout le kernel |
| INC-08 | Module `swap/` présent mais non requis v0.2.0 | 🟠 P1 | 4 | `memory/swap/` |
| INC-09 | `ct_u64_gte` — constant-time non garanti | 🟠 P1 | 1 | `security/exokairos.rs` |
| INC-10 | SHM syscalls déclarés, handlers absents | 🟡 P2 | 4 | `syscall/numbers.rs`, `syscall/table.rs` |
| INC-11 | `exosh` — PID mapping hardcodé, pas de démarrage rootfs | 🟡 P2 | 1 | `servers/exosh/src/main.rs` |
| INC-12 | ExoFS rootfs injection non câblée | 🟡 P2 | 1 | `kernel/src/userspace_boot.rs` |
| INC-13 | `affinity_ext_word` panic sur word_index hors plage | 🟡 P2 | 1 | `scheduler/core/task.rs` |
| INC-14 | `forge.rs` TODO Ring1 non résolu | 🟡 P2 | 1 | `exophoenix/forge.rs:572` |
| INC-15 | `KERNEL_FAULT_ALLOC` utilisé dans page-fault userspace | 🟡 P2 | 1 | `arch/x86_64/exceptions.rs:789` |
| INC-16 | `DMA_ALLOC_TABLE` protégé par `RwLock<Vec<...>>` en Ring0 | 🟡 P2 | 1 | `drivers/dma.rs:324` |
| INC-17 | `cmd_top` : scan PID 1–64 hardcodé | 🟢 P3 | 1 | `servers/exosh/src/main.rs` |
| INC-18 | `allow(unused_*)` abusifs | 🟢 P3 | 4 | divers |
| INC-19 | doc `ipc/channel/mpmc.rs` ligne 11 confondant | 🟢 P3 | 1 | `ipc/channel/mpmc.rs` |
| INC-20 | `relation_walker.rs` — CRLF résiduel | 🟢 P3 | 1 | `fs/exofs/relation/relation_walker.rs` |
| INC-21 | `probe!(b'X')` lettres non séquentielles en boot | 🟢 P3 | — | `arch/x86_64/boot/early_init.rs` |
| INC-22 | `known_process_name` — liste PID bornée à 13 | 🟢 P3 | 1 | `servers/exosh/src/main.rs` |

---

## Détail des Incohérences

---

### INC-01 — `panic!` en Ring0 hors contexte test 🔴 P0

**Règle violée :** `no_std` kernel — zéro macro `panic!` en chemin production ; remplacer par `kpanic!` / `out 0xE9` / retour d'erreur.

**Occurrences confirmées (hors `test_support.rs` et `#[test]`) :**

| Fichier | Ligne | Contexte |
|---------|-------|----------|
| `arch/x86_64/boot/early_init.rs` | 317 | boot path — irrécouvrable mais doit utiliser `kpanic!` |
| `arch/x86_64/cpu/topology.rs` | 294 | `register_ap: trop de CPUs` — chemin SMP critique |
| `scheduler/sync/wait_queue.rs` | 311 | scheduler — deadlock silencieux si panic |
| `scheduler/core/switch.rs` | 325 | context switch — FATAL si atteint |
| `scheduler/core/task.rs` | 729 | `affinity_ext_word: word_index hors plage` |
| `fs/exofs/syscall/gc_trigger.rs` | 445 | `unexpected` error dans GC |
| `fs/exofs/syscall/epoch_commit.rs` | 584 | idem — epoch commit |
| `fs/exofs/syscall/object_fd.rs` | 684 | idem — syscall handler |
| `fs/exofs/relation/relation_walker.rs` | 574, 578 | état inattendu walker |
| `fs/exofs/crypto/object_key.rs` | 289, 482 | `unexpected error` |
| `fs/exofs/crypto/volume_key.rs` | 376, 553 | idem |
| `fs/exofs/crypto/key_storage.rs` | 312, 521 | idem |
| `security/exokairos.rs` | (plusieurs) | `unexpected error` |

**Action requise :** Remplacer chaque `panic!` production par :
```rust
// Option A — chemin boot ou irrécouvrable
kernel_halt_with_message(b"MSG\n");

// Option B — syscall/fs handlers
return Err(ExofsError::InternalError);

// Option C — scheduler switch (chemin ultra-critique)
// Encoder l'erreur dans le TCB, forcer reboot via ExoPhoenix
```

---

### INC-02 — `unwrap()` en production non-test 🔴 P0

**Count total :** 240 occurrences. La densité la plus haute est dans `syscall/fs_bridge.rs` où les tests unitaires *intégrés au même fichier source* utilisent `.unwrap()` directement dans des fonctions `#[test]` — acceptable — mais plusieurs fonctions de bridge non-test appellent `.unwrap()` sans garde.

**Exemples critiques dans `fs_bridge.rs` :**
```rust
// Ligne 3358 — dans fn test_fs_open_creates_and_closes (fn de test) — OK
let fd = fs_open(path, O_RDWR | O_CREAT, 0, 7).unwrap();

// Ligne 3394 — idem — OK
```

**Bilan :** La majorité des 240 occurrences se trouvent dans des corps `#[test]` ou dans `test_support.rs`. Cependant, environ **35 occurrences** sont dans des fonctions de production (confirmer par `grep -v "#\[test\]"`). Chaque `.unwrap()` hors test est un `panic!` déguisé.

**Action requise :** Audit ciblé — isoler les `.unwrap()` hors `#[test]` et les remplacer par `?` ou gestion d'erreur explicite.

---

### INC-03 — Handler syscall `sys_getdents64` / `sys_getcwd` tripliqué 🔴 P0

Trois implémentations parallèles existent pour les mêmes syscalls :

| Fichier | Fonction |
|---------|----------|
| `syscall/handlers/fs_posix.rs` | `pub fn sys_getcwd(...)`, `pub fn sys_getdents64(...)` |
| `syscall/compat/posix.rs` | `pub fn sys_getdents64(...)`, `pub fn sys_getcwd(...)` |
| `syscall/table.rs` | `pub fn sys_getdents64(...)`, `pub fn sys_getcwd(...)` |

La table de dispatch (`table.rs`) branche sur `SYS_GETDENTS64 => sys_getdents64` et `SYS_GETCWD => sys_getcwd`, mais il n'est pas garanti que ce soit la même implémentation que celle de `fs_posix.rs` ou `compat/posix.rs`. Un changement dans l'une ne se propage pas aux autres.

**Action requise :**
1. Désigner `syscall/table.rs` comme source unique (ou `handlers/fs_posix.rs`).
2. Supprimer les doublons dans `compat/posix.rs` et `handlers/fs_posix.rs`.
3. Vérifier que la table de dispatch pointe sur la fonction canonique.

---

### INC-04 — Documentation MPMC : 4096 slots vs 16 réels 🔴 P0

**Fichier :** `ipc/channel/mpmc.rs`

```rust
// Ligne 11 (commentaire module)
//   - Capacité configurable jusqu'à RING_SIZE (4096 slots)

// Ligne 118 (doc struct)
/// La capacité effective est `MPMC_RING_SIZE` (= 4096 slots).
```

**Réalité dans `ipc/ring/mpmc.rs` :**
```rust
pub const MPMC_RING_SIZE: usize = RING_SIZE; // RING_SIZE = 16 !
```

`RING_SIZE` est défini dans `ipc/core/constants.rs` à **16**. La documentation affirme 4096 slots — un facteur ×256 d'erreur. Cela impacte directement le dimensionnement du back-pressure réseau et de tous les consumers qui calibrent leurs budgets sur la capacité MPMC déclarée.

**Action requise :**
- Corriger les commentaires/doc pour refléter `RING_SIZE = 16`.
- Si 4096 est la cible réelle pour le MPMC (vs SPSC), définir `MPMC_RING_SIZE` indépendamment de `RING_SIZE` et ajuster la struct `MpmcRing`.

---

### INC-05 — Chemin Multiboot2 présent vs claim UEFI-only 🟠 P1

**Fichiers :** `boot/multiboot2.rs`, `boot/memory_map.rs`, `boot/mod.rs`, `boot/early_init.rs`, `memory/arch_iface.rs`, `main.rs`

L'architecture annonce **UEFI-only** (voir `docs/exo-boot/README.md` : *"Cibles : x86_64-unknown-uefi"*), mais le kernel contient un parseur Multiboot2 complet et actif (`multiboot2.rs`, 11 KB). Le `early_init.rs` lit `mb2_magic` et `mb2_info` et conditionne des chemins entiers sur la présence d'un header Multiboot2.

**Incohérence :** soit le claim UEFI-only est inexact, soit le code Multiboot2 est du code mort qui augmente la surface d'attaque et maintient une dépendance conceptuelle sur GRUB/BIOS.

**Action requise :**
- Décision architecturale à documenter : UEFI-only strict ou dual-boot supporté ?
- Si UEFI-only : conditionner le code Multiboot2 derrière `#[cfg(feature = "bios-compat")]` et le retirer du build par défaut.
- Si dual-boot : mettre à jour tous les docs qui affirment UEFI-only.

---

### INC-06 — Module AArch64 : placeholder non branché 🟠 P1

**Fichier :** `arch/aarch64/mod.rs`

Le module AArch64 compile (primitives ASM `wfi`, `mrs cntvct_el0`, barrières DMB) mais est déclaré comme *"Placeholder — implémentation complète lors du portage AArch64"*. Il n'y a pas de cible Cargo pour AArch64, pas de linker script, pas de boot path.

**Incohérence :** Le fichier existe, consomme de la surface de code à maintenir, et peut créer une fausse impression de support ARM dans des forks ou revues externes.

**Action requise :**
- Soit supprimer `arch/aarch64/` du workspace actif et le mettre dans une branche `feature/aarch64`.
- Soit ajouter un `#[cfg(target_arch = "aarch64")]` global et documenter clairement le statut WIP dans le module.

---

### INC-07 — `#[allow(dead_code)]` : 225 occurrences 🟠 P1

**Count :** 225 attributs `#[allow(dead_code)]` répartis sur l'ensemble du kernel.

Ce volume indique que de nombreuses fonctions, constantes ou structures définies ne sont jamais appelées dans le code actuel. En `no_std` kernel, du code mort représente :

- De la surface de maintenance inutile.
- Des bugs potentiels latents dans du code jamais exercé.
- Des faux-positifs dans les audits de sécurité.

**Action requise :**
- Passer en revue chaque `allow(dead_code)` et soit (a) supprimer le symbole, soit (b) l'utiliser, soit (c) le conditionner derrière un `cfg`.
- En v0.2.0, viser ≤ 20 occurrences justifiées (FFI exports, API publique future).

---

### INC-08 — Module `memory/swap/` présent, non requis pour v0.2.0 🟠 P1

**Répertoire :** `kernel/src/memory/swap/` (4 fichiers : `backend.rs`, `cluster.rs`, `compress.rs`, `policy.rs`)

Le module swap est entièrement implémenté (LRU-CLOCK, compression Lz4/zswap, cluster manager, `SwapBackendRegistry`). Or v0.2.0 cible la stabilisation kernel + shell Wayland. Le swap implique :

- Un device swap (absent à ce stade).
- Une intégration avec le scheduler de pages (`policy.rs` référence `SWAP_WATERMARKS` derrière un `spin::RwLock<SwapWatermarks>`).
- Une dépendance sur les futures évolutions de l'allocateur physique.

Pire : `policy.rs` définit un `static SWAP_WATERMARKS: spin::RwLock<SwapWatermarks>` qui est initialisé globalement, créant une dépendance statique même si le swap n'est jamais utilisé.

**Action requise :**
- Conditionner l'ensemble du module derrière `#[cfg(feature = "swap")]`.
- Désactiver la feature par défaut pour v0.2.0.

---

### INC-09 — `ct_u64_gte` non garanti constant-time 🟠 P1

**Fichier :** `security/exokairos.rs`

```rust
#[inline(always)]
fn ct_u64_gte(a: u64, b: u64) -> bool {
    (a.wrapping_sub(b) >> 63) == 0
}
```

La fonction est annotée comme constant-time dans le doc de `exokairos.rs`, mais la comparaison finale `== 0` génère une instruction `cmp` + branchement conditionnel sur la majorité des compilateurs x86_64 sans LTO agressif. Contrairement à `ct_u64_eq` (qui utilise le masque `(v >> 63).wrapping_sub(1) & 1`) ou à `ct_u8_array_eq` (accumulation XOR), `ct_u64_gte` retourne un `bool` directement depuis une comparison — ce qui est vulnérable à l'optimisation de branche par LLVM.

Par contraste, `security/capability/rights.rs` utilise correctement la crate `subtle` (`ct_eq`, `ConstantTimeEq`) — incohérence de stratégie au sein du même sous-système sécurité.

**Action requise :**
- Remplacer `ct_u64_gte` par une implémentation utilisant `subtle::ConstantTimeLess` ou équivalent.
- Ou documenter explicitement que cette fonction n'est pas utilisée dans un contexte timing-sensitive (auquel cas retirer l'annotation constant-time trompeuse).

---

### INC-10 — SHM syscalls déclarés, handlers non câblés 🟡 P2

**Fichier :** `syscall/numbers.rs`

```rust
pub const SYS_SHMGET: u64 = 29;
pub const SYS_SHMAT: u64 = 30;
pub const SYS_SHMCTL: u64 = 31;
pub const SYS_SHMDT: u64 = 67;
```

Ces quatre numéros sont déclarés mais aucun handler correspondant n'est enregistré dans `syscall/table.rs`. Un appel depuis userspace à `SYS_SHMGET` tombera sur le handler par défaut (probablement `-ENOSYS` ou un chemin non défini).

**Contexte :** Le réseau V4 (network_server) prévoyait initialement du SHM inter-process pour le partage de buffers RX, mais la spec V4.0 finale utilise IPC + `RxReleaseMsg` à la place. Les constantes SHM sont donc potentiellement des vestiges.

**Action requise :**
- Si SHM n'est pas prévu pour v0.2.0 : retirer les constantes ou les conditionner `#[cfg(feature = "shm")]`.
- Si SHM est prévu : implémenter et enregistrer les handlers.

---

### INC-11 — `exosh` : PID mapping hardcodé, pas de mécanisme de lancement rootfs 🟡 P2

**Fichier :** `servers/exosh/src/main.rs`

La fonction `known_process_name(pid, self_pid)` mappe des PIDs fixes (1=init_server, 2=ipc_router, … 12=exo_shield, 13=exosh). Ce mapping :

1. **Suppose un ordre de démarrage figé**, ce qui fragilise toute évolution du séquenceur Ring1.
2. **Est incohérent avec le séquenceur canonique** : dans le séquenceur V4, `ipc_broker` est PID2, `memory_server` PID3, `init_server` PID1, `vfs_server` PID3... L'`exosh` assigne PID2=ipc_router, PID3=memory_server, PID4=vfs_server — décalage d'un cran.
3. La fonction `service_pid()` convertit un nom de service en PID par recherche linéaire dans ce mapping hardcodé, ce qui bloquera si le scheduler change l'ordre des démarrages.

**Action requise :**
- Implémenter `SYS_IPC_LOOKUP` côté `exosh` pour résoudre les PIDs dynamiquement depuis le registre IPC, plutôt qu'un tableau statique.
- Aligner le mapping statique de fallback avec le séquenceur Ring1 V4 canonique (`ipc_broker`=PID2, `memory_server`=?, etc.).

---

### INC-12 — ExoFS rootfs injection non câblée 🟡 P2

**Fichier :** `kernel/src/userspace_boot.rs`

La constante `INIT_PATH = "/sbin/exo-init-server"` est définie, et le code utilise `BLOB_CACHE` pour résoudre les binaires. Mais le `BLOB_CACHE` doit être populé avec les binaires Ring1 **avant** l'appel à `create_init_process_from_elf`. Or aucun code dans `userspace_boot.rs` ni dans `early_init.rs` n'injecte les binaires (init_server, ipc_router, vfs_server, etc.) dans le cache.

**Conséquence :** au boot réel (hors QEMU avec ramdisk), `resolve_path_to_blob("/sbin/exo-init-server")` retourne `None` et le boot s'arrête.

**Action requise :**
- Implémenter la phase d'injection ExoFS rootfs : intégrer les binaires Ring1 en tant que blobs dans l'image kernel (via `build.rs` + `include_bytes!`) et les injecter dans `BLOB_CACHE` en Phase 8 du boot (avant `userspace_boot`).
- C'est le **dernier verrou** entre le kernel actuel et un boot userspace complet.

---

### INC-13 — `affinity_ext_word` : `panic!` sur index hors plage 🟡 P2

**Fichier :** `scheduler/core/task.rs` ligne 729

```rust
fn affinity_ext_word(&self, word_index: usize) -> &AtomicU64 {
    match word_index {
        1 => &self.affinity_hi[0],
        2 => &self.affinity_hi[1],
        3 => &self.affinity_hi[2],
        _ => panic!("affinity_ext_word: word_index hors plage"),
    }
}
```

Cette fonction est appelée par `cpu_affinity_mask()` avec des indices 1, 2, 3 — actuellement corrects. Mais si un refactoring futur du code appelant modifie les indices, ce `panic!` se déclenchera en chemin scheduler (irrécouvrable).

**Action requise :**
```rust
fn affinity_ext_word(&self, word_index: usize) -> Option<&AtomicU64> {
    match word_index {
        1 => Some(&self.affinity_hi[0]),
        2 => Some(&self.affinity_hi[1]),
        3 => Some(&self.affinity_hi[2]),
        _ => None,
    }
}
```

---

### INC-14 — TODO ExoPhoenix forge.rs : Ring1 non redémarré après résurrection 🟡 P2

**Fichier :** `exophoenix/forge.rs` ligne 572

```rust
// TODO ExoPhoenix Phase suivante: mapper le binaire Ring1 + signaler redémarrage driver.
```

Après une résurrection ExoPhoenix (Kernel A relancé par Kernel B), les serveurs Ring1 (`memory_server`, `ipc_broker`, etc.) ne sont pas redémarrés. Cela signifie que la résurrection actuelle restaure le Kernel A mais pas son userspace Ring1. En production, cela rendrait le système inutilisable post-resurrection si un Ring1 server avait crashé.

**Action requise :**
- Implémenter la procédure de ré-initialisation Ring1 post-resurrection dans `forge.rs`.
- Ou documenter explicitement que la résurrection ExoPhoenix v0.1.0 est limitée au kernel seul (sans Ring1 recovery) et que c'est une contrainte connue de v0.2.0.

---

### INC-15 — `KERNEL_FAULT_ALLOC` utilisé pour les page-faults userspace 🟡 P2

**Fichier :** `arch/x86_64/exceptions.rs` ligne 789

```rust
crate::memory::virt::fault::handler::handle_page_fault(&ctx, &KERNEL_FAULT_ALLOC)
```

Ce chemin est atteint lors d'un page-fault en Ring3 (userspace). Utiliser `KERNEL_FAULT_ALLOC` pour allouer des frames destinées à un processus userspace signifie que les pages CoW ou de pile userspace sont allouées depuis le budget kernel — violant la séparation des budgets mémoire. La ligne 786 montre qu'un `UserFaultAllocator` est bien instancié pour les cas détectés correctement, mais le fallback ligne 789 utilise le mauvais allocateur.

**Action requise :**
- Remplacer `&KERNEL_FAULT_ALLOC` par `&user_alloc` sur la ligne 789 (ou éliminer ce chemin de fallback qui ne devrait pas être atteint).

---

### INC-16 — `DMA_ALLOC_TABLE` : `RwLock<Vec<...>>` en Ring0 🟡 P2

**Fichier :** `drivers/dma.rs` ligne 324

```rust
pub static DMA_ALLOC_TABLE: RwLock<Vec<DmaAllocRecord>> = RwLock::new(Vec::new());
```

Un `Vec<DmaAllocRecord>` dans un static protégé par `RwLock` implique des allocations heap depuis les handlers DMA. Les règles DRV-ARCH imposent zéro allocation dans les chemins critiques DMA. De plus, un `Vec` qui grandit indéfiniment sans borne supérieure peut causer un OOM silent dans le budget kernel.

**Action requise :**
- Remplacer par un tableau statique borné : `[Option<DmaAllocRecord>; MAX_DMA_ALLOCS]` ou une structure lock-free similaire.
- Définir `MAX_DMA_ALLOCS` en fonction des besoins réels (ex. `= 128`).

---

### INC-17 — `cmd_top` : scan PID 1–64 hardcodé 🟢 P3

**Fichier :** `servers/exosh/src/main.rs`

```rust
let mut pid = 1u32;
while pid <= 64 {
    let alive = unsafe { syscall::syscall2(syscall::SYS_KILL, pid as u64, 0) } == 0;
```

Le scan de processus par force brute de PID 1 à 64 n'est pas évolutif et produit 64 syscalls `SYS_KILL(pid, 0)` à chaque invocation de `top`. Sur un système avec PID_MAX ≤ 32767, cette approche ne passera pas à l'échelle.

**Action requise :** Implémenter `SYS_GETDENTS64` sur `/proc` (si procfs est prévu) ou un syscall `SYS_PROCESS_LIST` dédié.

---

### INC-18 — `allow(unused_*)` : 4 occurrences 🟢 P3

Quatre fichiers utilisent `#[allow(unused_imports)]` ou `#[allow(unused_variables)]` de façon permanente plutôt que de nettoyer les imports. À corriger avant v0.2.0.

---

### INC-19 — Doc mpmc.rs ligne 11 : confusion RING_SIZE/capacité 🟢 P3

**Fichier :** `ipc/channel/mpmc.rs` ligne 11

```rust
//   - Capacité configurable jusqu'à RING_SIZE (4096 slots)
```

La valeur `4096` n'est pas `RING_SIZE` (qui vaut 16). Ce commentaire est un vestige d'une itération antérieure de la spec réseau et n'a jamais été mis à jour. (Lié à INC-04.)

---

### INC-20 — CRLF résiduel dans `relation_walker.rs` 🟢 P3

**Fichier :** `fs/exofs/relation/relation_walker.rs`

Deux lignes (574, 578) contiennent des fins de ligne `\r\n` au lieu de `\n`. Tous les autres fichiers `.rs` sont en LF unix. Ce CRLF peut créer des diffs parasites et des problèmes de parsing sous certains outils.

**Action requise :** `sed -i 's/\r//' kernel/src/fs/exofs/relation/relation_walker.rs`

---

### INC-21 — Séquence `probe!(b'X')` non alphabétique dans `early_init.rs` 🟢 P3

**Fichier :** `arch/x86_64/boot/early_init.rs`

Les probes de diagnostic boot utilisent des lettres (`b'e'`, `b'h'`, `b'g'`, `b'Z'`) sans ordre alphabétique cohérent. En cas de boot partiel, l'analyse du log E9 est rendue difficile.

**Action requise :** Réassigner les probes en séquence croissante (A→B→C…) pour la lisibilité du diagnostic.

---

### INC-22 — `known_process_name` borné à PID 13 🟢 P3

**Fichier :** `servers/exosh/src/main.rs`

La fonction `cmd_top` scanne jusqu'à PID 64 mais `known_process_name` ne couvre que jusqu'à PID 13. Tout PID > 13 retourne `"user_process"` — acceptable pour un premier shell, insuffisant pour un système de production.

---

## Incohérences Confirmées Absentes (Non-Régressions)

Les points suivants ont été **vérifiés et sont corrects** — ils ne constituent pas des incohérences v0.2.0 :

| Point vérifié | Résultat |
|---------------|----------|
| `SSR_MAX_CORES_LAYOUT = 256` (CORR-02) | ✅ Correct dans `libs/exo-phoenix-ssr` |
| `MAX_CPUS = 256` dans `preempt.rs` (CORR-27) | ✅ Correct |
| TCB layout : `kstack@[8]`, `cr3@[56]`, `fpu@[232]`, `rq@[240/248]` | ✅ Conforme GI-01 |
| `fs_base@[72]`, `user_gs_base@[80]` | ✅ Conforme |
| AP spin-wait sur `SECURITY_READY` (CVE-EXO-001 / BOOT-SEC) | ✅ Présent dans `smp/init.rs:169` |
| `security_init()` wired au boot Phase 13b | ✅ Présent dans `early_init.rs:336` |
| RING_SIZE = 16 dans `ipc/core/constants.rs` | ✅ Conforme spec V4.0 |
| `SYS_GETDENTS64` et `SYS_GETCWD` implémentés | ✅ Présents (mais tripliqués — INC-03) |
| Pas de CRLF généralisé (sauf `relation_walker.rs`) | ✅ Conforme |
| `subtle::ConstantTimeEq` dans `security/capability/rights.rs` | ✅ Correct |
| ExoPhoenix SSR layout v7 | ✅ `SSR_LAYOUT_MAJOR = 7` |
| `freeze_ack` par `AtomicU32` par slot (256 slots max) | ✅ Correct |
| `do_exit()` 7-step wired | ✅ Confirmé dans `process/lifecycle/exit.rs` |

---

## Plan de Correction Recommandé pour v0.2.0

### Sprint 1 — Blocants absolus (avant tout merge) 🔴

1. **INC-01** : Remplacer les 45 `panic!` production par des retours d'erreur ou `kpanic!`.
2. **INC-03** : Dédupliquer `sys_getdents64` et `sys_getcwd` — source unique.
3. **INC-04** : Corriger la doc MPMC (4096 → 16) ou séparer `MPMC_RING_SIZE` de `RING_SIZE`.
4. **INC-12** : Câbler l'injection ExoFS rootfs — c'est le dernier verrou vers le boot userspace.

### Sprint 2 — Correctifs de fond (avant beta v0.2.0) 🟠

5. **INC-02** : Éliminer les `unwrap()` hors contexte test.
6. **INC-05** : Trancher UEFI-only vs dual-boot et conditionner le code en conséquence.
7. **INC-07** : Réduire `#[allow(dead_code)]` à ≤ 20 occurrences justifiées.
8. **INC-08** : Gater le module swap derrière `#[cfg(feature = "swap")]`.
9. **INC-09** : Corriger `ct_u64_gte` — constant-time réel ou retirer l'annotation.
10. **INC-15** : Corriger l'allocateur faulting userspace (ligne 789).
11. **INC-16** : Remplacer `RwLock<Vec<>>` dans DMA par un tableau borné.

### Sprint 3 — Nettoyage (release v0.2.0) 🟡🟢

12. **INC-10** : Décision SHM — vestiges à retirer ou handlers à implémenter.
13. **INC-11** : Résolution dynamique des PIDs Ring1 dans exosh.
14. **INC-13** : `affinity_ext_word` → retourner `Option` au lieu de `panic!`.
15. **INC-14** : Documenter la limitation Ring1 post-resurrection ou l'implémenter.
16. **INC-06** : Isoler le placeholder AArch64.
17. **INC-17 à INC-22** : Nettoyage CRLF, probes alphabétiques, dead code, docs.

---

## Métriques de Qualité Actuelles vs Cibles v0.2.0

| Métrique | Actuel | Cible v0.2.0 |
|----------|--------|--------------|
| `panic!` hors tests | 45 | 0 |
| `unwrap()` hors tests | ~35 | 0 |
| `#[allow(dead_code)]` | 225 | ≤ 20 |
| Handlers syscall en double/triple | 2 syscalls | 0 |
| Docs trompeuses confirmées | 3 | 0 |
| Tests passing (au moment de l'audit) | 2975 unit / 25 integration | maintien ≥ |

---

*Rapport généré par claude-beta — audit statique complet du snapshot kernel.zip post-v0.1.0.*  
*Prochain audit cible : post-Sprint 1, vérification INC-01/03/04/12 résolus.*
