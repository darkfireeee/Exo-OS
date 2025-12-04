# Phase 2 - Quick Start Guide

## 🚀 Démarrage Rapide

### État Actuel ThreadContext
```rust
pub struct ThreadContext {
    pub rsp: u64,      // Stack pointer
    pub rip: u64,      // Instruction pointer
    pub cr3: u64,      // Page table
    pub rflags: u64,   // Flags
    pub rdi: u64,      // Arg1 register
    pub rsi: u64,      // Arg2 register
}
```

**⚠️ Problème**: Contexte incomplet pour fork !
- Manque RAX (valeur retour)
- Manque RBX, RCX, RDX, R8-R15
- Manque RBP (base pointer)

### Actions Immédiates

#### 1. Compléter ThreadContext (PRIORITÉ 1)
```rust
pub struct ThreadContext {
    // Registres existants
    pub rsp: u64,
    pub rip: u64,
    pub cr3: u64,
    pub rflags: u64,
    
    // AJOUTER:
    pub rax: u64,  // Valeur retour syscall
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rbp: u64,  // Base pointer
    pub rdi: u64,  // Déjà présent
    pub rsi: u64,  // Déjà présent
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
}
```

#### 2. Modifier syscall_entry.asm
Le fichier `kernel/src/arch/x86_64/syscall_entry.asm` doit sauvegarder TOUS les registres.

Vérifier:
```bash
grep -A50 "syscall_handler_entry" kernel/src/arch/x86_64/syscall_entry.asm
```

#### 3. Modifier windowed context switch
`kernel/src/scheduler/switch/windowed.rs` doit restaurer tous les registres.

Vérifier:
```bash
grep -A20 "pub unsafe fn switch" kernel/src/scheduler/switch/windowed.rs
```

### Commandes de Diagnostic

```bash
# Voir la structure actuelle
cd /workspaces/Exo-OS
grep -A20 "pub struct ThreadContext" kernel/src/scheduler/thread/thread.rs

# Voir comment new_kernel crée thread
grep -A50 "pub fn new_kernel" kernel/src/scheduler/thread/thread.rs

# Voir syscall entry
cat kernel/src/arch/x86_64/syscall_entry.asm

# Voir context switch
grep -A30 "switch" kernel/src/scheduler/switch/windowed.rs
```

### Plan Simplifié Phase 2

**Option A: Approche Complète (Recommandée)**
1. Compléter ThreadContext avec tous registres
2. Modifier syscall_entry pour sauvegarder tout
3. Modifier windowed switch pour restaurer tout
4. Implémenter Thread::fork_from()
5. Tester

**Option B: Approche Minimale (Plus rapide)**
1. Ajouter seulement RAX à ThreadContext
2. Modifier sys_fork() pour set child.context.rax = 0
3. Enfant démarre à RIP du parent avec RAX=0
4. Tester avec code simple

**Recommandation**: Option A pour compatibilité future, mais Option B pour valider concept rapidement.

### Test Minimal

```rust
pub fn test_fork_rax() {
    let child_pid = sys_fork().unwrap();
    
    // Lire RAX d'une façon ou d'une autre
    // Pour l'instant, utiliser comportement:
    
    if child_pid == 0 {
        log::info!("✓ Child sees 0");
        sys_exit(42);
    } else {
        log::info!("✓ Parent sees {}", child_pid);
        sys_wait(child_pid, ...);
    }
}
```

### Prochaine Commande

```bash
# Démarrer Phase 2
cd /workspaces/Exo-OS
git checkout -b phase2-context-copy
code kernel/src/scheduler/thread/thread.rs
```

Chercher "ThreadContext" et ajouter les registres manquants.

---

## 📊 Checklist Phase 2

- [ ] Compléter ThreadContext avec tous registres
- [ ] Modifier syscall_entry.asm pour tout sauvegarder
- [ ] Modifier windowed.rs pour tout restaurer
- [ ] Implémenter Thread::fork_from()
- [ ] Modifier sys_fork() pour utiliser fork_from()
- [ ] Test fork return value
- [ ] Créer hello.elf binaire
- [ ] Test exec
- [ ] Test fork+exec+wait
- [ ] Documentation

## 🎯 Objectif Session

**Milestone 1**: Fork retourne 0 dans enfant
**Test**: `if (fork() == 0) { /* code enfant */ }`

Bonne chance ! 🚀
