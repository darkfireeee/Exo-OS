# Correctif ALPHA-03 Raffiné — RSP0 : `kstack_top` dans `_cold_reserve`
## ExoOS — kernel/src/scheduler/core/task.rs + switch.rs

**Auteur** : claude-alpha  
**Date** : 2026-05-03

---

## Analyse de l'espace disponible dans `_cold_reserve`

Le TCB actuel utilise `_cold_reserve: [u8; 88]` à partir de l'offset TCB **144**.

| Offset TCB | Taille | Usage actuel |
|---|---|---|
| 144..152 | 8 B | `shadow_stack_token` (ExoShield) |
| 152      | 1 B | `cet_flags` (ExoShield) |
| 153      | 1 B | `threat_score_u8` (ExoShield) |
| 154..160 | 6 B | **libre** |
| 160..168 | 8 B | `pt_buffer_phys` (ExoShield) |
| 168..176 | 8 B | `creation_tsc` (ExoShield) |
| **176..184** | **8 B** | **libre → `kstack_top`** |
| 184..192 | 8 B | libre |
| 192..200 | 8 B | `pl0_ssp` (FIX-CET-01) |
| 200..208 | 8 B | `affinity_hi[0]` (CPUs 64..127) |
| 208..216 | 8 B | `affinity_hi[1]` (CPUs 128..191) |
| 216..224 | 8 B | `affinity_hi[2]` (CPUs 192..255) |
| 224..232 | 8 B | libre |

**→ `kstack_top` occupe TCB[176..184] = `_cold_reserve[32..40]`**  
**Aucun offset existant n'est modifié. Taille TCB = 256 B inchangée.**

---

## Patch 1 — task.rs : Documentation du layout

```rust
// Dans le commentaire de layout au début du fichier, remplacer :
//       [176..200] réservé

// PAR (CORR-ALPHA-03) :
//       [176..184] kstack_top : u64  sommet initial pile kernel — INVARIANT RSP0
//       [184..192] réservé
```

---

## Patch 2 — task.rs : Accesseurs `kstack_top`

Ajouter dans `impl ThreadControlBlock`, après `pl0_ssp()` :

```rust
// ─── kstack_top — sommet initial de la pile kernel (CORR-ALPHA-03) ───────────

/// Offset de `kstack_top` dans `_cold_reserve` (relatif au champ).
/// Correspond à l'offset TCB absolu 176 (= 144 + 32).
const KSTACK_TOP_COLD_OFFSET: usize = 32;

/// Retourne le sommet initial de la pile kernel de ce thread.
///
/// INVARIANT : jamais modifié après `init_kstack_top()`.
/// Utilisé pour mettre à jour TSS.RSP0 après chaque context switch.
///
/// # Relation avec kstack_ptr
/// - `kstack_top`  : adresse haute fixe (sommet de la pile allouée)
/// - `kstack_ptr`  : RSP sauvegardé lors du dernier `context_switch_asm()`
///                   = kstack_top - N×8 (N registres callee-saved empilés)
///
/// RSP0 dans le TSS doit toujours être `kstack_top` pour qu'une IRQ Ring3→0
/// empile son frame au sommet de la pile kernel, loin de toute donnée sauvegardée.
#[inline(always)]
pub fn kstack_top(&self) -> u64 {
    // SAFETY: _cold_reserve[32..40] est réservé à kstack_top (CORR-ALPHA-03).
    // Accès en lecture via pointeur brut aligné 8 octets.
    unsafe {
        core::ptr::read_unaligned(
            self._cold_reserve
                .as_ptr()
                .add(Self::KSTACK_TOP_COLD_OFFSET)
                as *const u64,
        )
    }
}

/// Initialise le sommet de la pile kernel.
///
/// DOIT être appelé UNE SEULE FOIS lors de la création du thread.
/// Après cet appel, `kstack_top` est immuable pour la durée de vie du thread.
///
/// # Safety
/// `stack_top` doit pointer vers le sommet d'une pile kernel valide
/// (adresse haute, aucun alignement supplémentaire requis).
#[inline(always)]
pub fn init_kstack_top(&mut self, stack_top: u64) {
    // SAFETY: _cold_reserve[32..40] est réservé à kstack_top.
    // Écriture lors de la création — pas de concurrent à ce stade.
    unsafe {
        core::ptr::write_unaligned(
            self._cold_reserve
                .as_mut_ptr()
                .add(Self::KSTACK_TOP_COLD_OFFSET)
                as *mut u64,
            stack_top,
        );
    }
}
```

---

## Patch 3 — task.rs : Assertion compile-time

