; boot.asm - Bootloader Exo-OS
; Point d'entrée depuis GRUB (Multiboot1) en mode protégé 32-bit
; Transition vers le long mode 64-bit puis appel du kernel

BITS 32

; === MULTIBOOT HEADER ===
section .multiboot
align 4
multiboot_header:
    dd 0x1BADB002                   ; Magic Multiboot1
    dd 0x00000000                   ; Flags
    dd -(0x1BADB002 + 0x00000000)   ; Checksum (doit faire 0)

; === CONSTANTES ===
%define PAGE_PRESENT    (1 << 0)
%define PAGE_WRITE      (1 << 1)
%define PAGE_HUGE       (1 << 7)

; === SECTION BSS (données non initialisées) ===
section .bss
align 4096

; Tables de pagination (identity mapping)
p4_table:       resb 4096   ; PML4 (Page Map Level 4)
p3_table:       resb 4096   ; PDPT (Page Directory Pointer Table)
p2_table:       resb 4096   ; PD (Page Directory)

; Stack pour le kernel
stack_bottom:   resb 16384  ; 16 KB
stack_top:

; === SECTION DATA (données initialisées) ===
section .data

; GDT pour le long mode
align 16
gdt64:
    dq 0                                    ; Entrée nulle obligatoire
.code: equ $ - gdt64
    dq (1<<43) | (1<<44) | (1<<47) | (1<<53) ; Code segment 64-bit
.data: equ $ - gdt64
    dq (1<<44) | (1<<47) | (1<<41)          ; Data segment
.pointer:
    dw $ - gdt64 - 1                        ; Taille GDT - 1
    dq gdt64                                ; Adresse GDT

; Message de boot pour debug VGA
boot_msg: db 'Exo-OS Booting...', 0

; === SECTION CODE ===
section .text
global _start
extern kernel_main

_start:
    ; GRUB nous donne le contrôle en mode protégé 32-bit
    ; EAX = magic multiboot (0x2BADB002)
    ; EBX = adresse de la structure multiboot_info
    
    ; Configurer la stack temporaire IMMÉDIATEMENT
    mov esp, stack_top
    mov ebp, esp
    
    ; Maintenant on peut sauvegarder les valeurs Multiboot en mémoire
    mov [multiboot_magic], eax
    mov [multiboot_info_ptr], ebx
    
    ; Afficher message de boot sur VGA
    call vga_print_boot
    
    ; Vérifications système
    call check_multiboot
    call check_cpuid
    call check_long_mode
    
    ; Configurer le paging pour le mode 64-bit
    call setup_page_tables
    call enable_paging
    
    ; Charger la GDT 64-bit
    lgdt [gdt64.pointer]
    
    ; Passer en mode 64-bit (long mode)
    jmp gdt64.code:long_mode_start
    
    ; Ne devrait jamais arriver ici
    hlt

; === FONCTION: Afficher message de boot sur VGA ===
vga_print_boot:
    push eax
    push edi
    push esi
    
    mov edi, 0xB8000                ; Adresse buffer VGA
    mov esi, boot_msg
    mov ah, 0x0F                    ; Attribut: blanc sur noir
    
.loop:
    lodsb                           ; Charger caractère dans AL
    test al, al                     ; Tester si fin de chaîne
    jz .done
    stosw                           ; Écrire caractère + attribut
    jmp .loop
    
.done:
    pop esi
    pop edi
    pop eax
    ret

; === FONCTION: Vérifier magic Multiboot ===
check_multiboot:
    cmp dword [multiboot_magic], 0x2BADB002
    jne .no_multiboot
    ret
    
.no_multiboot:
    mov esi, error_no_multiboot
    call vga_print_error
    hlt

; === FONCTION: Vérifier support CPUID ===
check_cpuid:
    ; Essayer de flipper le bit ID (bit 21) dans FLAGS
    pushfd
    pop eax
    mov ecx, eax
    xor eax, 1 << 21
    push eax
    popfd
    pushfd
    pop eax
    push ecx
    popfd
    cmp eax, ecx
    je .no_cpuid
    ret
    
.no_cpuid:
    mov esi, error_no_cpuid
    call vga_print_error
    hlt

; === FONCTION: Vérifier support long mode ===
check_long_mode:
    ; Tester si CPUID étendu est disponible
    mov eax, 0x80000000
    cpuid
    cmp eax, 0x80000001
    jb .no_long_mode
    
    ; Tester le bit LM (Long Mode)
    mov eax, 0x80000001
    cpuid
    test edx, 1 << 29
    jz .no_long_mode
    ret
    
.no_long_mode:
    mov esi, error_no_long_mode
    call vga_print_error
    hlt

; === FONCTION: Configurer tables de pagination ===
setup_page_tables:
    ; Mapper P4[0] -> P3
    mov eax, p3_table
    or eax, PAGE_PRESENT | PAGE_WRITE
    mov [p4_table], eax
    
    ; Mapper P3[0] -> P2
    mov eax, p2_table
    or eax, PAGE_PRESENT | PAGE_WRITE
    mov [p3_table], eax
    
    ; Identity map premiers 2MB avec huge pages
    mov ecx, 0
.loop:
    mov eax, 0x200000               ; 2MB
    mul ecx
    or eax, PAGE_PRESENT | PAGE_WRITE | PAGE_HUGE
    mov [p2_table + ecx * 8], eax
    
    inc ecx
    cmp ecx, 512                    ; 512 entrées = 1GB
    jne .loop
    
    ret

; === FONCTION: Activer paging ===
enable_paging:
    ; Charger P4 dans CR3
    mov eax, p4_table
    mov cr3, eax
    
    ; Activer PAE (Physical Address Extension)
    mov eax, cr4
    or eax, 1 << 5
    mov cr4, eax
    
    ; Activer long mode dans EFER MSR
    mov ecx, 0xC0000080
    rdmsr
    or eax, 1 << 8
    wrmsr
    
    ; Activer paging et protection mode
    mov eax, cr0
    or eax, 1 << 31
    or eax, 1 << 0
    mov cr0, eax
    
    ret

; === FONCTION: Afficher erreur et halter ===
vga_print_error:
    push edi
    push eax
    
    mov edi, 0xB8000 + (80 * 2 * 2) ; Ligne 2
    mov ah, 0x4F                    ; Attribut: blanc sur rouge
    
.loop:
    lodsb
    test al, al
    jz .done
    stosw
    jmp .loop
    
.done:
    pop eax
    pop edi
    ret

; === SECTION DATA (messages d'erreur) ===
section .rodata
error_no_multiboot: db 'ERROR: Not booted by Multiboot bootloader', 0
error_no_cpuid: db 'ERROR: CPUID not supported', 0
error_no_long_mode: db 'ERROR: Long mode (64-bit) not supported', 0

; === SECTION DATA (variables Multiboot) ===
section .data
multiboot_magic: dd 0
multiboot_info_ptr: dd 0

; === CODE 64-BIT ===
BITS 64
section .text
long_mode_start:
    ; Nous sommes maintenant en mode 64-bit !
    
    ; Charger tous les segments avec le data segment
    mov ax, gdt64.data
    mov ss, ax
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    
    ; Reconfigurer la stack pour le mode 64-bit
    mov rsp, stack_top
    mov rbp, rsp
    
    ; Afficher confirmation passage en 64-bit
    mov rdi, 0xB8000 + (80 * 2 * 1)     ; Ligne 1
    mov rax, 0x0F620F360F340F36         ; "64-b" en blanc (little-endian)
    mov [rdi], rax
    mov rax, 0x0F200F620F690F74         ; " bit" en blanc (little-endian)
    mov [rdi + 8], rax
    
    ; Préparer paramètres pour kernel_main (System V ABI)
    ; Lire depuis la mémoire (segment data déjà configuré)
    xor rdi, rdi
    mov edi, dword [multiboot_magic]        ; 1er param: magic (32-bit étendu à 64-bit)
    xor rsi, rsi
    mov esi, dword [multiboot_info_ptr]     ; 2ème param: info ptr (32-bit étendu à 64-bit)
    
    ; Appeler kernel_main
    call kernel_main
    
    ; Si kernel_main retourne, halter définitivement
.hang:
    cli                              ; Désactiver interruptions
    hlt
    jmp .hang
