# Guide de démarrage rapide - Exo-OS

## 🚀 Démarrage rapide (5 minutes)

### Étape 1 : Installer les dépendances

Depuis Windows PowerShell :

```powershell
cd C:\Users\Eric\Documents\Exo-OS
.\build-wsl.ps1
# Choisir option [1] pour installer les dépendances
```

### Étape 2 : Compiler

```powershell
.\build-wsl.ps1
# Choisir option [2] pour compiler
```

### Étape 3 : Tester

```powershell
.\build-wsl.ps1
# Choisir option [3] pour compiler et lancer dans QEMU
```

## 📁 Fichiers importants

- `build-wsl.ps1` - Script PowerShell interactif pour Windows
- `scripts/setup-wsl.sh` - Installe les dépendances dans WSL
- `scripts/build-all.sh` - Compile tout (kernel + bootloader + ISO)
- `scripts/run-qemu.sh` - Lance dans QEMU
- `scripts/clean.sh` - Nettoie les fichiers de build
- `BUILD_GUIDE.md` - Guide complet et détaillé
- `RECAP_MIGRATION.md` - Récapitulatif de la migration Multiboot2

## 🐛 En cas de problème

Consultez :
1. `BUILD_GUIDE.md` - Section "Problèmes fréquents"
2. `KNOWN_ISSUES.md` - Problèmes connus
3. `bootloader/README.md` - Documentation du bootloader

## 🎯 Sortie attendue

```
===========================================
  Exo-OS Kernel v0.1.0
  Architecture: x86_64
  Bootloader: Multiboot2 + GRUB
===========================================
[BOOT] Multiboot2 magic validé: 0x36d76289
[MEMORY] Carte mémoire: ...
[INIT] Architecture x86_64...
[SUCCESS] Noyau initialisé avec succès!
```

## 📚 Documentation complète

Voir `BUILD_GUIDE.md` pour :
- Compilation manuelle étape par étape
- Débogage avancé
- Test sur matériel réel
- Références techniques