```rust
// Ajouter après les assertions existantes :
const _: () = assert!(
    core::mem::offset_of!(ThreadControlBlock, _cold_reserve) + 32 == 176,
    "TCB CORR-ALPHA-03: kstack_top doit être à l'offset absolu 176"
);
const _: () = assert!(
    core::mem::offset_of!(ThreadControlBlock, _cold_reserve) + 40 <= 232,
    "TCB CORR-ALPHA-03: kstack_top ne doit pas chevaucher fpu_state_ptr (offset 232)"
);
```

---

## Patch 4 — task.rs : Initialisation dans `new()`

```rust
// Dans ThreadControlBlock::new(), après :
kstack_ptr: kernel_stack_top,

// Ajouter l'initialisation de kstack_top :

// ... construction du TCB ...
let mut tcb = Self { /* champs */ };
// CORR-ALPHA-03 : initialiser kstack_top = sommet fixe de la pile kernel.
tcb.init_kstack_top(kernel_stack_top);
tcb
```

Ou bien directement dans la liste d'initialisation, via `_cold_reserve` pré-rempli :

```rust
// Dans ThreadControlBlock::new(), construire cold_reserve avec kstack_top inclus :
let mut cold_reserve = [0u8; 88];

// ExoShield : creation_tsc à _cold_reserve[24..32]
let creation_tsc = crate::arch::x86_64::cpu::tsc::read_tsc();
cold_reserve[24..32].copy_from_slice(&creation_tsc.to_le_bytes());

// CORR-ALPHA-03 : kstack_top à _cold_reserve[32..40]
cold_reserve[32..40].copy_from_slice(&kernel_stack_top.to_le_bytes());

// ... puis utiliser cold_reserve dans la struct init
```

---

## Patch 5 — switch.rs : Utiliser `kstack_top()` pour RSP0

```rust
// Dans context_switch(), remplacer :
unsafe {
    tss::update_rsp0(next.current_cpu().0 as usize, next.kstack_ptr);
    percpu::set_kernel_rsp(next.kstack_ptr);
}

// PAR (CORR-ALPHA-03) :
unsafe {
    // CORR-ALPHA-03 : RSP0 doit pointer vers le SOMMET INITIAL de la pile kernel
    // du thread entrant, pas vers kstack_ptr (RSP sauvegardé = milieu de pile).
    //
    // Quand `next` s'exécute en Ring3 et qu'une IRQ survient :
    //   → CPU charge RSP depuis TSS.RSP0
    //   → Le handler IRQ empile son frame (40B) sous RSP0
    //
    // Si RSP0 = kstack_ptr (= kstack_top - 48) après le premier switch,
    // le frame IRQ chevaucherait potentiellement les données sauvegardées
    // lors d'une imbrication d'interruptions profonde.
    //
    // kstack_top() est l'adresse haute fixe, toujours valide comme RSP0.
    let rsp0 = next.kstack_top();
    tss::update_rsp0(next.current_cpu().0 as usize, rsp0);
    percpu::set_kernel_rsp(rsp0);
}
```

---

## Patch 6 — switch.rs : Mettre à jour le commentaire de la séquence

```rust
// Dans le bloc doc de context_switch(), remplacer la ligne :
/// 9. Mettre à jour TSS.RSP0 ← next.kstack_ptr (V7-C-03 OBLIGATOIRE).

// PAR (CORR-ALPHA-03) :
/// 9. Mettre à jour TSS.RSP0 ← next.kstack_top() (V7-C-03 + CORR-ALPHA-03).
///    kstack_top() est le SOMMET INVARIANT de la pile kernel.
///    Ne jamais utiliser kstack_ptr (RSP sauvegardé, position intermédiaire).
```

---

## Test de non-régression

```rust
#[test]
fn kstack_top_invariant_after_switch_simulation() {
    // Vérifier que kstack_top() retourne bien la valeur initialisée,
    // même après avoir modifié kstack_ptr.
    let mut tcb = ThreadControlBlock::new(
        ThreadId(1), ProcessId(1), SchedPolicy::Normal,
        Priority::NORMAL_DEFAULT, 0, 0xFFFF_FFFF_8000_0000u64,
    );

    // kstack_top() doit retourner la valeur passée à new()
    assert_eq!(tcb.kstack_top(), 0xFFFF_FFFF_8000_0000u64,
        "kstack_top doit correspondre à kernel_stack_top");

    // Simuler ce que context_switch_asm fait : modifier kstack_ptr
    tcb.kstack_ptr = 0xFFFF_FFFF_8000_0000u64 - 48;

    // kstack_top() doit rester inchangé
    assert_eq!(tcb.kstack_top(), 0xFFFF_FFFF_8000_0000u64,
        "kstack_top ne doit pas changer après modification de kstack_ptr");

    // kstack_ptr est bien différent de kstack_top après simulation de switch
    assert_ne!(tcb.kstack_top(), tcb.kstack_ptr,
        "kstack_top et kstack_ptr doivent diverger après le premier switch");
}
```

---

*— claude-alpha*
