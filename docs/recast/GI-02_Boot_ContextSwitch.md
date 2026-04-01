# ExoOS — Guide d'Implémentation GI-02
## Boot Séquence, Context Switch, FPU Lazy, MSR FS/GS

**Prérequis** : GI-01 (types partagés compilant sans erreur)  
**Produit** : `kernel/src/arch/x86_64/boot/`, `scheduler/core/switch.rs`, `switch_asm.s`

**Traçabilité des IDs (vérifiée, mars 2026)** : `CORR-34` est défini dans `ExoOS_Corrections_07_Critiques_Majeures_v2.md`. `FIX-100` et `FIX-103` sont des IDs **FIX** historiques (v8/v10), documentés dans `ExoOS_Driver_Framework_v10.md` / `ExoOS_Kernel_Types_v10.md` (hors numérotation `CORR-*`).

---

## 1. Ordre d'Implémentation

```
Étape 1 : early_init.rs étapes 1-10  ← Boot ROM → APIC → IOMMU
Étape 2 : time.rs                    ← BOOT_TSC_KHZ AtomicU64 + calibrate_tsc_khz()
Étape 3 : Séquence boot étapes 11-14 ← Mémoire complète + IPI TLB
Étape 4 : Séquence boot étapes 15-18 ← Scheduler + APs + SECURITY_READY
Étape 5 : switch_asm.s               ← ASM context switch (CR3 + 6 GPRs)
Étape 6 : switch.rs context_switch() ← FS/GS + FPU + TSS.RSP0
Étape 7 : fpu/lazy.rs                ← #NM handler + xsave64/xrstor64
Étape 8 : tss.rs                     ← TSS.RSP0 helper
```

---

## 2. time.rs — Calibration TSC

