# 🚀 Guide de Compilation et Test d'Exo-OS

Ce guide vous montre comment compiler et tester Exo-OS avec le bootloader Multiboot2 custom et GRUB.

## 📋 Prérequis

### Sur Windows avec WSL

1. **Installer WSL2 avec Ubuntu** (si pas déjà fait) :
   ```powershell
   wsl --install -d Ubuntu
   ```

2. **Lancer WSL et installer les dépendances** :
   ```bash
   cd /mnt/c/Users/Eric/Documents/Exo-OS
   ./scripts/setup-wsl.sh
   ```

   Ceci installera :
   - Rust nightly
   - NASM (assembleur)
   - GRUB et outils (xorriso, mtools)
   - QEMU (émulateur)

## 🔨 Compilation

### Méthode rapide (Build complet)

Depuis WSL :

```bash
cd /mnt/c/Users/Eric/Documents/Exo-OS
./scripts/build-all.sh
```

Ce script effectue toutes les étapes automatiquement :
1. ✅ Compile le kernel Rust
2. ✅ Assemble le bootloader
3. ✅ Lie le kernel et le bootloader
4. ✅ Vérifie le header Multiboot2
5. ✅ Crée l'image ISO bootable

### Méthode manuelle (Étape par étape)

#### 1. Compiler le kernel Rust

```bash
cd kernel
cargo build --release --target ../x86_64-exo-os.json -Z build-std=core,alloc,compiler_builtins
cd ..
```

Résultat : `kernel/target/x86_64-exo-os/release/libexo_kernel.a`

#### 2. Assembler le bootloader

```bash
nasm -f elf64 bootloader/multiboot2_header.asm -o build/multiboot2_header.o
nasm -f elf64 bootloader/boot.asm -o build/boot.o
```

Résultats : 
- `build/multiboot2_header.o`
- `build/boot.o`

#### 3. Lier le kernel avec le bootloader

```bash
ld -n -T bootloader/linker.ld \
   -o build/kernel.bin \
   build/multiboot2_header.o \
   build/boot.o \
   kernel/target/x86_64-exo-os/release/libexo_kernel.a
```

Résultat : `build/kernel.bin` (kernel bootable)

#### 4. Vérifier le header Multiboot2

```bash
grub-file --is-x86-multiboot2 build/kernel.bin
echo $?  # Doit retourner 0 si valide
```

#### 5. Créer l'image ISO

```bash
mkdir -p isodir/boot/grub
cp build/kernel.bin isodir/boot/
cp bootloader/grub.cfg isodir/boot/grub/
grub-mkrescue -o exo-os.iso isodir
```

Résultat : `exo-os.iso` (image ISO bootable)

## 🎮 Test et Exécution

### Avec QEMU (depuis WSL)

```bash
./scripts/run-qemu.sh
```

Options QEMU utilisées :
- `-cdrom exo-os.iso` : Boot depuis l'ISO
- `-serial stdio` : Sortie série sur le terminal
- `-m 128M` : 128 MB de RAM
- `-cpu qemu64` : CPU x86_64 émulé
- `-no-reboot` : Ne pas rebooter en cas de triple-fault
- `-no-shutdown` : Ne pas fermer en cas de shutdown
- `-d int,cpu_reset` : Logs de debug

### Avec QEMU (depuis Windows PowerShell)

```powershell
.\scripts\run-qemu.ps1
```

### Avec VirtualBox/VMWare

1. Créer une nouvelle VM :
   - Type : **Other/Unknown (64-bit)**
   - RAM : **128 MB minimum**
   - Pas de disque dur nécessaire

2. Configurer le boot :
   - Monter `exo-os.iso` comme CD-ROM
   - Boot order : CD-ROM en premier

3. Activer le port série (optionnel) :
   - Settings → Serial Ports → Enable Serial Port
   - Port Number : COM1
   - Port Mode : Raw File ou Host Pipe

4. Démarrer la VM

### Avec du matériel réel (USB bootable)

⚠️ **ATTENTION : Ceci va effacer le contenu de la clé USB !**

```bash
# Identifier votre clé USB (ex: /dev/sdb)
lsblk

# Écrire l'ISO sur la clé (REMPLACER /dev/sdX par votre clé)
sudo dd if=exo-os.iso of=/dev/sdX bs=4M status=progress && sync
```

Puis booter depuis la clé USB.

## 🐛 Débogage

### Activer les logs de debug dans QEMU

