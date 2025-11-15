; boot.asm - Point d'entrée assembleur du noyau Exo-OS
; Ce fichier contient le code minimal pour démarrer le noyau en mode long

[BITS 32]

; Constantes Multiboot2
MAGIC       equ 0xE85250D6    ; Magic number multiboot2
ARCH        equ 0             ; Architecture i386 protected mode
HEADER_LEN  equ header_end - header_start
CHECKSUM    equ -(MAGIC + ARCH + HEADER_LEN)

section .multiboot_header
align 8
header_start:
    dd MAGIC
    dd ARCH
    dd HEADER_LEN
    dd CHECKSUM
    
    ; Tag de fin
    dw 0    ; type
    dw 0    ; flags
    dd 8    ; size
header_end:

section .bss
align 4096
; Pile du noyau (16 KiB)
stack_bottom:
    resb 16384
stack_top:

; Tables de pages pour le mode long (64 bits)
align 4096
p4_table:
    resb 4096
p3_table:
    resb 4096
p2_table:
    resb 4096

; Stockage de l'adresse Multiboot2 (sauvegardée depuis EBX en 32-bit)
align 8
multiboot_info_ptr:
    dq 0
; Stockage du magic Multiboot2 (sauvegardé depuis EAX en 32-bit)
align 8
multiboot_magic:
    dq 0

section .multiboot_header
global _start
extern kernel_main  ; Point d'entrée Rust principal (lib.rs)

_start:
    ; DEBUG: Écrire 'A' en mode 32-bit (prouve que GRUB a appelé _start)
    mov dword [0xB8000], 0x4F414F41  ; 'AA' en blanc sur fond rouge
    
    ; Désactiver les interruptions
    cli
    
    ; Configurer la pile
    mov esp, stack_top
    
    ; DEBUG: Écrire 'B' après config pile
    mov dword [0xB8004], 0x2F422F42  ; 'BB' en vert
    
    ; Sauvegarder l'adresse Multiboot2 fournie par GRUB (dans EBX)
    mov dword [multiboot_info_ptr], ebx
    ; Sauvegarder le magic Multiboot2 fourni par GRUB (dans EAX)
    mov dword [multiboot_magic], eax
    
    ; Vérifier le support du mode long (x86_64)
    call check_long_mode
    
    ; DEBUG: Écrire 'P' après check_long_mode
    mov dword [0xB8008], 0x1F501F50  ; 'PP' en bleu
    
    ; Activer la pagination et passer en mode long
    call setup_page_tables
    call enable_paging
    call enter_long_mode
    
    ; Ne devrait jamais arriver ici
    hlt

; Vérifie si le CPU supporte le mode long
check_long_mode:
    ; Vérifier si CPUID est supporté
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
    je .no_long_mode
    
    ; Vérifier le mode long via CPUID
    mov eax, 0x80000000
    cpuid
    cmp eax, 0x80000001
    jb .no_long_mode
    
    mov eax, 0x80000001
    cpuid
    test edx, 1 << 29
    jz .no_long_mode
    
    ret

.no_long_mode:
    mov al, 'L'
    mov byte [0xB8000], al
    hlt

; Configure les tables de pages pour le mode long
setup_page_tables:
    ; Mapper la première entrée P4 vers P3
    mov eax, p3_table
    or eax, 0b11  ; present + writable
    mov [p4_table], eax
    
    ; Mapper la première entrée P3 vers P2
    mov eax, p2_table
    or eax, 0b11
    mov [p3_table], eax
    
    ; Identity map les premiers 2 MiB avec des huge pages
    mov ecx, 0
.map_p2_table:
    mov eax, 0x200000  ; 2 MiB
    mul ecx
    or eax, 0b10000011  ; present + writable + huge page
    mov [p2_table + ecx * 8], eax
    
    inc ecx
    cmp ecx, 512
    jne .map_p2_table
    
    ret

; Active la pagination
enable_paging:
    ; Charger P4 dans CR3
    mov eax, p4_table
    mov cr3, eax
    
    ; Activer PAE (Physical Address Extension)
    mov eax, cr4
    or eax, 1 << 5
    mov cr4, eax
    
    ; Activer le mode long dans EFER MSR
    mov ecx, 0xC0000080
    rdmsr
    or eax, 1 << 8
    wrmsr
    
    ; Activer la pagination
    mov eax, cr0
    or eax, 1 << 31
    mov cr0, eax
    
    ret

; Entre en mode long (64 bits)
enter_long_mode:
    ; Charger la GDT 64 bits
    lgdt [gdt64.pointer]
    
    ; Sauter vers le code 64 bits
    jmp gdt64.code:long_mode_start

[BITS 64]
long_mode_start:
    ; DEBUG: Écrire '6' en mode long (prouve qu'on est arrivé en 64-bit)
    mov rax, 0xB8000
    mov word [rax], 0x4F36  ; '6' en blanc sur fond rouge
    
    ; Charger les segments (nécessite un segment de données R/W valide pour SS)
    mov ax, gdt64.data
    mov ss, ax
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    
    ; DEBUG: Écrire '4' après chargement des segments
    mov rax, 0xB8000
    mov word [rax + 2], 0x2F34  ; '4' en vert
    
    ; Configurer la pile 64-bit avant d'appeler Rust
    mov rsp, stack_top
    xor rbp, rbp
    
    ; DEBUG: Écrire 'S' après configuration de la pile (pile OK)
    mov rax, 0xB8000
    mov word [rax + 4], 0x1F53  ; 'S' en bleu

    ; Récupérer l'adresse Multiboot2 et le magic sauvegardés
    mov rdi, [rel multiboot_info_ptr]  ; RDI = adresse multiboot (1er argument SysV x86_64)
    mov esi, dword [rel multiboot_magic] ; RSI = magic (2e argument)
    
    ; DEBUG: Écrire 'C' avant CALL (prêt à appeler Rust)
    mov rax, 0xB8000
    mov word [rax + 6], 0x6F43  ; 'C' en jaune
    
    ; Appeler le point d'entrée Rust principal
    ; kernel_main(multiboot_info_ptr: u64, multiboot_magic: u32) -> !
    call kernel_main
    
    ; Si _kernel_start retourne (ne devrait pas arriver)
    cli
    hlt
    jmp $

section .rodata
; GDT pour le mode long
gdt64:
    dq 0  ; Entrée nulle
.code: equ $ - gdt64
    ; Descripteur de code 64-bit: P=1, DPL=0, S=1 (code/données), Type=0xA (exécutable, lisible), L=1, D=0, G=0
    dq 0x00209A0000000000
.data: equ $ - gdt64
    ; Descripteur de données 64-bit: P=1, DPL=0, S=1, Type=0x2 (données, lecture/écriture), L=0, D=0, G=0
    dq 0x0000920000000000
.pointer:
    dw $ - gdt64 - 1
    dq gdt64
