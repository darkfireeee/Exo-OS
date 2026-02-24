# 📋 DOC 1 — CORRECTIONS DE L'ARBORESCENCE : RÉFÉRENCE CONSOLIDÉE v1.2
> Exo-OS · Corrections C1/C2/C3 · Post-audit · Erreurs originales annotées
> Basé sur : KERNEL_INTEGRATION_MAP_v4 · Audit AUDIT_ARBORESCENCE_EXO_OS.md

---

## VUE D'ENSEMBLE DES 3 CORRECTIONS

| # | Erreur | Nature | Impact si non corrigé |
|---|---|---|---|
| C1 | `signal/` dans `scheduler/` | Mauvaise couche | Hot path pollué, latence dégradée, compilation alourdie |
| C2 | `capability/` dans `ipc/` | TCB non prouvable | Preuve formelle impossible, faille TOCTOU |
| C3 | Modules IA absents | Fonctionnalité manquante | Prédictions NUMA/scheduling/prefetch absentes |

> **Note DMA :** DMA reste sous `memory/dma/` — correct architecturalement.
> Le wakeup vers `process/` passe par le trait abstrait `DmaWakeupHandler`
> enregistré au boot. Zéro dépendance circulaire, couche 0 préservée.

---

## ═══════════════════════════════════════════════════════════
## CORRECTION 1 — `signal/` : de `scheduler/` vers `process/`
## ═══════════════════════════════════════════════════════════

### Pourquoi c'était une erreur

Le scheduler est la **Couche 1**, conçue pour une latence extrême de 100-150 cycles
sur `pick_next_task()`. Les signaux POSIX sont des concepts de cycle de vie processus :

```
Signal = "événement asynchrone adressé à un processus ou thread"
       = fork/exec/exit/wait territory → process/
       ≠ "choisir le prochain thread à exécuter" → scheduler/
```

**Seule interaction légitime scheduler ↔ signal :**
Un `AtomicBool` dans le TCB — le scheduler le lit au retour vers userspace.
C'est tout. La livraison elle-même est orchestrée par `arch/`, pas par `scheduler/`.

---

### Delta de l'arborescence

#### SUPPRIMÉ de `kernel/src/scheduler/`

```
kernel/src/scheduler/
  └── signal/              ← SUPPRIMÉ ENTIÈREMENT
      ├── delivery.rs
      ├── handler.rs
      ├── mask.rs
      ├── queue.rs
      └── default.rs
```

#### AJOUTÉ dans `kernel/src/process/`

```
kernel/src/process/
  └── signal/              ← DÉPLACÉ ICI (logique POSIX complète)
      ├── mod.rs
      ├── delivery.rs      # Livraison signal au retour kernel→userspace
      │                    # Appelé par : arch/x86_64/syscall.rs (retour syscall)
      │                    #              arch/x86_64/exceptions.rs (retour #PF/préemption)
      │                    #
      │                    # ⚠️ ERREUR DANS L'ORIGINAL : indiquait aussi
      │                    #   "scheduler/core/switch.rs (retour préemption)"
      │                    #   INCORRECT — switch.rs (couche 1) NE PEUT PAS
      │                    #   importer process::signal (couche 1.5).
      │                    #   C'est arch/exceptions.rs qui orchestre la livraison
      │                    #   après préemption, pas switch.rs directement.
      ├── handler.rs       # Exécution handler utilisateur (sigaltstack, frame)
      ├── mask.rs          # sigprocmask — masque par thread (stocké dans TCB)
      ├── queue.rs         # File RT signals (POSIX: SIGRTMIN..SIGRTMAX, 32 signaux)
      └── default.rs       # Actions par défaut: TERM, CORE, IGN, STOP, CONT
```

#### MODIFIÉ dans `kernel/src/scheduler/core/switch.rs`

```rust
// kernel/src/scheduler/core/switch.rs

/// Vérification signal pending — HOT PATH (≤5 cycles)
/// NE connaît PAS process::signal — lit uniquement un flag atomique dans TCB
/// NE LIVRAIT PAS les signaux — se contente de poser un flag pour arch/
#[inline(always)]
pub fn check_signal_pending(tcb: &ThreadControlBlock) -> bool {
    tcb.signal_pending.load(Ordering::Relaxed)
    // Si true → arch/exceptions.rs ou arch/syscall.rs orchestrent la livraison
    // switch.rs ne fait QUE lire — jamais appeler process::signal::*
}
```

#### AJOUTÉ dans `kernel/src/scheduler/core/task.rs`

```rust
// Dans ThreadControlBlock — le SEUL lien scheduler ↔ signal
pub struct ThreadControlBlock {
    // ...

    /// Flag: signal(s) en attente pour ce thread
    /// Écrit par : process::signal::delivery::send_signal()
    /// Lu par    : scheduler::core::switch::check_signal_pending()
    /// AtomicBool : lecture hot path sans lock
    pub signal_pending: AtomicBool,

    /// Masque de signaux bloqués
    /// Modifié par : process::signal::mask::sigprocmask()
    /// Lu par      : process::signal::delivery (filtre signaux masqués)
    pub signal_mask: AtomicU64,  // bitmask 64 signaux standard
}
```

#### MODIFIÉ dans `kernel/src/arch/x86_64/syscall.rs`

```rust
// arch/ est transverse — peut appeler n'importe quelle couche
pub fn syscall_return_to_user(tcb: &mut ThreadControlBlock) {
    if tcb.signal_pending.load(Ordering::Acquire) {
        process::signal::delivery::handle_pending_signals(tcb);
    }
    // SYSRET
}
```

#### MODIFIÉ dans `kernel/src/arch/x86_64/exceptions.rs`

```rust
// Point d'entrée manquant dans l'original — nécessaire pour le retour
// après préemption ou page fault géré
pub fn exception_return_to_user(tcb: &mut ThreadControlBlock) {
    // Vérifier signal pending (préemption, page fault, etc.)
    if tcb.signal_pending.load(Ordering::Acquire) {
        process::signal::delivery::handle_pending_signals(tcb);
    }
    // IRETQ
}
```

> ⚠️ **AJOUT par rapport à l'original :** `arch/exceptions.rs` était absent du
> résumé MODIFICATIONS du DOC1. Il est pourtant le point de retour userspace
> après préemption forcée — et doit donc orchestrer la livraison des signaux
> exactement comme `syscall.rs`.

---

### Règle absolue post-correction C1

```
RÈGLE SIGNAL-01 :
  scheduler/ NE connaît PAS process::signal::*
  scheduler/ LIT UNIQUEMENT tcb.signal_pending (AtomicBool dans TCB)
  process::signal::* ÉCRIT tcb.signal_pending
  arch/syscall.rs ET arch/exceptions.rs ORCHESTRENT la livraison

RÈGLE SIGNAL-02 (corrective) :
  switch.rs LIT le flag, NE LIVRE PAS les signaux
  La livraison s'effectue depuis arch/ APRÈS le retour de switch

INTERDIT ABSOLU :
  scheduler/core/switch.rs   → use process::signal::*  ✗
  scheduler/sync/*.rs        → use process::signal::*  ✗
  process::signal livré depuis scheduler/ directement   ✗
```

---

## ═══════════════════════════════════════════════════════════
## CORRECTION 2 — `capability/` : réorganisation TCB + bridge
## ═══════════════════════════════════════════════════════════

### Pourquoi c'était une erreur

**Problème A — Preuve formelle impossible :**
`capability/` dans `ipc/` → prouveur Coq/TLA+ doit traiter les rings SPSC/MPMC
lock-free, les canaux async, le SHM pool → des milliers de lignes hors périmètre.

**Problème B — Couplage `fs/` → `ipc/` fragile :**
`fs/ (couche 3) → ipc/capability/ (couche 2a)` : si `ipc/` change d'API → `fs/` casse.

### Solution : scission en deux niveaux

```
kernel/src/security/capability/    ← NOYAU TCB prouvé (~500 lignes)
kernel/src/ipc/capability_bridge/  ← SHIM léger (~50 lignes)
```

---

### Delta de l'arborescence

#### SUPPRIMÉ de `kernel/src/ipc/`

```
kernel/src/ipc/
  └── capability/          ← SUPPRIMÉ ENTIÈREMENT
      ├── token.rs / rights.rs / table.rs
      ├── revocation.rs / delegation.rs / namespace.rs
```

#### AJOUTÉ dans `kernel/src/security/`

```
kernel/src/security/
  └── capability/          ← NOYAU TCB — périmètre Coq/TLA+
      ├── mod.rs
      ├── PROOF_SCOPE.md   # Délimitation exacte: quels fichiers sont prouvés
      ├── model.rs         # Modèle formel: CapToken(128b), ObjectId(64b), Gen(32b)
      │
      │   ⚠️ NOTE TAILLE RÉELLE de CapToken :
      │   "128 bits inforgeable" = bits LOGIQUES (64+16+32+16 = 128 bits utiles)
      │   Taille mémoire réelle Rust = 18 bytes (u64+u16+u32+u16+u16_pad)
      │   À documenter dans PROOF_SCOPE.md pour éviter confusion avec size_of()
      │
      ├── token.rs         # CapToken inforgeable
      │                    # Layout: [ObjectId:64][Rights:16][Generation:32][Tag:16]
      ├── rights.rs        # Rights: READ|WRITE|EXEC|GRANT|REVOKE|DELEGATE
      ├── table.rs         # CapTable par processus (radix tree, O(log n))
      │                    # Source de vérité UNIQUE — toute vérif passe ici
      ├── revocation.rs    # Révocation O(1) via génération++
      │                    # Propriété Coq: aucun token révoqué n'est utilisable
      ├── delegation.rs    # Délégation (subset strict des droits)
      │                    # Propriété Coq: on ne délègue pas plus qu'on a
      └── namespace.rs     # Namespace capability (isolation conteneurs)
```

#### AJOUTÉ dans `kernel/src/ipc/`

```
kernel/src/ipc/
  └── capability_bridge/   ← SHIM — délègue TOUT à security/capability/
      ├── mod.rs
      └── bridge.rs        # Ajoute uniquement: EndpointId → ObjectId mapping
                           # Toute logique → security/capability/verify()
```

---

### Contenu de `security/capability/model.rs`

```rust
// kernel/src/security/capability/model.rs
// PÉRIMÈTRE DE PREUVE FORMELLE — modification = MAJ preuves Coq obligatoire

/// CapToken — 128 bits logiques, inforgeable
/// Taille size_of() = 18 bytes selon ABI Rust — voir PROOF_SCOPE.md
#[repr(C)]
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct CapToken {
    object_id:  ObjectId,    // u64
    rights:     Rights,      // u16
    generation: Generation,  // u32
    tag:        CapTag,      // u16
    _pad:       u16,
}

impl Rights {
    pub const READ:     Rights = Rights(1 << 0);
    pub const WRITE:    Rights = Rights(1 << 1);
    pub const EXEC:     Rights = Rights(1 << 2);
    pub const GRANT:    Rights = Rights(1 << 3);
    pub const REVOKE:   Rights = Rights(1 << 4);
    pub const DELEGATE: Rights = Rights(1 << 5);
}
```

### Contenu de `security/capability/revocation.rs`

```rust
// PROPRIÉTÉ PROUVÉE (Coq): ∀ token t, revoke(obj) → verify(t) = Err(Revoked)

pub fn revoke(table: &CapTable, object_id: ObjectId) {
    table.increment_generation(object_id, Ordering::Release);
    // Tous tokens avec ancienne génération → Err(Revoked) automatiquement
    // O(1) — jamais de parcours des tokens existants
}

/// Vérification — point d'entrée UNIQUE dans tout l'OS
pub fn verify(
    table:           &CapTable,
    token:           CapToken,
    required_rights: Rights,
) -> Result<(), CapError> {
    let entry = table.get(token.object_id).ok_or(CapError::ObjectNotFound)?;
    if entry.generation != token.generation { return Err(CapError::Revoked); }
    if !token.rights.contains(required_rights) { return Err(CapError::InsufficientRights); }
    Ok(())
}
```

### Contenu de `ipc/capability_bridge/bridge.rs`

```rust
// SHIM ~50 lignes — hors périmètre de preuve
use crate::security::capability::{CapToken, Rights, verify, CapTable};

pub fn verify_ipc_access(
    table: &CapTable, token: CapToken,
    endpoint_id: EndpointId, required: Rights,
) -> Result<(), IpcCapError> {
    let _object_id = endpoint_id.to_object_id(); // mapping IPC-spécifique
    verify(table, token, required).map_err(IpcCapError::from)
}
```

### Appelants modifiés

```rust
// AVANT (incorrect) :
use crate::ipc::capability::{CapToken, Rights};      // ✗

// APRÈS — fs/core/vfs.rs :
use crate::security::capability::{CapToken, Rights}; // ✓ fs → security direct

// APRÈS — process/lifecycle/exec.rs :
use crate::security::capability::{CapToken, Rights}; // ✓ process → security direct

// APRÈS — ipc/channel/sync.rs :
use crate::security::capability::CapToken;           // ✓ ipc → security
use crate::ipc::capability_bridge::bridge::IpcCapBridge; // ✓ ipc → bridge (interne)
```

