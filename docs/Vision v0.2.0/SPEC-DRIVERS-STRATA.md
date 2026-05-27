# SPEC-DRIVERS-STRATA — Drivers Bare Metal ExoOS v0.2.0
## État, Périmètre et Spécifications — Strata

**Auteur :** claude-alpha
**Date :** 2026-05-26
**Statut :** NOUVEAU — remplace SPEC-EXO-DRIVERS-V0.2.md

---

## 1. Inventaire Complet des Drivers

### 1.1 — Drivers Opérationnels (existants, conservés)

| Driver | Chemin | Lignes | État |
|---|---|---|---|
| `virtio_net` | `drivers/network/virtio_net/` | ~700 | ✅ Opérationnel |
| `e1000` | `drivers/network/e1000/` | ~629 | ✅ Opérationnel |
| `ps2` | `drivers/input/ps2/` | ~480 | ✅ Opérationnel |
| `virtio_blk` | `drivers/storage/virtio_blk/` | ~260 | ✅ Opérationnel |
| `tty` | `drivers/tty/` | ~140 | ✅ Opérationnel |
| `framebuffer` | `drivers/display/framebuffer/` | ~117 | ✅ Opérationnel |
| `vga` | `drivers/display/vga/` | ~156 | ✅ Opérationnel |
| `loopback` | `drivers/network/loopback/` | ~166 | ✅ Opérationnel |
| `fat32` | `drivers/fs/fat32/` | ~280 | ✅ Partiel — compléter v0.2.0 |
| `ext4` | `drivers/fs/ext4/` | ~300 | ✅ Partiel — lecture seule ok |

### 1.2 — Drivers à Implémenter — Strata

| Driver | Priorité | Phase | Usage |
|---|---|---|---|
| `storage/ahci` | P1 | 5 | SATA bare metal |
| `storage/nvme` | P1 | 5 | NVMe bare metal |
| `input/usb_hid` | P1 | 5 | USB clavier + clés USB |
| `audio/hda` | P1 | 5 | Son système (chime + bell) |
| `audio/virtio_sound` | P1 | 5 | Son système en VM |
| `clock` | P1 | 5 | RTC + HPET |
| `framework` | P2 | 5 | Bus unification |
| `manager` | P2 | 5 | Hot-plug lifecycle |

### 1.3 — Drivers Hors Périmètre Strata

| Driver | Raison | Version |
|---|---|---|
| `display/virtio_gpu` | Dépend wgpu/Wayland | v0.3.0 |
| `input/evdev` | Abstraction Linux-style non native | v0.3.0 |
| `audio/mixer` | Dépend compositeur audio Ring1 complet | v0.3.0 |

---

## 2. AHCI — SATA Bare Metal

**Fichiers :** `drivers/storage/ahci/src/`

```
ahci/src/
├── lib.rs       → point d'entrée, init, probe
├── hba.rs       → HBA registers, PORT registers
├── port.rs      → init port, command list, FIS buffer
├── cmd.rs       → command header, command table, build
├── fis.rs       → FIS types : H2D, D2H, PRDT
├── dma.rs       → allocation DMA SYS_DMA_ALLOC=534
├── irq.rs       → handler interrupt AHCI
└── error.rs     → AHCIError enum
```

### 2.1 — Séquence d'Initialisation

```rust
// Étape 1 : Détection PCI
// Class = 0x01, Subclass = 0x06, ProgIF = 0x01
// BAR[5] = ABAR (AHCI Base Address Register)

// Étape 2 : Enable AHCI
// GHC.AE (bit 31) = 1
// GHC.HR (bit 0) = 1 → reset → attendre reset = 0 (timeout 1s)

// Étape 3 : Déterminer ports actifs
// PI (Port Implemented) bitmask → jusqu'à 32 ports

// Étape 4 : Init chaque port actif
// Port DET = 3 (device present + comm established)
// Port IPM = 1 (interface active)

// Étape 5 : Allouer Command List + FIS Receive Area
// Command List : 1024 bytes aligné 1KB (32 × 32B command headers)
// FIS buffer   : 256 bytes aligné 256B
// Via SYS_DMA_ALLOC (physique + IOVA fournis)

// Étape 6 : START (PxCMD.ST = 1, PxCMD.FRE = 1)
```

### 2.2 — Lecture / Écriture

