# kernel/src/ipc/core/fastcall_asm.s
#
# ═══════════════════════════════════════════════════════════════════════════════
# FAST IPC — Inline ASM (avoids full syscall)
# (Exo-OS · IPC Layer 2a · x86_64)
# ═══════════════════════════════════════════════════════════════════════════════
#
# These functions implement a fast path IPC for small messages (<= 64B).
# Instead of going through a full syscall (SYSCALL/SYSRET + save context),
# a direct kernel-to-kernel call is used via a dedicated gate.
#
# Performance targets:
#   ipc_fast_send  : < 200 ns on modern x86_64 (≈ 400 cycles @ 2GHz)
#   ipc_fast_recv  : < 200 ns
#
# CALLING CONVENTION (System V ABI):
#   rdi = pointer to IpcFastMsg (caller-allocated struct on stack)
#   rsi = ChannelId (u64)
#   rax = return code (0 = success, IPC errno otherwise)
#   rdx = received length (for ipc_fast_recv)
#
# CLOBBERED REGISTERS (caller-saved): rax, rcx, r10, r11
# PRESERVED REGISTERS (callee-saved): rbx, rbp, r12-r15, rsp
#
# SAFETY: These functions must only be called from kernel code
# with capability checks already performed (see channel/sync.rs).
# ═══════════════════════════════════════════════════════════════════════════════

.section .text
.global ipc_fast_send
.global ipc_fast_recv
.global ipc_fast_call
.global ipc_ring_fence

# ─────────────────────────────────────────────────────────────────────────────
# ipc_ring_fence — lightweight memory barrier for IPC ring
#   Guarantees that previous stores are visible before incrementing
#   the ring's tail pointer.
#   Equivalent to sfence — but sfence is for WC stores.
#   We use a Release barrier via LOCK XADD on a dummy variable.
# ─────────────────────────────────────────────────────────────────────────────
.type ipc_ring_fence, @function
ipc_ring_fence:
    # MFENCE: guarantees total store/load order — required for MPMC.
    # On Skylake+, MFENCE costs ~30-40 cycles. Acceptable for a ring fence.
    mfence
    ret
.size ipc_ring_fence, . - ipc_ring_fence

# ─────────────────────────────────────────────────────────────────────────────
# ipc_fast_send — fast send of a message <= 64 bytes
#
#   Rust signature:
#     extern "C" fn ipc_fast_send(
#         msg: *const IpcFastMsg,  // rdi
#         channel_id: u64,         // rsi
#     ) -> u64;                    // rax = 0 or IpcError code
#
#   IpcFastMsg layout (64 bytes):
#     [0..8]  : msg_id      (u64, zero = allocate a new one)
#     [8..12] : flags       (u32)
#     [12..14]: len         (u16, <= 64 bytes)
#     [14..16]: _pad        (u16)
#     [16..80]: data        ([u8; 64])
# ─────────────────────────────────────────────────────────────────────────────
.type ipc_fast_send, @function
ipc_fast_send:
    # Save callee-saved registers used as temporaries.
    push    %rbx
    push    %r12

    # rdi = msg ptr, rsi = channel_id
    mov     %rdi, %rbx          # rbx = msg ptr (stable across entire function)
    mov     %rsi, %r12          # r12 = channel_id

    # Check msg ptr alignment (mod 8 == 0).
    test    $7, %rdi
    jnz     .Lfast_send_einval

    # Load len from msg+12 (u16).
    movzwl  12(%rbx), %ecx
    cmp     $64, %ecx
    ja      .Lfast_send_emsgsize

    # Call Rust fast send function via SPSC ring.
    # Tail pointer advancement logic is in Rust — we only perform
    # next slot prefetching to reduce latency of the next call.
    # rdi already = msg ptr, rsi = channel_id (already set).
    # Call ipc_ring_fast_write() (Rust no_mangle function).
    call    ipc_ring_fast_write

    # rax = result (0 = OK, errno otherwise).
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
# ipc_fast_recv — fast receive (polling, non-blocking)
#
#   Rust signature:
#     extern "C" fn ipc_fast_recv(
#         dst: *mut IpcFastMsg,  // rdi
#         channel_id: u64,       // rsi
#     ) -> u64;                  // rax = 0 (ok) or 1 (WouldBlock) or errno
#
#   Returns length in rdx on success.
# ─────────────────────────────────────────────────────────────────────────────
.type ipc_fast_recv, @function
ipc_fast_recv:
    push    %rbx
    push    %r12

    # Check dst alignment.
    test    $7, %rdi
    jnz     .Lfast_recv_einval

    mov     %rdi, %rbx
    mov     %rsi, %r12

    # Acquire memory barrier before reading ring.
    # Acquire is implicit in Rust atomic loads — we insert
    # an LFENCE here for non-Rust reads (MMIO, manual prefetch).
    lfence

    # Call Rust fast recv function via SPSC ring.
    call    ipc_ring_fast_read

    # rax = result, rdx = length on success.
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
# ipc_fast_call — send + wait for synchronous reply (RPC fast path)
#
#   Optimization: avoids two context switches by remaining on CPU
#   if the server is on the same core (donor scheduling).
#
#   Rust signature:
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

    # Save caller arguments.
    mov     %rdi, %rbx          # req
    mov     %rsi, %r12          # rep
    mov     %rdx, %r13          # channel_id
    mov     %rcx, %r14          # timeout_ns

    # Null/alignment check.
    test    %rbx, %rbx
    jz      .Lfast_call_einval
    test    %r12, %r12
    jz      .Lfast_call_einval
    test    $7, %rbx
    jnz     .Lfast_call_einval
    test    $7, %r12
    jnz     .Lfast_call_einval

    # 1. Send request.
    mov     %rbx, %rdi
    mov     %r13, %rsi
    call    ipc_ring_fast_write
    test    %rax, %rax
    jnz     .Lfast_call_err

    # 2. Wait for reply (polling + yield on timeout).
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
