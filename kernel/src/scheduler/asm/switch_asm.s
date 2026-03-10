// ═══════════════════════════════════════════════════════════════════════════════
// CONTEXT SWITCH ASM — Exo-OS Scheduler (x86_64)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE SWITCH-ASM (DOC3) :
//   • Sauvegarder tous les registres callee-saved ABI System V :
//     rbx, rbp, r12, r13, r14, r15, rsp
//   • Sauvegarder MXCSR + x87 FCW EXPLICITEMENT (pas via XSAVE)
//     Raison : du code Rust compilé avec optimisations SSE peut corrompre MXCSR
//              entre deux threads si on ne le sauvegarde pas dans switch_asm.
//   • CR3 switché ICI, AVANT la restauration des registres (KPTI atomique)
//   • GARANTIE FORMELLE r15 : push AVANT tout appel C
//     (ext4plus/inode/ops.rs utilise r15 — doit être préservé)
//
// Signature (System V ABI) :
//   context_switch_asm(
//     old_kernel_rsp: *mut u64,   // rdi
//     new_kernel_rsp: u64,        // rsi
//     new_cr3:        u64,        // rdx  (0 = pas de switch CR3)
//   )
//
// ═══════════════════════════════════════════════════════════════════════════════

.section .text
.global context_switch_asm
.type context_switch_asm, @function

context_switch_asm:
    // ── Sauvegarder les registres callee-saved du thread SORTANT ──────────────
    // ORDRE OBLIGATOIRE : r15 EN PREMIER (GARANTIE FORMELLE pour ext4plus/inode/ops.rs)
    pushq   %r15
    pushq   %r14
    pushq   %r13
    pushq   %r12
    pushq   %rbp
    pushq   %rbx

    // Sauvegarder MXCSR (contrôle SSE) — 4 bytes via stmxcsr
    subq    $16, %rsp               // Réserver 16 bytes (alignement 16B requis)
    stmxcsr 0(%rsp)

    // Sauvegarder x87 FCW (contrôle FP) — 2 bytes via fstcw
    fstcw   8(%rsp)

    // Sauvegarder le stack pointer kernel du thread SORTANT dans son TCB.
    // rdi = pointeur vers TCB::kernel_rsp du thread sortant
    movq    %rsp, (%rdi)

    // ── Switch CR3 si nécessaire (KPTI) ──────────────────────────────────────
    // rdx = new_cr3 (0 = même espace d'adressage, pas de TLB flush)
    testq   %rdx, %rdx
    jz      .L_skip_cr3

    // Switch CR3 atomique — invalide TLB user automatiquement.
    // CR3 est switché AVANT la restauration des registres (KPTI invariant).
    movq    %rdx, %cr3

.L_skip_cr3:
    // ── Charger le stack pointer kernel du thread ENTRANT ────────────────────
    // rsi = new_kernel_rsp
    movq    %rsi, %rsp

    // ── Restaurer x87 FCW et MXCSR du thread ENTRANT ────────────────────────
    fldcw   8(%rsp)                 // Restaurer x87 FCW en premier
    ldmxcsr 0(%rsp)                 // Restaurer MXCSR
    addq    $16, %rsp               // Libérer les 16 bytes réservés

    // ── Restaurer les registres callee-saved du thread ENTRANT ───────────────
    popq    %rbx
    popq    %rbp
    popq    %r12
    popq    %r13
    popq    %r14
    popq    %r15

    // Retour — continue dans le contexte du nouveau thread.
    // Le ret consomme l'adresse de retour qui fut pushée lors de l'appel
    // context_switch_asm() du thread ENTRANT (la fois où il était sortant).
    ret

.size context_switch_asm, . - context_switch_asm


// ═══════════════════════════════════════════════════════════════════════════════
// SWITCH VERS UN NOUVEAU THREAD — Premier démarrage (jamais switché avant)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Utilisé lors de la création d'un thread. Le stack kernel du nouveau thread
// est préparé avec l'adresse de `thread_entry_trampoline` comme adresse de retour.
//
// Signature :
//   switch_to_new_thread(
//     old_kernel_rsp: *mut u64,   // rdi — NULL si bootstrap
//     new_kernel_rsp: u64,        // rsi
//     new_cr3:        u64,        // rdx
//   )
//
// ═══════════════════════════════════════════════════════════════════════════════

.section .text
.global switch_to_new_thread
.type switch_to_new_thread, @function

