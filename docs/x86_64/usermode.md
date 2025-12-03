# User Mode Transition Documentation

## Overview

Le module `usermode` gère la transition du kernel (Ring 0) vers l'espace utilisateur (Ring 3).

## Architecture x86_64 Privilege Levels

```
Ring 0 (Kernel)  ─────────────────────────────────────
   │                                              │
   │ SYSCALL/SYSRET ou INT/IRET                   │
   ▼                                              ▲
Ring 3 (User)    ─────────────────────────────────────
```

## Composants

### 1. GDT (Global Descriptor Table)

Segments configurés dans `gdt.rs`:

| Selector | Description | DPL |
|----------|-------------|-----|
| 0x08 | Kernel Code | 0 |
| 0x10 | Kernel Data | 0 |
| 0x18 | User Code | 3 |
| 0x20 | User Data | 3 |

### 2. TSS (Task State Segment)

```rust
struct TaskStateSegment {
    rsp0: u64,      // Stack pour Ring 0 (syscalls/interrupts)
    rsp1: u64,      // Non utilisé
    rsp2: u64,      // Non utilisé
    ist: [u64; 7],  // Interrupt Stack Table
}
```

**RSP0** est critique : quand le CPU passe de Ring 3 à Ring 0 (syscall ou interrupt), il charge automatiquement RSP depuis TSS.rsp0.

### 3. UserContext

```rust
#[repr(C)]
struct UserContext {
    // Registres généraux (sauvés/restaurés)
    r15, r14, r13, r12, r11, r10, r9, r8: u64,
    rbp, rdi, rsi, rdx, rcx, rbx, rax: u64,
    
    // Interrupt frame (pour IRETQ)
    rip: u64,      // Instruction pointer
    cs: u64,       // Code segment (0x1B = user code | 3)
    rflags: u64,   // Flags (IF=1 pour interrupts)
    rsp: u64,      // Stack pointer
    ss: u64,       // Stack segment (0x23 = user data | 3)
}
```

## Méthodes de Transition

### 1. IRETQ (Initial Entry)

Utilisé pour la première entrée en user mode :

```rust
pub unsafe fn jump_to_usermode(context: &UserContext) -> ! {
    asm!(
        // Restaurer registres...
        
        // Préparer stack pour IRETQ
        "push ss",      // Stack Segment
        "push rsp",     // Stack Pointer
        "push rflags",  // Flags
        "push cs",      // Code Segment
        "push rip",     // Instruction Pointer
        
        // Transition vers Ring 3
        "iretq",
    );
}
```

### 2. SYSRET (Fast Path)

Utilisé pour retour de syscall (plus rapide) :

```rust
pub unsafe fn sysret_to_usermode(
    rip: u64,
    rsp: u64,
    rflags: u64,
    rax: u64,  // Return value
) -> ! {
    asm!(
        "mov rcx, {rip}",    // RIP dans RCX
        "mov r11, {rflags}", // RFLAGS dans R11
        "mov rsp, {rsp}",
        "mov rax, {rax}",
        "sysretq",           // -> Ring 3
    );
}
```

## Flux d'Exécution

### Démarrage d'un Processus

```
1. Charger ELF (loader::load_elf)
2. Créer address space (memory::create_address_space)
3. Mapper segments (.text, .data, .bss)
4. Allouer stack utilisateur
5. Configurer TSS.rsp0 (kernel stack pour ce thread)
6. Créer UserContext avec entry_point
7. jump_to_usermode(&context)
```

### Syscall

```
User: syscall
  │
  ▼
Kernel:
  1. CPU sauvegarde RIP→RCX, RFLAGS→R11
  2. CPU charge RSP depuis TSS.rsp0
  3. CPU charge CS/SS kernel
  4. syscall_entry() dispatch
  5. Handler exécute
  6. sysret_to_usermode()
  │
  ▼
User: continue
```

### Interrupt

```
User: (interrupt arrives)
  │
  ▼
Kernel:
  1. CPU push SS, RSP, RFLAGS, CS, RIP
  2. CPU charge RSP depuis TSS.rsp0
  3. Handler exécute
  4. iretq (restaure context)
  │
  ▼
User: continue
```

## Configuration

### Initialisation

```rust
// Au boot
gdt::init();          // Configure GDT avec segments user
tss::init();          // Configure TSS avec RSP0
syscall::init();      // Configure MSRs pour SYSCALL

// Par thread
fn spawn_user_thread(entry: VirtualAddress, stack: VirtualAddress) {
    // Allouer kernel stack pour ce thread
    let kernel_stack = allocate_kernel_stack(16 * 1024);
    
    // Configurer TSS.rsp0 pour ce thread
    unsafe { tss::set_rsp0(kernel_stack.top()); }
    
    // Créer contexte
    let ctx = UserContext::new(entry, stack);
    ctx.set_args(argc, argv, envp);
    
    // Go!
    unsafe { jump_to_usermode(&ctx); }
}
```

## Sécurité

### SMAP/SMEP

- **SMEP** (Supervisor Mode Execution Prevention) : Le kernel ne peut pas exécuter du code user
- **SMAP** (Supervisor Mode Access Prevention) : Le kernel ne peut pas accéder aux données user sans STAC/CLAC

### Isolation

- Chaque processus a son propre CR3 (page tables)
- Les pages kernel sont marquées supervisor-only
- Les pages user ont le bit U/S=1

## Performance

| Opération | Cycles approximatifs |
|-----------|---------------------|
| IRETQ | ~40 cycles |
| SYSRET | ~20 cycles |
| SYSCALL entry | ~25 cycles |

SYSRET est ~2x plus rapide que IRETQ pour le retour de syscall.
