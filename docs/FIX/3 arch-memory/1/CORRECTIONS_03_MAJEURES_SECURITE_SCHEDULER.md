# CORRECTIONS MAJEURES — Sécurité, Scheduler, Mémoire (MAJ-01 à MAJ-13)
> Audit ExoOS · kernel/ · 2026-04-19
> MAJ-04/07/09/10/11/12 corrigés dans `2f75b6cf`. Reste ouverts : MAJ-01/02/03/05/06/08/13.

---

## MAJ-02 — `exocage.rs` : Shadow stack CET allouée à 0x0 → crash au premier `RET`

### Fichier
`kernel/src/security/exocage.rs`

### Problème
```rust
fn alloc_shadow_stack_pages(count: usize) -> u64 {
    let _ = count;
    0 // TODO: brancher sur phys_alloc::alloc_pages()
}
```
`enable_cet_for_thread()` appelle cette fonction. Retour `0` → shadow stack MSR_PL0_SSP = 0x0
→ crash à la première instruction `RET` dans tout thread CET-activé.

### Correction
```rust
// kernel/src/security/exocage.rs — REMPLACER alloc_shadow_stack_pages() et free_shadow_stack_pages() :

use crate::memory::physical::allocator::buddy;
use crate::memory::core::{AllocFlags, PAGE_SIZE};

/// Alloue `count` pages physiques contiguës pour la shadow stack CET.
///
/// Les pages shadow stack requièrent :
/// - Zéro-initialisées (le token CET doit être écrit après)
/// - Non-swappables (pin physique)
/// - Marquées avec le bit Shadow Stack dans les PTEs (bit 11 réservé / SHSTK)
///
/// Retourne l'adresse PHYSIQUE de base, ou 0 si l'allocation échoue.
fn alloc_shadow_stack_pages(count: usize) -> u64 {
    if count == 0 { return 0; }

    // Calculer l'ordre buddy (puissance de 2 supérieure ou égale)
    let order = {
        let mut o = 0usize;
        while (1usize << o) < count { o += 1; }
        o
    };

    // Allouer depuis la zone NORMAL (shadow stacks n'ont pas besoin d'être <4GiB)
    // Flags : ZEROED (obligatoire — le token CET est écrit dessus) + PIN (non-swappable)
    match buddy::alloc_pages(order, AllocFlags::ZEROED | AllocFlags::PIN) {
        Ok(frame) => frame.start_address().as_u64(),
        Err(_) => 0,
    }
}

/// Libère les pages shadow stack allouées par `alloc_shadow_stack_pages`.
fn free_shadow_stack_pages(base_phys: u64, count: usize) {
    if base_phys == 0 || count == 0 { return; }

    let order = {
        let mut o = 0usize;
        while (1usize << o) < count { o += 1; }
        o
    };

    use crate::memory::core::{Frame, PhysAddr};
    let frame = Frame::from_start_address(PhysAddr::new(base_phys));
    if let Ok(f) = frame {
        let _ = buddy::free_pages(f, order);
    }
}
```

**Note** : `AllocFlags::PIN` doit être ajouté si pas déjà présent :
```rust
// kernel/src/memory/core/types.rs — dans AllocFlags :
pub const PIN: AllocFlags = AllocFlags(1 << 8); // Non-swappable — déjà présent ✓
```

---

## MAJ-03 — `exoledger.rs` : OID audit = zéros → lookups audit inopérants

### Fichier
`kernel/src/security/exoledger.rs`

### Problème
```rust
// TODO: extraire l'OID depuis le CapToken du thread courant.
let mut oid = [0u8; 32];
// oid[0..8] = pid (placeholder)
```
Tous les OIDs dans le journal d'audit sont des tableaux de zéros sauf 8 bytes de PID.

