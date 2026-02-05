# Plan d'Intégration TOTALE du Copy-on-Write

**Date**: 9 Janvier 2026  
**Objectif**: CoW 100% fonctionnel avec processus userspace réels  
**État actuel**: 30% (infrastructure OK, intégration partielle)

---

## 🔍 DIAGNOSTIC PRÉCIS

### Ce qui fonctionne ✅
1. **CoW Manager** (393 lignes)
   - mark_cow() : Incrémente refcount ✅
   - handle_cow_fault() : Copie page sur écriture ✅
   - get_stats() : Retourne métriques ✅
   - Tests: 6 pages trackées correctement ✅

2. **Page Fault Handler**
   - Détecte fautes CoW (bit 9) ✅
   - Appelle handle_cow_fault() ✅
   - Intégré dans IDT ✅

3. **UserPageFlags**
   - Bit CoW défini ✅
   - remove_writable() fonctionne ✅
   - BitOr pour combiner flags ✅

### Ce qui NE fonctionne PAS ❌

1. **capture_address_space() : 0 pages capturées**
   ```
   [FORK] Captured 0 pages ❌
   ```
   - Scanne user space (0x400000-1GB)
   - Tests tournent dans kernel threads
   - Kernel threads n'ont pas de pages user

2. **sys_fork() ne fait pas de CoW réel**
   - Crée thread mais sans cloner address space
   - 0 pages marquées CoW
   - 0 pages partagées
   - CoW jamais appliqué en pratique

3. **Pas de Process abstraction**
   - Seulement des Threads
   - Pas d'UserAddressSpace par processus
   - Pas de hierarchie parent/enfant

4. **Tests dans mauvais contexte**
   - Kernel threads au lieu de user processes
   - Pas de pages user mappées
   - Impossible de tester CoW réellement

---

## 📋 PLAN D'ACTION EN 4 PHASES

### PHASE 1: Process Table + Address Space ⭐ CRITIQUE

**Objectif**: Chaque processus a son UserAddressSpace

**Tâches**:
1. [ ] Créer `kernel/src/process/table.rs`
   ```rust
   pub struct Process {
       pub pid: Pid,
       pub parent_pid: Option<Pid>,
       pub address_space: UserAddressSpace,
       pub main_thread: ThreadId,
       pub fd_table: FdTable,
       pub credentials: Credentials,
   }
   
   pub struct ProcessTable {
       processes: BTreeMap<Pid, Arc<Mutex<Process>>>,
   }
   
   static PROCESS_TABLE: Mutex<ProcessTable> = ...;
   ```

2. [ ] Ajouter `Process* current_process` dans Thread
   ```rust
   pub struct Thread {
       // ... existing fields
       pub process: Option<Arc<Mutex<Process>>>,
   }
   ```

3. [ ] Helper: `get_current_process()`
   ```rust
   pub fn get_current_process() -> Option<Arc<Mutex<Process>>> {
       SCHEDULER.with_current_thread(|t| t.process.clone())
   }
   ```

**Tests**:
- [ ] Créer process de test avec address space
- [ ] Vérifier process.address_space accessible
- [ ] Valider PID unique par process

**Temps estimé**: 2h

---

### PHASE 2: UserAddressSpace.fork_cow() COMPLET

**Objectif**: Cloner VRAIMENT l'address space avec CoW

**Tâches**:
1. [ ] Implémenter `walk_pages()` (page table walker)
   ```rust
   impl UserAddressSpace {
       /// Parcourt TOUTES les pages via page tables
       fn walk_pages(&self) -> Vec<(VirtualAddress, PhysicalAddress, UserPageFlags)> {
           let mut pages = Vec::new();
           
           // PML4 -> PDPT -> PD -> PT
           for pml4_idx in 0..256 { // User space (première moitié)
               if let Some(pdpt) = self.get_pdpt(pml4_idx) {
                   for pdpt_idx in 0..512 {
                       if let Some(pd) = self.get_pd(pdpt, pdpt_idx) {
                           for pd_idx in 0..512 {
                               if let Some(pt) = self.get_pt(pd, pd_idx) {
                                   for pt_idx in 0..512 {
                                       if let Some((virt, phys, flags)) = 
                                           self.get_page_entry(pt, pt_idx) {
                                           pages.push((virt, phys, flags));
                                       }
                                   }
                               }
                           }
                       }
                   }
               }
           }
           
           pages
       }
   }
   ```

2. [ ] Implémenter `fork_cow()` complet
   ```rust
   pub fn fork_cow(&self) -> Result<Self, MemoryError> {
       let mut child = Self::new()?;
       
       // Parcourir TOUTES les pages
       for (virt, phys, flags) in self.walk_pages() {
           if flags.contains(UserPageFlags::WRITABLE) {
               // Marquer parent comme CoW (read-only + bit CoW)
               let cow_flags = flags.remove_writable().cow();
               self.update_flags(virt, cow_flags)?;
               
               // Mapper enfant sur MÊME page physique en CoW
               child.map_page(virt, phys, cow_flags)?;
               
               // Incrémenter refcount (2 processus partagent)
               cow_manager::mark_cow(phys);
           } else {
               // Page read-only: partager directement
               child.map_page(virt, phys, flags)?;
           }
       }
       
       Ok(child)
   }
   ```

