; AP (Application Processor) Trampoline Code
; This code runs in 16-bit real mode when an AP boots
; It must be located below 1MB (real mode addressable)

BITS 16

section .ap_trampoline

global ap_trampoline_start
global ap_trampoline_end
global ap_trampoline_gdt64_ptr
global ap_trampoline_page_table
global ap_trampoline_stack_top
global ap_trampoline_entry_point

ap_trampoline_start:
    cli                         ; Disable interrupts
    cld                         ; Clear direction flag
    
    ; We're in 16-bit real mode at this point
    ; The BSP will copy this code to low memory (e.g., 0x8000)
    
    ; Setup segments
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    
    ; Load GDT for 32-bit mode
    ; Use absolute address 0x8000 + offset since we're copied there
    ; ap_gdt32_ptr is at offset ~0x60 from start, so it's at 0x8060
    mov si, 0x8000              ; Base address where we're loaded
    add si, (ap_gdt32_ptr - ap_trampoline_start)
    lgdt [si]
    
    ; Enable protected mode (CR0.PE = 1)
    mov eax, cr0
    or eax, 1
    mov cr0, eax
    
    ; Jump to 32-bit code segment
    ; Use absolute address for jump target
    jmp 0x08:0x8000 + (protected_mode_32 - ap_trampoline_start)
    
BITS 32
protected_mode_32:
    ; Setup 32-bit segments
    mov ax, 0x10                ; Data segment selector
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax
    
    ; Load page table (PML4) - BSP writes this to 0x9000
    mov eax, [0x9000]
    mov cr3, eax
    
    ; Enable PAE (CR4.PAE = 1, bit 5)
    mov eax, cr4
    or eax, (1 << 5)
    mov cr4, eax
    
    ; Enable long mode (EFER.LME = 1)
    mov ecx, 0xC0000080         ; EFER MSR
    rdmsr
    or eax, (1 << 8)            ; LME bit
    wrmsr
    
    ; Enable paging (CR0.PG = 1)
    mov eax, cr0
    or eax, (1 << 31)
    mov cr0, eax
    
    ; Load 64-bit GDT - BSP writes pointer to 0x9020
    lgdt [0x9020]
    
    ; Jump to 64-bit code - use absolute address
    push 0x08
    push 0x8000 + (long_mode_64 - ap_trampoline_start)
    retf
    
BITS 64
long_mode_64:
    ; Setup 64-bit segments
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax
    
    ; Load IDT - BSP writes pointer to 0x902a
    lidt [0x902a]
    
    ; Load stack pointer - BSP writes this to 0x9008
    mov rsp, [0x9008]
    
    ; Jump to Rust AP entry point - BSP writes this to 0x9018
    mov rax, [0x9018]
    call rax
    
    ; Should never return, but halt just in case
.halt:
    hlt
    jmp .halt

; 32-bit GDT for initial protected mode
align 16
ap_gdt32:
    dq 0x0000000000000000       ; Null descriptor
    dq 0x00CF9A000000FFFF       ; Code segment (32-bit)
    dq 0x00CF92000000FFFF       ; Data segment (32-bit)
ap_gdt32_end:

ap_gdt32_ptr:
    dw ap_gdt32_end - ap_gdt32 - 1
    ; Base address: 0x8000 + offset of ap_gdt32
    dd 0x8000 + (ap_gdt32 - ap_trampoline_start)

; These values will be filled by BSP before starting APs
align 8
ap_gdt64_ptr:
    dw 0                        ; Limit (will be set by BSP)
    dq 0                        ; Base address (will be set by BSP)

align 8
ap_trampoline_page_table:
    dd 0                        ; PML4 physical address

align 8
ap_trampoline_stack_top:
    dq 0                        ; Stack top address

align 8
ap_trampoline_entry_point:
    dq 0                        ; Rust entry point address

ap_trampoline_end:
