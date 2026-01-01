; MINIMAL TEST TRAMPOLINE
; Just test if the AP can execute ANY instruction

[BITS 16]

global ap_trampoline_start_minimal
global ap_trampoline_end_minimal

ap_trampoline_start_minimal:
    ; Output 'X' to debug console
    mov al, 'X'
    out 0xE9, al
    
    ; Output 'Y' to confirm second instruction works
    mov al, 'Y'
    out 0xE9, al
    
    ; Output 'Z' to confirm third instruction works
    mov al, 'Z'
    out 0xE9, al
    
    ; Infinite loop
.halt_loop:
    hlt
    jmp .halt_loop

ap_trampoline_end_minimal:

