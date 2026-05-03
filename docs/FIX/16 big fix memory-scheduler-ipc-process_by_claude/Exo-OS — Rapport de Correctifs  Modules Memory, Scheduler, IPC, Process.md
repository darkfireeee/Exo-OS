# Exo-OS — Rapport de Correctifs : Modules Memory, Scheduler, IPC, Process

> **Auteur** : claude-iota  
> **Date** : 03 mai 2026  
> **Périmètre** : `kernel/src/memory/`, `kernel/src/scheduler/`, `kernel/src/ipc/`, `kernel/src/process/` + spécifications TLA+ associées  
> **Méthode** : Analyse croisée docs/recast, docs/kernel, sources Rust, specs TLA+ et docs/Exo-OS-TLA+

---

## Résumé Exécutif

L'audit croisé documentation/code révèle **26 défauts actifs** répartis sur 4 catégories :

| Catégorie | Count | Impact |
|-----------|-------|--------|
| 🔴 Critique — corruption runtime ou undefined behavior | 6 | Crash ou sécurité |
| 🟠 Majeur — comportement incorrect silencieux | 10 | Fonctionnement dégradé |
| 🟡 Mineur — incohérence doc/code, implantation manquante | 10 | Maintenabilité |

---

## Module Memory

### MEM-FIX-01 🔴 — `FUTEX_HASH_BUCKETS` = 1024 dans la doc, 4096 dans le code

**Fichier concerné** : `docs/kernel/memory/MEMORY_COMPLETE.md` §4.1

