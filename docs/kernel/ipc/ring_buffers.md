# Ring Buffers IPC

Les ring buffers sont le moteur de transport bas-niveau du module IPC. Tous les canaux (sync, async, MPMC, broadcast) s'appuient sur l'un de ces anneaux.

## Vue d'ensemble

```
ring/
├── spsc.rs       — SPSC ultra-rapide (1 producteur, 1 consommateur)
├── mpmc.rs       — MPMC lock-free (N producteurs, M consommateurs)
├── fusion.rs     — Fusion Ring (batching adaptatif EWA)
├── slot.rs       — Gestion générique de slots (SlotPool, SlotGuard)
├── batch.rs      — BufferBatch (accumule avant flush)
└── zerocopy.rs   — ZcRing (partage de pages physiques)
```

---

## SPSC Ring — `ring/spsc.rs`

### Description

Ring buffer single-producteur / single-consommateur ultra-rapide. Conçu pour le chemin chaud IPC : latence < 50 cycles en L1 cache.

### Structure

```rust
#[repr(align(64))]
struct CachePad(AtomicU64, [u8; 56]);  // 64 octets exacts

pub struct SpscRing {
    head: CachePad,  // lu par le producteur — ligne de cache dédiée (IPC-01)
    tail: CachePad,  // lu par le consommateur — ligne de cache dédiée (IPC-01)
    cells: UnsafeCell<[SlotCell; RING_SIZE]>,
}
```

**Règle IPC-01** : `head` et `tail` sont sur des lignes de cache distinctes (64 octets chacune par `CachePad`). Sans cet alignement, écrire `head` invalide la ligne de cache contenant `tail` sur les cœurs SMP voisins.

### Constantes

```rust
pub const SPSC_CAPACITY: usize = RING_SIZE;  // 16 slots
```

### API

```rust
impl SpscRing {
    pub const fn new() -> Self

    /// Push bloquant : attend une place libre.
    pub fn push_copy(&self, src: *const RingSlot) -> bool

    /// Pop bloquant : attend un message disponible.
    pub fn pop_into(&self, dst: *mut RingSlot) -> bool

    /// Versions non bloquantes (retour immédiat).
    pub fn try_push(&self, src: *const RingSlot) -> bool
    pub fn try_pop(&self, dst: *mut RingSlot) -> bool
}
```

### Accès aux cellules — Spectre v1

```rust
#[inline(always)]
fn cell_at(&self, idx: usize) -> *const SlotCell {
    // IPC-08 : array_index_nospec évite les accès spéculatifs hors-borne
    let safe_idx = array_index_nospec(idx, RING_SIZE);
    unsafe { &(*self.cells.get())[safe_idx] }
}
```

### Fast IPC path

```rust
// Appelées depuis core/fastcall_asm.s (IPC-07)
pub fn spsc_fast_write(ring: *mut SpscRing, slot: *const RingSlot) -> bool
pub fn spsc_fast_read(ring: *mut SpscRing, slot: *mut RingSlot) -> bool
```

---

## MPMC Ring — `ring/mpmc.rs`

### Description

Ring buffer multi-producteurs / multi-consommateurs sans verrous, basé sur l'algorithme à séquences de **Dmitry Vyukov**.

### Algorithme

Chaque cellule porte un `AtomicU64 sequence`. Le producteur :
1. Réserve un slot via `fetch_add` sur `enqueue_pos`
2. Compare la séquence de la cellule au slot attendu (CAS-like)
3. Écrit si et seulement si la séquence correspond

Le consommateur suit le même schéma en lecture.

```rust
pub struct MpmcCell {
    sequence: AtomicU64,
    data: UnsafeCell<RingSlot>,
}

pub struct MpmcRing {
    buffer: [MpmcCell; RING_SIZE],
    enqueue_pos: CachePad,  // ligne de cache producteurs
    dequeue_pos: CachePad,  // ligne de cache consommateurs
}
```

### API

```rust
impl MpmcRing {
    pub const fn new() -> Self
    pub fn push_copy(&self, src: *const RingSlot) -> bool
    pub fn pop_into(&self, dst: *mut RingSlot) -> bool
}
```

### Spectre v1

```rust
fn cell_at(&self, idx: usize) -> &MpmcCell {
    let safe_idx = array_index_nospec(idx, RING_SIZE);
    &self.buffer[safe_idx]  // IPC-08
}
```

---

## Fusion Ring — `ring/fusion.rs`

### Description

Ring hybride avec **batching adaptatif** anti-thundering herd. Combine un mode `Direct` (latence minimale) et un mode `Batch` (débit maximal).

### Algorithme EWA

La décision de flush est basée sur une Moyenne Mobile Exponentielle du débit :

```
ewa_throughput = 0.875 × ewa_throughput + 0.125 × current_throughput
threshold ∈ [1, FUSION_RING_SIZE / 2]
```

