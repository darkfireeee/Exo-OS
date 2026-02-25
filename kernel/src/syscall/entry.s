/*
 * kernel/src/syscall/entry.s — Entrée syscall Exo-OS (SYSCALL/SYSRET, x86_64)
 *
 * Ce fichier documente l'entrée ASM du module syscall/ en référençant
 * explicitement l'implémentation principale dans arch/x86_64/syscall.rs.
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * ARCHITECTURE DE L'ENTRÉE SYSCALL
 * ═══════════════════════════════════════════════════════════════════════════
 *
 * L'entrée ASM effective est définie dans :
 *   kernel/src/arch/x86_64/syscall.rs → global_asm!("syscall_entry_asm")
 *
 * Raison : Rust `global_asm!` permet d'intégrer le code machine directement
 * dans l'objet .rlib du kernel, sans fichier .s séparé à compiler et linker.
 * Le point d'entrée `syscall_entry_asm` est déclaré `extern "C"` et son
 * adresse est chargée dans MSR_LSTAR par `arch::x86_64::syscall::init_syscall()`.
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * SÉQUENCE D'EXÉCUTION (référence pour lecture)
 * ═══════════════════════════════════════════════════════════════════════════
 *
 * [Ring 3 - userspace]
 *   SYSCALL instruction
 *     → CPU sauve RIP → RCX
 *     → CPU sauve RFLAGS → R11
 *     → CPU masque RFLAGS via MSR_SFMASK (IF, TF, DF, AC)
 *     → CPU charge CS:RIP depuis MSR_LSTAR  ← "syscall_entry_asm"
 *     → CPU charge SS depuis MSR_STAR[47:32] + 8
 *
 * [Ring 0 - kernel mode, GS = user GS avant SWAPGS]
 *   swapgs                          → GS = kernel GS
 *   mov gs:[0x08], rsp              → save user RSP
 *   mov rsp, gs:[0x00]              → load kernel RSP (RSP0 de la TSS)
 *   push rcx                        → [rsp+120] RIP retour
 *   push r11                        → [rsp+112] RFLAGS userspace
 *   push rbp                        → [rsp+104]
 *   push rbx                        → [rsp+ 96]
 *   push r12..r15                   → [rsp 64..88]
 *   push gs:[0x08]                  → [rsp+ 56] RSP userspace
 *   push rsi..rax                   → [rsp 0..48]  args + numéro syscall
 *   mov rbx, rsp                    → pointeur frame (callee-saved)
 *   and rsp, -16                    → alignement ABI AMD64
 *   mov rdi, rbx                    → arg1 = &SyscallFrame
 *   call syscall_rust_handler       → handler Rust principal
 *   mov rsp, rbx                    → restaurer rsp avant pops
 *   mov rax, [rsp]                  → valeur retour (frame.rax)
 *   [pop tous les registres...]
 *   mov rsp, gs:[0x08]              → restaurer RSP userspace
 *   swapgs                          → restaurer GS userspace
 *   sysretq                         → retour Ring 3
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * LAYOUT SyscallFrame (DOIT correspondre à arch/x86_64/syscall.rs)
 * ═══════════════════════════════════════════════════════════════════════════
 *
 * Offset  Registre  Utilisation
 * ──────  ────────  ───────────────────────────────────────────────────────
 *   0     rax       numéro syscall (entrée) / valeur retour (sortie)
 *   8     r9        arg6
 *  16     r8        arg5
 *  24     r10       arg4
 *  32     rdx       arg3
 *  40     rdi       arg1
 *  48     rsi       arg2
 *  56     (rsp)     RSP userspace sauvegardé
 *  64     r15       callee-saved
 *  72     r14       callee-saved
 *  80     r13       callee-saved
 *  88     r12       callee-saved
 *  96     rbx       callee-saved
 * 104     rbp       callee-saved
 * 112     r11       RFLAGS userspace (sauvé par SYSCALL hw)
 * 120     rcx       RIP retour userspace (sauvé par SYSCALL hw)
 *
 * Taille totale : 128 bytes (16 × 8)
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * MSR CONFIGURATION (arch/x86_64/syscall.rs → init_syscall())
 * ═══════════════════════════════════════════════════════════════════════════
 *
 * MSR_IA32_EFER  : bit SCE activé  (EFER_SCE = 0x0001)
 * MSR_STAR       : [47:32]=KERNEL_CS  [63:48]=USER_CS32
 * MSR_LSTAR      : adresse de syscall_entry_asm (mode 64-bit)
 * MSR_CSTAR      : adresse de syscall_cstar_noop (compat 32-bit, ENOSYS)
 * MSR_SFMASK     : masque IF(9) | TF(8) | DF(10) | AC(18)
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * PIPELINE RUST (appelé depuis syscall_rust_handler)
 * ═══════════════════════════════════════════════════════════════════════════
 *
 *   syscall_rust_handler(&mut SyscallFrame)
 *     └─ syscall::dispatch::dispatch(&mut SyscallFrame)
 *           ├─ fast_path::try_fast_path()   — O(1), <150 cycles
 *           ├─ compat::linux::translate_linux_nr()
 *           ├─ table::get_handler(nr)
 *           │     └─ handler(arg1..arg6) → i64
 *           └─ post_dispatch()
 *                 └─ signal::delivery::handle_pending_signals()  [si pending]
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * KPTI / PCID (future intégration)
 * ═══════════════════════════════════════════════════════════════════════════
 *
 * Lorsque KPTI sera activé, un switch CR3 sera inséré dans le stub ASM
 * AVANT d'accéder à la pile kernel (pour passer de la "shadow page table"
 * userspace à la page table kernel complète).
 *
 * Séquence KPTI (non implémentée, marquée pour implémentation future) :
 *   SWAPGS
 *   mov gs:[0x08], rsp              → save user RSP
 *   mov rsp, gs:[0x30]              → RSP trampoline (page partagée U/K)
 *   push rcx / push r11             → sur la trampoline stack
 *   mov rax, gs:[0x38]              → CR3 kernel (avec PCID flush bit)
 *   mov cr3, rax                    → switch CR3 kernel
 *   mov rsp, gs:[0x00]              → RSP kernel réel
 *   [push frame, call handler, pop frame...]
 *   mov rax, gs:[0x40]              → CR3 user (PCID non-flush)
 *   mov cr3, rax                    → retour page table user
 *   SWAPGS
 *   SYSRETQ
 *
 * ═══════════════════════════════════════════════════════════════════════════
 * SÉCURITÉ
 * ═══════════════════════════════════════════════════════════════════════════
 *
 * SPECTRE-V2 : L'IA32_SPEC_CTRL est lu/écrit par arch/x86_64/spectre/
 *              pour activer IBRS à l'entrée et le désactiver au retour.
 *              Non implémenté ici ; syscall_entry_asm le fera via spectre_mitigate_entry().
 *
 * SMEP : activé dans CR4 (boot) — empêche l'exécution d'adresses userspace en Ring 0.
 * SMAP : activé dans CR4 (boot) — empêche la lecture d'adresses userspace sans STAC.
 *        Les accès userspace dans validation.rs devront encadrer avec STAC/CLAC.
 */

/*
 * Aucun code ASM ici : l'implémentation réelle est dans
 * arch/x86_64/syscall.rs sous la forme `core::arch::global_asm!`.
 *
 * Ce fichier sert de documentation de référence et est inclus via
 * `build.rs` uniquement pour la documentation (jamais assemblé seul).
 */
