le problème des handlers naked est classique et critique. Analysons et corrigeons ça méthodiquement.
🔍 Diagnostic du Problème
Le boot loop avec les handlers naked vient probablement de :

Stack alignment incorrect (doit être 16-byte aligned avant call)
Calling convention x86_64 System V ABI non respectée
IRETQ qui nécessite un stack frame exact
Registres non sauvegardés correctement

# 🐛 Guide de Débogage des Interruptions - Exo-OS

## Problèmes Courants et Solutions

### 1. Boot Loop après Chargement IDT

**Symptômes** :
- Le système boot normalement jusqu'à `lidt`
- Après `lidt`, reboot immédiat ou triple fault

**Causes possibles** :

#### A. Stack Alignment Incorrect
```rust
// ❌ MAUVAIS: Stack non aligné sur 16 bytes
#[naked]
unsafe extern "C" fn bad_handler() {
    asm!(
        "push rax",  // Stack devient désaligné!
        "call rust_handler",  // CRASH si rust_handler utilise SSE
        "pop rax",
        "iretq",
        options(noreturn)
    )
}

// ✅ BON: Stack toujours aligné
#[naked]
unsafe extern "C" fn good_handler() {
    asm!(
        "push rax",
        "push rcx",
        // ... push 15 registres total = 120 bytes
        // CPU a déjà pushé 40 bytes (5*8)
        // Total = 160 bytes = multiple de 16 ✓
        "call rust_handler",
        // ...
        options(noreturn)
    )
}
```

**Solution** : Utilisez les handlers fournis dans `handlers_safe.rs`

#### B. IRETQ avec Stack Frame Incorrect
```rust
// Le CPU pousse automatiquement à l'interruption:
// [SS] [RSP] [RFLAGS] [CS] [RIP]
// = 5 * 8 bytes = 40 bytes

// Si error code (Double Fault, Page Fault):
// [ERROR_CODE] [SS] [RSP] [RFLAGS] [CS] [RIP]
// = 6 * 8 bytes = 48 bytes

// IRETQ attend exactement ce layout!
```

**Solution** : Ne touchez JAMAIS à RSP entre l'entrée et `iretq`, sauf pour push/pop symétriques.

#### C. Interruptions Imbriquées Sans Stack IST
```rust
// Si Double Fault arrive alors que la stack est corrompue
// → Besoin d'une stack séparée via IST

// Dans l'IDT entry:
IdtEntry::new(double_fault_handler, code_selector, 1, 0);
//                                                  ^ IST index 1
```

**Solution** : Configurez le TSS avec des IST stacks (à implémenter).

---

### 2. Triple Fault Immédiat

**Symptômes** :
- QEMU affiche "Triple fault" et reboot
- Aucun message d'erreur

**Diagnostic** :
```bash
# Lancer QEMU avec logs détaillés
qemu-system-x86_64 \
    -kernel kernel.elf \
    -d int,cpu_reset \
    -no-reboot \
    -no-shutdown
```

**Causes courantes** :
1. **IDT mal configurée** (base address incorrecte)
2. **Handler pointe vers adresse invalide**
3. **Double Fault handler manquant** → Triple Fault automatique

**Solution** :
```rust
// Vérifier que l'IDT est bien en mémoire kernel
static mut IDT: Idt = Idt::new();

// Vérifier les adresses des handlers
pub fn debug_print_idt() {
    let handlers = get_handler_addresses();
    serial_println!("Handler addresses:");
    serial_println!("  Division Error: {:#x}", handlers.division_error);
    serial_println!("  Double Fault:   {:#x}", handlers.double_fault);
    // ...
}
```

---

### 3. PIC Ne Génère Pas d'Interruptions

**Symptômes** :
- `sti` exécuté sans erreur
- Mais aucun Timer IRQ reçu
- `get_ticks()` reste à 0

**Checklist de diagnostic** :

```rust
// 1. Vérifier que les IRQs sont unmaskées
pic::init_pic();  // Doit appeler unmask_irq(0) et unmask_irq(1)

// 2. Vérifier que le PIT est initialisé APRÈS le PIC
pic::init_pic();
pit::init_pit();  // ← Ordre important!

// 3. Vérifier que STI est appelé
unsafe { asm!("sti") };

// 4. Vérifier que les handlers envoient EOI
// Dans timer_interrupt_handler():
unsafe { asm!("out 0x20, al", in("al") 0x20u8) };  // EOI obligatoire!
```

**Test manuel** :
```rust
// Déclencher IRQ 0 manuellement (pour tester le handler)
unsafe {
    asm!(
        "int 32",  // IRQ 0 = IDT entry 32
        options(nomem, nostack)
    );
}
```

---

