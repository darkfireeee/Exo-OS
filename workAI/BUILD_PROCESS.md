# 🏗️ PROCÉDURE DE BUILD - Exo-OS

**Créé** : 23 novembre 2025
**Pour** : Copilot & Gemini
**Statut** : ACTIF

---

## 📋 Aperçu

Le kernel Exo-OS mélange du code Rust, C et Assembly. Le processus de build nécessite deux étapes distinctes à cause des incompatibilités entre rust-lld et les objets ELF64 natifs.

---

## 🔧 Build Complet (Windows)

### Étape 1 : Compiler les objets Boot
```powershell
.\link_boot.ps1
```

**Ce que fait ce script** :
1. Compile `boot.asm` avec NASM → `boot.o`
2. Compile `boot.c` avec GCC → `boot_c.o`
3. Crée une archive statique `libboot_combined.a` avec ar
4. Copie l'archive dans le répertoire cargo OUT_DIR

**Fichiers générés** :
- `target/x86_64-unknown-none/debug/boot_objs/boot.o`
- `target/x86_64-unknown-none/debug/boot_objs/boot_c.o`
- `target/x86_64-unknown-none/debug/boot_objs/libboot_combined.a`
- `target/x86_64-unknown-none/debug/libboot_combined.a` (copie finale)

### Étape 2 : Compiler le Kernel Rust
```powershell
cargo build
```

**Ce que fait cargo** :
1. Lance `build.rs` qui déclare la dépendance à `libboot_combined.a`
2. Compile tous les modules Rust
3. Link avec `libboot_combined.a` via rust-lld
4. Génère le binaire final `exo-kernel`

---

## 🔧 Build Complet (Linux/macOS)

### Étape 1 : Compiler les objets Boot
```bash
chmod +x link_boot.sh
./link_boot.sh
```

### Étape 2 : Compiler le Kernel
```bash
cargo build
```

---

## 🎯 Build Release

Pour une version optimisée :

```powershell
# Windows
.\link_boot.ps1 target\x86_64-unknown-none\release
cargo build --release

# Linux
./link_boot.sh target/x86_64-unknown-none/release
cargo build --release
```

---

## 🧪 Test QEMU

Après un build réussi :

```powershell
# Créer l'image bootable
cargo bootimage

# Lancer QEMU
qemu-system-x86_64 -drive format=raw,file=target/x86_64-unknown-none/debug/bootimage-exo-kernel.bin -serial stdio
```

---

## ⚠️ Dépannage

### Erreur : "libboot_combined.a not found"
**Cause** : Vous n'avez pas exécuté `link_boot.ps1` avant `cargo build`
**Solution** : Lancez d'abord `.\link_boot.ps1`

### Erreur : "nasm: command not found"
**Cause** : NASM n'est pas installé ou pas dans le PATH
**Solution** : 
- Windows : `winget install nasm` ou télécharger depuis https://www.nasm.us/
- Linux : `sudo apt install nasm`
- macOS : `brew install nasm`

### Erreur : "gcc: command not found"
**Cause** : GCC n'est pas installé
**Solution** :
- Windows : Installer MinGW-w64 ou MSYS2
- Linux : `sudo apt install build-essential`
- macOS : `xcode-select --install`

### Erreur : "ar: command not found"
**Cause** : Binutils non installé
**Solution** :
- Généralement fourni avec GCC/MinGW
- Linux : `sudo apt install binutils`

### Erreur : "undefined symbol: boot_main"
**Cause** : Le linkage n'a pas fonctionné correctement
**Solution** :
1. Supprimer `target/` : `Remove-Item -Recurse -Force target`
2. Relancer `.\link_boot.ps1`
3. Relancer `cargo build`

### Erreur : "rust-lld: archive member is neither ET_REL nor LLVM bitcode"
**Cause** : Vous utilisez directement les .o sans les archiver
**Solution** : Toujours utiliser `link_boot.ps1` qui crée l'archive .a correcte

---

## 🔄 Workflow de Développement

### Modification du code Rust uniquement
```powershell
# Pas besoin de recompiler boot
cargo build
```

### Modification de boot.asm ou boot.c
```powershell
# Recompiler les objets boot
.\link_boot.ps1

# Puis recompiler le kernel
cargo build
```

### Clean complet
```powershell
# Supprimer tous les artefacts
Remove-Item -Recurse -Force target

# Rebuild from scratch
.\link_boot.ps1
cargo build
```

---

## 📊 Performance de Build

**Build from scratch** (après clean) :
- link_boot.ps1 : ~2-5 secondes
- cargo build (debug) : ~30-60 secondes
- **Total** : ~35-65 secondes

**Build incrémental** (changement Rust) :
- cargo build (debug) : ~5-15 secondes

**Build incrémental** (changement boot.asm/c) :
- link_boot.ps1 : ~2-5 secondes
- cargo build (debug) : ~10-20 secondes
- **Total** : ~12-25 secondes

---

## 🎓 Pourquoi ce Workflow ?

### Problème Initial
rust-lld (le linker par défaut de Rust) utilise un format LLVM bitcode et ne peut pas directement lire les fichiers objets ELF64 générés par NASM ou GCC.

### Solutions Envisagées
1. ❌ Utiliser GNU ld : Pas disponible sur Windows facilement
2. ❌ Convertir ASM en inline Rust : Trop complexe, perd les avantages de NASM
3. ✅ **Créer une archive statique (.a)** : Compatible rust-lld, simple, portable

### Avantages de la Solution
- ✅ Compatible Windows, Linux, macOS
- ✅ Conserve la séparation ASM/C/Rust
- ✅ Builds incrémentaux rapides
- ✅ Pas de dépendances système complexes
- ✅ Standard dans l'écosystème bare-metal

---

## 📝 Notes pour Gemini

**Si tu dois modifier boot.asm ou boot.c** :
1. Édite le fichier normalement
2. Rappelle à l'utilisateur de lancer `.\link_boot.ps1`
3. Puis `cargo build`

**Si tu ajoutes du code Rust qui appelle boot** :
- Déclare les symboles extern en Rust : `extern "C" { fn boot_main(...); }`
- Pas besoin de modifier le build system

**Si tu ajoutes d'autres fichiers C/ASM** :
- Ajoute-les dans `link_boot.ps1` (section compilation)
- Ajoute-les dans l'archive ar
- Documente dans ce fichier

---

**Maintenu par** : Copilot
**Révision** : Chaque modification du build system
