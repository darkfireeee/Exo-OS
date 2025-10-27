# Bootloader Multiboot2 pour Exo-OS

Ce dossier contient le bootloader personnalisé utilisant le protocole **Multiboot2**.

## Structure

- `multiboot2_header.asm` - Header Multiboot2 (doit être dans les 8 premiers KB)
- `boot.asm` - Code de démarrage en mode protégé/long mode
- `grub.cfg` - Configuration GRUB
- `linker.ld` - Script de liaison pour le bootloader + kernel

## Protocole Multiboot2

Le header Multiboot2 a la structure suivante:

```
+-------------------+
| Magic (0xE85250D6)|  4 bytes
+-------------------+
| Architecture (0)  |  4 bytes (0 = i386, 4 = MIPS)
+-------------------+
| Header Length     |  4 bytes
+-------------------+
| Checksum          |  4 bytes
+-------------------+
| Tags...           |  Variable
+-------------------+
| End Tag (0,0,8)   |  8 bytes
+-------------------+
```

## Processus de boot

1. **GRUB charge le kernel** à l'adresse `1 MB` (0x100000)
2. **GRUB vérifie le header Multiboot2** dans les 8 premiers KB
3. **GRUB passe en mode protégé** (32-bit)
4. **GRUB jump à `_start`** avec:
   - `EAX` = 0x36d76289 (magic Multiboot2)
   - `EBX` = adresse physique de la structure d'informations

5. **Le bootloader (`boot.asm`)** :
   - Configure une GDT basique
   - Active le mode 64-bit (long mode)
   - Configure la pagination
   - Setup la stack
   - Jump au `kernel_main` Rust

6. **Le kernel Rust prend le contrôle**

## Compilation

Le bootloader est assemblé avec NASM:

```bash
nasm -f elf64 multiboot2_header.asm -o multiboot2_header.o
nasm -f elf64 boot.asm -o boot.o
```

Puis lié avec le kernel:

```bash
ld -n -T linker.ld -o kernel.bin \
   multiboot2_header.o \
   boot.o \
   kernel.o
```

## Vérification

Pour vérifier que le kernel a un header Multiboot2 valide:

```bash
grub-file --is-x86-multiboot2 kernel.bin
```

## Références

- [Multiboot2 Specification](https://www.gnu.org/software/grub/manual/multiboot2/multiboot.html)
- [OSDev Wiki - Multiboot](https://wiki.osdev.org/Multiboot)
- [Intel 64 and IA-32 Architectures Software Developer's Manual](https://www.intel.com/content/www/us/en/architecture-and-technology/64-ia-32-architectures-software-developer-manual-325462.html)
