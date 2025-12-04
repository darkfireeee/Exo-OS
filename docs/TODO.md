# 📋 Roadmap Exo-OS - Vision v1.0.0 "Linux Crusher"

**Dernière mise à jour:** 4 décembre 2025  
**Version actuelle:** v0.5.0 "Stellar Engine"  
**Version cible:** v1.0.0 "Linux Crusher"  
**Licence:** GPL-2.0 (compatible drivers Linux)

---

## 🎯 VISION: Écraser Linux sur les Performances

| Métrique | Linux | Exo-OS Target | Ratio |
|----------|-------|---------------|-------|
| IPC Latence | 1247 cycles | **347 cycles** | 3.6x |
| Context Switch | 2134 cycles | **304 cycles** | 7x |
| Alloc Thread-Local | ~50 cycles | **8 cycles** | 6.25x |
| Scheduler Pick | ~200 cycles | **87 cycles** | 2.3x |

---

## 📊 Progression Globale v1.0.0

| Phase | Version | Objectif | État | Priorité |
|-------|---------|----------|------|----------|
| **Phase 0** | v0.5.0 | Timer + Context Switch + Virtual Memory | 🟡 30% | 🔴 CRITIQUE |
| **Phase 1** | v0.6.0 | VFS Complet + POSIX-X + fork/exec | 🔴 25% | 🔴 CRITIQUE |
| **Phase 2** | v0.7.0 | SMP Multi-core + Network TCP/IP | 🔴 10% | 🟠 HAUTE |
| **Phase 3** | v0.8.0 | Drivers Linux GPL-2.0 + Storage | 🔴 5% | 🟠 HAUTE |
| **Phase 4** | v0.9.0 | Security + Crypto + TPM | 🟡 40% | 🟡 MOYENNE |
| **Phase 5** | v1.0.0 | Performance Tuning + Polish | 🔴 0% | 🟡 MOYENNE |

**Progression globale v1.0.0:** ~20% 🟩🟩⬜⬜⬜⬜⬜⬜⬜⬜

📚 **Documents de référence:**
- [ROADMAP_v1.0.0_LINUX_CRUSHER.md](ROADMAP_v1.0.0_LINUX_CRUSHER.md) - Plan détaillé 9-10 mois
- [TODO_TECHNIQUE_IMMEDIAT.md](TODO_TECHNIQUE_IMMEDIAT.md) - Actions cette semaine
- [POSIX_X_SYSCALL_ANALYSIS.md](POSIX_X_SYSCALL_ANALYSIS.md) - Analyse 100+ syscalls

---

## 🚀 PHASE 0 - v0.5.0 "Stellar Engine" (EN COURS)

**Objectif:** Timer preemption + Context switch fonctionnel + Virtual memory de base
**Statut:** 🟢 85% COMPLÈTE

### ✅ Terminé Phase 0
- [x] **Boot ISO fonctionnel** - grub-bios installé, El Torito OK
- [x] **Linkage C/ASM/Rust** - boot.asm → boot.c → rust_main
- [x] **Timer preemption** - 3 threads avec préemption automatique (PIT 100Hz)
- [x] **Context switch** - windowed_context_switch ASM fonctionnel
- [x] **MMU fonctions réelles** - get/set CR3, invalidate TLB
- [x] **Benchmark infrastructure** - rdtsc/rdtscp pour mesurer cycles
- [x] **Clavier PS/2** - IRQ1 handler, scancode→ASCII (QWERTY/AZERTY)
- [x] **pipe() syscall** - IPC named channels implémenté
- [x] **tmpfs** - read/write/create fonctionnels

### 🔴 Restant Phase 0
- [ ] Mesurer cycles context switch (objectif: <500)
- [ ] Tests exhaustifs mémoire virtuelle
- [ ] Shell interactif avec clavier

---

## ✅ Terminé dans v0.4.1

### Boot & Initialisation
- [x] Boot GRUB2 → Multiboot2 → Rust kernel
- [x] Serial output (COM1 @ 115200 baud)
- [x] VGA text mode avec splash screen animé
- [x] **SSE/SIMD activé** (init_early avant tout code)

### Mémoire
- [x] Frame allocator (bitmap, ~256MB supporté)
- [x] Heap allocator (linked-list, 10MB)
- [x] Structures mmap/VMA (pas encore mapper)

### Interruptions & CPU
- [x] GDT avec segments kernel
- [x] IDT avec 256 vecteurs
- [x] PIC 8259 configuré (IRQs 32-47)
- [x] Timer PIT 100Hz fonctionnel
- [x] Interrupts timer reçus

