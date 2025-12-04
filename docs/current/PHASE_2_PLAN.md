# Phase 2: Fork Context Copy & Full POSIX Semantics

## Date: 2024-12-04
## Version: v0.5.0 → v0.6.0

---

## 🎯 Objectifs Phase 2

### 1. Fork Proper - Return 0 in Child ✨ PRIORITÉ 1
**Objectif**: Fork doit retourner 0 dans l'enfant, child_pid dans le parent (POSIX)

**État actuel**:
- Fork crée threads avec `child_entry_point()` fixe
- Les deux (parent et enfant) voient `child_pid` comme retour
- Enfant ne peut pas exécuter code conditionnel `if (fork() == 0)`

**Solution**:
1. Copier contexte CPU parent (registres, RIP, RSP)
2. Créer nouvelle stack pour enfant avec copie de la stack parent
3. Modifier RAX de l'enfant = 0 (valeur retour syscall)
4. Parent continue avec RAX = child_pid

**Fichiers à modifier**:
- `kernel/src/scheduler/thread/thread.rs`
  - Ajouter `Thread::fork_from(parent: &Thread) -> Self`
  - Copier ThreadContext parent
  - Allouer et copier stack
  
- `kernel/src/syscall/handlers/process.rs`
  - Modifier `sys_fork()` pour utiliser `fork_from()` au lieu de `new_kernel()`
  - Setup RAX=0 pour enfant

- `kernel/src/arch/x86_64/context.rs` (si existe)
  - Vérifier structure ThreadContext
  - Méthode `clone_for_fork()` ?

**Tests à ajouter**:
```rust
pub fn test_fork_return_value() {
    match sys_fork() {
        Ok(0) => {
            // Code enfant - vérifie qu'on est bien ici
            log::info!("Child: fork returned 0 ✓");
            sys_exit(42);
        }
        Ok(child_pid) => {
            // Code parent
            log::info!("Parent: fork returned {}", child_pid);
            let (pid, status) = sys_wait(child_pid, ...);
            assert_eq!(status.exit_code(), 42);
        }
        Err(_) => panic!("Fork failed")
    }
}
```

---

### 2. Exec with Real ELF Binary 📦 PRIORITÉ 2
**Objectif**: Tester `sys_exec()` avec un vrai binaire ELF

**État actuel**:
- `sys_exec()` implémenté (parsing ELF, loading segments, setup stack)
- Jamais testé car `/tmp/hello.elf` n'existe pas
- Test skip avec warning

**Solution**:
1. Créer petit binaire ELF de test (C ou ASM)
2. L'inclure dans l'image ISO ou le VFS
3. Tester chargement et exécution

**Binaire test minimal** (`hello.c`):
```c
int main(void) {
    // Syscall write(1, "Hello\n", 6)
    asm volatile(
        "mov $1, %%rax\n"    // sys_write
        "mov $1, %%rdi\n"    // stdout
        "lea msg(%%rip), %%rsi\n"
        "mov $6, %%rdx\n"
        "syscall\n"
        "mov $60, %%rax\n"   // sys_exit
        "xor %%rdi, %%rdi\n"
        "syscall\n"
        ::: "rax", "rdi", "rsi", "rdx"
    );
    return 0;
}
```

**Compilation**:
```bash
gcc -static -nostdlib -o hello.elf hello.c
# Ou avec musl-gcc pour statique léger
```

**Intégration**:
- Option 1: Inclure dans ISO sous `/bin/hello`
- Option 2: Ajouter au ramdisk initial
- Option 3: Créer dans VFS tmpfs au boot

**Tests**:
```rust
pub fn test_exec_hello() {
    match sys_exec("/bin/hello", &[], &[]) {
        Ok(_) => {
            // Ne devrait jamais arriver - exec remplace processus
            panic!("exec returned!");
        }
        Err(e) => {
            log::error!("exec failed: {:?}", e);
        }
    }
}
```

---

### 3. Fork + Exec + Wait Integration Test 🔄 PRIORITÉ 3
**Objectif**: Test complet du cycle avec vraie séparation parent/enfant

**Test complet**:
```rust
pub fn test_fork_exec_wait() {
    log::info!("Test: fork + exec + wait integration");
    
    match sys_fork() {
        Ok(0) => {
            // Enfant: exécute programme externe
            log::info!("Child: executing /bin/hello...");
            sys_exec("/bin/hello", &[], &[]);
            panic!("exec should not return");
        }
        Ok(child_pid) => {
            // Parent: attend l'enfant
            log::info!("Parent: waiting for child {}...", child_pid);
            
            let (pid, status) = sys_wait(child_pid, WaitOptions {
                nohang: false,
                ..Default::default()
            }).expect("wait failed");
            
            log::info!("Parent: child {} exited with status {:?}", pid, status);
            assert_eq!(pid, child_pid);
        }
        Err(e) => {
            panic!("fork failed: {:?}", e);
        }
    }
}
```

---

## 📋 Plan d'Action

### Étape 1: Context Copy Infrastructure
1. **Examiner ThreadContext actuel**
   ```bash
   grep -r "ThreadContext" kernel/src/
   ```

2. **Vérifier structure de stack**
   - Comment stack est allouée dans `Thread::new_kernel()`
   - Format de la stack (RSP, frame layout)

