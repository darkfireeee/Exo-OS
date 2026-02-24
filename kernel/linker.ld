/* ============================================================================
 * Exo-OS Kernel — Script de linker x86_64
 *
 * Adresse de chargement physique : 1 MiB (standard Multiboot2 / GRUB).
 * Le kernel est un exécutable statique non-PIE (code-model=kernel, static).
 *
 * KERNEL_BASE (0xFFFF_FFFF_8000_0000) est un constant virtuel utilisé par le
 * kernel pour configurer son propre espace d'adressage APRÈS le boot ;
 * il n'est PAS l'adresse de linkage. Le kernel s'exécute en adressage physique
 * jusqu'à ce que le sous-système mémoire configure ses tables de pages finales.
 * ============================================================================ */

ENTRY(_start)

SECTIONS
{
    /* ── Adresse de chargement physique ─────────────────────────────────── */
    . = 1M;

    /* ── Header Multiboot2 (DOIT être dans les 32 premiers KiB après 1M) ─ */
    .multiboot2 : ALIGN(8)
    {
        KEEP(*(.multiboot2))
        KEEP(*(.multiboot))
    }

    /* ── Code de démarrage (ASM _start → kernel_main) ─────────────────── */
    .text.boot : ALIGN(4K)
    {
        KEEP(*(.text.boot))
    }

    /* ── Trampoline SMP (copié à 0x6000 par install_trampoline) ────────── */
    . = ALIGN(4K);
    __trampoline_start = .;
    .trampoline : ALIGN(4K)
    {
        KEEP(*(.trampoline))
    }
    __trampoline_end = .;

    /* ── Code exécutable kernel ──────────────────────────────────────────── */
    . = ALIGN(4K);
    __text_start = .;
    .text : ALIGN(4K)
    {
        *(.text .text.*)
    }
    __text_end = .;

    /* ── Données en lecture seule ───────────────────────────────────────── */
    . = ALIGN(4K);
    __rodata_start = .;
    .rodata : ALIGN(4K)
    {
        *(.rodata .rodata.*)
    }
    __rodata_end = .;

    /* ── Données initialisées ───────────────────────────────────────────── */
    . = ALIGN(4K);
    __data_start = .;
    .data : ALIGN(4K)
    {
        *(.data .data.*)
    }
    __data_end = .;

    /* ── Pile de boot BSP (64 KiB, avant .bss pour garantir l'ordre) ────── */
    . = ALIGN(4K);
    .boot_stack : ALIGN(4K)
    {
        _exo_boot_stack_bottom = .;
        KEEP(*(.boot_stack .boot_stack.*))
        . = ALIGN(4K);
        _exo_boot_stack_top = .;
    }

    /* ── Données non initialisées ───────────────────────────────────────── */
    . = ALIGN(4K);
    __bss_start = .;
    .bss (NOLOAD) : ALIGN(4K)
    {
        *(COMMON)
        *(.bss .bss.*)
    }
    . = ALIGN(4K);
    __bss_end = .;

    /* ── Fin du kernel ──────────────────────────────────────────────────── */
    . = ALIGN(4K);
    __kernel_end = .;

    /* ── Sections de débogage et métadonnées (éliminées) ──────────────── */
    /DISCARD/ :
    {
        *(.comment)
        *(.note .note.*)
        *(.eh_frame .eh_frame_hdr)
        *(.gnu_debuglink)
    }
}
