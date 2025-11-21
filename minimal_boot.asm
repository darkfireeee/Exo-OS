; Minimal bootloader pour Exo-OS
; Affiche "Exo-OS Booting..." et attend

[BITS 16]
[ORG 0x7C00]

start:
    ; Initialiser les segments
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7C00

    ; Afficher le message
    mov si, msg
    call print_string

    ; Boucle infinie
    cli
    hlt
    jmp $

print_string:
    lodsb
    or al, al
    jz .done
    
    ; Afficher sur VGA
    mov ah, 0x0E
    mov bh, 0x00
    mov bl, 0x07
    int 0x10
    
    ; Afficher sur port série (COM1)
    push dx
    push ax
    mov dx, 0x3F8  ; COM1
    pop ax
    out dx, al
    pop dx
    
    jmp print_string
.done:
    ret

msg db 'Exo-OS Kernel Booting...', 13, 10
    db 'Boot successful!', 13, 10, 0

; Remplir jusqu'à 510 octets
times 510-($-$$) db 0

; Signature de boot
dw 0xAA55
