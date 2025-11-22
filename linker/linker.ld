/* Linker Script for Exo-OS Kernel */
/* Charge le kernel à 1MB (0x100000) comme requis par Multiboot */

ENTRY(_start)

SECTIONS
{
    /* Kernel chargé à 1MB - standard Multiboot */
    . = 1M;
    
    _kernel_start = .;
    
    /* Header Multiboot DOIT être dans les premiers 8KB du binaire */
    .multiboot ALIGN(4K) : {
        KEEP(*(.multiboot))
        KEEP(*(.multiboot_header))
    }
    
    /* Code executable */
    .text ALIGN(4K) : {
        *(.text)
        *(.text.*)
    }
    
    /* Données read-only (constantes, strings, etc.) */
    .rodata ALIGN(4K) : {
        *(.rodata)
        *(.rodata.*)
    }
    
    /* Données initialisées */
    .data ALIGN(4K) : {
        *(.data)
        *(.data.*)
    }
    
    /* Données non-initialisées (zéro au démarrage) */
    .bss ALIGN(4K) : {
        *(COMMON)
        *(.bss)
        *(.bss.*)
    }
    
    _kernel_end = .;
    
    /* Supprimer les sections debug et info non nécessaires */
    /DISCARD/ : {
        *(.comment)
        *(.eh_frame)
        *(.eh_frame_hdr)
        *(.note*)
        *(.gcc_except_table)
    }
}