```rust
pub enum FusionMode { Direct, Batch }

pub struct FusionRing {
    inner:             SpscRing,
    mode:              FusionMode,
    pending_count:     usize,
    batch_threshold:   usize,      // threshold adaptatif
    msgs_since_last:   usize,
    ewa_throughput:    u64,
}
```

### Déclenchement du flush

```rust
// dans send() :
if self.pending_count >= self.batch_threshold
    || self.ring_utilization() > 75
{
    self.flush();
}
```

### Ajustement de threshold

```rust
fn adjust_batch_threshold(&mut self, observed_throughput: u64) {
    self.ewa_throughput = (7 * self.ewa_throughput + observed_throughput) / 8;
    // Ajuste threshold entre 1 et FUSION_RING_SIZE/2
}
```

**Règle IPC-06** : Le mécanisme EWA est obligatoire. Il empêche que N producteurs reveillent N fois le consommateur pour N messages individuels.

---

## Slot Ring — `ring/slot.rs`

### Description

Gestion générique de slots avec RAII via `SlotGuard`. Permet d'allouer un slot, d'y écrire, et de le relâcher automatiquement.

### Structures

```rust
pub struct SlotCell {
    sequence: AtomicU64,
    data: UnsafeCell<RingSlot>,
}

pub struct SlotPool {
    cells: [SlotCell; RING_SIZE],
    // ...
}

pub struct SlotGuard<'a> {
    pool:  &'a SlotPool,
    index: usize,
}

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        // Relâche le slot vers le pool
    }
}
```

### Spectre v1

```rust
fn cell_at(&self, idx: usize) -> &SlotCell {
    let safe_idx = array_index_nospec(idx, RING_SIZE);
    &self.cells[safe_idx]  // IPC-08
}
```

---

## Batch Buffer — `ring/batch.rs`

### Description

Accumule des messages dans un buffer local avant de les transférer en lot vers un ring cible. Réduit les barrières mémoire lors de transferts massifs.

### Structures

```rust
pub struct BatchEntry {
    data: RingSlot,
}

pub struct BatchBuffer {
    entries: [BatchEntry; RING_SIZE],
    count:   usize,
}

impl BatchBuffer {
    pub fn push_entry(&mut self, slot: &RingSlot) -> bool
    pub fn flush_to_ring(&mut self, ring: &SpscRing) -> usize
    pub fn is_full(&self) -> bool
    pub fn len(&self) -> usize
}
```

---

## Zero-Copy Ring — `ring/zerocopy.rs`

### Description

Ring of `ZeroCopyRef` : transfert de références à des buffers SHM sans copie des données. Chaque entrée du ring est une référence de 24 octets vers un buffer physique partagé.

### Structures

```rust
// core/transfer.rs — réexporté depuis zerocopy.rs
pub struct ZeroCopyRef {
    phys_addr: u64,    // adresse physique du buffer
    data_len:  u32,    // octets valides
    flags:     u32,    // PageFlags
}  // 16 octets + padding = 24 octets

pub struct ZeroCopyBuffer {
    refcount: AtomicU32,
    data_len: u32,
    data_ptr: *mut u8,
    flags:    PageFlags,
}

pub struct ZcRing {
    slots: [ZcSlot; RING_SIZE],
    head:  CachePad,
    tail:  CachePad,
}
```

### API

```rust
impl ZcRing {
    pub fn push_ref(&self, r: ZeroCopyRef) -> bool
    pub fn pop_ref(&self) -> Option<ZeroCopyRef>
}
```

### Spectre v1

```rust
fn slot_at(&self, idx: usize) -> &ZcSlot {
    let safe_idx = array_index_nospec(idx, RING_SIZE);
    &self.slots[safe_idx]  // IPC-08
}
```

### Chemin zero-copy

```
Producteur                     Consommateur
─────────────────────────────────────────────
1. Alloue ZeroCopyBuffer (SHM)
2. Écrit données dans buffer
3. push_ref(ZeroCopyRef{phys, len, flags})
                                4. pop_ref() → ZeroCopyRef
                                5. Lit buffer via physmap
                                6. ZeroCopyBuffer::release() → décrémente refcount
```

Aucune copie de données n'a lieu entre les étapes 2 et 5.

---

## Comparatif des types de ring

| Ring | Producteurs | Consommateurs | Copie | Usage typique |
|---|---|---|---|---|
| `SpscRing` | 1 | 1 | Oui (RingSlot) | Canal sync/async 1:1 |
| `MpmcRing` | N | M | Oui (RingSlot) | Canal MPMC |
| `FusionRing` | 1 | 1 | Oui (batch) | Canal haute densité |
| `ZcRing` | 1 | 1 | Non (référence) | Zero-copy SHM |
| `SlotPool` | N | M | Non (slot RAII) | Allocation statique |
| `BatchBuffer` | 1 | — | Tampon | Pré-flush groupé |
