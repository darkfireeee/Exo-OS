# Phase 3: Drivers & Storage - Plan de Démarrage

**Date**: 2026-01-01  
**Phase précédente**: ✅ Phase 2c COMPLETE (100%)  
**Status**: 🚀 **DÉMARRAGE IMMÉDIAT**

---

## Clarification: Pas de Phase 2d

**Important**: Il n'existe pas de "Phase 2d" dans le ROADMAP officiel.

**Structure des phases**:
- ✅ **Phase 0**: Boot & Memory (COMPLETE)
- ✅ **Phase 1**: POSIX-X Foundation (COMPLETE)
- ✅ **Phase 2**: SMP + Scheduler (COMPLETE)
  - ✅ Phase 2a: SMP Foundation
  - ✅ **Phase 2b**: Scheduler Multi-core  
  - ✅ **Phase 2c**: Tests + Optimizations
- 🚀 **Phase 3**: **Drivers + Storage** ← **NOUS SOMMES ICI**
- ⏳ Phase 4: Advanced Features
- ⏳ Phase 5: AI Integration

**Prochaine étape** : Phase 3 (pas 2d)

---

## Phase 3 - Vue d'ensemble

### Objectifs Principaux

**Selon ROADMAP.md** :
1. **Drivers Linux-Compatible** (4 semaines)
   - Device driver framework
   - PCI device enumeration
   - Interrupts threaded handlers
   - Network drivers (virtio-net, e1000)

2. **Storage Stack** (4 semaines)
   - Block device layer
   - Disk drivers (AHCI SATA, virtio-blk)
   - File systems (ext4 read/write, FAT32 complete)
   - VFS integration

**Durée totale** : 8 semaines (~60-80h)

---

## Phase 3 Détaillée

### Partie 1: Device Driver Framework (Semaines 1-2, ~20h)

#### Semaine 1: Core Infrastructure ✅

**Objectifs**:
1. Device Model (inspiré Linux)
2. Driver Registration
3. Device-Driver Matching
4. Bus Subsystem (PCI, USB, Platform)

**Fichiers à créer**:
```
kernel/src/drivers/
├── mod.rs              // Driver framework core
├── device.rs           // struct Device + traits
├── driver.rs           // struct Driver + callbacks
├── bus.rs              // Bus abstraction
└── registry.rs         // Global device/driver registry
```

**API Essentielle**:
```rust
// Device trait
pub trait Device: Send + Sync {
    fn name(&self) -> &str;
    fn bus_type(&self) -> BusType;
    fn device_id(&self) -> DeviceId;
}

// Driver trait  
pub trait Driver: Send + Sync {
    fn name(&self) -> &str;
    fn probe(&self, dev: &dyn Device) -> Result<(), DriverError>;
    fn remove(&self, dev: &dyn Device);
}

// Registration
pub fn register_driver(driver: Arc<dyn Driver>) -> Result<(), DriverError>;
pub fn register_device(device: Arc<dyn Device>) -> Result<(), DriverError>;
```

**Tests (5)**:
1. `test_device_registration()` - Enregistrer device
2. `test_driver_registration()` - Enregistrer driver
3. `test_driver_probe()` - Match device→driver
4. `test_device_removal()` - Cleanup
5. `test_multiple_devices()` - Plusieurs devices même driver

**Livrable**: Framework de base compilable, tests PASS

---

#### Semaine 2: PCI Enumeration ✅

**Objectifs**:
1. PCI Configuration Space Access
2. Device Enumeration (Scan bus 0-255)
3. BAR (Base Address Register) Mapping
4. MSI/MSI-X Interrupt Setup

**Fichiers à créer**:
```
kernel/src/drivers/pci/
├── mod.rs              // PCI subsystem
├── config.rs           // Config space I/O (0xCF8/0xCFC)
├── device.rs           // PCIDevice struct
├── bar.rs              // BAR mapping (MMIO/I/O port)
└── msi.rs              // MSI/MSI-X interrupt setup
```

**Implémentation Critique**:
```rust
// PCI Config Space Access
pub fn read_config_u32(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    let address = 0x80000000 
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | (offset as u32);
    
    unsafe {
        outl(0xCF8, address);  // Address port
        inl(0xCFC)             // Data port
    }
}

// Device Enumeration
pub fn enumerate_devices() -> Vec<PCIDevice> {
    let mut devices = Vec::new();
    
    for bus in 0..256 {
        for dev in 0..32 {
            for func in 0..8 {
                let vendor = read_config_u16(bus, dev, func, 0);
                if vendor == 0xFFFF { continue; } // No device
                
                devices.push(PCIDevice::probe(bus, dev, func));
            }
        }
    }
    devices
}
```

**Devices à détecter** (QEMU/Bochs):
- Vendor 0x8086 (Intel): e1000 NIC, AHCI SATA
- Vendor 0x1AF4 (Red Hat): virtio-net, virtio-blk
- Vendor 0x1234 (Bochs): VGA graphics

**Tests (5)**:
1. `test_pci_config_read()` - Lire vendor ID
2. `test_pci_enumerate()` - Trouver >1 device
3. `test_pci_bar_mapping()` - Mapper BAR0
4. `test_pci_msi_setup()` - Configurer MSI
5. `test_pci_device_info()` - Class/subclass correct

**Livrable**: Enumération PCI fonctionnelle, devices détectés

---

### Partie 2: Network Drivers (Semaines 3-4, ~20h)

#### Semaine 3: virtio-net Driver ✅

**Pourquoi virtio-net en premier?**
- Supporté nativement par QEMU/Bochs
- Specification simple (virtio 1.0)
- Pas de quirks hardware
- Performance excellente (paravirtualization)

**Architecture**:
```
kernel/src/drivers/net/
├── mod.rs              // Network device trait
├── virtio_net.rs       // virtio-net driver
└── virtio/
    ├── mod.rs          // virtio framework
    ├── virtqueue.rs    // Virtqueue (ring buffer)
    └── device.rs       // virtio PCI device
```

**virtio-net Basics**:
```rust
pub struct VirtioNetDevice {
    base: PCIDevice,
    rx_queue: VirtQueue,    // Receive queue
    tx_queue: VirtQueue,    // Transmit queue
    mac_addr: [u8; 6],
}

impl VirtioNetDevice {
    // Send packet
    pub fn transmit(&mut self, packet: &[u8]) -> Result<(), NetError> {
        // 1. Get buffer from tx_queue
        // 2. Copy packet data
        // 3. Submit to device (kick)
        // 4. Wait for completion (interrupt)
    }
    
    // Receive packet (called from interrupt)
    pub fn receive(&mut self) -> Option<Vec<u8>> {
        // 1. Check rx_queue for completed buffers
        // 2. Copy packet data
        // 3. Return buffer to queue
    }
}
```

**Tests (5)**:
1. `test_virtio_probe()` - Detect virtio-net
2. `test_virtio_init()` - Initialize queues
3. `test_virtio_mac()` - Read MAC address
4. `test_virtio_send()` - Transmit packet
5. `test_virtio_receive()` - Receive packet (loopback)

**Livrable**: Ping localhost fonctionne

---

#### Semaine 4: e1000 Driver ✅

**Pourquoi e1000?**
- Hardware réel commun (Intel Gigabit)
- Documentation publique (datasheet 350 pages)
- Supporté par QEMU/Bochs
- Test de compatibilité hardware

**Implementation**:
```
kernel/src/drivers/net/e1000/
├── mod.rs              // E1000 driver entry
├── registers.rs        // MMIO register definitions
├── rx.rs               // Receive ring
├── tx.rs               // Transmit ring
└── eeprom.rs           // MAC address from EEPROM
```

**E1000 Registers** (MMIO):
```rust
const E1000_REG_CTRL: u32 = 0x0000;     // Device Control
const E1000_REG_STATUS: u32 = 0x0008;   // Device Status
const E1000_REG_RDBAL: u32 = 0x2800;    // RX Descriptor Base Low
const E1000_REG_TDBAL: u32 = 0x3800;    // TX Descriptor Base Low
const E1000_REG_RDH: u32 = 0x2810;      // RX Descriptor Head
const E1000_REG_TDH: u32 = 0x3810;      // TX Descriptor Head
```

**RX/TX Descriptors**:
```rust
#[repr(C)]
struct E1000RxDesc {
    addr: u64,       // Buffer physical address
    length: u16,
    checksum: u16,
    status: u8,
    errors: u8,
    special: u16,
}

#[repr(C)]
struct E1000TxDesc {
    addr: u64,       // Buffer physical address
    length: u16,
    cso: u8,
    cmd: u8,
    status: u8,
    css: u8,
    special: u16,
}
```

**Tests (5)**:
1. `test_e1000_probe()` - Detect via PCI
2. `test_e1000_reset()` - Device reset
3. `test_e1000_mac()` - Read from EEPROM
4. `test_e1000_rx_ring()` - Setup RX descriptors
5. `test_e1000_tx_ring()` - Transmit packet

