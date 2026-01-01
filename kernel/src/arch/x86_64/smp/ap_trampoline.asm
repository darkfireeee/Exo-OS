; ============================================================================
; AP Bootstrap Trampoline - Production Grade
; ============================================================================
; Transitions Application Processors from 16-bit real mode to 64-bit long mode
; This code is copied to 0x8000 and executed by APs during SMP initialization
;
; MEMORY LAYOUT:
;   0x8000: Trampoline code (this file)
;   0x8200: Boot data structure (written by BSP)
;
; EXECUTION FLOW:
;   Real Mode (16-bit) → Protected Mode (32-bit) → Long Mode (64-bit)
;
; REQUIREMENTS:
;   - Position-independent code
;   - No external dependencies
;   - Robust error handling
;   - Maximum size: 256 bytes for code
; ============================================================================

[BITS 16]

global ap_trampoline_start
global ap_trampoline_end

; ============================================================================
; REAL MODE ENTRY (16-bit)
; SIPI sets CS:IP = (vector << 8):0, so for vector 0x08, we start at 0x0800:0x0000 = 0x8000
; ============================================================================
ap_trampoline_start:
    cli                         ; Disable interrupts immediately
    cld                         ; Clear direction flag
    
    ; === DEBUG: Mark entry ===
    mov al, 'A'
    out 0xE9, al               ; QEMU/Bochs debug console
    
    ; === Initialize segments to zero ===
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7C00             ; Temporary real-mode stack
    
    ; === DEBUG: Segments initialized ===
    mov al, 'B'
    out 0xE9, al
    
    ; === Enable A20 line (required for >1MB access) ===
    ; Fast A20 method via port 0x92
    in al, 0x92
    test al, 0x02
    jnz .a20_enabled           ; Already enabled
    or al, 0x02
    out 0x92, al
.a20_enabled:
    
    ; === DEBUG: A20 enabled ===
    mov al, 'C'
    out 0xE9, al
    
    ; === Load temporary GDT for 32-bit protected mode ===
    ; GDT is embedded at end of this code
    mov si, 0x8000
    add si, (gdt32_ptr - ap_trampoline_start)
    lgdt [si]
    
    ; === DEBUG: GDT loaded ===
    mov al, 'D'
    out 0xE9, al
    
    ; === Enable Protected Mode ===
    mov eax, cr0
    or eax, 0x01               ; Set CR0.PE (Protected Mode Enable)
    mov cr0, eax
    
    ; === DEBUG: Protected mode enabled ===
    mov al, 'E'
    out 0xE9, al
    
    ; === Far jump to flush prefetch queue and enter 32-bit mode ===
    ; Code segment selector 0x08 from GDT32
    jmp 0x08:(protected_mode_32 - ap_trampoline_start + 0x8000)

; ============================================================================
; PROTECTED MODE (32-bit)
; ============================================================================
[BITS 32]
protected_mode_32:
    ; === DEBUG: Entered 32-bit mode ===
    mov al, 'F'
    out 0xE9, al
    
    ; === Setup 32-bit data segments ===
    mov ax, 0x10               ; Data segment selector from GDT32
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax
    mov esp, 0x7C00            ; Temporary 32-bit stack
    
    ; === DEBUG: 32-bit segments set ===
    mov al, 'G'
    out 0xE9, al
    
    ; === Enable PAE (Physical Address Extension) ===
    mov eax, cr4
    or eax, (1 << 5)           ; Set CR4.PAE
    mov cr4, eax
    
    ; === DEBUG: PAE enabled ===
    mov al, 'H'
    out 0xE9, al
    
    ; === Load page table (PML4) from boot data ===
    ; Boot data is at 0x8200, PML4 address is first 8 bytes
    mov edi, 0x8200            ; Boot data base
    mov eax, [edi]             ; Load lower 32 bits of PML4 address
    mov cr3, eax               ; Set CR3 to PML4 physical address
    
    ; === DEBUG: CR3 loaded ===
    mov al, 'I'
    out 0xE9, al
    
    ; === Enable Long Mode ===
    ; Set EFER.LME (Long Mode Enable) via MSR
    mov ecx, 0xC0000080        ; EFER MSR
    rdmsr
    or eax, (1 << 8)           ; Set LME bit
    wrmsr
    
    ; === DEBUG: Long mode enabled in EFER ===
    mov al, 'J'
    out 0xE9, al
    
    ; === Enable Paging (activates long mode) ===
    mov eax, cr0
    or eax, (1 << 31)          ; Set CR0.PG (Paging)
    mov cr0, eax
    
    ; Now in compatibility mode (32-bit code in 64-bit mode)
    ; Need to jump to 64-bit code segment
    
    ; === DEBUG: Paging enabled ===
    mov al, 'K'
    out 0xE9, al
    
    ; === Load 64-bit GDT ===
    ; GDT64 descriptor is at boot_data + 0x20
    mov edi, 0x8200
    add edi, 0x20
    lgdt [edi]
    
    ; === Far jump to 64-bit code segment ===
    ; Must use absolute address from boot data (entry point at offset 0x18)
    mov edi, 0x8200
    mov eax, [edi + 0x18]      ; Load entry point (lower 32 bits)
    
    ; Use retf trick to far jump to 64-bit code
    push 0x08                  ; 64-bit code selector
    push eax                   ; Entry point address
    retf