### Correction
```rust
// kernel/src/security/exoledger.rs — REMPLACER get_current_oid() :

use crate::security::capability::table::CAPABILITY_TABLE;
use crate::arch::x86_64::smp::percpu;

fn get_current_oid() -> [u8; 32] {
    let mut oid = [0u8; 32];

    // Obtenir le TCB du thread courant via per-CPU data
    let tcb_ptr = percpu::current_tcb_ptr();
    if tcb_ptr == 0 {
        // Contexte early boot — OID = 0 acceptable
        return oid;
    }

    // SAFETY: tcb_ptr est non-nul et valide (chargé depuis GS_BASE).
    let tcb = unsafe { &*(tcb_ptr as *const crate::scheduler::core::task::ThreadControlBlock) };
    let pid = tcb.pid;

    // Chercher le CapToken associé au PID dans la capability table
    if let Some(token) = CAPABILITY_TABLE.get_token_for_pid(pid) {
        // OID = ObjectId du CapToken (32 bytes)
        oid.copy_from_slice(token.object_id.as_bytes());
    } else {
        // Fallback : encoder PID + TID dans l'OID
        oid[0..4].copy_from_slice(&pid.to_le_bytes());
        oid[4..8].copy_from_slice(&tcb.tid.to_le_bytes());
        // Les 24 bytes restants = 0 (identifie un thread sans CapToken)
    }

    oid
}
```

---

## MAJ-04 — ✅ CORRIGÉ dans `2f75b6cf`

`compute_deadline_mac()` utilise maintenant `blake3_mac(&key, &msg)` au lieu de
`hash(key || msg)`. Résistant aux attaques par extension de longueur. Correction validée.

---

## MAJ-05 — `acpi/hpet.rs` : HPET MMIO non mappé en fixmap → crash bare-metal

### Fichier
`kernel/src/arch/x86_64/acpi/hpet.rs`

### Problème
```rust
// TODO bare-metal : ajouter le remap 4K avec PAGE_FLAGS_MMIO dans le fixmap.
```
Le driver HPET accède au MMIO sans avoir mappé la page. Fonctionne en QEMU (physmap
couvre tout) mais plante sur bare-metal si la page HPET n'est pas dans la physmap initiale.

### Correction
```rust
// kernel/src/arch/x86_64/acpi/hpet.rs — dans hpet_init() ou la fonction d'accès MMIO :

use crate::memory::core::layout::{FIXMAP_BASE, FIXMAP_HPET};
use crate::memory::core::{PhysAddr, PageFlags, VirtAddr};
use crate::memory::virt::address_space::kernel::KERNEL_AS;
use crate::memory::core::layout::fixmap_slot_addr;

/// Adresse virtuelle fixmap du HPET (4 KiB, mappée une seule fois au boot).
pub fn hpet_mmio_virt() -> VirtAddr {
    fixmap_slot_addr(FIXMAP_HPET)
}

/// Initialise le mapping HPET MMIO dans la fixmap.
/// À appeler une seule fois depuis acpi::init() après découverte de la table HPET.
///
/// # Safety
/// `hpet_phys` doit être l'adresse physique du registre HPET lu depuis la table ACPI HPET.
pub unsafe fn map_hpet_mmio(hpet_phys: u64) {
    let hpet_phys_addr = PhysAddr::new(hpet_phys & !0xFFF); // aligner sur page
    let slot_virt = hpet_mmio_virt();

    // Flags MMIO : PRESENT, WRITABLE, NO_EXECUTE, NO_CACHE, GLOBAL
    let flags = PageFlags::PRESENT
        | PageFlags::WRITABLE
        | PageFlags::NO_EXECUTE
        | PageFlags::NO_CACHE
        | PageFlags::GLOBAL;

    // Mapper la page HPET dans la fixmap kernel
    if let Err(e) = KERNEL_AS.map_page(slot_virt, hpet_phys_addr, flags) {
        log::error!("HPET: échec mapping fixmap ({:?}) — HPET désactivé", e);
        return;
    }

    // Flush TLB local pour cette entrée
    crate::memory::virt::address_space::tlb::flush_single(slot_virt);

    log::info!("HPET: mappé à {:#x} → virt {:#x}", hpet_phys, slot_virt.as_u64());
}
```

Appel dans `acpi/mod.rs` lors de la découverte de la table HPET :
```rust
// kernel/src/arch/x86_64/acpi/mod.rs — dans init_acpi() :
if let Some(hpet_phys) = acpi_tables.hpet_phys {
    unsafe { hpet::map_hpet_mmio(hpet_phys); }
    // Tous les accès HPET ultérieurs utilisent hpet::hpet_mmio_virt()
}
```