```bash
qemu-system-x86_64 \
    -cdrom exo-os.iso \
    -serial stdio \
    -d int,cpu_reset,guest_errors \
    -D qemu-debug.log
```

Les logs seront écrits dans `qemu-debug.log`.

### Vérifier les symboles du kernel

```bash
nm build/kernel.bin | grep kernel_main
# Doit montrer l'adresse de kernel_main
```

### Examiner les sections du kernel

```bash
readelf -S build/kernel.bin
```

### Afficher le contenu du header Multiboot2

```bash
xxd -l 64 build/kernel.bin
```

Les 4 premiers bytes doivent être : `D6 50 25 E8` (magic Multiboot2 en little-endian)

## 🧹 Nettoyage

Pour nettoyer les fichiers de build :

```bash
./scripts/clean.sh
```

Ceci supprime :
- `kernel/target/`
- `build/`
- `isodir/`
- `exo-os.iso`

## 📊 Structure des fichiers générés

```
Exo-OS/
├── build/
│   ├── multiboot2_header.o   # Header Multiboot2 assemblé
│   ├── boot.o                 # Bootloader assemblé
│   └── kernel.bin             # Kernel final lié (exécutable ELF)
├── isodir/
│   └── boot/
│       ├── kernel.bin         # Copie du kernel
│       └── grub/
│           └── grub.cfg       # Config GRUB
└── exo-os.iso                 # Image ISO bootable
```

## 🔍 Sortie attendue

Au démarrage, vous devriez voir dans la sortie série :

```
===========================================
  Exo-OS Kernel v0.1.0
  Architecture: x86_64
  Bootloader: Multiboot2 + GRUB
===========================================
[BOOT] Multiboot2 magic validé: 0x36d76289
[BOOT] Multiboot info @ 0x...

[MEMORY] Carte mémoire:
  0x0000000000000000 - 0x000000000009fc00 (0 MB) [Disponible]
  0x0000000000100000 - 0x0000000007fe0000 (126 MB) [Disponible]

  2 régions mémoire utilisables
  Mémoire utilisable totale: 127 MB

[BOOT] Bootloader: GRUB 2.06

[INIT] Architecture x86_64...
[INIT] Gestionnaire de mémoire...
[INIT] Ordonnanceur...
[INIT] IPC...
[INIT] Appels système...
[INIT] Pilotes...

[SUCCESS] Noyau initialisé avec succès!

[KERNEL] Entrant dans la boucle principale...
```

## ❓ Problèmes fréquents

### Erreur : "grub-mkrescue: command not found"

```bash
sudo apt install grub-common grub-pc-bin xorriso mtools
```

### Erreur : "nasm: command not found"

```bash
sudo apt install nasm
```

### Erreur : "Target x86_64-exo-os.json not found"

Assurez-vous d'être dans le bon répertoire :
```bash
cd /mnt/c/Users/Eric/Documents/Exo-OS
```

### Le kernel ne boot pas (triple fault)

1. Vérifiez le header Multiboot2 :
   ```bash
   grub-file --is-x86-multiboot2 build/kernel.bin
   ```

2. Vérifiez les logs QEMU :
   ```bash
   qemu-system-x86_64 -cdrom exo-os.iso -d int,cpu_reset -D debug.log
   cat debug.log
   ```

3. Vérifiez que `_start` est bien défini :
   ```bash
   nm build/kernel.bin | grep _start
   ```

### Erreur : "can't find crate for `core`"

Installez rust-src :
```bash
rustup component add rust-src
```

## 📚 Références

- [Multiboot2 Specification](https://www.gnu.org/software/grub/manual/multiboot2/multiboot.html)
- [OSDev Wiki](https://wiki.osdev.org/)
- [Intel 64 Manual](https://www.intel.com/content/www/us/en/architecture-and-technology/64-ia-32-architectures-software-developer-manual-325462.html)
- [Writing an OS in Rust](https://os.phil-opp.com/)

## 🎯 Prochaines étapes

Une fois que le kernel boot correctement :

1. ✅ Implémenter la pagination avec les infos Multiboot2
2. ✅ Initialiser le heap allocator
3. ✅ Configurer l'IDT (Interrupt Descriptor Table)
4. ✅ Tester les interruptions (timer, clavier)
5. ✅ Implémenter l'ordonnanceur
6. ✅ Créer les premiers processus utilisateur

Bon développement ! 🚀
