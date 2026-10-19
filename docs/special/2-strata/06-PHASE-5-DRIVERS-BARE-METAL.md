## Fichier 6 : `06-PHASE-5-DRIVERS-BARE-METAL.md`

```markdown
# 06 — PHASE 5 : Drivers Bare Metal (AHCI, NVMe, USB, Audio, Clock)

## 1. État Réel dans le Dépôt
* **Déjà implémentés et fonctionnels dans `drivers/`** :
  * **Stockage** : `drivers/storage/ahci` (binaire `ahci_driver`, endpoint 30), `drivers/storage/nvme` (binaire `nvme_driver`, endpoint 31), `drivers/storage/virtio_blk` (binaire `virtio_blk_driver`, endpoint 32) et `drivers/storage/common_hal` (`ExoUserspaceStorageHal` + `run_block_server`) sont **100 % écrits et testés** avec des contrôleurs simulés (`MockAhci`, `MockNvme`, `SparseBackend`)[cite: 17].
  * **Réseau** : `drivers/network/e1000` (endpoint 16), `drivers/network/virtio_net` (endpoint 14), `drivers/network/loopback` (endpoint 15) et `drivers/network/common` sont complets[cite: 17].
  * **Entrée & Console** : `drivers/input/ps2` (`exo-ps2-input`, clavier Set1/Set2 + souris PS/2) et `drivers/tty` (`exo-tty`, endpoint 22) sont complets[cite: 17].
  * **Framework** : `drivers/src/lib.rs` (`exo_os_drivers`) fournit déjà `DeviceHandle::claim_and_map`, `IrqHandle`, et les traits `ExoDriver`, `BlockDevice`, `InputDevice`, `AudioDevice`, `NetDevice`[cite: 17].
* **Écarts restants à implémenter dans `drivers/`** :
  1. **`drivers/input/usb_hid/`** : Totalement absent (xHCI, EHCI, énumération USB, HID clavier/souris, Mass Storage BBB/SCSI)[cite: 10, 14, 17].
  2. **`drivers/audio/hda/` et `drivers/audio/virtio_sound/`** : Totalement absents[cite: 10, 14, 17].
  3. **`drivers/clock/`** : Absent en tant que crate dédiée dans `drivers/` (le noyau gère déjà HPET/TSC en Ring 0 lors de la Phase 2c `time_init()`, mais il manque le lecteur RTC CMOS et l'interface IPC vers `scheduler_server`)[cite: 7, 10, 14, 17].

---

## 2. Procédure d'Implémentation Sans Ambiguïté

### A. Implémentation de `drivers/input/usb_hid/` (`5.9`–`5.13`)
1. Créer `drivers/input/usb_hid/Cargo.toml` et structurer `src/` selon `SPEC-DRIVERS-STRATA.md` §4[cite: 14] :
   * `xhci/mod.rs`, `ring.rs`, `context.rs` : Détecter le contrôleur xHCI via `SYS_PCI_FIND_DEVICE` (547) avec `class = 0x0C, subclass = 0x03` (`prog_if = 0x30`), revendiquer et mapper le BAR0 via `DeviceHandle::claim_and_map`, allouer le Device Context Base Address Array (DCBAA), le Command Ring et l'Event Ring via `SYS_DMA_ALLOC` (534)[cite: 7, 14, 17].
   * `enumeration.rs` : Scanner les registres `PORTSC` (`CCS == 1`), émettre un Port Reset (`PR = 1`), assigner une adresse (`Enable Slot` + `Address Device`), lire les descripteurs (`Device`, `Configuration`, `Interface`, `Endpoint`) et aiguiller selon `bInterfaceClass`[cite: 14] :
     * **Classe `0x03` (HID Boot Protocol)** : Configurer `SET_PROTOCOL(0)`, lire les rapports de 8 octets sur l'endpoint Interrupt IN, convertir en `syscall::InputEventWire` et envoyer `INPUT_MSG_PUSH` (`0x120`) à `INPUT_SERVER_ENDPOINT` (11), exactement comme `drivers/input/ps2/src/main.rs`[cite: 14, 17, 18].
     * **Classe `0x08` (Mass Storage BBB, `subclass = 0x06`, `protocol = 0x50`)** : Dans `mass_storage/bbb.rs` et `scsi.rs`, implémenter l'envoi de `CBW` (signature `0x43425355` "USBC") sur Bulk OUT, la phase de données DMA, et la lecture de `CSW` (signature `0x53425355` "USBS") sur Bulk IN pour les commandes SCSI `TEST_UNIT_READY (0x00)`, `INQUIRY (0x12)`, `READ_CAPACITY (0x25)`, `READ_10 (0x28)` et `WRITE_10 (0x2A)`[cite: 14].

### B. Implémentation de `drivers/audio/hda` et `drivers/audio/virtio_sound` (`5.14`–`5.17`)
1. **`drivers/audio/virtio_sound/`** : Détecter le périphérique VirtIO Sound (Vendor `0x1AF4`, Device ID modern `0x1059` / VirtIO ID `25`), initialiser les 4 virtqueues (`control=0`, `event=1`, `tx=2`, `rx=3`), envoyer `VIRTIO_SND_R_PCM_SET_PARAMS` (stream 0, format `S16`, rate `44100 Hz`, 2 canaux), `PCM_PREPARE` et `PCM_START`, puis implémenter le trait `AudioDevice::play_pcm()` de `drivers/src/lib.rs`[cite: 14, 17].
2. **`drivers/audio/hda/`** : Détecter Intel HDA via `SYS_PCI_FIND_DEVICE` (`class = 0x04, subclass = 0x03`), sortir le contrôleur de reset (`GCTL.CRST = 1`), allouer CORB/RIRB et la Buffer Descriptor List (BDL) via `SYS_DMA_ALLOC` (534), configurer le stream de sortie (`SDnFMT = 0x0011` pour 44.1 kHz, 16-bit, stéréo) et implémenter `AudioDevice::play_pcm()`[cite: 14, 17].

### C. Implémentation de `drivers/clock/` (`5.18`–`5.20`)
1. Créer `drivers/clock/src/{lib.rs, rtc.rs, hpet.rs}`[cite: 14].
2. Pour permettre la lecture des ports CMOS RTC (`0x70` / `0x71`) depuis l'espace utilisateur ou via le noyau, noter que `sys_ioport_read` / `sys_ioport_write` dans `kernel/src/syscall/table/drivers.rs` n'autorise actuellement que `ps2_driver` sur les ports `0x60` et `0x64` (`ps2_ioport_allowed`)[cite: 7]. Il faut donc soit lire le RTC CMOS en Ring 0 lors de `arch::x86_64::time::time_init()` pour initialiser `CLOCK_REALTIME_OFFSET` (`clock::realtime_offset_ns()` dans `kernel/src/syscall/fast_path.rs`), soit ajouter une `ServiceClass::ClockDriver` autorisée sur `0x70`/`0x71`[cite: 7, 14].

---

## 3. Vigilance et Invariants pour ne pas Perturber le Noyau
* **Respect absolu de l'ordre des syscalls PCI/IOMMU (Invariants TLA+ `S19`, `S20`, `S24`, `S39`, `S40`)** :
  Tout nouveau pilote (`usb_hid`, `hda`, `virtio_sound`) doit obligatoirement utiliser `DeviceHandle::claim_and_map()` (`drivers/src/lib.rs`) ou `ExoUserspaceStorageHal::claim_and_map_pci()` (`drivers/storage/common_hal/src/lib.rs`) qui exécute dans cet ordre strict[cite: 17] :
  1. `SYS_PCI_CLAIM` (540) — qui alloue également le domaine IOMMU du PID via `ensure_domain_for_pid(owner_pid)` dans `kernel/src/syscall/table/drivers.rs`[cite: 7, 17].
  2. `SYS_MMIO_MAP` (532)[cite: 7, 17].
  3. `SYS_PCI_BUS_MASTER` (539)[cite: 7, 17].
  4. `SYS_PCI_SET_TOPOLOGY` (546) — **obligatoire avant tout appel à `SYS_IRQ_REGISTER` (530)**[cite: 7, 14, 17].
* **Interdiction d'utiliser `PhysAddr` dans un registre DMA** : Un pilote Ring 1/Ring 3 ne doit **jamais** programmer autre chose que l'`iova` retournée par `SYS_DMA_ALLOC` (534) dans les registres de descripteurs matériels (PRDT AHCI, PRP NVMe, Virtqueue, BDL HDA, TRB xHCI)[cite: 7, 17, 19].
* **Règle `SRV-02` (Zéro flottant / SIMD dans les drivers)** : Les pilotes sont compilés sans sauvegarde d'état FPU/SIMD spécial ; toutes les opérations doivent rester en arithmétique entière pure[cite: 7, 17].