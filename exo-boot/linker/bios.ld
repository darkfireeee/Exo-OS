/* linker/bios.ld — Linker script BIOS MBR/GPT pour exo-boot
 *
 * Cible : x86_64-unknown-none (ou i686-unknown-none pour stage1 16-bit)
 * Linker : ld.lld (LLD en mode ELF) ou GNU ld
 *
 * Architecture du chargement BIOS :
 *
 *  [BIOS POST]
 *      │ Lit 512 octets (MBR) depuis le premier secteur du disque
 *      ↓
 *  [STAGE 1 — mbr.asm @ 0x7C00]  (max 440 bytes utiles, 16-bit)
 *      │ Configure A20, cherche stage 2 sur le disque, passe en mode protégé 32-bit
 *      ↓
 *  [STAGE 2 — stage2.asm @ 0x1000]  (16-bit → 32-bit → 64-bit)
 *      │ Configure GDT, IDT minimale, passe en mode long 64-bit
 *      │ Charge le binaire Rust de exo-boot en mémoire haute
 *      ↓
 *  [EXOBOOT 64-bit Rust @ 0x100000]  (ce fichier contrôle ce mapping)
 *      │ Setup paging identité + higher-half
 *      │ Lit la carte mémoire E820
 *      │ Charge le kernel ELF depuis le disque
 *      ↓
 *  [KERNEL @ adresse KASLR-randomisée]
 *
 * Layout mémoire physique au moment du handoff exo-boot → kernel :
 *
 *  0x000000 – 0x0003FF  : Real Mode IVT (préservé par BIOS)
 *  0x000400 – 0x0004FF  : BIOS Data Area
 *  0x000500 – 0x007BFF  : Libre (stack stage 1)
 *  0x007C00 – 0x007DFF  : MBR stage 1 (512 bytes)
 *  0x007E00 – 0x00FFFF  : Libre (utilisé par stage 2 pour buffers)
 *  0x001000 – 0x0017FF  : Stage 2 (approx 2KB)
 *  0x080000 – 0x09FFFF  : EBDA (Extended BIOS Data Area) — NE PAS ÉCRASER
 *  0x0A0000 – 0x0BFFFF  : VGA framebuffer (MMIO)
 *  0x0C0000 – 0x0FFFFF  : ROM BIOS + Option ROMs
 *  0x100000 – ...        : RAM haute — exo-boot Rust + stack + données
 */

OUTPUT_FORMAT("elf64-x86-64")
OUTPUT_ARCH(i386:x86-64)

ENTRY(_start)

/* Adresse de chargement de la partie Rust de exo-boot en mémoire haute.
 * Le stage 2 charge le binaire ELF à cette adresse avant de sauter dedans. */
LOAD_ADDRESS = 0x100000;

SECTIONS
{
    /* ── Code et données de démarrage ────────────────────────────────────── */
    . = LOAD_ADDRESS;

    .text ALIGN(4096) :
    {
        /* Section de démarrage — doit être en premier pour _start */
        KEEP(*(.text.boot))
        KEEP(*(.text.entry))
        *(.text .text.*)
    }

    .rodata ALIGN(4096) :
    {
        *(.rodata .rodata.*)
        /* Tables de polices embedded (font.rs) */
        *(.rodata.font)
        /* Clé publique Ed25519 embedded (verify.rs) */
        KEEP(*(.rodata.ed25519_pubkey))
    }

    .data ALIGN(4096) :
    {
        *(.data .data.*)
        /* Données initialisées du config parser */
        *(.data.config)
    }

    /* Section BSS — zéro-initialisée par le runtime de démarrage */
    .bss ALIGN(4096) :
    {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    }

    /* Stack initiale pour exo-boot (avant kernel) — 16 KB */
    .boot_stack ALIGN(4096) (NOLOAD) :
    {
        __stack_bottom = .;
        . += 0x4000;   /* 16 KB */
        __stack_top = .;
    }

    /* ── Symboles exportés pour le code Rust ───────────────────────────── */
    __exoboot_start  = ADDR(.text);
    __exoboot_end    = .;
    __exoboot_size   = __exoboot_end - __exoboot_start;

    /* ── Sections à supprimer ────────────────────────────────────────────── */
    /DISCARD/ :
    {
        *(.comment)
        *(.note*)
        *(.eh_frame*)
        *(.debug*)
        *(.gnu.attributes)
    }
}

/* Assertions de sécurité : le bootloader doit tenir sous 1 MB à partir de 0x100000
 * pour éviter de déborder dans la zone EBDA lors du démarrage. */
ASSERT(. < 0x200000, "exo-boot dépasse 1MB — réduire la taille ou ajuster LOAD_ADDRESS");
ASSERT(__stack_top <= 0x300000, "Stack déborde au-delà de 3MB — impossible");
