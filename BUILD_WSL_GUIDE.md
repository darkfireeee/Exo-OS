# 🚀 GUIDE BUILD & TEST WSL UBUNTU
## EXO-OS - Compilation et Tests sur WSL

**Date**: 12 Novembre 2025  
**Plateforme**: WSL Ubuntu + Rust + QEMU + GCC  
**Objectif**: Builder kernel bare-metal et lancer tests

---

## ✅ PRÉREQUIS (DÉJÀ INSTALLÉS)

- ✅ WSL Ubuntu
- ✅ Rust toolchain (rustc, cargo)
- ✅ QEMU (qemu-system-x86_64)
- ✅ GCC (build-essential)
- ✅ GRUB tools (grub-mkrescue)

---

## 📋 ÉTAPE 1: ACCÉDER AU PROJET DANS WSL

### Ouvrir WSL Ubuntu

```powershell
# Depuis PowerShell Windows
wsl
```

### Naviguer vers le projet

```bash
# Le projet Windows est accessible via /mnt/c/
cd /mnt/c/Users/Eric/Documents/Exo-OS

# Vérifier qu'on est au bon endroit
ls -la
# Devrait afficher: Cargo.toml, kernel/, Docs/, etc.
```

---

## 📋 ÉTAPE 2: CONFIGURATION RUST BARE-METAL

### Installer target x86_64-unknown-none

```bash
# Installer target bare-metal
rustup target add x86_64-unknown-none

# Vérifier installation
rustup target list | grep x86_64-unknown-none
# Devrait afficher: x86_64-unknown-none (installed)
```

### Vérifier toolchain

```bash
# Version Rust
rustc --version
# Exemple: rustc 1.75.0 (ou plus récent)

# Version Cargo
cargo --version
# Exemple: cargo 1.75.0
```

---

## 📋 ÉTAPE 3: BUILD KERNEL BARE-METAL

### Build Debug (rapide)

```bash
# Build debug avec target bare-metal
cargo build --target x86_64-unknown-none

# Vérifier binaire
ls -lh target/x86_64-unknown-none/debug/exo-kernel
```

**Attendu**: Compilation réussie, binaire créé

### Build Release (optimisé)

```bash
# Build release optimisé
cargo build --release --target x86_64-unknown-none

# Vérifier binaire
file target/x86_64-unknown-none/release/exo-kernel
# Devrait afficher: ELF 64-bit LSB executable, x86-64, statically linked
```

**Attendu**: 
- Compilation réussie
- Binaire ELF bare-metal
- Taille: ~500KB-2MB (selon optimisations)

---

## 📋 ÉTAPE 4: TESTS UNITAIRES (IMPORTANT!)

### Option A: Tests avec target par défaut (RECOMMANDÉ)

```bash
# Lancer tests unitaires
# Sur WSL Ubuntu, le linker GCC supporte l'inline assembly
cargo test --lib

# Devrait afficher:
# running 81 tests
# test ipc::channel::tests::test_create ... ok
# test memory::hybrid_allocator::tests::test_thread_cache ... ok
# ...
# test result: ok. 81 passed; 0 failed; 0 ignored
```

**SI ERREURS** avec inline assembly:
```bash
# Alternative: Tests sans inline asm (utilise stubs Windows)
cargo test --lib --target x86_64-pc-windows-msvc
```

### Option B: Tests bare-metal (AVANCÉ)

```bash
# Tester avec target bare-metal (peut nécessiter custom test framework)
cargo test --lib --target x86_64-unknown-none --no-run

# Puis exécuter dans QEMU (si test runner configuré)
```

---

## 📋 ÉTAPE 5: VÉRIFICATION COMPILATION

### Check sans build complet

```bash
# Vérification rapide (type checking)
cargo check --lib

# Avec target bare-metal
cargo check --lib --target x86_64-unknown-none
```

### Clippy (linter)

```bash
# Lancer Clippy pour suggestions qualité code
cargo clippy --lib --target x86_64-unknown-none

# Fix automatique warnings simples
cargo fix --lib --allow-dirty --allow-staged
```

---

## 📋 ÉTAPE 6: CRÉER IMAGE ISO BOOTABLE

### Créer structure répertoires

```bash
# Créer structure ISO
mkdir -p isodir/boot/grub

# Copier kernel
cp target/x86_64-unknown-none/release/exo-kernel isodir/boot/

# Vérifier copie
ls -lh isodir/boot/exo-kernel
```

### Créer fichier grub.cfg

```bash
# Créer configuration GRUB
cat > isodir/boot/grub/grub.cfg << 'EOF'
set timeout=0
set default=0

menuentry "EXO-OS Zero-Copy Fusion" {
    multiboot2 /boot/exo-kernel
    boot
}
EOF

# Vérifier contenu
cat isodir/boot/grub/grub.cfg
```

### Générer ISO avec GRUB

