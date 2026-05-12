# claude-iota-bug-P1-rflags-mask.md

**Sévérité** : P1 — Haut (sécurité + stabilité)  
**Fichier** : `kernel/src/process/lifecycle/fork.rs`  
**Symptôme** : Héritage de flags dangereux dans le processus fils

---

## Description

Dans `do_fork()`, le masquage des RFLAGS propagés au fils est incorrect :

```rust
// fork.rs lignes ~340-352
const RFLAGS_SAFE_MASK: u64 = 0x0000_0000_0020_0CD5; // CF,PF,AF,ZF,SF,DF,OF,ID
const RFLAGS_FORCE_SET: u64 = 0x0000_0000_0000_0200; // IF=1
const RFLAGS_FORCE_CLR: u64 = 0x0000_0000_0004_0100; // "TF=0, NT=0, RF=0, VM=0" ← FAUX

let child_rflags =
    ((ctx.parent_rflags & RFLAGS_SAFE_MASK) | RFLAGS_FORCE_SET) & !RFLAGS_FORCE_CLR;
```

### Décomposition de `RFLAGS_FORCE_CLR = 0x0000_0000_0004_0100`

| Bit | Valeur hex | Flag | Présent dans 0x40100 ? |
|-----|-----------|------|------------------------|
| 8   | 0x000100  | TF (Trap Flag)      | ✅ Oui — correct |
| 14  | 0x004000  | NT (Nested Task)    | ❌ **Non — manquant** |
| 16  | 0x010000  | RF (Resume Flag)    | ❌ **Non — manquant** |
| 17  | 0x020000  | VM (Virtual 8086)   | ❌ **Non — manquant** |
| **18**  | **0x040000**  | **AC (Alignment Check)** | ✅ **Oui — mais non documenté** |

Le commentaire dit effacer TF, NT, RF, VM. En réalité le masque efface TF et AC.  
NT, RF, VM ne sont **pas effacés**.

### Risques

- **NT (Nested Task)** : si le parent était en mode nested task (rare mais possible), le fils hérite ce flag → au premier `iret`, le CPU suit le lien de tâche. Comportement indéfini sur les noyaux modernes sans TSS de tâche réel.
- **RF (Resume Flag)** : le fils démarrerait avec RF=1 → le premier breakpoint matériel est ignoré. Problème surtout en debug.
- **VM (Virtual 8086)** : si hérité → le fils démarrerait en mode V86. Crash immédiat garanti.

### Valeur correcte

```
TF = bit  8 = 0x000100
NT = bit 14 = 0x004000
RF = bit 16 = 0x010000
VM = bit 17 = 0x020000
Total         = 0x034100
```

---

## Correction

```rust
// fork.rs — remplacer
const RFLAGS_FORCE_CLR: u64 = 0x0000_0000_0004_0100; // FAUX

// par
const RFLAGS_FORCE_CLR: u64 = 0x0000_0000_0003_4100; // TF | NT | RF | VM
```

### Vérification du RFLAGS_SAFE_MASK

Le masque actuel `0x0000_0000_0020_0CD5` vaut :
- bit 0  : CF ✅
- bit 2  : PF ✅  
- bit 4  : AF ✅
- bit 6  : ZF ✅
- bit 7  : SF ✅
- bit 10 : DF ✅
- bit 11 : OF ✅
- bit 21 : ID ✅

Le commentaire mentionne AC (bit 18) mais AC n'est pas dans le masque. C'est discutable (on pourrait vouloir laisser le fils contrôler SMAP/alignment), mais la conséquence est faible. Garder le masque actuel sauf si comportement SMAP requis.

---

## Test

```rust
#[test]
fn child_rflags_no_nt_rf_vm() {
    // Simuler un parent avec NT=1, RF=1, VM=1, TF=1, IF=1
    let parent_rflags = 0x0003_4302u64; // NT+RF+VM+TF+IF+ZF
    let child_rflags = apply_fork_rflags_mask(parent_rflags);
    assert_eq!(child_rflags & 0x0003_4100, 0, "NT, RF, VM, TF must be zero");
    assert_ne!(child_rflags & 0x0200, 0,     "IF must be set");
}
```
