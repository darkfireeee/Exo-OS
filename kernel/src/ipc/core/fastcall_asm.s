# kernel/src/ipc/core/fastcall_asm.s
#
# ═══════════════════════════════════════════════════════════════════════════════
# FAST IPC — Inline ASM (évite le syscall complet)
# (Exo-OS · IPC Couche 2a · x86_64)
# ═══════════════════════════════════════════════════════════════════════════════
#
# Ces fonctions implémentent un fast path IPC pour les messages petits (<= 64B).
# Au lieu de passer par le syscall complet (SYSCALL/SYSRET + save context),
# on utilise un appel direct kernel-to-kernel via une gate dédiée.
#
# Cibles de performance :
#   ipc_fast_send  : < 200 ns sur x86_64 moderne (≈ 400 cycles @ 2GHz)
#   ipc_fast_recv  : < 200 ns
#
# CONVENTION D'APPEL (System V ABI) :
#   rdi = pointeur vers IpcFastMsg (struct caller-alloué sur la stack)
#   rsi = ChannelId (u64)
#   rax = code de retour (0 = succès, errno IPC sinon)
#   rdx = longueur reçue (pour ipc_fast_recv)
#
# REGISTRES CLOBBERÉS (non callee-saved) : rax, rcx, r10, r11
# REGISTRES PRÉSERVÉS (callee-saved) : rbx, rbp, r12-r15, rsp
#
# SAFETY : Ces fonctions ne doivent être appelées que depuis du code kernel
# avec les vérifications de capability déjà effectuées (voir channel/sync.rs).
# ═══════════════════════════════════════════════════════════════════════════════

.section .text
.global ipc_fast_send
.global ipc_fast_recv
.global ipc_fast_call
.global ipc_ring_fence

# ─────────────────────────────────────────────────────────────────────────────
# ipc_ring_fence — barrière mémoire légère pour ring IPC
#   Garantit que les stores précédents sont visibles avant l'incrémentation
#   du tail pointer du ring.
#   Équivalent de sfence — mais sfence est pour stores WC.
#   On utilise une barrière Release via LOCK XADD sur une variable dummy.
# ─────────────────────────────────────────────────────────────────────────────
.type ipc_ring_fence, @function
ipc_ring_fence:
    # MFENCE : garantit l'ordre total stores/loads — nécessaire pour MPMC.
    # Sur Skylake+, MFENCE coûte ~30-40 cycles. Acceptable pour un ring fence.
    mfence
    ret
.size ipc_ring_fence, . - ipc_ring_fence

# ─────────────────────────────────────────────────────────────────────────────
# ipc_fast_send — envoi rapide d'un message ≤ 64 bytes
#
#   Signature Rust :
#     extern "C" fn ipc_fast_send(
#         msg: *const IpcFastMsg,  // rdi
#         channel_id: u64,         // rsi
#     ) -> u64;                    // rax = 0 ou IpcError code
#
#   IpcFastMsg layout (64 bytes) :
#     [0..8]  : msg_id      (u64, zéro = allouer un nouveau)
#     [8..12] : flags       (u32)
#     [12..14]: len         (u16, ≤ 64 bytes)
#     [14..16]: _pad        (u16)
#     [16..80]: data        ([u8; 64])
# ─────────────────────────────────────────────────────────────────────────────
.type ipc_fast_send, @function
ipc_fast_send:
    # Sauvegarder les callee-saved utilisés comme temporaires.
    push    %rbx
    push    %r12

    # rdi = msg ptr, rsi = channel_id
    mov     %rdi, %rbx          # rbx = msg ptr (stable sur toute la fonction)
    mov     %rsi, %r12          # r12 = channel_id

    # Vérifier alignement msg ptr (mod 8 == 0).
    test    $7, %rdi
    jnz     .Lfast_send_einval

    # Charger len depuis msg+12 (u16).
    movzwl  12(%rbx), %ecx
    cmp     $64, %ecx
    ja      .Lfast_send_emsgsize

    # Appeler la fonction Rust de fast send via le ring SPSC.
    # La logique de tail pointer avancement est en Rust — on fait juste
    # le prefetch du slot suivant pour réduire la latence du prochain appel.
    # rdi déjà = msg ptr, rsi = channel_id (déjà set).
    # On appelle ipc_ring_fast_write() (fonction Rust no_mangle).
    call    ipc_ring_fast_write

    # rax = résultat (0 = OK, errno sinon).
    pop     %r12
    pop     %rbx
    ret