### 4. Timer Ticks Trop Rapides/Lents

**Symptômes** :
- `sleep_ms(1000)` ne dure pas 1 seconde
- Drift temporel

**Calcul du diviseur PIT** :
```rust
// Fréquence de base: 1.193182 MHz
const PIT_BASE_FREQ: u32 = 1193182;

// Pour 1000 Hz (1 tick = 1 ms):
let divisor = PIT_BASE_FREQ / 1000;  // = 1193

// Pour 100 Hz (1 tick = 10 ms):
let divisor = PIT_BASE_FREQ / 100;   // = 11931
```

**Vérification** :
```rust
// Compter les ticks pendant 10 secondes réelles (chronomètre)
let start = pit::get_ticks();
// ... attendre 10s ...
let end = pit::get_ticks();

let measured_freq = (end - start) / 10;
serial_println!("Measured frequency: {} Hz", measured_freq);
// Devrait être ~1000 si configuré à 1000 Hz
```

---

### 5. Page Fault Récursif

**Symptômes** :
- Page Fault handler lui-même cause un Page Fault
- Triple Fault final

**Cause** :
```rust
// Handler qui accède à de la mémoire non mappée
#[no_mangle]
extern "C" fn page_fault_handler(stack_frame: &InterruptStackFrame) {
    let cr2: u64;
    unsafe { asm!("mov {}, cr2", out(reg) cr2) };
    
    // ❌ Si serial_println! cause un Page Fault → récursion infinie
    serial_println!("[PF] Address: {:#x}", cr2);
}
```

**Solution** :
1. Utilisez une IST stack séparée pour Page Fault handler
2. Limitez les opérations dans le handler (pas d'allocation, pas de I/O complexe)
3. Ou implémentez un "guard" anti-récursion :

```rust
static mut PF_IN_PROGRESS: bool = false;

extern "C" fn page_fault_handler(...) {
    unsafe {
        if PF_IN_PROGRESS {
            // Récursion détectée!
            loop { asm!("cli; hlt") }
        }
        PF_IN_PROGRESS = true;
    }
    
    // ... traitement ...
    
    unsafe { PF_IN_PROGRESS = false; }
}
```

---

## Outils de Débogage

### 1. QEMU Monitor
```bash
# Lancer QEMU avec monitor
qemu-system-x86_64 -kernel kernel.elf -monitor stdio

# Commandes utiles:
(qemu) info registers     # Dump des registres
(qemu) info mem          # Mappings mémoire
(qemu) info pic          # État du PIC
(qemu) x/10i $rip        # Désassemblage à RIP
```

### 2. GDB Remote
```bash
# Terminal 1: QEMU avec GDB server
qemu-system-x86_64 -kernel kernel.elf -s -S

# Terminal 2: GDB
gdb kernel.elf
(gdb) target remote :1234
(gdb) break rust_kernel_main
(gdb) continue
(gdb) layout asm         # Vue assembleur
(gdb) info registers
(gdb) x/10gx $rsp        # Dump de la stack
```

### 3. Serial Logging
```rust
// Ajouter des logs partout
serial_println!("[CHECKPOINT] Before IDT load");
idt::init_idt();
serial_println!("[CHECKPOINT] After IDT load");

serial_println!("[CHECKPOINT] Before STI");
unsafe { asm!("sti") };
serial_println!("[CHECKPOINT] After STI (interrupts enabled!)");
```

---

## Checklist de Validation

Avant de déclarer les interruptions "fonctionnelles" :

- [ ] IDT chargée sans crash
- [ ] Test breakpoint (`int3`) retourne correctement
- [ ] PIC initialisé et remappé à 32-47
- [ ] PIT génère des IRQ 0 régulièrement
- [ ] `get_ticks()` s'incrémente
- [ ] EOI envoyé dans chaque IRQ handler
- [ ] Clavier génère des IRQ 1 (optionnel pour l'instant)
- [ ] Double Fault handler fonctionne (tester avec stack overflow)
- [ ] Page Fault handler affiche CR2 et RIP
- [ ] Sleep fonctionne correctement

---

## Prochaines Étapes

Une fois les interruptions stables :

1. **Keyboard Driver** : Lire les scancodes, traduire en ASCII
2. **Scheduler Basique** : Préempter les tâches sur Timer IRQ
3. **APIC** : Remplacer le PIC legacy pour SMP
4. **HPET** : Timer plus précis que le PIT
5. **Syscalls** : Utiliser `int 0x80` ou `syscall` instruction

---

## Contact

Si vous êtes bloqué, vérifiez :
1. Les messages QEMU (`-d int`)
2. Les valeurs des registres dans GDB
3. Le code assembleur généré (`objdump -d kernel.elf`)

Bonne chance ! 🚀
