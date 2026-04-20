# ExoOS — Corrections P1 Majeures
## Commit de référence : `c4239ed1`

Ces cinq corrections éliminent des comportements incorrects
qui apparaîtront en production dès que les bloquants P0 seront levés.

---

## P1-01 — Fuite PML4 CoW dans `do_fork()` sur chemins d'erreur tardifs

### Localisation
`kernel/src/process/lifecycle/fork.rs` — fonction `do_fork()`

### Symptôme
À chaque collision de registry (PROCESS_REGISTRY pleine) ou CPU cible invalide,
le PML4 cloné par `AddressSpaceCloner::clone_cow()` est alloué mais jamais libéré.
Sur un système à forte charge de fork, cela provoque une fuite progressive de pages
physiques (1 page PML4 × nombre d'échecs).

### Analyse du code actuel

```rust
// fork.rs — chemin RegistryError (ligne ~290)
PROCESS_REGISTRY.insert(child_pcb).map_err(|_| {
    unsafe { drop(Box::from_raw(child_thread_ptr)); }
    // ← MANQUE : free cloned_as.addr_space_ptr
    PID_ALLOCATOR.free(child_pid_raw);
    TID_ALLOCATOR.free(child_tid_raw);
    ForkError::RegistryError
})?;

// fork.rs — chemin InvalidCpu (ligne ~300)
if ctx.target_cpu as usize >= MAX_CPUS {
    let _ = PROCESS_REGISTRY.remove(child_pid);
    unsafe { drop(Box::from_raw(child_thread_ptr)); }
    // ← MANQUE : free cloned_as.addr_space_ptr
    PID_ALLOCATOR.free(child_pid_raw);
    TID_ALLOCATOR.free(child_tid_raw);
    return Err(ForkError::InvalidCpu);
}
```

### Correction complète

**Étape A — Ajouter `free_addr_space` au trait `AddressSpaceCloner`** (décrite dans P0-01 étape B)

**Étape B — Appeler `free_addr_space` dans les deux chemins d'erreur**

```rust
// fork.rs — helper interne pour éviter la répétition

/// Libère les ressources partiellement allouées lors d'un échec de fork.
///
/// Doit être appelé depuis tout chemin d'erreur APRÈS clone_cow() et AVANT
/// le retour de do_fork(). Garantit qu'aucune page physique n'est perdue.
#[cold]
fn cleanup_failed_fork(
    child_thread_ptr:  *mut ProcessThread,
    cloned_addr_space: usize,
    child_pid_raw:     u32,
    child_tid_raw:     u32,
) {
    // 1. Libérer le TCB fils alloué sur le heap.
    // SAFETY: child_thread_ptr créé via Box::into_raw(), pas encore enfilé.
    unsafe { drop(Box::from_raw(child_thread_ptr)); }

    // 2. Libérer l'espace d'adressage cloné (PML4 + tables filles + refcounts).
    if let Some(cloner) = ADDR_SPACE_CLONER.get() {
        cloner.free_addr_space(cloned_addr_space);
    }

    // 3. Rendre PID et TID disponibles.
    PID_ALLOCATOR.free(child_pid_raw);
    TID_ALLOCATOR.free(child_tid_raw);
}

// fork.rs — remplacer les deux chemins d'erreur tardifs

// Chemin RegistryError :
PROCESS_REGISTRY.insert(child_pcb).map_err(|_| {
    cleanup_failed_fork(
        child_thread_ptr,
        cloned_as.addr_space_ptr,
        child_pid_raw,
        child_tid_raw,
    );
    ForkError::RegistryError
})?;

// Chemin InvalidCpu :
if ctx.target_cpu as usize >= MAX_CPUS {
    let _ = PROCESS_REGISTRY.remove(child_pid);
    cleanup_failed_fork(
        child_thread_ptr,
        cloned_as.addr_space_ptr,
        child_pid_raw,
        child_tid_raw,
    );
    return Err(ForkError::InvalidCpu);
}
```

---

## P1-02 — SHM : `virt_addr = phys_addr` stub non fonctionnel pour userspace

### Localisation
`kernel/src/ipc/shared_memory/mapping.rs:245–257`

### Code actuel

```rust
// mapping.rs:245–257
let phys = {
    region.descriptor.page_phys(0)
        .and_then(|d| d.page_phys(0))
        .unwrap_or(PhysAddr(0))
};
// Adresse virtuelle = adresse physique dans l'implémentation stub
// (sera remplacé par memory::virtual::find_vma() lors de l'intégration)
VirtAddr(phys.0)
```

### Symptôme
Un processus Ring1 qui appelle `SHM_MAP` reçoit une adresse virtuelle
égale à l'adresse physique brute (ex. `0x200000`).
Le premier déréférencement provoque un `#PF` (la page physique n'est pas mappée
dans l'espace virtuel du processus) → crash garanti.

### Correction

```rust
// kernel/src/ipc/shared_memory/mapping.rs — remplacer la fonction map_shm_into_process

/// Mappe la région SHM dans l'espace d'adressage virtuel du processus cible.
///
/// Utilise le VMA allocator du processus pour trouver une plage libre,
/// puis mappe chaque page physique à l'adresse virtuelle allouée.
///
/// # Arguments
/// * `region`  — descripteur de région SHM (pages physiques allouées)
/// * `pid`     — PID du processus cible
/// * `hint_virt` — adresse virtuelle suggérée (0 = choix automatique)
///
/// # Retour
/// Adresse virtuelle de base dans l'espace du processus, ou 0 en cas d'échec.
pub fn map_shm_into_process(
    region:     &ShmRegion,
    pid:        ProcessId,
    hint_virt:  VirtAddr,
) -> VirtAddr {
    use crate::process::core::registry::PROCESS_REGISTRY;
    use crate::memory::virtual::mmap::{mmap_anonymous_in_space, MapFlags};
    use crate::memory::core::PAGE_SIZE;

    let page_count = region.descriptor.page_count();
    if page_count == 0 { return VirtAddr(0); }

    let size_bytes = page_count * PAGE_SIZE;

    // 1. Retrouver le CR3 du processus cible via la registry.
    let pcb = match PROCESS_REGISTRY.find_by_pid(
        crate::process::core::pid::Pid(pid.0)
    ) {
        Some(p) => p,
        None    => return VirtAddr(0),
    };

    let target_cr3 = pcb.cr3.load(core::sync::atomic::Ordering::Acquire);
    if target_cr3 == 0 { return VirtAddr(0); }

    // 2. Trouver une plage virtuelle libre dans l'espace du processus cible.
    //    mmap_find_free_vma cherche dans les VMAs existantes du CR3 cible.
    let virt_base = match crate::memory::virtual::mmap::find_free_vma(
        target_cr3,
        size_bytes,
        hint_virt.0,
    ) {
        Some(addr) => addr,
        None       => return VirtAddr(0),
    };

    // 3. Mapper chaque page physique à l'adresse virtuelle trouvée.
    for i in 0..page_count {
        let phys = match region.descriptor.page_phys(i) {
            Some(p) => p,
            None    => return VirtAddr(0), // incohérence interne
        };

        let virt_page = virt_base + (i * PAGE_SIZE) as u64;

        // SAFETY: target_cr3 valide, virt_page dans la plage libre vérifiée ci-dessus.
        let ok = unsafe {
            crate::memory::virtual::page_table::x86_64::map_4k_page(
                target_cr3,
                phys.as_u64(),
                virt_page,
                crate::memory::core::PageFlags::USER_RW,
            )
        };

        if ok.is_err() {
            // Rollback des pages déjà mappées
            for j in 0..i {
                let vp = virt_base + (j * PAGE_SIZE) as u64;
                unsafe {
                    crate::memory::virtual::page_table::x86_64::unmap_4k_page(
                        target_cr3, vp
                    );
                }
            }
            return VirtAddr(0);
        }
    }

    // 4. TLB shootdown dans le processus cible si c'est le processus courant.
    // (Si c'est un autre processus, le TLB sera invalidé à son prochain context switch.)
    let current_cr3: u64;
    unsafe { core::arch::asm!("mov {}, cr3", out(reg) current_cr3); }
    if current_cr3 == target_cr3 {
        unsafe { crate::arch::x86_64::paging::flush_tlb_range(virt_base, size_bytes); }
    }

    VirtAddr(virt_base)
}
```

---

## P1-03 — Fixup ASM `#PF` absent dans les handlers syscall

### Localisation
`kernel/src/syscall/validation.rs:440,452`

### Code actuel

```rust
// validation.rs:440
/// - Un fixup ASM capture le page fault et retourne EFAULT
// validation.rs:452
// Un page fault ici serait normalement capturé par exception_fixup_table.
```

Ces commentaires décrivent un mécanisme non implémenté.

### Symptôme
Un handler syscall qui déréférence un pointeur userspace valide en adresse
mais dont la page est swappée ou CoW-protégée provoque un `#PF` en
contexte kernel → `kernel_panic()`.
Exemple : `sys_read(fd, ptr_to_cow_page, 4096)` → panic au lieu de EFAULT.

### Correction

**Étape A — Ajouter une table de fixup dans `arch/x86_64/exceptions.rs`**

```rust
// kernel/src/arch/x86_64/exceptions.rs — ajouter en bas du fichier

/// Entrée de la table de fixup d'exception.
///
/// Si une instruction à `fault_rip` provoque une exception #PF,
/// le handler redirige RIP vers `recovery_rip` et charge RAX avec `error_value`.
#[repr(C)]
pub struct FixupEntry {
    /// RIP de l'instruction fautive (adresse absolue dans .text).
    pub fault_rip:    u64,
    /// RIP de récupération (instruction suivante ou gestionnaire d'erreur).
    pub recovery_rip: u64,
    /// Valeur chargée dans RAX au retour (typiquement -EFAULT = -14).
    pub error_value:  i64,
}

/// Table de fixup — liens statiques via la section `.fixup`.
///
/// Remplie à la compilation par la macro `fixup_entry!`.
/// Le handler `#PF` la parcourt en O(N) (N petit — quelques dizaines d'entrées).
#[link_section = ".fixup"]
static FIXUP_TABLE: [FixupEntry; 0] = []; // étendu par les sites d'appel

extern "C" {
    static __start_fixup: FixupEntry;
    static __stop_fixup:  FixupEntry;
}

/// Recherche une entrée de fixup pour un RIP donné.
///
/// Retourne `Some((recovery_rip, error_value))` si une entrée correspond,
/// `None` sinon (→ le #PF est une vraie faute kernel → panic).
pub fn find_fixup(fault_rip: u64) -> Option<(u64, i64)> {
    // SAFETY: section .fixup lue en lecture seule, toujours mappée.
    let (start, stop) = unsafe {
        (
            &__start_fixup as *const FixupEntry,
            &__stop_fixup  as *const FixupEntry,
        )
    };
    let count = (stop as usize - start as usize) / core::mem::size_of::<FixupEntry>();
    for i in 0..count {
        // SAFETY: i < count, pointeur dans la section.
        let entry = unsafe { &*start.add(i) };
        if entry.fault_rip == fault_rip {
            return Some((entry.recovery_rip, entry.error_value));
        }
    }
    None
}
```

**Étape B — Modifier le handler `#PF` pour consulter la table**

```rust
// kernel/src/arch/x86_64/exceptions.rs — dans handle_page_fault()

pub extern "C" fn handle_page_fault(frame: &mut ExceptionFrame, error_code: u64) {
    let faulting_addr: u64;
    unsafe { core::arch::asm!("mov {}, cr2", out(reg) faulting_addr); }

    // 1. Vérifier si la faute vient d'un accès userspace depuis le kernel
    //    (bit U/S=0 dans error_code = faute superviseur, addr < USER_ADDR_MAX).
    let is_kernel_access_to_user = (error_code & 0x4) == 0
        && faulting_addr < crate::syscall::validation::USER_ADDR_MAX;

    if is_kernel_access_to_user {
        // 2. Chercher un fixup pour le RIP fautif.
        if let Some((recovery_rip, error_value)) = find_fixup(frame.rip) {
            // Rediriger vers le site de récupération.
            frame.rip = recovery_rip;
            frame.rax = error_value as u64;
            return; // retour sans panic
        }
    }

    // 3. Faute normale : tenter CoW break ou demand paging.
    if let Err(_) = crate::memory::virtual::fault::handler::handle_fault(
        faulting_addr, error_code
    ) {
        kernel_panic_with_fault(frame, faulting_addr, error_code);
    }
}
```

**Étape C — Macro `fixup_entry!` pour marquer les sites d'accès userspace**

```rust
// kernel/src/arch/x86_64/exceptions.rs — ajouter la macro

/// Marque le site d'accès courant comme récupérable sur #PF.
///
/// Usage :
/// ```
/// let val: u8;
/// fixup_entry!(
///     // Code à protéger
///     unsafe { val = *(user_ptr as *const u8); },
///     // Code de récupération si #PF
///     return Err(SyscallError::Fault)
/// );
/// ```
#[macro_export]
macro_rules! fixup_entry {
    ($access:expr, $recovery:expr) => {{
        // Générer une entrée dans .fixup avec les deux labels ASM.
        // NOTE : en Rust stable sans asm_sym, on utilise une approche
        // function-level avec une probe explicite :
        let _probe = crate::syscall::validation::probe_user_read($access as u64, 1);
        if _probe.is_err() { $recovery }
        else { $access }
    }};
}
```

**Étape D — Implémenter `probe_user_read` dans `validation.rs`**

```rust
// kernel/src/syscall/validation.rs — ajouter

/// Sonde un accès lecture userspace : retourne Ok si la page est présente,
/// Err(Fault) si elle est absente/swappée.
///
/// N'utilise PAS de fixup ASM (trop complexe sans asm_sym) mais force
/// le demand-paging de la page AVANT l'accès réel du handler.
/// Après probe réussie, l'accès suivant est garanti sans #PF.
#[inline]
pub fn probe_user_read(addr: u64, len: usize) -> Result<(), SyscallError> {
    if addr == 0 || addr >= USER_ADDR_MAX { return Err(SyscallError::Fault); }
    if len == 0 { return Ok(()); }

    // Vérifier que chaque page de la plage est présente en marchant le PML4.
    let page_start = addr & !0xFFF;
    let page_end   = (addr + len as u64 + 0xFFF) & !0xFFF;

    let cr3: u64;
    unsafe { core::arch::asm!("mov {}, cr3", out(reg) cr3); }

    let mut virt = page_start;
    while virt < page_end {
        let present = unsafe {
            crate::memory::virtual::page_table::x86_64::is_page_present(cr3, virt)
        };
        if !present {
            // Déclencher le demand paging / swap-in maintenant.
            match crate::memory::virtual::fault::handler::handle_fault(virt, 0x4) {
                Ok(())  => {},
                Err(_)  => return Err(SyscallError::Fault),
            }
        }
        virt += 0x1000;
    }
    Ok(())
}
```

---

## P1-04 — `MAX_SPSC_RINGS = 256` vs `MAX_CHANNELS = 65536`

### Localisation
- `kernel/src/ipc/ring/spsc.rs:276` : `const MAX_SPSC_RINGS: usize = 256`
- `kernel/src/ipc/core/constants.rs:92` : `pub const MAX_CHANNELS: usize = 65_536`
- `kernel/src/ipc/ring/spsc.rs:297–302` : `ring_for()` rejette `channel_id >= 256`

### Symptôme
Tout canal IPC avec `channel_id >= 256` retourne `IpcError::InvalidParam`.
Le système est limité à 256 canaux simultanés au lieu des 65 536 prévus.
Les services qui allouent dynamiquement des endpoints dépasseront cette limite
dès que le nombre de serveurs + clients dépasse 256.

### Analyse
`ring_for()` fait un bounds-check `if raw_idx >= MAX_SPSC_RINGS` correct,
mais `MAX_SPSC_RINGS` est arbitrairement petit.
Il faut aligner `MAX_SPSC_RINGS` sur `MAX_CHANNELS` ou segmenter la table.

### Correction

```rust
// kernel/src/ipc/ring/spsc.rs — remplacer la constante et la table

// Aligner sur MAX_CHANNELS pour cohérence.
// 65536 rings × ~200 bytes/ring ≈ 13 MiB .bss — acceptable pour un kernel.
// Si la mémoire est contrainte : utiliser une allocation dynamique (Vec ou Box<[]>)
// initialisée dans ipc_init() à partir de la pool SHM déjà allouée.
//
// Option 1 (simple, statique) :
use crate::ipc::core::constants::MAX_CHANNELS;
const MAX_SPSC_RINGS: usize = MAX_CHANNELS; // 65536

// Option 2 (économique, dynamique) — recommandée pour réduire l'empreinte .bss :
// Garder MAX_SPSC_RINGS = 256 mais ajouter un second niveau d'indirection :
// table[256] de blocs de 256 rings alloués à la demande.
// Cette option est plus complexe — choisir Option 1 en Phase 1.

// NOTE : vérifier que RING_SIZE est une puissance de 2 (requis pour RING_MASK).
// RING_SIZE = 256 (actuel), RING_MASK = 255 ✓ — pas de changement nécessaire.
```

```rust
// Alternative si 13 MiB statique est trop gros :
// kernel/src/ipc/ring/spsc.rs

// Garder MAX_SPSC_RINGS = 4096 comme compromis (800 KiB, 16× plus que 256)
// et documenter la limite dans constants.rs.
const MAX_SPSC_RINGS: usize = 4096;

// Mettre à jour constants.rs pour cohérence :
// pub const MAX_CHANNELS: usize = MAX_SPSC_RINGS; // 4096
// Ou documenter explicitement que MAX_CHANNELS est aspirationnel :
// pub const MAX_CHANNELS: usize = 65_536; // limite logique — ring table = 4096 actuel
```

**Choix recommandé** : `MAX_SPSC_RINGS = 4096` avec commentaire alignant les deux constantes.

```rust
// kernel/src/ipc/core/constants.rs — ajouter une assertion de cohérence

/// Nombre maximum de rings SPSC simultanément actifs.
/// DOIT être ≤ MAX_CHANNELS.
pub const MAX_ACTIVE_RINGS: usize = 4096;

// Assertion compile-time :
const _: () = assert!(
    MAX_ACTIVE_RINGS <= MAX_CHANNELS,
    "MAX_ACTIVE_RINGS doit être <= MAX_CHANNELS"
);
```

---

## P1-05 — ExoPhoenix `send_sipi_once` : SIPIs sans INIT IPI préalable

### Localisation
`kernel/src/exophoenix/stage0.rs:1161–1170`

### Code actuel

```rust
pub fn send_sipi_once(core_slot: u8, entry_vector: u8) -> Result<(), SendSipiError> {
    let prior = SIPI_SENT.fetch_or(SIPI_SENT_BIT, Ordering::AcqRel);
    if (prior & SIPI_SENT_BIT) != 0 {
        return Err(SendSipiError::AlreadySent);
    }

    let apic_id = resolve_apic_id_for_slot(core_slot).ok_or(SendSipiError::TargetNotFound)?;
    ipi::send_startup_ipi(apic_id, entry_vector);   // SIPI #1
    tsc::tsc_delay_ms(1);
    ipi::send_startup_ipi(apic_id, entry_vector);   // SIPI #2
    Ok(())
}
```

### Problème
La spec Intel MP §B.4 impose la séquence :
`INIT → 10ms → SIPI → 200µs → SIPI`

Le SMP boot (`smp/init.rs:boot_ap`) est correct. Mais `send_sipi_once()` (chemin ExoPhoenix Kernel-B)
envoie deux SIPIs sans INIT préalable. Sur certains chipsets bare-metal, un CPU en état
`WAIT-FOR-SIPI` non précédé d'un INIT peut ignorer le SIPI.

### Correction

```rust
// kernel/src/exophoenix/stage0.rs — remplacer send_sipi_once

/// Séquence de démarrage ExoPhoenix pour Kernel B.
///
/// Conforme Intel MP Spec §B.4 :
///   INIT → 10ms → SIPI → 200µs → SIPI
///
/// Le garde-fou AtomicU64 empêche l'envoi multiple (G8).
pub fn send_sipi_once(core_slot: u8, entry_vector: u8) -> Result<(), SendSipiError> {
    let prior = SIPI_SENT.fetch_or(SIPI_SENT_BIT, Ordering::AcqRel);
    if (prior & SIPI_SENT_BIT) != 0 {
        return Err(SendSipiError::AlreadySent);
    }

    let apic_id = resolve_apic_id_for_slot(core_slot)
        .ok_or(SendSipiError::TargetNotFound)?;

    // CORRECTION P1-05 : INIT IPI avant les SIPIs (spec Intel MP §B.4)
    //
    // Étape 1 : INIT IPI — remet le CPU cible en état RESET/WAIT-FOR-SIPI.
    ipi::send_init_ipi(apic_id);

    // Étape 2 : Délai 10ms — le CPU cible complète son INIT (requis par spec).
    tsc::tsc_delay_ms(10);

    // Étape 3 : Premier SIPI — démarre l'exécution depuis entry_vector × 0x1000.
    ipi::send_startup_ipi(apic_id, entry_vector);

    // Étape 4 : Délai 200µs — certains chipsets ont besoin de ce délai minimum.
    // tsc_delay_ms(1) ≈ 1ms > 200µs — conservateur, correct.
    tsc::tsc_delay_ms(1);

    // Étape 5 : Deuxième SIPI — redondance pour chipsets lents (G8 one-shot
    // est maintenu par le garde-fou : le CPU n'exécute que le premier SIPI reçu
    // si déjà en mode protégé après le premier).
    ipi::send_startup_ipi(apic_id, entry_vector);

    Ok(())
}
```

> **Note** : `ipi::send_init_ipi()` est déjà implémentée dans `arch/x86_64/apic/ipi.rs`
> et utilisée par `smp/init.rs:boot_ap()`. Pas de nouveau code à écrire côté APIC.
