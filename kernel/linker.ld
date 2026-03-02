/* =============================================================================
 * kernel/linker.ld — Script linker Exo-OS (x86_64 bare-metal, Multiboot2)
 * =============================================================================
 *
 * Adresse physique de chargement : 0x100000 (1 MiB — standard Multiboot2).
 * Le kernel est compilé avec code-model=kernel + relocation-model=static.
 *
 * Organisation des sections (ordre = ordre dans l'image ELF) :
 *   .multiboot2   → header Multiboot2 (DOIT être dans les 32 premiers KiB)
 *   .text.boot    → _start + début de l'exécution
 *   .text         → code kernel complet
 *   .rodata       → données lecture seule (code_model=kernel : addresses hautes)
 *   .data         → données initialisées
 *   .tdata/.tbss  → données thread-local (TLS) — kernel kthread
 *   .bss          → données zéro-initialisées
 *   .boot_stack   → pile de boot BSP (NOLOAD : absent de l'image)
 * =============================================================================
 */

ENTRY(_start)

SECTIONS
{
    /* ── Adresse de chargement physique ─────────────────────────────────────── */
    . = 0x100000;

    /* ── Header Multiboot2 ─────────────────────────────────────────────────── */
    /* RÈGLE : doit figurer dans les 32 premiers KiB de l'image (GRUB exigence) */
    .multiboot2 : {
        KEEP(*(.multiboot2))
    }

    /* ── Code d'entrée boot ─────────────────────────────────────────────────── */
    .text.boot : {
        KEEP(*(.text.boot))
    }

    /* ── Section code principale ────────────────────────────────────────────── */
    .text : ALIGN(16) {
        _text_start = .;
        *(.text .text.*)
        _text_end = .;
    }

    /* ── Données lecture seule ──────────────────────────────────────────────── */
    .rodata : ALIGN(4096) {
        _rodata_start = .;
        *(.rodata .rodata.*)
        _rodata_end = .;
    }

    /* ── Données initialisées ────────────────────────────────────────────────── */
    .data : ALIGN(4096) {
        _data_start = .;
        *(.data .data.*)
        _data_end = .;
    }

    /* ── Thread-local storage (kernel kthreads) ─────────────────────────────── */
    .tdata : ALIGN(4096) {
        _tdata_start = .;
        *(.tdata .tdata.*)
        _tdata_end = .;
    }

    /* ── BSS (zéro-initialisé par le kernel ou le bootloader) ──────────────── */
    .bss : ALIGN(4096) {
        _bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        _bss_end = .;
    }

    /* ── Pile de boot BSP (NOLOAD — absent de l'image) ──────────────────────── */
    /* Le contenu est défini par global_asm! dans main.rs (type @nobits).        */
    /* Le linker réserve l'espace mémoire mais ne l'écrit PAS dans l'image ELF. */
    .boot_stack (NOLOAD) : ALIGN(4096) {
        *(.boot_stack)
    }

    /* ── Sections à éliminer ────────────────────────────────────────────────── */
    /DISCARD/ : {
        *(.eh_frame)
        *(.eh_frame_hdr)
        *(.note.GNU-stack)
        *(.gcc_except_table)
        *(.comment)
    }
}
