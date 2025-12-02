# 📋 TODO List - Exo-OS v0.5.0

**Dernière mise à jour:** 2 décembre 2025  
**Version cible:** v0.5.0 "Stellar Engine"  
**État actuel:** v0.4.0 (~55% fonctionnel réel)

---

## 🔴 Priorité BLOQUANTE (Semaine 1)

### 1. ⚡ Context Switch Réel
**Statut:** 🚨 CRITIQUE - VIDE!  
**Fichier:** `kernel/src/scheduler/switch/windowed.rs`  
**Problème:** Le fichier ne contient que des stubs vides!

**Code actuel (5 lignes):**
```rust
pub fn init() { /* Placeholder */ }
pub fn windowed_context_switch(_old: &Context, _new: &Context) {
    // TODO: Implement
}
```

**Solution requise:**
```rust
extern "C" {
    fn windowed_context_switch(old_rsp: *mut u64, new_rsp: *const u64);
    fn windowed_init_context(ctx: *mut u64, entry: u64, stack: u64);
}

pub fn switch_to(old: &mut ThreadContext, new: &ThreadContext) {
    unsafe {
        windowed_context_switch(
            &mut old.rsp as *mut u64,
            &new.rsp as *const u64
        );
    }
}
```

**Tâches:**
- [ ] Implémenter liaison FFI avec windowed_context_switch.S
- [ ] Tester switch entre 2 threads
- [ ] Mesurer cycles (<500 objectif)
- [ ] Intégrer dans scheduler.rs switch_to_thread()

---

### 2. ⚡ Timer Preemption
**Statut:** 🚨 CRITIQUE  
**Fichier:** `kernel/src/arch/x86_64/interrupts.rs`  
**Problème:** Timer tick ne déclenche pas schedule()

**Tâches:**
- [ ] Modifier timer_interrupt_handler
- [ ] Appeler crate::scheduler::yield_now() tous les N ticks
- [ ] Configurer quantum (10-50ms)
- [ ] Tester preemption automatique

---

### 3. ⚡ Page Table Mapper
**Statut:** 🚨 CRITIQUE - NON IMPLÉMENTÉ  
**Fichier:** `kernel/src/memory/virtual_mem/mapper.rs`  
**Problème:** mmap/munmap créent des structures mais ne mappent pas!

**Tâches:**
- [ ] Implémenter map_page(virt, phys, flags)
- [ ] Implémenter unmap_page(virt)
- [ ] Flush TLB (invlpg)
- [ ] Tester mapping anonyme

---

## 🟠 Priorité HAUTE (Semaines 2-3)

### 4. 🔧 mmap/munmap Réels
**Statut:** ⚠️ Partiel  
**Fichier:** `kernel/src/memory/mmap.rs`

**Tâches:**
- [ ] Appeler mapper dans mmap()
- [ ] Appeler mapper dans munmap()
- [ ] Gérer protections (R/W/X)
- [ ] Allouer vraies frames physiques

---

### 5. 🔧 sys_brk Réel
**Statut:** ⚠️ Stub  
**Fichier:** `kernel/src/syscall/handlers/memory.rs`

**Tâches:**
- [ ] Implémenter expansion heap
- [ ] Implémenter réduction heap
- [ ] Mapper nouvelles pages
- [ ] Retourner nouvelle adresse

---

### 6. 🔧 tmpfs Fonctionnel
**Statut:** ❌ Stub  
**Fichier:** `kernel/src/fs/vfs/tmpfs.rs`

**Tâches:**
- [ ] Implémenter TmpfsInode (fichier/dossier)
- [ ] Implémenter create, read, write
- [ ] Monter sur /
- [ ] Créer /dev, /tmp, /etc

---

### 7. 🔧 Keyboard Driver
**Statut:** ❌ Non existant  
**Fichier:** `kernel/src/drivers/input/keyboard.rs` (à créer)

**Tâches:**
- [ ] Créer fichier + module
- [ ] Handler IRQ1
- [ ] Scan code → ASCII (US layout)
- [ ] Buffer circulaire 256 chars
- [ ] Exposer via /dev/tty

---

## 🟡 Priorité MOYENNE (Semaines 3-4)

### 8. 📂 Initramfs/TarFS
**Statut:** ❌ Non existant  
**Fichier:** `kernel/src/fs/tarfs/` (à créer)

**Tâches:**
- [ ] Parser header TAR
- [ ] Extraire fichiers en mémoire
- [ ] Monter sur /initrd
- [ ] Accès lecture seule

---

### 9. 📂 ELF Loader
**Statut:** ❌ Stub  
**Fichier:** `kernel/src/posix_x/elf/loader.rs`

**Tâches:**
- [ ] Parser ELF64 header
- [ ] Charger segments PT_LOAD
- [ ] Initialiser .bss
- [ ] Préparer stack userspace
- [ ] Retourner entry point

