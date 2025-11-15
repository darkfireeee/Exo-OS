/* linker.ld - Script de l'éditeur de liens pour Exo-OS Kernel */

OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)

ENTRY(_start)

SECTIONS {
    . = 0x100000;  /* 1 MiB */

    kernel_phys_start = .;

    /* Section boot avec Multiboot2 header et code de démarrage */
    .boot : ALIGN(4096) {
        KEEP(*(.multiboot_header))
        KEEP(*(.multiboot))
    }

    /* Pile et tables de pages de boot.asm */
    .bss.boot : ALIGN(4096) {
        *(.bss)
    }

    /* Code exécutable */
    .text : ALIGN(4096) {
        *(.text._start)
        *(.text .text.*)
    }

    /* Données en lecture seule */
    .rodata : ALIGN(4096) {
        *(.rodata .rodata.*)
    }

    /* Données modifiables */
    .data : ALIGN(4096) {
        *(.data .data.*)
    }

    kernel_phys_end = .;

    /DISCARD/ : {
        *(.eh_frame)
        *(.eh_frame_hdr)
        *(.comment)
        *(.note .note.*)
        *(.gcc_except_table)
        *(.gnu.hash)
    }
}