3. [ ] Ajouter `update_flags()` dans UserAddressSpace
   ```rust
   pub fn update_flags(&mut self, virt: VirtualAddress, 
                      flags: UserPageFlags) -> Result<(), MemoryError> {
       // Trouver PTE et mettre à jour flags
       let pte = self.get_pte_mut(virt)?;
       *pte = PageTableEntry::new(pte.phys_addr(), flags);
       
       // Invalider TLB
       Self::invalidate_tlb(virt);
       Ok(())
   }
   ```

**Tests**:
- [ ] fork_cow() avec 10 pages mappées
- [ ] Vérifier: parent et enfant ont les mêmes pages physiques
- [ ] Vérifier: pages marquées CoW (read-only + bit 9)
- [ ] Vérifier: refcount = 2 pour chaque page

**Temps estimé**: 4h

---

### PHASE 3: sys_fork() Complet avec Process

**Objectif**: Fork crée VRAI processus enfant avec address space cloné

**Tâches**:
1. [ ] Réécrire `sys_fork()` complètement
   ```rust
   pub fn sys_fork() -> MemoryResult<Pid> {
       // 1. Get current process
       let parent_process = get_current_process()
           .ok_or(MemoryError::InvalidAddress)?;
       
       let parent = parent_process.lock();
       
       // 2. Clone address space avec CoW COMPLET
       let child_address_space = parent.address_space.fork_cow()?;
       
       // 3. Clone file descriptors
       let child_fd_table = parent.fd_table.clone();
       
       // 4. Create child process
       let child_pid = allocate_pid();
       let child_process = Process {
           pid: child_pid,
           parent_pid: Some(parent.pid),
           address_space: child_address_space,
           main_thread: 0, // Will be set below
           fd_table: child_fd_table,
           credentials: parent.credentials.clone(),
       };
       
       // 5. Create child thread (clone parent context)
       let parent_thread = SCHEDULER.current_thread()
           .ok_or(MemoryError::InvalidAddress)?;
       
       let mut child_context = parent_thread.context().clone();
       child_context.rax = 0; // fork() returns 0 in child
       
       let child_thread = Thread::new_with_context(
           allocate_tid(),
           "forked_child",
           child_context,
           Some(Arc::new(Mutex::new(child_process.clone()))),
       );
       
       // 6. Add to tables
       PROCESS_TABLE.lock().insert(child_pid, 
           Arc::new(Mutex::new(child_process)));
       SCHEDULER.add_thread(child_thread)?;
       
       // 7. Return child PID to parent
       Ok(child_pid)
   }
   ```

2. [ ] Créer `Thread::new_with_context()`
   ```rust
   impl Thread {
       pub fn new_with_context(
           tid: ThreadId,
           name: &str,
           context: ThreadContext,
           process: Option<Arc<Mutex<Process>>>,
       ) -> Self {
           Self {
               id: tid,
               name: String::from(name),
               context,
               process,
               // ... autres champs
           }
       }
   }
   ```

**Tests**:
- [ ] Fork crée process enfant avec PID différent
- [ ] Child.parent_pid == parent.pid
- [ ] Child.address_space != parent.address_space (instances différentes)
- [ ] Mais pages physiques partagées (même adresses)
- [ ] Refcount = 2 pour pages partagées

**Temps estimé**: 3h

---

### PHASE 4: Tests Userspace Réels

**Objectif**: Tester avec VRAIS processus user (pas kernel threads)

**Tâches**:
1. [ ] Créer processus test minimal
   ```rust
   // kernel/src/tests/cow_user_test.rs
   pub fn create_test_user_process() -> Result<Pid, MemoryError> {
       // 1. Create user address space
       let mut address_space = UserAddressSpace::new()?;
       
       // 2. Map code segment (simple infinite loop)
       let code: [u8; 16] = [
           0xEB, 0xFE, // jmp $  (infinite loop)
           // ... padding
       ];
       address_space.map_segment_data(
           VirtualAddress::new(0x400000),
           &code,
           4096,
           UserPageFlags::user_code(),
       )?;
       
       // 3. Map data segment (shared_data variable)
       address_space.map_range(
           VirtualAddress::new(0x401000),
           4096,
           UserPageFlags::user_data(),
       )?;
       
       // Write test value: 0xDEADBEEF
       unsafe {
           let data_ptr = 0x401000 as *mut u64;
           *data_ptr = 0xDEADBEEF;
       }
       
       // 4. Map stack
       address_space.map_range(
           VirtualAddress::new(0x7FFFFF000),
           4096 * 2,
           UserPageFlags::user_stack(),
       )?;
       
       // 5. Create process
       let process = Process {
           pid: allocate_pid(),
           parent_pid: None,
           address_space,
           main_thread: 0,
           fd_table: FdTable::new(),
           credentials: Credentials::root(),
       };
       
       // 6. Create thread with user context
       let context = ThreadContext {
           rip: 0x400000, // Start at code
           rsp: 0x7FFFFFF000, // Top of stack
           rflags: 0x202, // IF flag
           // ... autres registres
       };
       
       let thread = Thread::new_with_context(
           allocate_tid(),
           "test_user",
           context,
           Some(Arc::new(Mutex::new(process.clone()))),
       );
       
       PROCESS_TABLE.lock().insert(process.pid, 
           Arc::new(Mutex::new(process)));
       SCHEDULER.add_thread(thread)?;
       
       Ok(process.pid)
   }
   ```