```rust
pub fn read_sectors(port: u8, lba: u64, count: u16, buf: &mut [u8])
    -> Result<(), AhciError>
{
    // ATA command : READ DMA EXT (0x25)
    // Construct FIS_H2D avec LBA 48-bit, sector count
    // Allouer PRDT dans command table
    // Soumettre slot → PxCI |= (1 << slot)
    // Attendre completion via interrupt ou polling (timeout 30s)
    // Vérifier TFD.ERR == 0
}

pub fn write_sectors(port: u8, lba: u64, count: u16, buf: &[u8])
    -> Result<(), AhciError>
{
    // ATA command : WRITE DMA EXT (0x35)
    // Même séquence, sens inversé
    // Flush : FLUSH CACHE EXT (0xEA) après write critique
}
```

### 2.3 — Interface device_server

```rust
// IPC depuis device_server → AHCI driver
pub enum AhciRequest {
    Identify(u8 port),                           // → AhciDeviceInfo
    ReadBlocks { port: u8, lba: u64, count: u32 }, // → Vec<u8>
    WriteBlocks { port: u8, lba: u64, data: Vec<u8> },
    Flush(u8 port),
}
```

---

## 3. NVMe — SSD Bare Metal

**Fichiers :** `drivers/storage/nvme/src/`

```
nvme/src/
├── lib.rs       → détection PCI, init, probe
├── ctrl.rs      → NVMe controller registers (BAR0)
├── queue.rs     → submission queue + completion queue
├── cmd.rs       → commandes NVM : Identify, Read, Write
├── dma.rs       → PRP (Physical Region Page) management
├── irq.rs       → MSI-X handler, MSI fallback
└── error.rs     → NVMeError enum
```

### 3.1 — Registres Contrôleur (MMIO BAR0)

```rust
// Registres principaux (offsets)
const CAP:   u64 = 0x00;   // Capabilities
const VS:    u64 = 0x08;   // Version
const CC:    u64 = 0x14;   // Controller Config (CC.EN, CC.MPS, CC.CSS)
const CSTS:  u64 = 0x1C;   // Controller Status (CSTS.RDY)
const AQA:   u64 = 0x24;   // Admin Queue Attributes
const ASQ:   u64 = 0x28;   // Admin Submission Queue Base Address
const ACQ:   u64 = 0x30;   // Admin Completion Queue Base Address
// I/O queues : Doorbell registers à 0x1000 + (2n * dstrd)
```

### 3.2 — Séquence d'Initialisation

```rust
// 1. Disable controller : CC.EN = 0, attendre CSTS.RDY = 0 (timeout 500ms)
// 2. Configurer Admin Queue :
//    AQA.ASQS = 63 (64 entrées), AQA.ACQS = 63
//    ASQ = phys_addr(admin_submission_queue)
//    ACQ = phys_addr(admin_completion_queue)
// 3. Enable : CC.MPS = 0 (4KB pages), CC.CSS = 0 (NVM command set), CC.EN = 1
// 4. Attendre CSTS.RDY = 1 (timeout 500ms)
// 5. Identify Controller (Admin CMD 0x06, CNS=0x01)
// 6. Identify Namespace 1 (Admin CMD 0x06, CNS=0x00, NSID=1)
// 7. Créer I/O Submission Queue (Admin CMD 0x01)
// 8. Créer I/O Completion Queue (Admin CMD 0x05)
// 9. Configurer MSI-X (si disponible) ou MSI
```

### 3.3 — Read / Write NVM

```rust
// NVM Read (opcode 0x02)
// NVM Write (opcode 0x01)
// PRP1 = physical address premier 4KB bloc
// PRP2 = physical address suivant 4KB (ou PRP List si > 2 blocs)

pub fn read_blocks(nsid: u32, slba: u64, nlb: u16, buf: &mut [u8])
    -> Result<(), NvmeError>
{
    let cmd = NvmCommand {
        opcode: 0x02,
        nsid,
        cdw10: (nlb as u32 - 1) | (slba as u32),
        cdw11: (slba >> 32) as u32,
        prp1:  buf_phys,
        prp2:  prp_list_phys_if_needed,
        ..Default::default()
    };
    submit_io_cmd(cmd)
}
```

---

## 4. USB HID + Mass Storage

**Fichiers :** `drivers/input/usb_hid/src/`

```
usb_hid/src/
├── lib.rs           → détection contrôleur XHCI/EHCI, probe
├── xhci/
│   ├── mod.rs       → init XHCI
│   ├── ring.rs      → Transfer Ring, Command Ring, Event Ring
│   └── context.rs   → Device Context, Input Context
├── ehci/
│   ├── mod.rs       → init EHCI (fallback)
│   └── qh.rs        → Queue Head, Queue Transfer Descriptor
├── enumeration.rs   → USB enumeration : reset, address, descriptors
├── hid/
│   ├── keyboard.rs  → HID boot protocol keyboard
│   └── mouse.rs     → HID boot protocol mouse
├── mass_storage/
│   ├── bbb.rs       → Bulk-Only Transport protocol
│   └── scsi.rs      → SCSI commands : INQUIRY, READ_CAPACITY, READ_10
└── event.rs         → USB events vers input_server / device_server
```

