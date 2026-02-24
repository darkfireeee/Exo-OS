# Scheduler FPU — Lazy FPU, XSAVE/XRSTOR, FpuState

> **Sources** : `kernel/src/scheduler/fpu/`  
> **Règles** : SCHED-09, SCHED-16  
> **Principe** : Aucune instruction FPU dans scheduler/ — tout délégué via FFI vers `arch/x86_64/cpu/fpu.rs`

---

## Table des matières

1. [Mécanisme Lazy FPU](#1-mécanisme-lazy-fpu)
2. [lazy.rs — CR0.TS et handler #NM](#2-lazyrs--cr0ts-et-handler-nm)
3. [save_restore.rs — XSAVE/XRSTOR via FFI](#3-save_restorerss--xsavexrstor-via-ffi)
4. [state.rs — FpuState layout](#4-staters--fpustate-layout)
5. [Flux complet Lazy FPU](#5-flux-complet-lazy-fpu)

---

## 1. Mécanisme Lazy FPU

Le **Lazy FPU** évite de sauvegarder/restaurer l'état FPU à chaque commutation de contexte en exploitant le bit TS (Task Switched) du registre CR0.

### Principe

```
context_switch(prev → next) :
  1. Sauvegarde FPU de prev si fpu_loaded == true
  2. Fixe next.fpu_loaded = false
  3. SET CR0.TS = 1  (mark_fpu_not_loaded)

Prochain accès FPU par next :
  4. Exception #NM (Device Not Available) levée par le CPU
  5. Handler #NM → sched_fpu_handle_nm(tcb_ptr)
  6. CLTS (clear TS)
  7. Restauration état FPU de next (XRSTOR)
  8. fpu_loaded = true
  9. Reprise de l'instruction FPU
```

**Avantage** : si un thread ne touche pas au FPU, aucun coût de sauvegarde/restauration.

---

## 2. lazy.rs — CR0.TS et handler #NM

### Initialisation

```rust
pub fn init()
```
- Appelle `cr0_set_ts()` sur le CPU courant.
- Positionne `LAZY_FPU_INIT: AtomicBool = true`.

### Primitives CR0

```rust
// Met CR0.TS = 1 → prochain accès FPU lèvera #NM
pub unsafe fn cr0_set_ts()

// Met CR0.TS = 0 → FPU accessible
pub unsafe fn cr0_clear_ts()

// Lit l'état actuel de CR0.TS
pub unsafe fn cr0_ts_is_set() -> bool
```

Implémentation (inline ASM via LLVM) :
```rust
// cr0_set_ts :
asm!("mov %cr0, {0}", "or $8, {0}", "mov {0}, %cr0", out(reg) _)

// cr0_clear_ts :
asm!("clts")
```

### mark_fpu_not_loaded

```rust
pub unsafe fn mark_fpu_not_loaded()
```
- Appelle `cr0_set_ts()`.
- Utilisé dans `context_switch()` après le switch pour marquer que le FPU de `next` n'est pas chargé.

### handle_nm_exception

```rust
pub unsafe fn handle_nm_exception(tcb: &mut ThreadControlBlock)
```
Séquence :
1. `cr0_clear_ts()` — CLTS pour permettre l'accès FPU
2. Si `tcb.fpu_state_ptr.is_null()` → `alloc_fpu_state(tcb)` (premier accès)
3. `xrstor_for(tcb)` — Restauration XSAVE depuis `fpu_state_ptr`
4. `tcb.set_fpu_loaded(true)`

### Export C ABI (pont arch→sched, SCHED-16)

```rust
#[no_mangle]
pub unsafe extern "C" fn sched_fpu_handle_nm(tcb_ptr: *mut u8) {
    let tcb = &mut *(tcb_ptr as *mut ThreadControlBlock);
    handle_nm_exception(tcb);
}
```

Appelé depuis le handler IDT de l'exception #NM dans `arch/x86_64/exceptions.rs`.

### Utilitaires

```rust
pub fn is_fpu_used(tcb: &ThreadControlBlock) -> bool
    // → tcb.fpu_state_ptr != null && tcb.fpu_loaded()

pub fn is_initialized() -> bool
    // → LAZY_FPU_INIT.load(Relaxed)
```

---

## 3. save_restore.rs — XSAVE/XRSTOR via FFI

### FFI vers arch/ (SCHED-16)

```rust
extern "C" {
    // Sauvegarde l'état XSAVE dans le buffer pointé par ptr
    // mask : XCR0 bitmask (quels composants sauvegarder)
    fn arch_xsave64(ptr: *mut u8, mask_lo: u32, mask_hi: u32);

    // Restauration depuis buffer XSAVE
    fn arch_xrstor64(ptr: *const u8, mask_lo: u32, mask_hi: u32);

    // Fallback FXSAVE (512 octets, SSE uniquement)
    fn arch_fxsave64(ptr: *mut u8);
    fn arch_fxrstor64(ptr: *const u8);

    // Détection matérielle
    fn arch_has_xsave() -> bool;   // CPUID.01H:ECX.XSAVE[26]
    fn arch_has_avx()   -> bool;   // CPUID.01H:ECX.AVX[28]
}
```

### Initialisation

```rust
pub fn init()
```
- Appelle `arch_has_xsave()` une fois, stocke dans `HAS_XSAVE: AtomicBool`.
- Appelle `detect_xsave_size()` pour initialiser `XSAVE_AREA_SIZE`.

### xsave_current

```rust
pub unsafe fn xsave_current(tcb: &mut ThreadControlBlock)
```
1. Si `tcb.fpu_state_ptr.is_null()` → return (jamais utilisé)
2. Si `HAS_XSAVE` → `arch_xsave64(ptr, 0xFFFFFFFF, 0xFFFFFFFF)` (tous composants)
3. Sinon → `arch_fxsave64(ptr)`

Appelé dans `context_switch()` AVANT la commutation (SCHED-09).

### xrstor_for

```rust
pub unsafe fn xrstor_for(tcb: &mut ThreadControlBlock)
```
1. Si `tcb.fpu_state_ptr.is_null()` → `alloc_fpu_state(tcb)` + init à zéro
2. Si `HAS_XSAVE` → `arch_xrstor64(ptr, 0xFFFFFFFF, 0xFFFFFFFF)`
3. Sinon → `arch_fxrstor64(ptr)`
4. `tcb.set_fpu_loaded(true)`

### alloc_fpu_state

```rust
pub unsafe fn alloc_fpu_state(tcb: &mut ThreadControlBlock) -> bool
```
- Alloue `XSAVE_AREA_SIZE` octets alignés sur 64 octets via `__rust_alloc`.
- Initialise le buffer à zéro (état FPU reset propre).
- Stocke le pointeur dans `tcb.fpu_state_ptr`.
- Retourne `false` si allocation échoue (OOM kernel).

### Cache de détection

```rust
static HAS_XSAVE: AtomicBool = AtomicBool::new(false);
```
Initialisé une seule fois dans `init()`, lu en Relaxed ensuite.

---

## 4. state.rs — FpuState layout

### Constantes de taille

```rust
pub const FXSAVE_SIZE:       usize = 512;   // FXSAVE legacy (SSE)
pub const XSAVE_AVX_SIZE:    usize = 832;   // XSAVE + AVX (YMM 256 bits)
pub const XSAVE_AVX512_SIZE: usize = 2688;  // XSAVE + AVX-512 (ZMM 512 bits)
pub const FPU_STATE_MAX_SIZE:usize = 2688;  // Taille buffer maximale

pub static XSAVE_AREA_SIZE: AtomicUsize = AtomicUsize::new(512);
```

### detect_xsave_size

```rust
pub fn detect_xsave_size()
```
Logique :
```
Si !arch_has_xsave() → XSAVE_AREA_SIZE = 512 (FXSAVE)
Sinon si arch_has_avx512() → XSAVE_AREA_SIZE = 2688
Sinon si arch_has_avx()    → XSAVE_AREA_SIZE = 832
Sinon                       → XSAVE_AREA_SIZE = 512
```

### FpuState

```rust
#[repr(C, align(64))]
pub struct FpuState {
    buffer:       [u8; FPU_STATE_MAX_SIZE],  // 2688 octets
    active_size:  usize,                      // Taille effective utilisée
    generation:   u64,                        // Numéro de génération (debug)
}
```

**Note** : `FpuState` n'est pas stocké **dans** le TCB (TCB = 128 B max). Il est alloué dynamiquement et son pointeur stocké dans `tcb.fpu_state_ptr`.

### API FpuState

```rust
impl FpuState {
    pub const fn new() -> Self
    pub fn as_mut_ptr(&mut self) -> *mut u8
    pub fn as_ptr(&self)    -> *const u8
    pub fn refresh_size(&mut self)   // Met active_size = XSAVE_AREA_SIZE.load()
    pub fn size(&self) -> usize      // → active_size
}
```

### Layout XSAVE area (x86-64)

```
Offset   Size    Contenu
  0       512    Legacy FXSAVE (x87 + XMM0-15)
  512     64     XSAVE header (XSTATE_BV bitmask)
  576     256    YMM high (bits 255:128 de YMM0-15)  → si AVX
  832    1856    ZMM high (bits 511:256) + k0-k7     → si AVX-512
 2688    ← fin
```

---

## 5. Flux complet Lazy FPU

```
Thread A (FPU actif) → Thread B (FPU jamais utilisé)
─────────────────────────────────────────────────────────

1. context_switch(A, B) appelé :
   A.fpu_loaded = true
   → xsave_current(A)          ← arch_xsave64(A.fpu_state_ptr, …)
   → set_state(A, Runnable)
   → context_switch_asm(…)     ← switch registres, CR3
   → set_state(B, Running)
   → B.set_fpu_loaded(false)
   → mark_fpu_not_loaded()     ← cr0_set_ts() = 1

2. B exécute une instruction normale (pas FPU) : OK, aucun coût

3. Thread A (FPU actif) → Thread C (FPU déjà utilisé)
   C.fpu_loaded = false (mis à false lors du switch précédent)
   → xsave_current(A)
   → context_switch_asm(…)
   → C.set_fpu_loaded(false)
   → mark_fpu_not_loaded()

4. C essaie : ADDSS xmm0, xmm1
   → CR0.TS=1 → CPU lève #NM
   → arch IDT[7] → sched_fpu_handle_nm(C_tcb_ptr)
     → handle_nm_exception(C) :
         cr0_clear_ts()           ← CLTS
         xrstor_for(C)            ← arch_xrstor64(C.fpu_state_ptr, …)
         C.set_fpu_loaded(true)
   → Reprise de ADDSS xmm0, xmm1 ← instruction rejouée automatiquement

Coût Lazy FPU :
  • Thread sans FPU : 0 instruction FPU supplémentaire
  • Thread avec FPU : 1 XSAVE + 1 XRSTOR par commutation (seulement si utilisé)
  • Premier accès FPU : 1 #NM exception (acceptable)
```
