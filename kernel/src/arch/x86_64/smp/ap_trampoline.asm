; ABSOLUTE MINIMAL TEST - No jumps, no loops
[BITS 16]

global ap_trampoline_start
global ap_trampoline_end

ap_trampoline_start:
    mov al, 'X'
    out 0xE9, al
    mov al, 'Y'
    out 0xE9, al
    mov al, 'Z'
    out 0xE9, al
    hlt
    hlt
    hlt
    hlt
    hlt
    hlt
    hlt
    hlt

ap_trampoline_end:

    
    ; Load 32-bit GDT (position-independent)
    ; Calculate: gdt_ptr = 0x8000 + (gdt32_ptr - ap_trampoline_start)
    mov si, 0x8000
    add si, (gdt32_ptr - ap_trampoline_start)
    lgdt [si]
    
    ; DEBUG: GDT loaded
    mov al, 'C'
    out 0xE9, al
    
    ; Enable Protected Mode (set CR0.PE)
    mov eax, cr0
    or al, 1
    mov cr0, eax
    
    ; DEBUG: Protected mode enabled
    mov al, 'D'
    out 0xE9, al
    
    ; Far jump to 32-bit code segment
    ; Target: 0x8000 + offset of mode32_entry
    jmp 0x08:(0x8000 + (mode32_entry - ap_trampoline_start))

; ============================================================================
; PROTECTED MODE (32-bit)
; ============================================================================
[BITS 32]
mode32_entry:
    ; DEBUG: Entered 32-bit mode
    mov al, 'E'
    out 0xE9, al
    
    ; Initialize 32-bit segment registers
    mov ax, 0x10                ; Data segment selector
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax
    mov esp, 0x7000             ; Temporary 32-bit stack
    
    ; DEBUG: 32-bit segments set
    mov al, 'F'
    out 0xE9, al
    
    ; === Setup Paging for Long Mode ===
    
    ; Load PML4 table address from data area (0x8200 + 0x00)
    ; Manual 32-bit load to avoid NASM confusion
    db 0xA1                     ; mov eax, moffs32
    dd 0x00008200               ; absolute address 0x8200
    mov cr3, eax
    
    ; DEBUG: CR3 loaded
    mov al, 'G'
    out 0xE9, al
    
    ; Enable PAE (Physical Address Extension)
    mov eax, cr4
    or eax, (1 << 5)            ; CR4.PAE = 1
    mov cr4, eax
    
    ; DEBUG: PAE enabled
    mov al, 'H'
    out 0xE9, al
    
    ; Enable Long Mode in EFER MSR
    mov ecx, 0xC0000080         ; EFER MSR number
    rdmsr
    or eax, (1 << 8)            ; EFER.LME = 1
    wrmsr
    
    ; DEBUG: Long mode enabled in EFER
    mov al, 'I'
    out 0xE9, al
    
    ; Enable Paging (activates Long Mode)
    mov eax, cr0
    or eax, (1 << 31)           ; CR0.PG = 1
    mov cr0, eax
    
    ; Load 64-bit GDT from data area (0x8200 + 0x20)
    ; Manual lgdt with 32-bit address
    db 0x0F, 0x01, 0x15         ; lgdt [abs32]
    dd 0x00008220               ; absolute address 0x8220
    
    ; Far jump to 64-bit code segment
    push 0x08                   ; 64-bit code segment selector
    push (0x8000 + (mode64_entry - ap_trampoline_start))
    retf

; ============================================================================
; LONG MODE (64-bit)
; ============================================================================
[BITS 64]
mode64_entry:
    ; Initialize 64-bit segment registers
    mov ax, 0x10                ; 64-bit data segment
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax
    
    ; Load IDT - use absolute address
    ; idt_desc_temp is at 0x8000 + offset
    mov rax, 0x8000 + (idt_desc_temp - ap_trampoline_start)
    lidt [rax]
    
    ; Load kernel stack pointer (0x8200 + 0x08)
    ; mov rsp, [abs64] - manual encoding
    db 0x48, 0x8B, 0x24, 0x25   ; mov rsp, [abs32]
    dd 0x00008208               ; absolute address
    
    ; Clear frame pointer
    xor rbp, rbp
    
    ; Load CPU ID into first argument register (0x8200 + 0x10)
    ; mov rdi, [abs64] - manual encoding
    db 0x48, 0x8B, 0x3C, 0x25   ; mov rdi, [abs32]
    dd 0x00008210               ; absolute address
    
    ; Jump to kernel AP entry point (0x8200 + 0x18)
    ; mov rax, [abs64] - manual encoding
    db 0x48, 0x8B, 0x04, 0x25   ; mov rax, [abs32]
    dd 0x00008218               ; absolute address
    call rax
    
    ; Should never return - halt CPU
.halt:
    cli
    hlt
    jmp .halt

; ============================================================================
; 32-BIT GDT (for Protected Mode transition)
; ============================================================================
align 16
gdt32:
    ; Null descriptor (required)
    dq 0x0000000000000000
    
    ; Code segment: Base=0, Limit=4GB, 32-bit, Read/Execute
    ;   Flags: G=1 (4KB granularity), D/B=1 (32-bit)
    ;   Access: P=1 (present), DPL=0 (ring 0), S=1 (code/data), Type=1010 (code, exec, read)
    dq 0x00CF9A000000FFFF
    
    ; Data segment: Base=0, Limit=4GB, 32-bit, Read/Write
    ;   Flags: G=1 (4KB granularity), D/B=1 (32-bit)
    ;   Access: P=1 (present), DPL=0 (ring 0), S=1 (code/data), Type=0010 (data, read/write)
    dq 0x00CF92000000FFFF
gdt32_end:

; GDT pointer structure for LGDT instruction
align 4
gdt32_ptr:
    dw gdt32_end - gdt32 - 1    ; Limit
    dd 0x8000 + (gdt32 - ap_trampoline_start)  ; Base (absolute address)

; Temporary IDT descriptor for debugging
align 16
idt_desc_temp:
    dw 0x0fff                   ; Limit (4096 bytes)
    dd 0x001511a0               ; Base low 32 bits
    dd 0x00000000               ; Base high 32 bits

; ============================================================================
; PADDING AND END MARKER
; ============================================================================
align 16
ap_trampoline_end:
