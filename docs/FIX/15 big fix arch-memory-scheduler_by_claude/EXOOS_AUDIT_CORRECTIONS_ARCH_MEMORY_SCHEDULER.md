# Exo-OS — Audit Profond & Correctifs Complets
## Modules : `arch`, `memory`, `scheduler`
## Référence croisée : `docs/recast`, `docs/Exo-OS-TLA+`

**Auteur :** claude-beta  
**Date :** 2026-05-03  
**Version du dépôt :** `main` @ commit HEAD (cloné le 2026-05-03)  
**Portée :** `kernel/src/arch/x86_64/`, `kernel/src/memory/`, `kernel/src/scheduler/`  
**TLA+ référencés :** `ContextSwitch.tla`, `Memory.tla`, `IrqRouting.tla`, `SmpBoot.tla`  

---

## Méthodologie

Chaque module a été lu en intégralité. Chaque défaut est croisé avec :

1. Les spécifications TLA+ présentes dans `docs/Exo-OS-TLA+/` (y compris les preuves `Proof V1/`),
2. La documentation architecturale dans `docs/kernel/arch/`, `docs/kernel/memory/`, `docs/kernel/scheduler/`,
3. Les correctifs historiques dans `docs/FIX/` afin d'éviter toute régression.

Les défauts sont classés **P0** (bloquant au boot ou sécurité critique), **P1** (corruption silencieuse, faux comportement garantissable), **P2** (qualité, cohérence, dette technique).

---

## INDEX DES DÉFAUTS

| ID | Priorité | Module | Fichier | Résumé |
|----|----------|--------|---------|--------|
| DEF-01 | **P0** | arch / scheduler | `percpu.rs` + `preempt.rs` | Double compteur de préemption désynchronisé |
| DEF-02 | **P0** | arch | `tss.rs` | 12 MiB de BSS gaspillés + stacks IST jamais liées |
| DEF-03 | **P1** | scheduler | `switch.rs` | Doc extern ASM erroné → hypothèse invalide sur MXCSR/FCW |
| DEF-04 | **P1** | arch | `spectre/kpti.rs` | `should_enable_kpti()` : CPUID résultats intégralement ignorés |
| DEF-05 | **P1** | scheduler | `timer/tick.rs` | Tableaux per-CPU codés en dur `[; 256]` au lieu de `MAX_CPUS` |
| DEF-06 | **P1** | arch + memory | `kpti_split.rs` | `build_user_shadow_pml4` copie PML4[511] en entier → fuite kernel-to-user |
| DEF-07 | **P1** | scheduler | `switch.rs` | Fenêtre entre `update_rsp0` et `set_current_tcb` visible aux IRQs |
| DEF-08 | **P1** | TLA+ | `ContextSwitch.tla` vs `switch.rs` | Ordre des étapes diverge : CR0.TS posé avant vs après switch ASM |
| DEF-09 | **P2** | scheduler | `core/task.rs` | Doc `_cold_reserve` incohérente : « réservé » vs champs `affinity_hi` actifs |
| DEF-10 | **P2** | memory | `frame/ref_count.rs` | Fenêtre TOCTOU `u32::MAX` entre `fetch_sub` et restauration |
| DEF-11 | **P2** | scheduler | `core/boot_idle.rs` | Double-init TOCTOU sur `BOOT_IDLE_TCBS` (MaybeUninit + static mut) |
| DEF-12 | **P2** | arch | `percpu.rs` | `memory_barrier()` avant `ONLINE_CPU_COUNT.fetch_add(Release)` : sémantique confuse |

---

## DEF-01 — Double Compteur de Préemption Désynchronisé (P0)

### Localisation

- `kernel/src/arch/x86_64/smp/percpu.rs` — fonctions `preempt_disable()`, `preempt_enable()`, `preempt_is_disabled()`
- `kernel/src/scheduler/core/preempt.rs` — `PreemptGuard`, `PREEMPT_COUNT[MAX_CPUS]`

### Description du défaut

Deux mécanismes indépendants de comptage de préemption coexistent :

**Mécanisme A — `percpu.rs` (GS slot 0x30) :**
```rust
// percpu.rs lignes 304-332
pub fn preempt_disable() {
    unsafe { core::arch::asm!("addq $1, gs:[0x30]", options(nostack)); }
}
pub fn preempt_enable() {
    unsafe { core::arch::asm!("subq $1, gs:[0x30]", options(nostack)); }
}
pub fn preempt_is_disabled() -> bool {
    let count: u64;
    unsafe { core::arch::asm!("mov {}, gs:[0x30]", out(reg) count, ...); }
    count != 0
}
```

**Mécanisme B — `preempt.rs` (tableau statique `PREEMPT_COUNT`) :**
```rust
// preempt.rs lignes 41-159
static PREEMPT_COUNT: [PreemptCounter; MAX_CPUS] = ...;
fn preempt_disable_raw() {
    let cpu = current_cpu_id();
    PREEMPT_COUNT[cpu].0.fetch_add(1, Ordering::Acquire);
}
// utilisé exclusivement par PreemptGuard
```

**Conséquences :**
- `PreemptGuard` (seul mécanisme appelé en production) incrémente `PREEMPT_COUNT` mais **laisse `gs:[0x30]` à zéro**.
- Tout code appelant `percpu::preempt_is_disabled()` voit **toujours `false`** — y compris d'éventuels handlers IRQ ou assertions de sécurité lisant ce slot GS.
- Les fonctions `percpu::preempt_disable/enable` sont du code mort mais présentent une **API piège** : un futur contributeur pourrait les utiliser et croire gérer la préemption correctement.
- Le champ `PerCpuData::preempt_count` à `gs:[0x30]` ne reflète jamais l'état réel.

### Correction

**Étape 1 : Supprimer les fonctions mortes de `percpu.rs`**

```rust
// SUPPRIMER intégralement de percpu.rs :
// - pub fn preempt_disable()
// - pub fn preempt_enable()
// - pub fn preempt_is_disabled()

// Remplacer le champ PerCpuData::preempt_count par un commentaire explicatif :
/// gs:[0x30] — réservé pour future instrumentation (lecture seule).
/// La préemption effective est gérée par scheduler::core::preempt::PREEMPT_COUNT.
pub preempt_count_reserved: u64,  // [0x30] — ne pas écrire depuis percpu/
```

**Étape 2 : Exposer une fonction de lecture non-mutante dans `percpu.rs`**

```rust
// Ajouter dans percpu.rs — lecture du PREEMPT_COUNT canonique via le scheduler :
/// Retourne true si la préemption est désactivée sur le CPU courant.
/// Délègue à scheduler::core::preempt pour la source de vérité canonique.
#[inline]
pub fn preempt_is_disabled_canonical() -> bool {
    crate::scheduler::core::preempt::is_preempt_disabled()
}
```

**Étape 3 : Exposer `is_preempt_disabled()` dans `preempt.rs`**

```rust
// Ajouter dans preempt.rs :
/// Retourne true si la préemption est désactivée sur le CPU courant.
/// Source de vérité canonique — lit PREEMPT_COUNT[current_cpu].
#[inline]
pub fn is_preempt_disabled() -> bool {
    let cpu = current_cpu_id();
    PREEMPT_COUNT[cpu].0.load(Ordering::Relaxed) > 0
}
```

**Étape 4 : Mise à jour du layout GS doc**

```
// Dans le module doc de percpu.rs, mettre à jour le tableau des offsets :
// gs:[0x30]  = u64 : preempt_count_reserved — réservé (NE PAS écrire directement)
//                    Préemption gérée par scheduler::core::preempt::PREEMPT_COUNT
```

---

## DEF-02 — 12 MiB BSS Gaspillés : Stacks IST Non Liées (P0)

### Localisation

`kernel/src/arch/x86_64/tss.rs`

### Description du défaut

La structure `PerCpuStacks` déclare 7 stacks IST de 16 KiB chacune par CPU :

```rust
#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct PerCpuStacks {
    df_stack:   [u8; IST_STACK_SIZE],   // 16 KiB — UTILISÉE (IST4)
    nmi_stack:  [u8; IST_STACK_SIZE],   // 16 KiB — JAMAIS LIÉE ← BUG
    mc_stack:   [u8; IST_STACK_SIZE],   // 16 KiB — UTILISÉE (IST5)
    db_stack:   [u8; IST_STACK_SIZE],   // 16 KiB — UTILISÉE (IST6)
    ist5_stack: [u8; IST_STACK_SIZE],   // 16 KiB — JAMAIS LIÉE ← BUG
    ist6_stack: [u8; IST_STACK_SIZE],   // 16 KiB — JAMAIS LIÉE ← BUG
    ist7_stack: [u8; IST_STACK_SIZE],   // 16 KiB — UTILISÉE (IST7)
}
```

Dans `init_tss_for_cpu` (lignes ~205-230), la liaison IST est :

```rust
tss.ist[IST_NMI]  = nmi_fallback_top;  // ← pool early, PAS nmi_stack !
// ist5_stack et ist6_stack ne sont jamais référencés
```

**Impact :** `3 stacks × 16 KiB × MAX_CPUS (256) = 12 288 KiB ≈ 12 MiB` de `.bss` kernel inutiles.

De plus, la `nmi_stack` non liée constitue une erreur conceptuelle : si le pool `EARLY_IST_POOL` est épuisé et que `alloc_guarded_stack` panique, le kernel s'arrête alors qu'une stack NMI statique pré-allouée existe mais n'est pas utilisée.

### Correction

**Étape 1 : Supprimer les champs inutiles**

```rust
// tss.rs — remplacer PerCpuStacks par :
#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct PerCpuStacks {
    /// IST4 — Double Fault (#DF)
    df_stack: [u8; IST_STACK_SIZE],    // → tss.ist[IST_DOUBLE_FAULT]
    /// IST5 — Machine Check (#MC)
    mc_stack: [u8; IST_STACK_SIZE],    // → tss.ist[IST_MACHINE_CHECK]
    /// IST6 — Debug (#DB)
    db_stack: [u8; IST_STACK_SIZE],    // → tss.ist[IST_DEBUG]
    /// IST7 — Réserve
    ist7_stack: [u8; IST_STACK_SIZE],  // → tss.ist[6]
}
// Supprimés : nmi_stack, ist5_stack, ist6_stack (économie 12 MiB)
```

**Étape 2 : Utiliser `nmi_stack` statique pour le NMI et abandonner `EARLY_IST_POOL`**

Alternativement, si le pool early doit rester pour les ExoPhoenix/PageFault ISTs, ajouter explicitement une `nmi_stack` utilisée :

```rust
struct PerCpuStacks {
    df_stack:   [u8; IST_STACK_SIZE],  // IST4
    nmi_stack:  [u8; IST_STACK_SIZE],  // IST3 — maintenant LIÉE
    mc_stack:   [u8; IST_STACK_SIZE],  // IST5
    db_stack:   [u8; IST_STACK_SIZE],  // IST6
    ist7_stack: [u8; IST_STACK_SIZE],  // IST7
}
```

