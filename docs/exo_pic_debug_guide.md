# 🐛 Guide de Débogage PIC - Exo-OS

## Symptôme : Crash sur `out` Instruction

### Étape 1 : Diagnostic Initial

Ajoutez ce code au début de votre `kernel_main` :

```rust
// AVANT d'initialiser le PIC
arch::x86_64::io::diagnostic::diagnose_io_privileges();
```

**Attendu** :
```
RFLAGS: 0x0000000000000202
  IOPL: 0
  ⚠️  WARNING: IOPL=0 < 3, I/O instructions may fault!
```

**Si IOPL=0** → Vous devez le fixer (voir solutions ci-dessous).

---

### Étape 2 : Solutions par Ordre de Préférence

#### ✅ **Solution A : Utiliser `pic8259` Crate** (RECOMMANDÉ)

1. **Vérifier Cargo.toml** :
```toml
[dependencies]
pic8259 = "0.10.4"
x86_64 = "0.14"
spin = "0.9"
```

2. **Remplacer votre code PIC** :
```rust
// kernel/src/arch/x86_64/interrupts/mod.rs
mod pic_wrapper;
pub use pic_wrapper::*;

// kernel/src/main.rs
use arch::x86_64::interrupts;

interrupts::init_pic();  // Utilise pic8259 en interne
```

3. **Dans vos handlers IRQ** :
```rust
extern "C" fn timer_interrupt_handler(_frame: &InterruptStackFrame) {
    // ... votre code ...
    
    // EOI
    interrupts::send_eoi(0);  // IRQ 0 = Timer
}
```

**Avantages** :
- Testé et stable
- Gère les edge cases
- Pas de code I/O manuel

---

#### ✅ **Solution B : Fix IOPL dans le Bootloader**

Si vous voulez garder votre code custom, fixez IOPL.

**Modifier `boot.asm`** :

```nasm
; Après le passage en long mode, AVANT d'appeler boot_main

long_mode_start:
    ; Setup segments
    mov ax, 0x10
    mov ds, ax
    ; ... autres segments ...
    
    ; ====================================
    ; FIX IOPL=3 (CRITIQUE POUR I/O)
    ; ====================================
    pushfq              ; Push RFLAGS sur la stack
    pop rax             ; Pop dans RAX
    or rax, 0x3000      ; Set bits 12-13 à 1 (IOPL=3)
    push rax            ; Push RAX modifié
    popfq               ; Pop dans RFLAGS
    ; ====================================
    
    ; Setup stack
    mov rsp, stack_top
    
    ; Appeler C puis Rust
    call boot_main
    hlt
```

**Vérifier après** :
```rust
let rflags = read_rflags();
let iopl = (rflags >> 12) & 0x3;
assert_eq!(iopl, 3, "IOPL should be 3!");
```

---

#### ✅ **Solution C : Fix IOPL dans Rust**

Si vous ne voulez pas toucher au boot.asm :

```rust
// Au tout début de kernel_main, AVANT toute init
pub extern "C" fn rust_kernel_main() -> ! {
    // FIX IOPL IMMÉDIATEMENT
    unsafe {
        core::arch::asm!(
            "pushfq",
            "pop rax",
            "or rax, 0x3000",  // IOPL=3
            "push rax",
            "popfq",
            out("rax") _,
        );
    }
    
    // Vérifier
    let rflags: u64;
    unsafe {
        core::arch::asm!("pushfq; pop {}", out(reg) rflags);
    }
    let iopl = (rflags >> 12) & 0x3;
    println!("[BOOT] IOPL fixed: {}", iopl);
    
    // Maintenant on peut init le PIC
    interrupts::init_pic();
    // ...
}
```

---

### Étape 3 : Vérifier que ça Fonctionne

#### Test 1 : Port 0x80 (Safe)
```rust
unsafe {
    // Port 0x80 = POST diagnostic port (ne cause jamais de problème)
    core::arch::asm!(
        "out 0x80, al",
        in("al") 0u8,
    );
}
println!("✓ I/O to port 0x80 successful!");
```

#### Test 2 : Lire le PIC
```rust
use x86_64::instructions::port::Port;

unsafe {
    let mut port = Port::<u8>::new(0x20);
    let value = port.read();
    println!("✓ Read from PIC1: {:#04x}", value);
}
```