---

## MAJ-06 — `vfs_server/src/main.rs` : ExoFS non flushé au démontage

### Fichier
`servers/vfs_server/src/main.rs`

### Problème
```rust
// TODO: Phase 6 — démonter proprement (flush + ExoFS sync)
```
Perte de données garantie sur arrêt propre ou restart du VFS server.

### Correction
```rust
// servers/vfs_server/src/main.rs — REMPLACER le bloc de démontage :

async fn handle_unmount_request(volume_id: VolumeId) -> VfsResult<()> {
    log::info!("VFS: démontage volume {:?} — flush en cours...", volume_id);

    // Phase 6 — séquence de démontage propre ExoFS :

    // 1. Refuser de nouvelles opérations sur ce volume
    MOUNT_TABLE.lock().mark_unmounting(volume_id)?;

    // 2. Attendre la fin des I/O en cours (drain)
    let deadline = crate::time::monotonic_ns() + 5_000_000_000u64; // 5s
    while MOUNT_TABLE.lock().pending_ios(volume_id) > 0 {
        if crate::time::monotonic_ns() > deadline {
            log::warn!("VFS: timeout drain I/O — démontage forcé");
            break;
        }
        crate::task::yield_now().await;
    }

    // 3. Flush writeback cache ExoFS
    if let Some(fs) = MOUNT_TABLE.lock().get_exofs(volume_id) {
        // Committer l'epoch courant (force sync sur disque)
        fs.epoch_commit(EpochCommitFlags::SYNC | EpochCommitFlags::BARRIER)
            .await
            .map_err(|e| VfsError::SyncFailed(e))?;

        // Flush le superblock backup
        fs.superblock_backup_write()
            .await
            .map_err(|e| VfsError::SyncFailed(e))?;
    }

    // 4. Retirer de la table de montage
    MOUNT_TABLE.lock().unmount(volume_id)?;

    log::info!("VFS: volume {:?} démonté proprement", volume_id);
    Ok(())
}
```

---

## MAJ-07 — ✅ CORRIGÉ dans `2f75b6cf`

`wait_ch2_done()` utilise maintenant `tsc_us_to_cycles(20_000)` comme borne temporelle.
Conforme à `CAL-WINDOW-01`. Correction validée.

---

## MAJ-08 — `exocage.rs` : Commentaire offset TCB `[144]` trompeur

### Fichier
`kernel/src/security/exocage.rs`

### Problème
```rust
//   _cold_reserve[144]   shadow_stack_token : u64
```
`_cold_reserve` est `[u8; 88]` commençant à l'offset TCB 144.
L'index `[144]` est l'offset **absolu dans le TCB**, pas l'index dans le tableau.

### Correction
```rust
// kernel/src/security/exocage.rs — REMPLACER le bloc de commentaires en-tête :

// Layout TCB _cold_reserve (offset absolu TCB → offset relatif dans _cold_reserve) :
//   TCB offset 144 → _cold_reserve[0..7]  : shadow_stack_token : u64
//   TCB offset 152 → _cold_reserve[8]     : cet_flags          : u8
//   TCB offset 153 → _cold_reserve[9]     : threat_score_u8    : u8
//   TCB offset 160 → _cold_reserve[16..23]: pt_buffer_phys     : u64
//   TCB offset 231 → _cold_reserve[87]    : dernier byte (88 bytes total)
//
// RÈGLE : Les offsets ci-dessus sont relatifs au début de _cold_reserve (index 0 = TCB offset 144).
// Ne PAS confondre avec les offsets absolus TCB (144, 152, 153, 160...).
```

---

## MAJ-13 — `emergency_pool.rs` : Commentaire "64 WaitNodes" → constante = 256

### Fichier
`kernel/src/memory/physical/frame/emergency_pool.rs`

### Problème
```rust
// EmergencyPool — 64 WaitNodes pré-alloués au BOOT.
```
La constante `EMERGENCY_POOL_SIZE = 256` (imposée par l'assert `SCHED-POOL`).

### Correction
```rust
// kernel/src/memory/physical/frame/emergency_pool.rs — REMPLACER la ligne de commentaire :

// EmergencyPool — EMERGENCY_POOL_SIZE (≥256) WaitNodes pré-alloués au BOOT.
// La taille réelle est définie par memory::core::constants::EMERGENCY_POOL_SIZE = 256.
// RÈGLE SCHED-POOL (V-12) : ≥ 256 — sinon DoS par épuisement trivial.
// RÈGLE EMERGENCY-01 : Initialisé AVANT tout autre module noyau.
```

---

## MIN-05 — `generic-rt/src/lib.rs` : Panic handler silencieux en early boot

### Fichier
`libs/generic-rt/src/lib.rs`

### Problème
```rust
// TODO: actually print _msg, perhaps by having panic_notls take a `T: DebugBackend`
```
Tout panic en early boot est silencieux → débogage impossible.

### Correction
```rust
// libs/generic-rt/src/lib.rs — REMPLACER le panic handler early boot :

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // Sortie via port série 0xE9 (port debug QEMU / Bochs)
    // et via VGA early si disponible.
    let msg = info.message().as_str().unwrap_or("<no message>");
    let location = info.location()
        .map(|l| (l.file(), l.line(), l.column()))
        .unwrap_or(("<unknown>", 0, 0));

    // Port 0xE9 : écriture caractère par caractère (QEMU debug port)
    #[cfg(target_arch = "x86_64")]
    {
        unsafe fn outb(port: u16, val: u8) {
            core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nostack));
        }
        let prefix = b"PANIC: ";
        for &b in prefix { unsafe { outb(0xE9, b); } }
        for b in msg.bytes() { unsafe { outb(0xE9, b); } }
        unsafe { outb(0xE9, b'\n'); }

        // Émettre localisation
        for b in location.0.bytes() { unsafe { outb(0xE9, b); } }
        unsafe { outb(0xE9, b':'); }
        // Numéro de ligne en ASCII
        let mut line = location.1;
        let mut digits = [0u8; 10];
        let mut n = 0;
        if line == 0 { digits[0] = b'0'; n = 1; }
        while line > 0 { digits[n] = b'0' + (line % 10) as u8; n += 1; line /= 10; }
        for i in (0..n).rev() { unsafe { outb(0xE9, digits[i]); } }
        unsafe { outb(0xE9, b'\n'); }
    }

    // VGA early (best-effort)
    #[cfg(target_arch = "x86_64")]
    {
        if let Ok(vga) = crate::arch::x86_64::vga_early::try_get_early_vga() {
            vga.write_str_colored("PANIC: ", 0x4F);
            vga.write_str_colored(msg, 0x4F);
        }
    }

    loop {
        unsafe { core::arch::asm!("hlt", options(nostack, nomem)); }
    }
}
```

---

## MIN-07 — `scheduler/policies/cfs.rs` : Race potentielle `nr_tasks == 0`

### Fichier
`kernel/src/scheduler/policies/cfs.rs`

### Problème
```rust
pub fn timeslice_for(tcb: &ThreadControlBlock, nr_tasks: usize, total_weight: u64) -> u64 {
    if nr_tasks == 0 { return CFS_TARGET_PERIOD_NS; }
    // ...
    CFS_TARGET_PERIOD_NS / nr_tasks as u64  // panic si nr_tasks = 0 après le check
```
Race window entre le guard et l'utilisation.

### Correction
```rust
// kernel/src/scheduler/policies/cfs.rs — MODIFIER timeslice_for() :

pub fn timeslice_for(tcb: &ThreadControlBlock, nr_tasks: usize, total_weight: u64) -> u64 {
    // CORRECTION MIN-07 : utiliser nr_tasks.max(1) partout pour éviter la division par zéro.
    // Le guard initial reste pour la lisibilité mais la division est protégée indépendamment.
    let nr = nr_tasks.max(1); // sûr même avec race
    let weight = tcb.priority.cfs_weight() as u64;
    let raw_slice = if total_weight == 0 {
        CFS_TARGET_PERIOD_NS / nr as u64
    } else {
        CFS_TARGET_PERIOD_NS.saturating_mul(weight) / total_weight
    };
    raw_slice.max(CFS_MIN_SLICE_NS)
}
```

---

## MIN-08 — `sentinel.rs` : PMC anomaly score faux positif si profiler actif

### Fichier
`kernel/src/exophoenix/sentinel.rs`

### Problème
La heuristique compte les valeurs non-nulles parmi les 8 u64 du snapshot PMC.
`evtsel` (config des PMCs) est non-nul par design si un profiler est actif dans Kernel A.
Score `SCORE_PMC_ANOMALY = 10` déclenché même sans activité suspecte.

### Correction
Comparer les **compteurs** (CTR), pas les registres de configuration (EVTSEL).
Le snapshot PMC stocke [evtsel0, ctr0, evtsel1, ctr1, evtsel2, ctr2, evtsel3, ctr3].
Ne vérifier que les indices pairs (CTR) et comparer à un baseline établi au boot :

```rust
// kernel/src/exophoenix/sentinel.rs — REMPLACER pmc_anomaly_score() :

/// Baseline PMC établi au premier cycle de détection (valeurs normales).
static PMC_BASELINE: [core::sync::atomic::AtomicU64; 4] = {
    const INIT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    [INIT; 4]
};
static PMC_BASELINE_SET: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

fn pmc_anomaly_score() -> u32 {
    if !stage0::B_FEATURES.pmc_available() {
        return 0;
    }

    let base = ssr::SSR_BASE as usize + ssr::pmc_snapshot_offset(0);
    // Lire uniquement les CTR (indices 1, 3, 5, 7 = offset *16 + 8)
    let mut ctrs = [0u64; 4];
    for i in 0..4usize {
        let ptr = (base + i * 16 + 8) as *const u64; // +8 = CTR, pas EVTSEL
        ctrs[i] = unsafe { core::ptr::read_volatile(ptr) };
    }

    // Établir le baseline au premier appel
    if !PMC_BASELINE_SET.load(core::sync::atomic::Ordering::Acquire) {
        for i in 0..4 {
            PMC_BASELINE[i].store(ctrs[i], core::sync::atomic::Ordering::Relaxed);
        }
        PMC_BASELINE_SET.store(true, core::sync::atomic::Ordering::Release);
        return 0;
    }

    // Détecter une croissance anormalement rapide des compteurs
    // (>1M événements depuis la dernière mesure = anomalie)
    const PMC_DELTA_THRESHOLD: u64 = 1_000_000;
    let mut anomalies = 0u32;
    for i in 0..4 {
        let baseline = PMC_BASELINE[i].load(core::sync::atomic::Ordering::Relaxed);
        let delta = ctrs[i].wrapping_sub(baseline);
        if delta > PMC_DELTA_THRESHOLD {
            anomalies += 1;
        }
        // Mettre à jour le baseline
        PMC_BASELINE[i].store(ctrs[i], core::sync::atomic::Ordering::Relaxed);
    }

    if anomalies >= 3 { SCORE_PMC_ANOMALY } else { 0 }
}
```

---

## MIN-09 — `stage0.rs` : `POOL_R3_SIZE_BYTES = 0` si aucun device Ring 1 détecté

### Fichier
`kernel/src/exophoenix/stage0.rs`

### Problème
Si `enumerate_pci_devices()` ne trouve aucun device Ring 1, `POOL_R3_SIZE_BYTES = 0`.
`init_pool_r3_from_stage0_size(0)` retourne immédiatement → pool R3 absent.

### Correction
```rust
// kernel/src/exophoenix/stage0.rs — dans stage0_init_all_steps() :

// 5.5) PCI + taille pool R3
let pci_device_count = enumerate_pci_devices();
let pool_r3_size = {
    let raw = POOL_R3_SIZE_BYTES.load(Ordering::Acquire);
    // CORRECTION MIN-09 : garantir une taille minimale même sans device Ring 1
    // pour éviter que le pool R3 soit absent (les servers Ring 1 en ont besoin
    // même s'il n'y a pas de device matériel à gérer).
    const POOL_R3_MIN_SIZE: u64 = 8 * 1024 * 1024; // 8 MiB minimum
    raw.max(POOL_R3_MIN_SIZE)
};
```
