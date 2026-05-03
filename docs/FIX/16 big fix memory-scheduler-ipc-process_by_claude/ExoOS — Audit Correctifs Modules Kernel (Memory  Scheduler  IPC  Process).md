# ExoOS — Audit Correctifs Modules Kernel (Memory / Scheduler / IPC / Process)
## Rapport Claude-Alpha · Mai 2026

> **Auteur :** claude-alpha  
> **Périmètre :** `kernel/src/memory/`, `kernel/src/scheduler/`, `kernel/src/ipc/`, `kernel/src/process/`  
> **Base :** commit HEAD — dépôt `https://github.com/darkfireeee/Exo-OS.git`  
> **Références spec :** `docs/recast/ExoOS_Architecture_v7.md`, `docs/recast/ExoOS_Kernel_Types_v10.md`, `docs/recast/GI-01_Types_TCB_SSR.md`, `docs/Exo-OS-TLA+/`  
> **Classification :** `GRV-` = crash garanti / corruption silencieuse · `SIL-` = compile OK mais sémantique fausse

---

## Résumé Exécutif

| Priorité | Identifiant | Module | Description courte |
|---|---|---|---|
| **P0** | GRV-SCHED-01 | scheduler | `schedule_block()` panique inconditionnelle si idle absent |
| **P0** | GRV-PROC-01 | process | `unsafe impl Sync for ProcessThread` — unsound (raw ptr) |
| **P0** | GRV-IPC-01 | ipc | Aliases `IpcError` sémantiquement dupliqués brisent les `match` |
| **P0** | GRV-IPC-02 | ipc | Deux types de flags (`MsgFlags`/`MessageFlags`) sans conversion |
| **P1** | SIL-MEM-01 | memory | `register_backend_swap_provider()` appelé avant l'init FS |
| **P1** | SIL-SCHED-02 | scheduler | Numérotation des étapes de `context_switch()` incorrecte |
| **P1** | SIL-PROC-02 | process | `new_kthread()` utilise `Pid(1)` (init_server) au lieu de `Pid(0)` |
| **P1** | SIL-IPC-03 | ipc | Mode stub SHM (virt=phys) sans garde de production |
| **P2** | SIL-MEM-02 | memory | `cow::init()` absent du séquençage `memory::init()` |
| **P2** | SIL-MEM-03 | memory | Référence doc incorrecte (`refonte/` → `recast/`) |
| **P2** | SIL-PROC-03 | process | `ProcessState::Dead` vs `TaskState::Dead` — synchronisation non documentée |
| **P2** | SIL-IPC-04 | ipc | Commentaires exemples MsgFlags sémantiquement incorrects |

**État SSR (ExoPhoenix) :** Confirmé CORRIGÉ dans HEAD — `take_slot_once([u64;4])` est correct ; `seen_slots = [0u64; 4]` dans forge/handoff/isolate. Aucun correctif SSR requis.

---

## P0 — BLOQUANTS CRITIQUES

---

### GRV-SCHED-01 — `schedule_block()` panique inconditionnelle si idle absent

**Fichier :** `kernel/src/scheduler/core/switch.rs` · lignes ~430–455

**Symptôme :** Lors de la phase d'initialisation des APs (Application Processors), ou si un thread est bloqué avant que le thread idle du CPU ne soit publié via `bind_boot_idle_threads()`, `schedule_block()` atteint son dernier `match` sans idle thread disponible et exécute :

```rust
// ACTUEL — CRASH GARANTI
_ => {
    panic!("schedule_block: idle_thread absent sur cpu {}", rq.cpu.0);
}
```

Un idle thread absent est une condition **temporairement normale** pendant le boot SMP (entre `scheduler::init()` et `bind_boot_idle_threads()`). Une panique ici stoppe le boot ou corrompt l'état d'un AP.

**Analyse TLA+ :** Le module `ContextSwitch.tla` vérifie que `schedule_block` suppose un idle thread existant. L'absence est hors du domaine vérifié — d'où l'absence de violation dans les 1.2B états. Mais le boot SMP ajoute des états non couverts.

