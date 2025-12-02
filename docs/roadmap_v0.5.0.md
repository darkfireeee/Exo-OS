# 🚀 Exo-OS v0.5.0 - Roadmap "Stellar Engine"

**Date de début**: 2 décembre 2025  
**État de départ**: v0.4.0 (~55% fonctionnel réel)  
**Objectif v0.5.0**: 75%+ fonctionnel avec scheduler et VFS opérationnels  
**Durée estimée**: 6-8 semaines

---

## 📊 État Réel de Départ

### Ce qui FONCTIONNE maintenant
| Composant | État | Description |
|-----------|------|-------------|
| Boot | ✅ 100% | GRUB → boot.asm → C stub → Rust |
| Heap Allocator | ✅ 100% | 10MB, linked-list |
| Frame Allocator | ✅ 100% | Bitmap, 512MB supporté |
| GDT/IDT | ✅ 100% | Tables chargées |
| PIC/PIT | ✅ 100% | Timer 100Hz |
| Serial | ✅ 100% | COM1 output |
| VGA Text | ✅ 100% | 80x25, splash |
| 3-Queue Scheduler | ⚠️ Structure | Hot/Normal/Cold OK |
| Thread Creation | ⚠️ Partiel | spawn() OK, switch ❌ |

### Ce qui NE FONCTIONNE PAS
| Composant | Problème |
|-----------|----------|
| Context Switch | windowed.rs est VIDE! |
| Page Table | mapper.rs non implémenté |
| Multi-core | trampoline.asm incompatible |
| Keyboard | IRQ1 non géré |
| Filesystem | Aucun FS monté |
| Syscalls | ~70% sont des stubs |

---

## 🎯 Objectifs v0.5.0

### Phase 1: Scheduler Fonctionnel (Semaines 1-2)
**Objectif**: Threads qui s'exécutent vraiment avec preemption

#### 1.1 Context Switch Réel
```rust
// Fichier: kernel/src/scheduler/switch/windowed.rs
// État actuel: VIDE (5 lignes de stubs)
// Action: Implémenter liaison avec windowed_context_switch.S

Tâches:
[x] Analyser windowed_context_switch.S existant
[ ] Créer FFI extern "C" pour les fonctions ASM
[ ] Implémenter windowed_context_switch()
[ ] Implémenter windowed_init_context()
[ ] Tester avec 2 threads qui alternent
[ ] Mesurer cycles (objectif: <500)
```

#### 1.2 Timer Preemption
```rust
// Fichier: kernel/src/arch/x86_64/interrupts.rs
// Action: Appeler scheduler depuis timer IRQ

Tâches:
[ ] Modifier timer_handler pour appeler schedule()
[ ] Implémenter quantum expiration
[ ] Tester preemption automatique
```

#### 1.3 Thread Blocking/Unblocking
```rust
// Fichier: kernel/src/scheduler/core/scheduler.rs

Tâches:
[ ] Implémenter block_current()
[ ] Implémenter unblock(thread_id)
[ ] Ajouter waiting queue
[ ] Tester sleep/wake pattern
```

**Critères de succès Phase 1**:
- [ ] 3 threads tournent en round-robin
- [ ] Timer tick déclenche context switch
- [ ] Console affiche counters des 3 threads
- [ ] Pas de crash après 1 minute

---

### Phase 2: Mémoire Virtuelle (Semaines 2-3)
**Objectif**: mmap/munmap fonctionnels

#### 2.1 Page Table Manipulation
```rust
// Fichier: kernel/src/memory/virtual_mem/mapper.rs
// État actuel: ~10% (structures seulement)

Tâches:
[ ] Implémenter map_page(virt, phys, flags)
[ ] Implémenter unmap_page(virt)
[ ] Implémenter translate(virt) -> Option<phys>
[ ] Flush TLB après modifications
[ ] Tester mapping/unmapping
```

#### 2.2 mmap Réel
```rust
// Fichier: kernel/src/memory/mmap.rs
// État actuel: ~40% (crée structures, ne mappe pas)

Tâches:
[ ] Appeler mapper.map_page() dans mmap()
[ ] Appeler mapper.unmap_page() dans munmap()
[ ] Gérer protections (R/W/X)
[ ] Tester allocation anonyme
```

#### 2.3 brk/sbrk
```rust
// Fichier: kernel/src/syscall/handlers/memory.rs

Tâches:
[ ] Implémenter sys_brk() réel
[ ] Étendre/réduire heap
[ ] Tester avec allocation userspace
```

**Critères de succès Phase 2**:
- [ ] mmap alloue vraiment des pages
- [ ] munmap libère les pages
- [ ] Pas de page fault inattendu

---

### Phase 3: VFS Minimal (Semaines 3-4)
**Objectif**: Lire des fichiers depuis initramfs

#### 3.1 tmpfs Fonctionnel
```rust
// Fichier: kernel/src/fs/vfs/tmpfs.rs
// État actuel: ~10% (stub)

Tâches:
[ ] Implémenter TmpfsInode
[ ] Implémenter create_file(), create_dir()
[ ] Implémenter read(), write()
[ ] Monter sur /
```

#### 3.2 Initramfs (TarFS)
```rust
// Fichier: kernel/src/fs/tarfs/ (nouveau)

Tâches:
[ ] Parser header TAR
[ ] Extraire fichiers en mémoire
[ ] Monter comme /initrd
[ ] Créer /bin/init minimal
```

#### 3.3 File Operations
```rust
// Fichier: kernel/src/syscall/handlers/io.rs

Tâches:
[ ] Compléter sys_open() avec VFS
[ ] Compléter sys_read() avec inode
[ ] Compléter sys_close()
[ ] Tester lecture fichier
```