```rust
// kernel/src/time.rs
//
// ERREURS GRAVES À ÉVITER :
//
// ❌ static BOOT_TSC_KHZ: u64 = 0;  → ERREUR DE COMPILATION
//    En Rust, les static globaux sont immutables.
//    calibrate_tsc_khz() ne peut pas assigner à un static immuable.
//    → E0594: cannot assign to immutable static item
//
// ❌ static mut BOOT_TSC_KHZ: u64 = 0; → UNSAFE REQUIS + NON THREAD-SAFE
//    Lecture concurrente depuis plusieurs CPUs = UB
//
// ✅ static BOOT_TSC_KHZ: AtomicU64 = AtomicU64::new(0);
//    → Lecture safe depuis tous les CPUs (Ordering::Relaxed suffit)
//    → La barrière enable_interrupts() garantit la visibilité
//
// ORDRE IMPÉRATIF :
//   calibrate_tsc_khz()    AVANT enable_interrupts()
//   enable_interrupts()    = barrière mémoire implicite
//   → current_time_ms()    peut être appelé après enable_interrupts()
//
// ERREUR SILENCIEUSE : appeler current_time_ms() avant calibrate_tsc_khz()
//   → BOOT_TSC_KHZ = 0 → division par zéro (ou résultat infini)
//   → Watchdog IRQ se déclenche immédiatement (tous les masked_since = énorme)

use core::sync::atomic::{AtomicU64, Ordering};
use core::arch::x86_64::_rdtsc;

static BOOT_TSC_KHZ:  AtomicU64 = AtomicU64::new(0);
static BOOT_TSC_BASE: AtomicU64 = AtomicU64::new(0);
static PHOENIX_MS_OFFSET: AtomicU64 = AtomicU64::new(0);

/// Calibre le TSC via le PIT (Programmable Interval Timer).
///
/// DOIT être appelé :
///   - AVANT enable_interrupts()
///   - UNE SEULE FOIS au boot
///   - Depuis le CPU bootstrap (BSP)
///
/// COMMENT ÇA MARCHE (PIT calibration 50ms) :
///   1. Lire TSC avant
///   2. Attendre 50ms via PIT (polling sur port 0x61 bit 5)
///   3. Lire TSC après
///   4. khz = (tsc_after - tsc_before) / 50
pub fn calibrate_tsc_khz() {
    // ─── Vérification invariant TSC ─────────────────────────────────
    // CPUID.80000007H:EDX[8] = Invariant TSC
    // Sans TSC invariant, la fréquence varie selon C-states et P-states
    let invariant = check_invariant_tsc();
    if !invariant {
        log::warn!("TSC non-invariant — calibration peut être inexacte");
        // En Phase 8 : utiliser HPET comme fallback si disponible
    }

    // ─── Mesure via PIT ──────────────────────────────────────────────
    let tsc_start = unsafe { _rdtsc() };
    pit_wait_50ms(); // Attente blocking sur PIT (interruptions désactivées)
    let tsc_end   = unsafe { _rdtsc() };

    let delta_tsc = tsc_end.saturating_sub(tsc_start);
    let measured_khz = delta_tsc / 50; // ticks / 50ms = ticks/ms = KHz

    if measured_khz == 0 {
        // Fallback si mesure échoue (hardware exotique)
        let fallback_khz = 3_000_000u64; // 3 GHz approximatif
        log::error!("TSC calibration failed — using fallback {}KHz", fallback_khz);
        BOOT_TSC_KHZ.store(fallback_khz, Ordering::Relaxed);
    } else {
        BOOT_TSC_KHZ.store(measured_khz, Ordering::Relaxed);
    }

    // Capturer la base TSC pour le calcul différentiel (CORR-34)
    let tsc_now = unsafe { _rdtsc() };
    BOOT_TSC_BASE.store(tsc_now, Ordering::Relaxed);
    // Ordering::Relaxed SUFFISANT car enable_interrupts() qui suit
    // agit comme barrière mémoire implicite (serializing instruction)
}

/// Temps monotone en ms depuis le boot.
///
/// ERREURS SILENCIEUSES :
///   - Appeler avant calibrate_tsc_khz() → résultat 0 ou astronomique
///   - Appeler depuis ISR avant calibration → watchdog faux positifs
pub fn current_time_ms() -> u64 {
    let khz = BOOT_TSC_KHZ.load(Ordering::Relaxed);
    debug_assert!(khz > 0,
        "current_time_ms() appelé avant calibrate_tsc_khz()");

    let tsc_now  = unsafe { _rdtsc() };
    let tsc_base = BOOT_TSC_BASE.load(Ordering::Relaxed);
    let offset   = PHOENIX_MS_OFFSET.load(Ordering::Relaxed);
    let delta    = tsc_now.saturating_sub(tsc_base);
    offset.saturating_add(delta / khz.max(1))
}
```

---

## 3. switch_asm.s — Règles Critiques

```asm
; kernel/src/arch/x86_64/asm/switch_asm.s
;
; RÈGLES ABSOLUES (violations = crash immédiat ou corruption silencieuse) :
;
; RÈGLE ASM-01 : JAMAIS toucher les registres SSE/MMX/AVX (xmm*, ymm*, zmm*)
;   → Cela appartient aux processus Ring 3 (Lazy FPU)
;   → Le target JSON a -mmx,-sse qui empêche le compilateur de les utiliser,
;     mais l'ASM explicite peut les utiliser accidentellement
;
; RÈGLE ASM-02 : JAMAIS toucher MXCSR ou FCW
;   → Gérés exclusivement par fpu/lazy.rs via XSAVE/XRSTOR
;
; RÈGLE ASM-03 : Le code sauvegarde SEULEMENT les 6 callee-saved (SysV ABI)
;   → rbx, rbp, r12, r13, r14, r15
;   → rip est implicite (via call/ret)
;   → Les caller-saved sont sous la responsabilité du caller Rust
;
; RÈGLE ASM-04 : Le switch doit être atomique du point de vue du scheduler
;   → Pas d'interruption possible entre la sauvegarde de RSP et la restauration
;   → Les interruptions sont désactivées AVANT l'appel par le caller
;
; COMMENTAIRE SUR "15 GPRs dans le TCB vs 6 dans switch_asm.s" :
;   Ce n'est PAS une contradiction. Le TCB contient 15 GPRs pour le cas
;   d'une PRÉEMPTION par IRQ (le handler IRQ empile tous les GPRs).
;   switch_asm.s est pour le YIELD COOPÉRATIF uniquement (6 callee-saved).

.global context_switch_asm
.code64

; extern "C" fn context_switch_asm(
;     prev_kstack_ptr_location: *mut u64,  // rdi = &prev.kstack_ptr
;     next_kstack_ptr:          u64,       // rsi = next.kstack_ptr
;     next_cr3_phys:            u64,       // rdx = next.cr3_phys
; )
context_switch_asm:
    ; Sauvegarder les 6 registres callee-saved sur la pile kernel du prev
    push %rbx
    push %rbp
    push %r12
    push %r13
    push %r14
    push %r15
    ; 6 × 8 = 48 bytes + rip implicite = 56 bytes total sur pile

    ; Sauvegarder RSP du prev dans prev.kstack_ptr
    mov  %rsp, (%rdi)

    ; Changer l'espace d'adressage si nécessaire (KPTI)
    ; Optimisation : skip si même CR3 (processus multi-threads)
    cmp  %rdx, %cr3
    je   .skip_cr3
    mov  %rdx, %cr3     ; TLB flush implicite pour le nouveau PML4
.skip_cr3:

    ; Charger RSP du next depuis next.kstack_ptr
    mov  %rsi, %rsp

    ; Restaurer les 6 registres callee-saved du next
    pop  %r15
    pop  %r14
    pop  %r13
    pop  %r12
    pop  %rbp
    pop  %rbx

    ; RET : retourne dans le contexte du next thread
    ; (l'adresse de retour était sur la pile de next depuis son dernier switch)
    ret

; NOTE : CR0.TS et TSS.RSP0 sont gérés dans switch.rs APRÈS le retour
; de context_switch_asm. Ne pas les modifier dans l'ASM.
```

---

## 4. switch.rs — FS/GS Base + TSS.RSP0

