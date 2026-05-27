# SPEC-EXO-DRIVERS-V0.2 — Drivers ExoOS v0.2.0
## Nouveaux Drivers, Architecture Ring1, Règles DRV-ARCH

**Auteur :** claude-alpha  
**Date :** 2026-05-14  
**Statut :** SPEC OFFICIELLE v0.2.0

---

## 1. Principes Architecturaux des Drivers ExoOS

### Règle DRV-ARCH-01 (Absolue)
**Zéro logique de driver en Ring0.**

Ring0 ne connaît que :
- La découverte PCI (scan du bus, lecture config space)
- L'allocation de ressources initiales (BARs, IRQ vectors)
- Le claim de device (`SYS_PCI_CLAIM=540`)
- Le routage des interruptions vers les ISRs Ring1

Toute la logique de protocole (NVMe commands, virtio rings, e1000e descriptors) est en Ring1 dans `device_server` ou dans un serveur dédié.

### Règle DRV-ISR-01 (Absolue)
**Les ISR ne font que trois choses :**
1. Lire le registre de statut pour acquitter l'interruption (EOI)
2. Poser un flag atomique (AtomicBool) ou écrire dans un canal wake
3. Retourner

Pas d'allocation, pas de yield, pas d'IPC complexe dans une ISR.

```rust
// Patron ISR correct (Ring1)
extern "x86-interrupt" fn virtio_net_isr(frame: InterruptStackFrame) {
    // 1. Lire statut et acquitter
    let status = VIRTIO_NET_ISR_STATUS.load();
    
    // 2. Signaler au driver thread
    if status & VIRTIO_NET_ISR_QUEUE != 0 {
        VIRTIO_RX_PENDING.store(true, Ordering::Release);
        wake_driver_thread();  // simple IPI ou futex wake
    }
    
    // 3. EOI — TOUJOURS en dernier
    unsafe { LAPIC.end_of_interrupt(); }
    // PAS de return Err, PAS de panic!, PAS d'alloc
}
```

### Règle DRV-DMA-01
**Tout accès DMA passe par `SYS_DMA_ALLOC=534`.**

Ce syscall retourne un tuple `(VirtAddr, IoVirtAddr)` :
- `VirtAddr` : adresse virtuelle du buffer (utilisable par le driver)
- `IoVirtAddr` : adresse IOMMU du buffer (à écrire dans les registres DMA du device)

L'`IoVirtAddr` ne doit jamais être utilisée autrement que dans les registres DMA. Elle ne correspond pas à une adresse physique directe — c'est une adresse dans le domaine IOMMU du device.

---

## 2. Drivers Prioritaires v0.2.0

### 2.1 Inventaire des Drivers Requis

| Driver | Type | Ring | Statut actuel | Cible v0.2.0 |
|--------|------|------|--------------|--------------|
| `virtio-net` | Réseau | R1 | Partiel | Complet + IOMMU |
| `virtio-blk` | Stockage | R1 | Partiel | Complet + IOMMU |
| `virtio-console` | Console | R1 | Stub | Fonctionnel |
| `ps2-keyboard` | HID | R1 | Fonctionnel | Stable |
| `ps2-mouse` | HID | R1 | Partiel | Fonctionnel |
| `fb-gop` | Affichage | R1 | Fonctionnel | Stable + modes |
| `nvme` | Stockage | R1 | Absent | MVP (bare-metal) |
| `ahci` | Stockage | R1 | Absent | MVP (bare-metal) |
| `e1000e` | Réseau | R1 | Absent | MVP (bare-metal) |
| `rtl8139` | Réseau | R1 | Absent | MVP (bare-metal) |
| `xhci` | USB | R1 | Absent | Post-v0.2.0 |
| `intel-hda` | Audio | R1 | Absent | Post-v0.2.0 |

---

### 2.2 virtio-net — Complet v0.2.0

```rust
// device_server/src/virtio/net.rs (Ring1)

pub struct VirtioNet {
    // Queues virtio
    rx_queue:    VirtQueue,  // Receive (device → driver)
    tx_queue:    VirtQueue,  // Transmit (driver → device)
    ctrl_queue:  Option<VirtQueue>,  // Control (optionnel)

    // DMA buffers (VirtAddr + IoVirtAddr via SYS_DMA_ALLOC)
    rx_bufs: Vec<DmaBuf>,  // MAX_RX_BUFS = 256
    tx_bufs: Vec<DmaBuf>,  // MAX_TX_BUFS = 256

    // État
    mac_addr:   [u8; 6],
    link_up:    bool,
    stats:      NetStats,
}

pub struct DmaBuf {
    virt:  VirtAddr,    // Pour la CPU
    iova:  IoVirtAddr,  // Pour le device (via IOMMU)
    size:  usize,
}

impl VirtioNet {
    pub fn init(pci: PciDevice) -> Result<Self, DriverError> {
        // 1. Reset + Ack + Driver bits dans device status
        pci.set_device_status(VIRTIO_STATUS_RESET);
        pci.set_device_status(VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER);

        // 2. Négociation des features
        let host_features = pci.read_device_features();
        let driver_features = host_features
            & (VIRTIO_NET_F_MAC | VIRTIO_NET_F_STATUS | VIRTIO_NET_F_CSUM);
        pci.write_driver_features(driver_features);

        // 3. Lire MAC address
        let mac = pci.read_mac_address();

        // 4. Allouer les queues virtio
        let rx_queue = VirtQueue::new(pci.clone(), 0, RX_QUEUE_SIZE)?;
        let tx_queue = VirtQueue::new(pci.clone(), 1, TX_QUEUE_SIZE)?;

        // 5. Allouer les DMA buffers via SYS_DMA_ALLOC
        let rx_bufs: Vec<DmaBuf> = (0..MAX_RX_BUFS).map(|_| {
            let (virt, iova) = sys_dma_alloc(MTU + VIRTIO_NET_HDR_SIZE)?;
            Ok(DmaBuf { virt, iova, size: MTU + VIRTIO_NET_HDR_SIZE })
        }).collect::<Result<_, _>>()?;

        // 6. Pré-remplir la RX queue avec les buffers
        for buf in &rx_bufs {
            rx_queue.add_buffer(buf.iova, buf.size, VirtqueueFlags::WRITE)?;
        }

        // 7. Finaliser l'init
        pci.set_device_status(VIRTIO_STATUS_DRIVER_OK);

        Ok(VirtioNet { rx_queue, tx_queue, mac_addr: mac, rx_bufs, .. })
    }

    /// Recevoir un paquet (appelé depuis le driver thread après ISR signal)
    pub fn receive(&mut self) -> Option<&[u8]> {
        if !VIRTIO_RX_PENDING.swap(false, Ordering::AcqRel) {
            return None;
        }

        // Vider la used ring de la RX queue
        while let Some((idx, len)) = self.rx_queue.pop_used() {
            let buf = &self.rx_bufs[idx as usize];
            let data = unsafe {
                core::slice::from_raw_parts(buf.virt.as_ptr(), len as usize)
            };
            // Transmettre à smoltcp via le device trait
            return Some(&data[VIRTIO_NET_HDR_SIZE..]);
        }
        None
    }

    /// Envoyer un paquet
    pub fn transmit(&mut self, packet: &[u8]) -> Result<(), DriverError> {
        // Trouver un TX buffer libre
        let buf = self.tx_bufs.iter_mut()
            .find(|b| !b.in_use)
            .ok_or(DriverError::TxRingFull)?;

        // Copier le paquet dans le DMA buffer (côté CPU)
        let total = VIRTIO_NET_HDR_SIZE + packet.len();
        let dst = unsafe { core::slice::from_raw_parts_mut(buf.virt.as_ptr(), total) };
        dst[..VIRTIO_NET_HDR_SIZE].fill(0);  // header virtio-net (GSO disabled)
        dst[VIRTIO_NET_HDR_SIZE..].copy_from_slice(packet);
        buf.in_use = true;

        // Soumettre à la TX queue avec l'IoVirtAddr (pour l'IOMMU)
        self.tx_queue.add_buffer(buf.iova, total, VirtqueueFlags::READ)?;
        self.tx_queue.notify();
        Ok(())
    }
}
```

**Intégration smoltcp :**
```rust
// Implémenter le trait Device de smoltcp pour virtio-net
impl smoltcp::phy::Device for VirtioNetDevice {
    type RxToken<'a> = VirtioRxToken<'a>;
    type TxToken<'a> = VirtioTxToken<'a>;

    fn receive(&mut self, _ts: smoltcp::time::Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        if let Some(data) = self.driver.receive() {
            Some((VirtioRxToken(data), VirtioTxToken(&mut self.driver)))
        } else { None }
    }

    fn transmit(&mut self, _ts: smoltcp::time::Instant) -> Option<Self::TxToken<'_>> {
        Some(VirtioTxToken(&mut self.driver))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = 1514;
        caps.max_burst_size = Some(64);
        caps
    }
}
```

---

### 2.3 NVMe — MVP v0.2.0 (Bare-metal)

Le driver NVMe est nécessaire pour ExoOS bare-metal (sans virtio). En v0.2.0 : MVP fonctionnel (admin queue + IO queues, read/write, pas de namespace management avancé).

```rust
// device_server/src/nvme/driver.rs (Ring1)

pub struct NvmeDriver {
    regs:       &'static mut NvmeRegisters,  // MMIO registers
    admin_q:    NvmeQueue,                   // Admin Submission + Completion
    io_queues:  Vec<NvmeQueue>,              // IO Submission + Completion (1 par CPU)
    ns_list:    Vec<NvmeNamespace>,          // Namespaces détectés
}

pub struct NvmeQueue {
    sq:         DmaBuf,   // Submission Queue (host → device)
    cq:         DmaBuf,   // Completion Queue (device → host)
    sq_tail:    u32,
    cq_head:    u32,
    sq_db:      *mut u32, // Doorbell register
    cq_db:      *mut u32,
    q_depth:    u16,
}

impl NvmeDriver {
    pub fn read_blocks(&mut self, ns_id: u32, lba: u64, count: u16, buf: DmaBuf)
        -> Result<(), NvmeError>
    {
        // Construire une commande NVMe Read
        let cmd = NvmeReadCmd {
            opc:     NvmeOpc::Read as u8,
            nsid:    ns_id,
            slba:    lba,
            nlb:     count - 1,  // NVMe: 0-based count
            prp1:    buf.iova.as_u64(),
            prp2:    0,           // PRP list si > 4K (non géré en MVP)
            ..Default::default()
        };
        self.submit_and_wait(0, cmd)  // queue 0 = IO queue 0
    }

    pub fn write_blocks(&mut self, ns_id: u32, lba: u64, count: u16, buf: DmaBuf)
        -> Result<(), NvmeError>
    {
        let cmd = NvmeWriteCmd {
            opc:  NvmeOpc::Write as u8,
            nsid: ns_id,
            slba: lba,
            nlb:  count - 1,
            prp1: buf.iova.as_u64(),
            ..Default::default()
        };
        self.submit_and_wait(0, cmd)
    }

    fn submit_and_wait(&mut self, qid: usize, cmd: impl AsNvmeCmd)
        -> Result<(), NvmeError>
    {
        let q = &mut self.io_queues[qid];

        // Écrire la commande dans la SQ
        let sq_slot = &mut q.sq.as_mut_slice::<NvmeCmd>()[q.sq_tail as usize];
        *sq_slot = cmd.as_nvme_cmd();
        q.sq_tail = (q.sq_tail + 1) % q.q_depth as u32;

        // Notifier le device via doorbell
        unsafe { q.sq_db.write_volatile(q.sq_tail); }

        // Polling de la CQ (IRQ-based en production, polling en MVP)
        loop {
            let cqe = &q.cq.as_slice::<NvmeCqEntry>()[q.cq_head as usize];
            if cqe.phase_bit() == q.expected_phase {
                let status = cqe.status >> 1;
                if status != 0 {
                    return Err(NvmeError::CommandFailed(status));
                }
                q.cq_head = (q.cq_head + 1) % q.q_depth as u32;
                unsafe { q.cq_db.write_volatile(q.cq_head); }
                return Ok(());
            }
            core::hint::spin_loop();
        }
    }
}
```

---

### 2.4 AHCI — MVP v0.2.0 (SATA)

```rust
// device_server/src/ahci/driver.rs (Ring1) — structure minimale

pub struct AhciDriver {
    hba:    &'static mut AhciHba,   // HBA Memory-Mapped registers
    ports:  [Option<AhciPort>; 32], // Max 32 ports AHCI
}

pub struct AhciPort {
    port:      &'static mut AhciPortRegs,
    cmd_list:  DmaBuf,  // 32 * CommandHeader = 1024 bytes
    fis_buf:   DmaBuf,  // FIS Receive Area = 256 bytes
    cmd_table: DmaBuf,  // Command Table (PRDT entries)
}

impl AhciPort {
    pub fn read_dma(&mut self, lba: u64, count: u16, buf: DmaBuf)
        -> Result<(), AhciError>
    {
        // Construire un Command FIS (H2D Register FIS)
        let fis = FisRegH2D {
            fis_type: FisType::RegH2D as u8,
            c:        1,  // Command register
            command:  ATA_CMD_READ_DMA_EXT,
            lba_lo:   lba as u32,
            lba_hi:   (lba >> 32) as u16,
            count:    count,
            ..Default::default()
        };

        // Construire le PRDT dans la Command Table
        let prdt = PrdtEntry {
            dba:  buf.iova.as_u64(),
            dbc:  (count as u32 * 512) - 1,  // byte count - 1
            ..Default::default()
        };

        self.submit_command(fis, &[prdt])
    }
}
```

---

### 2.5 e1000e — Réseau Intel (Bare-metal)

```rust
// device_server/src/e1000e/driver.rs (Ring1) — structure

pub struct E1000eDriver {
    regs:    *mut u8,        // MMIO base
    rx_ring: Vec<E1000eRxDesc>,
    tx_ring: Vec<E1000eTxDesc>,
    rx_bufs: Vec<DmaBuf>,
    tx_bufs: Vec<DmaBuf>,
    mac:     [u8; 6],
}

impl E1000eDriver {
    pub fn init(pci: PciDevice) -> Result<Self, DriverError> {
        let base = pci.map_bar(0)?;

        // Reset global
        let ctrl = unsafe { read_mmio::<u32>(base, E1000E_CTRL) };
        unsafe { write_mmio(base, E1000E_CTRL, ctrl | E1000E_CTRL_RST); }
        exo_udelay(1000);

        // Lire MAC address depuis EEPROM
        let mac = read_mac_from_eeprom(base);

        // Configurer RX et TX descriptors + DMA
        let rx_ring_dma = sys_dma_alloc(RX_RING_SIZE * core::mem::size_of::<E1000eRxDesc>())?;
        let tx_ring_dma = sys_dma_alloc(TX_RING_SIZE * core::mem::size_of::<E1000eTxDesc>())?;
        // ... setup RDBA, TDBA, RDLEN, TDLEN, RDH, RDT, TDH, TDT ...
        // ... configure RCTL, TCTL ...

        Ok(E1000eDriver { regs: base, mac, .. })
    }
}
```

---

### 2.6 rtl8139 — Réseau Realtek (QEMU default)

Driver simple ciblant la carte réseau par défaut de QEMU sans virtio.

```rust
// device_server/src/rtl8139/driver.rs (Ring1)
// RTL8139 : registre-based, pas de descriptor ring
// RX : ring buffer linéaire de 8/16/32/64 KiB

pub struct Rtl8139Driver {
    iobase:  u16,          // I/O port base
    rx_buf:  DmaBuf,       // Ring buffer RX (8 KiB + 16 overhead)
    rx_ptr:  u16,          // Pointeur de lecture courant
    mac:     [u8; 6],
}

impl Rtl8139Driver {
    pub fn receive(&mut self) -> Option<Vec<u8>> {
        let capr = unsafe { inw(self.iobase + RTL8139_CAPR) };
        let cbr  = unsafe { inw(self.iobase + RTL8139_CBR) };

        if capr == cbr { return None; }  // Buffer vide

        let rx_buf = self.rx_buf.virt.as_slice::<u8>(RX_BUF_SIZE);
        let offset = (capr + 16) % RX_BUF_SIZE as u16;

        // Header RTL8139 : status (u16) + length (u16)
        let rx_status = u16::from_le_bytes([rx_buf[offset as usize], rx_buf[offset as usize + 1]]);
        let rx_len    = u16::from_le_bytes([rx_buf[offset as usize + 2], rx_buf[offset as usize + 3]]);

        if rx_status & RTL8139_ROK == 0 { return None; }

        let data_offset = (offset + 4) as usize;
        let packet = rx_buf[data_offset..data_offset + (rx_len - 4) as usize].to_vec();

        // Avancer le pointeur CAPR
        self.rx_ptr = (capr + 4 + rx_len + 3) & !3;
        unsafe { outw(self.iobase + RTL8139_CAPR, self.rx_ptr - 16); }

        Some(packet)
    }
}
```

