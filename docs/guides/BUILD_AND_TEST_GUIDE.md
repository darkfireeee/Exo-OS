# 🔧 Guide de Build et Test - Exo-OS

## 📋 Processus de Build Complet

### 1. **Compilation Rust du Kernel**
```bash
cd /workspaces/Exo-OS
make build          # Build debug
# ou
make release        # Build optimisé
```

**Résultat :** `target/x86_64-unknown-none/[debug|release]/libexo_kernel.a`

### 2. **Structure du Linkage**

Le kernel final est créé en plusieurs étapes :

#### Étape 1 : Boot Objects
- `boot.asm` → `boot.o` (NASM)
- `boot.c` → `boot_c.o` (GCC)
- → `libboot_combined.a` (archive statique)

#### Étape 2 : Linkage Final
```bash
ld -n -o build/kernel.elf -T linker.ld \
    build/libboot_combined.a \
    target/x86_64-unknown-none/debug/libexo_kernel.a
```

#### Étape 3 : Conversion Binaire
```bash
objcopy -O binary build/kernel.elf build/kernel.bin
```

### 3. **Création de l'ISO Bootable**
```bash
bash scripts/make_iso.sh
```

**Processus :**
1. Crée `build/iso/boot/grub/`
2. Copie `kernel.bin` → `build/iso/boot/`
3. Copie `grub.cfg` → `build/iso/boot/grub/`
4. Exécute `grub-mkrescue -o build/exo_os.iso build/iso/`

### 4. **Test QEMU**
```bash
bash scripts/test_qemu.sh
```

**Commande équivalente :**
```bash
qemu-system-x86_64 \
    -cdrom build/exo_os.iso \
    -m 128M \
    -nographic \
    -serial mon:stdio \
    -no-reboot \
    -no-shutdown
```

**Notes :**
- `-nographic` : Pas d'interface graphique (obligatoire en devcontainer)
- `-serial mon:stdio` : Sortie série sur stdout
- `-m 128M` : 128 MB de RAM
- Pour quitter : `Ctrl+A` puis `X`

## 🚀 Workflow Complet

### Build et Test en Une Commande
```bash
# 1. Compiler
make build

# 2. Relinker (si nécessaire)
bash scripts/relink_kernel.sh

# 3. Créer ISO
bash scripts/make_iso.sh

# 4. Lancer QEMU
bash scripts/test_qemu.sh
```

### Workflow Automatisé
```bash
bash scripts/build_and_test.sh
```

## 🔍 Débogage

### Vérifier les Fichiers de Build
```bash
ls -lh build/
# kernel.elf (5.7M) - Kernel ELF linké
# kernel.bin (5.7M) - Format binaire GRUB
# libexo_kernel.a (45M) - Bibliothèque Rust
# libboot_combined.a (12K) - Boot objects
# exo_os.iso (11M) - ISO bootable
```

### Vérifier les Symboles
```bash
nm build/kernel.elf | grep rust_main
objdump -d build/kernel.elf | less
```

### Serial Output
Toute la sortie du kernel apparaît sur le port série, capturée par QEMU et affichée sur stdout.

## 📝 Linker Script (linker.ld)

**Caractéristiques :**
- **Entry Point** : `_start`
- **Base Address** : `1MB` (0x100000) - Standard Multiboot
- **Sections** :
  - `.boot` : Multiboot header + code de boot
  - `.text` : Code exécutable
  - `.rodata` : Données en lecture seule
  - `.data` : Données initialisées
  - `.bss` : Données non-initialisées
- **Alignment** : 4K pages

## 🐛 Problèmes Connus

### 1. Linkage de Symboles C
**Problème :** Le kernel Rust référence des fonctions C (`serial_puts`, `vga_putc`, etc.) qui doivent être fournies par le code boot C.

**Solution :** Compiler le code C boot complet avec :
```bash
gcc -m64 -ffreestanding -fno-pic -mno-red-zone -mcmodel=kernel \
    -nostdlib -c bootloader/*.c -o build/*.o
```

### 2. VFS Non Initialisé
**Problème :** Le shell utilise le VFS qui doit être initialisé au boot.

**Solution :** Appeler `vfs::init()` dans `rust_main()` après l'initialisation du heap.

### 3. QEMU GTK Error
**Problème :** `gtk initialization failed` dans devcontainer

**Solution :** Toujours utiliser `-nographic` et `-display none`

## 📊 Statistiques

- **Kernel Rust** : ~45MB (avec symboles debug)
- **Kernel Final** : ~5.7MB (ELF)
- **ISO** : ~11MB (avec GRUB)
- **Temps de Build** : ~25s (debug), ~40s (release)
- **Warnings** : 194 (non-bloquants)

## ✅ Tests Validés

- ✅ Boot GRUB multiboot2
- ✅ Initialisation mémoire (frame allocator + heap)
- ✅ GDT/IDT/PIC configuration
- ✅ Scheduler préemptif (3-queue EMA)
- ✅ Threads de test (A/B/C) tournent correctement
- ⚠️ Shell v0.5.0 intégré mais nécessite linkage complet

## 🎯 Prochaines Étapes

1. **Résoudre le linkage complet** avec toutes les fonctions C
2. **Initialiser le VFS** dans le boot path
3. **Tester le shell** interactif
4. **Valider les 14 commandes** du shell
5. **Créer des tests automatisés** pour le VFS

## 📚 Références

- **Multiboot2 Spec** : https://www.gnu.org/software/grub/manual/multiboot2/
- **GRUB Manual** : https://www.gnu.org/software/grub/manual/
- **QEMU Documentation** : https://www.qemu.org/docs/master/
- **OSDev Wiki** : https://wiki.osdev.org/

---

**Version** : 0.5.0  
**Date** : 2024-12-03  
**Status** : Build process documenté, shell en cours d'intégration
