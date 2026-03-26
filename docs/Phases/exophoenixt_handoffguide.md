Phase 3.6 maintenant.**

---

```
Phase 3.6 du guide ExoOS v2 — handoff.rs
TLA+ validé : 356 états, 0 erreur, 2026-03-21.

Modifier sentinel.rs :
Remplacer :
    if score >= THREAT_THRESHOLD {
        PHOENIX_STATE.store(PhoenixState::Threat as u8,
                            Ordering::Release);
    }
Par :
    if score >= THREAT_THRESHOLD {
        PHOENIX_STATE.store(PhoenixState::Threat as u8,
                            Ordering::Release);
        let _ = handoff::begin_isolation_soft();
    }

Implémenter kernel/src/exophoenix/handoff.rs :

PHASE 1 — begin_isolation_soft() :
1. IPI 0xF1 broadcast ET soft revoke IOMMU simultanément (G4)
2. Timeout 100µs via TICKS_PER_US — attendre :
   - tous ACK dans SSR[freeze_ack_offset(slot)] avec Acquire
   - iommu_drain confirmé
3. Si timeout → begin_isolation_hard()
4. Hard revoke IOMMU + IOTLB flush QI/Completion Wait (S-N1)
5. PHOENIX_STATE = IsolationHard (Ordering::Release)
6. forge::reconstruct_kernel_a()
7. Si succès → PHOENIX_STATE = Restore puis Normal
8. Si échec × 3 → PHOENIX_STATE = Degraded

PHASE 2 — begin_isolation_hard() :
1. mask_all_msi_msix() AVANT INIT IPI (G2)
2. INIT IPI vers cores résistants
3. Hard revoke IOMMU + IOTLB flush sans drain
4. scan_and_release_spinlocks()
5. PHOENIX_STATE = Certif (Ordering::Release)

Contraintes absolues :
- SSR_HANDOFF_FLAG Ordering::Release/Acquire (S9)
- apic_to_slot — jamais apic_id*64 direct (S10)
- ZÉRO spinlock dans chemins critiques (S1)
- G8 : SIPI_SENT déjà posé en stage0 — ne pas renvoyer
- Ne pas toucher init_reaper
- Cargo check WSL à la fin
```