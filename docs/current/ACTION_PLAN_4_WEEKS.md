# 🎯 PLAN D'ACTION - Exo-OS Complétion Réelle

**Date:** 4 février 2026  
**Durée:** 4 semaines (28 jours)  
**Objectif:** Passer de 35% → 80% fonctionnel avec code RÉEL  
**Philosophie:** Zéro stub, zéro fake success, code production uniquement

---

## 📋 SEMAINE 1: PHASE 1 COMPLÉTION (Jours 1-7)

### 🎯 Objectif: Phase 1 de 45% → 80%

---

### JOUR 1: exec() VFS Loading - Partie 1

**Temps estimé:** 8-10h  
**Priorité:** P0 - CRITIQUE (Bloquant pour tout le reste)

#### Tâches

**Matin (4h)**
1. ✅ Lire PREP_JOUR_4-5_EXEC_VFS.md complet
2. ✅ Analyser kernel/src/loader/elf.rs (430 lignes)
3. ✅ Analyser kernel/src/fs/vfs/ structure
4. ✅ Comprendre VFS::open() et VFS::read()

**Après-midi (4-6h)**
1. Implémenter `load_elf_from_vfs(path: &str)` dans loader/elf.rs
   ```rust
   pub fn load_elf_from_vfs(path: &str) -> Result<LoadedElf, ElfError> {
       // 1. VFS::open(path) → file handle
       // 2. VFS::read(handle, 0, 52) → ELF header
       // 3. Validate magic 0x7F 'E' 'L' 'F'
       // 4. Read program headers
       // 5. Return parsed ELF
   }
   ```

2. Créer structure `LoadedElf`
   ```rust
   pub struct LoadedElf {
       pub entry: u64,
       pub segments: Vec<ElfSegment>,
       pub interp: Option<String>,
   }
   ```

**Validation:**
- [ ] Compile sans erreurs
- [ ] Open file via VFS réussit
- [ ] Headers parsés correctement
- [ ] Git commit: "exec: Implement VFS loading (part 1)"

**Bloqueurs potentiels:**
- VFS::read() pas implémenté → Implémenter stub minimal
- File handle management → Utiliser FD table simple

---

### JOUR 2: exec() VFS Loading - Partie 2

**Temps estimé:** 8-10h

#### Tâches

**Matin (4h)**
1. Implémenter segment mapping dans sys_execve()
   ```rust
   pub fn sys_execve(path: &str, argv: &[&str], envp: &[&str]) 
       -> Result<!, ExecError> {
       
       let elf = load_elf_from_vfs(path)?;
       
       // Map PT_LOAD segments
       for seg in elf.segments {
           let pages = allocate_pages(seg.mem_size);
           map_pages(seg.vaddr, pages, seg.flags);
           copy_from_file(pages, seg.file_offset, seg.file_size);
       }
       
       // Setup stack
       let stack = setup_user_stack(argv, envp)?;
       
       // Jump to entry
       jump_to_userspace(elf.entry, stack);
   }
   ```

2. Implémenter `setup_user_stack(argv, envp)`
   ```rust
   fn setup_user_stack(argv: &[&str], envp: &[&str]) 
       -> Result<u64, ExecError> {
       
       // Stack layout (x86_64 System V ABI):
       // [envp strings]
       // [argv strings]  
       // [envp pointers] NULL
       // [argv pointers] NULL
       // [argc]
       // <-- stack pointer
       
       let stack_base = 0x7fff_ffff_f000;
       // ... push strings and pointers
       Ok(stack_ptr)
   }
   ```

**Après-midi (4-6h)**
1. Créer test binaire simple
   ```c
   // userland/test_exec_vfs.c
   void _start() {
       write(1, "Loaded from VFS!\n", 17);
       exit(0);
   }
   ```

2. Compiler avec musl
   ```bash
   musl-gcc -static -nostdlib test_exec_vfs.c -o test_exec_vfs.elf
   ```

3. Tester dans QEMU
   ```rust
   #[test]
   fn test_exec_vfs_real() {
       // VFS mount tmpfs
       mount("tmpfs", "/tmp", "tmpfs");
       
       // Copy binary to /tmp/test
       copy_to_vfs("/tmp/test", test_exec_vfs_data);
       
       // Exec
       let pid = fork();
       if pid == 0 {
           execve("/tmp/test", &[], &[]); // Should print "Loaded from VFS!"
       }
       wait4(pid);
   }
   ```

**Validation:**
- [ ] Binary chargé depuis VFS (pas hardcodé)
- [ ] Segments mappés avec bonnes permissions
- [ ] Stack setup correct (argc/argv/envp)
- [ ] Programme userspace s'exécute
- [ ] Git commit: "exec: Complete VFS loading + tests"

---

### JOUR 3: FD Table → VFS Connection

**Temps estimé:** 6-8h  
**Priorité:** P0

#### Tâches

**Matin (3-4h)**
1. Créer global FD table dans Process
   ```rust
   // kernel/src/process/fd_table.rs
   pub struct FdTable {
       entries: BTreeMap<i32, FdEntry>,
       next_fd: i32,
   }
   
   impl FdTable {
       pub fn allocate_fd(&mut self, handle_id: u64) -> i32 {
           let fd = self.next_fd;
           self.next_fd += 1;
           self.entries.insert(fd, FdEntry::new(handle_id));
           fd
       }
       
       pub fn get_handle(&self, fd: i32) -> Option<u64> {
           self.entries.get(&fd).map(|e| e.handle_id)
       }
   }
   ```

2. Connecter sys_open() au VFS
   ```rust
   // kernel/src/syscall/handlers/io.rs
   pub fn sys_open(path: *const u8, flags: i32, mode: u32) -> isize {
       let path_str = unsafe { CStr::from_ptr(path as *const i8) }
           .to_str().unwrap();
       
       // VFS open (RÉEL maintenant)
       let handle_id = match vfs::open(path_str, flags, mode) {
           Ok(h) => h,
           Err(e) => return -1, // ENOENT
       };
       
       // Allocate FD
       let fd = CURRENT_PROCESS.fd_table.lock().allocate_fd(handle_id);
       fd as isize
   }
   ```

**Après-midi (3-4h)**
1. Connecter sys_read() au VFS
   ```rust
   pub fn sys_read(fd: i32, buf: *mut u8, count: usize) -> isize {
       // Get VFS handle from FD
       let handle_id = match CURRENT_PROCESS.fd_table.lock().get_handle(fd) {
           Some(h) => h,
           None => return -1, // EBADF
       };
       
       // VFS read (RÉEL)
       let buffer = unsafe { core::slice::from_raw_parts_mut(buf, count) };
       match vfs::read(handle_id, buffer) {
           Ok(n) => n as isize,
           Err(_) => -1,
       }
   }
   ```

2. Connecter sys_write() au VFS
   ```rust
   pub fn sys_write(fd: i32, buf: *const u8, count: usize) -> isize {
       let handle_id = match CURRENT_PROCESS.fd_table.lock().get_handle(fd) {
           Some(h) => h,
           None => return -1,
       };
       
       let buffer = unsafe { core::slice::from_raw_parts(buf, count) };
       match vfs::write(handle_id, buffer) {
           Ok(n) => n as isize,
           Err(_) => -1,
       }
   }
   ```

**Tests:**
```c
// test_fd_vfs.c
void _start() {
    // Test /dev/null
    int fd = open("/dev/null", O_WRONLY, 0);
    write(fd, "absorbed", 8); // Should be absorbed
    close(fd);
    
    // Test /dev/zero
    fd = open("/dev/zero", O_RDONLY, 0);
    char buf[16];
    read(fd, buf, 16); // Should be all zeros
    close(fd);
    
    // Test tmpfs
    fd = open("/tmp/test.txt", O_CREAT | O_WRONLY, 0644);
    write(fd, "Hello VFS", 9);
    close(fd);
    
    fd = open("/tmp/test.txt", O_RDONLY, 0);
    char buf2[16];
    int n = read(fd, buf2, 16);
    // buf2 should contain "Hello VFS"
    close(fd);
    
    exit(0);
}
```

**Validation:**
- [ ] open() retourne FD valide (pas stub)
- [ ] read(/dev/zero) → buffer rempli de 0x00
- [ ] write(/dev/null) → absorbe données
- [ ] tmpfs write+read → data correcte
- [ ] Git commit: "io: Connect FD table to VFS"

---

### JOUR 4: Scheduler Syscalls Réels

**Temps estimé:** 6-8h  
**Priorité:** P1

#### Tâches

**Matin (3-4h)**
1. Implémenter sys_sched_yield() RÉEL
   ```rust
   // kernel/src/syscall/handlers/sched.rs
   pub fn sys_sched_yield() -> isize {
       use crate::scheduler::SCHEDULER;
       
       // OLD: return 0; // Stub
       // NEW: Call scheduler
       SCHEDULER.lock().yield_cpu();
       
       0
   }
   ```

