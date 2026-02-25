# Règles de conformité IPC

Ce document liste les **8 règles obligatoires** du module `ipc/`, issues de `docs/refonte/DOC4_TO_DOC9_MODULES_FIXED.md` (DOC 5 — MODULE IPC/).

Toute modification du module IPC doit satisfaire **toutes** ces règles. La violation d'une règle constitue un défaut de conformité bloquant.

---

## IPC-01 — Anti-faux-partage sur les rings SPSC

**Règle** : Les champs `head` et `tail` d'un ring SPSC doivent être alignés sur des lignes de cache séparées (64 octets) via `CachePad`.

**Rationale** : Sans cet alignement, `head` et `tail` partagent la même ligne de cache (false sharing). Chaque accès par un producteur invalide la cache du consommateur et vice-versa, détruisant les performances sur SMP.

**Implémentation** :
```rust
// ring/spsc.rs
#[repr(align(64))]
struct CachePad(AtomicU64, [u8; 56]);  // 64 octets exacts

pub struct SpscRing {
    head: CachePad,  // ligne de cache producteur
    tail: CachePad,  // ligne de cache consommateur (séparée)
    cells: UnsafeCell<[SlotCell; RING_SIZE]>,
}
```

**Fichier** : `kernel/src/ipc/ring/spsc.rs`

---

## IPC-02 — Futex IPC = shim pur vers `memory::utils::futex_table`

**Règle** : `ipc/sync/futex.rs` ne doit contenir **aucune** logique futex locale. Il est un shim de délégation vers `memory::utils::futex_table`.

**Rationale** : La table futex est partagée entre le kernel, les threads et l'IPC. Dupliquer l'implémentation créerait deux tables désynchronisées et des deadlocks impossibles à diagnostiquer.

**Ce qui est interdit** :
- Déclarer une `IpcFutexTable`, `FutexBucket` ou `FutexWaiter` locale
- Implémenter un mécanisme de wait/wake propre à `ipc/`
- Dupliquer la logique de spin-wait sans déléguer à `memory`

**Implémentation correcte** :
```rust
// sync/futex.rs — shim pur
use crate::memory::utils::futex_table::{
    mem_futex_wait, mem_futex_wake, mem_futex_wake_n,
    mem_futex_cancel, mem_futex_requeue, FUTEX_STATS,
};

pub unsafe fn futex_wait(addr, key, expected, thread_id, spin_max, wake_fn)
    -> Result<WaiterState, IpcError>
{
    // Alloue FutexWaiter sur la pile, délègue à memory
    let waiter = FutexWaiter { ... };
    mem_futex_wait(key.0, expected, thread_id, spin_max, wake_fn, &waiter)?;
    // Spin-poll sur waiter.woken (AtomicBool géré par memory)
    ...
}
```

**Fichier** : `kernel/src/ipc/sync/futex.rs`

---

## IPC-03 — Pages SHM avec `NO_COW + PINNED` obligatoires

**Règle** : Toutes les pages de mémoire partagée allouées par `ipc/shared_memory/` doivent avoir les flags `NO_COW` et `PINNED` positionnés.

**Rationale** : 
- `NO_COW` : interdit la copie sur écriture — les deux processus doivent voir exactement les mêmes données physiques.
- `PINNED` : interdit la migration ou l'éviction de la page — une page IPC ne peut pas être swappée pendant une communication active.

**Implémentation** :
```rust
// shared_memory/page.rs
pub struct PageFlags(u32);
impl PageFlags {
    pub const NO_COW: Self = Self(1 << 3);
    pub const PINNED: Self = Self(1 << 4);
    pub const SHM_DEFAULT: Self = Self(
        Self::READ.0 | Self::WRITE.0 | Self::NO_COW.0 | Self::PINNED.0 | Self::SHARED.0
    );
}
// Toute ShmPage allouée utilise SHM_DEFAULT ou les deux flags explicitement.
```

**Fichier** : `kernel/src/ipc/shared_memory/page.rs`

---

## IPC-04 — `capability_bridge/` = shim pur vers `security/capability/`

**Règle** : `ipc/capability_bridge/` ne doit contenir **aucune** logique de vérification de droits. Il délègue exclusivement à `security::capability::verify()`.

**Rationale** : La politique de sécurité est centralisée dans `security/capability/`. Toute logique dans `ipc/` créerait une surface d'attaque indépendante et des incohérences de politique.

**Ce qui est interdit** :
- Implémenter une vérification de droits propre à `ipc/`
- Maintenir une liste d'autorisations locale
- Court-circuiter `security::capability` pour les chemins rapides

**Implémentation correcte** :
```rust
// capability_bridge/check.rs
pub fn verify_ipc_access(token: &CapToken, required: Rights) -> Result<(), IpcCapError> {
    // Délègue ENTIÈREMENT à security
    crate::security::capability::verify(token.object_id, token.rights as u32)
        .map_err(|_| IpcCapError::InsufficientRights)
}
```

**Fichier** : `kernel/src/ipc/capability_bridge/check.rs`

---

## IPC-05 — Interdiction d'importer `fs/`

**Règle** : Le module `ipc/` ne doit **jamais** importer `crate::fs` ou tout sous-module du VFS.