; ============================================================================
; LONG MODE (64-bit)
; ============================================================================
[BITS 64]
long_mode_64:
    ; === DEBUG: Entered 64-bit mode ===
    mov al, 'L'
    out 0xE9, al
    
    ; === Setup 64-bit segments ===
    mov ax, 0x10               ; Data segment selector
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax
    
    ; === Load stack from boot data ===
    ; Stack pointer at boot_data + 0x08
    mov rdi, 0x8200
    mov rsp, [rdi + 0x08]
    
    ; === Load IDT ===
    ; IDT descriptor at boot_data + 0x2a
    lea rdi, [0x8200 + 0x2a]
    lidt [rdi]
    
    ; === DEBUG: IDT loaded ===
    mov al, 'M'
    out 0xE9, al
    
    ; === Initialize FPU/SSE/AVX ===
    ; Clear CR0.EM (bit 2) - Enable FPU
    mov rax, cr0
    and rax, ~(1 << 2)         ; Clear EM
    or rax, (1 << 1)           ; Set MP (Monitor coprocessor)
    mov cr0, rax
    
    ; Enable OSFXSR and OSXMMEXCPT in CR4
    mov rax, cr4
    or rax, (1 << 9)           ; Set OSFXSR (SSE support)
    or rax, (1 << 10)          ; Set OSXMMEXCPT (SSE exceptions)
    mov cr4, rax
    
    ; Initialize FPU state
    fninit
    
    ; Initialize SSE state
    mov rax, 0x1F80            ; Default MXCSR value
    push rax
    ldmxcsr [rsp]
    pop rax
    
    ; === DEBUG: FPU/SSE initialized ===
    mov al, 'N'
    out 0xE9, al
    
    ; === Get CPU ID from boot data ===
    mov rdi, 0x8200
    mov rdi, [rdi + 0x10]      ; CPU ID at offset 0x10
    
    ; === Call Rust entry point ===
    ; Entry point function signature: extern "C" fn(cpu_id: u64) -> !
    mov rdi, [0x8218]          ; Entry point at boot_data + 0x18
    mov rsi, [0x8210]          ; CPU ID at boot_data + 0x10
    mov rdi, rsi               ; First argument (cpu_id)
    
    ; Get actual entry point address
    mov rax, [0x8218]
    call rax
    
    ; Should never return
    cli
.hang:
    hlt
    jmp .hang

; ============================================================================
; DATA SECTION - GDT for 32-bit Protected Mode
; ============================================================================
align 8
gdt32:
    ; NULL descriptor
    dq 0x0000000000000000
    
    ; 32-bit Code segment (selector 0x08)
    ; Base=0, Limit=0xFFFFF, Present, Ring 0, Code, Readable, Granularity=4K
    dq 0x00CF9A000000FFFF
    
    ; 32-bit Data segment (selector 0x10)
    ; Base=0, Limit=0xFFFFF, Present, Ring 0, Data, Writable, Granularity=4K
    dq 0x00CF92000000FFFF

gdt32_ptr:
    dw gdt32_ptr - gdt32 - 1   ; Limit
    dd gdt32                    ; Base (physical address)

; ============================================================================
; END MARKER
; ============================================================================
ap_trampoline_end:

; Pad to ensure we don't overflow 512 bytes
times 512 - ($ - $$) db 0