switch_to_new_thread:
    // Sauvegarder prev seulement si old_kernel_rsp != NULL
    testq   %rdi, %rdi
    jz      .L_no_save

    pushq   %r15
    pushq   %r14
    pushq   %r13
    pushq   %r12
    pushq   %rbp
    pushq   %rbx
    subq    $16, %rsp
    stmxcsr 0(%rsp)
    fstcw   8(%rsp)
    movq    %rsp, (%rdi)

.L_no_save:
    // Switch CR3 si nécessaire
    testq   %rdx, %rdx
    jz      .L_new_no_cr3
    movq    %rdx, %cr3

.L_new_no_cr3:
    // Charger le nouveau stack pointer
    movq    %rsi, %rsp

    // Pour un tout nouveau thread, le "stack" a été préparé par le code Rust
    // de création de thread avec le layout suivant (SANS la zone MXCSR+FCW) :
    //   [kernel_rsp+ 0] = rbx initial (0)
    //   [kernel_rsp+ 8] = rbp initial (0)
    //   [kernel_rsp+16] = r12 initial (0)
    //   [kernel_rsp+24] = r13 initial (0)
    //   [kernel_rsp+32] = r14 initial (0)
    //   [kernel_rsp+40] = r15 initial (0)
    //   [kernel_rsp+48] = adresse de retour → thread_entry_trampoline
    //
    // On dépile les 6 registres (48 bytes), puis ret saute vers thread_entry_trampoline.
    popq    %rbx
    popq    %rbp
    popq    %r12
    popq    %r13
    popq    %r14
    popq    %r15
    // Les 16 bytes MXCSR+FCW ne sont pas sur le stack initial → skip
    ret

.size switch_to_new_thread, . - switch_to_new_thread


// ═══════════════════════════════════════════════════════════════════════════════
// KTHREAD TRAMPOLINE — Premier démarrage d'un kthread (Exo-OS)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Appelé lors du PREMIER context_switch vers un kthread nouvellement créé.
//
// Context : context_switch_asm a restauré le frame préparé par create_kthread() :
//   r12 = entry_fn   (fn(usize)->!)   ← slot r12 du frame stack
//   r13 = arg        (usize)          ← slot r13 du frame stack
//
// Convention SystemV AMD64 :
//   Premier argument entier → rdi
//
// Ce trampoline déplace l'arg dans rdi puis saute à entry_fn.
// entry_fn est déclarée -> ! donc ne retourne JAMAIS.
// ═══════════════════════════════════════════════════════════════════════════════

.section .text
.global kthread_trampoline
.type kthread_trampoline, @function

kthread_trampoline:
    movq    %r13, %rdi      // arg → rdi (1er paramètre SystemV AMD64)
    jmpq    *%r12           // saute à entry_fn(arg) — ne revient jamais

.size kthread_trampoline, . - kthread_trampoline


// ═══════════════════════════════════════════════════════════════════════════════
// FORK CHILD TRAMPOLINE — Premier démarrage d'un processus fils (fork)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Appelé par switch_to_new_thread lors du PREMIER ordonnancement d'un processus
// créé via do_fork(). Le stack kernel du fils a été préparé par do_fork() :
//
//   [kernel_rsp + 0..48)  = 6 registres callee-saved = 0 (format switch_to_new_thread)
//   [kernel_rsp + 48]     = fork_child_trampoline  ← adresse de retour switch_to_new_thread
//   [kernel_rsp + 56]     = child_rip  ← RSP pointe ici à l'entrée du trampoline
//   [kernel_rsp + 64]     = 0x1B      (CS ring3)
//   [kernel_rsp + 72]     = 0x0202    (RFLAGS : IF=1)
//   [kernel_rsp + 80]     = child_rsp (RSP userspace)
//   [kernel_rsp + 88]     = 0x23      (SS ring3)
//
// Invariant : GS = kernel GS (le scheduler n'a PAS fait SWAPGS).
//             On doit faire SWAPGS avant IRETQ pour restaurer GS userspace.
//
// RÈGLE PROC-08 : TLB parent flushé dans do_fork() — aucun flush ici nécessaire.
// ═══════════════════════════════════════════════════════════════════════════════

.section .text
.global fork_child_trampoline
.type fork_child_trampoline, @function

fork_child_trampoline:
    xor     %eax, %eax      // rax = 0 : le fils retourne 0 de fork()
    swapgs                  // restaurer GS userspace (noyau avait GS kernel)
    iretq                   // dépile RIP, CS, RFLAGS, RSP, SS → retour Ring3

.size fork_child_trampoline, . - fork_child_trampoline