**Rationale** : `ipc/` est en Couche 2a, `fs/` est en Couche 3+. Une dépendance inverse crée un cycle d'initialisation et viole le DAG des couches.

**Vérification** :
```bash
# Ne doit rien retourner
grep -r "crate::fs" kernel/src/ipc/
grep -r "use crate::fs" kernel/src/ipc/
```

**Note** : L'IPC peut fournir des primitives sur lesquelles `fs/` s'appuie (canaux pour les drivers de stockage), mais jamais l'inverse.

---

## IPC-06 — FusionRing : anti-thundering herd via batching adaptatif EWA

**Règle** : `ring/fusion.rs` doit implémenter un mécanisme de batching adaptatif basé sur une **Moyenne Mobile Exponentielle (EWA)** du débit, empêchant le thundering herd lors de pics de charge.

**Rationale** : Sans batching adaptatif, N producteurs envoient N réveils pour N messages, surchargeant le scheduler. Le FusionRing accumule les messages et flush par lot, limitant les réveils au minimum nécessaire.

**Paramètres** :
```rust
pub const FUSION_BATCH_THRESHOLD: usize = 4;  // seuil initial

// Ajustement EWA dans adjust_batch_threshold() :
// ewa_throughput = 0.875 * ewa_throughput + 0.125 * current_throughput
// threshold ∈ [1, FUSION_RING_SIZE / 2]
```

**Déclenchement du flush** :
```
send() flush si :
  - messages accumulés >= threshold, OU
  - ring > 75% plein
```

**Fichier** : `kernel/src/ipc/ring/fusion.rs`

---

## IPC-07 — Fast IPC via fichier ASM (`.s`), jamais `.rs`

**Règle** : Le chemin fast IPC (évitant l'overhead syscall) doit être implémenté dans `core/fastcall_asm.s` (assembleur), **jamais** dans un fichier `.rs`.

**Rationale** : Le fast IPC requiert un contrôle précis des registres (passage ABI), de l'alignement de pile, et potentiellement des instructions spécialisées (`sysenter`, `syscall`, passages de registres directs). Rust ne garantit pas ces contraintes sans `asm!` verbeux — l'ASM pur est plus auditable.

**Contrainte de build** :
```rust
// core/mod.rs ou build.rs
// Le fichier fastcall_asm.s doit être inclus via cc ou global_asm!()
global_asm!(include_str!("fastcall_asm.s"));
```

**Fichier** : `kernel/src/ipc/core/fastcall_asm.s`

---

## IPC-08 — `array_index_nospec()` sur tous les accès ring

**Règle** : Tous les accès indexés dans les ring buffers (`ring/spsc.rs`, `ring/mpmc.rs`, `ring/slot.rs`, `ring/zerocopy.rs`) doivent utiliser `array_index_nospec()` pour se prémunir contre Spectre v1 (Branch Target Injection via bounds check bypass).

**Rationale** : Sans cette mitigation, un attaquant peut forger un index out-of-bounds qui sera spéculativement exécuté par le CPU avant que le bounds check soit évalué, permettant une lecture de mémoire arbitraire via le cache side-channel.

**Implémentation** :
```rust
// core/types.rs
/// Mitigation Spectre v1 — arithmétique d'index sûre pour les ring buffers.
/// Technique : arithmetic right-shift du masque de dépassement.
/// Si index >= size → retourne 0 (pas d'accès hors-borne spéculatif).
#[inline(always)]
pub fn array_index_nospec(index: usize, size: usize) -> usize {
    let mask = (size.wrapping_sub(index).wrapping_sub(1) as isize >> (isize::BITS - 1)) as usize;
    index & mask
}
```

**Utilisation dans ring/spsc.rs** :
```rust
#[inline(always)]
fn cell_at(&self, idx: usize) -> *const SlotCell {
    let safe_idx = array_index_nospec(idx, RING_SIZE);
    unsafe { &(*self.cells.get())[safe_idx] }
}
```

**Fichiers concernés** :
- `kernel/src/ipc/ring/spsc.rs` — `cell_at()`, `ring_for()`
- `kernel/src/ipc/ring/mpmc.rs` — `cell_at()`
- `kernel/src/ipc/ring/slot.rs` — `cell_at()`
- `kernel/src/ipc/ring/zerocopy.rs` — `slot_at()`, push, pop

---

## Tableau de conformité

| Règle | Description | Statut | Fichier |
|---|---|---|---|
| IPC-01 | CachePad sur head/tail SPSC | ✅ Conforme | `ring/spsc.rs` |
| IPC-02 | futex = shim pur → memory | ✅ Conforme | `sync/futex.rs` |
| IPC-03 | SHM pages NO_COW + PINNED | ✅ Conforme | `shared_memory/page.rs` |
| IPC-04 | capability_bridge = shim | ✅ Conforme | `capability_bridge/check.rs` |
| IPC-05 | Pas d'import fs/ | ✅ Conforme | tout `ipc/` |
| IPC-06 | FusionRing EWA anti-thundering | ✅ Conforme | `ring/fusion.rs` |
| IPC-07 | Fast IPC = fichier .s | ✅ Conforme | `core/fastcall_asm.s` |
| IPC-08 | array_index_nospec() | ✅ Conforme | `ring/*.rs` |