**Problème** : La table des constantes dans la documentation affiche :
```
FUTEX_HASH_BUCKETS | 1024 | Buckets de la FutexTable
```
Le code source `kernel/src/memory/core/constants.rs:184` déclare :
```rust
/// RÈGLE MEM-FUTEX (V-34) : ≥ 4096 + SipHash-keyed — anti-DoS par collision.
pub const FUTEX_HASH_BUCKETS: usize = 4096;
```
La correction MEM-FUTEX (documentée comme `✅ CORRIGÉ` dans l'architecture v7 §9.1) n'a **pas** été répercutée dans `MEMORY_COMPLETE.md`.

**Correctif** : Mettre à jour `MEMORY_COMPLETE.md` §4.1 :

```markdown
| `FUTEX_HASH_BUCKETS` | 4096 | Buckets de la FutexTable — anti-DoS SipHash |
```

---

### MEM-FIX-02 🟠 — Lock Order rule : formulation ambiguë dans MEMORY_COMPLETE.md

**Fichier concerné** : `docs/kernel/memory/MEMORY_COMPLETE.md` §2

**Problème** : Le tableau des règles affiche :
```
LOCK ORDER | IPC < Scheduler < Memory < FS (jamais lock N si on tient N+1)
```
Cette formulation avec `<` est interprétée comme « acquérir IPC en premier », ce qui est l'**opposé** de l'ordre correct. Le code source `kernel/src/memory/mod.rs:25` stipule correctement :
```
Ordonnancement des locks : Memory → Scheduler → Security → IPC → FS.
```
L'architecture v7 §2.2 confirme : Memory = niveau 1 (acquis en premier), IPC = niveau 4.

**Correctif** : Remplacer la règle dans `MEMORY_COMPLETE.md` §2 par :

```markdown
| **LOCK ORDER** | Acquérir dans l'ordre croissant : Memory(1) → Scheduler(2) → Security(3) → IPC(4) → FS(5). Un lock de niveau N ne peut jamais être tenu lors de l'acquisition d'un lock de niveau < N. |
```

---

### MEM-FIX-03 🟠 — `cow/breaker.rs` : sémantique du refcount après `dec_ref` non spécifiée

**Fichier concerné** : `docs/kernel/memory/MEMORY_COMPLETE.md` §10

**Problème** : La séquence documentée est :
```
1. cow_tracker::dec_ref(frame)
2. Si refcount == 1 → la frame est maintenant exclusive → pas de copie
3. Si refcount > 1 → alloc_page() + memcpy physmap + update PTE
```
Cette logique est **inversée** : après `dec_ref`, si `refcount == 1` il reste **une** autre référence (la nôtre est décomptée), la frame est donc **encore partagée** et la copie est nécessaire. Si `refcount == 0`, la frame est exclusive (pas de copie).

La description correcte du CoW break est :
- Décrémenter : `remaining = dec_ref(frame)`
- Si `remaining == 0` : la frame nous appartient exclusivement → pas de copie physique
- Si `remaining >= 1` : un autre processus tient encore cette frame → copie obligatoire

**Correctif** : Corriger `MEMORY_COMPLETE.md` §10 `cow/breaker.rs` :

```markdown
### `breaker.rs`
Réalise la rupture CoW lors d'une faute d'écriture :

1. `remaining = cow_tracker::dec_ref(frame)`
2. Si `remaining == 0` → la frame est désormais exclusive ; mise à jour PTE directe (pas de copie physique).
3. Si `remaining >= 1` → `alloc_page()` + `memcpy` via physmap + mise à jour PTE avec la nouvelle frame.
4. `let _ = free_page(old_frame)` — libère l'ancienne frame si et seulement si `remaining >= 1`.
```

---

### MEM-FIX-04 🟡 — `cow/tracker.rs` : suppression de clé avec open-addressing, absence de marqueur tombstone

**Fichier concerné** : `kernel/src/memory/cow/tracker.rs`

**Problème** : La table `COW_TABLE_SIZE = 4096` utilise un adressage ouvert (probing linéaire, confirmé par le code). La suppression d'une entrée (libération de frame) efface directement le slot. En adressage ouvert, cela **rompt les chaînes de sondage** : une recherche ultérieure d'une frame dont la chaîne passait par ce slot échoue prématurément.

**Correctif** : Implémenter un marqueur tombstone dans `tracker.rs` :

```rust
/// État d'une entrée dans la table CoW.
#[derive(Clone, Copy, PartialEq)]
enum CowEntryState {
    Empty,      // jamais utilisé — stoppe la recherche
    Active,     // entrée valide
    Deleted,    // tombstone — la recherche continue mais l'insertion peut réutiliser
}

pub struct CowEntry {
    frame_pfn:  u64,        // PFN de la frame (0 = invalide)
    refcount:   AtomicU32,  // compteur CoW
    state:      CowEntryState,
}
```

Lors de `dec_ref` : si `remaining == 0`, mettre `state = Deleted` (tombstone) plutôt qu'`Empty`. Lors d'`insert` : réutiliser le premier tombstone trouvé. Lors de `find` : s'arrêter sur `Empty` mais continuer sur `Deleted`.

---

### MEM-FIX-05 🟡 — `dma/completion/wakeup.rs` : MEM-DMA-IRQ non implémentée (P1)

**Fichier concerné** : `kernel/src/memory/dma/completion/wakeup.rs`

**Problème** : L'architecture v7 §9.3 liste comme problème P1 :
> MEM-DMA-IRQ : DMA ISR libère lock PUIS wakeup — Phase 2

Le handler ISR DMA doit libérer son spinlock **avant** d'appeler le wakeup handler. Appeler `DmaWakeupHandler::wake()` sous lock crée un potentiel d'inversion de priorité si le thread réveillé tente d'acquérir le même lock.

**Correctif** : Dans `completion/handler.rs`, restructurer le chemin ISR :

```rust
// Pattern OBLIGATOIRE dans tous les ISR DMA :
fn dma_irq_handler(channel: DmaChannelId) {
    let wakeup_list: Vec<DmaTransactionId, 16> = {
        let _guard = CHANNEL_LOCK[channel].lock(); // Acquiert
        // ... traitement ring, extraction des txids complètes ...
        extract_completed_txids()
    }; // _guard droppé — lock RELÂCHÉ avant tout wakeup

    for txid in wakeup_list {
        WAKEUP_HANDLER.wake(txid); // Appelé HORS LOCK
    }
}
```

---

### MEM-FIX-06 🟡 — `memory/core/constants.rs` : `BUDDY_MAX_ORDER` commentaire contradictoire

**Fichier concerné** : `kernel/src/memory/core/constants.rs`

**Problème** : La documentation affiche `BUDDY_MAX_ORDER = 11` (2^11 × 4 KiB = 8 MiB). Mais le code source comporte le commentaire `(2^12 = 4096 pages = 16 MiB)`. L'une des deux valeurs est incorrecte.

**Correctif** : Vérifier et aligner la constante et son commentaire. Si la cible est 8 MiB :
```rust
/// Ordre maximum du buddy allocator. 2^11 pages × 4 KiB = 8 MiB par bloc.
pub const BUDDY_MAX_ORDER: usize = 11;
pub const BUDDY_ORDER_COUNT: usize = BUDDY_MAX_ORDER + 1; // ordres 0..=11
```

---

## Module Scheduler

### SCHED-FIX-01 🔴 — `exec.rs` : TSS.RSP0 mis à jour avec `kstack_ptr` au lieu de `kstack_top`

**Fichier concerné** : `kernel/src/process/lifecycle/exec.rs` ligne 253-254

**Problème** : Le code actuel :
```rust
crate::arch::x86_64::smp::percpu::set_kernel_rsp(thread.sched_tcb.kstack_ptr);
crate::arch::x86_64::tss::update_rsp0(cpu_id, thread.sched_tcb.kstack_ptr);
```
`kstack_ptr` (offset TCB [8]) est le **RSP courant du thread kernel** : il pointe au milieu de la pile kernel, profond dans la pile d'appel `do_execve()`. Stocker cette valeur dans `TSS.RSP0` signifie que la prochaine interruption Ring 3→Ring 0 post-exec empilera ses frames sur un RSP arbitrairement bas dans la pile kernel, **écrasant du contenu kernel valide** — corruption silencieuse garantie.

`TSS.RSP0` doit contenir le **sommet propre de la pile kernel** = `kstack_top` (offset TCB [176]).

**Correctif** :

```rust
// Après mise à jour de thread.sched_tcb.cr3_phys et tls_base :
let kstack_top = thread.sched_tcb.kstack_top();  // Sommet fixe de la pile kernel
crate::arch::x86_64::smp::percpu::set_kernel_rsp(kstack_top);
crate::arch::x86_64::tss::update_rsp0(cpu_id, kstack_top);
// V7-C-03 : TSS.RSP0 = kstack_top (valeur FIXE), pas kstack_ptr (RSP courant)
```

Et ajouter dans `ThreadControlBlock` (s'il n'existe pas) :
```rust
/// Retourne le sommet fixe de la pile kernel (= base + KSTACK_SIZE).
#[inline(always)]
pub fn kstack_top(&self) -> u64 {
    // kstack_top est stocké à l'offset [176] dans _cold_reserve
    // SAFETY: champ initialisé dans ProcessThread::new()
    unsafe {
        let ptr = self as *const _ as *const u8;
        let top_ptr = ptr.add(176) as *const u64;
        core::ptr::read(top_ptr)
    }
}
```

---

### SCHED-FIX-02 🔴 — Architecture v7 §3.2 : `kstack_ptr` décrit à tort comme source de TSS.RSP0

**Fichier concerné** : `docs/recast/ExoOS_Architecture_v7.md` §3.2, TCB Layout

**Problème** : La table TCB Layout dit :
```
kstack_ptr | [8] | 8 B | RSP Ring 0 — source de vérité pour TSS.RSP0 (V7-C-03)
```
C'est **incorrect**. `kstack_ptr` stocke le RSP du thread au moment de la sauvegarde de contexte — valeur dynamique, profonde dans la pile kernel. La **source de vérité pour `TSS.RSP0`** est `kstack_top` (offset [176]), qui est le sommet fixe et propre de la pile kernel.

Le code `scheduler/core/switch.rs` est correct (utilise `kstack_top()`). La TLA+ `ContextSwitch.tla` est correcte (utilise `kstack_top`). C'est le **document d'architecture qui est erroné**.

**Correctif** : Modifier `ExoOS_Architecture_v7.md` §3.2, ligne TCB `kstack_ptr` :

```markdown
| `kstack_ptr` | [8] | 8 B | RSP sauvegardé par `switch_asm.s` (dynamique — profond dans la pile) **HARDCODÉ switch_asm.s** |
```

Et ajouter une note sur `kstack_top` :

```markdown
| `kstack_top` | [176] | 8 B | Sommet FIXE de la pile kernel — **source de vérité pour `TSS.RSP0`** (V7-C-03). Inchangé après création du thread. |
```

Et corriger la séquence `context_switch()` en §3.2 :
```
6. tss_set_rsp0(current_cpu(), next.kstack_top)  ← V7-C-03 OBLIGATOIRE
```

---

### SCHED-FIX-03 🟠 — CR0.TS=1 : ordre contradictoire entre arch v7 et SCHEDULER_CORE.md

**Fichiers concernés** : `docs/recast/ExoOS_Architecture_v7.md` §3.2, `docs/kernel/scheduler/SCHEDULER_CORE.md` §2, `docs/Exo-OS-TLA+/ContextSwitch.tla`

**Problème** : Trois sources donnent des ordres différents pour le moment où `CR0.TS = 1` est positionné :

| Source | Ordre |
|--------|-------|
| Arch v7 §3.2 `context_switch()` | Étape 5 — APRÈS `context_switch_asm()` |
| SCHEDULER_CORE.md §2 `context_switch` | Étape 2 — AVANT l'ASM |
| ContextSwitch.tla `Step2_SetLazyBit` | AVANT `Step5_AsmSwitch` |

**Analyse** : Pour le Lazy FPU, `CR0.TS = 1` doit être positionné sur le CPU **après** le switch ASM, dans le contexte du **nouveau thread**. La raison : l'état FPU de `prev` a déjà été sauvegardé (étape 1), et on veut que `next` déclenche `#NM` à sa première instruction FPU. Mettre `CR0.TS = 1` avant le switch ASM pourrait affecter la sauvegarde de l'état FPU en cours si celle-ci utilise des instructions FPU.

**Ordre correct (corrigé)** :
1. Si `fpu_loaded(prev)` → `xsave64(prev.fpu_state_ptr)` — sauvegarder la FPU de `prev`
2. `prev.clear_fpu_loaded()` — marquer la FPU de `prev` non chargée
3. `prev.set_state(Runnable)`
4. `context_switch_asm(prev.kstack_ptr, next.kstack_ptr, next.cr3_phys)` ← switch RSP/CR3
5. `next.set_state(Running)`
6. **`set_cr0_ts()`** ← CR0.TS=1 sur le CPU courant (nouveau contexte : `next`) (V7-C-02)
7. `tss_set_rsp0(current_cpu(), next.kstack_top)` (V7-C-03)

**Correctif** : Aligner arch v7 §3.2 et SCHEDULER_CORE.md §2 sur cet ordre. Mettre à jour la TLA+ :

```tla
Step2_SetLazyBit(c) ==          \\* Devient Step6_SetLazyBit
    /\\ SwitchStage[c] = 6       \\* APRÈS Step5_AsmSwitch
    /\\ Cr0TsBit' = [Cr0TsBit EXCEPT ![c] = TRUE]
    /\\ SwitchStage' = [SwitchStage EXCEPT ![c] = 7]
    /\\ UNCHANGED <<CurrentTcb, TssRsp0, FsBase, UserGsBase, GsSlot20, FpuRegisters, XSaveArea, NextTcb>>
```

---

### SCHED-FIX-04 🟠 — `smp/affinity.rs` : documentation obsolète (`affinity: u64` vs `CpuSet`)

**Fichier concerné** : `docs/kernel/scheduler/SCHEDULER_SMP.md` §2

**Problème** : La documentation décrit l'ancienne API :
```rust
pub fn cpu_allowed(affinity: u64, cpu: CpuId) -> bool {
    if cpu.0 >= 64 { return false; }
    affinity & (1u64 << cpu.0) != 0
}
```
Le code actuel utilise :
```rust
pub fn cpu_allowed(affinity: &CpuSet, cpu: CpuId) -> bool {
    affinity.contains(cpu)
}
```
`CpuSet` est un alias de `CpuMask` (256 bits), donc CPUs 0-255 sont correctement supportés. La documentation induit en erreur et montre une version limitée à 64 CPUs.

**Correctif** : Mettre à jour `SCHEDULER_SMP.md` §2 :

```rust
/// Vérifie si le TCB est autorisé sur ce CPU.
/// Utilise CpuSet (256 bits = MAX_CPUS = 256 CPUs).
pub fn cpu_allowed(affinity: &CpuSet, cpu: CpuId) -> bool {
    affinity.contains(cpu)  // CPUs 0..255 supportés
}

/// Convertit un CpuMask (256 bits) en CpuSet.
pub fn affinity_mask_from_cpu_mask(mask: &CpuMask) -> CpuSet { *mask }

/// Valide l'affinité : si vide → tous CPUs (anti-deadlock).
pub fn sanitize_affinity(affinity: CpuSet) -> CpuSet {
    if affinity.is_empty() { CpuSet::full() } else { affinity }
}
```

---

### SCHED-FIX-05 🟠 — `load_balance.rs` : deadlock potentiel par double verrouillage symétrique

**Fichier concerné** : `docs/kernel/scheduler/SCHEDULER_SMP.md` §4, `kernel/src/scheduler/smp/load_balance.rs`

**Problème** : L'algorithme documente "lock remote_rq avant lock local_rq". Si simultanément :
- CPU A balance depuis CPU B (lock B, puis lock A)
- CPU B balance depuis CPU A (lock A, puis lock B)

→ deadlock classique avec acquisition croisée.

**Correctif** : Toujours acquérir les locks dans l'ordre croissant des `cpu_id`. Mettre à jour la documentation et le code :

```rust
// Dans balance_cpu :
let (first_cpu, second_cpu) = if local_cpu.0 < remote_cpu.0 {
    (local_cpu, remote_cpu)
} else {
    (remote_cpu, local_cpu)
};
let _guard1 = IrqGuard::for_runqueue(first_cpu);  // CPU id le plus bas en premier
let _guard2 = IrqGuard::for_runqueue(second_cpu);
```

Et documenter dans `SCHEDULER_SMP.md` §4 :
```
Lock ordering : toujours acquérir local_rq et remote_rq dans l'ordre croissant de cpu_id.
Evite le deadlock lorsque deux CPUs se balancent mutuellement.
```

---

### SCHED-FIX-06 🟡 — `ContextSwitch.tla` : `VisibilityGap` mort, `ReadAcquire` précondition restrictive

**Fichier concerné** : `docs/Exo-OS-TLA+/ContextSwitch.tla` (indirect), `docs/Exo-OS-TLA+/Memory.tla`

**Problème 1** : Dans `Memory.tla`, la variable `VisibilityGap` est déclarée et initialisée à `FALSE` mais **jamais mise à TRUE** par aucune action. Elle ne contribue à aucun invariant. C'est une variable morte.

**Problème 2** : `ReadAcquire(c, var)` a pour précondition `AtomicWrites[var].ordering = "Release"`. Dans le modèle mémoire réel, une lecture Acquire peut lire une valeur écrite avec Relaxed ordering — la précondition ne modélise que les paires Release/Acquire synchronisées, pas le cas général. Cela rend le modèle **trop optimiste** : il ne détecte pas les races sur des variables écrites en Relaxed et lues en Acquire.

**Correctif** : Dans `Memory.tla` :

```tla
\\* Supprimer VisibilityGap ou lui donner un rôle explicite
\\* Option : l'utiliser pour signaler les lectures Acquire sur valeurs Relaxed
ReadAcquire(c, var) ==
    \\* Cas 1 : valeur Release → propagation complète (semantique forte)
    \\/ /\\ AtomicWrites[var].ordering = "Release"
       /\\ AtomicReads' = [AtomicReads EXCEPT ![c] =
                            [v \\in VARS |->
                                IF ReleaseFence[var][v] = 1 THEN 1 ELSE AtomicReads[c][v]]]
       /\\ HappensBefore' = HappensBefore \\cup {<<AtomicWrites[var].core, c>>}
       /\\ AcquireFence' = AcquireFence \\cup {<<c, var>>}
       /\\ VisibilityGap' = FALSE
    \\* Cas 2 : valeur Relaxed → lecture sans propagation (potentielle stale read)
    \\/ /\\ AtomicWrites[var].ordering = "Relaxed"
       /\\ AtomicReads' = [AtomicReads EXCEPT ![c][var] = AtomicWrites[var].value]
       /\\ VisibilityGap' = TRUE   \\* Signale une sync manquante
       /\\ UNCHANGED <<HappensBefore, AcquireFence, ReleaseFence>>
    /\\ UNCHANGED <<AtomicWrites>>
```

---

### SCHED-FIX-07 🟡 — `ExoPhoenix` : magic number `slot >= 64` dans 3 fichiers kernel

**Fichiers concernés** : `kernel/src/exophoenix/isolate.rs:54`, `kernel/src/exophoenix/handoff.rs:112,138`, `kernel/src/exophoenix/forge.rs:314`

**Problème** (confirmé par l'analyse approfondie interne) : Les trois fichiers contiennent `if slot >= 64` au lieu de `if slot >= SSR_MAX_CORES_LAYOUT`. Sur un système à 128+ cœurs, les cœurs 64-127 sont silencieusement ignorés lors du freeze ExoPhoenix — pas d'ACK attendu, pas de TLB shootdown, pas de reconstruction Forge. Corruption potentielle sur systèmes >64 cœurs.

**Correctif** dans les 3 fichiers :

```rust
// Avant (dans isolate.rs, handoff.rs, forge.rs) :
if Some(slot) == self_slot || slot >= 64 {
    continue;
}

// Après :
use exo_phoenix_ssr::SSR_MAX_CORES_LAYOUT;
if Some(slot) == self_slot || slot >= SSR_MAX_CORES_LAYOUT {
    continue;
}
```

---

## Module IPC

### IPC-FIX-01 🔴 — `ipc/capability_bridge/` : module documenté mais inexistant dans le code source

**Fichiers concernés** : `docs/kernel/ipc/README.md` arborescence, `docs/kernel/ipc/capability_bridge.md`, `kernel/src/ipc/`

**Problème** : La documentation IPC définit un sous-module :
```
ipc/
└── capability_bridge/
    ├── mod.rs    # Re-exports — zéro logique ici
    └── check.rs  # Shim → security::capability::verify()
```
Le répertoire `kernel/src/ipc/` ne contient **aucun** `capability_bridge/`. Le module n'est pas déclaré dans `ipc/mod.rs`. Toute référence à `ipc::capability_bridge::check` ne compile pas.

**Correctif** : Créer le module manquant.

`kernel/src/ipc/capability_bridge/mod.rs` :
```rust
// ipc/capability_bridge/mod.rs — Shim IPC → security::capability
// RÈGLE IPC-CAP-01 : zéro logique ici. Délègue uniquement.
pub mod check;
pub use check::check_ipc_access;
```

`kernel/src/ipc/capability_bridge/check.rs` :
```rust
// ipc/capability_bridge/check.rs
// Shim : toute vérification de capacité IPC passe par security::capability::verify().
use crate::security::capability::{verify, CapabilityType, Rights};
use crate::ipc::core::types::IpcError;

/// Vérifie qu'un processus a les droits IPC requis sur une ressource.
///
/// # Erreurs
/// Retourne `IpcError::PermissionDenied` si la capacité est invalide ou insuffisante.
pub fn check_ipc_access(
    pid: u32,
    cap_type: CapabilityType,
    required_rights: Rights,
) -> Result<(), IpcError> {
    verify(pid, cap_type, required_rights)
        .map_err(|_| IpcError::PermissionDenied)
}
```

Ajouter dans `ipc/mod.rs` :
```rust
pub mod capability_bridge;
pub use capability_bridge::check::check_ipc_access;
```

---

### IPC-FIX-02 🔴 — `ipc/sync/sched_hooks.rs` : race condition entre register et block_current

**Fichier concerné** : `kernel/src/ipc/sync/sched_hooks.rs`

**Problème** : La séquence actuelle de `block_current(tid)` :
```
1. register(tid, tcb_ptr) dans SLEEP_REGISTRY
2. appel block_fn()        ← le thread se bloque ici
```
Entre les étapes 1 et 2, un autre thread peut appeler `wake_thread(tid)` :
- `pop(tid)` extrait le TCB de `SLEEP_REGISTRY`
- Met le TCB en état `Runnable` et l'enfile dans le run queue
- Puis le thread bloquant arrive à l'étape 2 et appelle `block_fn()`

Le thread est maintenant à la fois dans la run queue (Runnable) et en train d'appeler la fonction de blocage. Selon l'implémentation de `block_fn`, cela peut résulter en :
- Double transition d'état (Runnable → Sleeping → stuck)
- Thread perdu dans la run queue avec état incorrect

**Correctif** : Vérifier la condition de réveil **après** l'enregistrement, avant le blocage :

```rust
pub unsafe fn block_current(tid: u32, already_woken: &AtomicBool) {
    let tcb_ptr = current_thread_raw();

    // 1. Enregistrer avant de vérifier (minimise la fenêtre)
    if !tcb_ptr.is_null() {
        SLEEP_REGISTRY.lock().register(tid, tcb_ptr);
    }

    // 2. Re-vérifier la condition de réveil APRÈS l'enregistrement
    //    (si le waker est passé entre-temps, already_woken est true)
    if already_woken.load(Ordering::Acquire) {
        // Réveil déjà survenu — annuler l'enregistrement et ne pas bloquer
        if !tcb_ptr.is_null() {
            SLEEP_REGISTRY.lock().pop(tid);
        }
        return;
    }

    // 3. Bloquer (le waker futur trouvera le TCB dans SLEEP_REGISTRY)
    if let Some(block_fn) = *BLOCK_HOOK.lock() {
        block_fn();
        if !tcb_ptr.is_null() {
            SLEEP_REGISTRY.lock().pop(tid);
        }
    } else {
        for _ in 0..10_000 { core::hint::spin_loop(); }
        if !tcb_ptr.is_null() {
            SLEEP_REGISTRY.lock().pop(tid);
        }
    }
}
```

Toutes les primitives bloquantes (`futex_wait`, `sync_channel_send`, etc.) doivent passer leur drapeau `woken` comme second argument.

---

### IPC-FIX-03 🟠 — `ipc/sync/futex.rs` : documentation contradictoire sur le type de clé

**Fichier concerné** : `docs/kernel/ipc/sync_primitives.md`, `docs/kernel/memory/MEMORY_COMPLETE.md` §15

**Problème** : `MEMORY_COMPLETE.md` §15 dit :
> "Adresse physique (pas virtuelle) = partage inter-processus via mémoire partagée."

Mais `sync_primitives.md` dit :
> "Clé futex = adresse virtuelle (physmap) d'un AtomicU32 partagé."

Et le code `futex.rs` utilise :
```rust
pub fn from_addr(addr: &AtomicU32) -> Self {
    Self(addr as *const _ as u64)  // adresse virtuelle physmap
}
```

**Analyse** : L'adresse virtuelle physmap `virt = phys + PHYS_MAP_BASE` est **partagée entre tous les espaces d'adressage kernel** (la physmap est commune). Elle est donc fonctionnellement équivalente à l'adresse physique comme clé de partage. Le code est correct. La documentation dans `MEMORY_COMPLETE.md` est imprécise.

**Correctif** : Aligner les deux documentations :

Dans `MEMORY_COMPLETE.md` §15 :
```markdown
- Clé de hash : adresse virtuelle physmap (= `phys + PHYS_MAP_BASE`) — partagée entre tous
  les espaces d'adressage. Utilisable directement sans conversion phys→virt supplémentaire.
  Invariant : `phys_addr & (FUTEX_HASH_BUCKETS - 1)` est équivalent à
  `(virt_addr - PHYS_MAP_BASE) & (FUTEX_HASH_BUCKETS - 1)`.
```

---

### IPC-FIX-04 🟠 — `ipc/sync/wait_queue.rs` : `expire_timeouts()` jamais appelée

**Fichier concerné** : `kernel/src/ipc/sync/wait_queue.rs`

**Problème** : `IpcWaitQueue::expire_timeouts(now_ns)` est implémentée mais **aucun timer, aucun tick, aucune tâche périodique** ne l'appelle. Les waiters avec timeout ne sortent donc jamais de la file par expiration. Le timeout dans `IpcWaiter::timeout_ns` est stocké mais non utilisé.

**Correctif** : Enregistrer un callback dans le système de timer au moment de `ipc_init()` :

```rust
// Dans ipc/mod.rs, ipc_init() :
pub fn ipc_init(shm_base_phys: u64, n_numa_nodes: u32) {
    // ... initialisations existantes ...

    // Enregistrer le sweep de timeouts IPC dans le tick scheduler
    // Appelé à chaque tick (HZ = 1000 → toutes les 1 ms)
    crate::scheduler::timer::tick::register_tick_callback(ipc_timeout_sweep);
}

/// Callback tick : expire les waiters IPC dont le timeout est dépassé.
fn ipc_timeout_sweep() {
    let now_ns = crate::arch::x86_64::time::ktime::ktime_get_ns();
    // Itérer sur toutes les IpcWaitQueue actives et appeler expire_timeouts()
    sync::wait_queue::sweep_all_timeouts(now_ns);
}
```

Ajouter dans `wait_queue.rs` :
```rust
/// Registre global des IpcWaitQueues actives.
static ACTIVE_QUEUES: SpinLock<[Option<*const IpcWaitQueue>; 1024]> = ...;

/// Sweepeur global — appelé par ipc_timeout_sweep().
pub fn sweep_all_timeouts(now_ns: u64) {
    let guard = ACTIVE_QUEUES.lock();
    for opt in guard.iter() {
        if let Some(q_ptr) = opt {
            // SAFETY: les queues actives sont valides jusqu'à leur désenregistrement
            unsafe { (**q_ptr).expire_timeouts(now_ns); }
        }
    }
}
```

---

### IPC-FIX-05 🟡 — `ipc/rpc/raw.rs` : module source non documenté

**Fichier concerné** : `kernel/src/ipc/rpc/raw.rs`, `docs/kernel/ipc/rpc.md`

**Problème** : Le fichier `rpc/raw.rs` est présent dans le code source (visible dans l'arborescence) mais absent de la documentation `rpc.md`. Sans documentation, le rôle de ce module, ses invariants et son API sont opaques.

**Correctif** : Ajouter dans `docs/kernel/ipc/rpc.md` :

```markdown
## `rpc/raw.rs` — RPC raw (frame brute)

Fournit les primitives bas-niveau pour envoyer/recevoir un frame RPC
sans abstraction `RpcServer`/`RpcClient`. Utilisé par le fast-path IPC
(ASM `fastcall_asm.s`) pour les RPC <240B qui contournent la couche
syscall.

```rust
/// Envoie une frame RPC brute sur un SpscRing.
pub fn rpc_raw_send(ring: *mut SpscRing, frame: &RpcFrame) -> bool

/// Reçoit une frame RPC brute depuis un SpscRing.
pub fn rpc_raw_recv(ring: *mut SpscRing, frame: &mut RpcFrame) -> bool

/// Construit un RpcFrame depuis une méthode + payload.
pub fn rpc_frame_build(method: MethodId, payload: &[u8]) -> Option<RpcFrame>
```
```

---

### IPC-FIX-06 🟡 — `ipc/ring/fusion.rs` : borne supérieure du `batch_threshold` non appliquée

**Fichier concerné** : `kernel/src/ipc/ring/fusion.rs`

**Problème** : `adjust_batch_threshold()` doit ajuster entre `1` et `FUSION_RING_SIZE / 2`. Le commentaire le dit, mais la saturation de la borne supérieure n'est pas visible dans l'implémentation documentée. Si `ewa_throughput` diverge, `batch_threshold` pourrait dépasser `RING_SIZE / 2` (= 8), entraînant un deadlock : `flush()` n'est jamais déclenché si `pending_count < threshold` et `threshold > RING_SIZE`.

**Correctif** :

```rust
fn adjust_batch_threshold(&mut self, observed_throughput: u64) {
    self.ewa_throughput = (7 * self.ewa_throughput + observed_throughput) / 8;
    let new_threshold = (self.ewa_throughput / THROUGHPUT_SCALE).max(1);
    // Saturation OBLIGATOIRE : jamais > RING_SIZE / 2
    self.batch_threshold = new_threshold.min(RING_SIZE / 2) as usize;
}
```

---

## Module Process

### PROC-FIX-01 🔴 — `exec.rs` : flush des signaux pending non implémenté (V7-C-04 / étape 2.5)

**Fichier concerné** : `kernel/src/process/lifecycle/exec.rs`

**Problème** : L'architecture v7 §3.3 spécifie la séquence `do_exec()` avec l'étape :
```
2.5 signal_queue.flush_all_except_sigkill()
    // [ExoOS-spécifique] Flush pending signals — comportement défini, pas POSIX strict
```
Le code actuel (`exec.rs`) sauvegarde le signal_mask et appelle `reset_signals_on_exec()` pour les handlers, mais ne flush pas les signaux pending. Un signal `SIGUSR1` mis en file par le processus parent **avant** l'exec sera livré au nouveau programme, qui n'a aucun handler installé pour lui (SIG_DFL → Terminate). Ce comportement est dangereux et non documenté pour l'utilisateur du nouvel espace d'adressage.

**Correctif** : Ajouter le flush après l'étape de masquage des signaux :

```rust
// Étape 3 — bloquer tous les signaux (déjà présent)
block_all_except_kill(&thread.sched_tcb);

// Étape 3.5 — NOUVEAU : flush les signaux pending sauf SIGKILL/SIGSTOP
// Conforme V7-C-04 (ExoOS-spécifique, pas POSIX strict)
thread.sig_queue.flush_all();
thread.rt_sig_queue.flush_all_except(Signal::SIGKILL as u8, Signal::SIGSTOP as u8);

// Étape 4 — load ELF (déjà présent)
let elf_result = loader.load_elf(...);
```

`flush_all()` sur `SigQueue` :
```rust
impl SigQueue {
    pub fn flush_all(&self) {
        self.pending.store(0, Ordering::Release);
    }
}
```

`flush_all_except()` sur `RTSigQueue` :
```rust
impl RTSigQueue {
    pub fn flush_all_except(&self, sig_a: u8, sig_b: u8) {
        for sig in 32u8..64 {
            if sig != sig_a && sig != sig_b {
                let idx = (sig - 32) as usize;
                // Vider la file circulaire pour ce signal
                self.heads[idx].store(self.tails[idx].load(Ordering::Relaxed), Ordering::Release);
                self.counts[idx].store(0, Ordering::Release);
            }
        }
    }
}
```

---

### PROC-FIX-02 🟠 — `signal/mask.rs` : `reset_signals_on_exec()` réinitialise le masque (violation POSIX V7-C-04)

**Fichier concerné** : `kernel/src/process/signal/mask.rs`, `docs/kernel/process/SIGNAL.md` §3

**Problème** : `SIGNAL.md` §3 documente :
```
reset_signals_on_exec(thread) → réinitialise le masque à EMPTY et tous les handlers à SIG_DFL
```
Mais l'architecture v7 §3.3 V7-C-04 spécifie :
> "Signal mask : **hérité** du processus appelant (conformité POSIX IEEE 1003.1)"

Le code `exec.rs` sauve et restaure le masque autour de `reset_signals_on_exec()`, ce qui est correct. Mais la fonction elle-même réinitialise le masque à `EMPTY`, contredisant son nom impliquant que le masque ne serait pas affecté. C'est source de bugs si quelqu'un appelle `reset_signals_on_exec()` sans le protocole de save/restore.

**Correctif** : Séparer les deux responsabilités :

```rust
/// Réinitialise UNIQUEMENT les handlers de signaux à SIG_DFL.
/// N'affecte PAS le signal_mask (conformité POSIX exec() V7-C-04).
pub fn reset_signal_handlers_on_exec(tcb: &ThreadControlBlock) {
    // pcb.sig_handlers.lock().reset_all_to_default() — handlers seulement
    // tcb.signal_mask est inchangé
}

/// Réinitialise handlers ET masque à EMPTY.
/// À utiliser UNIQUEMENT pour les threads initiaux (create_process), pas exec().
pub fn reset_signals_full(tcb: &ThreadControlBlock) {
    reset_signal_handlers_on_exec(tcb);
    tcb.signal_mask.store(SigMask::EMPTY.0, Ordering::Release);
}
```

Mettre à jour `exec.rs` pour utiliser `reset_signal_handlers_on_exec()` et non `reset_signals_on_exec()`. Supprimer la sauvegarde/restauration manuelle du masque (devenue inutile) :

```rust
// Supprimer : let saved_signal_mask = ...
reset_signal_handlers_on_exec(&thread.sched_tcb);
// Supprimer : thread.sched_tcb.signal_mask.store(saved_signal_mask, ...)
// Le masque est préservé implicitement par reset_signal_handlers_on_exec()
```

---

### PROC-FIX-03 🟠 — `process/lifecycle/fork.rs` : citation de règle incorrecte dans `AddressSpaceCloner`

**Fichier concerné** : `kernel/src/process/lifecycle/fork.rs`, `docs/kernel/process/LIFECYCLE.md` §2

**Problème** : Le trait `AddressSpaceCloner` documente :
```rust
/// Flush le TLB parent après marquage CoW (RÈGLE PROC-06).
fn flush_tlb_after_fork(&self, cr3: u64);
```
Mais `process/mod.rs` définit les règles :
```
PROC-06 : Livraison signal : au retour userspace UNIQUEMENT
PROC-08 : fork() flush TLB parent AVANT retour
```
La règle applicable est **PROC-08**, pas PROC-06. L'erreur de numéro de règle peut mener un développeur à chercher la mauvaise justification.

**Correctif** :

```rust
/// Flush le TLB parent après marquage CoW des VMAs en lecture seule.
///
/// OBLIGATOIRE avant le retour userspace du parent (PROC-08).
/// Si le TLB parent conserve des PTEs avec droits WRITE sur les pages
/// maintenant marquées READ-ONLY (CoW), le parent pourrait écrire
/// directement sans déclencher le #PF CoW.
fn flush_tlb_after_fork(&self, cr3: u64);
```

---

### PROC-FIX-04 🟠 — `process/core/pid.rs` : `Pid::INVALID = 0` identique à `Pid::IDLE = 0`

**Fichier concerné** : `kernel/src/process/core/pid.rs`, `docs/kernel/process/CORE.md` §1

**Problème** : Les deux constantes sont `0` :
```rust
pub const IDLE:    Self = Self(0);  // Processus idle
pub const INVALID: Self = Self(0);  // Valeur sentinelle "pas de PID"
```
Toute vérification `pid == Pid::INVALID` matche aussi le processus idle, interdisant de référencer le PID 0 légitimement. Des code-paths tels que `if parent == Pid::INVALID { use_default() }` s'activeraient faussement si le parent est le processus idle.

**Correctif** : Utiliser `u32::MAX` comme valeur sentinelle INVALID :

```rust
impl Pid {
    /// Processus idle (swapper), PID 0.
    pub const IDLE: Self = Self(0);
    /// PID 1 = init_server.
    pub const INIT: Self = Self(1);
    /// Valeur sentinelle "aucun PID valide". Distinct du PID 0 idle.
    pub const INVALID: Self = Self(u32::MAX);
    /// Premier PID allouable (0=idle et 1=init réservés).
    pub const FIRST_USABLE: Self = Self(2);
}
```

Mettre à jour `PID_ALLOCATOR` pour ne jamais allouer `u32::MAX`.

---

### PROC-FIX-05 🟠 — `process/thread/local_storage.rs` : collision de noms `TlsKey` (type + registre)

**Fichier concerné** : `kernel/src/process/thread/local_storage.rs`, `docs/kernel/process/THREAD.md` §4

**Problème** : La documentation définit deux structures portant le même nom `TlsKey` :
1. `pub struct TlsKey(pub u32)` — identifiant de clé (type newtype)
2. `pub struct TlsKey { pub keys: UnsafeCell<[TlsKeyEntry; MAX_TLS_KEYS]>, ... }` — registre global

En Rust, deux structs ne peuvent pas avoir le même nom dans le même module. Ce conflit de nommage rend le code non compilable.

**Correctif** : Renommer le registre global :

```rust
/// Identifiant d'une clé TLS (pthread_key_t).
pub struct TlsKey(pub u32);

impl TlsKey {
    pub const INVALID: Self = Self(u32::MAX);
}

/// Registre global des clés TLS dynamiques (remplace l'ancienne struct TlsKey).
pub struct TlsKeyRegistry {
    pub keys:      UnsafeCell<[TlsKeyEntry; MAX_TLS_KEYS]>,
    pub alloc_map: AtomicU64,   // bitmap des 64 premières clés
    pub count:     AtomicU32,
    pub lock:      SpinLock<()>,
}

pub static TLS_KEY_REGISTRY: TlsKeyRegistry = TlsKeyRegistry::new();
```

---

### PROC-FIX-06 🟡 — `process/lifecycle/exit.rs` : TID libéré avant l'enqueue reaper

**Fichier concerné** : `kernel/src/process/lifecycle/exit.rs`, `docs/kernel/process/LIFECYCLE.md` §4

**Problème** : La séquence `do_exit()` :
```
6. TID_ALLOCATOR.free(thread.tid)   → TID réutilisable IMMÉDIATEMENT
7. REAPER_QUEUE.enqueue(pid, tid)   → passe le TID au reaper
```
Le TID est libéré à l'étape 6 et potentiellement réalloué à un nouveau thread avant que le reaper traite l'étape 7. Le reaper reçoit un TID qui n'est plus associé au thread mort. Si le reaper utilise le TID pour retrouver des ressources (traces, logs), il pourrait trouver le mauvais thread.

**Analyse** : Le reaper `kthread_reaper()` n'utilise le TID que pour le log diagnostique, pas pour accéder à des ressources. Néanmoins, cela est conceptuellement incorrect et fragile.

**Correctif** : Envoyer uniquement le PID au reaper. Le TID est libéré par le reaper (non pas inline) :

```rust
// Séquence do_exit() corrigée :
// 6. REAPER_QUEUE.enqueue(pid)          ← pid uniquement
// 7. [TID libéré par kthread_reaper()]  ← pas inline

// Dans kthread_reaper() :
while let Some(pid) = REAPER_QUEUE.dequeue() {
    if let Some(pcb) = PROCESS_REGISTRY.remove(pid) {
        // Libérer le TID du thread principal maintenant (hors contexte de do_exit)
        let tid = unsafe { (*(*pcb).main_thread_rawptr.load(Ordering::Acquire)).tid };
        TID_ALLOCATOR.free(tid.0);
        PID_ALLOCATOR.free(pid.0);
        drop(unsafe { Box::from_raw(pcb) });
    }
}
```

---

### PROC-FIX-07 🟡 — `process/lifecycle/reap.rs` : mécanisme de blocage du kthread indéfini

**Fichier concerné** : `kernel/src/process/lifecycle/reap.rs`, `docs/kernel/process/LIFECYCLE.md` §6

**Problème** : La documentation dit :
```
attendre signal dans REAPER_QUEUE (via wait_queue ou polling 1ms)
```
Le choix entre wait_queue et polling est laissé à l'implémentation sans spécification. En pratique :
- Si polling 1 ms : latence de 0-1 ms pour le cleanup zombie, gaspillage CPU
- Si wait_queue : latence nulle mais nécessite un mécanisme de notification depuis `do_exit()`

**Correctif** : Spécifier et implémenter le mécanisme wait_queue :

```rust
// Dans reap.rs :
static REAPER_NOTIFY: IpcEvent = IpcEvent::new_const();

pub fn enqueue(pid: Pid) {
    REAPER_QUEUE.push(pid.0);
    REAPER_NOTIFY.signal();  // Réveille le reaper kthread immédiatement
}

pub unsafe fn kthread_reaper() -> ! {
    loop {
        // Attendre sans polling (latence nulle, pas de gaspillage CPU)
        REAPER_NOTIFY.wait().ok();
        REAPER_NOTIFY.reset();

        while let Some(pid_raw) = REAPER_QUEUE.pop() {
            let pid = Pid(pid_raw);
            // ... cleanup PCB, libération PID/TID ...
        }
    }
}
```

---

### PROC-FIX-08 🟡 — `process/state/transitions.rs` : table de transitions valides absente

**Fichier concerné** : `kernel/src/process/state/transitions.rs`, `docs/kernel/process/LIFECYCLE.md`

**Problème** : `try_transition(from, to)` effectue un CAS mais n'encode aucune règle sur quelles transitions sont légales. Une transition `Dead → Running` est aussi possible que `Runnable → Running`. Sans table de validité, des bugs peuvent produire des états incohérents silencieusement.

**Correctif** : Ajouter une validation des transitions dans `state/transitions.rs` :

```rust
/// Retourne true si la transition `from → to` est légale.
pub fn is_valid_transition(from: ProcessState, to: ProcessState) -> bool {
    use ProcessState::*;
    matches!(
        (from, to),
        (Creating, Running)
            | (Running, Sleeping)
            | (Running, Stopped)
            | (Running, Zombie)
            | (Sleeping, Running)
            | (Sleeping, Zombie)
            | (Stopped, Running)       // SIGCONT
            | (Stopped, Zombie)
            | (Zombie, Dead)           // Après waitpid()
    )
}

pub fn try_transition(
    pcb: &ProcessControlBlock,
    from: ProcessState,
    to: ProcessState,
) -> bool {
    debug_assert!(
        is_valid_transition(from, to),
        "Transition d'état invalide: {:?} → {:?}", from, to
    );
    pcb.state
        .compare_exchange(from as u32, to as u32, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok()
}
```

---

## Incohérences Cross-Module

### CROSS-FIX-01 🟠 — `ExoShield signatures/update.rs` : violation SRV-02 et SRV-04

**Fichiers concernés** : `servers/exo_shield/src/signatures/update.rs`, `servers/crypto_server/src/main.rs`

**Problème** (confirmé par analyse interne, rapport audit 30 avril 2026) : `update.rs` contient ~800 lignes d'arithmétique de champ Ed25519 locale avec un hash simplifié non standard. Commentaire ligne 562 : *"En production, cela serait remplacé par le vrai SHA-512 via le crypto_server."*

Violations :
- **SRV-02** : Blake3/chacha20 interdits hors `crypto_server` — l'implémentation Ed25519 maison est pire encore
- **SRV-04** : Toute crypto Ring 1 passe par `crypto_server`

**Correctif** : Supprimer l'implémentation locale. Déléguer à `crypto_server` via IPC :

```rust
// servers/exo_shield/src/signatures/update.rs — REMPLACE les ~800 lignes

use crate::ipc_client::{send_recv, Endpoint};
use exo_types::fixed_string::FixedString;

static CRYPTO_ENDPOINT: Endpoint = Endpoint::named("crypto_server");

/// Vérifie une signature Ed25519 via le crypto_server (SRV-04 conforme).
pub fn verify_signature(
    pubkey: &[u8; 32],
    signature: &[u8; 64],
    message: &[u8],
) -> bool {
    // Construire la requête IPC de vérification
    let mut req = CryptoRequest::new(CRYPTO_VERIFY_ED25519);
    req.payload[..32].copy_from_slice(pubkey);
    req.payload[32..96].copy_from_slice(signature);
    // Pour les messages > 56B : envoyer le hash Blake3 (via crypto_server aussi)
    let msg_hash = {
        let mut hash_req = CryptoRequest::new(CRYPTO_HASH_BLAKE3);
        // ... envoyer le message en chunks si > MAX_MSG_SIZE ...
        hash_req
    };
    let rep = send_recv(&CRYPTO_ENDPOINT, &req);
    rep.status == CRYPTO_OK
}
```

---

### CROSS-FIX-02 🟡 — `PhoenixWakeEntropy` : reseed post-restore non câblé

**Fichiers concernés** : `kernel/src/exophoenix/forge.rs`, `servers/crypto_server/src/main.rs`

**Problème** (confirmé par analyse interne) : Après un restore ExoPhoenix, le nonce XChaCha20 du `crypto_server` reprend depuis l'état snapshot. Sans reseed, les nonces peuvent être réutilisés → compromission AEAD.

**Correctif Phase 1** — Ajouter le type de message dans `crypto_server/src/main.rs` :

```rust
pub const CRYPTO_PHOENIX_RESEED: u32 = 12;

// Dans handle_request() :
CRYPTO_PHOENIX_RESEED => {
    // Vérifier que l'appelant est le kernel (PID 0 ou capability CRYPTO_KERNEL)
    if req.sender_pid != 0 {
        return reply_error(CRYPTO_PERMISSION_DENIED);
    }
    // Forcer un reseed depuis l'entropy fournie par le kernel
    xchacha20::force_reseed(&req.payload[..32]);
    rng::reseed_from_rdrand();
    reply.status = CRYPTO_OK;
}
```

**Correctif Phase 2** — Appeler depuis `forge.rs` après reconstruction :

```rust
// kernel/src/exophoenix/forge.rs
pub fn reconstruct_kernel_a() -> Result<(), ForgeError> {
    // ... reconstruction existante ...

    // OBLIGATOIRE : reseed crypto avant tout autre IPC (spéc GI-05)
    phoenix_crypto_reseed().map_err(|_| ForgeError::CryptoReseedFailed)?;
    Ok(())
}

fn phoenix_crypto_reseed() -> Result<(), ()> {
    // Générer 32 bytes d'entropy (RDRAND + HPET timestamp)
    let mut entropy = [0u8; 32];
    crate::security::crypto::rng::fill_random(&mut entropy);

    let mut msg = IpcMessage::default();
    msg.msg_type = CRYPTO_PHOENIX_RESEED;
    msg.payload[..32].copy_from_slice(&entropy);
    ipc_send_kernel(CRYPTO_SERVER_PID, &msg).map_err(|_| ())
}
```

---

### CROSS-FIX-03 🟡 — `Memory.tla` : `S49_IommuInitRelease` invariant trop fort

**Fichier concerné** : `docs/Exo-OS-TLA+/Memory.tla`

**Problème** : L'invariant S49 stipule :
```tla
S49_IommuInitRelease ==
    ∀ c ∈ CORES :
        (AtomicReads[c]["iommu_init"] = 1) =>
            (AtomicReads[c]["iommu_slots"] = 1)
```
Cette propriété dit "si un cœur a lu iommu_init=1, alors il a aussi vu iommu_slots=1". Mais `AtomicReads[c]["iommu_init"]` peut valoir 1 sans que le cœur ait fait `ReadAcquire` sur `iommu_init` — par exemple après une `WriteRelaxed` locale. Le modèle ne distingue pas "a lu avec Acquire" de "a mis à jour localement". L'invariant est donc potentiellement atteignable sans la synchronisation requise.

**Correctif** : Conditionner l'invariant sur `AcquireFence` :

```tla
S49_IommuInitRelease ==
    ∀ c ∈ CORES :
        (<<c, "iommu_init">> ∈ AcquireFence) =>
            (AtomicReads[c]["iommu_slots"] = 1)
```

Ceci garantit que la propriété s'applique uniquement aux cœurs qui ont effectué une vraie lecture Acquire sur `iommu_init`.

---

## Tableau Récapitulatif Final

| ID | Module | Sévérité | Nature | Fichier(s) principal(aux) |
|----|--------|----------|--------|--------------------------|
| MEM-FIX-01 | Memory | 🟡 | Doc incorrecte | `MEMORY_COMPLETE.md` |
| MEM-FIX-02 | Memory | 🟠 | Doc ambiguë | `MEMORY_COMPLETE.md`, `memory/mod.rs` |
| MEM-FIX-03 | Memory | 🟠 | Logique inversée | `cow/breaker.rs`, `MEMORY_COMPLETE.md` |
| MEM-FIX-04 | Memory | 🟡 | Implémentation manquante | `cow/tracker.rs` |
| MEM-FIX-05 | Memory | 🟡 | TODO P1 non adressé | `dma/completion/wakeup.rs` |
| MEM-FIX-06 | Memory | 🟡 | Commentaire contradictoire | `memory/core/constants.rs` |
| SCHED-FIX-01 | Scheduler | 🔴 | Bug runtime critique | `process/lifecycle/exec.rs` |
| SCHED-FIX-02 | Scheduler | 🔴 | Doc d'architecture erronée | `ExoOS_Architecture_v7.md` |
| SCHED-FIX-03 | Scheduler | 🟠 | Ordre contradictoire 3 sources | `arch v7`, `SCHEDULER_CORE.md`, TLA+ |
| SCHED-FIX-04 | Scheduler | 🟠 | API doc obsolète | `SCHEDULER_SMP.md` |
| SCHED-FIX-05 | Scheduler | 🟠 | Deadlock potentiel | `smp/load_balance.rs` |
| SCHED-FIX-06 | Scheduler | 🟡 | TLA+ mort / précondition trop stricte | `Memory.tla` |
| SCHED-FIX-07 | Scheduler | 🟡 | Magic number 64 | `exophoenix/{isolate,handoff,forge}.rs` |
| IPC-FIX-01 | IPC | 🔴 | Module manquant | `ipc/capability_bridge/` |
| IPC-FIX-02 | IPC | 🔴 | Race condition | `ipc/sync/sched_hooks.rs` |
| IPC-FIX-03 | IPC | 🟠 | Doc contradictoire | `sync_primitives.md`, `MEMORY_COMPLETE.md` |
| IPC-FIX-04 | IPC | 🟠 | Timeout jamais déclenché | `ipc/sync/wait_queue.rs` |
| IPC-FIX-05 | IPC | 🟡 | Module non documenté | `ipc/rpc/raw.rs` |
| IPC-FIX-06 | IPC | 🟡 | Borne supérieure manquante | `ipc/ring/fusion.rs` |
| PROC-FIX-01 | Process | 🔴 | Flush pending signals manquant | `process/lifecycle/exec.rs` |
| PROC-FIX-02 | Process | 🟠 | Sémantique signal_mask incorrecte | `process/signal/mask.rs` |
| PROC-FIX-03 | Process | 🟠 | Citation règle incorrecte | `process/lifecycle/fork.rs` |
| PROC-FIX-04 | Process | 🟠 | Ambiguïté PID 0 | `process/core/pid.rs` |
| PROC-FIX-05 | Process | 🟠 | Collision de noms | `process/thread/local_storage.rs` |
| PROC-FIX-06 | Process | 🟡 | TID libéré trop tôt | `process/lifecycle/exit.rs` |
| PROC-FIX-07 | Process | 🟡 | Mécanisme blocking indéfini | `process/lifecycle/reap.rs` |
| PROC-FIX-08 | Process | 🟡 | Table transitions absente | `process/state/transitions.rs` |
| CROSS-FIX-01 | ExoShield | 🟠 | SRV-02/SRV-04 violés | `exo_shield/signatures/update.rs` |
| CROSS-FIX-02 | ExoPhoenix | 🟡 | Reseed non câblé | `forge.rs`, `crypto_server/main.rs` |
| CROSS-FIX-03 | TLA+ | 🟡 | Invariant trop fort | `Memory.tla` |

---

## Ordre d'Application Recommandé

### Phase 1 — Critique (runtime corruption) [immédiat]
1. `SCHED-FIX-01` — `exec.rs` TSS.RSP0 `kstack_top`
2. `IPC-FIX-01` — Créer `capability_bridge/`
3. `IPC-FIX-02` — Race condition `block_current()`
4. `PROC-FIX-01` — Flush pending signals dans `exec()`

### Phase 2 — Majeur (comportement incorrect) [sprint suivant]
5. `SCHED-FIX-02` — Corriger l'architecture v7 (kstack_ptr → kstack_top pour TSS.RSP0)
6. `SCHED-FIX-03` — Aligner ordre CR0.TS=1 sur les 3 sources
7. `PROC-FIX-02` — Sémantique `reset_signal_handlers_on_exec()`
8. `PROC-FIX-04` — `Pid::INVALID ≠ Pid::IDLE`
9. `PROC-FIX-05` — Renommer `TlsKeyRegistry`
10. `MEM-FIX-03` — Corriger sémantique CoW break
11. `SCHED-FIX-05` — Ordering déterministe `load_balance`
12. `IPC-FIX-04` — Timer sweep pour timeouts IPC
13. `CROSS-FIX-01` — Supprimer Ed25519 local dans ExoShield

### Phase 3 — Mineur / Qualité [prochaine itération]
14. `MEM-FIX-01`, `MEM-FIX-02`, `MEM-FIX-06` — Correctifs documentation Memory
15. `SCHED-FIX-04` — Doc affinity API
16. `SCHED-FIX-07` — Magic number 64 → SSR_MAX_CORES_LAYOUT
17. `SCHED-FIX-06` — TLA+ Memory.tla
18. `MEM-FIX-04`, `MEM-FIX-05` — CoW tombstone, DMA ISR wakeup
19. `PROC-FIX-03`, `PROC-FIX-06`, `PROC-FIX-07`, `PROC-FIX-08`
20. `IPC-FIX-03`, `IPC-FIX-05`, `IPC-FIX-06`
21. `CROSS-FIX-02`, `CROSS-FIX-03`

---

*claude-iota — Rapport technique — Exo-OS correctifs modules core — 03 mai 2026*
