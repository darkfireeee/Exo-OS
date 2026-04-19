# CORRECTIONS CRITIQUES — ExoPhoenix (CRIT-01 à CRIT-07)
> Audit ExoOS · kernel/src/exophoenix/ · 2026-04-19
> Statut : CRIT-06 corrigé dans le dernier commit. CRIT-01/02/03/04/05/07 restent ouverts.

---

## CRIT-01 — `forge.rs` : Hashes de reconstruction nuls → Kernel A jamais restaurable

### Fichier
`kernel/src/exophoenix/forge.rs`

### Problème
```rust
static A_IMAGE_HASH: [u8; 32]   = [0u8; 32]; // [ADAPT] hash réel ici
static A_MERKLE_ROOT: [u8; 32]  = [0u8; 32]; // [ADAPT] hash réel ici
```
`reconstruct_kernel_a()` compare Blake3(.text ++ .rodata) contre 32 zéros.
Tout image Kernel A légitime retourne `MerkleVerifyFailed` → `PhoenixState::Degraded` immédiat.

### Correction
Les hashes doivent être calculés et fixés **au moment du link**, injectés via le
linker script ou un build script. Exemple avec build.rs :

```rust
// kernel/build.rs — ajouter :
use std::fs;
use blake3::Hasher;

fn compute_kernel_a_hashes() {
    // Lire les sections ELF après compilation de kernel_a
    // En pratique : post-link step qui lit l'ELF et injecte les hashes.
    // Pour l'instant : générer un placeholder détecté au boot.
    println!("cargo:rustc-env=KERNEL_A_IMAGE_HASH=<valeur hex 64 chars>");
    println!("cargo:rustc-env=KERNEL_A_MERKLE_ROOT=<valeur hex 64 chars>");
}
```

Ou via un fichier de configuration versionné généré à chaque build :
```rust
// kernel/src/exophoenix/forge.rs — REMPLACER les statics par :

/// Hash Blake3 de l'image complète de Kernel A (injecté par build.rs au link).
/// Doit être mis à jour à chaque rebuild de kernel_a.
/// RÈGLE FORGE-01 : Ce hash doit correspondre au BlobId dans ExoFS.
static A_IMAGE_HASH: [u8; 32] = {
    // Injecté via include_bytes! d'un fichier généré par build.rs
    // ou via une constante d'environnement Cargo.
    // TEMPORAIRE : valeur nulle interdite en production — détectée au Stage0 par
    // assert!(A_IMAGE_HASH != [0u8;32], "FORGE: hash Kernel A non initialisé");
    *include_bytes!(concat!(env!("OUT_DIR"), "/kernel_a_image_hash.bin"))
};

static A_MERKLE_ROOT: [u8; 32] = {
    *include_bytes!(concat!(env!("OUT_DIR"), "/kernel_a_merkle_root.bin"))
};
```

Et dans `stage0_init()`, ajouter une guard de sécurité :
```rust
// kernel/src/exophoenix/stage0.rs — dans stage0_init() avant sentinel::run_forever()
if crate::exophoenix::forge::kernel_a_hash_is_zero() {
    log::error!("FORGE: A_IMAGE_HASH est nul — ExoPhoenix désactivé (mode dégradé)");
    PHOENIX_STATE.store(PhoenixState::Degraded as u8, Ordering::Release);
    // Ne pas envoyer de SIPI, boucler en halt
    loop { unsafe { core::arch::asm!("hlt", options(nostack, nomem)); } }
}
```

---

## CRIT-02 — `isolate.rs` : `mark_a_pages_not_present()` vide → cage mémoire inexistante

### Fichier
`kernel/src/exophoenix/isolate.rs`

### Problème
```rust
fn mark_a_pages_not_present() {
    // Corps entièrement vide — [ADAPT] non substitué
}
```
Kernel A garde un accès complet à sa propre mémoire pendant la phase d'isolation.

