# ExoOS — Corrections Architecture & Boot
**Couvre : CORR-09, CORR-10, CORR-11, CORR-18, CORR-24, CORR-27**  
**Sources IAs : Gemini (§A-1), Z-AI (INCOH-03/04/05), Kimi (§2), Grok4 (SYN-06), Claude**

---

## CORR-09 🟠 — BootInfo : toujours virtuel, supprimer argv[1]

### Problème
Arborescence V4 §3 indique encore :
```
init_server (PID 1) : Reçoit CapToken ipc_broker en argv[1] du kernel
```
Architecture v7 V7-C-01 impose :
```
fn _start(boot_info_virt: usize) → adresse VIRTUELLE vers BootInfo
```
Ces deux descriptions sont **incompatibles**.

### Correction — `servers/init_server/src/main.rs`

```rust
// servers/init_server/src/main.rs — CORRECTION CORR-09
//
// Le kernel mappe la page BootInfo dans la VMA de init_server AVANT son lancement.
// init_server reçoit l'adresse VIRTUELLE de cette page comme premier argument.
// NE PAS déréférencer l'adresse physique directement — #PF garanti (paging actif).

#[no_mangle]
pub extern "C" fn _start(boot_info_virt: usize) -> ! {
    // Déréférencement via adresse VIRTUELLE (V7-C-01)
    let bi = unsafe { &*(boot_info_virt as *const BootInfo) };

    // Validation du magic et de l'intégrité
    assert_eq!(bi.magic, BOOT_INFO_MAGIC, "BootInfo magic invalide");
    assert!(bi.validate(), "BootInfo validate() échoué");

    // Vérification du CapToken ipc_broker (contenu dans BootInfo, pas argv[1])
    verify_cap_token(&bi.ipc_broker_cap, CapabilityType::IpcBroker);

    // Suite : memory_server, puis supervisor...
    supervisor::start(bi);
}

// BootInfo est mappé dans la VMA de init_server par le kernel avant exec().
// Séquence dans process/exec.rs :
//   1. Allouer une page physique pour BootInfo
//   2. Remplir les champs (ipc_broker_cap, ssr_phys_addr, nr_cpus, ...)
//   3. Mapper la page dans la VMA de init_server (adresse virtuelle fixe)
//   4. Passer cette adresse virtuelle comme premier argument de _start()
```

### Arborescence V4 §3 — correction textuelle
```
AVANT :
  init_server (PID 1) | Reçoit CapToken ipc_broker en argv[1] du kernel

APRÈS :
  init_server (PID 1) | Reçoit boot_info_virt: usize en arg[0] de _start().
                      | Le kernel mappe la page BootInfo dans la VMA avant lancement.
                      | BootInfo contient : ipc_broker_cap, ssr_phys_addr, nr_cpus, ...
                      | NE PAS passer d'argv[1] — toute info passe par BootInfo virtuel.
```

---

## CORR-10 🟠 — IPI broadcasts Kernel A : exclusion de Core 0 (Kernel B)

### Problème
Architecture v7 §3.1 définit les IPI :
- 0xF1 = reschedule (Kernel A → tous cores Kernel A)
- 0xF2 = TLB shootdown (Kernel A → tous cores Kernel A)

ExoPhoenix v6 place Kernel B sur **Core 0 dédié** avec ses propres handlers 0xF1/0xF2.  
Si Kernel A broadcaste 0xF1 vers tous les cores y compris Core 0, le handler ExoPhoenix de Kernel B se déclenche de façon inattendue.

### Correction — `kernel/src/arch/x86_64/apic/ipi.rs`

```rust
// kernel/src/arch/x86_64/apic/ipi.rs — RÈGLE IPI-EXCL-CORE0 (CORR-10)
//
// RÈGLE ABSOLUE : Les IPIs de Kernel A sur les vecteurs 0xF1, 0xF2
// NE DOIVENT PAS cibler Core 0 (APIC ID 0 = Kernel B sentinel).
// Raison : Kernel B installe ses propres handlers sur ces vecteurs.
//
// Implémentation :
// Utiliser APIC broadcast mode "All excluding self" (champ ICR Destination Shorthand = 0b11)
// filtré manuellement pour exclure Core 0.
// OU construire un masque explicite : all_cores & !{KERNEL_B_APIC_ID}.

use crate::arch::apic::KERNEL_B_APIC_ID; // = 0

/// Envoie un IPI à tous les cores Kernel A (exclut Core 0 = Kernel B).
///
/// vecteur : 0xF1 (reschedule) ou 0xF2 (TLB shootdown)
///
/// CORR-10 : Core 0 est dédié Kernel B, JAMAIS ciblé par les IPIs de Kernel A.
pub fn broadcast_ipi_excluding_b(vecteur: u8) {
    let current_apic_id = lapic::current_apic_id();
    let nr_cpus = exo_phoenix_ssr::MAX_CORES_RUNTIME.load(Ordering::Relaxed) as usize;

    for apic_id in 0..nr_cpus {
        // Exclusions : Core 0 (Kernel B) ET soi-même
        if apic_id as u32 == KERNEL_B_APIC_ID { continue; }
        if apic_id as u32 == current_apic_id   { continue; }
        lapic::send_ipi(apic_id as u32, vecteur);
    }
}

/// Envoi d'IPI TLB shootdown — CORR-10 appliqué.
pub fn send_tlb_shootdown() {
    broadcast_ipi_excluding_b(0xF2);
}

/// Envoi d'IPI reschedule — CORR-10 appliqué.
pub fn send_reschedule_ipi(target_apic_id: u32) {
    // Pour un target unique : vérifier que ce n'est pas Core 0
    assert_ne!(target_apic_id, KERNEL_B_APIC_ID,
        "IPI reschedule vers Core 0 (Kernel B) interdit — CORR-10");
    lapic::send_ipi(target_apic_id, 0xF1);
}
```

**Constante à ajouter dans `libs/exo-phoenix-ssr/src/lib.rs`** (déjà dans CORR-02) :
```rust
pub const KERNEL_B_APIC_ID: u32 = 0; // Core 0 dédié Kernel B
```

---

## CORR-11 🟠 — FS/GS base : sauvegarde/restauration dans context_switch()

### Problème
Architecture v7 TCB a `fs_base [32]` et `user_gs_base [40]`.  
Le changelog V7-C-02 dit que `switch_asm.s` ne touche pas la FPU → OK.  
Mais **aucun document ne spécifie explicitement** que `switch.rs` doit sauver/restaurer FS/GS via `rdmsr`/`wrmsr`.

**Sans ce code**, le TLS userspace (fs_base) est corrompu à chaque context switch entre threads de processus différents.

**Source** : Gemini §A-1 (Context Switch Intégral), Z-AI INCOH-01 (champs FS/GS dans TCB)

### Correction — `kernel/src/scheduler/core/switch.rs`

```rust
// kernel/src/scheduler/core/switch.rs — context_switch() v8
// CORR-11 : Ajout sauvegarde/restauration FS/GS via MSR rdmsr/wrmsr.
//
// Séquence v8 (remplace séquence v7 §3.2 Architecture) :
//
//  1. Si fpu_loaded(prev) → xsave64(prev.fpu_state_ptr)
//  2. Sauvegarder fs_base : prev.fs_base = rdmsr(0xC0000100)
//  3. Sauvegarder user_gs_base : prev.user_gs_base = rdmsr(0xC0000101)
//  4. prev.set_state(Runnable)
//  5. context_switch_asm(prev.kstack_ptr, next.kstack_ptr, next.cr3_phys)
//     ↳ dans switch_asm.s : push 6 callee-saved, swap RSP, pop 6 callee-saved
//  6. next.set_state(Running)
//  7. set_cr0_ts()             ← CR0.TS=1 (Lazy FPU — V7-C-02)
//  8. tss_set_rsp0(cpu, next.kstack_ptr)  ← V7-C-03 OBLIGATOIRE
//  9. Restaurer fs_base : wrmsr(0xC0000100, next.fs_base)
// 10. Restaurer user_gs_base : wrmsr(0xC0000101, next.user_gs_base)

pub fn context_switch(prev: &mut ThreadControlBlock, next: &mut ThreadControlBlock) {
    // Étape 1 : Sauvegarder FPU si chargée (Lazy FPU)
    if prev.fpu_state_ptr != 0 && fpu::is_fpu_loaded_for(prev.tid) {
        fpu::xsave64(prev.fpu_state_ptr as *mut XSaveArea);
    }

    // Étapes 2-3 : Sauvegarder FS/GS base (CORR-11 — NOUVEAU)
    prev.fs_base      = unsafe { arch::rdmsr(0xC000_0100) };
    prev.user_gs_base = unsafe { arch::rdmsr(0xC000_0101) };

    prev.set_state(ThreadState::Runnable);

    // Étape 5 : Context switch ASM (CR3 + 6 callee-saved GPRs)
    unsafe {
        context_switch_asm(
            &mut prev.kstack_ptr as *mut u64,
            next.kstack_ptr,
            next.cr3_phys,
        );
    }
    // Le CPU est maintenant dans le contexte de `next`

    next.set_state(ThreadState::Running);

    // Étape 7 : Lazy FPU — déclenche #NM si le thread utilise FPU
    unsafe { arch::set_cr0_ts(); }

    // Étape 8 : TSS.RSP0 obligatoire (V7-C-03)
    tss::set_rsp0(current_cpu(), next.kstack_ptr);

    // Étapes 9-10 : Restaurer FS/GS base (CORR-11 — NOUVEAU)
    unsafe {
        arch::wrmsr(0xC000_0100, next.fs_base);
        // Note : user_gs_base est restauré via WRMSR directement (pas via SWAPGS)
        // SWAPGS swap entre kernel GS et user GS à chaque entrée/sortie Ring 0.
        // Ici on est en Ring 0, donc GS.base contient le per-CPU data.
        // On écrit user_gs_base dans le MSR 0xC0000101 qui correspond à
        // la valeur userspace (sera swappée vers GS.base à SWAPGS iretq).
        arch::wrmsr(0xC000_0101, next.user_gs_base);
    }
}

// switch_asm.s — pseudo-code v8 (INCHANGÉ par rapport à v7, CORR-11 est dans switch.rs)
// context_switch_asm(prev_kstack_ptr*, next_kstack, next_cr3)
//
//   NOTE : switch_asm.s ne touche PAS FS/GS — géré par switch.rs ci-dessus.
//   NOTE : switch_asm.s ne touche PAS la FPU — Lazy FPU, seul CR0.TS=1 (V7-C-02).
//   NOTE : 6 callee-saved uniquement (ABI SysV) : rbx, rbp, r12, r13, r14, r15.
//          rip est implicite (ret/call).
//          Les caller-saved sont sauvés par le caller avant le switch.
```

**Note sur le commentaire trompeur switch_asm.s (CORR-18)** :
Le commentaire actuel "CR3 + 15 GPRs (rax..r14 + r15)" est **incorrect**. Correction :
```asm
// CORRECT : switch_asm.s sauvegarde :
// - CR3 (si différent du cr3_phys du next)
// - 6 callee-saved GPRs : rbx, rbp, r12, r13, r14, r15
// - rip implicitement (via call/ret)
//
// Ce n'est PAS 15 GPRs. Le TCB contient 15 GPRs pour le contexte COMPLET
// d'un thread interrompu par IRQ (où le CPU a déjà empilé rip/cs/rflags/rsp/ss
// et le handler a empilé les caller-saved). Ces deux contextes sont distincts.
```

---

## CORR-18 🟠 — switch_asm.s : correction du commentaire "15 GPRs"

### Problème
Le commentaire dans Architecture v7 §3.2 et `asm/switch_asm.s` dit :
```
// CR3 + 15 GPRs (rax..r14 + r15)
```
C'est **faux** pour un yield coopératif. switch_asm.s sauvegarde uniquement les **6 callee-saved**.

**Source** : MiniMax ES-05, Z-AI INCOH-03

### Explication complète

| Contexte | GPRs sauvés | Où |
|----------|-------------|-----|
| Yield coopératif (`switch_asm.s`) | 6 callee-saved (rbx, rbp, r12..r15) | pile kernel |
| Interruption/préemption (ISR frame) | Tous 15 GPRs + CPU auto (rip,cs,rflags,rsp,ss) | pile ISR |
| TCB layout | 15 GPRs | pour restore depuis IRQ context |

**Le TCB est dimensionné pour le cas le plus défavorable (IRQ préemptif)**.  
**switch_asm.s est optimisé pour le yield coopératif (6 GPRs)**.  
Ces deux choses sont correctes et compatibles — c'est le commentaire qui était trompeur.

### Correction du commentaire — `kernel/src/arch/x86_64/asm/switch_asm.s`
```asm
// context_switch_asm — Yield coopératif uniquement
//
// Sauvegarde : 6 callee-saved GPRs (SysV ABI x86_64) + RSP
// Push : rbx, rbp, r12, r13, r14, r15   → 6×8 = 48B + rip implicite
//
// NB : switch_asm.s NE SAUVEGARDE PAS les 15 GPRs du TCB.
//      Les GPRs caller-saved (rax, rcx, rdx, rsi, rdi, r8..r11) sont
//      sous la responsabilité du caller AVANT d'appeler context_switch().
//      Le TCB contient 15 GPRs pour le cas d'une préemption par IRQ,
//      qui est géré séparément dans l'ISR handler.
//
// NB : switch_asm.s NE TOUCHE PAS la FPU (Lazy FPU — V7-C-02).
// NB : switch_asm.s NE TOUCHE PAS FS/GS base (géré par switch.rs — CORR-11).
```

---

## CORR-24 ⚠️ — SeqLock Phase 9 : ajouter à la roadmap

### Problème
Driver Framework v10 et Kernel Types v10 documentent "ExoOS_SeqLock_Design.md (Phase 9)" comme fichier "à créer". Ce document n'existe pas et Phase 9 n'est pas dans la roadmap Architecture v7 §11.

### Correction — Architecture v7 §11 (ajout Phase 5)

```markdown
### Phase 5 — NMI-Safety & Preuve Formelle

**SeqLock NMI-safe pour PciTopology**
- Fichier : `docs/ExoOS_SeqLock_Design.md` (à créer)
- Remplace `spin::RwLock<heapless::Vec>` dans `pci_topology.rs`
- Pattern : `seqlock: AtomicU64` (impair = écriture en cours)
- Lecteurs (`parent_bridge`) : wait-free, NMI-safe, retry si seq impair
- Écrivains (`register`) : incr seq (impair), modifier, incr seq (pair), irq_save
- Élimine la limitation documentée FIX-105 v10

**Preuve formelle TLA+/Spin**
- Automate ExoPhoenix v5 (bloque SMP uniquement)
- Valide le protocole handoff SSR (HANDOFF_FLAG états 0→1→2→3)
- Prérequis pour déploiement multi-cœur Phase 9+

**IOMMU domains complets Ring 1 (S-29)**
- Chaque driver Ring 1 → domaine IOMMU dédié
- Voir `IommuDomainRegistry` (CORR-23, fichier 03_Driver_Framework)
```

---

## CORR-27 🔵 — MAX_CPUS preempt.rs : 64 → 256

### Problème
Architecture v7 §2.3 note :
```
MAX_CPUS (preempt) : 64 ⚠️ → corriger 256 — Phase 0 obligatoire
```
Cette correction est marquée "Phase 0" (immédiate) mais n'a pas été faite.

### Correction — `kernel/src/scheduler/core/preempt.rs`

```rust
// kernel/src/scheduler/core/preempt.rs
// CORR-27 : correction Phase 0 obligatoire (Architecture v7 §2.3, S-26)

/// Nombre maximum de CPUs supportés par le scheduler.
/// Doit être ≥ SSR_MAX_CORES_LAYOUT (256) pour cohérence avec ExoPhoenix.
/// ÉTAIT : 64 — CORRIGÉ : 256
pub const MAX_CPUS: usize = 256;

// Vérification compile-time
const _: () = assert!(MAX_CPUS >= exo_phoenix_ssr::SSR_MAX_CORES_LAYOUT);
```

**CI check à ajouter** :
```bash
# CI/Makefile — check S-26
grep -r "MAX_CPUS" kernel/src/scheduler/core/preempt.rs \
  | grep -q "256" || { echo "VIOLATION S-26 : MAX_CPUS != 256"; exit 1; }
```

---

*ExoOS — Corrections Architecture — Mars 2026*
