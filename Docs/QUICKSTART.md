# 🚀 Quick Start - Test Exo-OS

## ✅ Compilation Validée
Le kernel compile sans erreurs ! (42 warnings non-critiques)

---

## 🧪 Option 1: Test Rapide (Recommandé)

### Installation des Outils

```powershell
# Installer bootimage
cargo install bootimage

# Installer llvm-tools
rustup component add llvm-tools-preview
```

### Lancer le Test

```powershell
# Depuis le dossier kernel/
cd kernel
cargo bootimage --run
```

**Résultat attendu**: QEMU démarre et affiche le output du kernel

---

## 🖥️ Option 2: Script PowerShell

```powershell
# Depuis la racine du projet
.\test-qemu.ps1
```

Ce script :
- ✅ Compile automatiquement
- ✅ Vérifie QEMU
- ✅ Guide l'installation si nécessaire

---

## 🔧 Option 3: Compilation Manuelle

```powershell
# Compiler
cd kernel
cargo +nightly build --target "../x86_64-unknown-none.json" -Z build-std=core,alloc,compiler_builtins

# Vérifier le résultat
ls target/x86_64-unknown-none/debug/libexo_kernel.a
```

---

## 📊 État Actuel

### ✅ Fonctionnel
- Compilation sans erreurs
- Code C (serial.c, pci.c) intégré
- Architecture x86_64 configurée
- GDT, IDT, Interrupts définis
- Scheduler implémenté
- IPC channels créés

### ⚠️ Stubs (À Implémenter)
- Memory allocator (utilise linked_list_allocator)
- Page tables (stubbed)
- Syscall dispatch (stubbed)
- Block drivers (stubbed)

### 🎯 Prochaines Étapes
1. **[MAINTENANT]** Tester le boot avec bootimage
2. **[ENSUITE]** Valider serial output
3. **[PUIS]** Implémenter memory allocator
4. **[APRÈS]** Mesurer baseline de performance
5. **[ENFIN]** Optimiser vers objectifs

---

## 🐛 Troubleshooting

### "bootimage not found"
```powershell
cargo install bootimage
rustup component add llvm-tools-preview
```

### "QEMU not found"
Installer QEMU:
- Chocolatey: `choco install qemu`
- Scoop: `scoop install qemu`
- Direct: https://qemu.weilnetz.de/w64/

### "linking error"
Vérifier que vous êtes dans le dossier `kernel/` avant de compiler

---

## 📖 Documentation Complète

- **TESTING.md** - Guide complet de test
- **ROADMAP.md** - Plan de développement et optimisation
- **README.md** - Présentation du projet

---

## 🎯 Objectifs de Performance (Après Tests)

Une fois le kernel stable, nous optimiserons vers :

| Métrique | Objectif |
|----------|----------|
| IPC Latency | < 500 ns |
| Context Switch | < 1 µs |
| Syscalls | > 5M/sec |
| Boot Time | < 500 ms |
| Threads | > 1M scalable |

**Mais d'abord : faire fonctionner le kernel ! 🚀**