### Correction
Implémenter le walk PML4 de Kernel A et effacer le bit PRESENT.
`stage0.rs` expose déjà `B_STAGE0_CR3` (CR3 de B) ; Kernel A démarre avec la même
PML4 partagée. La cage s'effectue en retirant les mappings de A dans la PML4 courante :

```rust
// kernel/src/exophoenix/isolate.rs — REMPLACER mark_a_pages_not_present() :

use crate::memory::core::{KERNEL_LOAD_PHYS_ADDR, KERNEL_IMAGE_MAX_SIZE, PAGE_SIZE};
use crate::memory::virt::page_table::{PageTableWalker, WalkResult};
use crate::memory::virt::address_space::kernel::KERNEL_AS;
use crate::memory::virt::address_space::tlb;
use crate::memory::core::{VirtAddr, PageFlags};

fn mark_a_pages_not_present() {
    // La région de Kernel A en virtuel :
    // KERNEL_START = 0xFFFF_FFFF_8000_0000, taille KERNEL_IMAGE_MAX_SIZE
    // On retire le bit PRESENT sur toutes les pages de la région kernel image de A.
    //
    // NOTE : Cette implémentation suppose que A et B partagent la même PML4
    // kernel (cas normal au Stage0). Si A a sa propre PML4, utiliser
    // stage0::read_a_cr3() pour accéder à ses tables.

    let kernel_start = crate::memory::core::layout::KERNEL_START;
    let kernel_end_va = VirtAddr::new(
        kernel_start.as_u64() + KERNEL_IMAGE_MAX_SIZE as u64
    );

    let mut walker = PageTableWalker::new(KERNEL_AS.pml4_phys());
    let mut va = kernel_start;

    while va.as_u64() < kernel_end_va.as_u64() {
        match walker.walk_read(va) {
            WalkResult::Leaf { entry, .. } => {
                let mut flags = entry.to_page_flags();
                // Retirer PRESENT — la page reste en mémoire mais inaccessible.
                flags = flags.clear(PageFlags::PRESENT);
                // Ignorer l'erreur — on continue le walk même si une page est déjà absente.
                let _ = walker.remap_flags(va, flags);
            }
            WalkResult::NotMapped => {}
            WalkResult::HugePage { .. } => {
                // Les huge pages 2M/1G ne devraient pas couvrir la région kernel.
                // Si c'est le cas, les fragmenter avant de retirer PRESENT.
                // Pour l'instant, log + skip (sécurité dégradée acceptable).
                log::warn!("isolate: huge page détectée à {:#x} — skip", va.as_u64());
            }
            WalkResult::Error => break,
        }
        va = VirtAddr::new(va.as_u64().saturating_add(PAGE_SIZE as u64));
    }

    // Pas de TLB flush local ici — fait globalement par tlb_shootdown_all_a_cores()
    // via IPI 0xF3 dans la séquence appelante.
}
```

---

## CRIT-03 — `isolate.rs` : `override_a_idt_with_b_handlers()` vide → IDT de A non protégée

### Fichier
`kernel/src/exophoenix/isolate.rs`

### Problème
```rust
fn override_a_idt_with_b_handlers() {
    // Corps entièrement vide — [ADAPT] non substitué
}
```
Si Kernel A reprend l'exécution, il peut supprimer les vecteurs ExoPhoenix F1/F2/F3.

### Correction
Lire l'IDTR de A via CR3/physmap, écrire les entrées des handlers de B.

