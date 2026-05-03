# Correctifs P1 — Bugs SIL (silencieux / comportement erroné)
## ExoOS — Modules arch, memory, scheduler

**Auteur** : claude-alpha  
**Date** : 2026-05-03  
**Référence audit** : `AUDIT_ALPHA_ARCH_MEMORY_SCHEDULER.md`

---

## ALPHA-02 : TSS — Suppression des piles IST mortes

**Fichier** : `kernel/src/arch/x86_64/tss.rs`

### Problème détaillé

`PerCpuStacks` déclare 7 piles × 16 KiB = 112 KiB par CPU.
Seules 4 sont réellement assignées dans `init_tss_for_cpu()` :

| Champ struct | Slot IST assigné | Remarque |
|---|---|---|
| `df_stack`   | `ist[IST_DOUBLE_FAULT]` = ist[3] | ✅ utilisé |
| `nmi_stack`  | aucun | ❌ **mort** — NMI utilise `EARLY_IST_POOL` |
| `mc_stack`   | `ist[IST_MACHINE_CHECK]` = ist[4] | ✅ utilisé |
| `db_stack`   | `ist[IST_DEBUG]` = ist[5] | ✅ utilisé |
| `ist5_stack` | aucun | ❌ **mort** |
| `ist6_stack` | aucun | ❌ **mort** |
| `ist7_stack` | `ist[6]` | ✅ utilisé |

Sur 256 CPUs : **3 × 16 KiB × 256 = 12 582 912 octets** (~12 MiB) de BSS gaspillés.

### Patch

```rust
// AVANT :
struct PerCpuStacks {
    df_stack:   [u8; IST_STACK_SIZE],
    nmi_stack:  [u8; IST_STACK_SIZE],  // ← mort
    mc_stack:   [u8; IST_STACK_SIZE],
    db_stack:   [u8; IST_STACK_SIZE],
    ist5_stack: [u8; IST_STACK_SIZE],  // ← mort
    ist6_stack: [u8; IST_STACK_SIZE],  // ← mort
    ist7_stack: [u8; IST_STACK_SIZE],
}

impl PerCpuStacks {
    const fn zero() -> Self {
        Self {
            df_stack:   [0u8; IST_STACK_SIZE],
            nmi_stack:  [0u8; IST_STACK_SIZE],
            mc_stack:   [0u8; IST_STACK_SIZE],
            db_stack:   [0u8; IST_STACK_SIZE],
            ist5_stack: [0u8; IST_STACK_SIZE],
            ist6_stack: [0u8; IST_STACK_SIZE],
            ist7_stack: [0u8; IST_STACK_SIZE],
        }
    }
}

// APRÈS (CORR-ALPHA-02) :
/// Piles IST per-CPU — 4 piles uniquement (les 3 mortes supprimées).
///
/// Layout assigné :
///   ist4_df  → IST4 = ist[IST_DOUBLE_FAULT]   = ist[3]
///   ist5_mc  → IST5 = ist[IST_MACHINE_CHECK]   = ist[4]
///   ist6_db  → IST6 = ist[IST_DEBUG]            = ist[5]
///   ist7_rsv → IST7 = ist[6]                    (réserve)
///
/// Note : NMI (IST3) et ExoPhoenix IPIs (IST1), #PF (IST2) utilisent
///        EARLY_IST_POOL — aucune pile statique nécessaire ici.
#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct PerCpuStacks {
    ist4_df_stack:  [u8; IST_STACK_SIZE],
    ist5_mc_stack:  [u8; IST_STACK_SIZE],
    ist6_db_stack:  [u8; IST_STACK_SIZE],
    ist7_rsv_stack: [u8; IST_STACK_SIZE],
}

impl PerCpuStacks {
    const fn zero() -> Self {
        Self {
            ist4_df_stack:  [0u8; IST_STACK_SIZE],
            ist5_mc_stack:  [0u8; IST_STACK_SIZE],
            ist6_db_stack:  [0u8; IST_STACK_SIZE],
            ist7_rsv_stack: [0u8; IST_STACK_SIZE],
        }
    }
}
```

Adapter `init_tss_for_cpu()` :

```rust
// AVANT :
let df_top = stacks.df_stack.as_ptr() as u64 + IST_STACK_SIZE as u64;
let mc_top = stacks.mc_stack.as_ptr() as u64 + IST_STACK_SIZE as u64;
let db_top = stacks.db_stack.as_ptr() as u64 + IST_STACK_SIZE as u64;
let ist7_top = stacks.ist7_stack.as_ptr() as u64 + IST_STACK_SIZE as u64;

// APRÈS (CORR-ALPHA-02) :
let df_top  = stacks.ist4_df_stack.as_ptr()  as u64 + IST_STACK_SIZE as u64;
let mc_top  = stacks.ist5_mc_stack.as_ptr()  as u64 + IST_STACK_SIZE as u64;
let db_top  = stacks.ist6_db_stack.as_ptr()  as u64 + IST_STACK_SIZE as u64;
let ist7_top= stacks.ist7_rsv_stack.as_ptr() as u64 + IST_STACK_SIZE as u64;
```

