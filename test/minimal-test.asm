; minimal-test.asm
; Kernel minimal pour tester GRUB

BITS 32

; Multiboot2 header
section .multiboot
align 8
header_start:
    dd 0xE85250D6                ; magic
    dd 0                         ; architecture (i386)
    dd header_end - header_start ; header length
    dd -(0xE85250D6 + 0 + (header_end - header_start)) ; checksum
    
    ; End tag
    dw 0    ; type
    dw 0    ; flags
    dd 8    ; size
header_end:

section .text
global _start
_start:
    ; Écrire '!' en blanc sur fond rouge à l'écran
    mov dword [0xB8000], 0x4F214F21  ; '!!'
    mov dword [0xB8004], 0x2F542F45  ; 'ET' en vert
    mov dword [0xB8008], 0x1F531F54  ; 'TS' en bleu
    
    ; Loop infini
    cli
.hang:
    hlt
    jmp .hang