**Livrable**: e1000 envoie/reçoit packets

---

### Partie 3: Storage Stack (Semaines 5-6, ~20h)

#### Semaine 5: Block Device Layer ✅

**Objectifs**:
1. Block Device Abstraction
2. Request Queue (I/O scheduling)
3. Buffer Cache
4. Partition Table Parsing (MBR, GPT)

**Architecture**:
```
kernel/src/block/
├── mod.rs              // Block layer core
├── device.rs           // BlockDevice trait
├── request.rs          // Bio (block I/O request)
├── queue.rs            // Request queue + elevator
├── cache.rs            // Buffer cache (sector level)
└── partition.rs        // MBR/GPT parsing
```

**BlockDevice Trait**:
```rust
pub trait BlockDevice: Send + Sync {
    fn name(&self) -> &str;
    fn sector_size(&self) -> usize;
    fn total_sectors(&self) -> u64;
    
    fn read_sectors(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError>;
    fn write_sectors(&self, lba: u64, buf: &[u8]) -> Result<(), BlockError>;
    fn flush(&self) -> Result<(), BlockError>;
}
```

**Request Queue** (elevator algorithm):
```rust
pub struct RequestQueue {
    pending: VecDeque<BioRequest>,
    current_lba: u64,
}

impl RequestQueue {
    // C-LOOK elevator (sweep one direction)
    pub fn schedule(&mut self) -> Option<BioRequest> {
        self.pending.sort_by_key(|r| r.lba);
        
        // Find request >= current position
        let idx = self.pending.iter()
            .position(|r| r.lba >= self.current_lba)
            .unwrap_or(0);
        
        self.pending.remove(idx)
    }
}
```

**Tests (5)**:
1. `test_block_device_register()` - Register block device
2. `test_read_sectors()` - Read 1 sector
3. `test_write_sectors()` - Write + verify
4. `test_request_queue()` - Elevator scheduling
5. `test_partition_parse()` - Read MBR/GPT

**Livrable**: Block layer abstraction fonctionnel

---

#### Semaine 6: AHCI SATA Driver ✅

**Pourquoi AHCI?**
- Standard moderne (remplace IDE)
- Supporté par tous les PCs récents
- QEMU/Bochs support
- Hot-plug, NCQ (Native Command Queuing)

**Architecture**:
```
kernel/src/drivers/block/ahci/
├── mod.rs              // AHCI driver
├── hba.rs              // Host Bus Adapter
├── port.rs             // SATA port
├── fis.rs              // Frame Information Structure
└── command.rs          // ATA commands
```

**AHCI HBA Structure**:
```rust
#[repr(C)]
struct HBAMemory {
    cap: u32,           // Host Capabilities
    ghc: u32,           // Global Host Control
    is: u32,            // Interrupt Status
    pi: u32,            // Ports Implemented
    vs: u32,            // Version
    // ... 32 ports
    ports: [HBAPort; 32],
}

#[repr(C)]
struct HBAPort {
    clb: u64,           // Command List Base
    fb: u64,            // FIS Base
    is: u32,            // Interrupt Status
    ie: u32,            // Interrupt Enable
    cmd: u32,           // Command & Status
    tfd: u32,           // Task File Data
    sig: u32,           // Signature (device type)
    ssts: u32,          // SATA Status
    // ...
}
```

**Read/Write Implementation**:
```rust
impl AHCIPort {
    pub fn read(&mut self, lba: u64, count: u16, buf: &mut [u8]) -> Result<(), AHCIError> {
        // 1. Build command FIS (ATA READ_DMA_EXT)
        let fis = build_read_fis(lba, count);
        
        // 2. Setup command table
        let cmd_slot = self.alloc_command_slot();
        cmd_slot.set_fis(&fis);
        cmd_slot.set_prdt(buf);  // Physical Region Descriptor Table
        
        // 3. Issue command
        self.issue_command(cmd_slot);
        
        // 4. Wait for completion (interrupt or poll)
        self.wait_completion()?;
        
        Ok(())
    }
}
```

**Tests (5)**:
1. `test_ahci_probe()` - Detect AHCI controller
2. `test_ahci_port_detect()` - Find SATA devices
3. `test_ahci_identify()` - IDENTIFY DEVICE command
4. `test_ahci_read()` - Read sector 0
5. `test_ahci_write()` - Write + verify

**Livrable**: Lecture/écriture disque SATA

---

### Partie 4: File Systems (Semaines 7-8, ~20h)

#### Semaine 7: ext4 Read Support ✅

**Objectifs**:
1. Superblock parsing
2. Group descriptor table
3. Inode lookup
4. Extent tree navigation
5. Directory iteration

**Architecture**:
```
kernel/src/fs/ext4/
├── mod.rs              // ext4 driver
├── superblock.rs       // Superblock + features
├── inode.rs            // Inode structure
├── extent.rs           // Extent tree
├── dir.rs              // Directory iteration
└── read.rs             // File read operations
```

**Superblock**:
```rust
#[repr(C)]
struct Ext4Superblock {
    s_inodes_count: u32,
    s_blocks_count: u64,
    s_r_blocks_count: u64,
    s_free_blocks_count: u64,
    s_free_inodes_count: u32,
    s_first_data_block: u32,
    s_log_block_size: u32,
    // ... 100+ fields
}

impl Ext4Superblock {
    pub fn block_size(&self) -> usize {
        1024 << self.s_log_block_size
    }
    
    pub fn has_feature(&self, feature: u32) -> bool {
        (self.s_feature_compat & feature) != 0
    }
}
```

**Inode Read**:
```rust
pub fn read_inode(fs: &Ext4FileSystem, ino: u32) -> Result<Ext4Inode, Ext4Error> {
    // 1. Calculate block group
    let group = (ino - 1) / fs.sb.s_inodes_per_group;
    
    // 2. Read group descriptor
    let desc = fs.read_group_desc(group)?;
    
    // 3. Calculate inode table offset
    let local_index = (ino - 1) % fs.sb.s_inodes_per_group;
    let inode_offset = desc.bg_inode_table * block_size + local_index * inode_size;
    
    // 4. Read inode
    fs.read_block(inode_offset)
}
```

**Tests (5)**:
1. `test_ext4_mount()` - Parse superblock
2. `test_ext4_read_inode()` - Read root inode (ino 2)
3. `test_ext4_read_dir()` - List / directory
4. `test_ext4_lookup()` - Find file by path
5. `test_ext4_read_file()` - Read file content

**Livrable**: Montage ext4 + lecture fichiers

---

#### Semaine 8: ext4 Write Support ✅

**Objectifs**:
1. Block allocation (from block bitmap)
2. Inode allocation
3. Extent insertion
4. Directory entry creation
5. Write transactions (journal)

**Write Operations**:
```rust
impl Ext4FileSystem {
    pub fn write_file(&mut self, ino: u32, offset: u64, data: &[u8]) -> Result<usize, Ext4Error> {
        let mut inode = self.read_inode(ino)?;
        
        // 1. Calculate block range
        let start_block = offset / self.block_size();
        let end_block = (offset + data.len() as u64) / self.block_size();
        
        // 2. Allocate missing blocks
        for block_num in start_block..=end_block {
            if !inode.has_block(block_num) {
                let phys_block = self.alloc_block()?;
                inode.add_extent(block_num, phys_block)?;
            }
        }
        
        // 3. Write data
        self.write_blocks(inode.extent_map(), offset, data)?;
        
        // 4. Update inode size
        inode.i_size = max(inode.i_size, offset + data.len() as u64);
        self.write_inode(ino, &inode)?;
        
        Ok(data.len())
    }
    
    fn alloc_block(&mut self) -> Result<u64, Ext4Error> {
        // 1. Find group with free blocks
        for group in 0..self.groups_count {
            let desc = self.read_group_desc(group)?;
            if desc.bg_free_blocks_count > 0 {
                // 2. Read block bitmap
                let bitmap = self.read_block_bitmap(group)?;
                
                // 3. Find first free bit
                if let Some(bit) = bitmap.find_first_zero() {
                    bitmap.set_bit(bit);
                    self.write_block_bitmap(group, &bitmap)?;
                    
                    let block_num = group * blocks_per_group + bit;
                    return Ok(block_num);
                }
            }
        }
        Err(Ext4Error::NoSpace)
    }
}
```

**Tests (5)**:
1. `test_ext4_create_file()` - Create new file
2. `test_ext4_write_file()` - Write data
3. `test_ext4_append()` - Append to file
4. `test_ext4_truncate()` - Reduce file size
5. `test_ext4_delete()` - Delete file

**Livrable**: ext4 read/write complet

---

## Timeline & Milestones

### Semaine 1-2: Driver Framework + PCI
**Milestone**: PCI enumeration fonctionne, devices détectés

**Critères de succès**:
- ✅ Framework compile
- ✅ PCI config space accessible
- ✅ >3 devices détectés (VGA, NIC, AHCI)
- ✅ 10/10 tests PASS

---

### Semaine 3-4: Network Drivers
**Milestone**: Ping localhost via virtio-net ET e1000

**Critères de succès**:
- ✅ virtio-net envoie/reçoit packets
- ✅ e1000 envoie/reçoit packets
- ✅ ARP resolution fonctionne
- ✅ ICMP echo reply reçu
- ✅ 10/10 tests PASS

---

### Semaine 5-6: Block Layer + AHCI
**Milestone**: Read/write sectors sur disque SATA

**Critères de succès**:
- ✅ Block device abstraction
- ✅ AHCI controller init
- ✅ SATA device detected
- ✅ Read sector 0 (MBR)
- ✅ Write + verify sector
- ✅ 10/10 tests PASS

---

### Semaine 7-8: ext4 File System
**Milestone**: Mount ext4, read/write files

**Critères de succès**:
- ✅ Mount ext4 partition
- ✅ List root directory
- ✅ Read file content
- ✅ Create new file
- ✅ Write + verify content
- ✅ Delete file
- ✅ 10/10 tests PASS

---

## Tests Complets Phase 3

### Total: 40 tests

**Driver Framework** (10 tests):
- Device/Driver registration (5)
- PCI enumeration (5)

**Network** (10 tests):
- virtio-net (5)
- e1000 (5)

**Block** (10 tests):
- Block layer (5)
- AHCI (5)

**File System** (10 tests):
- ext4 read (5)
- ext4 write (5)

---

## Métriques de Succès Phase 3

### Performance
- **Network throughput**: >100 Mbps (virtio-net)
- **Disk read**: >50 MB/s (AHCI SATA)
- **Disk write**: >30 MB/s (journaling overhead)
- **File open latency**: <1ms (cached inode)

### Stability
- **Zero kernel panics** sur 1000 I/O operations
- **No memory leaks** (valgrind clean)
- **Graceful errors** (pas de silent corruption)

### Coverage
- **40/40 tests PASS** (100%)
- **0 compilation errors**
- **<50 warnings** (down from 178)

---

## Dépendances & Prérequis

### Matériel de Test
- **QEMU**: `-device virtio-net -device e1000 -device ahci`
- **Bochs**: Support PCI + AHCI
- **Disque virtuel**: 1GB ext4 formatted

### Outils Nécessaires
```bash
# Create test disk image
dd if=/dev/zero of=disk.img bs=1M count=1024
mkfs.ext4 disk.img

# QEMU avec devices
qemu-system-x86_64 \
    -kernel kernel.bin \
    -drive file=disk.img,format=raw \
    -device virtio-net \
    -device e1000 \
    -device ahci \
    -smp 4
```

---

## Risques & Mitigation

### Risque 1: PCI Enumeration Fails
**Mitigation**: Test avec QEMU d'abord (devices garantis)

### Risque 2: virtio-net Complexe
**Mitigation**: Virtqueue simplifié (1.0 spec), pas de legacy

### Risque 3: AHCI SATA Quirks
**Mitigation**: Tester avec QEMU, pas hardware physique initial

### Risque 4: ext4 Corruption
**Mitigation**: Read-only d'abord, write avec journal transactions

---

## Prochaine Action Immédiate

### 🚀 Commencer Phase 3 Week 1

**Tâche 1** (30 min): Créer structure drivers/
```bash
mkdir -p kernel/src/drivers/{pci,net,block}
touch kernel/src/drivers/{mod.rs,device.rs,driver.rs,bus.rs}
```

**Tâche 2** (1h): Implémenter Device trait
```rust
// kernel/src/drivers/device.rs
pub trait Device: Send + Sync {
    fn name(&self) -> &str;
    fn bus_type(&self) -> BusType;
    fn device_id(&self) -> DeviceId;
}
```

**Tâche 3** (1h): PCI config space I/O
```rust
// kernel/src/drivers/pci/config.rs
pub fn read_config_u32(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    // Implementation...
}
```

**Tâche 4** (30 min): Premier test
```rust
#[test]
fn test_pci_config_read() {
    let vendor = pci::read_config_u16(0, 0, 0, 0);
    assert_ne!(vendor, 0xFFFF, "No device at 0:0.0");
}
```

---

## Conclusion

Phase 3 démarre **MAINTENANT** avec:
- ✅ Phase 2c COMPLETE (100%)
- ✅ Roadmap claire (8 semaines)
- ✅ Tests définis (40 total)
- ✅ Milestones précis

**Première semaine**: Driver framework + PCI enumeration

**Let's go! 🚀**
