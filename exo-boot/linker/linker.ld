/* Exo-OS Kernel Linker Script */
/* Target: x86_64 bare-metal (multiboot2 + UEFI) */

ENTRY(_start)

SECTIONS
{
    /* Le kernel commence à 1 MB (adresse physique standard multiboot) */
    . = 1M;

    /* ── Multiboot2 header (doit être dans les 32 premiers KiB) ── */
    .boot :
    {
        KEEP(*(.multiboot2))
        KEEP(*(.multiboot))
        KEEP(*(.text.boot))
    }

    /* ── Trampoline SMP : copié à 0x6000 pour les APs ── */
    . = ALIGN(4K);
    .trampoline ALIGN(4K) :
    {
        __trampoline_start = .;
        KEEP(*(.trampoline))
        __trampoline_end = .;
    }

    /* ── Section texte executable ── */
    . = ALIGN(4K);
    __text_start = .;
    _text_start = .;
    .text :
    {
        *(.text .text.*)
    }
    __text_end = .;
    _text_end = .;

    /* ── Données en lecture seule ── */
    . = ALIGN(4K);
    __rodata_start = .;
    _rodata_start = .;
    .rodata :
    {
        *(.rodata .rodata.*)
    }
    __rodata_end = .;
    _rodata_end = .;

    /* Payloads Ring1 embarques: charges en memoire mais exclus du hash .rodata. */
    . = ALIGN(4K);
    __boot_payloads_start = .;
    .boot_payloads :
    {
        KEEP(*(.boot_payloads .boot_payloads.*))
    }
    __boot_payloads_end = .;

    /* ── Données initialisées ── */
    . = ALIGN(4K);
    __data_start = .;
    _data_start = .;
    .data :
    {
        *(.got .got.*)
        *(.got.plt .got.plt.*)
        *(.data .data.*)
    }
    __data_end = .;
    _data_end = .;

    /* ── Données non initialisées ── */
    . = ALIGN(4K);
    __bss_start = .;
    _bss_start = .;
    .bss :
    {
        *(COMMON)
        *(.bss .bss.*)
    }
    __bss_end = .;
    _bss_end = .;

    /* ── Pile de boot/idle BSP — doit rester couverte par __kernel_end ── */
    . = ALIGN(4K);
    .boot_stack :
    {
        KEEP(*(.boot_stack .boot_stack.*))
    }

    /* ── Fin du kernel ── */
    . = ALIGN(4K);
    __kernel_end = .;

    /* ── Déchets ── */
    /DISCARD/ :
    {
        *(.comment)
        *(.note*)
        *(.eh_frame*)
    }
}
