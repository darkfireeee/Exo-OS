# src/arch/x86_64/boot.asm
# Entrée assembleur (setup minimal)

.section .text
.global start

start:
    # Désactiver les interruptions pour un boot propre
    cli

    # Charger le pointeur de pile (défini dans linker.ld)
    mov rsp, stack_top

    # Appeler la fonction C `kmain`
    extern kmain
    call kmain

.hang:
    # Si kmain retourne, on attend
    hlt
    jmp .hang

.section .bss
.align 16
stack_bottom:
    .skip 8192 # 8 KiB pour la pile
stack_top: