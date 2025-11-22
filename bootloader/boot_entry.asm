; boot_entry.asm - Point d'entrée assembly du kernel
; GRUB charge en mode 32-bit, on doit passer en 64-bit

BITS 32

section .multiboot_header
align 4
multiboot_start:
    dd 0x1BADB002               ; Multiboot1 magic
    dd 0x00000000               ; flags
    dd -(0x1BADB002 + 0x00000000) ; checksum
multiboot_end:

section .bss
align 16
stack_bottom:
    resb 16384  ; 16 KB de stack
stack_top:

section .text
global _start

_start:
    cli                         ; Désactiver les interruptions
    
    ; Configurer la stack (32-bit pour l'instant)
    mov esp, stack_top
    mov ebp, esp
    
    ; Test VGA en mode 32-bit
    mov edi, 0xB8000            ; Adresse VGA
    mov word [edi], 0x0F48      ; 'H' blanc sur noir
    mov word [edi + 2], 0x0F65  ; 'e'
    mov word [edi + 4], 0x0F6C  ; 'l'
    mov word [edi + 6], 0x0F6C  ; 'l'
    mov word [edi + 8], 0x0F6F  ; 'o'
    mov word [edi + 10], 0x0F20 ; ' '
    mov word [edi + 12], 0x0F33 ; '3'
    mov word [edi + 14], 0x0F32 ; '2'
    
.hang:
    hlt
    jmp .hang
