# Audit de Stabilisation ExoOS — v0.2.0
## Rapport d'incohérences et corrections requises

**Projet** : Exo-OS — Microkernel formellement vérifié, capability-based, Rust no_std x86_64  
**Version auditée** : v0.1.0 → v0.2.0 (branche stabilisation)  
**Périmètre** : kernel/, servers/, exo-boot/, userspace/  
**Auteur** : claude-beta  
**Date** : 2026-05-15  
**Statut** : CONFIDENTIEL — Usage interne développement

---

## Table des matières

1. [Résumé exécutif](#1-résumé-exécutif)
2. [P0 — Défauts critiques (blocants démarrage)](#2-p0--défauts-critiques-blocants-démarrage)
3. [P1 — Défauts majeurs (instabilité runtime)](#3-p1--défauts-majeurs-instabilité-runtime)
4. [P2 — Défauts mineurs (dégradation silencieuse)](#4-p2--défauts-mineurs-dégradation-silencieuse)
5. [P3 — Incohérences architecturales (dette technique)](#5-p3--incohérences-architecturales-dette-technique)
6. [Matrice de risque v0.2.0](#6-matrice-de-risque-v020)
7. [Plan de correction recommandé](#7-plan-de-correction-recommandé)

---

## 1. Résumé exécutif

L'audit couvre 1 334 fichiers sources répartis dans six sous-arbres (`kernel/`, `servers/`, `exo-boot/`, `drivers/`, `loader/`, `userspace/`). L'analyse statique croisée révèle **4 défauts P0**, **5 défauts P1**, **4 défauts P2** et **3 incohérences architecturales P3**.

Les deux défauts les plus sévères concernent (a) l'absence d'appel à `map_physmap()` au-delà du GiB de boot, qui rend tout accès physique supérieur à 1 GiB non mappé en production, et (b) l'omission de `cgroup::init()` dans la séquence `kernel_init()`, ce qui laisse le cgroup racine dans un état `valid = 0` pour l'intégralité du cycle de vie du système.

Le kernel peut démarrer dans les configurations QEMU standard (< 1 GiB de RAM) et passer les suites de tests unitaires, mais **ne peut pas être considéré stable** avant résolution des défauts P0 et P1.

---

## 2. P0 — Défauts critiques (blocants démarrage)

### P0-A · Physmap non étendue au-delà de 1 GiB

**Fichiers concernés** :  
- `kernel/src/memory/virtual/page_table/builder.rs` — `fn map_physmap()`  
- `kernel/src/main.rs` — trampoline 32→64 bits  
- `kernel/src/arch/x86_64/boot/memory_map.rs`

**Description**

La fonction `PageTableBuilder::map_physmap(phys_size)` existe et mappe correctement toute la RAM physique détectée à `PHYS_MAP_BASE = 0xFFFF_8000_0000_0000`. Elle n'est cependant **jamais appelée** dans tout le codebase. Une recherche exhaustive de `.map_physmap(` ne retourne aucun résultat hors de sa propre définition.

Au boot, le trampoline ASM de `main.rs` établit uniquement un mapping identité 0..1 GiB via `_boot_pd` (512 huge-pages 2 MiB) et copie `_boot_pdpt` dans `PML4[256]`. Tout appel à `phys_to_virt(addr)` pour une adresse physique `addr ≥ 0x4000_0000` (1 GiB) génère un `#PF` Ring 0 immédiat.

L'ELF loader (`elf_loader_impl.rs`) utilise `copy_kernel_entries()` qui copie les entrées PML4[256..512] de la table active — mais cette table active est précisément la table boot qui ne couvre que 1 GiB. Les processus userspace héritent donc du même mapping tronqué.

**Impact** : `phys_to_virt` utilisé dans le buddy allocator, le BLOB_CACHE, le SLUB heap, l'ExoPhoenix SSR et les DMA engines. Tout système QEMU configuré avec `> 1 GiB` de RAM (`-m 2G` ou plus) panique au premier accès physique hors fenêtre.

**Correction requise**

Appeler `builder.map_physmap(total_ram_bytes)` dans la séquence `init_memory_subsystem_multiboot2()` ou `init_memory_subsystem_exoboot()`, après détection de la taille RAM, avant activation de l'allocateur buddy. Utiliser des huge-pages 2 MiB (flag `FLAG_HUGE`) plutôt que des pages 4 KiB pour éviter un mapping séquentiel de plusieurs millions d'entrées sur une machine avec 16+ GiB de RAM.

```rust
// kernel/src/arch/x86_64/boot/memory_map.rs
// Après la boucle de détection RAM :
let mut builder = PageTableBuilder::new(&frame_alloc)?;
unsafe { builder.map_physmap(total_ram_bytes)? };
// puis remplacer CR3 par la nouvelle PML4
```

---

### P0-B · `cgroup::init()` non appelé dans `kernel_init()`

**Fichiers concernés** :  
- `kernel/src/lib.rs` — `fn kernel_init()`, Phase 4  
- `kernel/src/process/mod.rs` — `fn init()`  
- `kernel/src/process/resource/cgroup.rs` — `fn init()`

**Description**

Le commentaire de la Phase 4 dans `kernel_init()` liste explicitement `resource::cgroup::init()` comme étape 5 de l'initialisation du sous-système processus. Le code réel, en revanche, appelle manuellement chaque sous-composant dans l'ordre suivant et **omet** cet appel :

```rust
// kernel/src/lib.rs — Phase 4, tel qu'implémenté
crate::process::core::pid::init(32768, 131072);     // ✓
crate::process::core::registry::init(32768);         // ✓
crate::process::lifecycle::reap::init_reaper();      // ✓
crate::process::state::wakeup::register_with_dma();  // ✓
// crate::process::resource::cgroup::init();         // ✗ ABSENT
```

La table `CGROUP_TABLE` est initialisée statiquement via `CgroupTable::new()` qui positionne `valid = AtomicU32::new(0)` pour tous les slots. `cgroup::init()` positionne `root.valid = 1` et `CGROUP_TABLE.count = 1`. Sans cet appel, le cgroup racine reste `valid = 0`.

Tout chemin qui vérifie `slot.valid.load() == 1` (ligne 179 de `cgroup.rs`) retourne une erreur ou saute l'opération pour le cgroup racine. Chaque processus créé reçoit `cgroup_handle = CgroupHandle::ROOT` (handle 0), mais les opérations de comptage, d'application des limites CPU/mémoire et d'accounting échouent silencieusement.

**Impact** : Limitation CPU/mémoire non appliquée sur aucun processus. Compteurs d'utilisation incorrects. Le reaper kthread peut tenter des opérations cgroup sur un état non initialisé.

**Correction requise**

Ajouter l'appel manquant dans la Phase 4 de `kernel_init()`, après `register_with_dma()` et avant la suppression du guard IRQ :

```rust
crate::process::resource::cgroup::init();
kdb(b'G'); // cgroup root initialized
```

---

### P0-C · Numéro magique `len == 128` dans `sys_exo_ipc_send()`

**Fichiers concernés** :  
- `kernel/src/syscall/table.rs` — `fn sys_exo_ipc_send()`, ligne ~2695  
- `servers/syscall_abi/src/lib.rs` — `IPC_ENVELOPE_SIZE = 128`

**Description**

Le handler `sys_exo_ipc_send()` contient la logique suivante :

```rust
if len == 128 {
    let caller_pid = syscall_current_pid();
    payload[..4].copy_from_slice(&caller_pid.to_le_bytes());
}
```

Cette injection de PID est conditionnée à `len == 128`, soit exactement `IPC_ENVELOPE_SIZE` du `syscall_abi`. Elle écrase silencieusement les 4 premiers octets du payload de tout message de 128 octets avec le PID de l'appelant, **indépendamment** de la structure attendue par le serveur destinataire.

Ce comportement présente trois défauts :

1. **Fragile** : tout message de 128 octets voit son début corrompu, même s'il n'est pas de type `NetMsg` ou `IpcEnvelope`.
2. **Non documenté côté ABI** : `syscall_abi/src/lib.rs` ne mentionne pas cette réécriture.
3. **Bypassable** : un appelant malveillant envoie un message de 127 ou 129 octets pour éviter l'injection.

La vérification correcte serait d'utiliser un champ dédié dans l'en-tête IPC (e.g. `sender_pid` dans `IPC_HEADER_SIZE`) plutôt qu'une réécriture de payload basée sur la taille.

**Impact** : Corruption silencieuse de messages. Vecteur de contournement de l'attribution PID. Incompatibilité dès que le protocole change de taille.

**Correction requise**

Déplacer le `sender_pid` dans les 8 octets d'en-tête `IPC_HEADER_SIZE` de `syscall_abi::IpcMessage`. Le kernel injecte systématiquement le PID appelant dans l'en-tête, sans condition sur la longueur :

```rust
// En-tête IPC — champ sender_pid à ajouter
pub struct IpcHeader {
    pub endpoint: u32,
    pub sender_pid: u32,  // injecté kernel-side, non falsifiable
}
```

---

### P0-D · `exo_shield` bloque le shell si un service `critical=false` échoue

**Fichiers concernés** :  
- `servers/init_server/src/service_table.rs` — `DEPS_EXO_SHIELD`, `DEPS_EXOSH`  
- `servers/init_server/src/boot_sequence.rs` — `fn boot_services()`  
- `servers/init_server/src/supervisor.rs` — `fn dependency_ready()`

**Description**

`exo_shield` est déclaré `critical = true` et dépend de dix services, dont `network_server` et `scheduler_server` qui sont tous les deux `critical = false`. La fonction `dependency_ready()` exige qu'un service ait un PID courant non nul (`service_started()`) pour être considéré comme satisfait :

```rust
pub fn dependency_ready(services: &[Service], dep: &str) -> bool {
    dep == "init_server" || service_manager::service_started(services, dep)
}
```

Si `network_server` échoue à démarrer (timeout réseau, absence de carte virtio), `boot_services()` le tue et appelle `mark_dead()` (pid → 0). `exo_shield` ne peut plus jamais satisfaire ses dépendances. `exosh` dépend à son tour de `exo_shield`. Le shell ne démarrera jamais, même si le réseau n'est pas nécessaire pour l'usage interactif.

**Impact** : Système sans shell interactif sur toute machine sans interface virtio-net. Régression critique pour le débogage et la v0.2.0.

**Correction requise**

Distinguer les dépendances optionnelles des dépendances obligatoires dans `ServiceMetadata`. Un service `critical = false` dont la dépendance est marquée `optional` ne bloque pas le démarrage du dépendant :

```rust
pub struct ServiceMetadata {
    pub requires: &'static [&'static str],
    pub requires_optional: &'static [&'static str], // n'empêchent pas can_start()
    ...
}
```

Les dépendances de `exo_shield` envers `network_server` et `scheduler_server` passent dans `requires_optional`.

---

## 3. P1 — Défauts majeurs (instabilité runtime)

### P1-A · Divergence de taille entre `ipc::message::IpcMessage` (4 096 B) et `ipc::core::MAX_MSG_SIZE` (240 B)

**Fichiers concernés** :  
- `kernel/src/ipc/message/builder.rs` — `MAX_MSG_INLINE = 4096`  
- `kernel/src/ipc/core/constants.rs` — `MAX_MSG_SIZE = 240`  
- `kernel/src/ipc/ring/spsc.rs` — slots de `RING_SLOT_SIZE = 248 B`

**Description**

Il existe deux abstractions de message IPC dans le kernel avec des tailles de payload radicalement différentes :

| Type | Fichier | Taille payload |
|---|---|---|
| `ipc::message::IpcMessage` | `message/builder.rs` | 4 096 B (stack-allocated) |
| `ipc::core::IpcFastMsg` | `core/transfer.rs` | 240 B (dans le ring SPSC) |

`IpcMessageBuilder` produit des `IpcMessage` avec un payload pouvant atteindre 4 096 octets. Cependant, `send_raw()` (chemin emprunté par `sys_exo_ipc_send()`) copie dans un `RingSlot` de `RING_SLOT_SIZE = 248 B`. Tout message produit par le builder au-delà de 240 octets tronque silencieusement le payload lors du transfert ring.

Ce n'est pas un problème actif en production (le syscall IPC limite à `MAX_MSG_SIZE = 240 B`), mais la présence simultanée des deux constantes est une bombe à retardement pour tout développeur utilisant le builder kernel-side sans passer par le syscall.

**Correction requise**

Aligner `MAX_MSG_INLINE` sur `MAX_MSG_SIZE` ou supprimer le builder kernel-side au profit de l'API ring directe. Ajouter un `const_assert` :

```rust
const _: () = assert!(
    MAX_MSG_INLINE <= MAX_MSG_SIZE,
    "IpcMessage inline dépasse la capacité du ring SPSC"
);
```

---

### P1-B · `ExoSmoltcpDevice` encapsule un `*mut ExoNetDevice` sous `Mutex<NetworkService>`

**Fichiers concernés** :  
- `servers/network_server/src/smoltcp_iface.rs` — `struct ExoSmoltcpDevice`  
- `servers/network_server/src/main.rs` — `static NETWORK_SERVICE: Mutex<NetworkService>`

**Description**

`ExoSmoltcpDevice` stocke un pointeur brut `*mut ExoNetDevice` :

```rust
struct ExoSmoltcpDevice<'a> {
    device: *mut ExoNetDevice,
    pool: &'a NetBufPool,
    ...
}
```

Ce pointeur est dérivé d'une référence `&mut ExoNetDevice` issue du `MutexGuard<NetworkService>`. Le guard reste verrouillé pendant tout le cycle `poll_one()`. En apparence sûr, mais `ExoSmoltcpDevice` implémente `Device` pour smoltcp dont les callbacks `RxToken::consume()` et `TxToken::consume()` peuvent être appelés de façon imbriquée par smoltcp.

Si smoltcp appelle `transmit()` depuis l'intérieur d'un callback `receive()`, le pointeur brut passe outre le borrow checker, créant une aliasing mutable `&mut device` + accès via `*mut device` simultanés — undefined behavior en Rust, même en `no_std`.

**Correction requise**

Passer `&mut ExoNetDevice` directement à chaque appel smoltcp via un wrapper à durée de vie bornée, sans stocker de pointeur brut, ou employer un `RefCell` pour exprimer l'emprunt exclusif de façon vérifiable.

---

### P1-C · Physmap boot ne couvre pas la SSR ExoPhoenix

**Fichiers concernés** :  
- `kernel/src/exophoenix/ssr.rs` — `SSR_BASE_PHYS`  
- `kernel/src/main.rs` — boot PML4, PDPT, PD (1 GiB max)  
- `exo-phoenix-ssr` (crate externe) — `SSR_BASE_PHYS`

**Description**

La SSR (Shared State Region) ExoPhoenix est positionnée à une adresse physique fixe `SSR_BASE_PHYS` définie par la crate `exo_phoenix_ssr`. Si cette adresse est supérieure à 1 GiB (configuration typique d'un système dual-kernel avec mémoire partagée placée en haut de la DRAM), tout accès via `ssr_atomic()` → `phys_to_virt(SSR_BASE)` génère un `#PF` Ring 0 non récupérable avant que `map_physmap()` soit appelé — ce qui, en l'état actuel (voir P0-A), ne se produit jamais.

Même après correction de P0-A, la SSR doit être mappée **avant** toute activation des APs SMP, or `map_physmap()` est appelé après `init_apic_system()`.

**Correction requise**

Mapper explicitement la page SSR dans le trampoline de boot (section `.boot_pd_high` existante) ou s'assurer que `map_physmap()` précède `arch_boot_init()` dans la séquence.

---

### P1-D · `USER_ELF_BASE_MIN = 0x100_0000_0000` (1 TiB) incompatible avec certains ELF PIE

**Fichiers concernés** :  
- `kernel/src/fs/elf_loader_impl.rs` — `const USER_ELF_BASE_MIN: u64 = 0x0000_0100_0000_0000`

**Description**

L'ELF loader refuse de charger tout segment dont l'adresse virtuelle est inférieure à 1 TiB :

```rust
if start < USER_ELF_BASE_MIN {
    return Err(ElfLoadError::InvalidElf);
}
```

Les binaires ELF statiques compilés avec une adresse de base standard Linux (`0x400000`, soit 4 MiB) ou les binaires PIE dont le linker produit un `PT_LOAD` commençant à `0x0` (pour relocation dynamique) sont rejetés avec `InvalidElf`.

L'espace `USER_START = 0x10000` à `USER_ELF_BASE_MIN = 0x100_0000_0000` est entièrement inutilisable. Les binaires `coreutils` et `exosh` embarqués dans `userspace/` peuvent être affectés selon leur configuration de linker.

**Correction requise**

Abaisser `USER_ELF_BASE_MIN` à `0x0000_0000_0040_0000` (4 MiB, standard ELF x86_64) ou à `USER_START` si les binaires sont compilés sans adresse fixe. Si un espace de garde bas est voulu, utiliser `0x0000_0000_0100_0000` (16 MiB) comme compromis.

---

### P1-E · Absence de writeback périodique automatique des blobs dirty

**Fichiers concernés** :  
- `kernel/src/fs/exofs/mod.rs` — `fn exofs_gc_kthread()`  
- `kernel/src/fs/exofs/cache/blob_cache.rs` — `fn collect_dirty()`  
- `kernel/src/syscall/fs_bridge.rs` — `fn fs_sync()`

**Description**

Le kthread de fond `exofs-gc` exécute uniquement le GC deux phases (scan + collect) sur les epochs âgées. Il n'appelle jamais `collect_dirty()` ni `persist_blob_data_if_disk()`. La persistance sur disque des données écrites via `vfs_write()` n'est déclenchée que par un appel userspace explicite à `sync()` ou `fsync()`.

En l'absence de ce mécanisme d'auto-flush, une panne système (crash kernel, extinction brutale) entre deux appels `sync()` entraîne la perte de toutes les données écrites dans l'intervalle. L'implémentation actuelle de `flush_all()` retourne même `ExofsError::DirtyDataLoss` si des données non flushées existent, ce qui prouve que le cas est considéré comme une erreur interne.

**Impact** : Perte de données garantie en cas de crash non planifié. Incompatible avec un système stable v0.2.0.

**Correction requise**

Ajouter un kthread `exofs-writeback` indépendant du GC, qui tourne à intervalle configurable (défaut : 5 s) et appelle `fs_sync()` ou l'équivalent kernel-side. Alternativement, intégrer le writeback dirty dans le kthread GC existant après chaque cycle GC.

---

## 4. P2 — Défauts mineurs (dégradation silencieuse)

### P2-A · Physmap par pages 4 KiB — performance dégradée pour > 256 MiB de RAM

**Fichier** : `kernel/src/memory/virtual/page_table/builder.rs` — `fn map_physmap()`

La fonction `map_physmap()` mappe chaque page de RAM physique individuellement en 4 KiB. Pour un système avec 4 GiB de RAM, cela représente 1 048 576 entrées PT à allouer et à remplir lors du boot, consommant ~8 MiB de tables de pages supplémentaires et allongeant significativement le temps de démarrage. La solution standard consiste à utiliser des huge-pages 2 MiB (ou 1 GiB si `CPUID.PDPE1GB` est disponible) pour la physmap.

**Correction** : Modifier `map_physmap()` pour utiliser `FLAG_HUGE` sur les entrées PD (2 MiB) dès que la plage est alignée, en retombant sur 4 KiB uniquement pour les fragments aux extrémités.

---

### P2-B · `should_enable_kpti()` retourne `false` pour AMD sans vérification `rdcl_no`

**Fichier** : `kernel/src/memory/virtual/page_table/kpti_split.rs` — `fn should_enable_kpti()`

```rust
CpuVendor::Amd => false,
```

Les processeurs AMD pré-Zen 3 sont vulnérables à des variantes de Meltdown (CVE-2021-26318 et dérivés). Le retour inconditionnel `false` pour tout vendeur AMD désactive KPTI sur ces CPUs. La vérification correcte pour AMD devrait consulter le bit `RDCL_NO` dans `MSR_AMD64_DE_CFG` ou le bit `NOREPLAY` dans `CPUID Fn8000_0008_EBX[bit 6]`.

**Correction** : Implémenter la vérification AMD symétrique à Intel (`rdcl_no()`) avant de désactiver KPTI.

---

### P2-C · `process::init()` existe mais n'est jamais appelée

**Fichiers** : `kernel/src/process/mod.rs` — `pub unsafe fn init()` / `kernel/src/lib.rs` — Phase 4

La fonction `process::init()` orchestre correctement toutes les sous-initialisations en séquence, y compris `cgroup::init()`. La Phase 4 de `kernel_init()` la remplace par des appels manuels fragmentés en omettant `cgroup::init()` (voir P0-B). Cette duplication crée un risque de divergence future : toute modification de `process::init()` ne sera pas répercutée dans `kernel_init()`.

**Correction** : Remplacer les appels fragmentés de la Phase 4 par un unique appel à `crate::process::init(&ProcessInitParams { ... })` pour garantir la cohérence.

---

### P2-D · Instrumentation `ARCH_INITIALIZED` déclarée mais jamais positionnée à `true`

**Fichier** : `kernel/src/arch/x86_64/mod.rs`

```rust
#[allow(dead_code)]
static ARCH_INITIALIZED: AtomicBool = AtomicBool::new(false);
```

L'attribut `#[allow(dead_code)]` confirme que cette sentinelle n'est jamais lue ni écrite. Aucun garde-fou ne protège les sous-systèmes arch contre une double initialisation ou une utilisation avant initialisation. À comparer avec `EXOFS_INITIALIZED` et `BRIDGE_INITIALIZED` qui sont correctement utilisés dans leurs modules respectifs.

**Correction** : Positionner `ARCH_INITIALIZED.store(true, Ordering::Release)` à la fin de `arch_boot_init()` et vérifier ce flag dans les appels AP pour prévenir toute initialisation hors séquence.

---

## 5. P3 — Incohérences architecturales (dette technique)

### P3-A · Deux définitions de message IPC non réconciliées entre kernel et userspace ABI

**Contexte** :

| Couche | Struct | Payload max | Fichier |
|---|---|---|---|
| Kernel ring | `IpcFastMsg` | 240 B | `ipc/core/transfer.rs` |
| Kernel builder | `IpcMessage` | 4 096 B | `ipc/message/builder.rs` |
| Syscall ABI | `IpcMessage` | 120 B | `syscall_abi/src/lib.rs` |
| Network server | `NetMsg` | 48 B (struct fixe) | `network_server/protocol.rs` |

Quatre définitions de "message IPC" coexistent sans documentation explicitant leur relation. Les serveurs Ring1 utilisent la définition `syscall_abi` (120 B de payload, 128 B total) tandis que le kernel impose une limite ring de 240 B. Le builder kernel-side propose 4 096 B mais cela tronque silencieusement via le ring.

**Recommandation** : Documenter explicitement dans `ipc/core/constants.rs` la relation entre les quatre niveaux, avec des `const_assert` vérifiant les invariants de compatibilité.

---

### P3-B · `exo_shield` déclaré `critical = true` mais dépend de services `critical = false`

**Contexte** : `exo_shield` est la couche de sécurité runtime (CET Shadow Stack, IBT, PKS). Sa criticité `true` implique que son échec au boot est censé bloquer le démarrage. Pourtant, ses dépendances `network_server` et `scheduler_server` sont `critical = false`, signifiant que leur échec est toléré par le système. Cette asymétrie crée une incohérence logique : un service critique ne peut pas dépendre de services optionnels sans mécanisme de dégradation gracieuse.

**Recommandation** : Soit abaisser `exo_shield` à `critical = false` (protection optionnelle), soit remonter ses dépendances à `critical = true` avec un timeout plus long.

---

### P3-C · La séquence `kernel_init()` contient des commentaires de séquence obsolètes

**Contexte** : Les commentaires de `scheduler/mod.rs` documentent 11 étapes d'initialisation dont l'étape 6 (`timer::tick::init(HZ=1000)`) est fusionnée dans l'étape 5 dans le code réel. La numérotation des étapes dans `lib.rs` (Phase 2a → Phase 7) diffère de celle de `scheduler/mod.rs` (étapes 1..11), de `process/mod.rs` (étapes 1..6) et de la doc `DOC2`/`DOC3` référencée en commentaire mais absente du dépôt audité.

L'absence de fichiers `docs/refonte.md`, `DOC1`, `DOC2`, `DOC3` dans le ZIP rend toute vérification de conformité impossible. Ces documents sont référencés à 23 reprises dans les sources.

**Recommandation** : Inclure les documents de référence dans le dépôt ou les convertir en commentaires de module rustdoc. Renuméroter les étapes d'init de façon cohérente et unique.

---

## 6. Matrice de risque v0.2.0

| ID | Sévérité | Probabilité de déclenchement | Impact | Statut |
|---|---|---|---|---|
| P0-A | CRITIQUE | Haute (dès `-m 2G`) | Panic Ring 0 | ✗ Non corrigé |
| P0-B | CRITIQUE | Haute (toujours) | Cgroup invalid | ✗ Non corrigé |
| P0-C | CRITIQUE | Moyenne | Corruption payload IPC | ✗ Non corrigé |
| P0-D | CRITIQUE | Haute (sans virtio-net) | Pas de shell | ✗ Non corrigé |
| P1-A | MAJEUR | Faible (kernel-side builder) | Troncature silencieuse | ✗ Non corrigé |
| P1-B | MAJEUR | Faible (smoltcp nested) | UB aliasing | ✗ Non corrigé |
| P1-C | MAJEUR | Moyenne (SSR > 1 GiB) | Panic PhoenixState | ✗ Non corrigé |
| P1-D | MAJEUR | Haute (binaires std) | ELF rejeté | ✗ Non corrigé |
| P1-E | MAJEUR | Haute (tout crash) | Perte de données | ✗ Non corrigé |
| P2-A | MINEUR | Haute (> 256 MiB) | Boot lent | ✗ Non corrigé |
| P2-B | MINEUR | Moyenne (AMD pré-Zen3) | KPTI désactivé à tort | ✗ Non corrigé |
| P2-C | MINEUR | Certaine | Dette technique | ✗ Non corrigé |
| P2-D | MINEUR | Faible | Sentinelle morte | ✗ Non corrigé |
| P3-A | DETTE | N/A | Maintenabilité | ✗ Non documenté |
| P3-B | DETTE | N/A | Incohérence logique | ✗ Non documenté |
| P3-C | DETTE | N/A | Docs manquantes | ✗ Non documenté |

---

## 7. Plan de correction recommandé

### Sprint 1 — Débloquants (1 semaine)

1. **P0-B** : Ajouter `cgroup::init()` dans `kernel_init()` Phase 4 → 5 lignes de code.
2. **P2-C** : Remplacer les appels fragmentés Phase 4 par `process::init()` → refactoring mécanique.
3. **P0-D** : Introduire `requires_optional` dans `ServiceMetadata` et déplacer `network_server` et `scheduler_server` hors des dépendances bloquantes de `exo_shield`.

### Sprint 2 — Stabilisation mémoire (1 semaine)

4. **P0-A** : Implémenter l'appel à `map_physmap()` avec huge-pages 2 MiB dans `init_memory_subsystem_*()`.
5. **P2-A** : Modifier `map_physmap()` pour utiliser FLAG_HUGE sur les plages alignées.
6. **P1-C** : Mapper la SSR ExoPhoenix avant `init_apic_system()` ou inclure son adresse dans le boot PD.

### Sprint 3 — Robustesse IPC et ABI (1 semaine)

7. **P0-C** : Déplacer `sender_pid` dans l'en-tête IPC, supprimer la condition `len == 128`.
8. **P1-A** : Aligner `MAX_MSG_INLINE` sur `MAX_MSG_SIZE`, ajouter `const_assert`.
9. **P3-A** : Documenter les quatre niveaux de message IPC avec `const_assert` croisés.

### Sprint 4 — Persistance et sécurité (1 semaine)

10. **P1-E** : Créer le kthread `exofs-writeback` avec flush périodique (5 s par défaut).
11. **P1-D** : Abaisser `USER_ELF_BASE_MIN` à `0x400000`.
12. **P2-B** : Implémenter la vérification `RDCL_NO` pour AMD dans `should_enable_kpti()`.
13. **P2-D** : Activer `ARCH_INITIALIZED` et l'utiliser dans les paths AP.

### Sprint 5 — Nettoyage et documentation (0.5 semaine)

14. **P1-B** : Corriger l'aliasing `*mut ExoNetDevice` dans le serveur réseau.
15. **P3-B** : Résoudre l'asymétrie criticité `exo_shield` vs ses dépendances.
16. **P3-C** : Intégrer ou réécrire les références `DOC1`/`DOC2`/`DOC3` en rustdoc inline.

---

*Rapport généré par claude-beta — Audit statique complet du commit v0.1.0 (1 334 fichiers, ~12 MiB de sources Rust). Toute correction doit être accompagnée d'un test de régression ciblant le chemin de défaut identifié.*
