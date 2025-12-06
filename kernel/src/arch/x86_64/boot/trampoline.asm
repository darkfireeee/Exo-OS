; AP (Application Processor) Trampoline Code
; This code boots APs from 16-bit real mode to 64-bit long mode
;
; Boot sequence:
; 1. AP receives SIPI with vector 0x08 (physical address 0x8000)
; 2. Starts in 16-bit real mode at CS:IP = 0x0800:0x0000
; 3. Loads GDT, enables protected mode (32-bit)
; 4. Enables PAE and long mode
; 5. Jumps to 64-bit Rust ap_startup() function
;
; Memory layout at 0x8000:
;   0x8000 - 0x8100: This trampoline code
;   0x8100 - 0x8200: Trampoline GDT
;   0x8200 - 0x8210: Data variables (PML4 addr, stack ptr, CPU ID)

[BITS 16]
section .text.trampoline
align 4096

global ap_trampoline_start
global ap_trampoline_end
global ap_trampoline_pml4_addr
global ap_trampoline_stack_ptr
global ap_trampoline_cpu_id
global ap_trampoline_entry_point
global ap_trampoline_gdt_ptr

ap_trampoline_start:
    cli                         ; Disable interrupts
    cld                         ; Clear direction flag
    
    ; Set up segment registers
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    
    ; Load GDT pointer (relative to 0x8000)
    lgdt [0x8000 + (ap_gdt_ptr - ap_trampoline_start)]
    
    ; Enable protected mode (set CR0.PE)
    mov eax, cr0
    or eax, 1
    mov cr0, eax
    
    ; Far jump to 32-bit protected mode code segment
    ; GDT entry 1 = code segment (0x08)
    jmp 0x08:(0x8000 + (ap_protected_mode - ap_trampoline_start))

[BITS 32]
ap_protected_mode:
    ; Set up 32-bit data segment (GDT entry 2 = 0x10)
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax
    
    ; Enable PAE (Physical Address Extension) - CR4.PAE (bit 5)
    mov eax, cr4
    or eax, (1 << 5)
    mov cr4, eax
    
    ; Load PML4 address into CR3
    ; Read from ap_pml4_addr variable at offset 0x8200
    mov eax, [0x8200]
    mov cr3, eax
    
    ; Enable long mode by setting IA32_EFER.LME (bit 8)
    ; IA32_EFER MSR = 0xC0000080
    mov ecx, 0xC0000080
    rdmsr
    or eax, (1 << 8)            ; Set LME bit
    wrmsr
    
    ; Enable paging (CR0.PG = bit 31)
    ; This activates long mode (since LME was set)
    mov eax, cr0
    or eax, (1 << 31)
    mov cr0, eax
    
    ; Far jump to 64-bit long mode code segment
    ; GDT entry 3 = 64-bit code segment (0x18)
    jmp 0x18:(0x8000 + (ap_long_mode - ap_trampoline_start))

[BITS 64]
ap_long_mode:
    ; Set up 64-bit data segment (GDT entry 4 = 0x20)
    mov ax, 0x20
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax
    
    ; Load stack pointer from ap_stack_ptr variable at 0x8208
    mov rsp, [0x8208]
    
    ; Align stack to 16 bytes (required by System V ABI)
    and rsp, ~0xF
    
    ; Load CPU ID from ap_cpu_id variable at 0x8210
    mov rdi, [0x8210]
    
    ; Load ap_startup entry point from 0x8218
    mov rax, [0x8218]
    
    ; Call Rust ap_startup(cpu_id) function
    call rax
    
    ; Should never return, but halt just in case
.hang:
    cli
    hlt
    jmp .hang

; ============================================================================
; GDT (Global Descriptor Table) for trampoline
; ============================================================================
align 16
ap_gdt:
    ; Entry 0: Null descriptor (required)
    dq 0x0000000000000000
    
    ; Entry 1: 32-bit code segment (base=0, limit=4GB, executable, readable)
    ; Flags: G=1 (4KB granularity), D=1 (32-bit), L=0, P=1 (present)
    ; Access: P=1, DPL=0, S=1 (code/data), Type=1010 (code, readable, accessed)
    dq 0x00CF9A000000FFFF
    
    ; Entry 2: 32-bit data segment (base=0, limit=4GB, writable)
    ; Access: P=1, DPL=0, S=1, Type=0010 (data, writable, accessed)
    dq 0x00CF92000000FFFF
    
    ; Entry 3: 64-bit code segment (base=0, limit=ignored, executable, readable)
    ; Flags: G=0, D=0, L=1 (64-bit), P=1
    ; Access: P=1, DPL=0, S=1, Type=1010 (code, readable, accessed)
    dq 0x00AF9A000000FFFF
    
    ; Entry 4: 64-bit data segment (base=0, limit=ignored, writable)
    dq 0x00AF92000000FFFF
ap_gdt_end:

; GDT pointer structure (loaded by LGDT)
align 8
ap_gdt_ptr:
    dw ap_gdt_end - ap_gdt - 1      ; Limit (size - 1)
    dq 0x8000 + (ap_gdt - ap_trampoline_start)  ; Base address

; ============================================================================
; Data variables (written by setup_trampoline() before booting AP)
; ============================================================================
align 16
ap_trampoline_pml4_addr:
    dq 0        ; PML4 physical address (written by setup_trampoline)

ap_trampoline_stack_ptr:
    dq 0        ; Stack top address (written by setup_trampoline)

ap_trampoline_cpu_id:
    dq 0        ; CPU ID (written by setup_trampoline)

ap_trampoline_entry_point:
    dq 0        ; ap_startup() function pointer (written by setup_trampoline)

ap_trampoline_end:

; Export size for Rust
global ap_trampoline_size
ap_trampoline_size equ ap_trampoline_end - ap_trampoline_start
