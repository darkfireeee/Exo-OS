# Debugging Checklist - Crash après early_print()

## Phase 1 : Vérifier le Cache (5 minutes)

### Étape 1.1 : Diagnostic automatique
```bash
chmod +x diagnose_cache.sh
./diagnose_cache.sh
```

**Attendu** : Script affiche si le kernel est à jour ou pas

**Si "CACHE PROBLEM CONFIRMED"** → Passer à Phase 2

**Si "No cache issues"** → Passer à Phase 3

---

## Phase 2 : Rebuild Complet (10 minutes)

### Étape 2.1 : Force rebuild
```bash
chmod +x force_rebuild.sh
./force_rebuild.sh
```

### Étape 2.2 : Vérifier le résultat
```bash
# Chercher le nouveau string
strings target/x86_64-unknown-none/release/kernel | grep "Magic: 0x"
```

**Attendu** : Le string "Magic: 0x" doit apparaître

### Étape 2.3 : Tester à nouveau
```bash
qemu-system-x86_64 -cdrom build/os.iso -serial file:serial.log -no-reboot -no-shutdown

# Dans un autre terminal
tail -f serial.log
```

**Attendu** : Voir le nouveau message "Magic: 0x..."

**Si ça marche** → Problème résolu ✅

**Si crash toujours** → Passer à Phase 3

---

## Phase 3 : Debug Runtime (30 minutes)

### Étape 3.1 : Remplacer main.rs par version debug

```bash
# Backup de l'ancien
cp kernel/src/main.rs kernel/src/main.rs.backup

# Utiliser la version debug
cp debug_crash.rs kernel/src/main.rs
```

### Étape 3.2 : Recompiler et tester
```bash
cd kernel
cargo build --release --target x86_64-unknown-none
cd ..
./build/image.sh

qemu-system-x86_64 -cdrom build/os.iso -serial file:serial.log -no-reboot -no-shutdown
```

### Étape 3.3 : Analyser les logs
```bash
cat serial.log
```

**Scénarios possibles** :

#### Scénario A : Crash sur "Small stack alloc"
```
=== RUST DEBUG START ===
Multiboot Magic:
0x36D76289
Multiboot Info:
0x00100000
Stack Pointer:
0x00120000
[CRASH]
```

**Diagnostic** : Stack overflow immédiat
**Solution** : Augmenter taille de stack dans boot.asm

#### Scénario B : Crash sur "Large stack alloc"
```
...
Medium stack alloc OK
Attempting large stack alloc...
[CRASH]
```

**Diagnostic** : Stack trop petite pour 4KB
**Solution** : Vérifier `stack_top` dans boot.asm

#### Scénario C : Tout passe
```
...
=== ALL TESTS PASSED ===
Entering infinite loop...
```

**Diagnostic** : Le problème vient du code original, pas de la stack
**Solution** : Réintroduire le code progressivement

---

## Phase 4 : Analyse Stack (si Phase 3 révèle un problème)

### Étape 4.1 : Vérifier la taille de stack définie

```bash
# Dans boot.asm, chercher :
grep -A 5 "section .bss" kernel/src/arch/x86_64/boot/boot.asm
```

**Attendu** :
```nasm
section .bss
align 16
stack_bottom:
    resb 16384          ; 16 KB de stack (devrait être suffisant)
stack_top:
```

**Si < 16KB** : Augmenter à 16384 (16 KB)

**Si déjà 16KB** : Augmenter à 65536 (64 KB)

### Étape 4.2 : Vérifier l'alignement
```bash
# Dans boot.asm, chercher la setup de RSP :
grep -A 2 "mov rsp" kernel/src/arch/x86_64/boot/boot.asm
```

**Attendu** :
```nasm
mov rsp, stack_top
and rsp, -16        ; Aligner sur 16 bytes
```

**Si pas d'alignement** : Ajouter la ligne `and rsp, -16`

### Étape 4.3 : Modifier boot.asm
```nasm
; kernel/src/arch/x86_64/boot/boot.asm

section .bss
align 16
stack_bottom:
    resb 65536          ; 64 KB de stack (généreux)
stack_top:

; Dans long_mode_start:
long_mode_start:
    ; ... setup segments ...
    
    ; Setup stack avec alignement
    mov rsp, stack_top
    and rsp, -16        ; CRITICAL: 16-byte alignment
    
    ; ... continuer ...
```