---

## 3. Catalogue Complet des Drivers v0.2.0

| Driver | Fichier | Statut cible | Testable sur |
|--------|---------|--------------|-------------|
| `virtio-net` | `device_server/src/virtio/net.rs` | ✅ Complet | QEMU |
| `virtio-blk` | `device_server/src/virtio/blk.rs` | ✅ Complet | QEMU |
| `virtio-console` | `device_server/src/virtio/console.rs` | ✅ Fonctionnel | QEMU |
| `ps2-keyboard` | `input_server/src/ps2/keyboard.rs` | ✅ Stable | QEMU + Bare-metal |
| `ps2-mouse` | `input_server/src/ps2/mouse.rs` | ✅ Fonctionnel | QEMU + Bare-metal |
| `fb-gop` | `fb_server/src/gop.rs` | ✅ Stable | QEMU OVMF + Bare-metal |
| `rtl8139` | `device_server/src/rtl8139/driver.rs` | ⚠️ MVP | QEMU (-net rtl8139) |
| `e1000e` | `device_server/src/e1000e/driver.rs` | ⚠️ MVP | QEMU + Bare-metal Intel |
| `nvme` | `device_server/src/nvme/driver.rs` | ⚠️ MVP | QEMU (-drive if=none,format=qcow2) |
| `ahci` | `device_server/src/ahci/driver.rs` | ⚠️ MVP | QEMU SATA + Bare-metal |
| `xhci` | — | ❌ Post-v0.2.0 | — |
| `intel-hda` | — | ❌ Post-v0.2.0 | — |

---

## 4. Processus d'Ajout d'un Nouveau Driver

1. **Claim PCI** : `SYS_PCI_CLAIM=540` avec Vendor/Device ID → reçoit un `DeviceHandle`
2. **Map BAR** : `SYS_PCI_MAP_BAR` → reçoit une `VirtAddr` pour les registres MMIO
3. **Allouer DMA** : `SYS_DMA_ALLOC=534` → reçoit `(VirtAddr, IoVirtAddr)` par buffer
4. **Enregistrer ISR** : `SYS_IRQ_REGISTER=530` avec le vecteur d'interruption
5. **Implémenter le trait** correspondant (`NetworkDevice`, `BlockDevice`, `InputDevice`)
6. **Enregistrer dans `device_server`** : ajouter au `DeviceRegistry`
7. **Écrire les tests** : `cargo test --test drivers_integration` sur QEMU

---

## 5. IommuFaultQueue — Monitoring des Fautes DMA

Tout access DMA hors de la plage autorisée est capturé par l'IOMMU et mis dans la `IommuFaultQueue` :

```rust
// kernel/src/security/iommu/fault.rs

pub struct IommuFaultQueue {
    entries: AtomicQueue<IommuFaultEntry, 64>,  // CAS-strong, lock-free
}

pub struct IommuFaultEntry {
    pub device_id:  PciDeviceId,
    pub iova:       u64,          // Adresse IOMMU illégale tentée
    pub access_type: DmaAccessType, // Read ou Write
    pub timestamp:  u64,
    pub domain_id:  u32,
}

// Handler de faute IOMMU (appelé par le kernel lors d'une faute VT-d)
pub fn iommu_fault_handler(fault: IommuFaultEntry) {
    // 1. Stopper le DMA du device fautif
    iommu_disable_domain(fault.domain_id);

    // 2. Enregistrer dans la queue pour device_server
    IOMMU_FAULT_QUEUE.push(fault);

    // 3. Logger (via klog, pas IPC depuis un handler)
    klog!(ERROR, "IOMMU fault: device={:?} iova={:#x} type={:?}",
          fault.device_id, fault.iova, fault.access_type);

    // 4. Notifier device_server via un flag atomique
    IOMMU_FAULT_PENDING.store(true, Ordering::Release);
}
```

---

*claude-alpha — ExoOS v0.2.0 — SPEC-EXO-DRIVERS-V0.2.md*