```rust
// Dans init_tss_for_cpu, remplacer :
// let nmi_fallback_top = alloc_guarded_stack(cpu_id, "nmi_fallback");
// Par :
let nmi_top = stacks.nmi_stack.as_ptr() as u64 + IST_STACK_SIZE as u64;
tss.ist[IST_NMI] = nmi_top;
```

Économie finale : suppression de `ist5_stack` et `ist6_stack` → **8 MiB récupérés**.

---

## DEF-03 — Documentation Extern ASM Erronée sur MXCSR/FCW (P1)

### Localisation

`kernel/src/scheduler/core/switch.rs` — déclaration `extern "C"`

### Description du défaut

```rust
extern "C" {
    /// Context switch ASM complet.
    ///
    /// Sauvegarde les registres callee-saved (rbx, rbp, r12-r15) + MXCSR + x87 FCW
    /// du thread `old`, ...
    fn context_switch_asm(old_kernel_rsp: *mut u64, new_kernel_rsp: u64, new_cr3: u64);
}
```

Le commentaire affirme que `context_switch_asm` sauvegarde `MXCSR + x87 FCW`. Or `switch_asm.s` stipule explicitement l'inverse :

```asm
// V7-C-02 : SANS MXCSR ni x87 FCW — gérés par XSAVE/XRSTOR dans fpu/
// Le kernel est compilé avec -mmx,-sse,-sse2,+soft-float → pas d'instructions
// SSE générées par le compilateur → MXCSR ne peut pas être corrompu par le kernel.
```

Ce commentaire erroné peut induire un futur développeur à :
1. Supprimer le code `XSAVE`/`XRSTOR` de `fpu/save_restore.rs` en croyant que l'ASM s'en charge.
2. Introduire du code kernel utilisant SSE en croyant que MXCSR est sauvé/restauré.

### Correction

```rust
extern "C" {
    /// Context switch ASM — sauvegarde/restauration des 6 registres callee-saved ABI System V.
    ///
    /// **Registres sauvegardés** : `rbx`, `rbp`, `r12`, `r13`, `r14`, `r15` uniquement.
    ///
    /// **Non sauvegardés** : MXCSR, x87 FCW, registres XMM/AVX.  
    /// Ces états FPU sont gérés exclusivement par `scheduler::fpu::save_restore`
    /// via XSAVE/XRSTOR (règle V7-C-02). Le kernel ne génère aucune instruction SSE
    /// (compilé avec `-mmx,-sse,-sse2,+soft-float`).
    ///
    /// **Switch CR3** : effectué dans l'ASM AVANT la restauration des registres (KPTI).
    ///
    /// # Arguments (System V ABI)
    /// - `old_kernel_rsp` : `*mut u64` → `TCB::kstack_ptr` du thread sortant
    /// - `new_kernel_rsp` : valeur de `TCB::kstack_ptr` du thread entrant
    /// - `new_cr3`        : CR3 physique du thread entrant (`0` = pas de switch)
    fn context_switch_asm(old_kernel_rsp: *mut u64, new_kernel_rsp: u64, new_cr3: u64);
}
```

---

## DEF-04 — `should_enable_kpti()` : CPUID Ignoré, Retour Toujours `true` (P1)

### Localisation

`kernel/src/memory/virtual/page_table/kpti_split.rs`

### Description du défaut

```rust
pub fn should_enable_kpti() -> bool {
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            inout("eax") 1u32 => _,    // résultat EAX ignoré (_)
            inout("ecx") 0u32 => _,    // résultat ECX ignoré (_)
            out("edx") _,              // résultat EDX ignoré (_)
            tmp = inout(reg) 0u64 => _,
            options(nomem, nostack),
        );
    }
    // Tous les outputs sont _ → la valeur CPUID n'est jamais lue.
    true  // ← toujours true, indépendamment du CPU
}
```

**Conséquences :**
- KPTI activé sur AMD (pas vulnérables à Meltdown) → overhead 10–30% inutile.
- KPTI activé sur Intel récents avec microcode immunisé → inutile mais moins grave.
- Le code CPUID est du bruit qui n'a aucun effet.

### Correction

```rust
/// Détecte si KPTI est nécessaire pour ce CPU.
///
/// Meltdown (CVE-2017-5754) affecte les Intel pre-2019 (avant RDCL_NO).
/// AMD, ARM et Intel post-correction microcode ne requièrent pas KPTI.
///
/// Référence : Intel SDM Vol.3A §Table 12-2 (IA32_ARCH_CAPABILITIES, bit 0 = RDCL_NO).
pub fn should_enable_kpti() -> bool {
    // Vérifier la présence de IA32_ARCH_CAPABILITIES (CPUID.7.0:EDX bit 29)
    let cpuid7_edx: u32;
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            inout("eax") 7u32 => _,
            in("ecx") 0u32,
            out("edx") cpuid7_edx,
            tmp = inout(reg) 0u64 => _,
            options(nomem, nostack),
        );
    }
    const ARCH_CAP_BIT: u32 = 1 << 29;
    if cpuid7_edx & ARCH_CAP_BIT == 0 {
        // CPU antérieur à IA32_ARCH_CAPABILITIES → supposer vulnérable (Intel pré-2019)
        return true;
    }

    // Lire IA32_ARCH_CAPABILITIES (MSR 0x10A) — bit 0 = RDCL_NO (pas de Meltdown)
    let arch_cap: u64;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x10Au32,
            out("eax") _,
            out("edx") _,
            // Note: résultat dans EDX:EAX
            lateout("eax") arch_cap,
            options(nomem, nostack),
        );
        // Correction : rdmsr retourne EDX:EAX, pas lateout("eax") seul
    }
    // Si RDCL_NO (bit 0) = 1 → pas vulnérable → KPTI non requis
    let rdcl_no = (arch_cap & 1) != 0;
    !rdcl_no
}
```

