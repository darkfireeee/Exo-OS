# 🎯 PLAN D'INTÉGRATION RÉELLE - Exo-OS v1.0

**Date**: 2026-01-02  
**Objectif**: Passer de 45% fonctionnel à 80-90% fonctionnel ET testé  
**Durée**: 8-10 semaines méthodiques  
**Philosophie**: **FONCTIONNEL > COMPILABLE**

---

## 📋 PRINCIPES DIRECTEURS

### Règles d'Intégration

1. **Un module à la fois** - Pas de multi-tasking
2. **Tests AVANT passage suivant** - Validation systématique
3. **Éliminer TODO au fur et à mesure** - Pas d'accumulation
4. **Code fonctionnel > Code élégant** - Pragmatisme
5. **Documentation état réel** - Honnêteté

### Critères de Validation Module

✅ **Module VALIDÉ** si:
- Aucun TODO/FIXME restant
- Tests passent en QEMU
- Intégration avec modules adjacents
- Documentation à jour
- Benchmark si pertinent

---

## 🗓️ TIMELINE DÉTAILLÉE

### 📅 SEMAINE 1: Memory Foundation (Jours 1-7)

#### Jour 1-2: CoW Manager
**Objectif**: fork() avec Copy-on-Write fonctionnel

**Fichiers à modifier**:
```
kernel/src/memory/
├── cow_manager.rs        CRÉER
├── page_table.rs         MODIFIER (mark_cow)
└── allocator.rs          MODIFIER (clone_with_cow)

kernel/src/syscall/handlers/process.rs
└── sys_fork()            MODIFIER (use CoW)
```

**Implémentation**:
```rust
// kernel/src/memory/cow_manager.rs
pub struct CowManager {
    refcounts: HashMap<PhysAddr, AtomicU32>,
}

impl CowManager {
    pub fn mark_cow(&mut self, page: PhysAddr) {
        // Increment refcount
        // Mark page table entry as read-only
    }
    
    pub fn handle_cow_fault(&mut self, addr: VirtAddr) -> Result<()> {
        // Check if COW page
        // If refcount == 1: make writable
        // If refcount > 1: copy page, decrement refcount
    }
}
```

**Tests**:
```rust
#[test_case]
fn test_cow_fork() {
    let parent_data = [1, 2, 3, 4];
    let pid = fork();
    
    if pid == 0 {
        // Child: modify data
        parent_data[0] = 99;
        assert_eq!(parent_data[0], 99);
    } else {
        // Parent: verify unchanged
        wait(pid);
        assert_eq!(parent_data[0], 1);
    }
}
```

**Critères validation**:
- [ ] fork() duplique address space
- [ ] Child writes trigger CoW
- [ ] Parent data unchanged
- [ ] Refcount correct
- [ ] Tests QEMU passent

---

#### Jour 3-4: exec() VFS Loading
**Objectif**: exec() charge ELF depuis VFS

**Fichiers à modifier**:
```
kernel/src/loader/
├── elf.rs                MODIFIER (load_from_vfs)
└── spawn.rs              MODIFIER (exec integration)

kernel/src/syscall/handlers/process.rs
└── sys_execve()          COMPLÉTER
```

**Implémentation**:
```rust
// kernel/src/loader/elf.rs
pub fn load_elf_from_vfs(path: &str) -> Result<LoadedElf> {
    // 1. Ouvrir fichier via VFS
    let fd = vfs::open(path, O_RDONLY)?;
    
    // 2. Lire header
    let mut header = [0u8; 64];
    vfs::read(fd, &mut header)?;
    
    // 3. Valider ELF
    validate_elf_header(&header)?;
    
    // 4. Lire program headers
    let phdrs = read_program_headers(fd, &header)?;
    
    // 5. Mapper segments
    for phdr in phdrs {
        if phdr.p_type == PT_LOAD {
            map_segment(fd, &phdr)?;
        }
    }
    
    // 6. Setup stack
    let stack = setup_user_stack(argv, envp)?;
    
    Ok(LoadedElf { entry, stack })
}
```

**Tests**:
```rust
#[test_case]
fn test_exec_hello() {
    // Créer fichier /tmp/hello (ELF simple)
    create_test_elf("/tmp/hello");
    
    let pid = fork();
    if pid == 0 {
        execve("/tmp/hello", &["hello"], &[]);
        panic!("exec failed");
    }
    
    let status = wait(pid);
    assert_eq!(status, 0);
}
```

**Critères validation**:
- [ ] exec() lit ELF depuis VFS
- [ ] Segments mappés correctement
- [ ] Stack setup avec argv/envp
- [ ] Entry point correct
- [ ] Tests QEMU passent