---

### Règles absolues post-correction C2

```
RÈGLE CAP-01 (⚠️ CORRIGÉE par rapport à l'original) :
  security/capability/ = UNIQUE source de vérité dans tout l'OS
  fs/      → security/capability/ DIRECTEMENT
  process/ → security/capability/ DIRECTEMENT
  ipc/     → security/capability/ VIA capability_bridge/ UNIQUEMENT
             (nécessaire pour le mapping EndpointId → ObjectId)
  scheduler/ et memory/ → NE TOUCHENT JAMAIS aux capabilities

  ⚠️ ERREUR ORIGINALE : disait "fs/ et ipc/ appellent security/ directement"
     CORRIGÉ : ipc/ DOIT passer par capability_bridge/ (semantique endpoint)

RÈGLE CAP-02 (périmètre de preuve Coq/TLA+) :
  DANS le périmètre :
    security/capability/model.rs
    security/capability/token.rs
    security/capability/rights.rs
    security/capability/revocation.rs
    security/capability/delegation.rs
  HORS périmètre (implémentation, non prouvé) :
    security/capability/table.rs
    security/capability/namespace.rs
    ipc/capability_bridge/bridge.rs

RÈGLE CAP-03 (révocation O(1)) :
  Révocation = génération++ uniquement
  JAMAIS de parcours de tokens
  JAMAIS de liste de révocation

INTERDIT ABSOLU :
  use crate::ipc::capability_bridge dans fs/       ✗
  use crate::ipc::capability_bridge dans process/  ✗
  use crate::ipc::capability_bridge dans arch/     ✗
  Duplication de verify() hors security/capability/ ✗
```

---

## ═══════════════════════════════════════════════════════════
## CORRECTION 3 — Modules IA : 4 ajouts dans l'arborescence
## ═══════════════════════════════════════════════════════════

### Contraintes IA noyau (non négociables)

```
IA-KERNEL-01 : AUCUN modèle dynamique en Ring 0
  → Pas de chargement runtime, pas d'inférence allouant de mémoire
  → Uniquement : lookup tables statiques .rodata + EMA O(1)

IA-KERNEL-02 : Dégradation gracieuse obligatoire
  → hint = None → fallback déterministe immédiat

IA-KERNEL-03 : Séparation apprentissage / inférence
  → Apprentissage : tools/ai_trainer/ (userspace, offline)
  → Inférence : kernel (.rodata compilée ou EMA inline)
```

---

### AJOUT 1 — `memory/physical/allocator/ai_hints.rs`

```rust
// kernel/src/memory/physical/allocator/ai_hints.rs

/// Table NUMA — générée par tools/ai_trainer/, compilée en .rodata
/// 256 × 8 = 2KB — tient dans L2 cache, zéro allocation
static NUMA_HINTS: [[u8; 8]; 256] = include!("numa_hints_table.gen");

/// Bit 7 du hint = validité ; bits 0..6 = nœud NUMA cible
#[inline(always)]
pub fn hint_numa_node(size_class: u8, current_cpu: u8) -> Option<NumaNode> {
    let current_node = cpu_to_numa_node(current_cpu);
    let hint = NUMA_HINTS[size_class as usize][current_node as usize];
    if hint & 0x80 != 0 { Some(NumaNode(hint & 0x7F)) } else { None }
}

static AI_HINTS_ENABLED: AtomicBool = AtomicBool::new(true);
pub fn set_hints_enabled(enabled: bool) {
    AI_HINTS_ENABLED.store(enabled, Ordering::Relaxed);
}
```

**Intégration dans `buddy.rs` :**

```rust
pub fn alloc_pages(order: u32, flags: AllocFlags) -> Result<PhysAddr, AllocError> {
    let preferred_node = if ai_hints::AI_HINTS_ENABLED.load(Ordering::Relaxed) {
        ai_hints::hint_numa_node(order as u8, current_cpu())
    } else { None };
    let node = preferred_node.unwrap_or_else(|| current_numa_node());
    buddy_alloc_on_node(order, node, flags)
}
```

---

### AJOUT 2 — `scheduler/policies/ai_guided.rs`