2. Implémenter sys_nice() RÉEL
   ```rust
   pub fn sys_nice(increment: i32) -> isize {
       let current_tid = crate::scheduler::current_thread_id();
       
       SCHEDULER.lock().adjust_priority(current_tid, increment);
       
       0
   }
   ```

3. Implémenter sys_sched_setscheduler()
   ```rust
   pub fn sys_sched_setscheduler(pid: Pid, policy: i32, param: *const SchedParam) -> isize {
       use crate::scheduler::SchedPolicy;
       
       let policy_enum = match policy {
           0 => SchedPolicy::Other,
           1 => SchedPolicy::FIFO,
           2 => SchedPolicy::RR,
           _ => return -1, // EINVAL
       };
       
       SCHEDULER.lock().set_policy(pid, policy_enum);
       0
   }
   ```

**Après-midi (3-4h)**
1. Ajouter méthodes au Scheduler
   ```rust
   // kernel/src/scheduler/mod.rs
   impl Scheduler {
       pub fn yield_cpu(&mut self) {
           // Force context switch
           self.preempt_current();
           self.schedule_next();
       }
       
       pub fn adjust_priority(&mut self, tid: u64, increment: i32) {
           if let Some(thread) = self.threads.get_mut(&tid) {
               thread.priority = thread.priority.saturating_add(increment);
               // Requeue if needed
               self.requeue_thread(tid);
           }
       }
       
       pub fn set_policy(&mut self, pid: Pid, policy: SchedPolicy) {
           // Set scheduling policy for process
           // ...
       }
   }
   ```

2. Tests
   ```c
   // test_sched_real.c
   void _start() {
       // Test yield
       int tid_before = gettid();
       sched_yield(); // Should switch thread
       int tid_after = gettid();
       // tid_before != tid_after (if other threads exist)
       
       // Test nice
       nice(-5); // Increase priority
       // Should get more CPU time
       
       exit(0);
   }
   ```

**Validation:**
- [ ] sched_yield() provoque context switch réel
- [ ] nice() modifie priorité dans scheduler
- [ ] Tests passent avec comportement réel
- [ ] Git commit: "sched: Implement real syscalls"

---

### JOUR 5: Signal Delivery Réel - Partie 1

**Temps estimé:** 8-10h  
**Priorité:** P1

#### Tâches

**Matin (4-5h)**
1. Implémenter sys_kill() RÉEL
   ```rust
   // kernel/src/syscall/handlers/signals.rs
   pub fn sys_kill(pid: Pid, sig: Signal) -> isize {
       use crate::scheduler::SCHEDULER;
       
       // OLD: return 0; // Stub
       
       // NEW: Lookup process and enqueue signal
       let thread = match SCHEDULER.lock().find_thread_by_pid(pid) {
           Some(t) => t,
           None => return -1, // ESRCH (No such process)
       };
       
       // Enqueue signal
       thread.pending_signals.lock().insert(sig);
       
       // Wake if sleeping
       if thread.state == ThreadState::Sleeping {
           SCHEDULER.lock().wake_thread(thread.tid);
       }
       
       0
   }
   ```

2. Ajouter pending_signals à Thread
   ```rust
   // kernel/src/scheduler/thread/thread.rs
   pub struct Thread {
       // ... existing fields
       pub pending_signals: Arc<Mutex<BTreeSet<Signal>>>,
       pub signal_handlers: Arc<Mutex<SignalHandlerTable>>,
   }
   ```

**Après-midi (4-5h)**
1. Implémenter signal delivery dans scheduler
   ```rust
   // kernel/src/scheduler/signal_delivery.rs
   pub fn deliver_pending_signals(thread: &mut Thread) {
       let pending = thread.pending_signals.lock();
       
       for sig in pending.iter() {
           if thread.signal_mask.is_blocked(*sig) {
               continue; // Signal masked
           }
           
           let handler = thread.signal_handlers.lock().get(*sig);
           
           match handler {
               SigHandler::Ignore => {
                   // Remove and ignore
                   pending.remove(sig);
               }
               SigHandler::Default => {
                   // Default action (terminate, stop, etc.)
                   default_signal_action(thread, *sig);
                   pending.remove(sig);
               }
               SigHandler::Custom(handler_addr) => {
                   // Setup signal frame
                   setup_signal_frame(thread, *sig, handler_addr);
                   pending.remove(sig);
               }
           }
       }
   }
   ```

2. Appeler delivery dans schedule()
   ```rust
   // kernel/src/scheduler/mod.rs
   impl Scheduler {
       pub fn schedule_next(&mut self) {
           // ... existing code ...
           
           // Before switching to thread, deliver signals
           if let Some(next_thread) = self.next_thread {
               deliver_pending_signals(next_thread);
           }
           
           // ... context switch ...
       }
   }
   ```

**Validation:**
- [ ] kill(pid, SIG) enqueue signal réel
- [ ] Signal délivré avant retour userspace
- [ ] Handler ignoré si SIG_IGN
- [ ] Git commit: "signals: Implement delivery (part 1)"

---

### JOUR 6: Signal Delivery Réel - Partie 2

**Temps estimé:** 8-10h

#### Tâches

**Matin (4-5h)**
1. Implémenter signal frame setup
   ```rust
   // kernel/src/arch/x86_64/signal_frame.rs
   
   #[repr(C)]
   pub struct SignalFrame {
       pub saved_rax: u64,
       pub saved_rbx: u64,
       // ... all registers ...
       pub saved_rip: u64,
       pub saved_rsp: u64,
       pub saved_rflags: u64,
       pub signal_number: u32,
   }
   
   pub fn setup_signal_frame(
       thread: &mut Thread,
       sig: Signal,
       handler_addr: u64
   ) {
       let user_stack = thread.context.rsp;
       
       // Save current context on user stack
       let frame_addr = user_stack - size_of::<SignalFrame>();
       unsafe {
           let frame = &mut *(frame_addr as *mut SignalFrame);
           frame.saved_rax = thread.context.rax;
           frame.saved_rbx = thread.context.rbx;
           // ... save all registers ...
           frame.saved_rip = thread.context.rip;
           frame.saved_rsp = user_stack;
           frame.signal_number = sig;
       }
       
       // Setup registers to call handler
       thread.context.rdi = sig as u64; // First arg
       thread.context.rip = handler_addr;
       thread.context.rsp = frame_addr;
       
       // Push return address (sigreturn syscall)
       let ret_addr = frame_addr - 8;
       unsafe {
           *(ret_addr as *mut u64) = sigreturn_trampoline as u64;
       }
   }
   
   extern "C" fn sigreturn_trampoline() {
       // Call sys_sigreturn()
       unsafe {
           asm!(
               "mov rax, 15", // __NR_rt_sigreturn
               "syscall",
               options(noreturn)
           );
       }
   }
   ```

**Après-midi (4-5h)**
1. Implémenter sys_sigreturn()
   ```rust
   // kernel/src/syscall/handlers/signals.rs
   pub fn sys_sigreturn() -> ! {
       let thread = current_thread();
       let frame_addr = thread.context.rsp;
       
       // Restore context from frame
       unsafe {
           let frame = &*(frame_addr as *const SignalFrame);
           thread.context.rax = frame.saved_rax;
           thread.context.rbx = frame.saved_rbx;
           // ... restore all registers ...
           thread.context.rip = frame.saved_rip;
           thread.context.rsp = frame.saved_rsp;
       }
       
       // Return to interrupted code
       unsafe {
           context_switch_to(thread);
       }
   }
   ```

2. Tests
   ```c
   // test_signals_real.c
   volatile int signal_received = 0;
   
   void signal_handler(int sig) {
       signal_received = sig;
       write(1, "Handler called!\n", 16);
   }
   
   void _start() {
       // Register handler
       struct sigaction sa = {
           .sa_handler = signal_handler,
           .sa_mask = 0,
           .sa_flags = 0,
       };
       rt_sigaction(SIGINT, &sa, NULL);
       
       // Fork
       int pid = fork();
       if (pid == 0) {
           // Child: wait for signal
           while (!signal_received) {
               sched_yield();
           }
           
           if (signal_received == SIGINT) {
               write(1, "SIGINT received!\n", 17);
               exit(0);
           }
           exit(1);
       } else {
           // Parent: send signal
           sleep(100); // Brief delay
           kill(pid, SIGINT); // Should trigger handler
           wait4(pid, NULL, 0);
           exit(0);
       }
   }
   ```

**Validation:**
- [ ] Handler appelé avec bon argument (sig)
- [ ] Context sauvé/restauré correctement
- [ ] sigreturn() restaure execution
- [ ] Test passe avec signal réel
- [ ] Git commit: "signals: Complete delivery + frame"

---

### JOUR 7: Process Limits Tracking

**Temps estimé:** 6-8h  
**Priorité:** P2

#### Tâches

**Matin (3-4h)**
1. Créer structures de tracking
   ```rust
   // kernel/src/process/limits.rs
   
   #[derive(Clone, Copy)]
   pub struct RLimit {
       pub cur: u64,  // Soft limit
       pub max: u64,  // Hard limit
   }
   
   pub struct ResourceLimits {
       pub nofile: RLimit,
       pub stack: RLimit,
       pub cpu: RLimit,
       pub as_limit: RLimit,
       pub nproc: RLimit,
       // ...
   }
   
   impl Default for ResourceLimits {
       fn default() -> Self {
           Self {
               nofile: RLimit { cur: 1024, max: 4096 },
               stack: RLimit { cur: 8 * 1024 * 1024, max: RLIM_INFINITY },
               // ...
           }
       }
   }
   ```

2. Ajouter à Process
   ```rust
   // kernel/src/process/mod.rs
   pub struct Process {
       // ... existing
       pub limits: Arc<Mutex<ResourceLimits>>,
       pub usage: Arc<Mutex<RUsage>>,
   }
   ```

**Après-midi (3-4h)**
1. Implémenter syscalls RÉELS
   ```rust
   // kernel/src/syscall/handlers/process_limits.rs
   pub fn sys_getrlimit(resource: i32, rlim: *mut RLimit) -> isize {
       let process = current_process();
       let limits = process.limits.lock();
       
       let limit = match resource {
           RLIMIT_NOFILE => limits.nofile,
           RLIMIT_STACK => limits.stack,
           // ...
           _ => return -1, // EINVAL
       };
       
       unsafe { *rlim = limit; }
       0
   }
   
   pub fn sys_setrlimit(resource: i32, rlim: *const RLimit) -> isize {
       let new_limit = unsafe { *rlim };
       
       // Check permissions
       if new_limit.max > current_limit.max {
           // Only root can increase hard limit
           if !is_root() {
               return -1; // EPERM
           }
       }
       
       let mut limits = current_process().limits.lock();
       match resource {
           RLIMIT_NOFILE => limits.nofile = new_limit,
           // ...
       }
       0
   }
   ```

2. Enforce limits
   ```rust
   // kernel/src/syscall/handlers/io.rs
   pub fn sys_open(...) -> isize {
       let fd_table = CURRENT_PROCESS.fd_table.lock();
       
       // Check RLIMIT_NOFILE
       if fd_table.count() >= CURRENT_PROCESS.limits.lock().nofile.cur {
           return -1; // EMFILE (Too many open files)
       }
       
       // ... proceed with open ...
   }
   ```

3. Track usage
   ```rust
   // kernel/src/scheduler/mod.rs
   impl Scheduler {
       pub fn tick(&mut self) {
           // Update CPU time for current thread
           if let Some(thread) = self.current_thread() {
               let process = thread.process;
               let mut usage = process.usage.lock();
               
               if thread.in_kernel {
                   usage.stime.usec += TICK_USEC;
               } else {
                   usage.utime.usec += TICK_USEC;
               }
           }
       }
   }
   ```

**Validation:**
- [ ] getrlimit() retourne limites réelles
- [ ] setrlimit() modifie limites
- [ ] open() respecte RLIMIT_NOFILE
- [ ] getrusage() retourne temps CPU réels
- [ ] Git commit: "process: Implement resource limits"

---

## 📊 RÉSUMÉ SEMAINE 1

### Livrables
- [ ] exec() charge binaires depuis VFS (pas stub)
- [ ] FD table connectée au VFS (read/write réels)
- [ ] Scheduler syscalls appellent scheduler réel
- [ ] Signals délivrés avec handler calls réels
- [ ] Resource limits trackés et enforced

### Métriques
- **Phase 1:** 45% → **80%** ✅
- **TODOs:** 200 → **<150** ✅
- **Stubs critiques:** 97 → **<60** ✅
- **Tests réels:** 50 → **65** ✅

### Git Commits
```
Week 1 commits:
1. exec: Implement VFS loading (part 1)
2. exec: Complete VFS loading + tests
3. io: Connect FD table to VFS
4. sched: Implement real syscalls
5. signals: Implement delivery (part 1)
6. signals: Complete delivery + frame
7. process: Implement resource limits
```

---

## 📅 SEMAINE 2: NETWORK STACK FONCTIONNEL (Jours 8-14)

**Objectif:** Network stack de 10% → 60%

### Jour 8-9: VirtIO Network Driver
- [ ] Init VirtIO NIC
- [ ] TX/RX queues
- [ ] DMA setup
- [ ] IRQ handling
- [ ] Tests: Transmit/receive raw frames

### Jour 10-11: TCP/IP Stack Réel
- [ ] TCP send_segment() → IP layer
- [ ] IP fragmentation + routing
- [ ] ARP resolve + cache
- [ ] Tests: TCP handshake réel

### Jour 12-13: Socket API Complet
- [ ] connect() → TCP connect réel
- [ ] send/recv → TCP buffers
- [ ] shutdown() → FIN sequence
- [ ] Tests: HTTP request/response

### Jour 14: Network Tests + Validation
- [ ] Wireshark validation
- [ ] Latency measurements
- [ ] Documentation

---

## 📅 SEMAINE 3: STORAGE FONCTIONNEL (Jours 15-21)

**Objectif:** Storage de 5% → 50%

### Jour 15-16: VirtIO Block Driver
- [ ] Init VirtIO block device
- [ ] Request queue
- [ ] Read/write sectors
- [ ] Tests: Read MBR

### Jour 17-18: FAT32 Driver Réel
- [ ] Connecter parser existant
- [ ] Implement VfsInode trait
- [ ] Read/write files
- [ ] Tests: Mount + ls + cat

### Jour 19-20: ext4 Basique
- [ ] Superblock parsing
- [ ] Inode lookup
- [ ] Extent tree
- [ ] Tests: Read files

### Jour 21: Storage Tests + Validation
- [ ] IOPS measurements
- [ ] Data integrity tests
- [ ] Documentation

---

## 📅 SEMAINE 4: IPC + FINITION (Jours 22-28)

**Objectif:** IPC de 20% → 70%, Finition globale

### Jour 22-23: Fusion Rings Réels
- [ ] Allocate shared memory
- [ ] Send/recv réels
- [ ] Futex blocking
- [ ] Tests: Latency <700 cycles

### Jour 24-25: Shared Memory Réel
- [ ] shmget() allocate
- [ ] shmat() map
- [ ] Tests: Multi-process

### Jour 26-27: Cleanup Final
- [ ] Éliminer TODOs restants
- [ ] Fix warnings
- [ ] Performance optimization
- [ ] Documentation

### Jour 28: Tests + Release
- [ ] Run all tests
- [ ] Validation QEMU
- [ ] Create v0.7.0 release
- [ ] Documentation finale

---

## 🎯 MÉTRIQUES FINALES (Semaine 4)

| Métrique | Actuel | Semaine 1 | Semaine 2 | Semaine 3 | Semaine 4 |
|----------|--------|-----------|-----------|-----------|-----------|
| **Phase 1** | 45% | **80%** | 85% | 90% | **95%** |
| **Phase 2** | 22% | 25% | **60%** | 65% | **70%** |
| **Phase 3** | 5% | 5% | 10% | **50%** | **55%** |
| **Global** | 35% | 50% | 65% | 75% | **80%** |
| **TODOs** | 200 | <150 | <100 | <60 | **<30** |
| **Stubs** | 97 | <60 | <35 | <20 | **<10** |

---

## ✅ RÈGLES DE PROGRESSION

### Code Quality
1. **Zéro stub success** - Pas de `return 0` fake
2. **Zéro TODO critique** - Implémenter ou supprimer
3. **Tests réels** - Vérifier comportement, pas structures
4. **Git commits atomiques** - 1 feature = 1 commit
5. **Documentation à jour** - Chaque jour

### Validation
- [ ] Chaque feature testée dans QEMU
- [ ] Pas de regression (anciens tests passent)
- [ ] Performance mesurée (rdtsc)
- [ ] Code reviewed avant commit

### Blockers
- Si bloqué >2h sur un module → Lire code COMPLET du module
- Si bloqué >4h → Demander aide / recherche exemples
- Si bloqué >1 jour → Revoir approche / simplifier

---

## 🚀 PRÊT À COMMENCER ?

**Prochaine action:** JOUR 1 - exec() VFS Loading Part 1

Confirmer prêt avec:
```bash
cd /workspaces/Exo-OS
git status
make clean
make build
```

**Go! 🎯**