**Critères de succès Phase 3**:
- [ ] open("/etc/hostname") retourne FD valide
- [ ] read() retourne contenu
- [ ] close() libère ressources

---

### Phase 4: Drivers Essentiels (Semaines 4-5)
**Objectif**: Clavier fonctionnel

#### 4.1 Keyboard Driver
```rust
// Fichier: kernel/src/drivers/input/keyboard.rs (nouveau)

Tâches:
[ ] Handler IRQ1
[ ] Scan code -> ASCII (US layout)
[ ] Buffer circulaire 256 chars
[ ] Exposer via /dev/tty
[ ] Tester input console
```

#### 4.2 DevFS
```rust
// Fichier: kernel/src/fs/devfs/

Tâches:
[ ] /dev/null (discard)
[ ] /dev/zero (zeros)
[ ] /dev/tty (keyboard)
[ ] /dev/console (serial)
```

**Critères de succès Phase 4**:
- [ ] Appuyer sur touche → caractère affiché
- [ ] read("/dev/tty") retourne input
- [ ] echo fonctionnel

---

### Phase 5: Premier Userspace (Semaines 5-6)
**Objectif**: Exécuter /bin/init

#### 5.1 ELF Loader
```rust
// Fichier: kernel/src/posix_x/elf/loader.rs

Tâches:
[ ] Parser ELF64 header
[ ] Charger segments .text, .data
[ ] Configurer .bss
[ ] Créer stack userspace
[ ] Préparer entry point
```

#### 5.2 User Mode Transition
```rust
// Fichier: kernel/src/arch/x86_64/usermode.rs (nouveau)

Tâches:
[ ] Configurer TSS
[ ] Sauvegarder contexte kernel
[ ] iretq vers Ring 3
[ ] Syscall return path
```

#### 5.3 /bin/init
```c
// Fichier: userspace/init/main.c

// Programme minimal
int main() {
    sys_write(1, "Exo-OS v0.5.0 - Userspace!\n", 28);
    while(1) { sys_pause(); }
}
```

**Critères de succès Phase 5**:
- [ ] ELF chargé en mémoire
- [ ] Jump to user mode sans crash
- [ ] sys_write affiche message
- [ ] sys_exit termine proprement

---

### Phase 6: Stabilisation (Semaines 6-8)
**Objectif**: Système stable pour démo

#### 6.1 Tests
```rust
Tâches:
[ ] Test unitaires scheduler
[ ] Test memory leaks
[ ] Test stress (100+ threads)
[ ] Test boot 100x sans crash
```

#### 6.2 Documentation
```
Tâches:
[ ] Mettre à jour ARCHITECTURE.md
[ ] Créer USERSPACE_GUIDE.md
[ ] Documenter syscalls supportés
```

#### 6.3 Multi-core (Optionnel)
```rust
Tâches:
[ ] Fixer trampoline.asm
[ ] Réactiver SMP
[ ] Tester sur 4 cores
```

---

## 📅 Planning Détaillé

| Semaine | Focus | Livrables |
|---------|-------|-----------|
| S1 | Context Switch | windowed.rs fonctionnel |
| S2 | Preemption | Timer-based scheduling |
| S3 | Memory | mmap/munmap réels |
| S4 | VFS | tmpfs + initramfs |
| S5 | Drivers | Keyboard input |
| S6 | Userspace | /bin/init exécuté |
| S7-8 | Stabilisation | Tests, docs, fixes |

---

## 🔧 Démarrage Immédiat (Aujourd'hui)

### Action 1: Corriger windowed.rs
```rust
// kernel/src/scheduler/switch/windowed.rs
// Remplacer le stub par liaison ASM

extern "C" {
    fn windowed_context_switch(old_ctx: *mut u64, new_ctx: *const u64);
    fn windowed_init_context(ctx: *mut u64, entry: u64, stack: u64);
}

pub fn switch_to(old: &mut ThreadContext, new: &ThreadContext) {
    unsafe {
        windowed_context_switch(
            old as *mut _ as *mut u64,
            new as *const _ as *const u64
        );
    }
}
```

### Action 2: Appeler depuis scheduler
```rust
// kernel/src/scheduler/core/scheduler.rs
// Dans switch_to_thread()

use crate::scheduler::switch::windowed;
windowed::switch_to(&mut old_ctx, &new_ctx);
```

### Action 3: Timer preemption
```rust
// kernel/src/arch/x86_64/interrupts.rs
// Dans timer_handler

if tick % QUANTUM == 0 {
    crate::scheduler::schedule();
}
```

---

## 📈 Métriques de Succès v0.5.0

| Métrique | Objectif | Actuel |
|----------|----------|--------|
| Kernel fonctionnel | 75% | ~55% |
| Context switches/sec | 10,000+ | 0 |
| Threads supportés | 100+ | 3 (stables) |
| Syscalls fonctionnels | 50+ | ~15 |
| Fichiers lisibles | 10+ | 0 |
| Userspace | 1 programme | 0 |
| Crash-free uptime | 1 heure | ~30 sec |

---

## 🎉 Definition of Done v0.5.0

- [ ] `cargo build --release` compile sans erreur
- [ ] Boot QEMU sans crash
- [ ] 3+ threads en round-robin
- [ ] Timer preemption fonctionne
- [ ] mmap/munmap créent/libèrent des pages
- [ ] Lecture fichier depuis VFS
- [ ] Keyboard input fonctionnel
- [ ] /bin/init exécuté en userspace
- [ ] Documentation à jour
- [ ] Pas de regression v0.4.0