---

#### Jour 5-6: Process Cleanup
**Objectif**: exit() nettoie ressources

**Fichiers à modifier**:
```
kernel/src/scheduler/
├── thread.rs             MODIFIER (cleanup method)
└── process.rs            MODIFIER (exit logic)

kernel/src/posix_x/core/
└── fd_table.rs           MODIFIER (close_all)
```

**Implémentation**:
```rust
// kernel/src/scheduler/thread.rs
impl Thread {
    pub fn cleanup(&mut self) {
        // 1. Close all FDs
        self.fd_table.close_all();
        
        // 2. Free memory
        self.address_space.destroy();
        
        // 3. Remove from parent's children
        if let Some(parent) = self.parent {
            parent.remove_child(self.tid);
        }
        
        // 4. Wake waiting parent
        if let Some(waiter) = self.waiter {
            scheduler::wake(waiter);
        }
        
        // 5. Free kernel stack
        free_kernel_stack(self.kernel_stack);
    }
}
```

**Tests**:
```rust
#[test_case]
fn test_process_cleanup() {
    let pid = fork();
    if pid == 0 {
        // Child: open file, allocate memory
        let fd = open("/tmp/test", O_RDWR);
        let mem = mmap(NULL, 4096, PROT_RW);
        exit(42);
    }
    
    let status = wait(pid);
    assert_eq!(status, 42);
    
    // Verify no leaks
    assert_no_fd_leak();
    assert_no_memory_leak();
}
```

**Critères validation**:
- [ ] exit() ferme tous FDs
- [ ] exit() free memory
- [ ] exit() notifie parent
- [ ] Pas de leaks
- [ ] Tests QEMU passent

---

#### Jour 7: Signal Delivery
**Objectif**: Signals délivrés réellement

**Fichiers à modifier**:
```
kernel/src/scheduler/
└── signals_stub.rs       RENOMMER → signals.rs (supprimer stub)

kernel/src/syscall/handlers/signals.rs
└── Remplacer tous stubs
```

**Implémentation**:
```rust
// kernel/src/scheduler/signals.rs
pub fn deliver_signal(pid: Pid, signal: Signal) -> Result<()> {
    let process = PROCESS_TABLE.get(pid)?;
    
    // 1. Add to signal queue
    process.signal_queue.push(signal);
    
    // 2. If process sleeping, wake it
    if process.state == ProcessState::Sleeping {
        scheduler::wake(process.tid);
    }
    
    // 3. Set pending flag
    process.signal_pending = true;
    
    Ok(())
}

pub fn check_signals() {
    let current = current_process();
    
    while let Some(signal) = current.signal_queue.pop() {
        if let Some(handler) = current.signal_handlers.get(signal) {
            // Setup signal frame et call handler
            setup_signal_frame(handler, signal);
        } else {
            // Default action
            default_signal_action(signal);
        }
    }
}
```

**Tests**:
```rust
#[test_case]
fn test_signal_delivery() {
    let received = AtomicBool::new(false);
    
    signal(SIGINT, || {
        received.store(true, Ordering::Release);
    });
    
    kill(getpid(), SIGINT);
    
    // Wait for delivery
    sleep(10);
    
    assert!(received.load(Ordering::Acquire));
}
```

**Critères validation**:
- [ ] kill() envoie signal
- [ ] Handler appelé
- [ ] Signal frame correct
- [ ] Return from signal works
- [ ] Tests QEMU passent

---

### 📅 SEMAINE 2: VFS & Filesystems (Jours 8-14)

#### Jour 8-9: FAT32 Integration
**Objectif**: FAT32 lecture/écriture fonctionnelle

**Fichiers à modifier**:
```
kernel/src/fs/real_fs/fat32/
├── mod.rs                VÉRIFIER (supprimer TODOs)
├── file.rs               TESTER
└── write.rs              TESTER

kernel/src/fs/vfs/
└── mount.rs              MODIFIER (register FAT32)
```

**Tests**:
```rust
#[test_case]
fn test_fat32_io() {
    // Mount FAT32 partition
    mount("/dev/vda1", "/mnt", "fat32");
    
    // Create file
    let fd = open("/mnt/test.txt", O_CREAT | O_RDWR);
    
    // Write
    let data = b"Hello FAT32!";
    write(fd, data);
    
    // Read back
    lseek(fd, 0, SEEK_SET);
    let mut buf = [0u8; 32];
    read(fd, &mut buf);
    
    assert_eq!(&buf[..12], data);
    
    // Delete
    close(fd);
    unlink("/mnt/test.txt");
}
```

**Critères validation**:
- [ ] mount FAT32 works
- [ ] create file works
- [ ] write file works
- [ ] read file works
- [ ] delete file works
- [ ] Tests QEMU passent

---

#### Jour 10-11: ext4 Integration
**Objectif**: ext4 lecture fonctionnelle

**Fichiers à modifier**:
```
kernel/src/fs/real_fs/ext4/
├── mod.rs                VÉRIFIER
├── inode.rs              TESTER
└── extent.rs             TESTER
```

**Tests**:
```rust
#[test_case]
fn test_ext4_read() {
    // Mount ext4 (read-only pour l'instant)
    mount("/dev/vda2", "/ext4", "ext4", MS_RDONLY);
    
    // Read existing file
    let fd = open("/ext4/test.txt", O_RDONLY);
    let mut buf = [0u8; 1024];
    let n = read(fd, &mut buf);
    
    assert!(n > 0);
    close(fd);
}
```

**Critères validation**:
- [ ] mount ext4 works (read-only)
- [ ] read file works
- [ ] readdir works
- [ ] stat works
- [ ] Tests QEMU passent

---

#### Jour 12-13: Page Cache Integration
**Objectif**: Page cache actif avec filesystems

**Fichiers à modifier**:
```
kernel/src/fs/page_cache.rs    CONNECTER aux FS
kernel/src/fs/real_fs/fat32/   UTILISER page cache
kernel/src/fs/real_fs/ext4/    UTILISER page cache
```

**Implémentation**:
```rust
// Modifier FAT32/ext4 pour utiliser page cache
impl Fat32Fs {
    fn read_cluster(&mut self, cluster: u32) -> &[u8] {
        // Check page cache first
        if let Some(page) = PAGE_CACHE.lookup(cluster as u64) {
            return page.data();
        }
        
        // Cache miss: read from device
        let page = self.device.read_cluster(cluster)?;
        PAGE_CACHE.insert(cluster as u64, page);
        page.data()
    }
}
```

**Tests**:
```rust
#[test_case]
fn test_page_cache_hit() {
    let fd = open("/mnt/test.txt", O_RDONLY);
    
    // First read (cache miss)
    let start1 = rdtsc();
    read(fd, &mut buf);
    let cycles1 = rdtsc() - start1;
    
    // Second read (cache hit)
    lseek(fd, 0, SEEK_SET);
    let start2 = rdtsc();
    read(fd, &mut buf);
    let cycles2 = rdtsc() - start2;
    
    // Cache hit should be faster
    assert!(cycles2 < cycles1 / 2);
    assert!(cycles2 < 200); // Target < 200 cycles
}
```

**Critères validation**:
- [ ] Page cache actif
- [ ] Cache hit < 200 cycles
- [ ] Cache miss functional
- [ ] Eviction works (CLOCK-Pro)
- [ ] Tests QEMU passent

---

#### Jour 14: Partition & Mount
**Objectif**: MBR/GPT parsing + mount/umount

**Fichiers à modifier**:
```
kernel/src/drivers/block/partition.rs    TESTER réel
kernel/src/fs/vfs/mount.rs               COMPLÉTER
```

**Tests**:
```rust
#[test_case]
fn test_partition_detection() {
    let device = open("/dev/vda", O_RDONLY);
    
    // Detect partition table type
    let pt_type = detect_partition_table(device);
    assert!(pt_type == PartitionTable::MBR || pt_type == PartitionTable::GPT);
    
    // Parse partitions
    let partitions = parse_partitions(device);
    assert!(partitions.len() > 0);
    
    // Verify first partition
    let p1 = &partitions[0];
    assert!(p1.size_sectors > 0);
}
```

**Critères validation**:
- [ ] MBR parsing works
- [ ] GPT parsing works
- [ ] mount() functional
- [ ] umount() functional
- [ ] Tests QEMU passent

---

### 📅 SEMAINE 3-4: Network Stack (Jours 15-28)

#### Jour 15-16: ARP & IP Layer
**Objectif**: ARP request/response fonctionnel

**Fichiers à modifier**:
```
kernel/src/net/arp.rs           COMPLÉTER (supprimer TODOs)
kernel/src/net/ipv4.rs          CONNECTER au device
kernel/src/drivers/net/         INTÉGRER transmission
```

**Implémentation**:
```rust
// kernel/src/net/arp.rs
pub fn arp_resolve(ip: Ipv4Addr) -> Result<MacAddr> {
    // Check ARP cache
    if let Some(mac) = ARP_CACHE.get(&ip) {
        return Ok(mac);
    }
    
    // Send ARP request
    let request = ArpPacket::request(ip);
    send_ethernet_frame(&request)?;
    
    // Wait for reply (with timeout)
    let reply = wait_arp_reply(ip, timeout_ms(100))?;
    
    // Cache result
    ARP_CACHE.insert(ip, reply.sender_mac);
    Ok(reply.sender_mac)
}
```

