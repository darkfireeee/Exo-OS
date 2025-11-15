# boot_minimal.S
# Point d'entrée minimal pour boot QEMU/Multiboot2
# Architecture: x86_64
# Auteur: EXO-OS Project
# Date: 12 Novembre 2025

.section .multiboot
.align 8

# Multiboot2 header
multiboot2_header_start:
    .long 0xe85250d6                    # Magic number (multiboot2)
    .long 0                             # Architecture: i386 (protected mode)
    .long multiboot2_header_end - multiboot2_header_start  # Header length
    .long -(0xe85250d6 + 0 + (multiboot2_header_end - multiboot2_header_start))  # Checksum

    # End tag
    .align 8
    .word 0    # type
    .word 0    # flags
    .long 8    # size
multiboot2_header_end:

.section .bss
.align 16
stack_bottom:
    .skip 16384  # 16 KB stack
stack_top:

.section .text
.global _start
.type _start, @function

_start:
    # Désactiver les interruptions
    cli

    # Configurer la pile
    movq $stack_top, %rsp
    movq $stack_top, %rbp

    # Effacer les flags
    pushq $0
    popfq

    # Vérifier si on est en mode 64-bit (cpuid)
    # Si pas en 64-bit, on halt
    movl $0x80000000, %eax
    cpuid
    cmpl $0x80000001, %eax
    jb no_long_mode
    
    movl $0x80000001, %eax
    cpuid
    testl $(1 << 29), %edx  # Test LM bit
    jz no_long_mode

    # Sauvegarder le pointeur Multiboot2 (dans RDI)
    # RDI contient déjà l'adresse des infos Multiboot2 passées par GRUB
    # C'est le premier argument pour _kernel_start selon ABI x86_64

    # Appeler _kernel_start (Rust)
    # _kernel_start doit être: extern "C" fn _kernel_start(multiboot_info_addr: usize) -> !
    call _kernel_start

    # Si _kernel_start retourne (ne devrait pas arriver)
    cli
halt_loop:
    hlt
    jmp halt_loop

no_long_mode:
    # Afficher 'E' en rouge en haut à gauche (mode VGA texte)
    movw $0x4F45, 0xb8000
    cli
    hlt
    jmp no_long_mode

.size _start, . - _start