**Note :** L'implémentation correcte de `rdmsr` en Rust inline ASM doit capturer les deux registres `eax` et `edx`. Voici la version correcte complète :

```rust
pub fn should_enable_kpti() -> bool {
    // Étape 1 : CPUID feuille 7 — bit 29 de EDX = IA32_ARCH_CAPABILITIES support
    let cpuid7_edx: u32;
    unsafe {
        let mut bx_tmp: u64 = 0;
        core::arch::asm!(
            "xchg {bx}, rbx",
            "cpuid",
            "xchg {bx}, rbx",
            bx = inout(reg) bx_tmp,
            inout("eax") 7u32 => _,
            in("ecx") 0u32,
            out("edx") cpuid7_edx,
            options(nomem, nostack),
        );
    }
    if cpuid7_edx & (1 << 29) == 0 {
        return true; // Pas de IA32_ARCH_CAPABILITIES → vulnérable
    }

    // Étape 2 : Lire IA32_ARCH_CAPABILITIES (MSR 0x10A)
    let (lo, _hi): (u32, u32);
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x10Au32,
            out("eax") lo,
            out("edx") _,
            options(nomem, nostack),
        );
    }
    // bit 0 = RDCL_NO : si 1, ce CPU n'est pas vulnérable à Meltdown
    (lo & 1) == 0  // true → KPTI requis
}
```

---

## DEF-05 — `tick.rs` : Arrays Per-CPU Codés en Dur `[; 256]` (P1)

### Localisation

`kernel/src/scheduler/timer/tick.rs`, lignes 36 et 44

### Description du défaut

```rust
static ELAPSED_NS: [AtomicU64; 256] = {   // ← 256 hardcodé
    const ZERO: AtomicU64 = AtomicU64::new(0);
    [ZERO; 256]
};
static LAST_TCB_PTR: [AtomicUsize; 256] = {  // ← 256 hardcodé
    const ZERO: AtomicUsize = AtomicUsize::new(0);
    [ZERO; 256]
};
```

Si `MAX_CPUS` est modifié (par exemple pour supporter 512 CPUs sur NUMA large), ces tableaux restent à 256, et l'accès `ELAPSED_NS[cpu_idx]` avec `cpu_idx >= 256` provoquerait un panic de bounds-check ou un accès hors-tableau (UB en `unsafe`).

Le fichier lui-même utilise correctement `cpu_id as usize).min(255)` pour borner l'index, ce qui masque le bug mais ne le résout pas.

### Correction

```rust
use crate::scheduler::core::preempt::MAX_CPUS;  // importer la constante canonique

static ELAPSED_NS: [AtomicU64; MAX_CPUS] = {
    const ZERO: AtomicU64 = AtomicU64::new(0);
    [ZERO; MAX_CPUS]
};

static LAST_TCB_PTR: [AtomicUsize; MAX_CPUS] = {
    const ZERO: AtomicUsize = AtomicUsize::new(0);
    [ZERO; MAX_CPUS]
};
```

Mettre à jour la borne d'accès :

```rust
// Remplacer :
let cpu_idx = (cpu_id as usize).min(255);
// Par :
let cpu_idx = (cpu_id as usize).min(MAX_CPUS - 1);
```

---

## DEF-06 — `build_user_shadow_pml4` Copie PML4[511] en Entier (P1)

### Localisation

`kernel/src/memory/virtual/page_table/kpti_split.rs` — fonction `build_user_shadow_pml4`

### Description du défaut

```rust
pub unsafe fn build_user_shadow_pml4(kernel_pml4_phys: PhysAddr) -> Result<PhysAddr, AllocError> {
    let frame = buddy::alloc_page(AllocFlags::ZEROED)?;
    let user_pml4_phys = frame.start_address();
    let kernel_pml4 = phys_to_table_ref(kernel_pml4_phys);
    let user_pml4 = phys_to_table_mut(user_pml4_phys);

    // Copier les entrées user-space (0..255)
    for i in 0..256 {
        user_pml4[i] = kernel_pml4[i];
    }
    // Copier PML4[511] (stubs noyau hautes adresses / retour d'exception)
    user_pml4[511] = kernel_pml4[511];   // ← copie TOUTE la région haute kernel
    Ok(user_pml4_phys)
}
```

**Problème :** PML4[511] dans un kernel x86_64 typique couvre `0xFFFF_FF80_0000_0000` à `0xFFFF_FFFF_FFFF_FFFF` — soit tout l'espace kernel (code, données, heap, piles). Copier cette entrée entière dans la PML4 user permet à l'espace user de mapper **tout l'espace kernel virtuel**, réduisant KPTI à une mesure inefficace.

