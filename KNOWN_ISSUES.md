# Problèmes Connus - Exo-OS

## 1. Erreur PageAlreadyMapped au boot (CRITIQUE)

**Symptôme** :
```
panicked at src\page_table.rs:105:25: failed to map segment starting at Page[4KiB](0x12d000): 
failed to map page Page[4KiB](0x12d000) to frame PhysFrame[4KiB](0x42e000): 
PageAlreadyMapped(PhysFrame[4KiB](0x42e000))
```

**Cause** :
Bug connu dans bootloader 0.9.x qui essaie de mapper des pages mémoire déjà mappées lors du chargement des segments ELF du kernel.

**Solutions tentées** :
1. ❌ Désactivation de `map_physical_memory` feature → Pas d'effet
2. ❌ Utilisation de versions spécifiques (0.9.8, 0.9.23, 0.9.27) → Même erreur
3. ❌ Migration vers bootloader 0.11 → Incompatibilité avec `serde_core` en mode `no_std`
4. ❌ Modification du linker script → Pas d'effet sur le bug

**Solutions possibles** :
1. **Créer un bootloader custom** basé sur multiboot2 ou UEFI
2. **Utiliser GRUB** comme bootloader avec une configuration multiboot
3. **Patcher bootloader 0.9** pour corriger le bug de mapping
4. **Attendre bootloader 0.12** avec meilleur support no_std

**Workaround temporaire** :
Pour tester le kernel sans bootloader :
```bash
# Compiler en tant que bibliothèque uniquement
cargo build --lib --target ../x86_64-unknown-none.json

# Utiliser GRUB pour booter
grub-mkrescue -o exo-os.iso isodir/
```

## 2. Incompatibilité bootloader 0.11

**Symptôme** :
Erreurs de compilation dans `serde_core` :
```
error[E0412]: cannot find type `Result` in this scope
error[E0425]: cannot find function `Err` in this scope
```

**Cause** :
Le bootloader 0.11 dépend de `serde` qui n'est pas fully compatible avec `build-std` en mode `no_std`.

**Status** : Non résolu

## 3. Code C non compilable

**Symptôme** :
```
rust-lld: warning: archive member 'serial.o' is neither ET_REL nor LLVM bitcode
rust-lld: error: undefined symbol: serial_write_char
```

**Cause** :
- GCC génère des fichiers objets ELF incompatibles avec rust-lld
- Clang n'est pas installé sur le système Windows

**Solution appliquée** :
✅ Remplacé le code C par une implémentation Rust pure dans `drivers/serial.rs`

## 4. Dates de résolution estimées

- **PageAlreadyMapped** : En attente de bootloader 0.12 ou développement bootloader custom (2-4 semaines)
- **Code C** : ✅ Résolu avec implémentation Rust
- **Bootloader 0.11** : Bloqué par upstream serde

## 5. État actuel du projet

✅ **Fonctionnel** :
- Compilation du kernel (lib + bin)
- Module serial Rust
- Architecture x86_64 (GDT, IDT, interruptions)
- Ordonnanceur de threads
- Système IPC
- Allocateur mémoire
- Syscalls

❌ **Non fonctionnel** :
- Boot réel (bloqué par bootloader)
- Tests d'intégration nécessitant le boot

🔄 **En cours** :
- Recherche d'alternative au bootloader
- Documentation complète