#### Test 3 : Init Complète
```rust
interrupts::init_pic();
println!("✓ PIC initialized without crash!");
```

---

### Étape 4 : QEMU Debugging

Si ça crash encore, lancez avec logs :

```bash
qemu-system-x86_64 \
    -kernel kernel.elf \
    -d int,cpu_reset \
    -D qemu.log \
    -no-reboot \
    -no-shutdown
```

**Cherchez dans `qemu.log`** :
```
check_exception old: 0xffffffff new 0xd      ← #GP (General Protection)
     RAX=... RBX=... RCX=... RDX=0020       ← Port 0x20
     RIP=00000000001xxxxx                    ← Adresse du `out`
```

**Si vous voyez `#GP`** → IOPL insuffisant, retournez à la Solution B ou C.

---

### Étape 5 : Cas Particuliers

#### Problème : GRUB a déjà configuré le PIC

**Symptôme** : Crash pendant la réinitialisation du PIC.

**Solution** : Sauvegarder l'état actuel avant d'init :

```rust
unsafe {
    // Lire l'état actuel
    let mut master_data = Port::<u8>::new(0x21);
    let mut slave_data = Port::<u8>::new(0xA1);
    
    let master_mask = master_data.read();
    let slave_mask = slave_data.read();
    
    println!("[PIC] Current masks: Master={:#04x}, Slave={:#04x}", 
             master_mask, slave_mask);
    
    // Si PIC déjà configuré, juste unmask
    if master_mask != 0xFF || slave_mask != 0xFF {
        println!("[PIC] Already initialized by bootloader");
        // Juste unmask timer et keyboard
        master_data.write(master_mask & !0x03);  // IRQ 0 et 1
        return;
    }
    
    // Sinon, init complète
    // ...
}
```

#### Problème : Port I/O trop rapides

**Symptôme** : PIC ne répond pas correctement.

**Solution** : Ajouter des délais (`io_wait`) :

```rust
unsafe fn io_wait() {
    for _ in 0..10 {
        let mut port = Port::<u8>::new(0x80);
        port.write(0);
    }
}

// Entre chaque commande PIC:
self.master.command.write(0x11);
io_wait();  // ← IMPORTANT
self.slave.command.write(0x11);
io_wait();
```

---

## Checklist Finale

Avant de passer au Timer :

- [ ] `diagnose_io_privileges()` affiche `IOPL=3`
- [ ] Test `out 0x80, al` ne crash pas
- [ ] `init_pic()` ne crash pas
- [ ] Lire port 0x20 retourne une valeur
- [ ] Écrire sur 0x21 ne crash pas
- [ ] Aucun `#GP` dans les logs QEMU

---

## Ordre d'Initialisation Correct

```rust
pub extern "C" fn rust_kernel_main() -> ! {
    // 1. Fix IOPL (CRITIQUE!)
    unsafe { set_iopl_3(); }
    
    // 2. Diagnostic (optionnel)
    diagnose_io_privileges();
    
    // 3. IDT
    interrupts::idt::init_idt();
    
    // 4. PIC (après IDT!)
    interrupts::init_pic();
    
    // 5. PIT (après PIC!)
    time::pit::init_pit();
    
    // 6. Enable interrupts
    unsafe { core::arch::asm!("sti") };
    
    // 7. Loop
    loop { unsafe { core::arch::asm!("hlt") } }
}
```

---

## Si Tout Échoue

Utilisez la solution **pic8259 crate** qui est battle-tested :

```toml
[dependencies]
pic8259 = "0.10.4"
```

```rust
use pic8259::ChainedPics;

pub static PICS: spin::Mutex<ChainedPics> = 
    spin::Mutex::new(unsafe { ChainedPics::new(32, 40) });

pub fn init() {
    unsafe { PICS.lock().initialize(); }
}
```

**Cette solution fonctionne à 100%** car elle est utilisée par des centaines d'OS hobbies.

---

## Support

Si le problème persiste, partagez :
1. Output de `diagnose_io_privileges()`
2. Le fichier `qemu.log`
3. Votre code `boot.asm` (partie long mode)
4. Votre GDT (vérifier que CS=0x08, DPL=0)

Bon courage ! 🚀