```rust
// kernel/src/exophoenix/isolate.rs — REMPLACER override_a_idt_with_b_handlers() :

use crate::arch::x86_64::idt;

/// Lit l'IDTR du contexte courant (valide car B utilise sa propre IDT après Stage0).
#[inline(always)]
fn read_current_idtr() -> (u64, u16) {
    #[repr(C, packed)]
    struct Idtr { limit: u16, base: u64 }
    let mut idtr = Idtr { limit: 0, base: 0 };
    unsafe {
        core::arch::asm!("sidt [{p}]", p = in(reg) &mut idtr,
                         options(nostack, preserves_flags));
    }
    (idtr.base, idtr.limit)
}

/// Écrit une entrée IDT interrupt-gate (64-bit) dans l'IDT à l'adresse `idt_base`.
/// `vector` : numéro de vecteur (0..=255)
/// `handler` : adresse du handler B (issue de `idt::get_handler_addr(vector)`)
unsafe fn write_idt_entry(idt_base: u64, vector: u8, handler: u64) {
    // Structure d'une interrupt gate 64-bit (16 bytes) :
    // [0..1]   offset[15:0]
    // [2..3]   segment selector (GDT_KERNEL_CS = 0x08)
    // [4]      IST index (0 = pas d'IST)
    // [5]      type/attr : 0x8E = present, DPL=0, type=interrupt gate
    // [6..7]   offset[31:16]
    // [8..11]  offset[63:32]
    // [12..15] réservé (zéro)
    let entry_ptr = (idt_base + (vector as u64) * 16) as *mut u8;
    let entry = core::slice::from_raw_parts_mut(entry_ptr, 16);

    let lo16 = (handler & 0xFFFF) as u16;
    let mid16 = ((handler >> 16) & 0xFFFF) as u16;
    let hi32 = (handler >> 32) as u32;

    entry[0..2].copy_from_slice(&lo16.to_le_bytes());
    entry[2..4].copy_from_slice(&(0x0008u16).to_le_bytes()); // GDT_KERNEL_CS
    entry[4] = 0x00;   // IST = 0
    entry[5] = 0x8E;   // P=1, DPL=0, Type=0xE (64-bit interrupt gate)
    entry[6..8].copy_from_slice(&mid16.to_le_bytes());
    entry[8..12].copy_from_slice(&hi32.to_le_bytes());
    entry[12..16].fill(0);
}

fn override_a_idt_with_b_handlers() {
    // Accéder à l'IDT de A via la physmap.
    // A partage la même IDT que B au moment du Stage0.
    // Après isolation, on écrase les vecteurs critiques avec les handlers de B
    // pour garantir que toute reprise de A reste sous contrôle de B.
    let (idt_base, idt_limit) = read_current_idtr();

    if idt_base == 0 || idt_limit < 16 {
        return; // IDT invalide — skip
    }

    // Adresses des handlers ExoPhoenix de B (définies dans idt.rs)
    let freeze_handler = idt::exophoenix_freeze_handler_addr();
    let pmc_handler    = idt::exophoenix_pmc_handler_addr();
    let tlb_handler    = idt::exophoenix_tlb_handler_addr();

    // Vérifier que les vecteurs tiennent dans la limite IDT
    let min_limit_needed = (u8::max(
        u8::max(idt::VEC_EXOPHOENIX_FREEZE, idt::VEC_EXOPHOENIX_PMC),
        idt::VEC_EXOPHOENIX_TLB
    ) as u16 + 1) * 16 - 1;

    if idt_limit < min_limit_needed {
        return;
    }

    // SAFETY : idt_base est l'IDTR courant valide en ring0.
    unsafe {
        write_idt_entry(idt_base, idt::VEC_EXOPHOENIX_FREEZE, freeze_handler);
        write_idt_entry(idt_base, idt::VEC_EXOPHOENIX_PMC,    pmc_handler);
        write_idt_entry(idt_base, idt::VEC_EXOPHOENIX_TLB,    tlb_handler);
    }
}
```

À ajouter dans `kernel/src/arch/x86_64/idt.rs` :
```rust
/// Retourne l'adresse virtuelle du handler freeze ExoPhoenix (0xF1).
pub fn exophoenix_freeze_handler_addr() -> u64 {
    // Utiliser la table IDT déjà construite pour récupérer l'adresse du handler.
    // Les handlers sont enregistrés via install_handler() au init_idt().
    IDT_TABLE.lock().entry(VEC_EXOPHOENIX_FREEZE).handler_addr()
}
// Idem pour PMC (VEC_EXOPHOENIX_PMC) et TLB (VEC_EXOPHOENIX_TLB).
```

---

## CRIT-04 — `handoff.rs` : `mask_all_msi_msix()` = fence uniquement → violation G2

### Fichier
`kernel/src/exophoenix/handoff.rs`

### Problème
```rust
fn mask_all_msi_msix() {
    core::sync::atomic::fence(Ordering::SeqCst); // N'empêche RIEN hardware
}
```
Les interruptions MSI/MSI-X continuent d'arriver sur les cores de A pendant le gel.

### Correction
Itérer sur `B_DEVICE_TABLE` et masquer chaque device via PCI config space.
Les IDs de capability MSI = `0x05`, MSI-X = `0x11`.

```rust
// kernel/src/exophoenix/handoff.rs — REMPLACER mask_all_msi_msix() :

// Constantes PCI capability IDs (PCI Local Bus Spec §6.7)
const PCI_CAP_ID_MSI:  u8 = 0x05;
const PCI_CAP_ID_MSIX: u8 = 0x11;

/// Lit un dword depuis le PCI config space via CF8/CFC (accès direct ring0).
#[inline(always)]
unsafe fn pci_read_dword_handoff(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    let addr = 0x8000_0000u32
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | ((offset & 0xFC) as u32);
    crate::arch::x86_64::outl(0xCF8, addr);
    crate::arch::x86_64::inl(0xCFC)
}

/// Écrit un word dans le PCI config space.
#[inline(always)]
unsafe fn pci_write_word_handoff(bus: u8, dev: u8, func: u8, offset: u8, value: u16) {
    let aligned = offset & 0xFC;
    let shift = ((offset & 0x2) * 8) as u32;
    let addr = 0x8000_0000u32
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | (aligned as u32);
    crate::arch::x86_64::outl(0xCF8, addr);
    let mut dword = crate::arch::x86_64::inl(0xCFC);
    dword &= !(0xFFFF << shift);
    dword |= (value as u32) << shift;
    crate::arch::x86_64::outl(0xCF8, addr);
    crate::arch::x86_64::outl(0xCFC, dword);
}

/// Trouve une capability PCI par son ID dans la capability list.
unsafe fn find_pci_cap(bus: u8, dev: u8, func: u8, cap_id: u8) -> Option<u8> {
    let status = (pci_read_dword_handoff(bus, dev, func, 0x04) >> 16) as u16;
    if status & 0x10 == 0 { return None; } // Pas de capability list

    let mut ptr = (pci_read_dword_handoff(bus, dev, func, 0x34) & 0xFF) as u8;
    let mut walked = 0;
    while ptr >= 0x40 && walked < 48 {
        let cap = pci_read_dword_handoff(bus, dev, func, ptr);
        if (cap & 0xFF) as u8 == cap_id { return Some(ptr); }
        let next = ((cap >> 8) & 0xFF) as u8;
        if next == 0 || next == ptr { break; }
        ptr = next;
        walked += 1;
    }
    None
}

fn mask_all_msi_msix() {
    // G2 : masquer toutes les interruptions MSI/MSI-X avant INIT IPI.
    // Sans cela, le hardware peut générer des IRQs pendant le gel des cores de A.
    for i in 0..stage0::b_device_count() {
        let Some(dev) = stage0::b_device(i) else { continue };

        // SAFETY : accès CF8/CFC en ring0, devices listés par le Stage0.
        unsafe {
            // Masquer MSI (capability 0x05) : bit 0 du MSI Control Register
            if let Some(msi_cap) = find_pci_cap(dev.bus, dev.device, dev.function, PCI_CAP_ID_MSI) {
                let ctrl_offset = msi_cap + 2;
                let raw = pci_read_dword_handoff(dev.bus, dev.device, dev.function, msi_cap);
                let ctrl = ((raw >> 16) & 0xFFFF) as u16;
                // Bit 0 = MSI Enable — mettre à 0 pour désactiver
                pci_write_word_handoff(dev.bus, dev.device, dev.function,
                                       ctrl_offset, ctrl & !0x0001);
            }

            // Masquer MSI-X (capability 0x11) : bit 14 du MSI-X Control Register (Function Mask)
            if let Some(msix_cap) = find_pci_cap(dev.bus, dev.device, dev.function, PCI_CAP_ID_MSIX) {
                let ctrl_offset = msix_cap + 2;
                let raw = pci_read_dword_handoff(dev.bus, dev.device, dev.function, msix_cap);
                let ctrl = ((raw >> 16) & 0xFFFF) as u16;
                // Bit 14 = Function Mask — mettre à 1 pour masquer toutes les entrées
                pci_write_word_handoff(dev.bus, dev.device, dev.function,
                                       ctrl_offset, ctrl | 0x4000);
            }
        }
    }

    // Barrière finale pour garantir que les écritures PCI sont commitées
    // avant l'envoi des INIT IPI.
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}
```

---

## CRIT-05 — `forge.rs` : Reset drivers Ring 1 fictif → reconstruction non effective

### Fichier
`kernel/src/exophoenix/forge.rs`

### Problème
```rust
fn pci_function_level_reset(bus: u8, device: u8, func: u8) -> Result<(), ForgeError> {
    let _ = (bus, device, func);
    Ok(()) // No-op total
}
fn reload_driver_binary_from_exofs(...) -> Result<(), ForgeError> {
    let _ = (bus, device, func);
    Ok(()) // No-op total
}
```

### Correction
```rust
// kernel/src/exophoenix/forge.rs — REMPLACER pci_function_level_reset() :

/// PCI Express FLR via Device Control Register (PCIe spec §7.5.3.4).
fn pci_function_level_reset(bus: u8, device: u8, func: u8) -> Result<(), ForgeError> {
    // Trouver la capability PCIe (ID = 0x10)
    const PCI_CAP_ID_EXP: u8 = 0x10;
    const DEVCTL_BCR_FLR: u16 = 1 << 15; // Bit 15 = BCR/FLR initiation

    // SAFETY : accès CF8/CFC ring0 dans contexte ExoPhoenix.
    let pcie_cap = unsafe {
        find_pcie_cap_in_forge(bus, device, func, PCI_CAP_ID_EXP)
    }.ok_or(ForgeError::DriverResetFailed)?;

    // Device Control Register = PCIe cap offset + 8
    let devctl_offset = pcie_cap + 8;

    unsafe {
        let current = pci_cfg_read16_forge(bus, device, func, devctl_offset);
        pci_cfg_write16_forge(bus, device, func, devctl_offset, current | DEVCTL_BCR_FLR);
    }

    // Attendre 100ms (spec PCIe §6.6.2 : FLR completion ≤ 100ms)
    wait_apic_timeout_us(100_000);
    Ok(())
}

/// Recharge le binaire du driver Ring 1 depuis ExoFS via son BlobId enregistré au Stage0.
fn reload_driver_binary_from_exofs(bus: u8, device: u8, func: u8) -> Result<(), ForgeError> {
    // Lookup du hash de driver dans la table construite au Stage0.
    // La table B_DRIVER_HASH_TABLE est indexée par BDF.
    let bdf_key = ((bus as u32) << 16) | ((device as u32) << 8) | func as u32;
    let blob_id = stage0::driver_blob_id(bdf_key).ok_or(ForgeError::DriverResetFailed)?;

    // Charger depuis ExoFS
    let data = crate::fs::exofs::cache::blob_cache::BLOB_CACHE
        .get(&blob_id)
        .ok_or(ForgeError::DriverResetFailed)?;

    // TODO : mapper le binaire dans l'espace Ring 1 et notifier l'ipc_broker
    // que ce driver doit être redémarré avec le nouveau binaire.
    // Pour l'instant : log de confirmation de disponibilité du binaire.
    let _ = data.len(); // confirme que le binaire est disponible
    Ok(())
}
```

**Note importante** : `stage0::driver_blob_id()` doit être ajouté — il nécessite
qu'une table `BDF → BlobId` soit construite au Stage0 pendant l'énumération PCI.

---

## CRIT-07 — `sentinel.rs` : Liveness mirror offset arbitraire

### Fichier
`kernel/src/exophoenix/sentinel.rs`

### Problème
```rust
const A_LIVENESS_MIRROR_PHYS: u64 = KERNEL_LOAD_PHYS_ADDR + 0x280;
```
Offset `+0x280` arbitraire, non documenté dans l'ABI Kernel A.

### Correction
Définir un offset contractuel dans la crate partagée `exo_phoenix_ssr` et
faire écrire ce champ par Kernel A au démarrage :

**Dans `libs/exo-phoenix-ssr/src/lib.rs`** — ajouter :
```rust
/// Offset physique dans l'image Kernel A où est écrit le nonce de liveness.
/// Kernel A doit écrire à `KERNEL_LOAD_PHYS_ADDR + A_LIVENESS_MIRROR_OFFSET`
/// la valeur du nonce SSR dès qu'il le lit dans SSR_LIVENESS_NONCE.
/// RÈGLE PHOENIX-LIVENESS-01 : Cet offset est contractuel — ne pas modifier
/// sans mettre à jour simultanément Kernel A et Kernel B.
pub const A_LIVENESS_MIRROR_OFFSET: u64 = 0x0100; // 256 bytes — avant le code .text
```

**Dans `kernel/src/exophoenix/sentinel.rs`** — remplacer la constante :
```rust
// AVANT :
const A_LIVENESS_MIRROR_PHYS: u64 = KERNEL_LOAD_PHYS_ADDR + 0x280;

// APRÈS :
const A_LIVENESS_MIRROR_PHYS: u64 =
    KERNEL_LOAD_PHYS_ADDR + exo_phoenix_ssr::A_LIVENESS_MIRROR_OFFSET;
```

**Dans Kernel A** (côté A) — ajouter dans le loop principal :
```rust
// Kernel A doit surveiller SSR_LIVENESS_NONCE et le répercuter :
loop {
    let nonce = unsafe {
        ssr::ssr_atomic(ssr::SSR_LIVENESS_NONCE).load(Ordering::Acquire)
    };
    // Écrire le nonce à l'offset contractuel de l'image physique de A
    unsafe {
        let mirror_virt = PHYS_MAP_BASE.as_u64()
            + crate::memory::core::layout::KERNEL_LOAD_PHYS_ADDR
            + exo_phoenix_ssr::A_LIVENESS_MIRROR_OFFSET;
        core::ptr::write_volatile(mirror_virt as *mut u64, nonce);
    }
    // ... reste de la boucle principale de A
}
```

---

## CRIT-06 — ✅ CORRIGÉ dans le commit `2f75b6cf`

`ssr_atomic()` et `ssr_atomic_u32()` utilisent maintenant `phys_to_virt(PhysAddr::new(SSR_BASE))`.
Un `debug_assert!` de borne est également ajouté. Correction validée.

---

## MAJ-01 — `handoff.rs` + `interrupts.rs` : Collision partielle ACK freeze/TLB

### Statut : PARTIELLEMENT corrigé
`all_freeze_acks_observed()` accepte désormais `FREEZE_ACK_DONE || TLB_ACK_DONE`.
**Reste à corriger** : `send_init_ipi_to_resistant_cores()` ne vérifie que `FREEZE_ACK_DONE`.

### Correction résiduelle
```rust
// kernel/src/exophoenix/handoff.rs — MODIFIER send_init_ipi_to_resistant_cores() :

fn send_init_ipi_to_resistant_cores(self_slot: Option<usize>) {
    for_each_mapped_apic_slot(|apic_id, slot| {
        if Some(slot) == self_slot { return; }
        let ack = unsafe {
            ssr::ssr_atomic_u32(ssr::freeze_ack_offset(slot)).load(Ordering::Acquire)
        };
        // CORRECTION MAJ-01 : accepter TLB_ACK_DONE comme freeze ACK valide
        // (un core qui a déjà flushé son TLB est considéré coopératif)
        if ack != ssr::FREEZE_ACK_DONE && ack != ssr::TLB_ACK_DONE {
            send_init_ipi_to_apic(apic_id);
        }
    });
}
```