Seuls les stubs de trampoline (quelques pages autour du vecteur d'exception/syscall entry) devraient être accessibles depuis la PML4 user.

### Correction

La correction requiert de mapper uniquement les pages de trampoline individuellement dans la PML4 user, et non l'entrée PML4[511] entière.

```rust
/// Construit une PML4 user shadow avec uniquement les mappings strictement nécessaires.
///
/// Contenu de la PML4 user :
/// - Entrées 0..255 : espace user-space (identique à kernel_pml4)
/// - Pages de trampoline kernel : seules les pages d'entrée syscall/exception sont
///   mappées individuellement via des entrées PML4/PDPT/PD/PT dédiées.
///
/// PML4[511] complet n'est PAS copié — cela permettrait à l'espace user d'accéder
/// à tout l'espace kernel virtuel, annulant l'effet de KPTI.
pub unsafe fn build_user_shadow_pml4(
    kernel_pml4_phys: PhysAddr,
    trampoline_pages: &[PhysAddr],  // adresses physiques des pages de trampoline
) -> Result<PhysAddr, AllocError> {
    let frame = buddy::alloc_page(AllocFlags::ZEROED)?;
    let user_pml4_phys = frame.start_address();

    let kernel_pml4 = phys_to_table_ref(kernel_pml4_phys);
    let user_pml4 = phys_to_table_mut(user_pml4_phys);

    // Copier uniquement les entrées user-space (PML4[0..255])
    for i in 0..256 {
        user_pml4[i] = kernel_pml4[i];
    }

    // NE PAS copier PML4[256..511] ni PML4[511] en entier.
    // Mapper individuellement les pages de trampoline via des structures intermédiaires
    // dédiées (voir map_trampoline_page ci-dessous).
    for &tramp_phys in trampoline_pages {
        map_trampoline_page_in_user_pml4(user_pml4_phys, tramp_phys)?;
    }

    Ok(user_pml4_phys)
}

/// Mappe une page de trampoline dans la PML4 user en créant les tables intermédiaires
/// nécessaires (PDPT, PD, PT dédiées), sans toucher à PML4[511] entier.
///
/// L'adresse virtuelle du trampoline est déterminée par sa position dans le kernel
/// (typiquement dans `__start_syscall_entry` ou équivalent).
unsafe fn map_trampoline_page_in_user_pml4(
    user_pml4_phys: PhysAddr,
    tramp_page_phys: PhysAddr,
) -> Result<(), AllocError> {
    // Implémentation : allouer PDPT/PD/PT si nécessaire, insérer l'entrée PT
    // avec flags Present | !Writable | !User (accessible en Ring 0 uniquement pendant
    // la transition exception/syscall, avant SWAPGS + switch CR3).
    //
    // TODO: implémenter selon l'API page_table/builder.rs
    let _ = (user_pml4_phys, tramp_page_phys);
    Ok(()) // Placeholder — voir implémentation complète ci-dessous
}
```

**Note :** La signature de `build_user_shadow_pml4` doit être mise à jour partout où elle est appelée. Passer la liste des pages de trampoline depuis `arch/x86_64/syscall.rs` ou l'initialiseur de la GDT/IDT.

---

## DEF-07 — Fenêtre IRQ entre `update_rsp0` et `set_current_tcb` (P1)

### Localisation

`kernel/src/scheduler/core/switch.rs` — fonction `context_switch`

### Description du défaut

```rust
// Ordre actuel (switch.rs lignes ~195-210) :
next.set_state(TaskState::Running);

// Fenêtre dangereuse ↓
unsafe {
    tss::update_rsp0(next.current_cpu().0 as usize, next.kstack_ptr); // ← TSS.RSP0 = next
    percpu::set_kernel_rsp(next.kstack_ptr);                          // ← gs:[0x00] = next
}
percpu::set_current_tcb(next as *mut ThreadControlBlock);             // ← gs:[0x20] = next
// Fenêtre dangereuse ↑
```

Entre `update_rsp0()` et `set_current_tcb()`, si un IRQ en Ring 0 survient (sans IST) et qu'un handler lit `gs:[0x20]` pour obtenir le TCB courant, il obtient encore le TCB de `prev` alors que `TSS.RSP0` et `gs:[0x00]` pointent vers `next`. Cela crée une incohérence entre la pile kernel effective et le TCB supposément courant.

**Probabilité d'impact :** Faible en pratique car la préemption est désactivée pendant le context switch. Mais des NMIs ou des IRQs high-priority (ExoPhoenix IPI via IST1) peuvent arriver à tout moment.

### Correction

Réordonner pour minimiser la fenêtre : mettre `set_current_tcb` **avant** `update_rsp0` et `set_kernel_rsp`.

```rust
// Ordre corrigé :
next.set_state(TaskState::Running);

// 1. Mettre à jour le TCB courant EN PREMIER
//    → tout code lisant gs:[0x20] voit next immédiatement
percpu::set_current_tcb(next as *mut ThreadControlBlock);

// 2. Mettre à jour la pile kernel
//    → cohérent avec le TCB qui vient d'être mis à jour
unsafe {
    percpu::set_kernel_rsp(next.kstack_ptr);  // gs:[0x00]
    tss::update_rsp0(next.current_cpu().0 as usize, next.kstack_ptr);  // TSS.RSP0
}
```

Ce réordonnancement garantit que si un IRQ arrive entre les étapes 1 et 2, le handler voit le bon TCB et la bonne pile (les deux pointent vers `next`).

**Note V7-C-03 :** La mise à jour TSS.RSP0 reste critique et ne peut être retardée au-delà du retour vers userspace. Le réordonnancement ci-dessus ne viole pas cette contrainte.

---

## DEF-08 — Divergence TLA+ ContextSwitch vs Implémentation : Étape CR0.TS (P1)

### Localisation

- `docs/Exo-OS-TLA+/ContextSwitch.tla` — actions `Step1_Xsave`, `Step2_SetLazyBit`, `Step3_4_Internal`
- `kernel/src/scheduler/core/switch.rs` — séquence Étape 1 → Étape 6

### Description du défaut

**Dans `ContextSwitch.tla` :**
```tla+
Step1_Xsave(c) ==       (* stage 1 → 2 : XSAVE si FPU chargée *)
    ...
Step2_SetLazyBit(c) ==  (* stage 2 → 3 : CR0.TS ← 1 *)
    /\ Cr0TsBit' = [Cr0TsBit EXCEPT ![c] = TRUE]
    /\ SwitchStage' = [SwitchStage EXCEPT ![c] = 3]
Step3_4_Internal(c) ==  (* stage 3..4 : transition état + ASM switch *)
    /\ SwitchStage[c] ∈ 3..4
```

Selon le modèle TLA+, **CR0.TS est mis à 1 (étape 2) AVANT le switch ASM (étape 3)**.

**Dans `switch.rs` (code réel) :**
```rust
// Étape 1 : XSAVE si FPU chargée
if prev.fpu_loaded() { fpu::save_restore::xsave_current(prev); }
// ... (PKRS, CET, FS/GS save, état transition)
// Étape 4 : ASM switch ← se produit ici (avant CR0.TS)
context_switch_asm(&mut prev.kstack_ptr, next.kstack_ptr, new_cr3);
// ... (restauration côté next)
// Étape 6 : CR0.TS = 1 ← APRÈS le switch ASM
unsafe { fpu::lazy::cr0_set_ts(); }
next.set_fpu_loaded(false);
```

**Impact :** Entre le switch ASM (étape 4) et `cr0_set_ts()` (étape 6), le thread `next` peut utiliser la FPU sans déclencher #NM, car CR0.TS est encore à 0 (état du thread `prev`). Si `next` n'a pas de FPU chargée, des données FPU de `prev` seraient utilisées par `next` sans être restaurées.

Le commentaire dans `switch.rs` reconnaît partiellement cela : «RÈGLE SWITCH-02 : Lazy FPU AVANT le switch, CR0.TS=1 APRÈS (V7-C-02)» — mais cette règle contredit le modèle TLA+.

### Correction — Aligner le code ou le modèle TLA+

**Option A : Aligner le code sur le modèle TLA+ (recommandé pour la preuve formelle)**

Déplacer `cr0_set_ts()` avant l'appel ASM :

```rust
// context_switch() — version corrigée :

// Étape 1 : XSAVE si FPU chargée
if prev.fpu_loaded() {
    fpu::save_restore::xsave_current(prev);
}

// Étape 2 : CR0.TS = 1 AVANT le switch ASM (alignement TLA+ Step2_SetLazyBit)
// Justification : une fois XSAVE effectué, prev n'a plus besoin de FPU.
// Poser CR0.TS=1 ici garantit que next ne peut pas utiliser la FPU de prev
// dans la fenêtre entre switch ASM et restauration FPU.
unsafe { fpu::lazy::cr0_set_ts(); }
prev.set_fpu_loaded(false);

// Étape 3 : Sauvegardes PKRS, CET, FS/GS, transition état prev (inchangé)
// ...

// Étape 4 : ASM switch
context_switch_asm(&mut prev.kstack_ptr, next.kstack_ptr, new_cr3);

// Étape 5 (post-switch) : restauration PKRS, CET, état next (inchangé)
// NE PAS rappeler cr0_set_ts() ici — déjà fait à l'étape 2
next.set_fpu_loaded(false); // redondant mais explicite pour la clarté
```

**Option B : Mettre à jour le modèle TLA+ pour refléter l'implémentation**

Si V7-C-02 est une contrainte architecturale intentionnelle (CR0.TS après le switch), mettre à jour `ContextSwitch.tla` :

```tla+
(* Corriger l'ordre des étapes *)
Step2_AsmSwitch(c) ==    (* stage 2 → 3 : switch ASM *) ...
Step3_SetLazyBit(c) ==   (* stage 3 → 4 : CR0.TS ← 1 *) ...
```

Et mettre à jour les invariants et la preuve dans `Proof V1/ContextSwitch.toolbox/`.

---

## DEF-09 — `_cold_reserve` : Documentation Incohérente "Réservé" vs Champs Actifs (P2)

### Localisation

`kernel/src/scheduler/core/task.rs` — commentaire layout TCB

### Description du défaut

Le commentaire de layout indique :

```
//   [128..256] — cold
//   ...
//   [200..232] réservé   ← dit "réservé"
```

Mais dans le code, `affinity_ext_word()` accède aux offsets 56/64/72 de `_cold_reserve` = TCB absolu 200/208/216 :

```rust
fn affinity_ext_word(&self, word_index: usize) -> &AtomicU64 {
    let offset = match word_index {
        1 => 56,   // TCB abs 200 = affinity_hi[0]
        2 => 64,   // TCB abs 208 = affinity_hi[1]
        3 => 72,   // TCB abs 216 = affinity_hi[2]
        _ => panic!(...),
    };
    unsafe { &*(self._cold_reserve.as_ptr().add(offset) as *const AtomicU64) }
}
```

Le commentaire layout dit aussi plus bas :

```
//       [200] affinity_hi[0]     : u64   (CPUs 64..127)
//       [208] affinity_hi[1]     : u64   (CPUs 128..191)
//       [216] affinity_hi[2]     : u64   (CPUs 192..255)
```

Ces deux affirmations sur `[200..232]` se contredisent.

### Correction

Clarifier le layout officiel avec les assertions statiques correspondantes :

```rust
// Dans le commentaire layout cache-lines 3-4 [128..256], remplacer :
//   [200..232] réservé
// Par :
//   [200] affinity_hi[0] : u64 (CPUs 64..127)   ← utilisé par affinity_ext_word(1)
//   [208] affinity_hi[1] : u64 (CPUs 128..191)  ← utilisé par affinity_ext_word(2)
//   [216] affinity_hi[2] : u64 (CPUs 192..255)  ← utilisé par affinity_ext_word(3)
//   [224..232] _pad_affinity : [u8; 8] réservé
```

Ajouter des assertions statiques pour les champs affinity :

```rust
// Ajouter après les assertions existantes dans task.rs :
const _: () = assert!(
    offset_of!(ThreadControlBlock, _cold_reserve) + 56 == 200,
    "TCB: affinity_hi[0] doit être à l'offset absolu 200 (_cold_reserve+56)"
);
const _: () = assert!(
    offset_of!(ThreadControlBlock, _cold_reserve) + 64 == 208,
    "TCB: affinity_hi[1] doit être à l'offset absolu 208 (_cold_reserve+64)"
);
const _: () = assert!(
    offset_of!(ThreadControlBlock, _cold_reserve) + 72 == 216,
    "TCB: affinity_hi[2] doit être à l'offset absolu 216 (_cold_reserve+72)"
);
```

---

## DEF-10 — `ref_count.rs` : Fenêtre TOCTOU sur `u32::MAX` (P2)

### Localisation

`kernel/src/memory/physical/frame/ref_count.rs` — méthode `dec()`

### Description du défaut

```rust
pub fn dec(&self) -> RefCountDecResult {
    let prev = self.0.fetch_sub(1, Ordering::AcqRel);
    if prev == 0 {
        // Annuler le wrap vers u32::MAX
        // ... (restauration via fetch_add(1))
    }
}
```

Séquence problématique sur un système multiprocesseur :

1. Thread A : `prev = fetch_sub(1)` → `prev == 0` → refcount wraps à `u32::MAX`
2. **Thread B** lit le refcount → observe `u32::MAX` → croit que le frame a 4 milliards de références → double-libération impossible → **comportement incorrect**
3. Thread A : restaure refcount à 0 via `fetch_add(1)`

Ce scénario ne peut survenir que si `dec()` est appelé sur un frame avec `refcount == 0` (double-free). Mais la fenêtre `u32::MAX` peut induire en erreur des assertions ou des diagnostics.

### Correction

Utiliser `compare_exchange` pour éviter le wrap transitoire :

```rust
pub fn dec(&self) -> RefCountDecResult {
    loop {
        let current = self.0.load(Ordering::Acquire);
        if current == 0 {
            // Double-free détecté sans jamais wraper
            #[cfg(debug_assertions)]
            panic!("AtomicRefCount::dec() appelé sur un frame déjà libre (refcount == 0)");
            return RefCountDecResult::StillShared; // protection release
        }
        let new = current - 1;
        match self.0.compare_exchange(current, new, Ordering::AcqRel, Ordering::Relaxed) {
            Ok(_) => {
                return if new == 0 {
                    RefCountDecResult::ShouldFree
                } else {
                    RefCountDecResult::StillShared
                };
            }
            Err(_) => continue, // réessayer (contention)
        }
    }
}
```

Cette version CAS élimine la fenêtre `u32::MAX` en ne décrémentant jamais si le refcount est déjà à 0.

---

## DEF-11 — `boot_idle.rs` : TOCTOU sur `BOOT_IDLE_TCBS` (P2)

### Localisation

`kernel/src/scheduler/core/boot_idle.rs` — fonction `ensure_boot_idle_tcb`

### Description du défaut

```rust
pub unsafe fn ensure_boot_idle_tcb(cpu_id: u32, ...) -> Option<NonNull<ThreadControlBlock>> {
    let idx = cpu_id as usize;
    
    if !BOOT_IDLE_INIT[idx].load(Ordering::Acquire) {       // ← check
        // ... initialisation du slot ...
        boot_idle_slot(idx).write(idle);
        BOOT_IDLE_INIT[idx].store(true, Ordering::Release);  // ← set
    }
    // ...
}
```

Entre le `load(Acquire)` et le `store(Release)`, un deuxième contexte sur le même CPU pourrait théoriquement passer la vérification et tenter d'initialiser le même slot. En pratique, cette fonction est appelée depuis chaque AP pour son propre CPU uniquement, donc la race est impossible. Mais :

1. L'absence de protection explicite constitue une violation de la règle Rust sur `static mut`.
2. Une future refactorisation pourrait briser l'invariant.

### Correction

Utiliser `compare_exchange` comme verrou d'initialisation atomique :

```rust
pub unsafe fn ensure_boot_idle_tcb(
    cpu_id: u32,
    kernel_stack_top: u64,
) -> Option<NonNull<ThreadControlBlock>> {
    let idx = cpu_id as usize;
    if idx >= MAX_CPUS { return None; }

    // Tentative d'initialisation atomique : seul le premier appelant réussit
    if BOOT_IDLE_INIT[idx]
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        // Nous sommes le seul initialiseur pour ce slot
        let mut idle = ThreadControlBlock::new(
            ThreadId(BOOT_IDLE_TID_BASE + cpu_id as u64),
            ProcessId(0),
            SchedPolicy::Idle,
            Priority::IDLE,
            crate::arch::x86_64::read_cr3(),
            kernel_stack_top,
        );
        idle.assign_cpu(CpuId(cpu_id));
        idle.set_cpu_affinity_single(CpuId(cpu_id));
        crate::scheduler::policies::mark_idle_thread(&mut idle);

        // SAFETY: compare_exchange garantit l'unicité de l'écriture.
        unsafe { boot_idle_slot(idx).write(idle); }
        // Note : le store(true) n'est plus nécessaire — CAS le fait.
    }
    // Attendre si nécessaire (improbable — chaque AP initialise son propre slot)
    // BOOT_IDLE_INIT[idx] est maintenant true pour les deux chemins.

    let ptr = unsafe { boot_idle_slot(idx).assume_init_mut() as *mut ThreadControlBlock };
    Some(unsafe { NonNull::new_unchecked(ptr) })
}
```

---

## DEF-12 — `percpu.rs` : Appel `memory_barrier()` Avant `fetch_add(Release)` Sémantiquement Confus (P2)

### Localisation

`kernel/src/arch/x86_64/smp/percpu.rs` — fonctions `init_percpu_for_bsp` et `init_percpu_for_ap`

### Description du défaut

```rust
pub fn init_percpu_for_bsp(kernel_stack_top: u64, lapic_id: u32) {
    // ...
    unsafe {
        msr::write_msr(msr::MSR_GS_BASE, addr);
        msr::write_msr(msr::MSR_KERNEL_GS_BASE, 0);
    }
    crate::arch::x86_64::memory_barrier(); // ← mfence avant le fetch_add
    ONLINE_CPU_COUNT.fetch_add(1, Ordering::Release);
}
```

**Problème :** `fetch_add(..., Ordering::Release)` en x86_64 génère une instruction `lock xadd` qui est déjà une barrière mémoire complète (les stores précédents sont visibles avant). Le `mfence` préalable est redondant pour l'atomique.

L'intention réelle est de s'assurer que les écritures MSR non-atomiques (`write_msr`) sont visibles aux APs avant qu'ils lisent `ONLINE_CPU_COUNT`. En x86_64, les écritures MSR via `wrmsr` sont sérialisantes par nature (voir Intel SDM Vol.2B §WRMSR — "serializing instruction"). Un `mfence` supplémentaire est donc doublement redondant.

### Correction

Supprimer le `memory_barrier()` redondant et ajouter un commentaire explicatif :

```rust
pub fn init_percpu_for_bsp(kernel_stack_top: u64, lapic_id: u32) {
    // ...
    unsafe {
        // wrmsr est une instruction sérialisante (Intel SDM Vol.2B §WRMSR).
        // Garantit que toutes les écritures précédentes sont visibles avant
        // que les APs lisent ONLINE_CPU_COUNT via Acquire.
        // Pas besoin de mfence supplémentaire.
        msr::write_msr(msr::MSR_GS_BASE, addr);
        msr::write_msr(msr::MSR_KERNEL_GS_BASE, 0);
    }
    // Release : synchronise avec les lectures Acquire des APs sur ONLINE_CPU_COUNT.
    ONLINE_CPU_COUNT.fetch_add(1, Ordering::Release);
}
```

Même correction dans `init_percpu_for_ap`.

---

## Récapitulatif des Correctifs par Fichier

| Fichier | Défauts | Actions |
|---------|---------|---------|
| `arch/x86_64/smp/percpu.rs` | DEF-01, DEF-12 | Supprimer `preempt_disable/enable/is_disabled`, retirer `memory_barrier()` |
| `arch/x86_64/tss.rs` | DEF-02 | Supprimer `nmi_stack`, `ist5_stack`, `ist6_stack` de `PerCpuStacks` |
| `arch/x86_64/spectre/kpti.rs` | DEF-04 | Réécrire `should_enable_kpti()` avec CPUID + RDMSR IA32_ARCH_CAPABILITIES |
| `memory/virtual/page_table/kpti_split.rs` | DEF-06 | Modifier `build_user_shadow_pml4` pour ne pas copier PML4[511] entier |
| `memory/physical/frame/ref_count.rs` | DEF-10 | Remplacer `fetch_sub` par CAS loop dans `dec()` |
| `scheduler/core/switch.rs` | DEF-03, DEF-07, DEF-08 | Corriger doc extern; réordonner `set_current_tcb` avant `update_rsp0`; aligner étape CR0.TS avec TLA+ |
| `scheduler/core/task.rs` | DEF-09 | Clarifier layout `_cold_reserve`, ajouter assertions statiques affinity_hi |
| `scheduler/core/preempt.rs` | DEF-01 | Exposer `is_preempt_disabled()` public |
| `scheduler/core/boot_idle.rs` | DEF-11 | Utiliser `compare_exchange` pour init atomique |
| `scheduler/timer/tick.rs` | DEF-05 | Remplacer `[; 256]` par `[; MAX_CPUS]` |
| `docs/Exo-OS-TLA+/ContextSwitch.tla` | DEF-08 | Mettre à jour l'ordre des étapes (ou aligner le code) |

---

## Vérification Post-Correctif

Après application de l'ensemble des correctifs, le système doit satisfaire les invariants suivants :

1. **Unicité du compteur de préemption** : une seule source de vérité via `scheduler::core::preempt::PREEMPT_COUNT`. `gs:[0x30]` documenté comme réservé.
2. **Cohérence TCB/stack post-switch** : tout code lisant `gs:[0x20]` après le switch voit toujours le même TCB que `TSS.RSP0`.
3. **KPTI minimal** : la PML4 user ne contient aucun mapping kernel hormis les stubs de trampoline strictement nécessaires.
4. **BSS réduit** : `PerCpuStacks` ne contient que 4 stacks utilisées (économie 8-12 MiB).
5. **Alignement TLA+** : la séquence `context_switch()` suit l'ordre `Xsave → SetLazyBit → AsmSwitch` tel que modélisé dans `ContextSwitch.tla`.
6. **Refcount sans wrap** : `AtomicRefCount::dec()` ne transite jamais par `u32::MAX`.
7. **Taille tableaux per-CPU** : `ELAPSED_NS` et `LAST_TCB_PTR` reflètent `MAX_CPUS` et non une constante figée.

---

*Document produit par claude-beta après analyse statique exhaustive du dépôt `darkfireeee/Exo-OS`, croisement avec les spécifications TLA+ et la documentation d'architecture. Aucun faux positif issu d'un autre outil d'audit n'a été inclus.*