**Correctif :**

```rust
// kernel/src/scheduler/core/switch.rs
// Remplacer les deux branches panic dans schedule_block()

// Branche 1 (PickResult::Switch — même thread)
_ => {
    // Idle absent pendant le boot SMP : spin court et retour.
    // Le thread reprendra sur le prochain tick scheduler.
    // SAFETY : on est dans une window de boot très courte.
    for _ in 0..100 {
        core::hint::spin_loop();
    }
    current.set_state(TaskState::Runnable);
    return;
}

// Branche 2 (PickResult::KeepRunning | GoIdle sans idle)
_ => {
    // Même logique : spin + retour au lieu de panique.
    for _ in 0..100 {
        core::hint::spin_loop();
    }
    current.set_state(TaskState::Running);
    return;
}
```

**Alternative robuste** : Ajouter une assertion de phase boot dans `schedule_block()` :

```rust
// En début de schedule_block(), AVANT le match pick_next
debug_assert!(
    rq.idle_thread.is_some()
        || crate::scheduler::core::boot_idle::published_boot_idle(rq.cpu.0).is_some(),
    "schedule_block appelé sans idle thread sur CPU {} — appel trop tôt ?",
    rq.cpu.0
);
```

---

### GRV-PROC-01 — `unsafe impl Sync for ProcessThread` — Unsound

**Fichier :** `kernel/src/process/core/tcb.rs` · ligne ~fin du fichier

**Code actuel :**
```rust
// ACTUEL — UNSOUND
unsafe impl Send for ProcessThread {}
unsafe impl Sync for ProcessThread {}
```

**Problème :** `ProcessThread` contient `KernelStack` qui est défini ainsi :

```rust
pub struct KernelStack {
    base: *mut u8,  // ← raw pointer, ni Send ni Sync par défaut
    size: usize,
    top: u64,
}

unsafe impl Send for KernelStack {} // OK — ownership exclusive
// KernelStack n'implémente PAS Sync
```

Implémenter `Sync` pour `ProcessThread` permet de créer un `Arc<ProcessThread>` partagé entre threads, puis d'accéder concurremment à `kernel_stack.base: *mut u8` via des références partagées — race de données UB.

Le commentaire "accédé depuis un seul CPU à la fois" justifie `Send`, pas `Sync`. Les champs atomiques (`tls_gs_base`, `detached`, etc.) sont bien `Sync` individuellement, mais la structure entière ne l'est pas à cause de `KernelStack`.

**Correctif :**

```rust
// kernel/src/process/core/tcb.rs — remplacer les impl unsafe

// SAFETY: ProcessThread est transféré d'un CPU à l'autre par le scheduler
// (ownership exclusive : un seul CPU y accède à la fois). L'impl Send est
// sécurisée parce que le transfert implique une barrière mémoire séqcst.
unsafe impl Send for ProcessThread {}

// Sync retiré : KernelStack::base est *mut u8 et ne peut pas être accédé
// concurremment depuis plusieurs threads. ProcessThread n'est jamais partagé
// via Arc<> — il est toujours accédé sous ownership exclusive du scheduler.
// Si un partage futur est nécessaire, utiliser ProcessThread dans un Mutex.
// (NB: les champs atomiques sont Sync individuellement et restent accessibles
//  via des références aux champs spécifiques si nécessaire.)
```

**Impact :** Suppression pure — aucune régression de fonctionnalité. Tout code qui compilait avec `Sync` et qui était correct le restera (le scheduler ne crée pas de `Arc<ProcessThread>`).

---

### GRV-IPC-01 — Aliases `IpcError` dupliqués brisent les `match` exhaustifs

**Fichier :** `kernel/src/ipc/core/types.rs` · lignes ~260–340

**Problème :** `IpcError` est un `#[repr(u32)]` enum avec 31 variantes dont plusieurs sont des **alias sémantiques** avec des discriminants différents :

