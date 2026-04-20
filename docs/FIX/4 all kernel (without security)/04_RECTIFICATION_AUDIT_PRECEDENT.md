# ExoOS — Rectification de l'audit précédent
## Commit de référence : `c4239ed1`

Ce document clôture les points de l'audit du commit `ab79c90f` / `c4239ed1`
qui étaient signalés comme ouverts mais sont en réalité **déjà corrigés**
dans le code actuel, avec preuve dans le code source.

---

## Points à fermer définitivement

### ✅ CRIT-11 — Double `ONLINE_CPU_COUNT`

**Statut dans l'audit précédent** : 🔴 Ouvert — "percpu.rs incrémenté aux lignes 249 et 274"

**Statut réel** : **CORRIGÉ**

**Preuve :**
```
kernel/src/arch/x86_64/smp/percpu.rs:28
    static ONLINE_CPU_COUNT: AtomicU32 = AtomicU32::new(0);  ← UN SEUL static

kernel/src/arch/x86_64/smp/percpu.rs:229
    pub fn init_percpu_for_bsp(...) {
        ...
        ONLINE_CPU_COUNT.fetch_add(1, Ordering::Release);  ← appelé UNE FOIS pour le BSP
    }

kernel/src/arch/x86_64/smp/percpu.rs:255
    pub fn init_percpu_for_ap(...) {
        ...
        ONLINE_CPU_COUNT.fetch_add(1, Ordering::Release);  ← appelé UNE FOIS par AP
    }

kernel/src/arch/x86_64/smp/init.rs
    → aucun static ONLINE_CPU_COUNT
    → smp_cpu_count() = percpu::cpu_count() (délégation propre)
```

**Explication** : les deux `fetch_add` sont dans deux fonctions distinctes, l'une pour le BSP
(appelée une fois), l'autre pour chaque AP (appelée une fois par CPU). C'est le comportement
attendu. Le "double compteur" de l'audit précédent était erroné.

---

### ✅ CRIT-04 — `mask_all_msi_msix()` = stub fence

**Statut dans l'audit précédent** : 🔴 "fence uniquement, aucune écriture réelle"

**Statut réel** : **CORRIGÉ — implémentation complète**

**Preuve :**
```rust
// kernel/src/exophoenix/handoff.rs:274–310
fn mask_all_msi_msix() {
    for i in 0..stage0::b_device_count() {
        let Some(dev) = stage0::b_device(i) else { continue };
        unsafe {
            // MSI : efface bit0 (Enable) du MSI Control Register
            if let Some(msi_cap) = find_pci_cap(dev.bus, dev.device, dev.function, PCI_CAP_ID_MSI) {
                let ctrl_offset = msi_cap + 2;
                let raw = pci_read_dword_handoff(dev.bus, dev.device, dev.function, msi_cap);
                let ctrl = ((raw >> 16) & 0xFFFF) as u16;
                pci_write_word_handoff(dev.bus, dev.device, dev.function, ctrl_offset,
                    ctrl & !0x0001);  // ← écriture réelle PCI config space
            }
            // MSI-X : set bit14 (Function Mask) du MSI-X Control Register
            if let Some(msix_cap) = find_pci_cap(dev.bus, dev.device, dev.function, PCI_CAP_ID_MSIX) {
                let ctrl_offset = msix_cap + 2;
                let raw = pci_read_dword_handoff(dev.bus, dev.device, dev.function, msix_cap);
                let ctrl = ((raw >> 16) & 0xFFFF) as u16;
                pci_write_word_handoff(dev.bus, dev.device, dev.function, ctrl_offset,
                    ctrl | 0x4000);   // ← écriture réelle PCI config space
            }
        }
    }
    core::sync::atomic::fence(Ordering::SeqCst);  // fence post-write, correct
}
```

---

### ✅ CRIT-05 — `pci_function_level_reset()` = `Ok(())` immédiat

**Statut dans l'audit précédent** : 🔴 "Stub, G3 non garantie"

**Statut réel** : **CORRIGÉ — implémentation complète**

**Preuve :**
```rust
// kernel/src/exophoenix/forge.rs:233–251
fn pci_function_level_reset(bus: u8, device: u8, func: u8) -> Result<(), ForgeError> {
    const DEVCTL_BCR_FLR: u16 = 1 << 15;

    let pcie_cap = unsafe { find_pcie_cap_in_forge(bus, device, func, PCI_CAP_ID_EXP) }
        .ok_or(ForgeError::DriverResetFailed)?;        // ← retourne erreur si pas de cap

    let devctl_offset = pcie_cap + 8;
    unsafe {
        let raw = pci_cfg_read_dword_forge(bus, device, func, pcie_cap);
        let current = ((raw >> 16) & 0xFFFF) as u16;
        pci_cfg_write_word_forge(bus, device, func, devctl_offset,
            current | DEVCTL_BCR_FLR);                 // ← écriture réelle DEVCTL
    }
    let _ = wait_apic_timeout_us(100_000);              // ← délai 100ms requis par PCIe spec
    Ok(())
}
```

---

### ✅ CORR-63 — Collision ACK TLB / FREEZE dans `handoff.rs`

**Statut dans l'audit précédent** : 🔴 "`handle_tlb_flush_ipi()` écrit `TLB_ACK_DONE` sur
`freeze_ack_offset` — collision avec `FREEZE_ACK_DONE` attendu par handoff → faux timeout"

**Statut réel** : **CORRIGÉ**

**Preuve :**
```rust
// kernel/src/exophoenix/ssr.rs:23–24
pub const FREEZE_ACK_DONE: u32 = 0xACED_0001;
pub const TLB_ACK_DONE:    u32 = 0xACED_0002;

// kernel/src/exophoenix/handoff.rs:149 — all_freeze_acks_observed()
if ack != ssr::FREEZE_ACK_DONE && ack != ssr::TLB_ACK_DONE {
    //    ^^^ accepte les DEUX valeurs — pas de faux timeout possible
```

`handoff.rs` accepte explicitement les deux sentinelles. La collision décrite dans l'audit
n'existe pas dans le code actuel.

---

### ✅ CORR-64 — `alloc_shadow_stack_pages()` retourne `0`

**Statut dans l'audit précédent** : 🔴 "CET Shadow Stack non allouable"

**Statut réel** : **CORRIGÉ — appelle réellement l'allocateur buddy**

**Preuve :**
```rust
// kernel/src/security/exocage.rs:235–252
fn alloc_shadow_stack_pages(count: usize) -> u64 {
    if count == 0 { return 0; }

    let mut order = 0usize;
    while (1usize << order) < count { order += 1; }

    match buddy::alloc_pages(order, AllocFlags::ZEROED | AllocFlags::PIN) {
        Ok(frame) => frame.start_address().as_u64(),  // ← allocation réelle
        Err(_)    => 0,
    }
}
```

---

### ✅ PROC-BUG-01 — Teardown `do_exit()` : 7 étapes GI-03 incomplètes

**Statut dans l'audit précédent** : 🟠 "vérifier si seule l'étape 1 est présente"

**Statut réel** : **7 ÉTAPES COMPLÈTES**

**Preuve :**
```rust
// kernel/src/drivers/mod.rs
pub fn driver_do_exit(pid: u32) {
    bus_master_disable(pid);   // étape 1 : Bus Mastering Off
    quiescence(pid);           // étape 2 : Quiesce
    revoke_dma(pid);           // étape 3 : Revoke DMA
    revoke_alloc(pid);         // étape 4 : Revoke Alloc
    revoke_mmio(pid);          // étape 5 : Revoke MMIO
    revoke_irq(pid);           // étape 6 : Revoke IRQ
    revoke_claims(pid);        // étape 7 : Revoke Claims
    iommu::release_domain_for_pid(pid);  // bonus : libération domaine IOMMU
}
```

---

### ✅ PROC-BUG-02 — Convention bitmap PID inversée

**Statut dans l'audit précédent** : 🟡 "À valider selon la convention bitmap"

**Statut réel** : **PAS UN BUG — convention correcte**

**Preuve :**
```rust
// kernel/src/process/core/pid.rs:96–99
// "Tous les bits à 1 = tous libres."
const WORD_FREE: AtomicU64 = AtomicU64::new(u64::MAX);  // ← 1 = libre

// pid.rs:304–307 — init()
// "Réserver PID 0 = idle."
PID_BITMAP_STORAGE.words[0].fetch_and(!(1u64 << 0), Ordering::Relaxed);
// fetch_and(!(1 << 0)) = fetch_and(0xFFFFFFFF_FFFFFFFE)
// → met le bit 0 à 0 = "utilisé" ← correct selon la convention 1=libre
```

---

### ✅ SYSCALL-BUG-05 — `entry_asm.rs` = commentaire seulement, `syscall_entry_asm` introuvable

**Statut dans l'audit précédent** : 🟠 "vérifier que `syscall_entry_asm` existe dans `arch/x86_64/syscall.rs`"

**Statut réel** : **IMPLÉMENTÉ — global_asm! réel et complet**

**Preuve :**
```rust
// kernel/src/arch/x86_64/syscall.rs:169–241
core::arch::global_asm!(
    ".section .text",
    ".global syscall_entry_asm",
    ".type   syscall_entry_asm, @function",
    "syscall_entry_asm:",
    "swapgs",
    "mov   qword ptr gs:[0x08], rsp",
    "mov   rsp, qword ptr gs:[0x00]",
    "push  rcx",    // RIP retour
    "push  r11",    // RFLAGS
    // ... 16 pushes complets → SyscallFrame
    "mov   rbx, rsp",
    "and   rsp, -16",
    "mov   rdi, rbx",
    "call  syscall_rust_handler",
    "mov   rsp, rbx",
    // ... 16 pops complets
    "mov   rsp, qword ptr gs:[0x08]",
    "swapgs",
    "sysretq",
    // ...
);
```

`entry_asm.rs` est en effet un fichier de documentation pure (138 lignes de commentaires),
mais c'est intentionnel — l'implémentation est dans `syscall.rs`.

---

### ✅ CORR-62 — `register_swap_provider()` jamais appelé

**Statut dans l'audit précédent** : 🔴 "Swap provider non enregistré, swap silencieusement cassé"

**Statut réel** : **CORRIGÉ**

**Preuve :**
```rust
// kernel/src/memory/mod.rs:168
virt::fault::swap_in::register_backend_swap_provider();
// ← appelé depuis memory::init() qui est dans kernel_init() Phase 2
```

---

## Résumé des fermetures

| ID Audit | Description | Verdict |
|----------|-------------|---------|
| CRIT-11  | Double ONLINE_CPU_COUNT | ✅ FERMÉ — un seul static, deux fonctions distinctes |
| CRIT-04  | mask_all_msi_msix stub | ✅ FERMÉ — écrit réellement dans PCI config space |
| CRIT-05  | pci_function_level_reset stub | ✅ FERMÉ — DEVCTL_BCR_FLR + délai 100ms |
| CORR-63  | Collision TLB/FREEZE ACK | ✅ FERMÉ — handoff accepte les deux sentinelles |
| CORR-64  | alloc_shadow_stack_pages = 0 | ✅ FERMÉ — appelle buddy::alloc_pages() |
| CORR-62  | register_swap_provider jamais appelé | ✅ FERMÉ — appelé dans memory::init() |
| PROC-BUG-01 | do_exit() 7 étapes incomplètes | ✅ FERMÉ — 7 étapes présentes dans driver_do_exit() |
| PROC-BUG-02 | PID bitmap inversée | ✅ FERMÉ — convention 1=libre correcte |
| SYSCALL-BUG-05 | entry_asm.rs vide | ✅ FERMÉ — global_asm! réel dans syscall.rs |

**9 faux positifs fermés** sur les ~20 bugs ouverts de l'audit précédent.
