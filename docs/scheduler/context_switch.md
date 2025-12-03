# ⚡ Context Switch - 304 Cycles

## Objectif

Le context switch d'Exo-OS cible **304 cycles** (vs ~1500 cycles pour Linux).

## Architecture Windowed

```
┌─────────────────────────────────────────────────────────────┐
│                   Windowed Context Switch                    │
├─────────────────────────────────────────────────────────────┤
│  Save Window (registres minimaux)    │  ~100 cycles         │
│  Switch Stack Pointer                │  ~4 cycles           │
│  Restore Window                      │  ~100 cycles         │
│  Flush TLB (si nécessaire)           │  ~100 cycles         │
│  Total                               │  ~304 cycles         │
└─────────────────────────────────────────────────────────────┘
```

## Registres Sauvegardés

### Window Minimal (Hot Path)

```rust
#[repr(C)]
pub struct ThreadContext {
    // Callee-saved registers only
    pub rbx: u64,
    pub rbp: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    
    // Stack pointer
    pub rsp: u64,
    
    // Instruction pointer (return address)
    pub rip: u64,
    
    // Flags (minimal)
    pub rflags: u64,
}
```

**Total**: 72 octets (9 registres × 8 octets)

### Full Context (si nécessaire)

```rust
#[repr(C)]
pub struct FullContext {
    // GPRs
    pub rax: u64, pub rbx: u64, pub rcx: u64, pub rdx: u64,
    pub rsi: u64, pub rdi: u64, pub rbp: u64, pub rsp: u64,
    pub r8: u64,  pub r9: u64,  pub r10: u64, pub r11: u64,
    pub r12: u64, pub r13: u64, pub r14: u64, pub r15: u64,
    
    // Control
    pub rip: u64,
    pub rflags: u64,
    pub cs: u64,
    pub ss: u64,
    
    // FPU/SSE (lazy save)
    pub fxsave: [u8; 512],
}
```

## Implémentation Assembly

```asm
; windowed_switch(old_ctx: *mut ThreadContext, new_ctx: *const ThreadContext)
global windowed_switch
windowed_switch:
    ; Save callee-saved registers
    mov [rdi + 0],  rbx
    mov [rdi + 8],  rbp
    mov [rdi + 16], r12
    mov [rdi + 24], r13
    mov [rdi + 32], r14
    mov [rdi + 40], r15
    
    ; Save stack pointer
    mov [rdi + 48], rsp
    
    ; Save return address
    mov rax, [rsp]
    mov [rdi + 56], rax
    
    ; Save flags
    pushfq
    pop rax
    mov [rdi + 64], rax
    
    ; ---- SWITCH POINT ----
    
    ; Restore flags
    mov rax, [rsi + 64]
    push rax
    popfq
    
    ; Restore callee-saved registers
    mov rbx, [rsi + 0]
    mov rbp, [rsi + 8]
    mov r12, [rsi + 16]
    mov r13, [rsi + 24]
    mov r14, [rsi + 32]
    mov r15, [rsi + 40]
    
    ; Restore stack pointer
    mov rsp, [rsi + 48]
    
    ; Jump to new thread
    jmp [rsi + 56]
```

## Optimisations

### 1. Lazy FPU Save

Les registres FPU/SSE ne sont sauvegardés que si le thread les utilise:

```rust
if thread.uses_fpu() {
    fxsave(old_ctx.fxsave.as_mut_ptr());
}
```

### 2. TLB Flush Évité

Si les deux threads partagent le même espace d'adressage:

```rust
if old_thread.page_table == new_thread.page_table {
    // Pas de flush TLB nécessaire
} else {
    // Flush TLB partiel (PCID si disponible)
    flush_tlb_pcid(new_thread.pcid);
}
```

### 3. Cache Warming

Préchargement du contexte du nouveau thread:

```rust
prefetch_read(&new_thread.context);
prefetch_read(new_thread.stack_top);
```

## Comparaison

| Aspect | Exo-OS | Linux |
|--------|--------|-------|
| Cycles total | 304 | ~1500 |
| Registres sauvés | 9 | 16+ |
| FPU save | Lazy | Toujours |
| TLB flush | PCID | Souvent full |
| Préemption overhead | ~50 | ~300 |

## API

```rust
// Switch manuel (yield)
pub fn yield_now() {
    let old_ctx = &mut current_thread().context;
    let new_thread = scheduler.pick_next();
    let new_ctx = &new_thread.context;
    
    unsafe {
        windowed_switch(old_ctx, new_ctx);
    }
}

// Switch par timer (préemption)
pub extern "C" fn timer_interrupt() {
    if scheduler.should_preempt() {
        yield_now();
    }
}
```