```bash
# Générer image ISO bootable
grub-mkrescue -o exo-os.iso isodir

# Vérifier ISO créé
ls -lh exo-os.iso
# Devrait afficher: ~5-10 MB

# Vérifier format ISO
file exo-os.iso
# Devrait afficher: ISO 9660 CD-ROM filesystem data
```

**SI ERREUR grub-mkrescue**:
```bash
# Installer dépendances GRUB
sudo apt-get update
sudo apt-get install -y grub-pc-bin xorriso mtools
```

---

## 📋 ÉTAPE 7: TESTS QEMU

### Test boot basique

```bash
# Lancer kernel dans QEMU
qemu-system-x86_64 \
    -cdrom exo-os.iso \
    -m 512M \
    -serial stdio \
    -display curses

# Touches:
# - ESC puis 2 pour quitter
# - Ctrl+A puis X pour forcer quit
```

**Attendu**:
- GRUB menu s'affiche
- Kernel boot
- Messages console visible

### Test avec output série

```bash
# Boot avec output série redirigé
qemu-system-x86_64 \
    -cdrom exo-os.iso \
    -m 512M \
    -serial file:serial.log \
    -display none \
    -no-reboot

# Puis visualiser log
cat serial.log
```

### Test avec debug GDB

```bash
# Terminal 1: Lancer QEMU avec GDB server
qemu-system-x86_64 \
    -cdrom exo-os.iso \
    -m 512M \
    -s -S \
    -serial stdio

# Terminal 2: Connecter GDB
gdb target/x86_64-unknown-none/release/exo-kernel
(gdb) target remote :1234
(gdb) continue
```

### Test avec KVM (accélération)

```bash
# Si CPU supporte virtualisation
qemu-system-x86_64 \
    -cdrom exo-os.iso \
    -m 512M \
    -enable-kvm \
    -cpu host \
    -serial stdio
```

**Note KVM**: Vérifier support virtualisation
```bash
# Vérifier si KVM disponible
lscpu | grep Virtualization
# Ou
egrep -c '(vmx|svm)' /proc/cpuinfo
# Si > 0, KVM supporté
```

---

## 📋 ÉTAPE 8: EXÉCUTER BENCHMARKS IN-KERNEL

### Modifier kernel pour auto-run benchmarks

Ajouter dans `kernel/src/main.rs`:

```rust
#[no_mangle]
pub extern "C" fn kernel_main() -> ! {
    // ... init hardware ...
    
    // Exécuter benchmarks automatiquement
    #[cfg(feature = "benchmarks")]
    {
        use crate::perf::BenchOrchestrator;
        
        serial_println!("\n=== EXO-OS BENCHMARKS START ===\n");
        
        let orchestrator = BenchOrchestrator::new();
        orchestrator.run_all_suites();
        
        serial_println!("\n=== BENCHMARKS COMPLETE ===\n");
    }
    
    // Halt kernel
    loop {
        unsafe { core::arch::asm!("hlt"); }
    }
}
```

### Build avec feature benchmarks

```bash
# Ajouter feature dans Cargo.toml (si pas déjà fait)
# [features]
# benchmarks = []

# Build avec benchmarks
cargo build --release --target x86_64-unknown-none --features benchmarks

# Recréer ISO
cp target/x86_64-unknown-none/release/exo-kernel isodir/boot/
grub-mkrescue -o exo-os.iso isodir

# Lancer avec output série
qemu-system-x86_64 \
    -cdrom exo-os.iso \
    -m 512M \
    -serial file:bench_results.log \
    -display none \
    -no-reboot

# Attendre fin (10-30 secondes)
# Puis visualiser résultats
cat bench_results.log
```

---

## 🔍 TROUBLESHOOTING

### Problème: "cannot find -lgcc"

```bash
# Installer gcc multilib
sudo apt-get install -y gcc-multilib
```

### Problème: "linker `rust-lld` not found"

```bash
# Installer lld
sudo apt-get install -y lld
# Ou utiliser ld.lld du système
rustup component add llvm-tools-preview
```

### Problème: Tests échouent avec "asm!" errors

```bash
# Vérifier qu'on est bien sur WSL (pas Windows PowerShell)
uname -a
# Devrait afficher: Linux ... x86_64 GNU/Linux

# Si toujours erreurs, utiliser stubs Windows
cargo test --lib --features windows-compat
```

### Problème: QEMU "Could not initialize SDL"

```bash
# Utiliser display curses ou none
qemu-system-x86_64 ... -display curses
# Ou
qemu-system-x86_64 ... -display none -serial stdio
```

### Problème: ISO trop grosse (>50MB)

```bash
# Build release strip symbols
cargo build --release --target x86_64-unknown-none

# Strip manuel
strip target/x86_64-unknown-none/release/exo-kernel

# Ou ajouter dans Cargo.toml
# [profile.release]
# strip = true
```

---

## 📊 COMMANDES RÉCAPITULATIVES

### Workflow complet (Copy-Paste Ready)