```rust
// kernel/src/scheduler/policies/ai_guided.rs

/// Historique compact — 8 bytes inline dans TCB, zéro allocation
#[repr(C)]
pub struct ThreadAiState {
    cpu_burst_ema:   u16,  // EMA burst CPU (cycles/256)
    io_wait_ema:     u16,  // EMA attente I/O
    voluntary_ctx:   u16,  // context switches volontaires
    involuntary_ctx: u16,  // préemptions forcées
}

impl ThreadAiState {
    #[inline(always)]
    pub fn classify(&self) -> ThreadBehavior {
        let io_ratio = self.io_wait_ema as u32 * 100
            / (self.cpu_burst_ema as u32 + self.io_wait_ema as u32 + 1);
        match io_ratio {
            80..=100 => ThreadBehavior::IoBound,
            20..=79  => ThreadBehavior::Mixed,
            _ => if self.involuntary_ctx < 10 && self.cpu_burst_ema > 50_000 {
                ThreadBehavior::RealtimeCandidate
            } else { ThreadBehavior::CpuBound }
        }
    }

    #[inline(always)]
    pub fn vruntime_adjustment(&self) -> i64 {
        match self.classify() {
            ThreadBehavior::IoBound           => -1_000_000,
            ThreadBehavior::Mixed             =>          0,
            ThreadBehavior::CpuBound          =>  2_000_000,
            ThreadBehavior::RealtimeCandidate =>   -500_000,
        }
    }

    /// EMA α=1/8 — mis à jour après chaque context switch
    #[inline(always)]
    pub fn update_after_run(&mut self, cpu_cycles: u64, io_wait_cycles: u64) {
        self.cpu_burst_ema = ((self.cpu_burst_ema as u64 * 7
            + (cpu_cycles >> 8).min(0xFFFF)) / 8) as u16;
        self.io_wait_ema = ((self.io_wait_ema as u64 * 7
            + (io_wait_cycles >> 8).min(0xFFFF)) / 8) as u16;
    }
}
```

**Intégration dans `cfs.rs` :**

```rust
pub fn pick_next_task(rq: &mut RunQueue) -> Option<TaskRef> {
    let task = rq.leftmost()?;
    #[cfg(feature = "ai_scheduler")]
    {
        let adj = task.tcb.ai_state.vruntime_adjustment();
        if adj != 0 {
            task.tcb.vruntime = task.tcb.vruntime.saturating_add_signed(adj);
        }
    }
    Some(task)
}
```

---

### AJOUT 3 — `fs/cache/prefetch.rs` (complété)

```rust
// kernel/src/fs/cache/prefetch.rs

/// 12 bytes inline dans InodeData — zéro allocation
#[repr(C)]
pub struct PrefetchState {
    last_page:  u32,           // dernier offset accédé (en pages)
    stride:     i32,           // delta entre accès consécutifs
    window:     u8,            // fenêtre de prefetch (4..128 pages)
    hit_streak: u8,
    mode:       PrefetchMode,
    _pad:       u8,
}

impl PrefetchState {
    pub fn update_and_get_prefetch(&mut self, accessed_page: u32) -> &[u32] {
        let delta = accessed_page as i32 - self.last_page as i32;
        self.last_page = accessed_page;

        if delta == 1 {
            self.hit_streak = self.hit_streak.saturating_add(1);
            if self.hit_streak > 4 {
                self.mode = PrefetchMode::Sequential;
                self.window = (self.window * 2).min(128); // exponentiel jusqu'à 128
            }
        } else if delta == self.stride && delta != 0 {
            self.mode = PrefetchMode::Stride;
        } else {
            self.hit_streak = 0;
            self.window = (self.window / 2).max(4);
            if self.window <= 4 { self.mode = PrefetchMode::Disabled; }
        }
        self.stride = delta;
        PREFETCH_BUFFER.get_mut().fill_prefetch(accessed_page, self.window, self.stride)
    }
}
```

---

### AJOUT 4 — `tools/ai_trainer/` (userspace, offline)

```
tools/
  └── ai_trainer/
      ├── Cargo.toml
      └── src/
          ├── main.rs              # CLI : collect traces → generate tables
          ├── trace_parser.rs      # Parse traces exo-trace (format binaire)
          ├── numa_optimizer.rs    # Génère numa_hints_table.gen (2KB .rodata)
          └── scheduler_trainer.rs # Calibre seuils EMA ThreadAiState
```

**Règle absolue :** entraînement offline uniquement.
Résultat = table `.rodata` compilée dans le kernel. JAMAIS de mise à jour
du modèle en runtime Ring 0.

