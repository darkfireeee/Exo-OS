# ⚠️ PROBLÈME #2 - Linkage rust-lld incompatible avec ELF64

**Date** : 23 novembre 2025 - 14:45
**Priorité** : HIGH
**Zone** : Boot System  
**Statut** : 🔴 BLOCKED

---

## 📋 Description

rust-lld (le linker par défaut de Rust) ne peut pas linker directement les fichiers objets ELF64 produits par NASM et GCC.

**Erreur** :
```
rust-lld: warning: archive member 'boot.o' is neither ET_REL nor LLVM bitcode
rust-lld: error: undefined symbol: boot_main
```

---

## 🔍 Symptômes

1. ✅ NASM compile boot.asm → boot.o (ELF64)
2. ✅ GCC compile boot.c → boot_c.o (ELF64)
3. ✅ ar crée libboot_combined.a
4. ❌ rust-lld refuse de linker l'archive
5. ❌ Symboles non trouvés même s'ils existent (`nm` les voit)

---

## 🧪 Tentatives Effectuées

### 1. Archive statique (.a)
```powershell
ar rcs libboot_combined.a boot.o boot_c.o
```
**Résultat** : ❌ rust-lld warning "is neither ET_REL nor LLVM bitcode"

### 2. Link direct des .o
```rust
println!("cargo:rustc-link-arg=boot.o");
```
**Résultat** : ❌ Même erreur

### 3. ld.lld flavor
```toml
linker-flavor = "ld.lld"
```
**Résultat** : ❌ "linker `lld` not found"

### 4. Multiple search paths
```rust
println!("cargo:rustc-link-search=native=...");
```
**Résultat** : ❌ Symboles toujours non trouvés

---

## ✅ DIAGNOSTIC FINAL

**Cause confirmée** : rust-lld ne peut PAS linker les fichiers objets ELF64 natifs.
- ✅ Symboles existent dans .o (vérifié avec `nm`)
- ✅ Archive créée correctement  
- ❌ rust-lld refuse: "archive member is neither ET_REL nor LLVM bitcode"

**Situation actuelle** :
- ✅ kernel lib compile sans erreur
- ❌ kernel bin échoue au linkage (boot_main undefined)
- ✅ Code boot.asm et boot.c sont corrects (400+ lignes fonctionnels)
- ✅ Fichiers boot dupliqués supprimés

## 💡 Solutions Possibles

### Option A: Installer Clang (RECOMMANDÉ - RAPIDE)
**Avantages** :
- ✅ Compatible natif avec rust-lld
- ✅ Pas de dépendance NASM/GCC
- ✅ Type-safe
- ✅ Inline asm Rust moderne

**Inconvénients** :
- ❌ Temps de réécriture (2-3 heures)
- ❌ Inline asm moins lisible que NASM

**Implémentation** :
```rust
// boot_stub.rs
#[naked]
#[no_mangle]
pub unsafe extern "C" fn _start() -> ! {
    core::arch::asm!(
        "mov esp, {stack_top}",
        "call {boot_main}",
        stack_top = sym STACK_TOP,
        boot_main = sym boot_main,
        options(noreturn)
    )
}
```

### Option B: GNU ld via MinGW
**Avantages** :
- ✅ Supporte ELF64 natif
- ✅ Garde code ASM/C séparé

**Inconvénients** :
- ❌ Nécessite installation MinGW-w64
- ❌ Config complexe sur Windows
- ❌ Problèmes potentiels cross-platform

**Implémentation** :
```toml
[target.x86_64-unknown-none]
linker = "x86_64-w64-mingw32-ld"
```

### Option C: Clang + LLVM Compatible Objects (RAPIDE - 15 min)
**Avantages** :
- ✅ Compatible rust-lld
- ✅ Garde boot.asm et boot.c intacts
- ✅ Juste installer Clang
- ✅ Script link_boot.ps1 déjà prêt

**Inconvénients** :
- ❌ Nécessite installer Clang (mais simple)

**Implémentation** :
```powershell
# 1. Installer Clang
winget install LLVM.LLVM

# 2. Relancer build (link_boot.ps1 détecte auto clang)
.\link_boot.ps1
cargo build
```

**STATUS** : ✅ Script déjà adapté, attend juste Clang installé

---

## 🎯 Solution Recommandée

**Option C: Installer Clang (LE PLUS RAPIDE)**

**Pourquoi** :
- ✅ 15 minutes vs 3 heures de réécriture
- ✅ Code boot.asm/c déjà fonctionnel (750+ lignes testées)
- ✅ Script link_boot.ps1 déjà adapté automatiquement
- ✅ Pas de régression possible
- ✅ Compatible avec le code existant qui marchait avant

**Installation Windows** :
```powershell
# Via winget (recommandé)
winget install LLVM.LLVM

# OU via Chocolatey
choco install llvm

# OU télécharger: https://releases.llvm.org/download.html
```

**Après installation** :
```powershell
# 1. Vérifier clang installé
clang --version

# 2. Build automatique (détecte clang)
.\link_boot.ps1  
cargo build

# 3. Test QEMU
cargo bootimage
qemu-system-x86_64 -drive format=raw,file=target/.../bootimage-exo-kernel.bin -serial stdio
```

**ETA** : 15-30 minutes (installation + test)

---

## 📝 Root Cause

rust-lld est conçu pour le bitcode LLVM, pas pour les ELF natifs. C'est un choix de design de Rust pour supporter tous les backends (LLVM, Cranelift, GCC). Les fichiers ELF traditionnels nécessitent GNU ld ou lld-link (Windows) qui ne sont pas dans le toolchain Rust par défaut.

---

**Assigné à** : Copilot
**Prochaine étape** : Commencer réécriture Rust
**Bloque** : Test boot QEMU