**Tests**:
```rust
#[test_case]
fn test_arp_resolution() {
    let gateway_ip = Ipv4Addr::new(10, 0, 2, 2);
    
    // Resolve gateway MAC
    let mac = arp_resolve(gateway_ip).expect("ARP failed");
    
    // Verify MAC is valid
    assert_ne!(mac, MacAddr::ZERO);
    
    // Check cache
    let cached = ARP_CACHE.get(&gateway_ip).unwrap();
    assert_eq!(cached, mac);
}
```

**Critères validation**:
- [ ] ARP request sent
- [ ] ARP reply received
- [ ] ARP cache works
- [ ] Timeout handled
- [ ] Tests QEMU passent

---

#### Jour 17-18: ICMP (ping)
**Objectif**: ping localhost et externe

**Fichiers à modifier**:
```
kernel/src/net/icmp.rs          COMPLÉTER
kernel/src/syscall/             AJOUTER sys_ping (ou via raw socket)
```

**Implémentation**:
```rust
// kernel/src/net/icmp.rs
pub fn ping(dest: Ipv4Addr, timeout_ms: u32) -> Result<PingReply> {
    // Create ICMP echo request
    let request = IcmpPacket::echo_request(seq, data);
    
    // Send via IP layer
    ip_send(dest, IpProto::ICMP, &request)?;
    
    // Wait for echo reply
    let start = tsc::read();
    let reply = wait_icmp_reply(seq, timeout_ms)?;
    let rtt_us = tsc::cycles_to_us(tsc::read() - start);
    
    Ok(PingReply {
        from: dest,
        seq,
        rtt_us,
        ttl: reply.ttl,
    })
}
```

**Tests**:
```rust
#[test_case]
fn test_ping_localhost() {
    let localhost = Ipv4Addr::new(127, 0, 0, 1);
    let reply = ping(localhost, 1000).expect("Ping failed");
    
    assert_eq!(reply.from, localhost);
    assert!(reply.rtt_us < 1000); // Should be fast
}

#[test_case]
fn test_ping_gateway() {
    let gateway = Ipv4Addr::new(10, 0, 2, 2);
    let reply = ping(gateway, 1000).expect("Ping failed");
    
    assert_eq!(reply.from, gateway);
    assert!(reply.rtt_us < 10_000); // < 10ms
}
```

**Critères validation**:
- [ ] ping localhost works
- [ ] ping gateway works
- [ ] RTT measured
- [ ] Timeout handled
- [ ] Tests QEMU passent

---

#### Jour 19-22: TCP Stack
**Objectif**: TCP handshake fonctionnel

**Fichiers à modifier**:
```
kernel/src/net/tcp.rs           COMPLÉTER (supprimer TODOs transmission)
kernel/src/net/socket.rs        CONNECTER au TCP
```

**Implémentation**:
```rust
// kernel/src/net/tcp.rs
impl TcpConnection {
    pub fn connect(&mut self, dest: SocketAddr) -> Result<()> {
        // 1. Send SYN
        self.send_syn()?;
        self.state = TcpState::SynSent;
        
        // 2. Wait for SYN-ACK
        let reply = self.wait_packet(timeout_ms(1000))?;
        if !reply.flags.contains(SYN | ACK) {
            return Err(TcpError::HandshakeFailed);
        }
        
        // 3. Send ACK
        self.send_ack(reply.seq + 1)?;
        self.state = TcpState::Established;
        
        Ok(())
    }
    
    fn send_syn(&mut self) -> Result<()> {
        let packet = TcpPacket {
            flags: SYN,
            seq: self.local_seq,
            ...
        };
        
        // CRITICAL: Actually send via IP layer (pas stub!)
        ip_send(self.dest_ip, IpProto::TCP, &packet.serialize())?;
        
        Ok(())
    }
}
```

**Tests**:
```rust
#[test_case]
fn test_tcp_handshake() {
    // Setup simple TCP echo server on port 7
    spawn_echo_server(7);
    
    // Connect
    let sock = socket(AF_INET, SOCK_STREAM);
    let addr = SocketAddr::new(Ipv4Addr::LOCALHOST, 7);
    connect(sock, &addr).expect("Connect failed");
    
    // Verify connection established
    let info = getsockopt(sock, SOL_TCP, TCP_INFO);
    assert_eq!(info.state, TcpState::Established);
    
    close(sock);
}
```

**Critères validation**:
- [ ] TCP SYN sent
- [ ] TCP SYN-ACK received
- [ ] TCP ACK sent
- [ ] Connection established
- [ ] Tests QEMU passent

---

#### Jour 23-24: UDP Stack
**Objectif**: UDP send/recv fonctionnel

**Fichiers à modifier**:
```
kernel/src/net/udp.rs           COMPLÉTER (supprimer TODOs)
```

**Tests**:
```rust
#[test_case]
fn test_udp_echo() {
    let sock = socket(AF_INET, SOCK_DGRAM);
    bind(sock, SocketAddr::new(Ipv4Addr::ANY, 9999));
    
    let dest = SocketAddr::new(Ipv4Addr::LOCALHOST, 9999);
    let msg = b"Hello UDP!";
    
    // Send
    sendto(sock, msg, &dest);
    
    // Receive
    let mut buf = [0u8; 1024];
    let (n, from) = recvfrom(sock, &mut buf);
    
    assert_eq!(n, msg.len());
    assert_eq!(&buf[..n], msg);
    assert_eq!(from.port(), 9999);
}
```

**Critères validation**:
- [ ] UDP send works
- [ ] UDP recv works
- [ ] Port binding works
- [ ] Checksum correct
- [ ] Tests QEMU passent

---

#### Jour 25-28: Socket API Complete
**Objectif**: Éliminer tous ENOSYS socket

**Fichiers à modifier**:
```
kernel/src/syscall/handlers/net_socket.rs    COMPLÉTER tous stubs
kernel/src/posix_x/syscalls/hybrid_path/socket.rs    SUPPRIMER ENOSYS
```

**Tests complets socket API**:
```rust
#[test_case]
fn test_socket_api_complete() {
    // socket()
    let sock = socket(AF_INET, SOCK_STREAM);
    assert!(sock >= 0);
    
    // bind()
    let addr = SocketAddr::new(Ipv4Addr::ANY, 8080);
    assert_eq!(bind(sock, &addr), 0);
    
    // listen()
    assert_eq!(listen(sock, 10), 0);
    
    // getsockname()
    let bound_addr = getsockname(sock);
    assert_eq!(bound_addr.port(), 8080);
    
    // setsockopt()
    assert_eq!(setsockopt(sock, SOL_SOCKET, SO_REUSEADDR, &1), 0);
    
    // close()
    assert_eq!(close(sock), 0);
}
```

**Critères validation**:
- [ ] Tous syscalls socket implémentés
- [ ] Aucun ENOSYS
- [ ] Tests API complets
- [ ] Integration avec VFS (/dev/socket)
- [ ] Tests QEMU passent

---

### 📅 SEMAINE 5-6: Drivers Real (Jours 29-42)

#### Jour 29-32: VirtIO-Net Real
**Objectif**: TX/RX paquets réels

**Fichiers à modifier**:
```
kernel/src/drivers/virtio/net.rs       COMPLÉTER (virt→phys, buffer tracking)
kernel/src/memory/                     AJOUTER virt_to_phys()
```

**Implémentation**:
```rust
// kernel/src/memory/translate.rs
pub fn virt_to_phys(virt: VirtAddr) -> Result<PhysAddr> {
    let page_table = current_page_table();
    
    // Walk page table
    let l4_entry = page_table.l4[virt.l4_index()];
    if !l4_entry.present() {
        return Err(MemoryError::NotMapped);
    }
    
    let l3_table = l4_entry.addr();
    let l3_entry = l3_table[virt.l3_index()];
    // ... continue walk
    
    let phys_frame = l1_entry.addr();
    let offset = virt.page_offset();
    Ok(phys_frame + offset)
}

// kernel/src/drivers/virtio/net.rs
impl VirtioNet {
    pub fn transmit(&mut self, packet: &[u8]) -> Result<()> {
        // Get physical address (REAL, not stub!)
        let virt = VirtAddr::from_ptr(packet.as_ptr());
        let phys = virt_to_phys(virt)?;
        
        // Add to TX virtqueue
        self.tx_queue.add_buffer(phys, packet.len())?;
        
        // Kick device
        self.tx_queue.notify();
        
        // Track buffer for later free
        self.pending_tx.insert(phys, virt);
        
        Ok(())
    }
}
```

**Tests**:
```rust
#[test_case]
fn test_virtio_net_tx_rx() {
    let net = VirtioNet::init().expect("VirtIO-Net init failed");
    
    // Send packet
    let packet = build_arp_request();
    net.transmit(&packet).expect("TX failed");
    
    // Wait for reply
    let reply = net.receive(timeout_ms(1000)).expect("RX failed");
    
    // Verify
    assert!(reply.len() > 0);
    assert_eq!(reply[12..14], [0x08, 0x06]); // ARP ethertype
}
```

**Critères validation**:
- [ ] virt_to_phys() works
- [ ] TX packet sent
- [ ] RX packet received
- [ ] Buffer tracking works
- [ ] No memory leaks
- [ ] Tests QEMU passent

---

#### Jour 33-36: VirtIO-Block Real
**Objectif**: Read/write secteurs réels

**Fichiers à modifier**:
```
kernel/src/drivers/virtio/block.rs     COMPLÉTER (virt→phys)
kernel/src/drivers/block/mod.rs        INTÉGRER au block layer
```

**Tests**:
```rust
#[test_case]
fn test_virtio_block_io() {
    let blk = VirtioBlock::init().expect("VirtIO-Block init failed");
    
    // Write sector
    let data = [0xABu8; 512];
    blk.write(0, &data).expect("Write failed");
    
    // Read back
    let mut buf = [0u8; 512];
    blk.read(0, &mut buf).expect("Read failed");
    
    // Verify
    assert_eq!(buf, data);
}
```

**Critères validation**:
- [ ] Read sector works
- [ ] Write sector works
- [ ] Async I/O works
- [ ] Integration avec filesystems
- [ ] Tests QEMU passent

---

#### Jour 37-39: Linux DRM Compat Integration
**Objectif**: Linux compat layer connecté

**Fichiers à modifier**:
```
kernel/src/drivers/compat/linux.rs     COMPLÉTER TODOs (kmalloc, IRQ)
kernel/src/memory/                     CONNECTER allocator
kernel/src/arch/x86_64/interrupts/     CONNECTER IRQ management
```

**Implémentation**:
```rust
// kernel/src/drivers/compat/linux.rs
pub unsafe extern "C" fn kmalloc(size: usize, flags: u32) -> *mut u8 {
    // REAL allocation (not stub!)
    let layout = Layout::from_size_align_unchecked(size, 8);
    alloc::alloc(layout)
}

pub unsafe extern "C" fn kfree(ptr: *mut u8) {
    // REAL deallocation
    if !ptr.is_null() {
        alloc::dealloc(ptr, Layout::from_size_align_unchecked(1, 1));
    }
}

pub unsafe extern "C" fn request_irq(
    irq: u32,
    handler: IrqHandler,
    flags: u32,
    name: *const c_char,
    dev: *mut c_void,
) -> c_int {
    // REAL IRQ registration
    let handler_fn = move |_frame| {
        handler(irq as i32, dev);
    };
    
    crate::arch::interrupts::register_handler(irq, handler_fn);
    0 // Success
}
```

**Critères validation**:
- [ ] kmalloc/kfree works
- [ ] request_irq works
- [ ] ioremap works
- [ ] No TODOs restants
- [ ] Tests integration

---

#### Jour 40-42: Block Device Integration
**Objectif**: Filesystems utilisent VirtIO-Block

**Tests end-to-end**:
```rust
#[test_case]
fn test_fs_on_virtio_block() {
    // Mount FAT32 sur VirtIO-Block
    mount("/dev/vda1", "/mnt", "fat32");
    
    // Write file
    let fd = open("/mnt/test.txt", O_CREAT | O_RDWR);
    write(fd, b"Test on VirtIO-Block");
    close(fd);
    
    // Unmount
    umount("/mnt");
    
    // Remount et verify
    mount("/dev/vda1", "/mnt", "fat32");
    let fd = open("/mnt/test.txt", O_RDONLY);
    let mut buf = [0u8; 64];
    let n = read(fd, &mut buf);
    assert_eq!(&buf[..20], b"Test on VirtIO-Block");
}
```

**Critères validation**:
- [ ] FAT32 sur VirtIO-Block
- [ ] ext4 sur VirtIO-Block
- [ ] I/O persistence
- [ ] Tests QEMU passent

---

### 📅 SEMAINE 7-8: IPC & Syscalls (Jours 43-56)

#### Jour 43-45: IPC Descriptor Table
**Objectif**: Handles dynamiques, pas hardcodés

**Fichiers à modifier**:
```
kernel/src/ipc/                CRÉER descriptor_table.rs
kernel/src/syscall/handlers/ipc.rs    COMPLÉTER (supprimer stubs)
```

