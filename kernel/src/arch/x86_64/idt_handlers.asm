; Gestionnaires d'interruption en assembleur pur
; Pour éviter les problèmes de naked_asm! avec LLVM sur Windows/MSVC

section .text

; Macro pour créer un handler simple
%macro SIMPLE_HANDLER 2
global %1
%1:
    push 0              ; Code d'erreur dummy
    push %2             ; Numéro d'exception
    iretq
%endmacro

; Exceptions CPU (0-31)
SIMPLE_HANDLER divide_error_handler, 0
SIMPLE_HANDLER debug_handler, 1
SIMPLE_HANDLER nmi_handler, 2
SIMPLE_HANDLER breakpoint_handler, 3
SIMPLE_HANDLER overflow_handler, 4
SIMPLE_HANDLER bound_range_exceeded_handler, 5
SIMPLE_HANDLER invalid_opcode_handler, 6
SIMPLE_HANDLER device_not_available_handler, 7
SIMPLE_HANDLER double_fault_handler, 8
SIMPLE_HANDLER coprocessor_segment_overrun_handler, 9
SIMPLE_HANDLER invalid_tss_handler, 10
SIMPLE_HANDLER segment_not_present_handler, 11
SIMPLE_HANDLER stack_segment_fault_handler, 12
SIMPLE_HANDLER general_protection_fault_handler, 13
SIMPLE_HANDLER page_fault_handler, 14
SIMPLE_HANDLER x87_fpu_error_handler, 16
SIMPLE_HANDLER alignment_check_handler, 17
SIMPLE_HANDLER machine_check_handler, 18
SIMPLE_HANDLER simd_floating_point_handler, 19
SIMPLE_HANDLER virtualization_exception_handler, 20

; Gestionnaires par défaut
SIMPLE_HANDLER default_irq_handler, 32
SIMPLE_HANDLER default_interrupt_handler, 255

; Handler commun pour les exceptions
global exception_common_handler
exception_common_handler:
    ; Sauvegarder les registres
    push rax
    push rbx
    push rcx
    push rdx
    push rsi
    push rdi
    push rbp
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15
    
    ; Appeler le handler Rust (frame pointeur dans rdi)
    mov rdi, rsp
    call exception_handler_rust
    
    ; Restaurer les registres
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rbx
    pop rax
    
    ; Nettoyer les valeurs poussées (error_code, int_num)
    add rsp, 16
    
    iretq

; Handler commun pour les IRQ
global irq_common_handler
irq_common_handler:
    push rax
    push rbx
    push rcx
    push rdx
    push rsi
    push rdi
    push rbp
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15
    
    mov rdi, rsp
    call irq_handler_rust
    
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rbx
    pop rax
    
    add rsp, 16
    iretq