### 4.1 — Détection Contrôleur USB

```rust
// XHCI : PCI class 0x0C, subclass 0x03, prog-if 0x30
// EHCI : PCI class 0x0C, subclass 0x03, prog-if 0x20
// OHCI : PCI class 0x0C, subclass 0x03, prog-if 0x10 (legacy, non supporté)

// Priorité : XHCI > EHCI
// Si XHCI présent : release ports EHCI vers XHCI (EECP)
```

### 4.2 — Énumération USB

```rust
// Pour chaque port avec device connected (PORTSC.CCS = 1) :
// 1. Reset port (PORTSC.PR = 1)
// 2. Attendre PORTSC.PRC = 1
// 3. Assign address (SET_ADDRESS, addr 1..127)
// 4. GET_DESCRIPTOR(Device) → DeviceDescriptor
// 5. GET_DESCRIPTOR(Configuration) → ConfigDescriptor
// 6. SET_CONFIGURATION(config_value)
// 7. Parse Interface Descriptors → déterminer classe
//    Class 0x03 : HID
//    Class 0x08 : Mass Storage
```

### 4.3 — HID Clavier (Boot Protocol)

```rust
// SET_PROTOCOL → Boot Protocol (protocol=0)
// Polling endpoint IN toutes les 8ms
// Boot Report : 8 octets
//   [0] = modifier keys (Ctrl, Shift, Alt, GUI)
//   [1] = reserved
//   [2..7] = jusqu'à 6 keycodes simultanés
// Conversion keycode → input_server::KeyEvent
```

### 4.4 — USB Mass Storage (BBB)

```rust
// BBB : Bulk-Only Transport
// Endpoints : 1 Bulk IN + 1 Bulk OUT
//
// Séquence par commande :
// 1. CBW (Command Block Wrapper) → Bulk OUT
//    Signature = 0x43425355 ("USBC")
//    CDB = SCSI command (jusqu'à 16 octets)
// 2. Data phase → Bulk IN ou OUT selon direction
// 3. CSW (Command Status Wrapper) → Bulk IN
//    Signature = 0x53425355 ("USBS")
//    Status = 0 (success), 1 (failure), 2 (phase error)

// SCSI commands implémentés :
// INQUIRY (0x12)       → identifier le device
// READ_CAPACITY (0x25) → taille totale
// READ_10 (0x28)       → lire secteurs
// WRITE_10 (0x2A)      → écrire secteurs
// TEST_UNIT_READY (0x00) → vérifier disponibilité
```

### 4.5 — Événements vers device_server

```rust
pub enum UsbEvent {
    MassStorageAttached {
        address: u8,
        vendor_id: u16,
        product_id: u16,
        total_sectors: u64,
        sector_size: u32,
    },
    MassStorageDetached { address: u8 },
    HidKeyEvent(KeyEvent),
    HidMouseEvent(MouseEvent),
}
// → IPC vers device_server (DEVICE_ATTACHED / DEVICE_DETACHED)
// → device_server → vfs_server pour mount automatique
```

---

## 5. Audio — Intel HDA + virtio-sound

**Fichiers :** `drivers/audio/hda/src/`, `drivers/audio/virtio_sound/src/`

### 5.1 — Périmètre v0.2.0 (Audio Système Uniquement)

ExoOS v0.2.0 n'est pas un ordinateur multimédia. L'audio v0.2.0 est la **voix du système** :

| Événement | Son | Durée | Fréquence |
|---|---|---|---|
| Boot complet (exosh prêt) | Chime agréable | ~0.5s | 44100Hz stereo |
| Terminal bell (BEL 0x07) | Beep court | 100ms | 800Hz mono |
| Alerte sécurité HIGH | 3 bips courts | 3×150ms | 300Hz |
| Alerte sécurité CRITICAL | 1 bip long | 1000ms | 200Hz |

Les PCM de ces sons sont **embarqués statiquement** dans le binaire `audio_server`.
Pas de lecture fichiers audio, pas de mixer, pas d'API Ring3 générique en v0.2.0.

### 5.2 — Intel HDA (Hardware)

```
hda/src/
├── lib.rs       → init HBA, registres MMIO
├── corb.rs      → Command Output Ring Buffer
├── rirb.rs      → Response Input Ring Buffer
├── codec.rs     → énumération codec, widget graph
├── dma.rs       → BDL (Buffer Descriptor List), DMA position
├── stream.rs    → Output Stream (SDnCTL, SDnSTS)
└── pcm.rs       → format PCM : 44100Hz, 16-bit, stereo
```

**Séquence init HDA :**
```
1. Détection PCI : class 0x04, subclass 0x03
2. GCTL.CRST = 0 → reset → GCTL.CRST = 1 (attendre codec présent)
3. GCAP → déterminer nb streams, nb codec slots
4. Init CORB : CORBSIZE, CORBLBASE/CORBUBASE, CORBWP, CORBRP
5. Init RIRB : RIRBSIZE, RIRBLBASE/RIRBUBASE, RINTCNT
6. Enumérer codecs (verbes GET_PARAMETER sur adresses 0..15)
7. Pour le premier codec valide :
   a. Discover widgets (AFG : Audio Function Group)
   b. Trouver Output DAC + Pin Complex → Line Out ou HP Out
   c. Configurer path : DAC → Mixer → Pin
   d. SET_POWER_STATE D0 sur tous les widgets du path
   e. SET_AMPLIFIER_GAIN sur Pin (décommute)
8. Init Output Stream SDn :
   SDnCBL = buffer taille totale
   SDnLVI = nombre BDL entries - 1
   SDnFMT = 0x0011 (44100Hz, stereo, 16-bit)
   BDL entries → phys addresses des buffers PCM
   SDnCTL.SRST = 1 → reset stream → SRST = 0
   SDnCTL.RUN = 1
```

### 5.3 — virtio-sound (VM/QEMU)

```
virtio_sound/src/
├── lib.rs       → négociation virtio, features
├── virtqueue.rs → control_vq + event_vq + tx_vq + rx_vq
├── stream.rs    → PCM_SET_PARAMS, PCM_PREPARE, PCM_START
└── xfer.rs      → pcm_xfer descriptors cycle
```

**Séquence init virtio-sound :**
```
1. Négociation features : VIRTIO_SND_F_CTLS (bits 0..7)
2. Créer 4 virtqueues : control (0), event (1), tx (2), rx (3)
3. PCM_QUERY_INFO → obtenir stream capabilities
4. PCM_SET_PARAMS sur stream 0 :
   format = VIRTIO_SND_PCM_FMT_S16
   rate   = VIRTIO_SND_PCM_RATE_44100
   channels = 2
5. PCM_PREPARE → préparer le stream
6. PCM_START → démarrer
```

**Envoi PCM :**
```
// Pour chaque chunk de données PCM à jouer :
// 1. Ajouter descriptor dans tx_vq :
//    [0] : pcm_xfer header {stream_id, period_bytes}
//    [1] : buffer PCM (read-only pour le device)
//    [2] : pcm_status (writable par device, status retour)
// 2. Kick tx_vq
// 3. Sur completion (event_vq notify) : recycler les descriptors
```

### 5.4 — audio_server Ring1 (Vague 4)

```rust
// audio_server/src/main.rs
//
// Sons embarqués (PCM statique, compilés dans le binaire) :
static SOUND_BOOT_COMPLETE: &[u8] = include_bytes!("../sounds/boot_complete.raw");
static SOUND_SECURITY_ALERT: &[u8] = include_bytes!("../sounds/security_alert.raw");
// Format : PCM 44100Hz 16-bit LE stereo interleaved

// IPC Protocol audio_server :
pub enum AudioRequest {
    PlaySystemSound(SoundId),          // BOOT_COMPLETE, SECURITY_ALERT
    Beep { freq_hz: u32, dur_ms: u32 },// PC speaker style → synthèse à la volée
    Stop,                              // arrêt immédiat
}

// Synthèse Beep (pas besoin de PCM embarqué) :
fn synthesize_beep(freq_hz: u32, dur_ms: u32) -> Vec<i16> {
    let samples = 44100 * dur_ms / 1000;
    (0..samples).map(|i| {
        let t = i as f32 / 44100.0;
        (f32::sin(2.0 * PI * freq_hz as f32 * t) * 16000.0) as i16
    }).collect()
}
```

---

## 6. Clock Driver

**Fichiers :** `drivers/clock/src/`

```
clock/src/
├── lib.rs   → init, sélection source (HPET > RTC)
├── hpet.rs  → HPET : init MMIO, timer 0 périodique
├── rtc.rs   → RTC CMOS : lecture heure/date au boot
└── ipc.rs   → interface vers scheduler_server
```

**RTC :** Lecture au boot uniquement pour initialiser l'epoch système.
```
RTC CMOS ports : 0x70 (index) / 0x71 (data)
Registres : seconds (0x00), minutes (0x02), hours (0x04),
            day (0x07), month (0x08), year (0x09)
```

**HPET :**
```
ACPI table HPET → base address
GCAP_ID : période (en femtosecondes), nb timers
GEN_CONF : ENABLE_CNF = 1, LEG_RT_CNF = 0
MAIN_CNT : compteur hardware running (64-bit)
Timer 0 : mode périodique, interruption toutes les 1ms
```

---

## 7. Driver Framework + Manager

**Fichiers :** `drivers/framework/src/`, `drivers/manager/src/`

### 7.1 — Framework

```rust
// Traits communs pour tous les drivers
pub trait BlockDevice: Send + Sync {
    fn read_blocks(&self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), DriveError>;
    fn write_blocks(&self, lba: u64, data: &[u8]) -> Result<(), DriveError>;
    fn block_count(&self) -> u64;
    fn block_size(&self) -> u32;
    fn flush(&self) -> Result<(), DriveError>;
}

pub trait InputDevice: Send + Sync {
    fn poll_event(&self) -> Option<InputEvent>;
    fn set_led(&self, led: LedFlags);
}

pub trait AudioDevice: Send + Sync {
    fn play_pcm(&self, data: &[i16], rate: u32, channels: u8) -> Result<(), AudioError>;
    fn stop(&self);
}

pub trait NetDevice: Send + Sync {
    fn send(&self, frame: &[u8]) -> Result<(), NetError>;
    fn recv(&self, buf: &mut [u8]) -> Result<usize, NetError>;
    fn mac_addr(&self) -> [u8; 6];
}
```

### 7.2 — Manager (Hot-plug)

```rust
// Event bus hot-plug
pub enum DeviceEvent {
    Attached { device_id: u32, class: DeviceClass, info: DeviceInfo },
    Detached { device_id: u32 },
}

// device_manager publie les événements vers device_server via IPC
// device_server → vfs_server (pour mount/umount automatique)
// device_server → exo_shield (pour scan automatique USB)
```

---

## 8. Règles Architecturales Rappelées

Ces règles s'appliquent à **tous** les drivers :

- **DRV-ARCH-01** : Zéro logique Ring0 dans les drivers. Tout driver s'exécute en Ring1 (device_server) ou Ring3 (compat). Le kernel ne contient que les primitives d'accès hardware (IRQ_REGISTER, DMA_ALLOC, PCI_CLAIM).
- **FIX-108** : Les ISR ne font jamais de yield ni d'allocation. EOI toujours émis même sur erreur.
- **FIX-109** : Résultat ISR → IPC SpscRing non-bloquant vers le serveur Ring1.
- **FIX-104** : `IommuFaultQueue` utilise CAS-strong pour l'insertion.
- **Syscalls drivers** (arch/constants.rs) : `IRQ_REGISTER=530`, `DMA_ALLOC=534`, `PCI_CLAIM=540`, `PCI_SET_TOPOLOGY=546`.
- **do_exit() driver** : 7 étapes obligatoires : `bus_master_disable → quiescence → revoke_DMA → revoke_alloc → revoke_MMIO → revoke_IRQ → revoke_claims`.

---

## 9. Tests Requis

```
# AHCI
ahci_test::detect_sata_controller             PASS
ahci_test::read_first_sector                  PASS
ahci_test::write_read_roundtrip               PASS

# NVMe
nvme_test::detect_nvme_controller             PASS
nvme_test::identify_controller                PASS
nvme_test::identify_namespace                 PASS
nvme_test::read_first_block                   PASS
nvme_test::write_read_roundtrip               PASS

# USB
usb_test::xhci_init                           PASS
usb_test::enumerate_keyboard                  PASS
usb_test::enumerate_mass_storage              PASS
usb_test::mass_storage_read_first_sector      PASS
usb_test::hid_keyboard_event_received         PASS

# Audio
audio_test::hda_init                          PASS
audio_test::virtio_sound_init                 PASS
audio_test::play_boot_chime                   PASS
audio_test::synthesize_beep_800hz             PASS

# Clock
clock_test::rtc_read_valid_datetime           PASS
clock_test::hpet_counter_running              PASS
clock_test::hpet_1ms_interrupt                PASS
```

---

*claude-alpha — ExoOS v0.2.0 — Strata — SPEC-DRIVERS-STRATA.md*