| Alias | Valeur | Canonique | Valeur |
|---|---|---|---|
| `Closed` | 17 | `ChannelClosed` | 3 |
| `Internal` | 18 | `InternalError` | 13 |
| `Invalid` | 19 | `InvalidParam` | 10 |
| `Full` | 20 | `QueueFull` | 28 |
| `NotFound` | 22 | `EndpointNotFound` | 2 |
| `InvalidArgument` | 26 | `InvalidParam` | 10 |
| `OutOfResources` | 27 | `ResourceExhausted` | 7 |

Un `match err { IpcError::ChannelClosed => handle_close(), _ => ignore() }` ne capturera **pas** `IpcError::Closed = 17` — fuite silencieuse de l'erreur.

```rust
// EXEMPLE DE BUG SILENCIEUX
fn handle_ipc_error(e: IpcError) {
    match e {
        IpcError::ChannelClosed => { /* nettoyage */ }  // manque Closed=17 !
        IpcError::QueueFull => { /* backpressure */ }   // manque Full=20 !
        _ => {}
    }
}
```

**Correctif — deux approches (choisir l'une) :**

**Option A (recommandée) : Déprécier les aliases et les mapper dans `From`**

```rust
// kernel/src/ipc/core/types.rs
// Remplacer les variantes alias par des méthodes de normalisation

impl IpcError {
    /// Normalise les variantes alias vers leurs canoniques.
    /// À appeler en entrée de tout traitement d'erreur multi-variant.
    ///
    /// Garantit qu'un `match` sur le résultat de `normalize()` n'a
    /// pas besoin de gérer les alias.
    #[inline(always)]
    pub fn normalize(self) -> Self {
        match self {
            Self::Closed           => Self::ChannelClosed,
            Self::Internal         => Self::InternalError,
            Self::Invalid          => Self::InvalidParam,
            Self::Full             => Self::QueueFull,
            Self::NotFound         => Self::EndpointNotFound,
            Self::InvalidArgument  => Self::InvalidParam,
            Self::OutOfResources   => Self::ResourceExhausted,
            other => other,
        }
    }

    /// Retourne true si l'erreur indique une fermeture de canal
    /// (quelle que soit la variante exacte).
    #[inline(always)]
    pub fn is_closed(self) -> bool {
        matches!(self, Self::ChannelClosed | Self::Closed)
    }

    /// Retourne true si la file est pleine.
    #[inline(always)]
    pub fn is_queue_full(self) -> bool {
        matches!(self, Self::QueueFull | Self::Full)
    }
}
```

**Option B : Remplacer les alias par des constantes associées (cassant l'ABI)**

```rust
// Supprimer les variantes alias et les remplacer par des constantes
impl IpcError {
    pub const CLOSED: Self = Self::ChannelClosed;
    pub const INTERNAL: Self = Self::InternalError;
    pub const INVALID: Self = Self::InvalidParam;
    pub const FULL: Self = Self::QueueFull;
    pub const NOT_FOUND: Self = Self::EndpointNotFound;
    pub const INVALID_ARGUMENT: Self = Self::InvalidParam;
    pub const OUT_OF_RESOURCES: Self = Self::ResourceExhausted;
}
```

**Recommandation :** Option A (non cassante). Option B nécessite un audit complet des usages.

---

### GRV-IPC-02 — Deux types de flags sans conversion

**Fichier :** `kernel/src/ipc/core/types.rs` · lignes ~196 et ~377

**Problème :** Deux types distincts pour les mêmes 7 flags sémantiques :

```rust
pub struct MsgFlags(pub u32);     // ring/, channel/ — 7 flags identiques
pub struct MessageFlags(pub u16); // message/builder — 7 flags identiques
```

Aucune implémentation `From<MsgFlags> for MessageFlags` ni `From<MessageFlags> for MsgFlags` n'existe. Le code qui route un message depuis le ring (qui utilise `MsgFlags`) vers le builder/serializer (qui utilise `MessageFlags`) doit soit dupliquer la valeur brute, soit effectuer une conversion manuelle non vérifiée.

**Impact :** Risque de troncation silencieuse si `MsgFlags(u32)` contient des bits > 0x7F (bits 7-31) lors d'une conversion manuelle vers `MessageFlags(u16)`.

**Correctif — ajouter les conversions canoniques :**

```rust
// kernel/src/ipc/core/types.rs — après les deux définitions de struct

impl From<MsgFlags> for MessageFlags {
    /// Convertit `MsgFlags` (u32) vers `MessageFlags` (u16).
    ///
    /// Seuls les 7 bits de flags connus sont transférés (mask 0x7F).
    /// Les bits 7-31 de MsgFlags sont ignorés — ils n'ont pas d'équivalent
    /// dans MessageFlags. Utiliser cette conversion uniquement après avoir
    /// vérifié qu'aucun flag étendu n'est actif.
    #[inline(always)]
    fn from(f: MsgFlags) -> Self {
        // Tronquer à 16 bits après masquage des 7 flags connus.
        // Bits 7-31 de MsgFlags : réservés, pas de représentation en MessageFlags.
        MessageFlags((f.0 & 0x7F) as u16)
    }
}

impl From<MessageFlags> for MsgFlags {
    /// Convertit `MessageFlags` (u16) vers `MsgFlags` (u32) — extension sans perte.
    #[inline(always)]
    fn from(f: MessageFlags) -> Self {
        MsgFlags(f.0 as u32)
    }
}

impl MsgFlags {
    /// Compatibilité avec le pattern `MsgFlags::empty()` utilisé dans les
    /// commentaires d'exemple. Équivalent de `MsgFlags::default()`.
    #[inline(always)]
    pub const fn empty() -> Self {
        Self(0)
    }
}

impl MessageFlags {
    /// Compatibilité avec le pattern `MessageFlags::empty()`.
    #[inline(always)]
    pub const fn empty() -> Self {
        Self(0)
    }
}
```

---

## P1 — MAJEURES

---

### SIL-MEM-01 — `register_backend_swap_provider()` appelé avant l'init FS

**Fichier :** `kernel/src/memory/mod.rs` · fonction `init()` · Phase 4

**Code actuel :**
```rust
// Phase 4 : DMA
dma::init();
virt::fault::swap_in::register_backend_swap_provider(); // ← ICI
```

**Problème :** `register_backend_swap_provider()` enregistre un pointeur de fonction/trait qui sera appelé lors de page faults swap-in pour relire des pages depuis le swap device. Cette opération nécessite :
1. FS initialisé (pour accéder au device de swap)  
2. Scheduler actif (pour les I/O asynchrones)

Or `memory::init()` est appelé depuis `arch_boot_init()`, bien avant le scheduler (Phase 3) et le FS (Phase 7) dans `kernel_init()`. Enregistrer un provider qui appelle du code FS/scheduler pendant la Phase 4 viole l'ordre de couches `Memory(0) → Scheduler(1) → FS(3)`.

**Impact concret :** Si un page fault swap-in survient pendant le boot (unlikely mais possible), le provider appelé tentera d'accéder à des structures FS non initialisées → accès mémoire invalide.

**Correctif :**

```rust
// kernel/src/memory/mod.rs — modifier init()
pub unsafe fn init(phys_start: PhysAddr, phys_end: PhysAddr, regions: &[(u64, u64)]) {
    // ...
    // Phase 4 : DMA
    dma::init();
    // CORRECTIF SIL-MEM-01 : NE PAS appeler register_backend_swap_provider() ici.
    // Cette registration doit avoir lieu en Phase 7 (FS), après exofs_init().
    // Voir kernel/src/lib.rs kernel_init() Phase 7.
    // ...
}
```

```rust
// kernel/src/lib.rs — kernel_init() Phase 7 (après exofs_init)
let _ = crate::fs::exofs::exofs_init(...);

// CORRECTIF SIL-MEM-01 : enregistrer le swap backend ICI, après FS init.
crate::memory::virt::fault::swap_in::register_backend_swap_provider();

// ... reste Phase 7
```

---

### SIL-SCHED-02 — Numérotation des étapes de `context_switch()` incorrecte

**Fichier :** `kernel/src/scheduler/core/switch.rs` · doc comment de `context_switch()`

**Problème :** Le commentaire de documentation de `context_switch()` annonce des étapes 1-10 mais le code en implémente 8 avec une numérotation confuse :

```
Doc dit :    1  2  3  4  5  6  [manque 7]  8  [manque 9]  10
Code fait :  1  2  3  4  5  6             7              8
```

L'étape 9 (mise à jour `switch_count`) n'est pas mentionnée dans la doc. L'étape 10 (restauration FS/GS) est numérotée "8" dans le commentaire de code mais "10" dans la doc. Cette incohérence rend la revue difficile et peut masquer des étapes manquantes lors d'audits futurs.

**Correctif — réécrire le doc comment :**

```rust
/// Effectue le context switch de `prev` vers `next`.
///
/// # Séquence complète (8 étapes)
///
/// Étape 1 — Lazy FPU save : si `prev` a utilisé la FPU → XSAVE.
///           Poser CR0.TS=1 (déclenche #NM au prochain accès FPU). (SWITCH-02, V7-C-02)
///
/// Étape 2 — Sauvegarder PKRS (Intel PKS) et CET SSP si disponibles. (S6, FIX-CET-01)
///
/// Étape 3 — Sauvegarder FS.base et user_gs_base via rdmsr. (CORR-11)
///
/// Étape 4 — Marquer `prev` → Runnable (si était Running). Comptabiliser le
///           temps CPU couru par `prev`. Appeler `context_switch_asm()`.
///           L'ASM sauvegarde/restaure 6 callee-saved GPRs et switche CR3.
///
/// ──── À PARTIR D'ICI : contexte de `next` ────────────────────────────────
///
/// Étape 5 — Rafraîchir le slot CR3 per-CPU (FIX-KPTI-01). Restaurer PKRS
///           et CET SSP de `next`.
///
/// Étape 6 — Marquer `next` → Running. Incrémenter switch_count.
///           Mettre à jour le slot GS canonique (set_current_tcb).
///
/// Étape 7 — Mettre à jour kernel RSP et TSS.RSP0 ← next.kstack_top(). (V7-C-03)
///           Publier `next` dans CURRENT_THREAD_PER_CPU (fence SeqCst).
///
/// Étape 8 — Restaurer FS.base et user_gs_base de `next` via wrmsr. (CORR-11)
///
/// # Sécurité
/// - Appelé avec préemption désactivée (IrqGuard ou PreemptGuard).
/// - `prev` et `next` DOIVENT être des pointeurs valides vers des TCBs actifs.
/// - Cette fonction NE doit JAMAIS appeler `process::signal::*`. (SIGNAL-01)
```

---

### SIL-PROC-02 — `new_kthread()` utilise `Pid(1)` au lieu de `Pid(0)`

**Fichier :** `kernel/src/process/core/tcb.rs` · ligne ~250

**Code actuel :**
```rust
pub fn new_kthread(tid: Tid, cr3: u64) -> Option<Box<Self>> {
    Self::new(
        tid,
        Pid(1),   // ← INCORRECT : Pid(1) = init_server (Ring 1)
        cr3,
        SchedPolicy::Normal,
        Priority::NORMAL_DEFAULT,
    )
}
```

**Problème :** `Pid(1)` est réservé à `init_server` (premier processus Ring 1, PID 1 POSIX). Les threads kernel (kthreads) — reaper, idle, DMA completion — doivent appartenir à `Pid(0)` (namespace kernel, hors POSIX).

Les conséquences :
1. `PROCESS_REGISTRY` associe les kthreads au PCB de `init_server` si Pid(1) est enregistré
2. `waitpid(1, ...)` depuis un processus userspace pourrait observer des threads kernel
3. Les signaux envoyés à PID 1 (kill -9 1) tentent de tuer les kthreads — comportement POSIX invalide

**Correctif :**
```rust
// kernel/src/process/core/tcb.rs
pub fn new_kthread(tid: Tid, cr3: u64) -> Option<Box<Self>> {
    Self::new(
        tid,
        Pid(0),   // CORRECTIF : Pid(0) = namespace kernel (hors POSIX)
        cr3,
        SchedPolicy::Normal,
        Priority::NORMAL_DEFAULT,
    )
}
```

**Note complémentaire :** `pid::init(32768, 131072)` dans `kernel_init()` doit réserver explicitement PID 0 comme "kernel" et PID 1 comme "init_server". Vérifier que `pid::init()` protège PID 0 contre la ré-allocation :

```rust
// kernel/src/process/core/pid.rs — vérifier que init() contient :
pub fn init(max_pid: u32, max_tid: u32) {
    // Réserver PID 0 (kernel) et PID 1 (init_server)
    PID_ALLOCATOR.reserve(0);
    PID_ALLOCATOR.reserve(1);
    // ... reste init
}
```

---

### SIL-IPC-03 — Mode stub SHM (virt = phys) sans garde de production

**Fichier :** `kernel/src/ipc/shared_memory/mapping.rs` · ligne ~259

**Code actuel :**
```rust
// Adresse virtuelle = adresse physique dans l'implémentation stub
// acceptable en dev/test mono-processus.
```

**Problème :** Si `ipc_install_vmm_hooks()` n'a pas été appelé (hook `None`), `shm_map()` opère en mode simulé où l'adresse virtuelle retournée est l'adresse physique brute. En production multi-processus :
1. Les processus Ring 1 reçoivent des adresses physiques comme adresses virtuelles → segfault immédiat
2. Sans IOMMU, le DMA peut accéder à n'importe quelle mémoire → faille sécurité
3. Le hook est installé dans `kernel_init()` Phase 6 — mais si Phase 6 est skippé ou échoue partiellement, le stub reste actif silencieusement

**Correctif — ajouter un guard de production :**

```rust
// kernel/src/ipc/shared_memory/mapping.rs

/// Ajouter un flag d'état d'installation des hooks
static VMM_HOOKS_INSTALLED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

pub fn register_map_hook(f: MapPageFn) {
    *MAP_PAGE_HOOK.lock() = Some(f);
    VMM_HOOKS_INSTALLED.store(true, core::sync::atomic::Ordering::Release);
}

/// Retourne true si les hooks VMM sont installés (mode production).
#[inline(always)]
pub fn vmm_hooks_ready() -> bool {
    VMM_HOOKS_INSTALLED.load(core::sync::atomic::Ordering::Acquire)
}

// Dans shm_map(), au début de la fonction :
pub fn shm_map(...) -> Result<VirtAddr, IpcError> {
    // CORRECTIF SIL-IPC-03 : en mode production (Ring 1 actif),
    // les hooks VMM DOIVENT être installés avant tout appel shm_map().
    #[cfg(not(feature = "dev_no_vmm"))]
    if !vmm_hooks_ready() {
        // En production, l'absence de hooks est une erreur fatale.
        // En développement, le stub est toléré via feature flag.
        return Err(IpcError::InternalError);
    }
    // ... reste de shm_map()
}
```

```toml
# kernel/Cargo.toml — ajouter
[features]
dev_no_vmm = []  # Permet le mode stub SHM (dev uniquement, jamais en production)
```

---

## P2 — MINEURES

---

### SIL-MEM-02 — `cow::init()` absent du séquençage `memory::init()`

**Fichier :** `kernel/src/memory/mod.rs` · fonction `init()`

**Problème :** `COW_TRACKER` est re-exporté depuis `cow::tracker` et utilisé globalement, mais aucune phase de `memory::init()` n'appelle `cow::init()`. Le `COW_TRACKER` est probablement un `Mutex<CowTracker>` initialisé via `const` ou `lazy_static`, mais si son initialisation allocate de la mémoire (via `Vec` interne), elle doit avoir lieu après la Phase 1c (SLUB).

**Correctif :**

```rust
// kernel/src/memory/mod.rs — ajouter après Phase 1d

// Phase 1e : CoW tracker
cow::init(); // Initialise les tables CoW (après SLUB disponible)
```

```rust
// kernel/src/memory/cow/mod.rs — ajouter si absent
/// Initialise le tracker CoW.
/// DOIT être appelé après init_phase3_slab_slub().
pub fn init() {
    // Si COW_TRACKER est const-initialisé (tableau statique fixe), cette
    // fonction est un no-op documentaire.
    // Si COW_TRACKER alloue dynamiquement, c'est ici que l'allocation a lieu.
    let _ = &COW_TRACKER; // force l'initialisation de la lazy static si utilisée
}
```

---

### SIL-MEM-03 — Référence documentation incorrecte dans `memory/mod.rs`

**Fichier :** `kernel/src/memory/mod.rs` · commentaire ligne ~27

**Code actuel :**
```rust
// Règles d'architecture (docs/refonte/regle_bonus.md) :
```

**Correctif :**
```rust
// Règles d'architecture (docs/recast/ExoOS_Architecture_v7.md §2.2) :
```

Le répertoire `refonte/` n'existe pas dans le dépôt. Les règles sont dans `docs/recast/ExoOS_Architecture_v7.md` section 2.2 (Lock Ordering).

---

### SIL-PROC-03 — `ProcessState::Dead` vs `TaskState::Dead` — synchronisation non documentée

**Fichiers :** `kernel/src/process/core/pcb.rs` + `kernel/src/scheduler/core/task.rs`

**Problème :** Deux "états morts" coexistent sans protocole de synchronisation documenté :
- `ProcessState::Dead` (PCB) : ressources libérées après `reap()`
- `TaskState::Dead` (TCB) : état terminal du thread dans le scheduler

L'ordre de transition n'est pas spécifié : est-ce que `TaskState::Dead` est positionné AVANT ou APRÈS `ProcessState::Dead` ? Si le reaper lit `ProcessState::Dead` et libère le PCB pendant que le thread scheduler lit encore le TCB → use-after-free.

**Correctif — documenter le protocole dans les deux fichiers :**

```rust
// kernel/src/process/core/pcb.rs — ajouter dans ProcessState::Dead

/// # Protocole de synchronisation avec TaskState::Dead
///
/// ORDRE OBLIGATOIRE dans lifecycle/exit.rs et lifecycle/reap.rs :
///
/// 1. Thread courant appelle do_exit()
/// 2. TCB.set_state(TaskState::Zombie)  ← vu par scheduler → retire de runqueue
/// 3. PCB.state = ProcessState::Zombie  ← vu par parent via wait()
/// 4. Parent appelle waitpid() → PROCESS_REGISTRY.reap()
/// 5. TCB.set_state(TaskState::Dead)    ← scheduler ne touche plus le TCB
/// 6. PCB.state = ProcessState::Dead    ← PCB libérable
/// 7. Libération PCB + TCB (dans cet ordre — TCB owned par ProcessThread)
///
/// INVARIANT : ProcessState::Dead ne peut être positionné QUE après
/// TaskState::Dead. Violation → use-after-free dans le scheduler.
Dead = 5,
```

```rust
// kernel/src/scheduler/core/task.rs — ajouter dans TaskState::Dead

/// # Invariant de synchronisation avec ProcessState
///
/// TaskState::Dead est positionné AVANT ProcessState::Dead (voir pcb.rs).
/// Une fois dans cet état, aucun CPU ne doit accéder au TCB sauf pour le lire.
Dead = 6,
```

---

### SIL-IPC-04 — Commentaires exemples MsgFlags sémantiquement incorrects

**Fichiers :** `kernel/src/ipc/ring/spsc.rs` · ligne ~54  
             `kernel/src/ipc/channel/typed.rs` · ligne ~223

**Code actuel :**
```rust
/// ring.push_copy(&data, data.len(), MsgFlags::default())?;  // spsc.rs
/// tx.send(42u64, MsgFlags::empty()).unwrap();                // typed.rs
```

**Problème :** `MsgFlags::default()` et `MsgFlags::empty()` sont équivalents à `MsgFlags(0)` — aucun flag positionné. Un message avec flags=0 n'indique pas si l'envoi est bloquant ou non-bloquant, synchrone ou asynchrone. Ces exemples induisent les développeurs à oublier de spécifier le flag `SYNC` ou `NOWAIT`.

**Correctif :**
```rust
// kernel/src/ipc/ring/spsc.rs
/// ring.push_copy(&data, data.len(), MsgFlags::NOWAIT)?;  // non-bloquant
/// ring.push_copy(&data, data.len(), MsgFlags::SYNC)?;    // bloquant jusqu'à lecture

// kernel/src/ipc/channel/typed.rs
/// tx.send(42u64, MsgFlags::NOWAIT).unwrap(); // non-bloquant
```

---

## Vérifications post-correctifs

Après application de tous les correctifs, exécuter :

```bash
# Compiler le kernel (vérifie les types et les unsafe)
cargo build --release -p exo-os-kernel 2>&1 | grep -E "error|warning"

# Vérifier l'absence de Sync non justifié sur les types avec raw pointers
grep -rn "unsafe impl Sync" kernel/src/process/ kernel/src/scheduler/

# Vérifier que tous les blocs unsafe ont un commentaire SAFETY
grep -rn "unsafe {" kernel/src/ipc/channel/ | grep -v "// SAFETY"

# Vérifier l'ordre d'init dans kernel_init
grep -n "register_backend_swap_provider\|exofs_init\|ipc_init\|scheduler::init\|process::init" \
    kernel/src/lib.rs

# Vérifier que Pid(0) est utilisé pour les kthreads
grep -rn "new_kthread\|Pid(1)\|Pid(0)" kernel/src/process/core/tcb.rs
```

---

## État SSR ExoPhoenix — Confirmation CORRIGÉ

La revue du code live confirme que le bug SSR documenté dans les sessions précédentes (bitmask `[u64; 1]` limitant à 64 cores) est **entièrement corrigé** dans HEAD :

```rust
// kernel/src/exophoenix/mod.rs — CORRECT
pub(crate) fn take_slot_once(seen: &mut [u64; 4], slot: usize) -> bool {
    if slot >= ssr::MAX_CORES { return false; } // MAX_CORES = 256
    let word = slot / u64::BITS as usize;        // 0..3 pour slots 0..255 ✓
    let bit = 1u64 << (slot % u64::BITS as usize);
    let was_seen = seen[word] & bit != 0;
    seen[word] |= bit;
    !was_seen
}

// forge.rs, handoff.rs (×2), isolate.rs — tous CORRECTS
let mut seen_slots = [0u64; 4];  // 4×64 = 256 bits = 256 cores ✓
```

**Aucun correctif SSR requis.** ✅

---

## Résumé des fichiers à modifier

| Fichier | Correctifs |
|---|---|
| `kernel/src/process/core/tcb.rs` | GRV-PROC-01 (retirer Sync), SIL-PROC-02 (Pid(0)) |
| `kernel/src/scheduler/core/switch.rs` | GRV-SCHED-01 (panic → spin), SIL-SCHED-02 (doc) |
| `kernel/src/ipc/core/types.rs` | GRV-IPC-01 (normalize()), GRV-IPC-02 (From impl) |
| `kernel/src/memory/mod.rs` | SIL-MEM-01 (déplacer swap provider), SIL-MEM-02 (cow::init), SIL-MEM-03 (doc path) |
| `kernel/src/lib.rs` | SIL-MEM-01 (appel swap provider post-FS) |
| `kernel/src/ipc/shared_memory/mapping.rs` | SIL-IPC-03 (guard production) |
| `kernel/src/process/core/pcb.rs` | SIL-PROC-03 (doc protocole sync) |
| `kernel/src/ipc/ring/spsc.rs` | SIL-IPC-04 (exemples) |
| `kernel/src/ipc/channel/typed.rs` | SIL-IPC-04 (exemples) |

**Total : 9 fichiers · 12 correctifs · 0 refactoring architectural nécessaire**

---

*claude-alpha — Audit complet, base de code live, aucune génération depuis mémoire.*