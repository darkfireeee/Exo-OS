; Boot Entry Point for Exo-OS Kernel
; Sets up stack and calls C boot_main

BITS 32

section .text
global _boot_start
extern boot_main
extern _stack_top

_boot_start:
    ; At this point:
    ; EAX = multiboot2 magic (0x36d76289)
    ; EBX = physical address of multiboot info structure
    
    ; Disable interrupts
    cli
    
    ; Set up stack (grows downward)
    mov esp, _stack_top
    mov ebp, esp
    
    ; Push multiboot info (arg2)
    push ebx
    ; Push magic (arg1)
    push eax
    
    ; Call C entry point
    call boot_main
    
    ; Should never return, but halt just in case
.hang:
    hlt
    jmp .hang
