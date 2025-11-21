# 🐛 Journal des Corrections - Exo-OS Kernel

## Session du 21 novembre 2025

### Bug #1: Syntaxe Assembleur AT&T vs Intel
**Sévérité:** 🔴 CRITIQUE  
**Fichier:** `kernel/src/arch/x86_64/interrupts/pic.rs`  
**Lignes:** 131, 137

**Erreur:**
```
error: invalid operand for instruction
  --> kernel/src/arch/x86_64/interrupts/pic.rs:131:5
   |
   | asm!("out %al, %dx", ...)
   |      ^^^^^^^^^^^^^^^^ invalid AT&T syntax
```

**Cause:** Utilisation de syntaxe AT&T (`%al`, `%dx`) au lieu de Intel (`al`, `dx`)

**Correction:**
```rust
// AVANT
asm!("out %al, %dx", in("al") value, in("dx") port, ...);
asm!("in %dx, %al", out("al") result, in("dx") port, ...);

// APRÈS
asm!("out dx, al", in("al") value, in("dx") port, options(nomem, nostack));
asm!("in al, dx", out("al") result, in("dx") port, options(nomem, nostack));
```

**Statut:** ✅ RÉSOLU

---

### Bug #2: Conflit Registre RBX en Mode PIC
**Sévérité:** 🔴 CRITIQUE  
**Fichier:** `kernel/src/arch/x86_64/cpu/cpuid.rs`  
**Lignes:** 133-157

**Erreur:**
```
error: inline assembly requires more registers than available
  --> kernel/src/arch/x86_64/cpu/cpuid.rs:140:5
```

**Cause:** En mode PIC, `rbx` est réservé pour le pointeur GOT (Global Offset Table). L'instruction CPUID modifie `ebx`, créant un conflit.

**Tentatives échouées:**
1. ❌ `xchg {0:r}, rbx` avec `inout(reg)` - toujours insuffisant
2. ❌ `out(reg)` sans initialisation - variable non initialisée
3. ❌ Contraintes complexes de registres - allocation impossible

**Solution finale:** Compiler intrinsics
```rust
// AVANT (épuisement registres)
unsafe {
    asm!(
        "push rbx",
        "cpuid",
        "mov {0:e}, ebx",
        "pop rbx",
        in("eax") eax,
        out("eax") eax_out,
        out("edx") edx_out,
        out("ecx") ecx_out,
        out(reg) ebx_out,
        options(nostack, preserves_flags)
    );
}

// APRÈS (intrinsics safe)
pub unsafe fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    let result = core::arch::x86_64::__cpuid(leaf);
    (result.eax, result.ebx, result.ecx, result.edx)
}
```

**Avantages:**
- ✅ Pas de gestion manuelle de `rbx`
- ✅ Allocation registres automatique par le compilateur
- ✅ Code plus sûr et portable

**Statut:** ✅ RÉSOLU

---

### Bug #3: Épuisement Registres dans read_gprs()
**Sévérité:** 🔴 CRITIQUE  
**Fichier:** `kernel/src/arch/x86_64/registers.rs`  
**Lignes:** 175-225

**Erreur:**
```
error: inline assembly requires more registers than available
  --> kernel/src/arch/x86_64/registers.rs:183:9
```

**Cause:** Tentative d'utiliser 18 contraintes `out(reg)` dans un seul bloc `asm!()`. En mode PIC x86_64, seulement ~13 registres généraux disponibles (`rax`-`r15` moins `rbx`, `rsp`, `rbp` réservés).

**Correction:** Séparation en appels individuels
```rust
// AVANT (18 out(reg) dans un bloc)
unsafe {
    asm!(
        "mov {}, rax",
        "mov {}, rbx",
        // ... 16 autres instructions
        out(reg) gprs.rax,
        out(reg) gprs.rbx,
        // ... 16 autres contraintes
        options(nomem, preserves_flags)
    );
}

// APRÈS (18 appels séparés)
unsafe {
    asm!("mov {}, rax", out(reg) gprs.rax, options(nomem, nostack, preserves_flags));
    asm!("mov {}, rbx", out(reg) gprs.rbx, options(nomem, nostack, preserves_flags));
    asm!("mov {}, rcx", out(reg) gprs.rcx, options(nomem, nostack, preserves_flags));
    // ... 15 autres appels
    asm!("pushf; pop {}", out(reg) gprs.rflags, options(nomem, nostack));
}
```

**Impact performance:** Négligeable (fonction de debug/diagnostics rarement appelée)

**Statut:** ✅ RÉSOLU

---

### Bug #4: Directives NASM dans global_asm!()
**Sévérité:** 🟡 MODÉRÉ  
**Fichier:** `kernel/src/arch/x86_64/cpu/smp.rs`  
**Ligne:** 21