### Scheduler
- [x] Structure 3-Queue EMA (Hot/Normal/Cold)
- [x] Thread spawn avec allocation stack
- [x] ThreadContext avec RSP/RIP
- [x] **Context switch ASM** (global_asm! inline)
- [x] 3 threads de test créés

### Syscalls
- [x] Dispatch table avec 400+ entrées
- [x] Handlers stubs pour la plupart

---

## 🔴 Phase 1: Context Switch Réel (CRITIQUE)

**Statut:** ✅ 80% - ASM implémenté, intégration en cours

### Fichiers concernés
- `kernel/src/scheduler/switch/windowed.rs` ✅
- `kernel/src/scheduler/core/scheduler.rs` 🟡
- `kernel/src/arch/x86_64/interrupts.rs` 🔴

### Tâches
- [x] Implémenter `windowed_context_switch` en global_asm!
- [x] Implémenter `windowed_init_context` pour setup stack
- [x] Corriger commentaires ASM (// → #)
- [ ] **Appeler switch depuis timer handler**
- [ ] Tester switch entre Thread A et Thread B
- [ ] Mesurer cycles (objectif: <500)

### Code à ajouter dans `interrupts.rs`
```rust
// Dans timer_interrupt_handler:
pub extern "x86-interrupt" fn timer_handler(_frame: InterruptStackFrame) {
    // Incrémenter tick
    crate::time::tick();
    
    // Preemption tous les 10 ticks (100ms)
    if crate::time::ticks() % 10 == 0 {
        crate::scheduler::schedule();
    }
    
    // EOI
    unsafe { crate::arch::x86_64::pic::end_of_interrupt(0x20); }
}
```

---

## 🔴 Phase 2: Timer Preemption (CRITIQUE)

**Statut:** 🟡 50% - Timer fonctionne, preemption pas encore

### Fichiers concernés
- `kernel/src/time/mod.rs` 🟡
- `kernel/src/scheduler/core/scheduler.rs` 🟡
- `kernel/src/arch/x86_64/pic.rs` ✅

### Tâches
- [x] PIT configuré à 100Hz
- [x] IRQ0 → handler appelé
- [ ] **Compteur de ticks global**
- [ ] **Fonction schedule() appelée depuis timer**
- [ ] Quantum configurable (10-50ms)
- [ ] Round-robin basique entre threads ready

---

## 🟠 Phase 3: Mémoire Virtuelle (HAUTE)

**Statut:** 🔴 20% - Structures OK, mapping non implémenté

### Fichiers concernés
- `kernel/src/memory/virtual_mem/mapper.rs` 🔴
- `kernel/src/memory/mmap.rs` 🟡
- `kernel/src/memory/page_table.rs` 🟡

### Tâches
- [x] Structures VMA et VmMapping
- [x] Fonction mmap() structure
- [ ] **Implémenter map_page(virt, phys, flags)**
- [ ] **Implémenter unmap_page(virt)**
- [ ] Flush TLB avec invlpg
- [ ] mmap anonyme fonctionnel
- [ ] mprotect pour changer permissions

### Code requis pour `mapper.rs`
```rust
pub fn map_page(
    page_table: &mut PageTable,
    virt: VirtAddr,
    phys: PhysAddr,
    flags: PageFlags,
) -> Result<(), MapError> {
    let p4 = page_table.level4_table();
    let p4_index = virt.p4_index();
    
    // Créer P3 si nécessaire
    if !p4[p4_index].is_present() {
        let frame = allocate_frame()?;
        p4[p4_index] = PageTableEntry::new(frame, PageFlags::PRESENT | PageFlags::WRITABLE);
    }
    
    // ... continuer pour P3, P2, P1
    
    // Mapper la page finale
    p1[p1_index] = PageTableEntry::new(phys, flags);
    
    // Flush TLB
    unsafe { asm!("invlpg [{}]", in(reg) virt.as_u64()); }
    
    Ok(())
}
```

---

## 🟠 Phase 4: VFS Minimal (HAUTE)

**Statut:** 🔴 10% - Structure VFS existe, tmpfs stub

### Fichiers concernés
- `kernel/src/fs/vfs/mod.rs` 🟡
- `kernel/src/fs/vfs/tmpfs.rs` 🔴
- `kernel/src/fs/vfs/devfs.rs` 🔴

### Tâches
- [x] Trait VfsNode défini
- [x] Structure VFS avec root
- [ ] **Implémenter TmpfsInode (fichier/dossier)**
- [ ] create(), read(), write() pour tmpfs
- [ ] Monter tmpfs sur /
- [ ] Créer /dev, /tmp, /proc
- [ ] /dev/null, /dev/zero, /dev/console

---

## 🟡 Phase 5: Clavier PS/2 (MOYENNE)

**Statut:** 🔴 0% - Non commencé

### Fichiers à créer
- `kernel/src/drivers/input/mod.rs`
- `kernel/src/drivers/input/keyboard.rs`
- `kernel/src/drivers/input/scancode.rs`

### Tâches
- [ ] Créer module drivers/input
- [ ] Handler IRQ1 (keyboard)
- [ ] Table scancode → ASCII (US layout)
- [ ] Buffer circulaire 256 caractères
- [ ] Fonction keyboard_read() bloquante
- [ ] Exposer via /dev/tty

---

## 🟡 Phase 6: Premier Userspace (MOYENNE)

**Statut:** 🔴 0% - Non commencé

### Fichiers concernés
- `kernel/src/posix_x/elf/loader.rs` 🔴
- `kernel/src/arch/x86_64/usermode.rs` (à créer)
- `userspace/init/main.c` (à créer)

### Tâches
- [ ] Parser ELF64 header
- [ ] Charger segments PT_LOAD
- [ ] Initialiser .bss à zéro
- [ ] Préparer stack userspace
- [ ] Configurer TSS pour ring 0 stack
- [ ] Transition vers Ring 3 (iretq)
- [ ] Syscall return (sysretq)

### /bin/init minimal
```c
// userspace/init/main.c
void _start() {
    const char* msg = "Exo-OS v0.5.0 Userspace!\n";
    // sys_write(1, msg, 26)
    asm volatile("syscall" :: "a"(1), "D"(1), "S"(msg), "d"(26));
    // Boucle infinie avec pause
    for(;;) asm volatile("syscall" :: "a"(34)); // sys_pause
}
```

---

## 🟢 Phase 7: Stabilisation (NORMALE)

### Tâches
- [ ] Réduire warnings (200+ → <50)
- [ ] Documenter syscalls implémentés
- [ ] Tests de regression
- [ ] Benchmark context switch
- [ ] Mettre à jour ARCHITECTURE.md

---

## 🔮 Après v0.5.0 (Futur)

### v0.6.0 "Nebula Core"
- [ ] Multi-core SMP (APIC, trampoline)
- [ ] fork/exec/wait complets
- [ ] Pipes et redirection
- [ ] Shell basique

### v0.7.0 "Quantum Gate"
- [ ] Network stack TCP/IP
- [ ] virtio-net driver
- [ ] Socket API

### v0.8.0 "Dark Matter"
- [ ] Filesystems réels (FAT32, ext4)
- [ ] AHCI/NVMe drivers
- [ ] Persistence

### v1.0.0 "Singularity"
- [ ] IA Agents intégrés
- [ ] Fusion Rings IPC
- [ ] Zero-copy everywhere
- [ ] Production ready

---

## 📅 Planning Semaine

### Semaine 1 (2-8 décembre 2025)
| Jour | Objectif | Fichiers |
|------|----------|----------|
| Lun | Timer preemption | interrupts.rs, time/mod.rs |
| Mar | schedule() dans timer | scheduler.rs |
| Mer | Test context switch | windowed.rs |
| Jeu | Debug + mesures | - |
| Ven | mapper.rs début | mapper.rs |
| Sam | map_page impl | mapper.rs |
| Dim | Tests mémoire | - |

### Semaine 2 (9-15 décembre 2025)
| Jour | Objectif |
|------|----------|
| Lun-Mar | mmap/munmap réels |
| Mer-Jeu | tmpfs basique |
| Ven-Dim | Clavier PS/2 |

---

## 🛠️ Commandes Utiles

```bash
# Build complet
wsl bash -c "./build.sh"

# Test QEMU
qemu-system-x86_64 -cdrom build/exo_os.iso -m 256M -serial stdio

# Debug avec logs
qemu-system-x86_64 -cdrom build/exo_os.iso -m 256M -serial file:serial.log -d int -D qemu.log

# Voir serial log
cat serial.log

# Clean build
rm -rf target build && ./build.sh
```

---

## 📈 Métriques de Succès v0.5.0

| Métrique | Objectif | Actuel |
|----------|----------|--------|
| Context switch | <500 cycles | 🔴 N/A |
| Preemption | Automatique | 🔴 Non |
| Threads actifs | 3+ | ✅ 3 |
| Uptime stable | >5 min | ✅ ∞ |
| Userspace | 1 process | 🔴 Non |

---

**Légende:**
- ✅ Terminé
- 🟡 En cours / Partiel
- 🔴 Non commencé
- 🔴 CRITIQUE - Bloquant
- 🟠 HAUTE - Important
- 🟡 MOYENNE - Nécessaire
- 🟢 NORMALE - Nice to have
- 🔮 FUTUR - Après v0.5.0
