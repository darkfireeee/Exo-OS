/* Exo-OS Kernel Linker Script */
/* Target: x86_64 bare-metal (multiboot) */

ENTRY(_start)

SECTIONS
{
    /* Le kernel commence à 1 MB (adresse physique standard multiboot) */
    . = 1M;

    .boot :
    {
        /* Boot code assembleur en premier */
        *(.multiboot)
        *(.text.boot)
    }

    /* Section text alignée sur page */
    . = ALIGN(4K);
    .text :
    {
        *(.text .text.*)
    }

    /* Section read-only data */
    . = ALIGN(4K);
    .rodata :
    {
        *(.rodata .rodata.*)
    }

    /* Section data */
    . = ALIGN(4K);
    .data :
    {
        *(.data .data.*)
    }

    /* Section BSS */
    . = ALIGN(4K);
    .bss :
    {
        *(COMMON)
        *(.bss .bss.*)
    }

    /* Stack guard page */
    . = ALIGN(4K);
    __bss_end = .;
    __kernel_end = .;
}
