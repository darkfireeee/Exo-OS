# PATCH NOTES — fix_17cf408b
## ExoOS v0.2.0 Strata — Corrections P0 → P2

**Base commit :** `17cf408b` (additionnal fix terminal 2)
**Auteur patch :** claude-alpha
**Date :** 2026-06-04
**Vérification :** `python3 tools/verify_all_patches.py` → 4/4 PASS

---

## Fichiers modifiés

| Fichier | Patch | Priorité |
|---|---|---|
| `kernel/src/exophoenix/resurrection.rs` | PATCH-P0-PHOENIX | P0 |
| `kernel/src/syscall/table.rs` | PATCH-P0-FORK-STUB | P0 (clarté) |
| `kernel/src/security/exocage.rs` | PATCH-P1-DEBUG | P1 |
| `kernel/src/main.rs` | PATCH-P2-BOOT | P2 |
| `kernel/src/arch/x86_64/boot/mod.rs` | PATCH-P2-BOOT | P2 |
| `kernel/Cargo.toml` | PATCH-P2-BOOT | P2 |

---

## PATCH-P0-PHOENIX — resurrection.rs

**Problème :** `try_recover_exception()` était gated par `TEST_ARMED`, un
`AtomicBool` initialisé à `false` uniquement activé par `trigger_self_destruct()`
dans les tests contrôlés. En production, toute exception Ring0 réelle passait
directement à un panic kernel sans tenter la récupération Phoenix.
La garantie "< 500ms recovery" ExoPhoenix n'existait qu'en mode test.

**Correction :**
```rust
// AVANT — guard toujours false en production
if !TEST_ARMED.swap(false, Ordering::AcqRel) {
    return false;
}

// APRÈS — production-ready
let phoenix_ready = PHOENIX_STATE.load(Ordering::Acquire) == PhoenixState::Normal as u8;
let test_triggered = TEST_ARMED.swap(false, Ordering::AcqRel);
if !phoenix_ready && !test_triggered {
    return false;
}
```

Le chemin test (`trigger_self_destruct`) fonctionne toujours via `test_triggered`.
En production, `phoenix_ready` est `true` dès que le kernel est en état `Normal`,
ce qui est le cas après le boot complet (avant toute exception).

---

## PATCH-P0-FORK-STUB — table.rs

**Problème :** `sys_fork`, `sys_vfork`, `sys_execve` dans `table.rs` retournaient
`ENOSYS` avec des commentaires cryptiques. L'implémentation réelle est dans
`dispatch.rs` (étapes [5b] et [5c]) qui intercepte ces syscalls AVANT la table.
Les stubs étaient du code mort mais créaient une confusion sur l'état réel.

**Correction :** Documentation explicite avec tags `PATCH-P0-FORK-STUB` et
`STRATA-DISPATCH-01` indiquant clairement que ces fonctions sont du code mort,
que le routing est dans `dispatch.rs`, et que `do_fork`/`do_execve` dans
`process/lifecycle/` sont les implémentations actives.

**Note importante :** `do_fork()` et `do_execve()` sont **entièrement implémentés**.
fork/exec fonctionnent en production via le chemin dispatch. Le rapport initial
P0-FORK était basé sur les stubs visibles sans voir le dispatch intercepteur.

---

## PATCH-P1-DEBUG — exocage.rs

**Problème :** 4 `debug_assert!` dans les fonctions d'accès au `_cold_reserve`
TCB (écriture/lecture u64 et u8) disparaissaient en release build. Sur un chemin
d'écriture TCB (accès de sécurité critique ExoCage), une violation d'offset
hors bornes aurait causé une corruption mémoire silencieuse en production.

**Correction :** 4 promotions `debug_assert!` → `assert!` avec message d'erreur
enrichi incluant la valeur de l'offset. Le `debug_assert!` sur `threat_score`
est conservé car il est déjà protégé par `.min(100)` (clamp matériel).

```rust
// AVANT — invisible en release
debug_assert!(offset + 8 <= 88, "TCB _cold_reserve write out of bounds");

// APRÈS — visible en release (production-safe)
assert!(offset + 8 <= 88,
    "PATCH-P1-DEBUG: TCB _cold_reserve write out of bounds: offset={}", offset);
```

---

## PATCH-P2-BOOT — main.rs + boot/mod.rs + Cargo.toml

**Problème :** Le chemin Multiboot2/GRUB restait actif malgré la vision Strata
"UEFI-only". Le header Multiboot2 était embarqué inconditionnellement dans le
binaire (section `.multiboot2`), et `init_memory_subsystem_multiboot2` était
exporté publiquement. Cela contredisait `SPEC-BOOTLOADER-GPT-STRATA.md`.

**Correction :**
1. `main.rs` : `global_asm!` du header Multiboot2 gated derrière
   `#[cfg(feature = "multiboot2_compat")]`.
2. `boot/mod.rs` : exports `parse_multiboot2`, `Multiboot2Info`,
   `init_memory_subsystem_multiboot2` gated derrière le même feature.
3. `Cargo.toml` : Feature `multiboot2_compat = []` déclarée avec commentaire
   de dépréciation.

**Impact :** En production UEFI (défaut, sans `--features multiboot2_compat`),
le header Multiboot2 n'est plus embarqué. Pour dev QEMU avec GRUB, utiliser :
```
cargo build --features multiboot2_compat
```

---

## Vérification

```bash
cd <racine_repo>
python3 tools/verify_all_patches.py
# Attendu : 4/4 PASS ✅ TOUS LES PATCHES VALIDÉS
```

---

*claude-alpha — ExoOS v0.2.0 Strata — PATCH_NOTES_17cf408b_FIX.md*