```rust
// kernel/src/scheduler/core/switch.rs
//
// COMPRENDRE FS/GS SUR x86_64 :
//
// Il y a 3 MSRs liés à GS :
//   MSR 0xC0000100 (IA32_FS_BASE)       = FS.base actuel
//   MSR 0xC0000101 (IA32_GS_BASE)       = GS.base actuel (user GS ou kernel GS)
//   MSR 0xC0000102 (IA32_KERNEL_GS_BASE) = GS.base "caché" (swappé par SWAPGS)
//
// SWAPGS échange GS.base et KERNEL_GS_BASE.
// Séquence kernel entry (depuis Ring 3) :
//   Ring 3 : GS.base = user_gs (TLS userspace)
//   SWAPGS : GS.base <-> KERNEL_GS_BASE = maintenant GS.base = kernel per-CPU
//   kernel s'exécute avec GS.base = per-CPU data
// Séquence kernel exit (vers Ring 3) :
//   SWAPGS : GS.base <-> KERNEL_GS_BASE = GS.base = user_gs à nouveau
//   IRETQ → Ring 3 exécute avec GS.base = user_gs
//
// QUE SAUVEGARDER DANS TCB.user_gs_base ?
//   La valeur userspace de GS.base (celle de Ring 3).
//   C'est la valeur dans IA32_KERNEL_GS_BASE quand on est en Ring 0
//   (puisque SWAPGS les a échangées à l'entrée kernel).
//
// ERREUR SILENCIEUSE CLASSIQUE :
//   Sauvegarder GS.base (0xC0000101) au lieu de KERNEL_GS_BASE (0xC0000102)
//   → On sauvegarde la valeur kernel (per-CPU), pas la valeur user
//   → Le TLS Ring 3 est corrompu après chaque context switch
//   → Fonctionne si un seul thread, plante avec plusieurs threads Ring 3

pub fn context_switch(
    prev: &mut ThreadControlBlock,
    next: &mut ThreadControlBlock,
) {
    // ─── Étape 1 : Sauvegarder FPU si chargée ─────────────────────────
    // Vérifier CR0.TS : 0 = FPU active dans registres, 1 = lazy (pas de sauvegarde)
    if !is_cr0_ts_set() && prev.fpu_state_ptr != 0 {
        unsafe { fpu::xsave64(prev.fpu_state_ptr as *mut XSaveArea); }
        fpu::mark_fpu_not_loaded(prev.tid);
    }

    // ─── Étape 2 : Sauvegarder FS.base du prev ────────────────────────
    // RDMSR sérialise les instructions → nécessaire pour mémoire cohérente
    prev.fs_base = unsafe { rdmsr(IA32_FS_BASE) };

    // ─── Étape 3 : Sauvegarder user GS.base du prev ───────────────────
    // ATTENTION : On est en Ring 0 → SWAPGS a été fait à l'entrée kernel
    // Donc GS.base actuel (0xC0000101) = kernel per-CPU
    // Et KERNEL_GS_BASE (0xC0000102) = user_gs_base
    // → Lire 0xC0000102 pour obtenir la valeur userspace
    prev.user_gs_base = unsafe { rdmsr(IA32_KERNEL_GS_BASE) };

    prev.set_state(ThreadState::Runnable);

    // ─── Étape 4 : Context switch ASM ─────────────────────────────────
    // Les interruptions DOIVENT être désactivées avant d'arriver ici
    // (garantie par le caller - scheduler avec irq_save())
    unsafe {
        context_switch_asm(
            &mut prev.kstack_ptr as *mut u64,
            next.kstack_ptr,
            next.cr3_phys,
        );
    }
    // À partir d'ici, on est dans le contexte de `next`

    next.set_state(ThreadState::Running);

    // ─── Étape 5 : CR0.TS = 1 (Lazy FPU) ─────────────────────────────
    // Déclenche #NM si next essaie d'utiliser la FPU sans l'avoir chargée
    unsafe { set_cr0_ts(); }

    // ─── Étape 6 : TSS.RSP0 obligatoire (V7-C-03) ─────────────────────
    // ERREUR SILENCIEUSE si oublié :
    //   La prochaine IRQ Ring 3→Ring 0 empile sur la pile du PREV thread
    //   → Corruption silencieuse de la pile de prev
    //   → Pas de crash immédiat : crash aléatoire plus tard
    tss::set_rsp0(current_cpu(), next.kstack_ptr);

    // ─── Étape 7 : Restaurer FS.base du next ──────────────────────────
    unsafe { wrmsr(IA32_FS_BASE, next.fs_base); }

    // ─── Étape 8 : Restaurer user GS.base du next ─────────────────────
    // Écrire dans KERNEL_GS_BASE (0xC0000102)
    // → Sera swappé dans GS.base par SWAPGS à IRETQ vers Ring 3
    unsafe { wrmsr(IA32_KERNEL_GS_BASE, next.user_gs_base); }
}

// MSR helpers (wrappers safe autour de asm inline)
unsafe fn rdmsr(msr: u32) -> u64 {
    let (eax, edx): (u32, u32);
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") eax,
        out("edx") edx,
        options(nostack, nomem, preserves_flags)
    );
    ((edx as u64) << 32) | (eax as u64)
}

unsafe fn wrmsr(msr: u32, val: u64) {
    let eax = val as u32;
    let edx = (val >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") eax,
        in("edx") edx,
        options(nostack, nomem, preserves_flags)
    );
}

// Constantes MSR
const IA32_FS_BASE:       u32 = 0xC000_0100;
const IA32_GS_BASE:       u32 = 0xC000_0101;
const IA32_KERNEL_GS_BASE:u32 = 0xC000_0102;
```

---

## 5. FPU Lazy — Implémentation #NM Handler

```rust
// kernel/src/scheduler/fpu/lazy.rs
//
// MODÈLE LAZY FPU :
//   - Principe : Ne sauvegarder/restaurer l'état FPU qu'en cas d'utilisation
//   - Mécanisme : CR0.TS=1 après chaque switch → prochain usage FPU = #NM
//   - #NM handler : allouer/restaurer l'état FPU du thread courant
//
// ERREURS GRAVES :
//
// ❌ Ne pas appeler fninit() sur la PREMIÈRE utilisation FPU d'un thread
//    → Thread démarre avec l'état FPU aléatoire du CPU
//    → Calculs flottants incorrects, silencieux en production
//
// ❌ Utiliser xrstor64() sur fpu_state_ptr==null
//    → Segfault (ou pire : lecture aléatoire)
//    → Toujours allouer ET initialiser AVANT xrstor64()
//
// ❌ Oublier de nettoyer CR0.TS après alloc FPU (CLTS)
//    → Le thread continue à recevoir des #NM à chaque instruction FPU
//    → Busy loop dans le handler #NM

/// Handler pour l'exception #NM (Device Not Available).
/// Appelé par le CPU quand CR0.TS=1 et qu'une instruction FPU est exécutée.
pub fn handle_nm_exception() {
    // CLTS = Clear Task Switched bit (CR0.TS = 0)
    // DOIT être fait en premier pour éviter re-déclenchement #NM immédiat
    unsafe { clear_cr0_ts(); }

    let tcb = scheduler::current_tcb_mut();

    if tcb.fpu_state_ptr == 0 {
        // ─── Première utilisation FPU pour ce thread ──────────────────
        // Allouer une XSaveArea alignée sur 64B
        let xsave_size = get_xsave_size(); // Via CPUID leaf 0Dh sub-leaf 0
        let layout = Layout::from_size_align(xsave_size, 64)
            .expect("xsave layout");
        let ptr = unsafe { alloc::alloc_zeroed(layout) };
        if ptr.is_null() {
            // OOM = terminer le thread (pas le kernel)
            scheduler::kill_current_thread(ExitCode::OutOfMemory);
            return;
        }
        tcb.fpu_state_ptr = ptr as u64;

        // Initialiser l'état FPU à un état propre connu
        unsafe {
            // FNINIT : initialise le x87 FPU (pile, contrôle, status)
            core::arch::asm!("fninit", options(nostack, nomem));
            // VZEROUPPER : zeroise les registres AVX (si disponible)
            #[cfg(target_feature = "avx")]
            core::arch::asm!("vzeroupper", options(nostack, nomem));
            // Créer un état initial valide dans fpu_state_ptr
            xsave64(tcb.fpu_state_ptr as *mut XSaveArea);
        }
    } else {
        // ─── Restauration de l'état FPU sauvegardé ────────────────────
        unsafe { xrstor64(tcb.fpu_state_ptr as *const XSaveArea); }
    }

    fpu::mark_fpu_loaded(tcb.tid);
    // Ne pas remettre CR0.TS ici — le thread peut utiliser la FPU maintenant
}

/// Taille de la XSaveArea (variable selon le CPU).
/// Détectée via CPUID leaf 0Dh sub-leaf 0, registre ECX.
pub fn get_xsave_size() -> usize {
    let (_, _, ecx, _) = cpuid(0x0D, 0);
    ecx as usize // Taille totale pour XSAVE
}

// Implémentation xsave64 / xrstor64 via inline ASM
unsafe fn xsave64(ptr: *mut XSaveArea) {
    core::arch::asm!(
        "xsave64 [{ptr}]",
        ptr = in(reg) ptr,
        in("eax") u32::MAX,  // Masque des composants à sauvegarder (tous)
        in("edx") u32::MAX,
        options(nostack)
    );
}

unsafe fn xrstor64(ptr: *const XSaveArea) {
    core::arch::asm!(
        "xrstor64 [{ptr}]",
        ptr = in(reg) ptr,
        in("eax") u32::MAX,  // Masque des composants à restaurer (tous)
        in("edx") u32::MAX,
        options(nostack)
    );
}
```

---

## 6. TSS — Mise à Jour RSP0

