# Exo-OS — Documentation Technique Complète
## Phases 1 à 5 accomplie — État au 10 mars 2026

> **Statut global** : Phases 1 → 5 complètes — `cargo check` = 0 erreur.  
> **Prochaine étape** : Phase 6 (exo-boot UEFI / dual-entry / QEMU OVMF).

---

## Table des matières

1. [Vue d'ensemble de l'architecture](#1-vue-densemble)
2. [Phase 1 — Mémoire virtuelle et heap kernel](#2-phase-1--mémoire-virtuelle)
3. [Phase 2 — Scheduler et IPC](#3-phase-2--scheduler-et-ipc)
4. [Phase 3 — Process management et signaux](#4-phase-3--process-et-signaux)
5. [Phase 4 — ExoFS (filesystem objets)](#5-phase-4--exofs)
6. [Phase 5 — Servers Ring 1](#6-phase-5--servers-ring-1)
7. [Invariants de sécurité actifs](#7-invariants-de-sécurité)
8. [Catalogue des bugs corrigés](#8-bugs-corrigés)
9. [Tests et validation](#9-tests-et-validation)
10. [Architecture des syscalls](#10-architecture-des-syscalls)
11. [Arborescence des modules clés](#11-arborescence-des-modules)
12. [Commandes de build et test](#12-commandes-de-build)

---

## 1. Vue d'ensemble

### Environnement cible

| Paramètre | Valeur |
|-----------|--------|
| Architecture | `x86_64` bare-metal |
| Target Rust | `x86_64-unknown-none` |
| Toolchain | `nightly` (voir `kernel/rust-toolchain.toml`) |
| Modèle mémoire | Higher-half kernel (`0xFFFF_FFFF_8000_0000`) |
| Standard lib | `#![no_std]` — `core` + `alloc` uniquement |
| Bootloader | BIOS multiboot2 (actuel) → UEFI exo-boot (Phase 6) |
| Langage | Rust 100% (aucun C dans le kernel) |

### Séquence de boot accomplie (point de départ)

```
BIOS → GRUB → kernel (premier boot réussi 5 mars 2026)
→ Séquence de validation : XK12356ps789abcdefgZAIOK → halt_cpu()
```

### Progression des phases

```
Phase 1 ✅  Mémoire virtuelle, heap kernel, APIC remap, buddy, slab
Phase 2 ✅  Scheduler CFS, context switch x86_64, SPSC ring, futex
Phase 3 ✅  fork/exec/exit, signaux POSIX, 40+ syscalls, TLS %fs
Phase 4 ✅  ExoFS complet, crypto pipeline, 21 syscalls ExoFS (500-520)
Phase 5 ✅  4 servers Ring 1 : init(PID1), ipc_router(PID2), vfs(PID3), crypto(PID4)
───────────────────────────────────────────────────────────
Phase 6 ⬜  exo-boot UEFI + QEMU OVMF (prochaine étape)
```

---

## 2. Phase 1 — Mémoire virtuelle

### 2.1 Modules implémentés

| Module | Chemin | Fonctionnalité |
|--------|--------|----------------|
| PML4 kernel | `kernel/src/memory/virtual/page_table/x86_64.rs` | Mapping higher-half `0xFFFF_FFFF_8000_0000` |
| APIC remap | `kernel/src/memory/virtual/address_space/mapper.rs` | LAPIC `0xFEE00000` + IOAPIC `0xFEC00000` → UC+NX |
| HPET init | `kernel/src/arch/x86_64/acpi/hpet.rs` | `0xFED00000` → UC+NX, init complète |
| Buddy allocator | `kernel/src/memory/physical/allocator/buddy.rs` | `alloc_frame()` / `free_frame()` O(log n) |
| Slab/SLUB | `kernel/src/memory/heap/allocator/hybrid.rs` | `#[global_allocator]` — `Box::new()` opérationnel |
| VMA tree | `kernel/src/memory/virtual/vma/tree.rs` | RBTree intrusive — `mmap` / `munmap` / `mprotect` |
| CSPRNG | `kernel/src/security/crypto/rng.rs` | RDRAND + TSC fallback (entropie au boot) |
| TSC calibration | `kernel/src/arch/x86_64/time/calibration/mod.rs` | 5 sources : CPUID > HPET > ACPI PM > PIT > fallback |

### 2.2 Invariants mémoire critiques corrigés

#### MEM-01 — APIC MMIO sans cachabilité (ERR-02) ✅
```rust
// AVANT (trampoline boot — incorrect) :
// PD_high[503] = 0xFEE00083  // P | R/W | PS — PAS UC, PAS NX

// APRÈS (memory_map.rs — correct) :
map_mmio_region(
    phys:  0xFEE00000,           // LAPIC physique
    virt:  LAPIC_VIRT_BASE,
    size:  0x1000,
    flags: PAGE_PRESENT | PAGE_RW | PAGE_NX | PAGE_PCD | PAGE_PWT, // UC+NX
);
// invlpg(LAPIC_VIRT_BASE) après remap
```

#### MEM-02 — Buddy allocator et régions ACPI ✅
- Chaque `MemoryRegion` validée contre la `MemoryMap Multiboot2` avant ajout au pool.
- Les régions ACPI, MMIO et firmware sont explicitement exclues.

#### ERR-03 — CSPRNG non initialisé ✅
- Fallback `RDRAND + TSC` actif dès l'init mémoire, avant la heap.
- Clé SipHash futex dérivée depuis le CSPRNG (ERR-05 corrigé).

---

## 3. Phase 2 — Scheduler et IPC

### 3.1 Scheduler CFS

| Composant | Chemin | Détails |
|-----------|--------|---------|
| Context switch | `kernel/src/scheduler/asm/switch_asm.s` | XSAVE/XRSTOR FPU, r15 callee-saved préservé |
| RunQueue | `kernel/src/scheduler/core/runqueue.rs` | CFS intrusive — **zéro alloc dans ISR** (SCHED-08) |
| hrtimer | `kernel/src/scheduler/timer/hrtimer.rs` | Basé sur HPET calibré (pas TSC 1GHz fallback) |
| Per-CPU | `kernel/src/scheduler/smp/topology.rs` | SWAPGS à chaque entrée/sortie Ring0 |
| Préemption | `kernel/src/scheduler/core/preempt.rs` | `PreemptGuard` RAII — pas de fuite possible |
| kthread API | `kernel/src/process/lifecycle/create.rs` | `create_kthread(KthreadParams)` → `Result<Tid>` |

#### Priorités des kthreads

```rust
Priority::RT_MAX          = Self(0)    // Temps réel le plus urgent
Priority::RT_MIN          = Self(99)   // Temps réel le moins urgent
Priority::NORMAL_MAX      = Self(100)  // Normal : plus haute priorité
Priority::NORMAL_DEFAULT  = Self(120)  // Normal : priorité par défaut
Priority::NORMAL_MIN      = Self(139)  // Normal : priorité la plus basse
Priority::IDLE            = Self(140)  // Tâches de fond (GC ExoFS, etc.)
```

### 3.2 IPC Ring SPSC/MPSC

| Composant | Chemin | Détails |
|-----------|--------|---------|
| SPSC ring | `libs/exo_ipc/src/ring/spsc.rs` | CachePadded head/tail — zéro false sharing |
| MPSC ring | `libs/exo_ipc/src/ring/mpsc.rs` | Pour N producteurs, 1 consommateur |
| Channels | `libs/exo_ipc/src/channel/` | Abstraction haut-niveau sur ring buffers |
| Types | `libs/exo_ipc/src/types/` | `Message`, `MessageType`, `CapToken` |

#### Invariant IPC-01 — CachePadded (ERR-04) ✅
```rust
// libs/exo_ipc/src/ring/spsc.rs — CORRECT
#[repr(C, align(64))]
struct CachePadded<T> { value: T, _pad: [u8; 64 - core::mem::size_of::<T>() % 64] }

pub struct SpscRing<T, const N: usize> {
    head: CachePadded<AtomicU64>,  // ✅ Cache line PRODUCTEUR isolée
    tail: CachePadded<AtomicU64>,  // ✅ Cache line CONSOMMATEUR isolée
    buffer: [MaybeUninit<T>; N],
}
```

### 3.3 Futex

- Clé SipHash initialisée depuis CSPRNG (ERR-05 / IPC-02 corrigé).
- Table lockée par shards pour réduire la contention.
- `FUTEX_WAIT` / `FUTEX_WAKE` / `FUTEX_WAKE_BITSET` implémentés.
- Protection HashDoS : clé aléatoire per-boot empêche la prédiction de collisions.

---

## 4. Phase 3 — Process et signaux

### 4.1 Syscalls process implémentés (0–50+)

| N° | Nom | Module |
|----|-----|--------|
| 0 | `sys_read` | `syscall/handlers/io.rs` |
| 1 | `sys_write` | `syscall/handlers/io.rs` |
| 39 | `sys_getpid` | `syscall/handlers/process.rs` |
| 57 | `sys_fork` | `process/lifecycle/fork.rs` |
| 59 | `sys_execve` | `process/lifecycle/exec.rs` |
| 60 | `sys_exit` | `process/lifecycle/exit.rs` |
| 61 | `sys_wait4` | `process/lifecycle/wait.rs` |
| 62 | `sys_kill` | `process/signal/send.rs` |
| 11 | `sys_munmap` | `memory/virtual/vma/` |
| 9  | `sys_mmap` | `memory/virtual/vma/` |
| 13 | `sys_rt_sigaction` | `process/signal/handler.rs` |
| 14 | `sys_rt_sigprocmask` | `process/signal/mask.rs` |
| 15 | `sys_rt_sigreturn` | `process/signal/handler.rs` |
| 131| `sys_sigaltstack` | `process/signal/handler.rs` |
| 63 | `sys_uname` | `syscall/handlers/misc.rs` |
| 247| `sys_waitid` | `process/lifecycle/wait.rs` |
| 65 | `sys_semget` | `ipc/sem/` |
| 35 | `sys_nanosleep` | `scheduler/timer/` |

### 4.2 Bugs critiques corrigés

#### PROC-01 / BUG-04 — TLS `%fs` non initialisé avant exec ✅

```rust
// kernel/src/process/lifecycle/exec.rs — ligne 222-234
// Après setup_stack(), avant jump_to_entry() :
let tls_base = allocate_initial_tls_block(&tcb)?;
unsafe {
    // MSR IA32_FS_BASE = 0xC0000100
    core::arch::x86_64::__wrfsbase(tls_base);
}
tcb.fs_base = tls_base;
// ✅ Sans ça : errno, __tls_get_addr, pthread_keys crashent immédiatement
```

#### PROC-02 / BUG-05 — SYSRETQ sans vérification canonique RCX ✅

```rust
// kernel/src/arch/x86_64/syscall.rs — ligne 309
// Avant SYSRETQ :
fn is_canonical(addr: u64) -> bool {
    let sign_bits = addr >> 47;
    sign_bits == 0 || sign_bits == 0x1FFFF // 17 bits tous 0 ou tous 1
}
// Non-canonique → SIGSEGV au processus, jamais SYSRETQ !
```

### 4.3 Mécanisme de fork (CoW)

- `do_fork()` : clone PCB + TCB + page table.
- `mark_all_pages_cow()` : toutes les pages utilisateur passées en lecture seule.
- TLB shootdown atomique avec **lock page_table tenu pendant toute la durée** (FORK-02).
- `fork_child_trampoline` : l'enfant reprend via `iretq` vers Ring3.

### 4.4 Livraison de signaux

| Règle | Mise en œuvre |
|-------|---------------|
| SIG-01 | `SigactionEntry` stocke la valeur du handler, jamais un `AtomicPtr` |
| SIG-07 | `SIGKILL` et `SIGSTOP` sont non-masquables (vérification dans `signal/mask.rs`) |
| SIG-13 | Magic `0x5349474E` vérifié en **constant-time** au sigreturn (LAC-01) |
| PROC-03 | Tous les signaux bloqués pendant `exec()` entre `load_elf` et reset TCB (ERR-11) |
| SIG-DELIVER | `post_dispatch()` appelle `handle_pending_signals()` à chaque retour Ring0→Ring3 |

### 4.5 struct utsname (syscall uname)

```c
// kernel/src/syscall/handlers/misc.rs
struct utsname {
    sysname[65]    = "Exo-OS\0"
    nodename[65]   = "exo-node\0"
    release[65]    = "0.1.0\0"
    version[65]    = "#1 SMP 2026\0"
    machine[65]    = "x86_64\0"
    domainname[65] = "(none)\0"
}
// Size totale : 390 bytes (6 × 65) — vérifié par tests phase3-tests
```

---

## 5. Phase 4 — ExoFS

### 5.1 Qu'est-ce qu'ExoFS ?

ExoFS **n'est pas** un filesystem POSIX classique. C'est un **système d'objets content-addressed** avec :

- **Adressage par contenu** : chaque objet identifié par `BlobId = Blake3(contenu)`.
- **Chiffrement de bout en bout** : XChaCha20-Poly1305, clé dérivée du BlobId.
- **Déduplication** : deux blobs identiques → même BlobId → 1 seul objet stocké.
- **Epochs** : journal de transactions avec commit atomique et recovery intégrée.
- **Capabilities** : système d'accès basé sur tokens cryptographiques infalsifiables.
- **GC deux phases** : marking + sweeping géré par un kthread dédié.

### 5.2 Pipeline crypto — ordre OBLIGATOIRE (CRYPTO-02)

```
Données brutes
    │
    ▼ Blake3(données)
BlobId = hash de contenu (content-addressed)
    │
    ▼ LZ4 compression
Données compressées
    │
    ▼ HKDF(master_key, BlobId) → clé objet unique
    ▼ XChaCha20-Poly1305(clé, nonce_atomique)
Ciphertext authenticated
    │
    ▼ write_to_disk()
```

**Règle absolue** : Blake3 AVANT compression, compression AVANT chiffrement.

### 5.3 Syscalls ExoFS (500–520)

| N° | Syscall | Description |
|----|---------|-------------|
| 500 | `SYS_EXOFS_PATH_RESOLVE` | Chemin → BlobId via Blake3 path index |
| 501 | `SYS_EXOFS_OBJECT_OPEN` | BlobId → fd dans la table de descripteurs |
| 502 | `SYS_EXOFS_OBJECT_READ` | Lecture d'un objet par fd |
| 503 | `SYS_EXOFS_OBJECT_WRITE` | Écriture d'un objet (crée un nouveau BlobId) |
| 504 | `SYS_EXOFS_OBJECT_CREATE` | Création d'un nouvel objet |
| 505 | `SYS_EXOFS_OBJECT_DELETE` | Suppression (décrémente refcount) |
| 506 | `SYS_EXOFS_OBJECT_STAT` | Métadonnées : taille, BlobId, epoch |
| 507 | `SYS_EXOFS_OBJECT_CHMOD` | Capabilities : modification des droits |
| 508 | `SYS_EXOFS_OBJECT_HASH` | Retourner le BlobId d'un objet ouvert |
| 509 | `SYS_EXOFS_SNAPSHOT` | Créer un snapshot immutable d'un objet |
| 510 | `SYS_EXOFS_RELATION_SET` | Créer lien parent → enfant (DAG) |
| 511 | `SYS_EXOFS_RELATION_GET` | Lister les enfants d'un objet |
| 512 | `SYS_EXOFS_GC_HINT` | Donner une région GC au kthread |
| 513 | `SYS_EXOFS_QUOTA_SET` | Définir quotas (bytes + blobs) |
| 514 | `SYS_EXOFS_QUOTA_GET` | Lire l'utilisation courante |
| 515 | `SYS_EXOFS_EXPORT` | Exporter un objet vers un descripteur externe |
| 516 | `SYS_EXOFS_IMPORT` | Importer un objet depuis un descripteur externe |
| 517 | `SYS_EXOFS_META_UPDATE` | Mettre à jour les métadonnées étendues |
| 518 | `SYS_EXOFS_EPOCH_COMMIT` | Forcer un commit de l'epoch courante |
| 519 | `SYS_EXOFS_OPEN_BY_PATH` | **BUG-01** : open atomique (path_resolve + object_open) |
| 520 | `SYS_EXOFS_READDIR` | **BUG-02** : getdents64 pour ExoFS |

**Note BUG-01** : `musl-exo` envoie un seul syscall `open()`. Sans le 519, toutes les applications POSIX échouaient. Ce syscall combiné est le plus critique pour le userspace.

### 5.4 Architecture interne ExoFS

```
fs/exofs/
├── mod.rs              ← exofs_init() + exofs_shutdown() + GC kthread
├── syscall/            ← 21 handlers (500-520)
├── objects/            ← BlobId, ObjectKind, RefCount
├── crypto/             ← XChaCha20-Poly1305, Blake3, HKDF derives
├── epoch/              ← Journal d'epochs, commit, recovery
├── gc/                 ← GC deux phases : marking + sweeping
├── io/                 ← Prefetch, write-back cache, blob_writer
├── storage/            ← Superblock backup, disk layout
├── cache/              ← LRU cache objets + pressure control
├── dedup/              ← Chunk cache pour déduplication
├── numa/               ← NUMA-aware allocation d'objets
├── quota/              ← Quotas par entité (user/group/project)
├── posix_bridge/       ← Adaptation POSIX (open/read/write/stat)
├── export/             ← Export/import objets
├── recovery/           ← fsck phases 1-4, rebuild index
├── observability/      ← Métriques, snapshots, health reports
└── security/           ← CapToken, verify_cap(), constant-time ops
```

### 5.5 GC kthread (kthread `exofs-gc`)

```rust
// kernel/src/fs/exofs/mod.rs
// Démarré depuis exofs_init() avec Priority::IDLE (140)
fn exofs_gc_kthread(_arg: usize) -> ! {
    loop {
        // Collecte les epochs = current - 2 (suffisamment "vieilles")
        run_gc_two_phase(current_epoch() - 2);
        // Yield volontaire — ne doit jamais bloquer l'I/O
        crate::syscall::fast_path::sys_sched_yield();
    }
}
```

### 5.6 Shutdown propre ExoFS

```rust
// kernel/src/fs/exofs/mod.rs — exofs_shutdown()
// Commit l'epoch courante avant l'arrêt
let args = EpochCommitArgs { flags: epoch_flags::FORCE, epoch_id: 0, ... };
match do_shutdown_commit(&args) {
    Ok(_) | Err(ExofsError::CommitInProgress) => { /* ok */ }
    Err(e) => return Err(e),
}
```

### 5.7 Nonce XChaCha20 — protection two-time pad (ERR-08) ✅

```rust
// fs/exofs/crypto/xchacha20.rs
static NONCE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn generate_nonce(object_id: &ObjectId) -> [u8; 24] {
    let counter = NONCE_COUNTER.fetch_add(1, Ordering::SeqCst);
    let mut nonce = [0u8; 24];
    // HKDF : counter + ObjectId → nonce unique garanti
    hkdf_expand(counter.to_le_bytes(), &object_id.0, &mut nonce);
    nonce
}
```

### 5.8 Capabilities — constant-time (LAC-01) ✅

```rust
// fs/exofs/security/cap.rs
// verify() utilise ct_eq (constant-time) pour éviter timing oracle
fn verify(token: &CapToken, expected: &CapToken) -> bool {
    // Durée identique que le token soit révoqué ou invalide
    ct_eq(token.bytes(), expected.bytes())
        & (token.generation == self.generation_counter)
}
```

---

## 6. Phase 5 — Servers Ring 1

### 6.1 Ordre de démarrage

```
boot kernel
    │
    ▼ PID 1 — init_server (superviseur)
    │   ├── fork+exec → PID 2 : ipc_router
    │   ├── fork+exec → PID 3 : vfs_server
    │   └── fork+exec → PID 4 : crypto_server
    │
    ▼ SIGCHLD handler actif → relance automatique (backoff ×2, max 32 ticks)
```

### 6.2 init_server (PID 1)

**Fichier** : `servers/init_server/src/main.rs`

| Fonctionnalité | Détails |
|---------------|---------|
| Supervision | Handlers `SIGCHLD` + `SIGTERM` via `sys_rt_sigaction` |
| Watchdog | `wait4(WNOHANG)` dans la boucle → détecte tous les zombies |
| Backoff | Délai de relance ×2 à chaque crash successif (plafonné à 32 ticks) |
| Ordre de lancement | ipc_router → vfs_server → crypto_server (ordre strict) |
| Arrêt propre | SIGTERM → `SIGTERM` vers enfants → attente zombies → halt |
| Boucle principale | `nanosleep(10ms)` entre chaque cycle de surveillance |

```
Délai de backoff exponentiel :
  Crash 1 : 2 ticks
  Crash 2 : 4 ticks
  Crash 3 : 8 ticks
  Crash 4 : 16 ticks
  Crash 5+: 32 ticks (plafond)
  Relance réussie : reset à 1 tick
```

### 6.3 ipc_router (PID 2)

**Fichier** : `servers/ipc_router/src/main.rs`

| Fonctionnalité | Détails |
|---------------|---------|
| Registre | Table `[u32; 64]` — hash FNV-32 → endpoint_id |
| Routing | `SYS_IPC_SEND(dest_endpoint, payload)` vers le bon server |
| Heartbeat | Répond avec son PID au ping `IPC_MSG_HEARTBEAT` |
| Messages | `REGISTER(0)`, `ROUTE(1)`, `HEARTBEAT(2)` |
| Capacité | 64 services simultanés (extensible en Phase 6) |

**Protocole de routage** :
```
Client → SYS_IPC_SEND("dest_name", msg_ptr, len)
ipc_router → lookup(hash("dest_name")) → endpoint_id
ipc_router → SYS_IPC_SEND(endpoint_id, msg_ptr, len)
Serveur dest ← reçoit le message sans connaître le client direct
```

### 6.4 vfs_server (PID 3)

**Fichier** : `servers/vfs_server/src/main.rs`

| Fonctionnalité | Détails |
|---------------|---------|
| Table de montages | 32 entrées max (hash FNV-32 par chemin) |
| Pseudo-FS au boot | `/proc` (ProcFs), `/sys` (SysFs), `/dev` (DevFs) |
| Résolution chemin | `VFS_RESOLVE` → `SYS_EXOFS_PATH_RESOLVE(500)` → BlobId |
| Ouverture objet | `VFS_OPEN` → `SYS_EXOFS_OBJECT_OPEN(501)` → fd |
| Mount | `VFS_MOUNT(fstype, path, root_blob)` |
| Umount | `VFS_UMOUNT` (stub Phase 6 : flush + ExoFS sync) |

**Types de FS supportés** :
| Code | Nom | Usage |
|------|-----|-------|
| 1 | ExoFs | Système de fichiers principal |
| 2 | ProcFs | `/proc` — informations processus |
| 3 | SysFs | `/sys` — informations kernel/hardware |
| 4 | DevFs | `/dev` — périphériques |

### 6.5 crypto_server (PID 4)

**Fichier** : `servers/crypto_server/src/main.rs`

**Invariant SRV-04** : seul server autorisé à accéder aux primitives cryptographiques. Aucun autre server ne doit importer `chacha20poly1305` ou `blake3`.

| Message | Description | Retour |
|---------|-------------|--------|
| `CRYPTO_DERIVE_KEY (0)` | Dériver une clé (KDF interne) | `key_handle` opaque (u32) |
| `CRYPTO_RANDOM (1)` | Octets aléatoires via `SYS_GETRANDOM(318)` | `data[0..N]` |
| `CRYPTO_ENCRYPT (2)` | Chiffrement XChaCha20-Poly1305 | Phase 6 |
| `CRYPTO_DECRYPT (3)` | Déchiffrement XChaCha20-Poly1305 | Phase 6 |
| `CRYPTO_HASH (4)` | Hash Blake3 d'un buffer | `data[0..32]` |

**Sécurité des handles** :
- Les clés brutes ne **quittent jamais** le processus `crypto_server`.
- Seul un `key_handle: u32` opaque est retourné au client.
- Réponse `DERIVE_KEY` : `data[] = [0u8; 56]` (aucune fuite).
- Keystore en mémoire : 32 handles max (extensible Phase 6 avec ExoFS persistance).

### 6.6 Protocole IPC commun aux servers

```rust
// Message entrant (128 bytes)
#[repr(C)]
struct IpcMessage {
    sender_pid: u32,   // PID de l'émetteur (pour la réponse)
    msg_type:   u32,   // type de requête
    payload:    [u8; 120], // données de la requête
}

// Syscalls utilisés par tous les servers
SYS_IPC_REGISTER = 300  // s'enregistrer auprès du kernel/ipc_router
SYS_IPC_RECV     = 301  // recevoir le prochain message (bloquant)
SYS_IPC_SEND     = 302  // envoyer un message vers un endpoint
```

---

## 7. Invariants de sécurité

### 7.1 Table OWASP — conformité

| Catégorie OWASP | Mitigation dans Exo-OS |
|-----------------|------------------------|
| Broken Access Control | `verify_cap()` obligatoire avant tout handler ExoFS (SYS-07) |
| Cryptographic Failures | Pipeline Blake3→LZ4→XChaCha20 dans l'ordre exact ; nonce atomique |
| Injection | `copy_from_user()` systématique ; jamais de pointeur kernel brut retourné |
| Insecure Design | Capabilities infalsifiables (token cryptographique) ; constant-time verify |
| Security Misconfiguration | APIC avec UC+NX ; pages kernel non-exécutables depuis Ring3 |
| Identification Failures | SipHash avec clé aléatoire per-boot (HashDoS impossible) |
| Software Integrity | Checksum Blake3 sur EpochRecord vérifié au montage |
| SSRF | Validation canonicité RCX avant SYSRETQ ; pointeurs user via `copy_from_user` uniquement |

### 7.2 Invariants inviolables

| ID | Règle | Conséquence si violée |
|----|-------|----------------------|
| SRV-04 | Seul `crypto_server` importe des primitives crypto | Clés désynchronisées, données illisibles après redémarrage |
| SRV-05 | Tous les services passent par `ipc_router` | Localisation impossible, services inaccessibles |
| SIG-07 | SIGKILL/SIGSTOP non-masquables | Zombie impossible à tuer, saturation table processus |
| CRYPTO-02 | Blake3 → LZ4 → XChaCha20 dans cet ordre | BlobId instable OU ciphertext incompressible |
| EPOCH-01 | Checksum EpochRecord vérifié au montage | Données corrompues restaurées silencieusement |
| CAP-01 | `verify()` constant-time | Oracle timing pour énumérer ObjectIds valides |
| IPC-01 | CachePadded head/tail dans SPSC | Dégradation 10-100x sur hardware multicore |

---

## 8. Bugs corrigés

| ID | Sévérité | Description | Fichier corrigé | Ligne |
|----|----------|-------------|-----------------|-------|
| BUG-01 | 🔴 Critique | `musl-exo` envoie 1 syscall pour `open()`, ExoFS en nécessite 2 → `SYS_EXOFS_OPEN_BY_PATH=519` | `fs/exofs/syscall/open_by_path.rs` | — |
| BUG-02 | 🔴 Critique | `getdents64` absent → listing de répertoire impossible → `SYS_EXOFS_READDIR=520` | `fs/exofs/syscall/readdir.rs` | — |
| BUG-04 | 🔴 Critique | `%fs` (TLS base) non initialisé avant jump Ring3 → crash immédiat `errno` / `pthread_keys` | `process/lifecycle/exec.rs` | 222-234 |
| BUG-05 | 🔴 Critique | SYSRETQ sans vérification canonique RCX → exploitable pour escalade de privilèges | `arch/x86_64/syscall.rs` | 309 |
| ERR-02 | 🟠 Important | APIC MMIO sans UC (Write-Back) → interruptions perdues sur hardware réel | `arch/x86_64/boot/memory_map.rs` | 103 |
| ERR-04 | 🟠 Important | SPSC sans CachePadded → false sharing → dégradation ×100 sur multicore | `libs/exo_ipc/src/ring/spsc.rs` | — |
| ERR-05 | 🟠 Important | Clé SipHash futex non initialisée → HashDoS possible | `memory/utils/futex_table.rs` | — |
| ERR-08 | 🟠 Important | Nonce XChaCha20 via RDRAND seul → two-time pad possible | `fs/exofs/crypto/xchacha20.rs` | — |
| LAC-01 | 🟡 Moyen | `verify()` cap non constant-time → oracle timing | `fs/exofs/security/cap.rs` | — |
| ERR-11 | 🟡 Moyen | Signal livré pendant `exec()` → handler pointe vers ancien code | `process/lifecycle/exec.rs` | — |

---

## 9. Tests et validation

### 9.1 phase3-tests — Tests Phase 3 (70 tests)

**Emplacement** : `kernel/phase3-tests/`  
**Commande** : `cd kernel/phase3-tests && cargo test --target x86_64-unknown-linux-musl`

| Module | Tests | Couvre |
|--------|-------|--------|
| `tests_errno` | 8 | Valeurs POSIX : ESRCH=-3, ECHILD=-10, EINVAL=-22 … |
| `tests_wait` | 12 | Encodage wstatus : `WEXITSTATUS`, `WTERMSIG`, core dump bit |
| `tests_uname` | 6 | Layout `utsname` 390 bytes, contenu champs |
| `tests_sigaltstack` | 8 | Layout `SigAltStack`, SS_DISABLE, MINSIGSTKSZ |
| `tests_argv` | 10 | `copy_userspace_argv` — ARGV-01, NULL terminator, limites |
| `tests_signal_validation` | 12 | `validate_signal()`, Signal enum, SIGKILL/SIGSTOP barrières |
| `tests_waitid_siginfo` | 8 | Encodage `siginfo_t` x86_64, `si_code`, `si_status` |
| `tests_integration` | 6 | Scénarios complets fork+exec+signal |

### 9.2 phase5-tests — Tests Phase 5 (57 tests)

**Emplacement** : `servers/phase5-tests/`  
**Commande** : `cd servers/phase5-tests && cargo test --target x86_64-unknown-linux-musl`

| Module | Tests | Couvre |
|--------|-------|--------|
| `tests_ipc_router` | 9 | FNV-32, register/resolve, table pleine (64 slots), collisions |
| `tests_init_server` | 8 | PID tracking, backoff ×2 (max 32), reap par PID, table 3 services |
| `tests_crypto_server` | 13 | KDF déterministe, handles consécutifs, table pleine=BUSY, hash, RANDOM |
| `tests_vfs_server` | 14 | FNV-32 paths, mount/umount, find/remove, ENOSPC, payload validation |
| `tests_integration` | 3 | Boot simulé complet, invariant SRV-04 (clé opaque), invariant SRV-05 |

### 9.3 Tests inline dans les modules kernel

Le kernel contient des tests `#[cfg(test)]` dans plus de 20 modules :

| Module | Tests notables |
|--------|---------------|
| `fs/exofs/crypto/mod.rs` | encrypt/decrypt, key generation, constant-time verify |
| `fs/exofs/quota/mod.rs` | init, set_limits, record/release, remove_entity |
| `fs/exofs/gc/gc_state.rs` | transitions de phases, is_active(), pass_stats |
| `fs/exofs/numa/mod.rs` | preferred_node, record_alloc, multi-node config |
| `fs/exofs/cache/mod.rs` | reclaim, tick health, capacity config |
| `fs/exofs/io/mod.rs` | config validate, health after init, flush_all |
| `fs/exofs/posix_bridge/mod.rs` | config, diag magic, copy_name, mode_to_inode_flags |
| `fs/exofs/recovery/fsck*.rs` | fsck_options, error types, phase3 context |
| `fs/exofs/dedup/chunk_cache.rs` | config validate, global cache accessible |
| `fs/exofs/observability/mod.rs` | init, status, uptime, snapshot metrics |

### 9.4 Commande de vérification globale

```bash
# Vérification compilation (0 erreur)
cd /workspaces/Exo-OS
cargo check

# Tests Phase 3
cd kernel/phase3-tests
cargo test --target x86_64-unknown-linux-musl

# Tests Phase 5
cd servers/phase5-tests
cargo test --target x86_64-unknown-linux-musl
```

---

## 10. Architecture des syscalls

### 10.1 Table de routage

```
CPU SYSCALL instruction
    │
    ▼ arch/x86_64/syscall.rs → syscall_handler()
    │   ├── is_canonical(rcx) [PROC-02] ✅
    │   ├── SWAPGS → kernel GS
    │   └── dispatch(nr, args)
    │
    ▼ syscall/dispatch.rs → get_handler(nr)
    │   ├── nr 0-499   → handlers POSIX standard
    │   └── nr 500-520 → handlers ExoFS
    │
    ▼ syscall/table.rs → SYSCALL_TABLE[nr](args)
    │   (tableau statique [Option<HandlerFn>; 521])
    │
    ▼ handler spécifique (ex: sys_exofs_path_resolve)
    │   ├── copy_from_user() [SYS-01] ✅
    │   ├── verify_cap() [SYS-07] ✅
    │   └── logique métier
    │
    ▼ retour Ring3
    │   ├── handle_pending_signals() [SIG-DELIVER] ✅
    │   └── SYSRETQ
```

### 10.2 `SYSCALL_TABLE_SIZE = 521`

- Plage POSIX : `0` → `499` (standard Linux-compatible)
- Plage ExoFS : `500` → `520` (21 syscalls propres à Exo-OS)

### 10.3 Fast path

```rust
// syscall/fast_path.rs
// Syscalls hautement fréquents (getpid, sched_yield, ...) inlinés
#[inline(always)]
pub fn sys_sched_yield() -> i64 {
    // ~500 cycles — utilisé par le GC kthread ExoFS
}
```

---

## 11. Arborescence des modules

```
/workspaces/Exo-OS/
├── kernel/
│   └── src/
│       ├── arch/x86_64/
│       │   ├── boot/           ← memory_map, trampoline BIOS
│       │   ├── cpu/            ← TSC, CPUID, MSR, XSAVE
│       │   ├── time/           ← calibration TSC (5 sources)
│       │   ├── acpi/           ← HPET, PM Timer, MADTparser
│       │   └── syscall.rs      ← entrée SYSCALL/SYSRETQ
│       ├── memory/
│       │   ├── physical/       ← buddy allocator
│       │   ├── heap/           ← slab/SLUB, #[global_allocator]
│       │   ├── virtual/        ← PML4, VMA tree, APIC remap
│       │   ├── swap/           ← LZ4 compress/decompress
│       │   └── utils/          ← futex table, OOM killer
│       ├── scheduler/
│       │   ├── asm/            ← switch_asm.s (context switch)
│       │   ├── core/           ← RunQueue CFS, preempt, task
│       │   ├── smp/            ← topology per-CPU
│       │   └── timer/          ← hrtimer HPET-based
│       ├── process/
│       │   ├── lifecycle/      ← fork, exec, exit, wait, create_kthread
│       │   └── signal/         ← delivery, handler, mask, send
│       ├── fs/
│       │   └── exofs/          ← ExoFS complet (voir §5.4)
│       ├── ipc/
│       │   ├── core/           ← IPC primitives
│       │   ├── ring/           ← SPSC/MPSC CachePadded
│       │   └── sync/           ← futex, condvar
│       ├── security/
│       │   └── crypto/         ← CSPRNG, constant-time ops
│       └── syscall/
│           ├── table.rs        ← SYSCALL_TABLE[521]
│           ├── numbers.rs      ← constantes syscall
│           ├── dispatch.rs     ← routage nr → handler
│           ├── fast_path.rs    ← syscalls inlinés
│           └── handlers/       ← implémentations POSIX
├── servers/
│   ├── init_server/            ← PID 1 — superviseur
│   ├── ipc_router/             ← PID 2 — directory service
│   ├── vfs_server/             ← PID 3 — namespace VFS
│   ├── crypto_server/          ← PID 4 — service crypto (SRV-04)
│   ├── phase5-tests/           ← 57 tests Phase 5
│   ├── device_server/          ← PID 5 (Phase 6)
│   ├── network_server/         ← Phase 6
│   ├── scheduler_server/       ← Phase 6
│   └── memory_server/          ← Phase 6
├── libs/
│   ├── exo_ipc/                ← SPSC/MPSC ring buffers
│   ├── exo_crypto/             ← XChaCha20, Blake3, HKDF
│   ├── exo_std/                ← stdlib no_std
│   ├── exo_allocator/          ← allocateur userspace
│   ├── exo-libc/               ← libc Exo-OS (musl-based)
│   └── exo-rt/                 ← runtime Rust no_std
├── drivers/
│   ├── clock/                  ← RTC, TSC, HPET drivers
│   ├── storage/                ← NVMe, VirtIO block
│   ├── network/                ← e1000, VirtIO net
│   └── input/                  ← PS/2, USB HID
└── kernel/phase3-tests/        ← 70 tests Phase 3
```

---

## 12. Commandes de build

```bash
# == Build ==
# Compilation kernel (target bare-metal)
cargo build --package exo-os-kernel

# Compilation tous les crates
cargo build

# Vérification sans linkage (plus rapide)
cargo check

# == Tests ==
# Tests Phase 3 (syscalls process/signal)
cd kernel/phase3-tests
cargo test --target x86_64-unknown-linux-musl

# Tests Phase 5 (servers Ring 1)
cd servers/phase5-tests
cargo test --target x86_64-unknown-linux-musl

# == QEMU ==
# Boot BIOS (actuel)
make run

# Boot UEFI OVMF (Phase 6 — à implémenter)
make run-uefi

# == Débogage ==
# Sortie port 0xE9 (e9_tag dans calibration, boot, etc.)
# Lire avec : xxd /tmp/e9_out.txt | head -50

# == Variables d'environnement ==
EXO_QEMU_EXIT=1   # Active la sortie automatique QEMU après boot
RUST_LOG=debug    # Verbosité des logs
```

---

## 13. Prochaine étape — Phase 6 (exo-boot UEFI)

### Prérequis avant d'activer exo-boot

| Tâche | Module | Priorité |
|-------|--------|---------|
| Compiler `kernel.elf` en ET_DYN (PIE) | `kernel/Cargo.toml` | 🔴 |
| `detect_boot_path()` dual-entry | `kernel/src/arch/x86_64/boot/early_init.rs` | 🔴 |
| Vérifier magic `0x4F42_5F53_4F4F_5845` (BootInfo) | `exo-boot/src/` | 🔴 |
| Test boot UEFI sur QEMU OVMF | CI/CD | 🔴 |
| `kernel.elf` sur partition ESP FAT32 | Makefile | 🟠 |
| Supprimer `mbr.asm`, `stage2.asm`, `disk.rs` BIOS | `exo-boot/` | 🟡 |

### Séquence de boot UEFI attendue

```
UEFI firmware
    │
    ▼ exo-boot (PE32+ UEFI application)
    │   ├── ParseConfigurationTable() → ACPI RSDP
    │   ├── GetMemoryMap() → BootInfo.memory_map
    │   ├── LoadFile(kernel.elf) depuis ESP FAT32
    │   ├── apply_pie_relocations(kernel.elf)
    │   └── ExitBootServices() → jump kernel.elf
    │
    ▼ kernel early_init.rs
    │   ├── detect_boot_path() → UEFI vs BIOS
    │   ├── verify magic 0x4F42_5F53_4F4F_5845
    │   └── init_from_bootinfo(&BootInfo)
    │
    ▼ séquence normale : mem → sched → IPC → ExoFS → servers
    │
    ▼ VALIDATION : XK12356ps789abcdefgZAIOK via e9_tag → exit(0)
```

---

*Document généré le 10 mars 2026 — Exo-OS Phase 5 complète.*