3. **Implémenter `Thread::fork_from()`**
   - Copier tous les registres généraux
   - Copier RIP (instruction pointer) - doit pointer vers retour de syscall
   - Allouer nouvelle stack de même taille
   - Copier contenu de la stack parent
   - Ajuster RSP pour pointer vers nouvelle stack

4. **Modifier RAX dans contexte enfant**
   - `child.context.rax = 0`
   - Parent garde `rax = child_pid` (déjà set par syscall handler)

### Étape 2: Créer Binaire Test
1. **Écrire hello.c minimal**
2. **Compiler statique**
3. **Vérifier avec `readelf -h hello.elf`**
4. **Ajouter à l'image ISO**

### Étape 3: Intégrer et Tester
1. **Modifier sys_fork() pour utiliser fork_from()**
2. **Ajouter test_fork_return_value()**
3. **Compiler et tester**
4. **Debug si nécessaire**

### Étape 4: Test Exec
1. **Ajouter hello.elf au VFS ou ISO**
2. **Activer test_exec()**
3. **Vérifier chargement ELF**
4. **Vérifier exécution**

### Étape 5: Test Intégration
1. **Ajouter test_fork_exec_wait()**
2. **Valider cycle complet**
3. **Documenter résultats**

---

## 🚧 Défis Techniques Anticipés

### 1. Stack Copying
**Challenge**: La stack contient des pointeurs absolus vers elle-même
- Frames de fonction avec saved RBP
- Adresses de retour relatives à la stack

**Solution potentielle**:
- Copier stack byte-par-byte
- Ajuster RSP et RBP relativement
- Laisser les saved RBP pointer vers ancienne stack (?)
- Alternative: ne pas copier stack, recommencer fresh

### 2. RIP (Instruction Pointer)
**Challenge**: Où l'enfant doit-il commencer l'exécution ?
- Parent est dans syscall_handler quand fork() appelé
- Enfant doit "revenir" du syscall avec RAX=0

**Solution**:
- RIP doit pointer vers instruction après `syscall`
- Copier RIP du parent tel quel
- Le contexte switch restaurera RIP normalement

### 3. Registres Caller-Saved
**Challenge**: Certains registres sont volatils pendant syscall
- RAX, RCX, RDX, RSI, RDI, R8-R11

**Solution**:
- syscall_entry.asm doit sauvegarder TOUS les registres
- ThreadContext doit contenir tous les registres généraux
- fork_from() copie tout

### 4. TLS (Thread Local Storage)
**Challenge**: Chaque thread a son propre FS/GS base
- FS pointe vers TLS du thread
- Contient variables thread-local

**Solution Phase 2**:
- Ignorer pour l'instant (pas de TLS userspace)
- Phase 3: copier TLS aussi

---

## 📊 Critères de Succès Phase 2

### Must Have ✅
- [ ] Fork retourne 0 dans enfant, child_pid dans parent
- [ ] Test `if (fork() == 0)` fonctionne
- [ ] Enfant et parent peuvent exécuter code différent
- [ ] Exec charge et exécute binaire ELF simple
- [ ] Test fork+exec+wait complet passe

### Nice to Have 🎁
- [ ] Stack copying parfait (toutes pointeurs ajustés)
- [ ] Performance acceptable (<1ms pour fork)
- [ ] Gestion erreurs robuste
- [ ] Logging détaillé pour debug

### Phase 3 ⏭️
- [ ] COW (Copy-on-Write) réel pour mémoire
- [ ] TLS (Thread Local Storage)
- [ ] Signal handling pendant fork
- [ ] vfork() optimization

---

## 📚 Ressources

### Documentation OS Dev
- https://wiki.osdev.org/Fork
- https://wiki.osdev.org/Context_Switching
- https://wiki.osdev.org/ELF

### Linux Source (référence)
- `kernel/fork.c` - copy_process()
- `arch/x86/kernel/process_64.c` - copy_thread()

### Exo-OS Files à Étudier
- `kernel/src/scheduler/thread/thread.rs`
- `kernel/src/scheduler/switch/windowed.rs` (context switch)
- `kernel/src/arch/x86_64/syscall_entry.asm`
- `kernel/src/syscall/handlers/process.rs`

---

## 🔄 Workflow Phase 2

1. **Jour 1**: Context copy infrastructure
   - Thread::fork_from() skeleton
   - Stack allocation/copy
   - Register copy

2. **Jour 2**: Test fork return values
   - Modifier sys_fork()
   - Test validation
   - Debug

3. **Jour 3**: Binaire test ELF
   - Créer hello.c
   - Compiler
   - Intégrer ISO

4. **Jour 4**: Test exec
   - Activer test_exec()
   - Valider chargement
   - Debug

5. **Jour 5**: Intégration finale
   - test_fork_exec_wait()
   - Documentation
   - Commit

---

## 🎯 Prochaine Session

**Commencer par**:
1. Examiner structure ThreadContext actuelle
2. Regarder comment new_kernel() alloue stack
3. Créer Thread::fork_from() skeleton
4. Premiers tests

**Commande rapide démarrage**:
```bash
cd /workspaces/Exo-OS
grep -A20 "pub struct ThreadContext" kernel/src/
grep -A30 "pub fn new_kernel" kernel/src/scheduler/thread/thread.rs
```
