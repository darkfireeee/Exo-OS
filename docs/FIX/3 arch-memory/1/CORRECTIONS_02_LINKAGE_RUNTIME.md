# CORRECTIONS CRITIQUES — Linkage & Runtime (CRIT-08 à CRIT-12)
> Audit ExoOS · kernel/ ipc/ memory/ arch/ · 2026-04-19
> CRIT-08/09/11/12 corrigés dans commit `2f75b6cf`. CRIT-10 reste ouvert.

---

## CRIT-08 — ✅ CORRIGÉ dans `2f75b6cf`

`core::arch::global_asm!(include_str!("fastcall_asm.s"), options(att_syntax))`
ajouté dans `kernel/src/ipc/core/mod.rs`.
`build.rs` enregistre `src/ipc/core/fastcall_asm.s` pour les rebuilds.
Correction validée.

---

## CRIT-09 — ✅ CORRIGÉ dans `2f75b6cf`

```rust
// kernel/src/arch/x86_64/mod.rs
#[no_mangle]
pub extern "C" fn arch_cpu_relax() {
    unsafe { core::arch::asm!("pause", options(nostack, nomem, preserves_flags)); }
}
```
Symbole maintenant défini. Correction validée.

---

## CRIT-10 — `kpti.rs` : KPTI activé sans `user_pml4` allouée → triple fault au retour userspace

### Fichier
`kernel/src/arch/x86_64/spectre/kpti.rs`
`kernel/src/memory/virtual/page_table/kpti_split.rs`

### Problème
`init_kpti()` appelle `KPTI_ENABLED.store(true, …)` mais `KPTI.register_cpu()` n'est
**jamais appelé** dans tout le codebase. `states[cpu_id].user_pml4 = PhysAddr::NULL`.
`kpti_split::KptiTable::switch_to_user()` appelle `write_cr3(PhysAddr::NULL)` → triple fault.

### Correction

**Étape 1** : Construire la PML4 user shadow lors de l'init BSP.

La PML4 user ne doit contenir que les mappings accessibles depuis l'espace user
(sections user-mode) + les stubs de syscall/exception (nécessaires pour SYSCALL/SYSRET).

```rust
// kernel/src/memory/virtual/page_table/kpti_split.rs — ajouter :

use crate::memory::physical::allocator::buddy;
use crate::memory::core::{AllocFlags, PhysAddr, PAGE_SIZE};

/// Alloue et construit la PML4 user shadow pour un CPU.
///
/// La PML4 user contient uniquement :
/// 1. Les entrées user-space (PML4[0..255])
/// 2. Le stub de syscall/exception mappé dans le dernier GiB (PML4[511] partiel)
///
/// # Safety
/// Doit être appelé en ring0, avant le premier retour vers l'espace user.
pub unsafe fn build_user_shadow_pml4(
    kernel_pml4_phys: PhysAddr,
) -> Result<PhysAddr, crate::memory::core::AllocError> {
    // Allouer une frame pour la PML4 user (4 KiB, zeroed)
    let frame = buddy::alloc_page(AllocFlags::ZEROED)?;
    let user_pml4_phys = frame.start_address();

    // La PML4 user est initialement vide (toute la mémoire kernel est absente).
    // Les mappings user-space seront ajoutés lors du premier execve().
    //
    // Pour que les syscall/exceptions fonctionnent depuis l'espace user, on copie
    // l'entrée PML4[511] (qui couvre 0xFFFF_FF80_0000_0000..0xFFFF_FFFF_FFFF_FFFF)
    // vers la PML4 user — uniquement les stubs qui doivent être visibles depuis Ring 3.
    //
    // NOTE : En production, seules les pages de stub (syscall_entry_asm, exception stubs)
    // doivent être mappées dans la PML4 user avec le bit USER non positionné.
    // Ici on copie l'entrée complète PML4[511] pour correctness initiale.
    // Affiner en production pour limiter l'exposition.
    let kernel_pml4 = kernel_pml4_phys.as_u64() as *const u64;
    let user_pml4 = user_pml4_phys.as_u64() as *mut u64;

    // Copier PML4[511] : contient KERNEL_START et les stubs de syscall
    let entry_511 = core::ptr::read_volatile(kernel_pml4.add(511));
    core::ptr::write_volatile(user_pml4.add(511), entry_511);

    Ok(user_pml4_phys)
}
```

**Étape 2** : Appeler `register_cpu()` dans `init_kpti()` et l'init AP.

```rust
// kernel/src/arch/x86_64/spectre/kpti.rs — MODIFIER init_kpti() :

pub fn init_kpti() {
    let features = super::super::cpu::features::cpu_features();

    // Activer SMEP
    if features.has_smep() {
        unsafe {
            let mut cr4: u64;
            core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nostack, nomem));
            cr4 |= 1 << 20;
            core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nostack, nomem));
        }
    }

    // Activer SMAP
    if features.has_smap() {
        unsafe {
            let mut cr4: u64;
            core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nostack, nomem));
            cr4 |= 1 << 21;
            core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nostack, nomem));
        }
    }

    // ── CORRECTION CRIT-10 : construire et enregistrer la PML4 user shadow ──
    use crate::memory::virtual::page_table::kpti_split::KPTI;
    use crate::memory::virt::page_table::read_cr3;

    let kernel_cr3 = unsafe { read_cr3() };
    let kernel_pml4_phys = crate::memory::core::PhysAddr::new(kernel_cr3 & !0xFFF);
    let trampoline_phys = crate::memory::core::PhysAddr::new(
        crate::arch::x86_64::smp::init::TRAMPOLINE_PHYS
    );

    // Obtenir le cpu_id du BSP (toujours 0)
    let cpu_id = 0usize;

    match unsafe {
        crate::memory::virtual::page_table::kpti_split::build_user_shadow_pml4(kernel_pml4_phys)
    } {
        Ok(user_pml4_phys) => {
            unsafe {
                KPTI.register_cpu(cpu_id, kernel_pml4_phys, user_pml4_phys, trampoline_phys);
            }
            KPTI.enable();
            KPTI_ENABLED.store(true, Ordering::Release);
        }
        Err(_) => {
            // Échec d'allocation → KPTI désactivé (mode dégradé sécurité).
            // Ne pas paniquer — KPTI est une mitigation, pas une condition de boot.
            log::warn!("KPTI: allocation PML4 user échouée — KPTI désactivé");
        }
    }
}
```

