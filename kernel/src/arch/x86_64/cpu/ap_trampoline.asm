; AP Trampoline - 16-bit real mode to 64-bit long mode transition
; This code is copied to low memory (< 1MB) and executed by each AP
;
; The trampoline must:
; 1. Start in 16-bit real mode (CS:IP from SIPI)
; 2. Enable A20 line
; 3. Load temporary GDT
; 4. Enable protected mode (CR0.PE)
; 5. Jump to 32-bit protected mode code
; 6. Enable PAE (CR4.PAE)
; 7. Load page tables (CR3)
; 8. Enable long mode (EFER.LME)
; 9. Enable paging (CR0.PG)
; 10. Jump to 64-bit code
; 11. Call ap_entry(apic_id)

[BITS 16]
[ORG 0x8000]

global ap_trampoline_start
global ap_trampoline_end

ap_trampoline_start:
    cli
    cld
    
    ; Setup segments
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7000      ; Temporary stack
    
    ; Enable A20 line (fast method)
    in al, 0x92
    or al, 2
    out 0x92, al
    
    ; Load temporary GDT
    lgdt [ap_gdt_ptr - ap_trampoline_start + 0x8000]
    
    ; Enable protected mode
    mov eax, cr0
    or eax, 1           ; CR0.PE = 1
    mov cr0, eax
    
    ; Far jump to 32-bit code
    jmp dword 0x08:(ap_protected_mode - ap_trampoline_start + 0x8000)

[BITS 32]
ap_protected_mode:
    ; Setup 32-bit segments
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax
    
    ; Enable PAE
    mov eax, cr4
    or eax, (1 << 5)    ; CR4.PAE = 1
    mov cr4, eax
    
    ; Load page tables from data area
    ; The BSP writes CR3 value here before booting APs
    mov eax, [ap_data_cr3 - ap_trampoline_start + 0x8000]
    mov cr3, eax
    
    ; Enable long mode in EFER
    mov ecx, 0xC0000080 ; IA32_EFER MSR
    rdmsr
    or eax, (1 << 8)    ; EFER.LME = 1
    wrmsr
    
    ; Enable paging
    mov eax, cr0
    or eax, (1 << 31)   ; CR0.PG = 1
    mov cr0, eax
    
    ; Far jump to 64-bit code
    jmp dword 0x18:(ap_long_mode - ap_trampoline_start + 0x8000)

[BITS 64]
ap_long_mode:
    ; Setup 64-bit segments
    mov ax, 0x20
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax
    
    ; Load the 64-bit stack pointer from data area
    mov rsp, [ap_data_stack - ap_trampoline_start + 0x8000]
    
    ; Get our APIC ID
    mov ecx, 0x802      ; x2APIC ID MSR (assuming x2APIC)
    rdmsr
    mov edi, eax        ; First argument = APIC ID
    
    ; Load ap_entry address from data area
    mov rax, [ap_data_entry - ap_trampoline_start + 0x8000]
    
    ; Call ap_entry(apic_id)
    call rax
    
    ; Should never return, but halt if it does
.halt:
    hlt
    jmp .halt

; Data area (filled by BSP before booting APs)
align 8
ap_data_cr3:    dq 0    ; Page table root
ap_data_stack:  dq 0    ; Stack pointer for this AP
ap_data_entry:  dq 0    ; Address of ap_entry function

; Temporary GDT for AP boot
align 16
ap_gdt:
    ; Null descriptor
    dq 0
    
    ; 32-bit code segment (selector 0x08)
    dw 0xFFFF           ; Limit 0-15
    dw 0x0000           ; Base 0-15
    db 0x00             ; Base 16-23
    db 0x9A             ; Access: present, ring 0, code, exec/read
    db 0xCF             ; Flags: 4KB granularity, 32-bit
    db 0x00             ; Base 24-31
    
    ; 32-bit data segment (selector 0x10)
    dw 0xFFFF
    dw 0x0000
    db 0x00
    db 0x92             ; Access: present, ring 0, data, read/write
    db 0xCF
    db 0x00
    
    ; 64-bit code segment (selector 0x18)
    dw 0x0000
    dw 0x0000
    db 0x00
    db 0x9A             ; Access: present, ring 0, code, exec/read
    db 0x20             ; Flags: long mode
    db 0x00
    
    ; 64-bit data segment (selector 0x20)
    dw 0x0000
    dw 0x0000
    db 0x00
    db 0x92             ; Access: present, ring 0, data, read/write
    db 0x00
    db 0x00

ap_gdt_ptr:
    dw ap_gdt_ptr - ap_gdt - 1  ; Limit
    dd ap_gdt - ap_trampoline_start + 0x8000  ; Base (32-bit, adjusted at runtime)

ap_trampoline_end:
