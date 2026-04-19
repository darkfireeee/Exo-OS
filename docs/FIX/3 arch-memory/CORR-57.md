# CORR-57 — handoff.rs + forge.rs : MSI non masqués et FLR fictif (CRIT-04 + CRIT-05)

**Sévérité :** 🔴 CRITIQUE — Sécurité ExoPhoenix  
**Fichiers :** `kernel/src/exophoenix/handoff.rs`, `kernel/src/exophoenix/forge.rs`  
**Impact :** Violation garantie de la garantie G2 (MSI) et G3 (FLR) d'ExoPhoenix

---

## Problème CRIT-04 — `mask_all_msi_msix()`

```rust
// ÉTAT ACTUEL — handoff.rs:217–221
fn mask_all_msi_msix() {
    // Best-effort temporaire : l'infra PCIe capability MSI/MSI-X globale n'est
    // pas encore exposée
    core::sync::atomic::fence(Ordering::SeqCst);
}
```

Une `fence(SeqCst)` ne masque **aucun** MSI/MSI-X matériel. Les interruptions PCI peuvent arriver sur les cores de Kernel A pendant le gel, corrompant l'état d'isolation.

## Problème CRIT-05 — `pci_function_level_reset()`

```rust
// ÉTAT ACTUEL — forge.rs:171–180
fn pci_function_level_reset(bus: u8, device: u8, func: u8) -> Result<(), ForgeError> {
    // [ADAPT] : pci::config_write16(...)
    let _ = (bus, device, func);
    Ok(())
}
```

La séquence G3 (FLR → drain DMA → IOTLB flush → reload driver) est entièrement fictive.

---

## Correction CRIT-04 — MSI masking via PCI config space

```rust
/// Masque tous les MSI et MSI-X des devices PCI actifs avant le freeze IPI.
///
/// Méthode : parcourir le bus PCI via accès I/O ports (CF8/CFC) et désactiver
/// le bit Enable dans la capability MSI (offset cap+2, bit 0) et le bit
/// Function Mask dans MSI-X (offset cap+6, bit 14).
///
/// # Garantie G2
/// Après retour, aucun device PCI ne peut émettre de MSI/MSI-X.
/// Les interruptions pin-based (LINT0/LINT1) sont gérées séparément par
/// le masquage IOAPIC dans mask_ioapic_pins().
fn mask_all_msi_msix() {
    use crate::arch::x86_64::pci::{pci_config_read16, pci_config_write16,
                                    pci_config_read8, PCI_CAP_ID_MSI,
                                    PCI_CAP_ID_MSIX, PCI_STATUS_CAP_LIST};

    // Scanner les 256 premiers buses, 32 devices, 8 fonctions.
    // En pratique, s'arrêter au premier bus vide pour performance.
    for bus in 0u8..=255 {
        for dev in 0u8..32 {
            for func in 0u8..8 {
                let vendor = pci_config_read16(bus, dev, func, 0x00);
                if vendor == 0xFFFF { 
                    if func == 0 { break; } // pas de multi-function
                    continue;
                }

                let status = pci_config_read16(bus, dev, func, 0x06);
                if status & PCI_STATUS_CAP_LIST == 0 {
                    continue; // pas de capability list
                }

                // Parcourir la chaîne de capabilities.
                let mut cap_ptr = pci_config_read8(bus, dev, func, 0x34) & 0xFC;
                let mut guard = 0u8; // anti-boucle infinie

                while cap_ptr != 0 && guard < 48 {
                    guard += 1;
                    let cap_id = pci_config_read8(bus, dev, func, cap_ptr);

                    if cap_id == PCI_CAP_ID_MSI {
                        // MSI — désactiver le bit Enable (bit 0 du Message Control)
                        let mc = pci_config_read16(bus, dev, func, cap_ptr + 2);
                        pci_config_write16(bus, dev, func, cap_ptr + 2, mc & !0x0001);
                    } else if cap_id == PCI_CAP_ID_MSIX {
                        // MSI-X — activer le bit Function Mask (bit 14 du Message Control)
                        let mc = pci_config_read16(bus, dev, func, cap_ptr + 2);
                        pci_config_write16(bus, dev, func, cap_ptr + 2, mc | 0x4000);
                    }

                    cap_ptr = pci_config_read8(bus, dev, func, cap_ptr + 1) & 0xFC;
                }
            }
        }
    }

    // Barrière : s'assurer que les écritures config space sont visibles
    // avant l'envoi de l'IPI Freeze.
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    // Lecture dummy pour flush le write buffer PCI (posted writes).
    let _ = unsafe { core::ptr::read_volatile(0xCFC as *const u32) };
}

/// Ré-active tous les MSI/MSI-X après restauration de Kernel A.
/// Appelé depuis la séquence de recovery ExoPhoenix.
fn unmask_all_msi_msix() {
    // Symétrique à mask_all_msi_msix() — inverser les bits.
    // Implémentation identique avec bits inversés (Enable=1, FunctionMask=0).
    // TODO: stocker la liste des devices MSI dans la SSR pour éviter le re-scan.
}
```

---

## Correction CRIT-05 — `pci_function_level_reset()`

```rust
/// Effectue un Function-Level Reset (FLR) sur un device PCI via PCIe capability.
///
/// Séquence PCIe Base Spec r5.0 §6.6.2 :
/// 1. Vider les pending transactions (attendre 100ms)
/// 2. Écrire FLR bit dans Device Control register (offset cap+8, bit 15)
/// 3. Attendre 100ms (temps de recovery FLR)
/// 4. Vérifier que le device répond (vendor ID != 0xFFFF)
fn pci_function_level_reset(bus: u8, device: u8, func: u8) -> Result<(), ForgeError> {
    use crate::arch::x86_64::pci::{pci_config_read16, pci_config_write16,
                                    pci_config_read8, PCI_CAP_ID_PCIE};
    use crate::arch::x86_64::time::ktime::ktime_elapsed_ns;
    use crate::arch::x86_64::time::ktime::ktime_get_ns;

    // Vérifier que le device est présent.
    let vendor = pci_config_read16(bus, device, func, 0x00);
    if vendor == 0xFFFF {
        return Err(ForgeError::PciDeviceAbsent);
    }

    // Trouver la PCIe capability.
    let status = pci_config_read16(bus, device, func, 0x06);
    if status & 0x0010 == 0 {
        // Pas de capability list → pas de FLR supporté.
        return Err(ForgeError::FlrNotSupported);
    }

    let mut cap_ptr = pci_config_read8(bus, device, func, 0x34) & 0xFC;
    let mut pcie_cap_offset: Option<u8> = None;
    let mut guard = 0u8;

    while cap_ptr != 0 && guard < 48 {
        guard += 1;
        let cap_id = pci_config_read8(bus, device, func, cap_ptr);
        if cap_id == PCI_CAP_ID_PCIE {
            pcie_cap_offset = Some(cap_ptr);
            break;
        }
        cap_ptr = pci_config_read8(bus, device, func, cap_ptr + 1) & 0xFC;
    }

    let pcie_offset = pcie_cap_offset.ok_or(ForgeError::FlrNotSupported)?;

    // Vérifier que FLR est supporté (Device Capabilities register, bit 28).
    let dev_cap = pci_config_read16(bus, device, func, pcie_offset + 4);
    // Note: FLR bit est dans les 32 bits — lire le mot haut (offset+6 pour bits 31:16).
    let dev_cap_hi = pci_config_read16(bus, device, func, pcie_offset + 6);
    if dev_cap_hi & 0x1000 == 0 {
        // Bit 28 (bit 12 du mot haut) = FLR Capability.
        return Err(ForgeError::FlrNotSupported);
    }

    // Étape 1 : quiescence — attendre 100ms pour vider les transactions en cours.
    let start = ktime_get_ns();
    while ktime_elapsed_ns(start) < 100_000_000 {
        core::hint::spin_loop();
    }

    // Étape 2 : déclencher FLR via Device Control register (offset cap+8, bit 15).
    let dev_ctrl = pci_config_read16(bus, device, func, pcie_offset + 8);
    pci_config_write16(bus, device, func, pcie_offset + 8, dev_ctrl | 0x8000);

    // Étape 3 : attendre 100ms recovery.
    let start = ktime_get_ns();
    while ktime_elapsed_ns(start) < 100_000_000 {
        core::hint::spin_loop();
    }

    // Étape 4 : vérifier que le device répond à nouveau.
    let vendor_after = pci_config_read16(bus, device, func, 0x00);
    if vendor_after == 0xFFFF {
        return Err(ForgeError::FlrDeviceUnresponsive);
    }

    Ok(())
}
```

---

## Nouveaux variants `ForgeError`

```rust
// Dans l'enum ForgeError — ajouter :
/// Device PCI absent (Vendor ID = 0xFFFF).
PciDeviceAbsent,
/// Device PCI ne supporte pas FLR (PCIe capability absente ou bit FLR Capability = 0).
FlrNotSupported,
/// Device non répondu après FLR (Vendor ID reste 0xFFFF après 100ms recovery).
FlrDeviceUnresponsive,
```

---

## Note d'implémentation Phase 1

Pour Phase 1 (sécurité critique), une **implémentation minimale acceptable** de `mask_all_msi_msix()` peut se limiter à masquer les devices connus (liste statique de BDF stockée dans la SSR lors du boot). Le scan complet CF8/CFC peut être différé à Phase 2. Mais la `fence()` seule est inacceptable — au minimum, masquer les 4 devices PCI les plus actifs du système.

---

**Dépendances :** `crate::arch::x86_64::pci` — vérifier que `pci_config_read16/write16` existent  
**Priorité :** CRIT-04 bloquant pour G2, CRIT-05 bloquant pour G3