---

### 10. 📂 User Mode Transition
**Statut:** ❌ Non existant  
**Fichier:** `kernel/src/arch/x86_64/usermode.rs` (à créer)

**Tâches:**
- [ ] Configurer TSS pour Ring 0 stack
- [ ] Préparer iretq frame
- [ ] Jump Ring 3
- [ ] Syscall return (sysretq)

---

### 11. 📂 /bin/init
**Statut:** ❌ Non existant  
**Fichier:** `userspace/init/main.c` (à créer)

**Code minimal:**
```c
void _start() {
    const char* msg = "Exo-OS v0.5.0 Userspace!\n";
    asm volatile("syscall" :: "a"(1), "D"(1), "S"(msg), "d"(26));
    for(;;) asm volatile("syscall" :: "a"(34)); // pause
}
```

---

## 🟢 Priorité NORMALE (Semaines 4-6)

### 12. Multi-core (SMP)
**Statut:** ⚠️ Désactivé  
**Fichier:** `kernel/src/arch/x86_64/boot/trampoline.asm`  
**Problème:** Directives NASM incompatibles avec global_asm!()

**Tâches:**
- [ ] Compiler trampoline.asm séparément avec NASM
- [ ] Lier via build.rs
- [ ] Réactiver SMP dans smp.rs
- [ ] Tester sur QEMU -smp 4

---

### 13. Syscall Handlers Manquants
**Statut:** ⚠️ ~70% stubs  
**Fichiers:** `kernel/src/syscall/handlers/*.rs`

**Priorités:**
- [ ] fork() - Duplication process (structure, pas COW)
- [ ] exec() - Charger ELF
- [ ] wait() - Attendre child
- [ ] pipe() - IPC basique
- [ ] dup/dup2() - Duplication FD

---

### 14. Cleanup Warnings
**Statut:** 📝 TODO  
**Objectif:** Réduire 200+ warnings à <50

**Tâches:**
- [ ] `cargo fix --allow-dirty`
- [ ] Ajouter #[allow(dead_code)] sur code préparatoire
- [ ] Préfixer _ sur variables debug
- [ ] Migrer static mut vers SyncUnsafeCell

---

### 15. Documentation
**Statut:** 📝 TODO

**Tâches:**
- [ ] Mettre à jour ARCHITECTURE.md
- [ ] Documenter syscalls supportés
- [ ] Créer USERSPACE_GUIDE.md
- [ ] Générer rustdoc

---

## 🔵 Priorité BASSE (Après v0.5.0)

### 16. 🔮 Prediction EMA Scheduler
- [ ] Implémenter scheduler/prediction/ema.rs
- [ ] Historique exécutions
- [ ] Classification Hot/Normal/Cold automatique

### 17. 🔮 Zero-Copy IPC
- [ ] Shared memory réel
- [ ] Fusion Ring avec mapping
- [ ] Benchmark vs Linux pipes

### 18. 🔮 Network Stack
- [ ] TCP/IP stack
- [ ] Socket API
- [ ] virtio-net driver

### 19. 🔮 Real Filesystems
- [ ] FAT32 driver
- [ ] ext4 read-only
- [ ] AHCI/NVMe drivers

---

## 📊 Progression v0.5.0

| Phase | Objectif | État |
|-------|----------|------|
| **Phase 1** | Context Switch | 🔴 0% |
| **Phase 2** | Mémoire Virtuelle | 🔴 0% |
| **Phase 3** | VFS Minimal | 🔴 0% |
| **Phase 4** | Keyboard | 🔴 0% |
| **Phase 5** | Userspace | 🔴 0% |
| **Phase 6** | Stabilisation | 🔴 0% |

**Progression globale v0.5.0:** 0% ⬜⬜⬜⬜⬜⬜⬜⬜⬜⬜

---

## 🎯 Objectif Cette Semaine

1. **Jour 1-2:** Implémenter windowed.rs avec liaison ASM
2. **Jour 3:** Tester context switch entre 2 threads
3. **Jour 4:** Timer preemption
4. **Jour 5:** Debug et stabilisation
5. **Weekend:** Début mapper.rs

---

## ✅ Terminé (Héritage v0.4.0)

- [x] Boot GRUB → Rust
- [x] Frame allocator (bitmap)
- [x] Heap allocator (10MB)
- [x] GDT/IDT
- [x] PIC 8259 + PIT 100Hz
- [x] Serial output
- [x] VGA text mode
- [x] 3-Queue scheduler (structure)
- [x] Thread spawn (structure)
- [x] Syscall dispatch table

---

**Légende:**
- 🚨 BLOQUANT - Empêche le fonctionnement
- ⚠️ Partiel - Structure OK, implémentation manquante
- ❌ Non existant - À créer
- 📝 TODO - Planifié
- ✅ Terminé
- 🔮 Futur - Après v0.5.0