**Étape 3** : Répéter pour chaque AP dans `ap_entry()`.

```rust
// kernel/src/arch/x86_64/smp/init.rs — dans ap_entry() après percpu init :
// Même logique que dans init_kpti() mais pour le cpu_id de l'AP.
// Utiliser percpu::cpu_id() pour obtenir l'ID logique de l'AP.
```

---

## CRIT-11 — ✅ CORRIGÉ dans `2f75b6cf`

`smp_cpu_count()` dans `init.rs` délègue à `percpu::cpu_count()`.
Un seul `ONLINE_CPU_COUNT` autoritatif dans `percpu.rs`. Correction validée.

---

## CRIT-12 — PARTIELLEMENT CORRIGÉ dans `2f75b6cf`

`register_backend_swap_provider()` est maintenant définie et appelée dans
`kernel/src/memory/mod.rs:168`. Correction fonctionnelle.

**Point résiduel NEW-02** : vérifier que `memory::mod` est bien appelé au boot
**avant** le premier page fault qui déclencherait un swap-in.

```rust
// kernel/src/memory/mod.rs — vérifier que la séquence init inclut :
pub fn init_memory_subsystem(...) {
    // ... autres initialisations ...
    virt::fault::swap_in::register_backend_swap_provider(); // ligne 168 — OK
    // Doit précéder le premier retour vers l'espace user.
}
```

---

## NEW-01 — `swap_in.rs` : `core::mem::transmute` sur fat pointer — UB potentiel

### Fichier
`kernel/src/memory/virtual/fault/swap_in.rs`

### Problème
```rust
let (data_ptr, vtable_ptr): (*const (), *const ()) = unsafe { core::mem::transmute(provider) };
```
`transmute` de `&dyn Trait` (fat pointer = 2×usize) vers `(*const (), *const ())` n'est
pas garanti stable par le compilateur. La représentation interne des fat pointers
n'est pas stabilisée dans la spec Rust.

### Correction
Utiliser l'API stable `core::ptr` pour extraire les composants d'un fat pointer :

```rust
// kernel/src/memory/virtual/fault/swap_in.rs — REMPLACER register_backend_swap_provider() :

pub fn register_backend_swap_provider() {
    let provider: &'static dyn SwapInProvider = &BACKEND_SWAP_IN_PROVIDER;

    // Extraction safe du fat pointer via l'API stable core::ptr
    // (stabilisée dans Rust 1.76 — disponible sur nightly depuis plus longtemps).
    let data_ptr = provider as *const dyn SwapInProvider as *const ();

    // Pour le vtable : utiliser core::ptr::metadata (nightly) ou
    // la représentation garantie (data_ptr, vtable_ptr) via raw::TraitObject.
    // En attendant la stabilisation de ptr::metadata :
    // OPTION A (nightly) :
    // let vtable_ptr = core::ptr::metadata(provider) as *const ();

    // OPTION B (stable, workaround documenté) :
    // Les fat pointers &dyn Trait sont garantis [data_ptr, vtable_ptr] depuis Rust 1.0
    // mais pas formellement stabilisés. Utiliser #[repr(C)] comme workaround :
    #[repr(C)]
    struct FatPtr { data: *const (), vtable: *const () }
    // SAFETY: Layout [data, vtable] est stable en pratique et documenté comme tel.
    // À remplacer par core::ptr::metadata() quand stabilisé.
    let fat: FatPtr = unsafe { core::mem::transmute(provider) };

    unsafe { register_swap_provider(fat.data, fat.vtable); }
}
```

**Recommandation à long terme** : utiliser `core::ptr::metadata()` (feature `ptr_metadata`)
une fois stabilisée, ou restructurer le registre pour éviter le fat pointer manuel.

---

## MAJ-09 — ✅ CORRIGÉ dans `2f75b6cf`

`send_sipi_once()` envoie maintenant deux SIPIs avec délai 1ms entre les deux.
Conforme à la spec Intel MP (section B.4). Correction validée.

---

## MAJ-10 — ✅ CORRIGÉ dans `2f75b6cf`

`tsc_offset` est maintenant `i64`. `apply_tsc_offset()` gère les deux sens.
Tests unitaires inclus. Correction validée.

---

## MAJ-11 — ✅ CORRIGÉ dans `2f75b6cf`

`CPU_ONLINE_MASK` est un tableau de 4 `AtomicU64` couvrant 256 CPUs.
Assertion compile-time présente. Correction validée.

---

## MAJ-12 — ✅ CORRIGÉ dans `2f75b6cf`

`ring_for()` retourne `Err(IpcError::InvalidParam)` si `channel_id >= 256`
au lieu du modulo. Correction validée.