**Erreur:**
```
error: expected expression, found keyword `bits`
  --> kernel/src/arch/x86_64/cpu/smp.rs:21:1
   |
21 | global_asm!(include_str!("../boot/trampoline.asm"));
   | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```

**Cause:** `global_asm!()` attend du GAS (GNU Assembler) syntax, mais `trampoline.asm` contient des directives NASM:
- `bits 16` / `bits 32`
- `section .text`
- `global trampoline_start`
- `align 4096`

**Solution temporaire:** Commentaire du code
```rust
// DÉSACTIVÉ TEMPORAIREMENT - Directives NASM incompatibles avec global_asm!()
// TODO: Compiler trampoline.asm avec NASM séparément et linker comme objet
// global_asm!(include_str!("../boot/trampoline.asm"));
```

**Solution permanente (à implémenter):**
1. Ajouter dans `build.rs`:
   ```rust
   cc::Build::new()
       .file("src/arch/x86_64/boot/trampoline.asm")
       .compiler("nasm")
       .flag("-f").flag("elf64")
       .compile("trampoline");
   ```
2. Décommenter le `global_asm!()` une fois l'objet linké

**Statut:** ⚠️ WORKAROUND (SMP désactivé temporairement)

---

### Bug #5: Placeholder CPU Topology
**Sévérité:** 🟢 MINEUR  
**Fichier:** `kernel/src/arch/x86_64/cpu/topology.rs`  
**Ligne:** 154

**Problème:** Implémentation complexe de détection topologie CPU causait des erreurs inline asm

**Solution temporaire:**
```rust
pub fn get_intel_topology_level(_level: u32) -> Option<TopologyLevel> {
    // TODO: Implémenter détection topologie Intel avec CPUID leaf 0xB
    // Nécessite gestion correcte des registres en mode PIC
    None
}
```

**À implémenter:**
- Utiliser compiler intrinsics `__cpuid()` avec leaf 0xB
- Parser les bits de topology level (SMT, Core, Package)
- Gérer les CPUs AMD (différent de Intel)

**Statut:** 📝 TODO

---

### Bug #6: Conflit Allocateur Global et Panic Handler
**Sévérité:** 🟡 MODÉRÉ  
**Fichiers:** `kernel/src/main.rs`, `kernel/src/lib.rs`

**Erreur:**
```
error: found duplicate lang item `panic_impl`
error: the `#[global_allocator]` in this crate conflicts with global allocator in: exo_kernel
```

**Cause:** `main.rs` (binaire) et `lib.rs` (bibliothèque) définissaient tous deux:
- `#[panic_handler]`
- `#[global_allocator]`

**Solution:** Suppression du binaire
- ❌ Supprimé `[[bin]]` de `kernel/Cargo.toml`
- ❌ Supprimé `kernel/src/main.rs`
- ✅ Compilation bibliothèque uniquement (`cargo build --lib`)

**Raison:** La bibliothèque kernel sera liée par un boot stub externe

**Statut:** ✅ RÉSOLU (architecture modifiée)

---

## 📊 Statistiques de Correction

| Catégorie | Erreurs Initiales | Erreurs Finales |
|-----------|-------------------|-----------------|
| Syntaxe assembleur | 206 | 0 |
| Allocation registres | 2 | 0 |
| Lang items | 2 | 0 |
| **TOTAL** | **210** | **0** ✅ |

| Warnings | Nombre |
|----------|--------|
| Variables inutilisées | ~180 |
| Imports inutilisés | ~30 |
| Static mut refs | ~15 |
| Naming conventions | ~6 |
| **TOTAL** | **231** |

## 🔍 Leçons Apprises

1. **Mode PIC et Registres:**
   - `rbx` est **toujours** réservé en PIC mode
   - Préférer compiler intrinsics aux hacks inline asm
   - Limiter le nombre de contraintes `out(reg)` par bloc

2. **Syntaxe Assembleur:**
   - Rust inline asm utilise **Intel syntax** par défaut
   - `global_asm!()` attend du **GAS**, pas NASM
   - Toujours spécifier `options(nomem, nostack)` quand possible

3. **Architecture Kernel:**
   - Séparer clairement bibliothèque et exécutable
   - Un seul panic handler par crate final
   - Un seul allocateur global par binaire

4. **Build System:**
   - NASM/GCC/GAS peuvent coexister via `build.rs`
   - Linker les objets externes avant Rust linking
   - Profil release: taille vs vitesse (`opt-level = "z"` vs `"3"`)

---

**Dernière mise à jour:** 21 novembre 2025  
**Prochaine révision:** Après implémentation boot stub
