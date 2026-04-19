# CORR-59 — fastcall_asm.s jamais inclus + arch_cpu_relax indéfini (CRIT-08 + CRIT-09)

**Sévérité :** 🔴 CRITIQUE — Link error garanti (build cassé)  
**Fichiers :** `kernel/src/ipc/core/mod.rs`, `kernel/src/ipc/ring/spsc.rs`, `kernel/build.rs`  
**Impact :** Tout binaire utilisant l'IPC fast-path produit des symboles non résolus au link

---

## Problème CRIT-08 — fastcall_asm.s

```rust
// kernel/src/ipc/core/mod.rs:21,52 — déclarations extern "C"
// Les fonctions ipc_fast_send, ipc_fast_recv, ipc_fast_call sont déclarées
// mais aucun global_asm!(include_str!("fastcall_asm.s")) n'existe.
// build.rs ne référence que switch_asm.s et fast_path.s.
```

## Problème CRIT-09 — arch_cpu_relax

```rust
// kernel/src/ipc/ring/spsc.rs:359
extern "C" { fn arch_cpu_relax(); }
arch_cpu_relax(); // symbole introuvable dans tout le codebase
```

---

## Correction CRIT-08 — Inclure fastcall_asm.s

### Option A — `global_asm!` dans `ipc/core/mod.rs` (recommandé)

```rust
// kernel/src/ipc/core/mod.rs — AJOUTER en tête de fichier après les use

// Inclusion du fichier ASM fast-path IPC.
// Les symboles ipc_fast_send / ipc_fast_recv / ipc_fast_call sont définis ici.
#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(include_str!("fastcall_asm.s"));
```

### Option B — Si le fichier ASM référence des symboles Rust qui n't existent pas encore

Vérifier d'abord que `fastcall_asm.s` compile sans erreur :

```bash
# Tester l'inclusion isolée
as --64 -o /tmp/fastcall.o kernel/src/ipc/core/fastcall_asm.s 2>&1
```

Si des symboles sont manquants dans l'ASM :

```asm
# kernel/src/ipc/core/fastcall_asm.s — ajouter si absent
# Stub minimal pour ipc_fast_send / ipc_fast_recv / ipc_fast_call
# en attendant l'implémentation complète Phase 3.

.section .text
.global ipc_fast_send
.global ipc_fast_recv  
.global ipc_fast_call

# ipc_fast_send(channel: u64 [rdi], msg_ptr: *const u8 [rsi], len: usize [rdx])
# -> Result<(), IpcError> en rax
ipc_fast_send:
    # Déléguer vers la version Rust spsc_send (Phase 1 stub)
    jmp spsc_try_send_raw     # symbole Rust #[no_mangle]

# ipc_fast_recv(channel: u64 [rdi], buf: *mut u8 [rsi], len: usize [rdx])
# -> Option<usize> en rax (len reçu, 0 = vide)
ipc_fast_recv:
    jmp spsc_try_recv_raw     # symbole Rust #[no_mangle]

# ipc_fast_call(channel: u64 [rdi], req: *const u8 [rsi], req_len [rdx],
#               resp: *mut u8 [rcx], resp_cap [r8])
# -> Result<usize, IpcError> en rax
ipc_fast_call:
    jmp spsc_call_raw         # symbole Rust #[no_mangle]
```

### Mettre à jour `build.rs` pour le rerun-if-changed

```rust
// kernel/build.rs — AJOUTER :
println!("cargo:rerun-if-changed={dir}/src/ipc/core/fastcall_asm.s");
```

---

## Correction CRIT-09 — Définir `arch_cpu_relax`

```rust
// kernel/src/arch/x86_64/cpu/relax.rs — CRÉER ce fichier (ou ajouter dans mod.rs)

/// Instruction de relaxation CPU pour les boucles de spin-wait.
/// Sur x86_64 : PAUSE — réduit la consommation d'énergie et améliore
/// les performances de sortie de spin-lock (pipeline flush partiel).
///
/// Exportée avec `#[no_mangle]` pour être appelable depuis l'ASM et l'extern "C".
#[no_mangle]
pub extern "C" fn arch_cpu_relax() {
    // SAFETY: PAUSE est une instruction x86_64 sans effet de bord sur l'état architectural.
    unsafe { core::arch::x86_64::_mm_pause(); }
}
```

```rust
// kernel/src/arch/x86_64/mod.rs ou cpu/mod.rs — AJOUTER :
pub mod relax;
pub use relax::arch_cpu_relax;
```

### Alternative inline dans spsc.rs (si on ne veut pas le extern "C")

Remplacer le `extern "C"` par une fonction inline directe :

```rust
// kernel/src/ipc/ring/spsc.rs:355–362 — REMPLACER :

// AVANT :
// extern "C" { fn arch_cpu_relax(); }
// arch_cpu_relax();

// APRÈS :
#[inline(always)]
fn cpu_relax() {
    // SAFETY: PAUSE est safe sur tout x86_64.
    unsafe { core::arch::x86_64::_mm_pause(); }
}

// Dans spsc_wait_reply() :
// Remplacer arch_cpu_relax() par cpu_relax()
```

**Recommandation :** Préférer l'option `#[no_mangle] pub extern "C" fn arch_cpu_relax()` dans `arch/x86_64/cpu/` pour maintenir la cohérence avec les déclarations `extern "C"` existantes dans l'ASM.

---

## Vérification post-correction

```bash
# Vérifier qu'il n'y a plus de symboles non résolus
cargo build --target x86_64-unknown-none 2>&1 | grep "undefined symbol\|unresolved"

# Vérifier que fastcall_asm.s est bien inclus
nm target/.../exo-kernel | grep "ipc_fast_send\|ipc_fast_recv\|ipc_fast_call"

# Vérifier arch_cpu_relax
nm target/.../exo-kernel | grep "arch_cpu_relax"
```

---

**Priorité :** Build cassé — à corriger en premier (avant tout test)  
**Note :** CRIT-08 et CRIT-09 sont indépendants — peuvent être corrigés séparément