### Étape 4.4 : Rebuild et test
```bash
./force_rebuild.sh
```

---

## Phase 5 : Debug QEMU Avancé (si tout échoue)

### Étape 5.1 : Lancer avec GDB
```bash
# Terminal 1: QEMU avec GDB server
qemu-system-x86_64 \
    -cdrom build/os.iso \
    -serial file:serial.log \
    -s -S \
    -no-reboot -no-shutdown

# Terminal 2: GDB
gdb target/x86_64-unknown-none/release/kernel
(gdb) target remote :1234
(gdb) break rust_main
(gdb) continue
```

### Étape 5.2 : Inspecter au moment du crash
```gdb
(gdb) info registers
(gdb) x/20i $rip          # Désassembler à l'instruction actuelle
(gdb) x/100xb $rsp        # Dump de la stack
(gdb) backtrace           # Call stack
```

### Étape 5.3 : Chercher l'instruction qui plante
```gdb
(gdb) stepi               # Exécuter instruction par instruction
(gdb) info registers      # Après chaque step
```

**Si crash sur une instruction spécifique** :
- Noter l'adresse RIP
- Désassembler avec `objdump -d kernel`
- Identifier la fonction Rust correspondante

---

## Phase 6 : Solutions Connues

### Solution A : Stack Overflow
**Symptômes** : Crash sur premier appel de fonction
**Fix** : Augmenter stack à 64KB dans boot.asm

### Solution B : Stack Misalignment
**Symptômes** : Crash sur appel de fonction avec paramètres
**Fix** : Ajouter `and rsp, -16` après `mov rsp, stack_top`

### Solution C : Cache Cargo
**Symptômes** : Ancien code s'exécute malgré recompilation
**Fix** : `cargo clean && cargo build --release`

### Solution D : ISO pas regénérée
**Symptômes** : Nouveau kernel compilé mais ancien dans ISO
**Fix** : `rm build/os.iso && ./build/image.sh`

### Solution E : Section .bss non chargée
**Symptômes** : Variables globales non initialisées
**Fix** : Vérifier linker script que .bss est dans l'image

### Solution F : early_print() corrompt la stack
**Symptômes** : Crash exactement après premier appel
**Fix** : Utiliser la version debug qui fait char-by-char

---

## Quick Reference Commands

### Rebuild propre
```bash
cargo clean && cargo build --release && ./build/image.sh
```

### Test rapide
```bash
qemu-system-x86_64 -cdrom build/os.iso -serial stdio -display none
```

### Vérifier string dans kernel
```bash
strings target/x86_64-unknown-none/release/kernel | grep -i rust
```

### Dump complet kernel
```bash
objdump -d target/x86_64-unknown-none/release/kernel > kernel.asm
```

### Voir sections kernel
```bash
readelf -S target/x86_64-unknown-none/release/kernel
```

### Taille stack utilisée (approximation)
```bash
# Dans le code Rust, ajouter:
let stack_ptr: u64;
unsafe { asm!("mov {}, rsp", out(reg) stack_ptr); }
println!("Stack: 0x{:016X}", stack_ptr);
```

---

## Checklist Finale

Avant de demander de l'aide, vérifier :

- [ ] `cargo clean` exécuté
- [ ] `strings kernel | grep "nouveau_string"` trouve le texte
- [ ] ISO regénérée (`rm build/os.iso`)
- [ ] Timestamps cohérents (source < binary < ISO)
- [ ] Stack ≥ 16KB dans boot.asm
- [ ] Stack alignée sur 16 bytes (`and rsp, -16`)
- [ ] serial.log contient TOUS les messages (pas de buffer)
- [ ] QEMU lancé avec `-no-reboot` pour voir l'erreur finale
- [ ] Testé avec la version debug (debug_crash.rs)

---

## Contact / Help

Si toujours bloqué après avoir suivi TOUTES les étapes :

1. Partager :
   - Le contenu de `serial.log`
   - Le résultat de `./diagnose_cache.sh`
   - Le résultat de `strings kernel | head -50`
   - La sortie de `cargo build -vv`

2. Screenshots QEMU si applicable

3. Version de :
   - rustc (`rustc --version`)
   - QEMU (`qemu-system-x86_64 --version`)
   - grub-mkrescue (`grub-mkrescue --version`)
