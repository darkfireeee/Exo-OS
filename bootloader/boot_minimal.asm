; boot_minimal.asm - Bootloader Exo-OS MINIMAL
; Version ultra-simple pour débugger le boot
; Multiboot1 → 32bit → 64bit → kernel_main

BITS 32

; === MULTIBOOT HEADER (DOIT être dans les premiers 8KB) ===
section .multiboot
align 4
dd 0x1BADB002              ; Magic Multiboot1
dd 0x00000000              ; Flags
dd -(0x1BADB002)           ; Checksum

; === BSS - Données non initialisées ===
section .bss
align 4096
p4_table:
    resb 4096
p3_table:
    resb 4096
p2_table:
    resb 4096
stack_bottom:
    resb 16384
stack_top:

; === CODE 32-bit ===
section .text
global _start
extern kernel_main

_start:
    ; Configuration stack IMMEDIATE
    mov esp, stack_top
    
    ; Sauvegarder magic et multiboot info
    push ebx        ; Multiboot info
    push eax        ; Magic
    
    ; Message VGA "BOOT"
    mov dword [0xB8000], 0x2f422f4f  ; 'BO' en vert
    mov dword [0xB8004], 0x2f542f4f  ; 'OT' en vert
    
    ; Vérifier magic multiboot
    cmp eax, 0x2BADB002
    jne .bad_magic
    
    ; Message "OK"
    mov word [0xB8008], 0x2f4f       ; 'O' en vert
    mov word [0xB800A], 0x2f4b       ; 'K' en vert
    
    ; Setup paging
    call setup_paging
    call enable_paging
    
    ; Message "64"
    mov word [0xB800C], 0x2f36       ; '6'
    mov word [0xB800E], 0x2f34       ; '4'
    
    ; Charger GDT
    lgdt [gdt64.pointer]
    
    ; Jump en 64-bit
    jmp gdt64.code:long_mode_start

.bad_magic:
    mov dword [0xB8000], 0x4f4e4f42  ; 'BAD!' en rouge
    hlt

; Setup page tables - Identity map 1GB
setup_paging:
    ; P4[0] -> P3
    mov eax, p3_table
    or eax, 0b11       ; Present + Writable
    mov [p4_table], eax
    
    ; P3[0] -> P2
    mov eax, p2_table
    or eax, 0b11
    mov [p3_table], eax
    
    ; P2: 512 huge pages de 2MB
    mov ecx, 0
.loop:
    mov eax, 0x200000
    mul ecx
    or eax, 0b10000011  ; Present + Writable + Huge
    mov [p2_table + ecx * 8], eax
    inc ecx
    cmp ecx, 512
    jne .loop
    ret

; Activer paging
enable_paging:
    ; Charger P4 dans CR3
    mov eax, p4_table
    mov cr3, eax
    
    ; Activer PAE (bit 5 de CR4)
    mov eax, cr4
    or eax, 1 << 5
    mov cr4, eax
    
    ; Activer long mode (bit 8 de EFER MSR)
    mov ecx, 0xC0000080
    rdmsr
    or eax, 1 << 8
    wrmsr
    
    ; Activer paging (bit 31 de CR0)
    mov eax, cr0
    or eax, 1 << 31
    mov cr0, eax
    ret

; === GDT pour 64-bit ===
section .rodata
align 16
gdt64:
    dq 0                                    ; Null
.code: equ $ - gdt64
    dq (1<<43) | (1<<44) | (1<<47) | (1<<53) ; Code 64-bit
.data: equ $ - gdt64
    dq (1<<44) | (1<<47) | (1<<41)          ; Data
.pointer:
    dw $ - gdt64 - 1
    dq gdt64

; === CODE 64-bit ===
BITS 64
section .text
long_mode_start:
    ; Charger segments
    mov ax, gdt64.data
    mov ss, ax
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    
    ; Stack 64-bit
    mov rsp, stack_top
    
    ; Message ">>" en cyan
    mov word [0xB8050], 0x3e3e      ; '>>'
    mov ax, 0x3f3f
    mov [0xB8052], ax               ; '??'
    
    ; Message "C?" avant appel kernel_main
    mov word [0xB8054], 0x3f43      ; 'C?'
    
    ; Les paramètres sont déjà sur la stack 32-bit
    ; On les récupère maintenant en 64-bit
    xor rdi, rdi
    xor rsi, rsi
    pop rsi         ; multiboot info (était empilé en second)
    pop rdi         ; magic (était empilé en premier)
    
    ; Aligner la stack sur 16 bytes (requis par System V ABI)
    and rsp, -16
    
    ; Message "GO" avant call
    mov word [0xB8056], 0x3f47      ; 'G'
    mov word [0xB8058], 0x3f4f      ; 'O'
    
    ; Appeler kernel_main
    call kernel_main
    
    ; Si retour, halter
.hang:
    cli
    hlt
    jmp .hang
