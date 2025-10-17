# Exo-OS Testing Guide

## 🧪 Guide de Test du Kernel

### État Actuel

✅ **Compilation**: Le kernel compile sans erreurs  
⚠️ **Boot**: Nécessite un bootloader pour créer une image bootable  
📊 **Tests**: Framework de test à implémenter

---

## 🚀 Option 1: Test Rapide avec Bootimage (Recommandé)

### Installation

```powershell
# Installer bootimage
cargo install bootimage

# Installer llvm-tools (requis par bootimage)
rustup component add llvm-tools-preview
```

### Configuration

Ajouter au `kernel/Cargo.toml`:

```toml
[dependencies]
bootloader = { version = "0.9.23", features = ["map_physical_memory"] }

[package.metadata.bootimage]
# Définir le point d'entrée
test-args = [
    "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
    "-serial", "stdio",
    "-display", "none"
]
test-success-exit-code = 33  # (0x10 << 1) | 1
```

### Lancer le Test

```powershell
# Depuis le dossier kernel/
cargo bootimage

# Pour tester avec QEMU
cargo bootimage --run
```

---

## 🖥️ Option 2: Test Manuel avec QEMU

### Prérequis

Télécharger QEMU:
- **Windows**: https://qemu.weilnetz.de/w64/
- **Chocolatey**: `choco install qemu`
- **Scoop**: `scoop install qemu`

### Créer une Image ISO avec GRUB

```powershell
# 1. Compiler le kernel
cd kernel
cargo +nightly build --target ../x86_64-unknown-none.json -Z build-std=core,alloc,compiler_builtins

# 2. Créer la structure ISO
mkdir -p iso/boot/grub
cp target/x86_64-unknown-none/debug/libexo_kernel.a iso/boot/kernel.bin

# 3. Créer grub.cfg
echo "set timeout=0
set default=0

menuentry 'Exo-OS' {
    multiboot2 /boot/kernel.bin
    boot
}" > iso/boot/grub/grub.cfg

# 4. Créer l'ISO (nécessite grub-mkrescue)
grub-mkrescue -o exo-os.iso iso/
```

### Lancer avec QEMU

```powershell
qemu-system-x86_64 -cdrom exo-os.iso -serial stdio
```

---

## 🔬 Option 3: Test avec Script PowerShell

```powershell
# Exécuter le script de test
.\test-qemu.ps1
```

Ce script :
- ✅ Compile le kernel
- ✅ Vérifie QEMU
- ✅ Guide l'installation de bootimage
- ⚠️ Nécessite configuration bootloader

---

## 📝 Tests Unitaires (À Implémenter)

### Structure de Test

Créer `kernel/tests/` avec des tests d'intégration:

```rust
// tests/basic_boot.rs
#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(crate::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use exo_kernel::println;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    test_main();
    loop {}
}

fn test_runner(tests: &[&dyn Fn()]) {
    println!("Running {} tests", tests.len());
    for test in tests {
        test();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("[failed]\n");
    println!("Error: {}\n", info);
    loop {}
}

#[test_case]
fn test_println() {
    println!("test_println output");
}
```

### Exécuter les Tests

```powershell
cargo test --target ../x86_64-unknown-none.json
```

---

## 🎯 Checklist de Validation

Avant d'optimiser pour atteindre les objectifs de performance, valider :

### ✅ Phase 1: Boot Basique
- [ ] Le kernel compile sans erreurs (✅ FAIT)
- [ ] Le kernel boot avec bootloader
- [ ] Serial output fonctionne
- [ ] Pas de panic au démarrage

### ✅ Phase 2: Fonctionnalités de Base
- [ ] GDT chargée correctement
- [ ] IDT configurée
- [ ] Interruptions fonctionnent
- [ ] Timer interrupt visible
- [ ] Keyboard interrupt détecté

### ✅ Phase 3: Sous-systèmes
- [ ] Memory allocator initialisé
- [ ] Scheduler démarre
- [ ] IPC channel créé
- [ ] Thread spawn fonctionne
- [ ] Syscall interface OK

### ✅ Phase 4: Prêt pour Optimisation
- [ ] Tous les tests passent
- [ ] Aucun panic/crash
- [ ] Logs clairs et compréhensibles
- [ ] Benchmark baseline établi

---

## 🎪 Tests de Performance (Après Phase 4)

Une fois le kernel stable, mesurer les métriques de base :

```rust
// Exemple de benchmark IPC
let start = rdtsc();
for _ in 0..1000000 {
    channel.send(msg);
}
let end = rdtsc();
println!("IPC latency: {} cycles", (end - start) / 1000000);
```

### Métriques à Établir (Baseline)

| Métrique | Outil de Mesure | Objectif Actuel |
|----------|----------------|-----------------|
| **IPC Latency** | rdtsc + boucle | Mesurer baseline |
| **Context Switch** | rdtsc avant/après | Mesurer baseline |
| **Syscall Speed** | getpid() x 1M | Mesurer baseline |
| **Boot Time** | Timer depuis reset | Mesurer baseline |
| **Thread Creation** | spawn x 1000 | Mesurer baseline |

---

## 🐛 Debugging

### Serial Output

Le kernel utilise le port série pour debug:

```powershell
qemu-system-x86_64 -serial stdio ...
```

Tout `println!()` apparaît dans la console.

### GDB Debugging

```powershell
# Terminal 1: Lancer QEMU en mode debug
qemu-system-x86_64 -s -S -kernel kernel.bin

# Terminal 2: Connecter GDB
rust-gdb target/x86_64-unknown-none/debug/libexo_kernel.a
(gdb) target remote :1234
(gdb) break rust_main
(gdb) continue
```

---

## 📊 Prochaines Étapes

1. **[MAINTENANT]** Choisir une méthode de boot (bootimage recommandé)
2. **[ENSUITE]** Valider que le kernel boot et affiche du texte
3. **[PUIS]** Implémenter tests unitaires de base
4. **[APRÈS]** Établir baseline de performance
5. **[ENFIN]** Optimiser vers objectifs:
   - IPC < 500ns
   - Context Switch < 1µs
   - Syscalls > 5M/sec
   - Boot < 500ms

---

## 🆘 Problèmes Courants

### "bootimage not found"
```powershell
cargo install bootimage
rustup component add llvm-tools-preview
```

### "linking error"
Vérifier que `linker.ld` est à la racine et accessible depuis kernel/.cargo/config.toml

### "QEMU ne démarre pas"
Vérifier la version de QEMU: `qemu-system-x86_64 --version`  
Minimum recommandé: QEMU 5.0+

### "No output in serial"
Vérifier que `serial_init()` est appelé dans `rust_main()`

---

**Bon testing ! 🧪**
