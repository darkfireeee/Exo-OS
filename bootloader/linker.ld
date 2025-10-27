/* Linker script pour Exo-OS avec bootloader multiboot2 */

ENTRY(_start)

SECTIONS
{
    /* Le bootloader multiboot2 doit être aligné à 4KB */
    . = 1M;

    /* Section du header multiboot2 - DOIT être dans les premiers 8KB */
    .multiboot_header : ALIGN(4K)
    {
        KEEP(*(.multiboot_header))
    }

    /* Code du bootloader */
    .boot : ALIGN(4K)
    {
        *(.boot)
    }

    /* Section de texte (code) */
    .text : ALIGN(4K)
    {
        *(.text .text.*)
    }

    /* Section read-only data */
    .rodata : ALIGN(4K)
    {
        *(.rodata .rodata.*)
    }

    /* Section data initialisée */
    .data : ALIGN(4K)
    {
        *(.data .data.*)
    }

    /* Section BSS (données non-initialisées) */
    .bss : ALIGN(4K)
    {
        *(COMMON)
        *(.bss .bss.*)
    }

    /* Stack du bootloader */
    .stack (NOLOAD) : ALIGN(16)
    {
        . = . + 16K;
        stack_top = .;
    }

    /* Informations de débogage (ne seront pas chargées en mémoire) */
    /DISCARD/ :
    {
        *(.comment)
        *(.eh_frame)
        *(.note.GNU-stack)
    }
}