.Lfast_send_einval:
    mov     $10, %eax           # IpcError::InvalidParam = 10
    pop     %r12
    pop     %rbx
    ret

.Lfast_send_emsgsize:
    mov     $5, %eax            # IpcError::MessageTooLarge = 5
    pop     %r12
    pop     %rbx
    ret

.size ipc_fast_send, . - ipc_fast_send

# ─────────────────────────────────────────────────────────────────────────────
# ipc_fast_recv — réception rapide (polling, non bloquant)
#
#   Signature Rust :
#     extern "C" fn ipc_fast_recv(
#         dst: *mut IpcFastMsg,  // rdi
#         channel_id: u64,       // rsi
#     ) -> u64;                  // rax = 0 (ok) ou 1 (WouldBlock) ou errno
#
#   Retourne la longueur dans rdx si succès.
# ─────────────────────────────────────────────────────────────────────────────
.type ipc_fast_recv, @function
ipc_fast_recv:
    push    %rbx
    push    %r12

    # Vérifier alignement dst.
    test    $7, %rdi
    jnz     .Lfast_recv_einval

    mov     %rdi, %rbx
    mov     %rsi, %r12

    # Acquérir la barrière mémoire avant lecture du ring.
    # L'acquire est implicite dans les atomic loads Rust — on insère
    # un LFENCE ici pour les reads non-Rust (MMIO, prefetch manuel).
    lfence

    # Appeler la fonction Rust de fast recv via le ring SPSC.
    call    ipc_ring_fast_read

    # rax = résultat, rdx = longueur si succès.
    pop     %r12
    pop     %rbx
    ret

.Lfast_recv_einval:
    mov     $10, %eax           # IpcError::InvalidParam
    xor     %edx, %edx
    pop     %r12
    pop     %rbx
    ret

.size ipc_fast_recv, . - ipc_fast_recv

# ─────────────────────────────────────────────────────────────────────────────
# ipc_fast_call — envoi + attente réponse synchrone (RPC fast path)
#
#   Optimisation : évite two context switches en restant sur le CPU
#   si le serveur est sur le même cœur (donor scheduling).
#
#   Signature Rust :
#     extern "C" fn ipc_fast_call(
#         req: *const IpcFastMsg,  // rdi
#         rep: *mut IpcFastMsg,    // rsi
#         channel_id: u64,         // rdx
#         timeout_ns: u64,         // rcx
#     ) -> u64;                    // rax
# ─────────────────────────────────────────────────────────────────────────────
.type ipc_fast_call, @function
ipc_fast_call:
    push    %rbx
    push    %r12
    push    %r13
    push    %r14

    # Sauvegarder les arguments callee-clobber.
    mov     %rdi, %rbx          # req
    mov     %rsi, %r12          # rep
    mov     %rdx, %r13          # channel_id
    mov     %rcx, %r14          # timeout_ns

    # Vérification null/alignement.
    test    %rbx, %rbx
    jz      .Lfast_call_einval
    test    %r12, %r12
    jz      .Lfast_call_einval
    test    $7, %rbx
    jnz     .Lfast_call_einval
    test    $7, %r12
    jnz     .Lfast_call_einval

    # 1. Envoyer la requête.
    mov     %rbx, %rdi
    mov     %r13, %rsi
    call    ipc_ring_fast_write
    test    %rax, %rax
    jnz     .Lfast_call_err

    # 2. Attendre la réponse (polling + yield si timeout).
    mov     %r12, %rdi
    mov     %r13, %rsi
    mov     %r14, %rdx
    call    ipc_ring_fast_wait_reply

    pop     %r14
    pop     %r13
    pop     %r12
    pop     %rbx
    ret

.Lfast_call_einval:
    mov     $10, %eax
    pop     %r14
    pop     %r13
    pop     %r12
    pop     %rbx
    ret

.Lfast_call_err:
    pop     %r14
    pop     %r13
    pop     %r12
    pop     %rbx
    ret

.size ipc_fast_call, . - ipc_fast_call
