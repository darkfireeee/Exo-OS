; boot.asm
; Point d'entrée du bootloader pour Multiboot2

global start
extern kernel_main

section .text
bits 32
start:
    ; EAX contient le magic number multiboot2 (0x36d76289)
    ; EBX contient l'adresse de la structure d'information multiboot2
    
    ; Sauvegarder les registres multiboot2
    mov edi, ebx                 ; EDI = pointeur vers multiboot info
    mov esi, eax                 ; ESI = multiboot2 magic
    
    ; Vérifier le magic number
    cmp eax, 0x36d76289
    jne .no_multiboot
    
    ; Désactiver les interruptions
    cli
    
    ; Setup initial de la pile (4KB)
    mov esp, stack_top
    
    ; Réinitialiser EFLAGS
    push 0
    popf
    
    ; Vérifier le support du mode long (x86_64)
    call check_multiboot
    call check_cpuid
    call check_long_mode
    
    ; Setup page tables pour le mode long
    call setup_page_tables
    
    ; Activer PAE (Physical Address Extension)
    mov eax, cr4
    or eax, 1 << 5               ; Bit 5 = PAE
    mov cr4, eax
    
    ; Charger la table de pages niveau 4
    mov eax, p4_table
    mov cr3, eax
    
    ; Activer le mode long dans EFER MSR
    mov ecx, 0xC0000080          ; EFER MSR
    rdmsr
    or eax, 1 << 8               ; Bit 8 = LM (Long Mode)
    wrmsr
    
    ; Activer le paging
    mov eax, cr0
    or eax, 1 << 31              ; Bit 31 = PG (Paging)
    or eax, 1 << 16              ; Bit 16 = WP (Write Protect)
    mov cr0, eax
    
    ; Charger le GDT 64-bit
    lgdt [gdt64.pointer]
    
    ; Effectuer un far jump vers le code 64-bit
    jmp gdt64.code:long_mode_start

.no_multiboot:
    mov al, "0"
    jmp error

; Vérification du support Multiboot2
check_multiboot:
    cmp esi, 0x36d76289
    jne .no_multiboot
    ret
.no_multiboot:
    mov al, "1"
    jmp error

; Vérification du support CPUID
check_cpuid:
    ; Tenter de flipper le bit ID (bit 21) dans FLAGS
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
    xor eax, ecx
    jz .no_cpuid
    ret
.no_cpuid:
    mov al, "2"
    jmp error

; Vérification du support du mode long
check_long_mode:
    ; Vérifier si CPUID étendu est disponible
    mov eax, 0x80000000
    cpuid
    cmp eax, 0x80000001
    jb .no_long_mode
    
    ; Utiliser CPUID étendu pour vérifier le mode long
    mov eax, 0x80000001
    cpuid
    test edx, 1 << 29            ; Bit 29 = LM (Long Mode)
    jz .no_long_mode
    ret
.no_long_mode:
    mov al, "3"
    jmp error

; Configuration des tables de pages
setup_page_tables:
    ; Mapper la première entrée P4 vers P3
    mov eax, p3_table
    or eax, 0b11                 ; present + writable
    mov [p4_table], eax
    
    ; Mapper la première entrée P3 vers P2
    mov eax, p2_table
    or eax, 0b11                 ; present + writable
    mov [p3_table], eax
    
    ; Mapper toutes les entrées P2 (512 entrées de 2MB = 1GB)
    mov ecx, 0
.map_p2_table:
    mov eax, 0x200000            ; 2MB
    mul ecx
    or eax, 0b10000011           ; present + writable + huge page
    mov [p2_table + ecx * 8], eax
    
    inc ecx
    cmp ecx, 512
    jne .map_p2_table
    
    ret

; Affichage d'erreur (VGA text mode)
error:
    mov dword [0xb8000], 0x4f524f45  ; "ER" en rouge
    mov dword [0xb8004], 0x4f3a4f52  ; "R:" en rouge
    mov byte [0xb8008], al
    mov byte [0xb8009], 0x4f
    hlt

section .bss
align 4096
; Tables de pages pour le mode long
p4_table:
    resb 4096
p3_table:
    resb 4096
p2_table:
    resb 4096

; Pile (16 KB)
stack_bottom:
    resb 16384
stack_top:

section .rodata
; GDT pour le mode 64-bit
gdt64:
    dq 0                         ; entrée nulle
.code: equ $ - gdt64
    dq (1<<43) | (1<<44) | (1<<47) | (1<<53) ; code segment
.data: equ $ - gdt64
    dq (1<<44) | (1<<47)         ; data segment
.pointer:
    dw $ - gdt64 - 1
    dq gdt64

section .text
bits 64
long_mode_start:
    ; Nous sommes maintenant en mode long!
    
    ; Charger les segments de données avec le sélecteur null
    mov ax, 0
    mov ss, ax
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    
    ; Préparer les arguments pour kernel_main
    ; RDI = multiboot info pointer (déjà dans EDI depuis le 32-bit)
    ; RSI = multiboot magic (déjà dans ESI depuis le 32-bit)
    mov rdi, rdi                 ; zero-extend EDI vers RDI
    mov rsi, rsi                 ; zero-extend ESI vers RSI
    
    ; Appeler le kernel Rust
    call kernel_main
    
    ; Si kernel_main retourne (ne devrait jamais arriver)
    cli
.loop:
    hlt
    jmp .loop
