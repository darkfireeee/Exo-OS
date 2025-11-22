; boot_minimal.asm - Test ultra-simple pour debug
; Juste écrire sur VGA sans aucune table de paging ni transition 64-bit

BITS 32

section .multiboot
align 4
    dd 0x1BADB002                   ; Magic
    dd 0x00000000                   ; Flags
    dd -(0x1BADB002)                ; Checksum

section .bss
align 16
stack_bottom:
    resb 16384
stack_top:

section .text
global _start

_start:
    cli
    
    ; Setup stack
    mov esp, stack_top
    mov ebp, esp
    
    ; Écrire "HELLO" en VGA
    mov edi, 0xB8000
    mov eax, 0x0F480F48         ; "HH" blanc sur noir
    mov [edi], eax
    mov eax, 0x0F4C0F45         ; "EL"
    mov [edi + 4], eax
    mov eax, 0x0F4F0F4C         ; "LO"
    mov [edi + 8], eax
    
.hang:
    hlt
    jmp .hang
