# Scheduler FFI Bridges — Ponts C ABI complets

> Carte exhaustive de toutes les frontières FFI entre `scheduler/`, `arch/x86_64/`, et `memory/`.

---

## Table des matières

1. [Direction scheduler → arch/](#1-direction-scheduler--arch)
2. [Direction arch/ → scheduler (exports #[no_mangle])](#2-direction-arch--scheduler-exports-no_mangle)
3. [Direction scheduler → memory/](#3-direction-scheduler--memory)
4. [Règles des ponts FFI](#4-règles-des-ponts-ffi)
5. [Diagramme de flux complet](#5-diagramme-de-flux-complet)

---

## 1. Direction scheduler → arch/

Déclarés en `extern "C"` dans les fichiers scheduler/. Implémentés dans `arch/x86_64/`.

### FPU (fpu/save_restore.rs)

```c
// Sauvegarde l'état XSAVE dans le buffer (ptr = FpuState.buffer, align 64)
// mask_lo/hi = XCR0 bitmask décomposé en 2×32 bits
void arch_xsave64(uint8_t *ptr, uint32_t mask_lo, uint32_t mask_hi);

// Restauration état XSAVE
void arch_xrstor64(const uint8_t *ptr, uint32_t mask_lo, uint32_t mask_hi);

// Fallback FXSAVE 512 octets (SSE uniquement, sans XSAVE)
void arch_fxsave64(uint8_t *ptr);
void arch_fxrstor64(const uint8_t *ptr);

// Détection CPUID (appelées une seule fois à l'init, résultat mis en cache)
bool arch_has_xsave(void);  // CPUID.01H:ECX[26]
bool arch_has_avx(void);    // CPUID.01H:ECX[28]
```

**Fichier impl** : `arch/x86_64/cpu/fpu.rs`

### SMP (smp/migration.rs)

```c
// Envoie un IPI RESCHEDULE au CPU cible (vecteur dédié dans IDT)
void arch_send_reschedule_ipi(uint32_t target_cpu);

// Retourne le numéro du CPU courant
// Implémentation : lit CPUID.1H:EBX[31:24] ou MSR_TSC_AUX
uint32_t arch_current_cpu(void);
```

**Fichier impl** : `arch/x86_64/smp/ipi.rs`

### Energy (energy/frequency.rs)

```c
// Écrit p_state dans IA32_PERF_CTL MSR (0x199) sur le CPU donné
// p_state encodé selon Intel EIST : [15:8] = ratio target
void arch_set_cpu_pstate(uint32_t cpu, uint32_t p_state);
```

**Fichier impl** : `arch/x86_64/cpu/msr.rs`

---

## 2. Direction arch/ → scheduler (exports #[no_mangle])

Compilés avec `#[no_mangle] pub unsafe extern "C"`. Appelés depuis `arch/x86_64/`.

### Handler Exception #NM — Device Not Available

```c
// Appelé depuis le stub IDT de l'exception #NM (vecteur 7)
// tcb_ptr : pointeur vers le ThreadControlBlock du thread courant
// SCHED-16 : seule interface pour accéder aux instructions FPU
void sched_fpu_handle_nm(uint8_t *tcb_ptr);
```

**Fichier impl** : `scheduler/fpu/lazy.rs`

**Ce que fait le handler** :
1. Cast `tcb_ptr` → `&mut ThreadControlBlock`
2. `cr0_clear_ts()` (CLTS)
3. `alloc_fpu_state(tcb)` si premier accès
4. `xrstor_for(tcb)` — restaure état FPU via `arch_xrstor64`
5. `tcb.set_fpu_loaded(true)`
6. Retour : l'instruction FPU incriminée est rejouée automatiquement

**Fichier appelant** : `arch/x86_64/exceptions.rs`, handler IDT[7]

---

### Handler IPI Reschedule

```c
// Appelé depuis le handler IPI (vecteur dédié, ex. 0xF0) sur le CPU cible
// tcb_ptr : TCB du thread actuellement en cours sur ce CPU
void sched_ipi_reschedule(uint8_t *tcb_ptr);
```

**Fichier impl** : `scheduler/timer/tick.rs`

**Ce que fait le handler** :
1. Cast `tcb_ptr` → `&mut ThreadControlBlock`
2. `tcb.flags.fetch_or(NEED_RESCHED, SeqCst)`
3. Retour : le flag sera vérifié au prochain point de retour d'IRQ

**Fichier appelant** : `arch/x86_64/smp/ipi_handler.rs`, handler IPI vecteur reschedule

---

### Context Switch (asm direct)

```c
// Appelé depuis scheduler/core/switch.rs (Rust) via global_asm! reference
// save_rsp : adresse où sauvegarder RSP du thread sortant
// load_rsp : RSP du thread entrant (déjà dans la pile de ce thread)
// next_cr3 : adresse physique PML4 du thread entrant
void context_switch_asm(uint64_t *save_rsp, uint64_t load_rsp, uint64_t next_cr3);
```

**Fichier impl** : `scheduler/asm/switch_asm.s` (AT&T, LLVM integrated assembler)

---

## 3. Direction scheduler → memory/

### EmergencyPool — WaitNodes (WAITQ-01)

```c
// Alloue un WaitNode depuis le pool d'urgence (pas d'allocateur heap)
// Retourne NULL si le pool est épuisé (64 slots maximum)
// Appelé avec préemption désactivée (IrqGuard)
struct WaitNode *emergency_pool_alloc_wait_node(void);

// Libère un WaitNode dans le pool d'urgence
void emergency_pool_free_wait_node(struct WaitNode *node);
```

**Fichier impl** : `memory/physical/frame/emergency_pool.rs`

**Fichier appelant** : `scheduler/sync/wait_queue.rs` (WaitNode::alloc et WaitNode::free)

### Structure WaitNode (partagé memory↔scheduler)

```c
// Taille = 32 octets, alignement = 8 octets
struct WaitNode {
    struct ThreadControlBlock *tcb;  // Thread en attente
    struct WaitNode           *next; // Suivant dans la queue
    struct WaitNode           *prev; // Précédent dans la queue
    uint32_t                   flags; // EXCLUSIVE = 1<<0
    uint32_t                   _pad;
};
```

### Allocateur heap — FpuState (via Rust global allocator)

```rust
// Dans fpu/save_restore.rs::alloc_fpu_state()
extern "C" {
    fn __rust_alloc(size: usize, align: usize) -> *mut u8;
    fn __rust_dealloc(ptr: *mut u8, size: usize, align: usize);
}
```

Fourni par le global allocator de `memory/` (`exo_allocator`). Utilisé une seule fois par thread lors du premier accès FPU.

---

## 4. Règles des ponts FFI

### Convention C ABI

Toutes les fonctions FFI utilisent `extern "C"` (ABI System V x86-64) :
- Paramètres : rdi, rsi, rdx, rcx, r8, r9 (dans cet ordre)
- Valeur de retour : rax (+ rdx pour 128 bits)
- Registres sauvés par l'appelé : rbx, rbp, r12-r15
- Registres sauvés par l'appelant : rax, rcx, rdx, rsi, rdi, r8-r11

### Sécurité des pontages

| Pont | Unsafe requis | Raison |
|------|--------------|--------|
| `arch_xsave64` | ✅ Oui | Pointeur brut + accès registres FPU |
| `arch_send_reschedule_ipi` | ✅ Oui | Effet de bord matériel (IPI) |
| `arch_set_cpu_pstate` | ✅ Oui | Écriture MSR |
| `sched_fpu_handle_nm` | ✅ Oui | Appelé depuis handler d'exception |
| `sched_ipi_reschedule` | ✅ Oui | Appelé depuis handler IPI |
| `emergency_pool_alloc_wait_node` | ✅ Oui | Pointeur brut, pas de Drop automatique |

### Invariants garantis

1. **scheduler → arch** : jamais appelé avec préemption activée ET en contexte user.
2. **arch → scheduler** : handlers appelés avec IRQ désactivées (dans le handler IDT).
3. **scheduler → memory** : `emergency_pool_alloc_wait_node` appelé systématiquement sous `IrqGuard`.
4. **Aucun appel FFI** depuis les lock guards (évite deadlocks cross-layer).

---

## 5. Diagramme de flux complet

```
arch/x86_64/interrupts/         scheduler/                  memory/
───────────────────────         ──────────────────────────  ───────────────

Timer IRQ ──────────────────►  scheduler_tick()
                                 ├── drain_pending_migrations()
                                 ├── advance_vruntime()
                                 ├── cfs/rt/deadline tick
                                 └── balance_cpu()
                                       └── cfs_dequeue_for_migration()

IPI vecteur reschedule ────────► sched_ipi_reschedule()
                                    └── tcb.flags |= NEED_RESCHED

Exception #NM (vecteur 7) ─────► sched_fpu_handle_nm()
                                    ├── cr0_clear_ts()
                                    ├── arch_xrstor64() ────────────────────► (arch)
                                    └── set_fpu_loaded(true)

context_switch() ──────────────► xsave_current(prev)
                                    └── arch_xsave64() ─────────────────────► (arch)
                                  context_switch_asm()  ← switch_asm.s
                                    └── mov %rdx, %cr3 (KPTI)

WaitQueue::wait() ─────────────────────────────────────► emergency_pool_alloc()

migration IPI outbound ────────► arch_send_reschedule_ipi() ─────────────────► (arch)

P-state change ────────────────► arch_set_cpu_pstate() ──────────────────────► (arch)
```

### Légende

```
──────────────────────────►  Appel depuis X vers Y
(arch)                       Implémenté dans arch/x86_64/
```
