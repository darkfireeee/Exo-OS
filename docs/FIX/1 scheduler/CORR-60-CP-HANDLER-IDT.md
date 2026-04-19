# CORR-60 — #CP Handler IDT : connecter exocage::cp_handler (CRITIQUE)

**Source :** Audit Claude3 (BUG-S1, P0)  
**Fichiers :** `kernel/src/arch/x86_64/exceptions.rs`, `kernel/src/arch/x86_64/idt.rs`  
**Impact TLA+ :** Propriété `CetNoRop` = **FAUSSE en production jusqu'à cette correction**  
**Priorité :** Phase 0 — BLOQUANT sécurité

---

## Constat exact

L'IDT (idt.rs:288) enregistre pour le vecteur 21 (#CP) :
```
exc_ctrl_protection_handler → do_ctrl_protection()
```

Ce handler fait :
- Userspace → `exception_return_to_user()` avec SIGSEGV (pas de HANDOFF ExoPhoenix)
- Kernel → `kernel_panic_exception()` (pas de HANDOFF ExoPhoenix)

`exocage::cp_handler()` implémente correctement la chaîne sécuritaire (ExoLedger P0 +
HANDOFF ExoPhoenix) mais **n'est enregistré nulle part dans l'IDT** et n'est appelé
depuis aucun site dans `kernel/src/arch/`.

Conséquence : toute violation CET (ROP/JOP, Shadow Stack corruption) en userspace est
traitée comme un SIGSEGV ordinaire. ExoShield est aveugle aux attaques CET.

---

## Correction

### Étape 1 — exceptions.rs : remplacer do_ctrl_protection

```rust
// exceptions.rs — SUPPRIMER ou remplacer le handler actuel
// AVANT (ligne 651)
extern "C" fn do_ctrl_protection(frame: *mut ExceptionFrame) {
    let frame = unsafe { &mut *frame };
    EXC_COUNTERS[21].fetch_add(1, Ordering::Relaxed);
    if frame.from_userspace() {
        exception_return_to_user(frame);
    } else {
        kernel_panic_exception("#CP Control Protection kernel", frame);
    }
}

// APRÈS — déléguer à exocage::cp_handler
extern "C" fn do_ctrl_protection(frame: *mut ExceptionFrame) {
    EXC_COUNTERS[21].fetch_add(1, Ordering::Relaxed);

    // Extraire le error_code depuis le frame (poussé par le CPU avant #CP)
    // error_code format pour #CP : bits [2:0] = type de violation CET
    let error_code: u64 = unsafe { (*frame).error_code() };

    // Déléguer ENTIÈREMENT à ExoShield — logging P0 + HANDOFF ExoPhoenix
    // SAFETY: appelé depuis l'IDT en Ring 0, frame valide, interruptions désactivées.
    unsafe {
        crate::security::exocage::cp_handler(frame as usize, error_code);
    }

    // cp_handler ne retourne pas normalement en cas de violation confirmée
    // (déclenche HANDOFF ou remplace le thread par un handler de signal).
    // Si cp_handler retourne (cas userspace récupérable), retour normal.
    if unsafe { (*frame).from_userspace() } {
        unsafe { exception_return_to_user(frame); }
    } else {
        kernel_panic_exception("#CP Control Protection kernel (post ExoCage)", unsafe { &mut *frame });
    }
}
```

### Étape 2 — vérifier la signature de cp_handler dans exocage.rs

```rust
// exocage.rs:402 — vérifier que la signature est compatible
pub extern "C" fn cp_handler(frame: usize, error_code: u64) {
    // ... implémentation existante
}
```

Si la signature diffère, adapter l'appel dans do_ctrl_protection en conséquence.

### Étape 3 — vérifier que ExceptionFrame expose error_code()

```rust
// Dans ExceptionFrame, ajouter si absent :
impl ExceptionFrame {
    /// Lit le error_code poussé par le CPU (valide pour #CP, #GP, #PF, etc.)
    pub fn error_code(&self) -> u64 {
        // L'error_code est poussé par le CPU avant l'adresse de retour
        // Layout exact dépend du stub ASM — vérifier exception_frame.rs
        self.error_code
    }
}
```

### Étape 4 — idt.rs : aucun changement nécessaire

`exc_ctrl_protection_handler` est déjà enregistré correctement sur `EXC_CTRL_PROT`
(ligne 288). Seul le corps de `do_ctrl_protection` doit changer.

---

## Vérification post-correction

```bash
# Vérifier que cp_handler est appelé lors d'un #CP synthétique en QEMU
# avec CET activé (nécessite CPU simulé avec CET)
grep -r "cp_handler" kernel/src/ --include="*.rs"
# Doit montrer : exocage.rs (définition) + exceptions.rs (appel)
```

- [ ] `grep -r "cp_handler" kernel/src/arch/` retourne au moins un hit dans `exceptions.rs`
- [ ] Test QEMU CET : injection d'un #CP → log ExoLedger P0 créé
- [ ] Test QEMU CET : violation userspace → HANDOFF ExoPhoenix déclenché (pas SIGSEGV seul)
- [ ] Propriété TLA+ `CetNoRop` satisfaite après correction