---

### Règles IA transversales

```
RÈGLE IA-01 : Apprentissage → tools/ai_trainer/ (offline)
              Inférence → .rodata + EMA O(1) uniquement
RÈGLE IA-02 : Fallback déterministe garanti si hint = None
              #[cfg(feature = "ai_*")] pour désactivation compile-time
RÈGLE IA-03 : Zéro allocation dans fonctions IA kernel
              (inline TCB/inode, .rodata, per-CPU static)
RÈGLE IA-04 : Overhead max : ai_hints ≤5c, ai_guided ≤10c, prefetch ≤100c
```

---

## RÉSUMÉ FINAL — Delta consolidé

```
DÉPLACEMENTS :
  kernel/src/scheduler/signal/  →  kernel/src/process/signal/
  kernel/src/ipc/capability/    →  kernel/src/security/capability/

AJOUTS NOYAU :
  kernel/src/ipc/capability_bridge/       (mod.rs + bridge.rs)
  kernel/src/memory/physical/allocator/ai_hints.rs
  kernel/src/scheduler/policies/ai_guided.rs
  kernel/src/fs/cache/prefetch.rs         (complété)

AJOUTS OUTILLAGE :
  tools/ai_trainer/

MODIFICATIONS NOYAU :
  scheduler/core/task.rs              +signal_pending, +signal_mask, +ai_state
  arch/x86_64/syscall.rs              +livraison signal au retour syscall
  arch/x86_64/exceptions.rs           +livraison signal au retour préemption/#PF
                                       ⚠️ MANQUAIT dans l'original
  memory/physical/allocator/buddy.rs  +hint NUMA optionnel
  scheduler/policies/cfs.rs           +vruntime_adjustment optionnel
  process/core/tcb.rs                 +ThreadAiState inline, +dma_completion_result
                                       ⚠️ dma_completion_result manquait dans original
```

---

## 📋 ERREURS CORRIGÉES DANS LE DOC1 ORIGINAL

| # | Localisation | Erreur originale | Correction appliquée |
|---|---|---|---|
| E1 | `delivery.rs` commentaire | "Appelé par scheduler/core/switch.rs" | **Supprimé** — switch.rs ne peut pas appeler process::signal (couche 1→1.5 interdit) |
| E2 | `RÈGLE CAP-01` | "fs/ et ipc/ appellent security/capability/ directement" | **Corrigé** — ipc/ DOIT passer par capability_bridge/ ; fs/ et process/ appellent directement |
| E3 | `model.rs` commentaire | "128 bits inforgeable" = taille mémoire | **Clarifié** — 128 bits = valeur logique, taille réelle = 18 bytes (ABI Rust) |
| E4 | Résumé MODIFICATIONS | `arch/exceptions.rs` absent | **Ajouté** — exceptions.rs orchestre la livraison au retour préemption |
| E5 | Résumé MODIFICATIONS | `+dma_completion_result` absent du TCB | **Ajouté** — champ requis par process/state/wakeup.rs |
| E6 | `check_signal_pending` commentaire | "scheduler appelle arch::signal_return_trampoline()" | **Clarifié** — c'est arch/ qui orchestre, switch.rs n'appelle rien |

---

## Règles consolidées (copier en tête de session)

```
[RÈGLES CORRECTIONS v1.2 — post-audit]

SIGNAL :
  scheduler/ LIT signal_pending (AtomicBool dans TCB) — UNIQUEMENT
  process/signal/ GÈRE toute la logique POSIX
  arch/syscall.rs + arch/exceptions.rs ORCHESTRENT la livraison

CAPABILITY :
  security/capability/ = source de vérité UNIQUE
  fs/ et process/ → security/capability/ DIRECTEMENT
  ipc/ → security/capability/ VIA capability_bridge/ UNIQUEMENT
  memory/ et scheduler/ → NE TOUCHENT JAMAIS aux capabilities

IA KERNEL :
  Inférence = .rodata + EMA O(1) — jamais de modèle dynamique Ring 0
  Apprentissage = tools/ai_trainer/ (offline userspace)
  Fallback déterministe garanti pour chaque module IA
  #[cfg(feature = "ai_*")] pour désactivation propre
```

---

*DOC 1 — Corrections arborescence — Exo-OS — v1.2 corrigé*
*Série : DOC1 · DOC2 · DOC3 · DOC4-9 · DOC10 (Userspace/Bootloader/Drivers)*
