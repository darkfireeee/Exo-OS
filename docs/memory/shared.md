# 🤝 Shared Memory

## Vue d'ensemble

La mémoire partagée permet le transfert zero-copy entre processus pour l'IPC haute performance.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Shared Memory Region                      │
├─────────────────────────────────────────────────────────────┤
│  Process A           Shared Pages        Process B          │
│  ┌─────────┐        ┌───────────┐       ┌─────────┐        │
│  │ Virtual │───────►│ Physical  │◄──────│ Virtual │        │
│  │ 0x1000  │        │ Frames    │       │ 0x2000  │        │
│  └─────────┘        └───────────┘       └─────────┘        │
│      │                   ▲                   │              │
│      └───────────────────┴───────────────────┘              │
│                    Same Physical Memory                      │
└─────────────────────────────────────────────────────────────┘
```

## API

### Création

```rust
// Créer une région partagée
let region = SharedMemory::create(
    "ipc_buffer",
    4096,  // Taille
    SharedFlags::READ | SharedFlags::WRITE,
)?;

// Obtenir un handle
let handle = region.handle();
```

### Mapping

```rust
// Dans Process A
let addr_a = region.map(None, SharedFlags::READ | SharedFlags::WRITE)?;

// Dans Process B (via handle transmis)
let region = SharedMemory::open(handle)?;
let addr_b = region.map(None, SharedFlags::READ)?;
```

### Transfert Zero-Copy

```rust
// Écrire dans A
unsafe {
    ptr::write(addr_a as *mut Message, message);
}

// Lire dans B (même mémoire physique!)
let message = unsafe {
    ptr::read(addr_b as *const Message)
};
```

## Pool de Buffers

### Structure

```rust
pub struct SharedPool {
    /// Buffers pré-alloués
    buffers: Vec<SharedBuffer>,
    
    /// Free list
    free_list: Mutex<VecDeque<usize>>,
    
    /// Taille de chaque buffer
    buffer_size: usize,
}

pub struct SharedBuffer {
    pub ptr: *mut u8,
    pub size: usize,
    pub ref_count: AtomicUsize,
}
```

### API Pool

```rust
// Obtenir un buffer
let buffer = pool.acquire()?;

// Utiliser
unsafe {
    ptr::copy_nonoverlapping(data.as_ptr(), buffer.ptr, data.len());
}

// Envoyer via IPC (transfert de ownership)
ipc_send(channel, buffer.ptr, data.len())?;

// Le récepteur release
pool.release(buffer);
```

## Reference Counting

```rust
impl SharedBuffer {
    pub fn retain(&self) {
        self.ref_count.fetch_add(1, Ordering::Acquire);
    }
    
    pub fn release(&self) -> bool {
        let old = self.ref_count.fetch_sub(1, Ordering::Release);
        if old == 1 {
            fence(Ordering::Acquire);
            true  // Dernier référent, peut libérer
        } else {
            false
        }
    }
}
```

## Synchronisation

### Futex sur Shared Memory

```rust
// Dans la région partagée
#[repr(C)]
struct SharedSync {
    futex: AtomicU32,
    // ... données
}

// Process A: attendre
futex_wait(&shared.futex, expected_value)?;

// Process B: réveiller
shared.futex.store(new_value, Ordering::Release);
futex_wake(&shared.futex, 1)?;
```

## Intégration IPC

```rust
// IPC zero-copy avec shared memory
pub fn send_zerocopy(channel: &Channel, data: &[u8]) -> Result<()> {
    // Allouer depuis le pool partagé
    let buffer = SHARED_POOL.acquire(data.len())?;
    
    // Copier les données
    unsafe {
        ptr::copy_nonoverlapping(data.as_ptr(), buffer.ptr, data.len());
    }
    
    // Envoyer juste le pointeur + taille
    channel.send_ptr(buffer.ptr, data.len())
}

pub fn recv_zerocopy(channel: &Channel) -> Result<SharedBuffer> {
    // Recevoir le pointeur
    let (ptr, size) = channel.recv_ptr()?;
    
    // Retourner le buffer (ownership transféré)
    Ok(SharedBuffer::from_raw(ptr, size))
}
```
