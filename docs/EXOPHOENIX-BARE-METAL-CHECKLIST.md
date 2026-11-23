# ExoPhoenix — Checklist de Certification Bare-Metal x86_64 (`F10` / Palier 3)

**Statut** : Prêt pour exécution sur matériel physique x86_64 (non exécutable en VM/WSL2).  
**Prérequis** : `F0`–`F9` validés à 100 % sous QEMU TCG `-smp 4` et QEMU KVM + Intel VT-d (`make qemu-phoenix-kvm`).

---

## 1. Prérequis Matériels de la Plateforme de Test

| Composant | Exigence minimale |
| :--- | :--- |
| **CPU** | Intel Core 11e gén.+ ou Intel Xeon avec **VT-d**, **Invariant TSC** (`CPUID 0x8000_0007 EDX[8]`), **CET** (Shadow Stack + IBT), **x2APIC** |
| **Firmware** | UEFI 64-bit, Intel VT-d / IOMMU activé dans le BIOS, Secure Boot en mode clé custom Exo-OS ou désactivé pour le banc `phoenix-fault-injection` |
| **Périphériques PCIe** | Contrôleur **NVMe** (ExoFS réel) + NIC **Intel e1000 / VirtIO** supportant **PCIe FLR** (`Cap 0x10`) |
| **Console d'audit** | Port série UART `COM1 (0x3F8, 115200 8N1)` ou adaptateur USB-Serial / capture netconsole |

---

## 2. Préparation des Artefacts

1. **Production pure (audit `F0`)** :
   ```bash
   EXO_PROFILE=pure make release rootfs-image
   python3 scripts/qemu/run_security_audit.py --s0-only
   ```
   Vérifier : `0` symbole `phoenix_inject_*`, `0` référence `out 0xf4`, signature Ed25519 `EXOSIG01` vérifiée (`make verify-kernel`).
2. **Image de qualification matérielle (`F1`–`F9` + `F10`)** :
   ```bash
   make iso-phoenix-fault-test
   ```
   Écrire `target/qemu/exo-os-fault-test.iso` sur média USB bootable (`dd if=target/qemu/exo-os-fault-test.iso of=/dev/sdX bs=4M status=progress oflag=sync`).

---

## 3. Protocole de Certification Bare-Metal (`F10`) — Cases à Valider

- [ ] **BM-01 — Boot UEFI réel (`exo-boot`)** :
  - `exo-boot` charge Kernel B (Core 0) puis Kernel A sans écraser la mémoire basse (`T13-001`/`T13-002`).
  - `CPUID 0x8000_0007` confirme **Invariant TSC** (`tsc_khz` stable, 0 dérive négative inter-cœurs).
  - `V7-C-05` vérifié : `MAX_CORES_RUNTIME <= SSR_MAX_CORES_LAYOUT (256)` sur tous les cœurs physiques détectés dans la table ACPI MADT.

- [ ] **BM-02 — Intel VT-d matériel réel (`DMA-0` / `F7`)** :
  - Tables ACPI **DMAR** parsées (`INTEL_VTD.unit_count() >= 1`).
  - Contrôleurs NVMe et NIC attachés en politique *default-deny* (`blocked_domain_id != 0`).
  - Une transaction DMA hors domaine déclenche une vraie interruption DMAR captée par `iommu_fault_isr`, enfilée dans `IOMMU_FAULT_QUEUE` sans slot orphelin (`FIX-104`), bloque physiquement la transaction côté chipset, tue le pilote fautif (`SIGKILL`), et laisse la RAM Kernel A/B intacte (`sentinel_is_intact() == true`).

- [ ] **BM-03 — Intel CET matériel (`#CP` / `ExoCage`)** :
  - Shadow Stack (`CR4.CET`, `IA32_S_CET`) actif au boot.
  - Violation volontaire de pile de retour génère un `#CP` (vecteur 21) intercepté par `do_ctrl_protection` -> `exocage::cp_handler` et gravé dans la zone P0 immuable d'`ExoLedger`.

- [ ] **BM-04 — Vrai `PCIe FLR` sur contrôleur physique (NVMe / e1000)** :
  - Bascule ExoPhoenix exécutant `disable_bus_master()` -> PCIe Function Level Reset -> `wait_link_retraining()` (`100 ms` / fallback `250 ms` `DRV-37/41`).
  - **0** `#MC` (Machine Check Exception), **0** Completer Abort (`CA`), **0** page `DmaLease` libérée prématurément avant complétion/invalidation IOTLB (`V5-2`).

- [ ] **BM-05 — Survie matérielle non coopérative & SLA $\le 1\text{ s}$ au TSC physique (`F1` + `F3`)** :
  - Exécuter `/bin/exo-compliance-runner --phoenix-qual --suite all` sur la machine physique.
  - `FAULT_KERNEL_A_DEADLOCK` (`cli; pause`) intercepté par vrai **NMI matériel** sur pile dédiée `IST_NMI` -> restauration physique depuis le Golden RAM Mirror -> reprise écran `fb_server` + réseau + `exosh`.
  - En mode `COLD_RESPAWN` (`F3`) : `S0_US <= 50_000 us` (50 ms) et `S1_TOTAL_US (t0 -> t5) <= 1_000_000 us` (**$\le 1\text{ s}$** mesuré au TSC matériel).

- [ ] **BM-06 — Endurance & Soak 24 h (`F9` étendu)** :
  - 100 cycles ExoPhoenix enchaînés (mix `SOFT_FORCE` + `COLD_RESPAWN` + `CRASH_NMI`) suivis de 24 h de charge continue (réseau, ExoFS, Wayland `/run/wayland-0`).
  - **0** fuite de frames physiques, **0** fuite PID/FD/epoll/ancillary, `exo audit --verify-chain` **100 % INTACT (PASS)**.

---

## 4. Critère de Signature Finale

La plateforme matérielle est certifiée **« ExoPhoenix Niveau Critique Indiscutable »** lorsque :
1. Les logs série UART de `BM-01` à `BM-06` sont archivés dans `docs/reports/baremetal-<modele>-<date>.log`.
2. Tous les tests `F0`..`F10` affichent `PASS` avec `DEGRADED=0` en nominal et `S1_TOTAL_US <= 1_000_000 us` en `COLD_RESPAWN`.
