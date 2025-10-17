/* linker.ld - Script de l'éditeur de liens pour Exo-OS Kernel */

/* Point d'entrée du noyau (défini dans boot.asm ou boot.c) */
ENTRY(_start)

SECTIONS
{
    /* Le noyau est chargé à l'adresse 1 MiB par convention */
    . = 1M;

    /* Début de la section kernel */
    _kernel_start = .;

    /* Section de code (texte) */
    .text ALIGN(4K) : {
        *(.multiboot_header)  /* Header multiboot doit être au début */
        *(.text .text.*)
        *(.rodata .rodata.*)
        . = ALIGN(4K);
    }

    /* Section de données initialisées */
    .data ALIGN(4K) : {
        *(.data .data.*)
        . = ALIGN(4K);
    }

    /* Section BSS (données non initialisées) */
    .bss ALIGN(4K) : {
        _bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(4K);
        _bss_end = .;
    }

    /* Section pour les tables de pages (si nécessaire) */
    .page_tables ALIGN(4K) : {
        _page_tables_start = .;
        . += 4K * 512;  /* Réserver de l'espace pour les tables de pages */
        _page_tables_end = .;
    }

    /* Section pour le stack du noyau */
    .stack ALIGN(4K) : {
        _stack_bottom = .;
        . += 16K;  /* Stack de 16 KiB */
        _stack_top = .;
    }

    /* Marqueur de fin du noyau */
    _kernel_end = .;

    /* Supprimer les sections inutiles */
    /DISCARD/ : {
        *(.eh_frame)
        *(.eh_frame_hdr)
        *(.comment)
        *(.note .note.*)
        *(.gcc_except_table)
        *(.gnu.hash)
    }
}