```rust
// kernel/src/arch/x86_64/tss.rs
//
// TSS.RSP0 = adresse de la pile kernel utilisée lors d'une IRQ Ring 3→Ring 0
//
// ERREUR SILENCIEUSE V7-C-03 :
//   Si TSS.RSP0 pointe sur la pile du PREV thread après un context switch,
//   la prochaine IRQ empile sur la pile du mauvais thread.
//   → Corruption de la pile du prev, crash aléatoire
//   → Particulièrement difficile à déboguer : le crash survient plus tard
//
// QUAND METTRE À JOUR :
//   1. Après CHAQUE context_switch() — obligatoire
//   2. Après exec() — le nouveau thread a une pile différente
//   3. Lors de la création du premier thread d'un processus

/// Met à jour TSS.RSP0 pour le CPU courant.
///
/// DOIT être appelé APRÈS context_switch_asm() et AVANT la fin de context_switch().
pub fn set_rsp0(cpu_id: usize, kstack_ptr: u64) {
    // Obtenir le TSS du CPU courant (un TSS par CPU en SMP)
    let tss = unsafe { &mut TSS_TABLE[cpu_id] };
    tss.rsp0 = kstack_ptr;
    // Pas de flush requis — le CPU lit TSS.RSP0 à chaque entrée Ring 3→0
}

// TSS Table statique — un TSS par CPU
static mut TSS_TABLE: [TaskStateSegment; MAX_CPUS] = {
    const TSS_INIT: TaskStateSegment = TaskStateSegment::new();
    [TSS_INIT; MAX_CPUS]
};
```

---

## 7. Séquence Boot — 18 Étapes

```rust
// kernel/src/arch/x86_64/boot/early_init.rs
//
// ORDRE IMPÉRATIF — les violations créent des bugs silencieux :
//
// ❌ ERREUR : calibrate_tsc_khz() APRÈS enable_interrupts()
//    → La mesure PIT est perturbée par les interruptions = calibration fausse
//    → Watchdog IRQ se déclenche dans le mauvais délai
//    → Difficile à détecter : le système fonctionne mais le timing est faux
//
// ❌ ERREUR : register_tlb_ipi_sender() AVANT buddy allocator (step 11)
//    → Le sender IPI ne peut pas allouer le buffer de requêtes
//    → TLB shootdowns silencieusement ignorés → pages stales en SMP
//
// ❌ ERREUR : APs franchissent spin-wait AVANT SECURITY_READY (step 18)
//    → APs exécutent du code Ring 1 avec les capabilities non initialisées
//    → Élévation de privilège silencieuse (CVE-EXO-001)

pub fn early_init() {
    // ─── Étapes 1-3 : Bootstrap ──────────────────────────────────────
    detect_boot_protocol();  // Multiboot2 ou UEFI
    parse_memory_map();      // E820 ou UEFI memory map

    // ─── Étape 4 : Emergency Pool AVANT tout ─────────────────────────
    // Nécessaire pour les allocations avant que le buddy soit prêt
    EMERGENCY_POOL.init(); // FRAMES=256, WAITNODES=256 en BSS statique

    // ─── Étapes 5-7 : Architecture bas niveau ────────────────────────
    bootstrap_bitmap_allocator();
    init_paging_kpti();
    init_gdt_idt();  // Réserve 0xF0-0xFF + 0xF3 pour ExoPhoenix

    // ─── Étapes 8-9 : CPU ─────────────────────────────────────────────
    init_kernel_stack();
    init_per_cpu_gs();  // GS.base = per-CPU data
    detect_cpuid_features();

    // ─── Étape 10 : APIC + IOMMU ──────────────────────────────────────
    init_local_apic();
    init_ioapic();
    init_iommu_vtd(); // VT-d si disponible

    // ─── Étape 11 : IOMMU Fault Queue AVANT interrupts ────────────────
    // OBLIGATOIRE AVANT enable_interrupts() (FIX-100, ID FIX v8 — hors CORR)
    IOMMU_FAULT_QUEUE.init();

    // ─── Étape 12 : Calibrer TSC AVANT enable_interrupts() ────────────
    // OBLIGATOIRE AVANT enable_interrupts() (FIX-103, ID FIX v10 — hors CORR)
    calibrate_tsc_khz();

    // ─── Étape 13 : Activer les interruptions ─────────────────────────
    // Barrière mémoire implicite = BOOT_TSC_KHZ visible sur tous CPUs
    arch::enable_interrupts();

    // ─── Étape 14 : Sous-système mémoire complet ──────────────────────
    init_buddy_allocator();
    init_slub_allocator();
    init_per_cpu_allocator();
    init_numa_allocator();
    init_kernel_address_space();

    // ─── Étape 15 : Register IPI TLB sender ───────────────────────────
    // APRÈS buddy (step 14) — peut allouer (MEM-01)
    memory::register_tlb_ipi_sender();

    // ─── Étape 16 : ACPI + topology ───────────────────────────────────
    parse_acpi_madt();
    parse_acpi_hpet();

    // ─── Step 14* : Vérification MAX_CORES (V7-C-05) ──────────────────
    let runtime_cores = cpuid_detected_cores() as usize;
    if runtime_cores > SSR_MAX_CORES_LAYOUT {
        log_error!("FATAL: {} CPUs > SSR layout {}", runtime_cores, SSR_MAX_CORES_LAYOUT);
        kernel_halt_diagnostic(HaltCode::SSR_OVERFLOW);
    }
    MAX_CORES_RUNTIME.store(runtime_cores as u32, Ordering::Release);

    // ─── Étape 17 : Allouer piles APs ─────────────────────────────────
    alloc_ap_kernel_stacks(); // Via buddy (step 14)

    // ─── Étape 18 : Scheduler + APs ───────────────────────────────────
    init_scheduler_and_run_queues();
    start_application_processors_sipi(); // APs entrent en spin-wait
    // APs attendent SECURITY_READY == true (CVE-EXO-001)

    // ─── Étape 19 : IPC + ExoFS ───────────────────────────────────────
    init_ipc_subsystem();
    init_exofs_and_mount(); // boot_recovery_sequence si nécessaire

    // ─── Étape 20 : SECURITY_READY = true ─────────────────────────────
    // Les APs peuvent maintenant franchir le spin-wait
    security::init(); // SECURITY_READY.store(true, Release)
    // Les APs lisent SECURITY_READY avec Acquire et sortent du spin-wait
}
```

---

## 8. Erreurs Silencieuses Spécifiques au Boot

| Erreur | Symptôme | Détection |
|--------|----------|-----------|
| IOMMU init après IRQ IOMMU | Premières fautes IOMMU perdues | Debug assert dans push() |
| TSC calibré après IRQ | Calibration 10-20% erronée | log temps anormaux |
| APs avant SECURITY_READY | Caps non vérifiées | CVE-EXO-001 |
| IPI TLB avant buddy | TLB stales en SMP | crash rare multi-thread |
| `wrmsr(IA32_GS_BASE)` vs `IA32_KERNEL_GS_BASE` | TLS Ring 3 corrompu | crash aléatoire threads |
| `fpu_state_ptr=0` + xrstor64() | Segfault ou corruption | debug_assert!(ptr != 0) |
| Pas de `tss_set_rsp0()` | Mauvaise pile IRQ | crash difficile à tracer |

---

## 9. Tests de Validation Phase 1

```bash
# Boot minimal sur QEMU (1 CPU)
qemu-system-x86_64 \
  -kernel target/x86_64-exoos-kernel/debug/kernel \
  -serial stdio -display none -m 512M \
  -append "loglevel=debug"
# ATTENDU dans la sortie série :
# [BOOT] TSC calibrated: XXXXX KHz
# [BOOT] SECURITY_READY set
# [BOOT] Kernel halted

# Boot SMP (4 CPUs)
qemu-system-x86_64 -smp 4 ...
# ATTENDU : 4 APs font "AP CPU X spinning on SECURITY_READY"
# puis "AP CPU X released, continuing"

# Test context switch (2 threads)
# Créer 2 threads kernel qui se font context_switch mutuellement
# Vérifier que FS.base est correctement restauré entre les switches
```

---

*ExoOS — Guide d'Implémentation GI-02 : Boot, Context Switch, FPU — Mars 2026*
