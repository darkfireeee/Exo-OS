# Exo-OS Bootloader Configuration

## GRUB2 Multiboot2 Configuration

The bootloader uses GRUB2 with Multiboot2 specification.

### Files:
- `grub.cfg` - GRUB menu configuration
- `README.md` - This file

### Multiboot2 Header

The kernel includes a Multiboot2 header in `kernel/src/arch/x86_64/boot/boot.asm`:

```asm
section .multiboot_header
align 8
multiboot_header_start:
    dd 0xe85250d6                ; Magic number
    dd 0                         ; Architecture (i386)
    dd multiboot_header_end - multiboot_header_start
    dd -(0xe85250d6 + 0 + (multiboot_header_end - multiboot_header_start))
    
    ; End tag
    dw 0
    dw 0
    dd 8
multiboot_header_end:
```

### Boot Process:

1. **BIOS/UEFI** loads GRUB2 from ISO
2. **GRUB2** reads grub.cfg and displays menu
3. **User** selects boot option
4. **GRUB2** loads kernel at 1MB physical address
5. **GRUB2** sets up multiboot2 info structure
6. **GRUB2** jumps to kernel entry point (`boot.asm:_start`)
7. **Kernel** initializes in 32-bit protected mode
8. **Kernel** transitions to 64-bit long mode
9. **Kernel** calls `kernel_main()` in boot.c
10. **Kernel** jumps to Rust `_start()` in main.rs

### Memory Layout at Boot:

```
0x00000000 - 0x000FFFFF : Real Mode IVT, BIOS data
0x00100000 - 0x00200000 : Kernel code (loaded by GRUB)
0x00200000 - 0x00300000 : Kernel data & BSS
0x00300000 - ...        : Heap & dynamic allocation
```

### Creating Bootable ISO:

```bash
./build.sh              # Builds kernel + creates ISO
./scripts/make_iso.sh   # ISO creation only
```

### Testing:

```bash
# Linux/WSL:
./scripts/test_qemu.sh

# Windows PowerShell:
.\scripts\test_qemu.ps1

# Manual QEMU:
qemu-system-x86_64 -cdrom build/exo_os.iso -m 512M
```
