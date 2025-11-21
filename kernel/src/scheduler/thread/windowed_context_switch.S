# windowed_context_switch.S
# Context Switch Optimisé - Sauvegarde RSP+RIP uniquement (16 bytes vs 128)
#
# Basé sur l'hypothèse que les registres callee-saved (RBX, RBP, R12-R15)
# sont déjà sur la pile grâce à l'ABI x86_64 System V
#
# Gain attendu: 5-10× plus rapide que context_switch.S complet

.section .text
.global windowed_context_switch
.type windowed_context_switch, @function
// Alias pour compatibilité avec le scheduler existant
.global context_switch

# void windowed_context_switch(u64* old_rsp_ptr, u64 new_rsp)
# Arguments:
#   RDI: pointeur où sauvegarder l'ancien RSP
#   RSI: nouveau RSP à restaurer
windowed_context_switch:
    # === SAUVEGARDE MINIMAL (16 bytes) ===
    # Sauvegarde RSP actuel
    test %rdi, %rdi
    jz 1f
    mov %rsp, (%rdi)
1:
    
    # === SWITCH VERS NOUVELLE PILE ===
    mov %rsi, %rsp
    
    # === RESTAURATION ===
    # RIP est au sommet de la nouvelle pile
    # Le RET va automatiquement la restaurer
    
    ret

# Rendre le symbole context_switch compatible en pointant vers l'implémentation windowed
context_switch:
    jmp windowed_context_switch

# Version avec registres callee-saved explicites (fallback si ABI violée)
.global windowed_context_switch_full
.type windowed_context_switch_full, @function

# void windowed_context_switch_full(WindowedContext* old_ctx, WindowedContext* new_ctx)
# Arguments:
#   RDI: pointeur vers WindowedContext ancien
#   RSI: pointeur vers WindowedContext nouveau
windowed_context_switch_full:
    # === SAUVEGARDE (64 bytes) ===
    # Sauvegarde callee-saved registers sur la structure
    mov %rsp, 0(%rdi)            # RSP
    mov %rbp, 8(%rdi)            # RBP
    mov %rbx, 16(%rdi)           # RBX
    mov %r12, 24(%rdi)           # R12
    mov %r13, 32(%rdi)           # R13
    mov %r14, 40(%rdi)           # R14
    mov %r15, 48(%rdi)           # R15
    
    # Sauvegarde RIP (adresse de retour)
    pop %rax
    mov %rax, 56(%rdi)           # RIP
    
    # === RESTAURATION (64 bytes) ===
    mov 0(%rsi), %rsp            # RSP
    mov 8(%rsi), %rbp            # RBP
    mov 16(%rsi), %rbx           # RBX
    mov 24(%rsi), %r12           # R12
    mov 32(%rsi), %r13           # R13
    mov 40(%rsi), %r14           # R14
    mov 48(%rsi), %r15           # R15
    mov 56(%rsi), %rax           # RIP
    
    # Jump vers nouvelle adresse
    jmp *%rax

# Helper: Initialize windowed context for new thread
.global windowed_init_context
.type windowed_init_context, @function

# void windowed_init_context(WindowedContext* ctx, u64 stack_top, u64 entry_point)
# Arguments:
#   RDI: pointeur vers WindowedContext
#   RSI: sommet de la pile (stack_top)
#   RDX: point d'entrée (entry_point)
windowed_init_context:
    # Setup stack pointer
    mov %rsi, 0(%rdi)            # RSP = stack_top
    
    # Setup base pointer
    mov %rsi, 8(%rdi)            # RBP = stack_top
    
    # Clear callee-saved registers
    movq $0, 16(%rdi)            # RBX = 0
    movq $0, 24(%rdi)            # R12 = 0
    movq $0, 32(%rdi)            # R13 = 0
    movq $0, 40(%rdi)            # R14 = 0
    movq $0, 48(%rdi)            # R15 = 0
    
    # Set entry point
    mov %rdx, 56(%rdi)           # RIP = entry_point
    
    ret

.size windowed_context_switch, .-windowed_context_switch
.size windowed_context_switch_full, .-windowed_context_switch_full
.size windowed_init_context, .-windowed_init_context