**Implémentation**:
```rust
// kernel/src/ipc/descriptor_table.rs
pub struct IpcDescriptorTable {
    descriptors: HashMap<IpcHandle, Arc<IpcDescriptor>>,
    next_handle: AtomicU64,
}

impl IpcDescriptorTable {
    pub fn allocate(&mut self, ring: Arc<FusionRing>) -> IpcHandle {
        let handle = self.next_handle.fetch_add(1, Ordering::Relaxed);
        
        self.descriptors.insert(handle, Arc::new(IpcDescriptor {
            ring,
            permissions: IpcPermissions::SEND | IpcPermissions::RECV,
        }));
        
        handle
    }
    
    pub fn get(&self, handle: IpcHandle) -> Result<Arc<IpcDescriptor>> {
        self.descriptors.get(&handle)
            .cloned()
            .ok_or(IpcError::InvalidHandle)
    }
}
```

**Tests**:
```rust
#[test_case]
fn test_ipc_descriptor_alloc() {
    let table = IpcDescriptorTable::new();
    
    let handle1 = table.allocate(ring1);
    let handle2 = table.allocate(ring2);
    
    assert_ne!(handle1, handle2);
    assert!(table.get(handle1).is_ok());
    assert!(table.get(handle2).is_ok());
}
```

**Critères validation**:
- [ ] Allocation handles dynamique
- [ ] Lookup handles works
- [ ] Permissions checked
- [ ] No hardcoded handles
- [ ] Tests QEMU passent

---

#### Jour 46-48: FD Passing Protocol
**Objectif**: Envoyer FD via IPC

**Implémentation**:
```rust
// kernel/src/ipc/fd_passing.rs
pub fn send_fd(channel: IpcHandle, fd: Fd, target_pid: Pid) -> Result<()> {
    // 1. Get FD from current process
    let fd_entry = current_process().fd_table.get(fd)?;
    
    // 2. Create FD message
    let msg = IpcMessage::FdTransfer {
        source_pid: current_pid(),
        target_pid,
        fd_entry: fd_entry.clone(),
    };
    
    // 3. Send via IPC channel
    ipc_send(channel, &msg)?;
    
    Ok(())
}

pub fn recv_fd(channel: IpcHandle) -> Result<Fd> {
    // 1. Receive IPC message
    let msg = ipc_recv(channel)?;
    
    // 2. Verify it's FD transfer
    let IpcMessage::FdTransfer { fd_entry, .. } = msg else {
        return Err(IpcError::WrongMessageType);
    };
    
    // 3. Install FD in current process
    let new_fd = current_process().fd_table.install(fd_entry)?;
    
    Ok(new_fd)
}
```

**Tests**:
```rust
#[test_case]
fn test_fd_passing() {
    let (send_ch, recv_ch) = ipc_channel_create();
    
    let child = fork();
    if child == 0 {
        // Child: send FD
        let fd = open("/tmp/test.txt", O_RDONLY);
        send_fd(send_ch, fd, getppid());
        exit(0);
    }
    
    // Parent: receive FD
    let received_fd = recv_fd(recv_ch).expect("FD passing failed");
    
    // Verify FD works
    let mut buf = [0u8; 64];
    let n = read(received_fd, &mut buf);
    assert!(n > 0);
    
    wait(child);
}
```

**Critères validation**:
- [ ] send_fd() works
- [ ] recv_fd() works
- [ ] FD usable in target process
- [ ] Permissions preserved
- [ ] Tests QEMU passent

---

#### Jour 49-50: IPC Benchmark
**Objectif**: Mesurer et optimiser IPC

**Benchmark**:
```rust
#[bench]
fn bench_ipc_roundtrip() {
    let (send, recv) = ipc_channel_create();
    
    let iterations = 10000;
    let msg = [1u8; 40]; // Inline message
    
    let start = rdtsc();
    
    for _ in 0..iterations {
        ipc_send(send, &msg);
        ipc_recv(recv, &mut buf);
    }
    
    let elapsed = rdtsc() - start;
    let cycles_per_roundtrip = elapsed / iterations;
    
    println!("IPC roundtrip: {} cycles", cycles_per_roundtrip);
    assert!(cycles_per_roundtrip < 400); // Target
}
```

**Optimisations**:
1. Cache line alignment (64 bytes)
2. Prefetch instructions
3. Lock-free où possible
4. Memory barriers minimaux

**Critères validation**:
- [ ] IPC < 400 cycles
- [ ] Inline path < 350 cycles
- [ ] Zero-copy path measured
- [ ] No regressions
- [ ] Benchmark documented

---

#### Jour 51-56: Syscalls Cleanup
**Objectif**: Éliminer TOUS ENOSYS/stubs critiques