2. [ ] Test CoW avec processus user
   ```rust
   pub fn test_cow_with_user_process() {
       // 1. Create parent process
       let parent_pid = create_test_user_process().unwrap();
       
       // 2. Fork it
       let child_pid = sys_fork().unwrap();
       
       // 3. Verify pages shared
       let parent = PROCESS_TABLE.lock().get(parent_pid).unwrap();
       let child = PROCESS_TABLE.lock().get(child_pid).unwrap();
       
       let parent_pages = parent.address_space.walk_pages();
       let child_pages = child.address_space.walk_pages();
       
       // Same physical addresses
       assert_eq!(parent_pages[0].1, child_pages[0].1);
       
       // 4. Write in child (trigger CoW fault)
       // ... (requires page fault handler to work)
       
       // 5. Verify parent unchanged
       // 6. Verify child has new page
   }
   ```

3. [ ] Métriques page fault handler
   ```rust
   static COW_FAULT_COUNT: AtomicUsize = AtomicUsize::new(0);
   
   pub fn handle_cow_fault(...) {
       COW_FAULT_COUNT.fetch_add(1, Ordering::SeqCst);
       // ... existing code
   }
   
   pub fn get_cow_fault_count() -> usize {
       COW_FAULT_COUNT.load(Ordering::SeqCst)
   }
   ```

**Tests**:
- [ ] Process user créé avec pages mappées
- [ ] Fork capture >10 pages
- [ ] Écriture déclenche page fault CoW
- [ ] COW_FAULT_COUNT > 0
- [ ] Nouvelle page allouée
- [ ] Parent garde valeur originale
- [ ] Enfant a valeur modifiée

**Temps estimé**: 4h

---

## 🎯 CRITÈRES DE VALIDATION FINALE

### Tests Obligatoires
- [ ] **Fork capture pages** : >10 pages capturées (pas 0)
- [ ] **Refcount initial** : = 2 pour pages partagées
- [ ] **Page fault CoW** : Déclenché sur première écriture enfant
- [ ] **Copie de page** : Nouvelle page allouée et copiée
- [ ] **Refcount décrémenté** : = 1 après copie
- [ ] **Isolation** : Parent et enfant ont données indépendantes
- [ ] **Performance** : Latence fork < 100K cycles
- [ ] **Pas de leaks** : Toutes pages libérées après exit

### Métriques Cibles
| Métrique | Actuel | Cible | Priority |
|----------|--------|-------|----------|
| Pages capturées | 0 | >10 | P0 |
| Latence fork | 703M | <100K | P1 |
| CoW faults | 0 | >1 | P0 |
| Refcount pages | 2 | 2 | ✅ |
| Memory leaks | ? | 0 | P1 |

---

## 📅 PLANNING

### Semaine 1 (9-15 Jan)
- Lundi: Phase 1 (Process Table)
- Mardi: Phase 2 (fork_cow complet)
- Mercredi: Phase 3 (sys_fork refonte)
- Jeudi: Phase 4 (tests user)
- Vendredi: Validation + métriques

### Milestones
- **M1** (12 Jan): Process Table opérationnel
- **M2** (14 Jan): fork_cow() capture pages
- **M3** (15 Jan): Tests userspace passent
- **M4** (16 Jan): CoW 100% validé

---

## 🚀 PROCHAINE ACTION IMMÉDIATE

**Commencer MAINTENANT par Phase 1**:
1. Créer `kernel/src/process/mod.rs`
2. Définir `struct Process`
3. Créer `ProcessTable` avec mutex global
4. Ajouter `process: Option<Arc<Mutex<Process>>>` dans Thread
5. Implémenter `get_current_process()`

**Commande**:
```bash
touch kernel/src/process/mod.rs
touch kernel/src/process/table.rs
```

**Premier test**:
```rust
// Créer un process de test
let process = Process::new(1, UserAddressSpace::new()?);
PROCESS_TABLE.lock().insert(1, process);
assert!(PROCESS_TABLE.lock().get(1).is_some());
```

---

**RAPPEL**: CoW 100% = Fork avec pages réelles + Page fault handler fonctionnel + Métriques validées

L'infrastructure existe (30%), l'intégration manque (70% restant).
