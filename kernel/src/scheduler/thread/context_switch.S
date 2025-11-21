# src/scheduler/context_switch.S
#
# Routine de changement de contexte pour l'architecture x86_64.
# Cette fonction est optimisée pour être la plus rapide possible.
#
# Convention d'appel (System V AMD64 ABI):
# - RDI : premier argument (pointeur vers l'ancien contexte)
# - RSI : deuxième argument (nouveau pointeur de pile RSP)
#
# La fonction sauvegarde l'état complet du thread actuel,
# puis restaure l'état du nouveau thread.

.section .text
.global context_switch
.p2align 4, 0x90

# RDI = *mut ThreadContext (pointeur vers l'ancien contexte)
# RSI = VirtAddr (nouveau RSP)
context_switch:
    # --- Sauvegarde du contexte de l'ancien thread ---
    
    # Si le pointeur vers l'ancien contexte est nul, cela signifie que c'est
    # le premier lancement d'un thread (pas d'ancien contexte à sauvegarder).
    test %rdi, %rdi
    jz .Lrestore_context

    # Sauvegarder tous les registres généraux (callee-saved) sur la pile actuelle.
    push %rbp
    push %rbx
    push %r12
    push %r13
    push %r14
    push %r15

    # Sauvegarder le pointeur de pile actuel (RSP) dans la structure de l'ancien thread.
    # La structure ThreadContext ne contient que RSP, on stocke donc notre RSP actuel
    # à l'adresse pointée par RDI.
    mov %rsp, (%rdi)

.p2align 4, 0x90
.Lrestore_context:
    # --- Restauration du contexte du nouveau thread ---

    # Charger le nouveau pointeur de pile depuis RSI.
    mov %rsi, %rsp

    # Restaurer les registres depuis la nouvelle pile.
    pop %r15
    pop %r14
    pop %r13
    pop %r12
    pop %rbx
    pop %rbp

    # Retourner au nouveau thread.
    # L'instruction `ret` dépile l'adresse de retour (RIP) de la pile.
    # Pour un thread fraîchement créé, cette adresse est celle de sa fonction d'entrée.
    ret