**Fichiers à traiter**:
```
kernel/src/syscall/handlers/
├── sched.rs          COMPLÉTER TODOs (yield, nice, setpriority)
├── fs_poll.rs        COMPLÉTER stubs (poll, select)
├── inotify.rs        IMPLÉMENTER (inotify_init, add_watch)
├── fs_futex.rs       COMPLÉTER robust futex
└── process_limits.rs IMPLÉMENTER getrlimit/setrlimit réels
```

**Checklist élimination**:
```
Jour 51: sched_yield, nice, setpriority/getpriority
Jour 52: poll, select, ppoll, pselect
Jour 53: inotify_init, inotify_add_watch, inotify_rm_watch
Jour 54: futex robustness, FUTEX_WAKE_BITSET
Jour 55: getrlimit, setrlimit (RLIMIT_*)
Jour 56: Validation finale + tests
```

**Critères validation**:
- [ ] < 10 ENOSYS restants
- [ ] < 20 TODOs restants
- [ ] Tous syscalls critiques implémentés
- [ ] Tests syscall passent
- [ ] Pas de panics

---

## 📊 VALIDATION FINALE

### Checklist v1.0 (Réaliste)

**Memory & Process**:
- [ ] fork() avec CoW fonctionnel
- [ ] exec() charge ELF depuis VFS
- [ ] exit() nettoie ressources
- [ ] Signal delivery works
- [ ] Tests fork+exec+wait+signal passent

**VFS & Filesystems**:
- [ ] FAT32 read/write testé
- [ ] ext4 read testé
- [ ] Page cache intégré (<200 cycles hit)
- [ ] Partition MBR+GPT parsing
- [ ] mount/umount fonctionnels

**Network**:
- [ ] ping localhost works
- [ ] ping gateway works
- [ ] TCP handshake works
- [ ] UDP echo works
- [ ] Socket API complet (no ENOSYS)

**Drivers**:
- [ ] VirtIO-Net TX/RX réels
- [ ] VirtIO-Block I/O réel
- [ ] virt_to_phys() works
- [ ] Linux compat kmalloc/IRQ

**IPC**:
- [ ] Descriptor table works
- [ ] FD passing works
- [ ] IPC < 400 cycles
- [ ] Benchmark validé

**Syscalls**:
- [ ] < 10 ENOSYS
- [ ] < 20 TODOs
- [ ] ~200+ syscalls fonctionnels
- [ ] Tests passent

---

## 🎯 MÉTRIQUES DE SUCCÈS

### Code Quality

```bash
# Avant (état actuel)
TODOs: 200+
ENOSYS: 30
Stubs: 150+
Tests passant: 50/67

# Après (objectif)
TODOs: < 20
ENOSYS: < 10
Stubs: < 10
Tests passant: 150+/150+
```

### Performance Targets

| Métrique | Linux | Exo-OS Target | Status Final |
|----------|-------|---------------|--------------|
| IPC Roundtrip | 1247 cycles | < 400 cycles | À mesurer |
| Page Cache Hit | ~150 cycles | < 200 cycles | À mesurer |
| Context Switch | 2134 cycles | < 800 cycles | À mesurer |
| Syscall Fast Path | ~150 cycles | < 100 cycles | À mesurer |

---

## 📝 DOCUMENTATION

### À Créer

1. **INTEGRATION_LOG.md** - Journal quotidien
2. **TEST_COVERAGE.md** - État tests
3. **BENCHMARK_RESULTS.md** - Résultats mesurés
4. **SYSCALL_STATUS.md** - État chaque syscall
5. **TODO_TRACKER.md** - Suivi élimination TODOs

### Format Log Quotidien

```markdown
## Jour X: [Module]

### Objectif
[Objectif du jour]

### Travail effectué
- [ ] Task 1
- [ ] Task 2

### Tests
- Test A: PASS/FAIL
- Test B: PASS/FAIL

### TODOs éliminés
- File.rs:123 - TODO description

### Problèmes rencontrés
[Description]

### Prochaines étapes
[Next steps]
```

---

## 🚀 CONCLUSION

### Engagement

✅ **8-10 semaines** de travail méthodique  
✅ **Un module à la fois**  
✅ **Tests AVANT passage suivant**  
✅ **Documentation continue**  
✅ **Élimination progressive TODOs**

### Objectif Final

Passer de **45% fonctionnel** à **80-90% fonctionnel ET testé**

**Exo-OS v1.0: Un OS réel qui fonctionne, pas juste qui compile** 🚀

---

**Signatures**:
- Date: 2026-01-02
- Plan: RÉALISTE et MÉTHODIQUE
- Philosophie: FONCTIONNEL > COMPILABLE
- Engagement: HONNÊTETÉ dans le reporting
