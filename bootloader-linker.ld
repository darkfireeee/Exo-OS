/* bootloader-linker.ld
 * Script de linkage pour combiner bootloader et kernel
 */

OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)

ENTRY(start)  /* Point d'entrée dans boot.asm */

SECTIONS {
    /* Placer le code à 1 MiB (convention multiboot) */
    . = 1M;

    _kernel_start = .;

    /* Section multiboot2 header - DOIT être au début */
    .boot ALIGN(8) : {
        KEEP(*(.multiboot_header))
        *(.text.boot)
    }

    /* Code 32-bit du bootloader */
    .text.boot32 ALIGN(4K) : {
        *(.text)
    }

    /* Code 64-bit et kernel */
    .text ALIGN(4K) : {
        *(.text.*)
    }

    /* Données en lecture seule */
    .rodata ALIGN(4K) : {
        *(.rodata)
        *(.rodata.*)
    }

    /* Données initialisées */
    .data ALIGN(4K) : {
        *(.data)
        *(.data.*)
        *(.got)
        *(.got.plt)
    }

    /* Section BSS (non initialisée) */
    .bss ALIGN(4K) : {
        _bss_start = .;
        *(.bss)
        *(.bss.*)
        *(COMMON)
        _bss_end = .;
    }

    _kernel_end = .;

    /* Supprimer les sections inutiles */
    /DISCARD/ : {
        *(.eh_frame)
        *(.eh_frame_hdr)
        *(.comment)
        *(.note)
        *(.note.*)
        *(.gcc_except_table)
        *(.gnu.hash)
        *(.gnu.version)
        *(.gnu.version_*)
    }
}