```bash
#!/bin/bash
# Script build & test EXO-OS

# 1. Naviguer vers projet
cd /mnt/c/Users/Eric/Documents/Exo-OS

# 2. Installer target
rustup target add x86_64-unknown-none

# 3. Tests unitaires (81 tests)
echo "=== RUNNING UNIT TESTS ==="
cargo test --lib
echo ""

# 4. Build release
echo "=== BUILDING RELEASE KERNEL ==="
cargo build --release --target x86_64-unknown-none
echo ""

# 5. Créer ISO
echo "=== CREATING BOOTABLE ISO ==="
mkdir -p isodir/boot/grub
cp target/x86_64-unknown-none/release/exo-kernel isodir/boot/

cat > isodir/boot/grub/grub.cfg << 'EOF'
set timeout=0
set default=0
menuentry "EXO-OS Zero-Copy Fusion" {
    multiboot2 /boot/exo-kernel
    boot
}
EOF

grub-mkrescue -o exo-os.iso isodir
echo ""

# 6. Lancer QEMU
echo "=== BOOTING IN QEMU ==="
echo "Press ESC then 2 to quit, or Ctrl+C to stop"
sleep 2

qemu-system-x86_64 \
    -cdrom exo-os.iso \
    -m 512M \
    -serial stdio \
    -display curses

echo ""
echo "=== BUILD & TEST COMPLETE ==="
```

### Sauvegarder script

```bash
# Créer script exécutable
cat > build_and_test.sh << 'EOF'
#!/bin/bash
cd /mnt/c/Users/Eric/Documents/Exo-OS
rustup target add x86_64-unknown-none
cargo test --lib
cargo build --release --target x86_64-unknown-none
mkdir -p isodir/boot/grub
cp target/x86_64-unknown-none/release/exo-kernel isodir/boot/
cat > isodir/boot/grub/grub.cfg << 'GRUBEOF'
set timeout=0
set default=0
menuentry "EXO-OS" {
    multiboot2 /boot/exo-kernel
    boot
}
GRUBEOF
grub-mkrescue -o exo-os.iso isodir
qemu-system-x86_64 -cdrom exo-os.iso -m 512M -serial stdio -display curses
EOF

# Rendre exécutable
chmod +x build_and_test.sh

# Lancer
./build_and_test.sh
```

---

## 🎯 PROCHAINES ÉTAPES

### Après validation QEMU

1. **Collecter benchmarks réels**
   - Exécuter suite complète in-kernel
   - Exporter résultats BENCH_RESULTS.md
   - Comparer vs prévisions

2. **Tests hardware physique** (optionnel)
   - Graver ISO sur USB
   - Boot sur machine x86_64 réelle
   - Valider tous composants

3. **Optimisations Phase 9** (si souhaité)
   - NUMA awareness
   - Lock-free complète
   - SIMD acceleration
   - NVMe driver natif

---

## 📝 NOTES IMPORTANTES

### Différences WSL vs Windows

| Aspect | Windows PowerShell | WSL Ubuntu |
|--------|-------------------|------------|
| **Inline asm** | ❌ Non supporté (MSVC) | ✅ Supporté (GCC) |
| **Tests unitaires** | ❌ Bloqués | ✅ Fonctionnent |
| **Build bare-metal** | ⚠️ Possible mais limité | ✅ Complet |
| **QEMU** | ⚠️ GUI uniquement | ✅ GUI + curses + none |
| **GRUB** | ❌ Non disponible | ✅ Natif |
| **ISO creation** | ❌ Difficile | ✅ Simple |

### Performances attendues

- **Build debug**: 30-60 secondes
- **Build release**: 1-3 minutes
- **Tests (81 tests)**: 5-15 secondes
- **ISO creation**: 2-5 secondes
- **QEMU boot**: 1-2 secondes

### Ressources WSL

```bash
# Vérifier ressources WSL
free -h           # Mémoire disponible
df -h /mnt/c      # Espace disque
nproc             # Nombre CPUs
```

---

## ✅ CHECKLIST VALIDATION

- [ ] WSL Ubuntu accessible (`wsl` dans PowerShell)
- [ ] Projet accessible (`cd /mnt/c/Users/Eric/Documents/Exo-OS`)
- [ ] Rust installé (`rustc --version`)
- [ ] Target bare-metal installé (`rustup target add x86_64-unknown-none`)
- [ ] Tests passent (`cargo test --lib` → 81/81 OK)
- [ ] Build réussit (`cargo build --release --target x86_64-unknown-none`)
- [ ] ISO créé (`grub-mkrescue -o exo-os.iso isodir`)
- [ ] QEMU boot (`qemu-system-x86_64 -cdrom exo-os.iso`)
- [ ] Benchmarks exécutés (in-kernel ou standalone)
- [ ] Résultats documentés (BENCH_RESULTS.md)

---

**Auteur**: Guide EXO-OS  
**Date**: 12 Novembre 2025  
**Version**: 1.0.0  

**FIN GUIDE BUILD WSL**