### Vérification BSS

Avant : `MAX_CPUS × 7 × IST_STACK_SIZE = 256 × 7 × 16384 = 29 360 128 B` (~28 MiB)  
Après : `MAX_CPUS × 4 × IST_STACK_SIZE = 256 × 4 × 16384 = 16 777 216 B` (~16 MiB)  
**Gain : ~12 MiB BSS**

---

## ALPHA-04 : Doc — ThreadId u32 → u64

**Fichier** : `docs/kernel/scheduler/SCHEDULER_CORE.md`

### Patch

```markdown
<!-- AVANT : -->
```rust
pub struct ThreadId(pub u32);     // identifiant unique de thread
```

<!-- APRÈS (CORR-ALPHA-04) : -->
```rust
/// Identifiant unique de thread — 64 bits (monotone croissant depuis le boot).
/// Jamais réutilisé pendant la vie du kernel (architecture v7 §3.2).
pub struct ThreadId(pub u64);
```
```

---

## ALPHA-05 : Implémenter `TaskState::try_transition()`

**Fichier** : `kernel/src/scheduler/core/task.rs`

### Patch — Ajout dans `impl ThreadControlBlock`

Placer après les méthodes `state()` et `set_state()` existantes :

```rust
// ─── Transition atomique d'état ───────────────────────────────────────────────

/// Tente une transition d'état atomique via CAS sur `sched_state[7:0]`.
///
/// Retourne `true` si la transition de `from` vers `to` a réussi.
/// Retourne `false` si l'état courant ne correspondait pas à `from`
/// (un autre CPU a pu changer l'état entre la lecture et le CAS).
///
/// # Utilisation
/// ```rust
/// // Dans un context de wakeup par wait_queue :
/// if tcb.try_transition(TaskState::Sleeping, TaskState::Runnable) {
///     rq.enqueue(tcb_ptr);
/// }
/// // Si false → un autre CPU a déjà réveillé ou tué le thread.
/// ```
///
/// # Thread-safety
/// Cette méthode est ISR-safe (Ordering::AcqRel + Acquire).
/// Peut être appelée depuis n'importe quel CPU.
#[inline(always)]
pub fn try_transition(&self, from: TaskState, to: TaskState) -> bool {
    let from_u64 = from as u64;
    let to_u64   = to   as u64;

    // Lire l'état courant avec Acquire pour garantir la visibilité des
    // écritures précédentes sur ce TCB (ex : champs wakeup).
    let mut old = self.sched_state.load(Ordering::Acquire);

    loop {
        // Vérifier que les bits d'état [7:0] correspondent à `from`.
        if (old & SCHED_STATE_MASK) != from_u64 {
            return false; // état inattendu — transition abandonnée
        }

        // Construire la nouvelle valeur : conserver tous les flags sauf [7:0].
        let new = (old & !SCHED_STATE_MASK) | to_u64;

        match self.sched_state.compare_exchange_weak(
            old, new,
            Ordering::AcqRel,  // succès : Release (visible aux autres CPUs)
            Ordering::Acquire, // échec : Acquire (relire la valeur fraîche)
        ) {
            Ok(_)    => return true,
            Err(cur) => old = cur, // réessayer avec la valeur lue
        }
    }
}

/// Version infaillible — panic si la transition échoue.
/// À utiliser uniquement dans les chemins où la transition DOIT réussir.
///
/// # Panics
/// En debug uniquement. En release, le comportement est indéfini si
/// la transition échoue (état corrompu).
#[inline(always)]
#[track_caller]
pub fn force_transition(&self, from: TaskState, to: TaskState) {
    debug_assert!(
        self.try_transition(from, to),
        "force_transition: état attendu {:?} mais trouvé {:?}",
        from,
        TaskState::from_u8((self.sched_state.load(Ordering::Relaxed) & SCHED_STATE_MASK) as u8)
    );
    // En release : écriture directe sans CAS (non-race si invariant respecté).
    #[cfg(not(debug_assertions))]
    {
        let old = self.sched_state.load(Ordering::Acquire);
        let new = (old & !SCHED_STATE_MASK) | to as u64;
        self.sched_state.store(new, Ordering::Release);
    }
}
```

---

## ALPHA-06 : Doc — TaskState::Blocked → Sleeping / Uninterruptible

**Fichier** : `docs/kernel/scheduler/SCHEDULER_CORE.md`

### Patch

```markdown
<!-- AVANT : -->
### État du thread

```rust
pub enum TaskState {
    Running,   // Actuellement sur un CPU
    Runnable,  // Prêt, dans une run queue
    Blocked,   // En attente d'un événement (wait_queue)
    Zombie,    // Terminé, en attente de join/reap
    Dead,      // Totalement terminé
}
```

**Transitions atomiques** via `task.try_transition(from, to)` (compare-and-swap).

<!-- APRÈS (CORR-ALPHA-06) : -->
### État du thread

```rust
pub enum TaskState {
    Runnable        = 0, // Prêt, dans la run queue
    Running         = 1, // En exécution sur un CPU
    Sleeping        = 2, // Bloqué interruptible (signal POSIX peut réveiller)
    Uninterruptible = 3, // Bloqué non interruptible (I/O critique, SIGKILL résiste)
    Stopped         = 4, // Stoppé par SIGSTOP
    Zombie          = 5, // Terminé, attend reap du parent
    Dead            = 6, // Totalement nettoyé
}
```

**Transitions atomiques** via `tcb.try_transition(from, to)` — compare-and-swap
sur `sched_state[7:0]`. Retourne `bool` : `true` si la transition a réussi,
`false` si un autre CPU a changé l'état entretemps.

| Transition | Contexte |
|---|---|
| `Runnable → Running` | `context_switch()` côté `next` |
| `Running → Runnable` | `context_switch()` côté `prev` (yield) |
| `Running → Sleeping` | `schedule_block()` avant wait_queue |
| `Sleeping → Runnable` | `wake_enqueue()` depuis wait_queue |
| `Running → Stopped` | `do_signal(SIGSTOP)` |
| `Stopped → Runnable` | `do_signal(SIGCONT)` |
| `Running → Zombie` | `do_exit()` |
| `Zombie → Dead` | `reap()` depuis le parent |
```

---

## ALPHA-07 : Commentaire FFI `context_switch_asm` — correction

**Fichier** : `kernel/src/scheduler/core/switch.rs`

### Patch

```rust
// AVANT (commentaire trompeur) :
extern "C" {
    /// Context switch ASM complet.
    ///
    /// Sauvegarde les registres callee-saved (rbx, rbp, r12-r15) + MXCSR + x87 FCW
    /// du thread `old`, puis switche CR3 si nécessaire (KPTI), puis restaure
    /// le contexte du thread `new`.
    fn context_switch_asm(old_kernel_rsp: *mut u64, new_kernel_rsp: u64, new_cr3: u64);
}

// APRÈS (CORR-ALPHA-07 — commentaire précis) :
extern "C" {
    /// Context switch ASM — registres callee-saved uniquement (V7-C-02 / CORR-18).
    ///
    /// # Registres sauvegardés / restaurés
    /// Les 6 registres callee-saved ABI System V AMD64 :
    ///   rbx, rbp, r12, r13, r14, r15
    ///
    /// # Registres NON touchés par cet ASM
    /// - MXCSR, x87 FCW, registres XMM/YMM/ZMM :
    ///   gérés exclusivement par `scheduler::fpu::save_restore` via XSAVE/XRSTOR.
    ///   Appelés AVANT l'ASM dans `context_switch()` (RÈGLE SWITCH-02).
    /// - FS.base, GS.base :
    ///   gérés par `context_switch()` via rdmsr/wrmsr (CORR-11).
    ///
    /// # CR3
    /// Si `new_cr3 != 0`, CR3 est commuté AVANT la restauration des registres
    /// du thread entrant, garantissant l'atomicité KPTI.
    ///
    /// # Arguments (System V ABI)
    /// - `old_kernel_rsp` : `*mut u64` → `TCB::kstack_ptr` du thread sortant
    /// - `new_kernel_rsp` : valeur de `TCB::kstack_ptr` du thread entrant
    /// - `new_cr3`        : CR3 à charger (0 = pas de switch, même espace d'adressage)
    fn context_switch_asm(old_kernel_rsp: *mut u64, new_kernel_rsp: u64, new_cr3: u64);
}
```

---

## ALPHA-09 : SchedNodePool — passage à 256 blocs

**Fichier** : `kernel/src/memory/physical/frame/emergency_pool.rs`

### Patch complet

```rust
// AVANT :
const SCHED_POOL_SIZE: usize = 64;

struct SchedNodePool {
    blocks:    UnsafeCell<[RawBlock64; SCHED_POOL_SIZE]>,
    free_bits: AtomicU64,           // 64 bits → 64 blocs max
    initialized: AtomicBool,
    alloc_count: AtomicUsize,
    exhausted:   AtomicUsize,
}

// APRÈS (CORR-ALPHA-09) :
// EMERGENCY_POOL_SIZE = 256 → le pool sched doit pouvoir les absorber tous.
// Sur 256 CPUs avec plusieurs wait queues actives, 64 blocs est insuffisant.
const SCHED_POOL_SIZE: usize = 256;

// 256 blocs nécessitent un bitmap de 256 bits = 4 × AtomicU64.
struct SchedNodePool {
    blocks:      UnsafeCell<[RawBlock64; SCHED_POOL_SIZE]>,
    free_bits:   [AtomicU64; 4],   // 4 × 64 = 256 bits → 256 blocs
    initialized: AtomicBool,
    alloc_count: AtomicUsize,
    exhausted:   AtomicUsize,
}

unsafe impl Sync for SchedNodePool {}
unsafe impl Send for SchedNodePool {}

impl SchedNodePool {
    const fn new_uninit() -> Self {
        SchedNodePool {
            blocks:      UnsafeCell::new(unsafe { core::mem::zeroed() }),
            free_bits:   [
                AtomicU64::new(0), // initialisés à 0 ; mis à !0 dans init()
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
            initialized: AtomicBool::new(false),
            alloc_count: AtomicUsize::new(0),
            exhausted:   AtomicUsize::new(0),
        }
    }

    unsafe fn init(&self) {
        if self.initialized.load(Ordering::Acquire) {
            return;
        }
        // Marquer les 256 blocs comme libres
        for word in &self.free_bits {
            word.store(u64::MAX, Ordering::Release);
        }
        self.initialized.store(true, Ordering::Release);
    }

    /// Alloue un bloc de 64 bytes — O(1) amorti via trailing_zeros sur 4 mots.
    fn alloc(&self) -> *mut u8 {
        if !self.initialized.load(Ordering::Acquire) {
            return core::ptr::null_mut();
        }

        // Itérer sur les 4 mots du bitmap pour trouver un bloc libre.
        for (word_idx, word) in self.free_bits.iter().enumerate() {
            loop {
                let bits = word.load(Ordering::Acquire);
                if bits == 0 {
                    break; // ce mot est épuisé, passer au suivant
                }
                let bit_idx = bits.trailing_zeros() as usize;
                let global_idx = word_idx * 64 + bit_idx;
                if global_idx >= SCHED_POOL_SIZE {
                    break;
                }
                let new_bits = bits & !(1u64 << bit_idx);
                match word.compare_exchange_weak(
                    bits, new_bits,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        self.alloc_count.fetch_add(1, Ordering::Relaxed);
                        // SAFETY: global_idx < SCHED_POOL_SIZE, blocks initialisé.
                        return unsafe {
                            (*self.blocks.get())[global_idx].data.as_mut_ptr()
                        };
                    }
                    Err(_) => continue, // CAS échoué, retry sur ce mot
                }
            }
        }

        // Tous les blocs sont occupés.
        self.exhausted.fetch_add(1, Ordering::Relaxed);
        core::ptr::null_mut()
    }

    /// Libère un bloc alloué par `alloc()`.
    ///
    /// # Safety
    /// `ptr` doit être un pointeur retourné par `alloc()` de ce pool.
    unsafe fn free(&self, ptr: *mut u8) {
        if ptr.is_null() {
            return;
        }
        let blocks_base = unsafe { (*self.blocks.get()).as_ptr() as usize };
        let ptr_addr    = ptr as usize;
        let pool_size   = core::mem::size_of::<[RawBlock64; SCHED_POOL_SIZE]>();

        if ptr_addr < blocks_base || ptr_addr >= blocks_base + pool_size {
            debug_assert!(false, "SchedNodePool::free() — pointeur hors bornes");
            return;
        }

        let offset     = ptr_addr - blocks_base;
        let global_idx = offset / 64;
        let word_idx   = global_idx / 64;
        let bit_idx    = global_idx % 64;

        // Remettre le bit à 1 (bloc libre).
        self.free_bits[word_idx].fetch_or(1u64 << bit_idx, Ordering::Release);
        self.alloc_count.fetch_sub(1, Ordering::Relaxed);
    }
}

// Assertion statique : bitmap couvre exactement SCHED_POOL_SIZE blocs.
const _: () = assert!(
    4 * 64 == SCHED_POOL_SIZE,
    "SCHED_POOL_SIZE doit être 256 (4 mots de 64 bits)"
);
```

---

*— claude-alpha*
