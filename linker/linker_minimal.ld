/* linker_minimal.ld - Linker script MINIMAL pour debug */

ENTRY(_start)

SECTIONS
{
    . = 1M;
    
    .multiboot ALIGN(4K) : {
        KEEP(*(.multiboot))
    }
    
    .text ALIGN(4K) : {
        *(.text)
        *(.text.*)
    }
    
    .rodata ALIGN(4K) : {
        *(.rodata)
        *(.rodata.*)
    }
    
    .data ALIGN(4K) : {
        *(.data)
        *(.data.*)
    }
    
    .bss ALIGN(4K) : {
        *(COMMON)
        *(.bss)
        *(.bss.*)
    }
    
    /DISCARD/ : {
        *(.comment)
        *(.eh_frame*)
        *(.note*)
    }
}
