# ⚡ UltraFastRing - Ring Optimisé 80-100 Cycles

## Objectif

L'`UltraFastRing` est le cœur du hot path IPC, optimisé pour atteindre **80-100 cycles** par message inline (vs ~1200 cycles pour Linux pipes).

## Architecture

```
┌──────────────────────────────────────────────────────────────────────────┐
│                           UltraFastRing                                   │
├──────────────────────────────────────────────────────────────────────────┤
│  [CacheLineU64]     [CacheLineU64]     [CacheLineU64]     [CacheLineU64] │
│  producer_head      producer_tail      consumer_head      consumer_tail  │
│  (64B aligned)      (64B aligned)      (64B aligned)      (64B aligned)  │
├──────────────────────────────────────────────────────────────────────────┤
│                     TimestampedSlot Array                                 │
│  ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐                  │
│  │ Slot 0 │ │ Slot 1 │ │ Slot 2 │ │ Slot 3 │ │  ...   │                  │
│  │  64B   │ │  64B   │ │  64B   │ │  64B   │ │        │                  │
│  └────────┘ └────────┘ └────────┘ └────────┘ └────────┘                  │
├──────────────────────────────────────────────────────────────────────────┤
│  CoalesceController  │  CreditController  │  LaneStats[5]                │
└──────────────────────────────────────────────────────────────────────────┘
```

## Optimisations Clés

### 1. Isolation Cache Line

```rust
#[repr(C, align(64))]
struct CacheLineU64 {
    value: AtomicU64,
    _pad: [u8; 56],  // Remplit à 64 octets
}
```

**Pourquoi ?** Évite le **false sharing** où plusieurs CPUs invalident mutuellement leurs caches.

### 2. Power-of-2 Masking

```rust
let index = seq & self.mask;  // Rapide: AND au lieu de MOD
```

### 3. Prefetching Prédictif

```rust
// Pendant l'envoi, précharger le prochain slot
prefetch_write(self.get_slot(seq.wrapping_add(1)));
```

### 4. Ordering Relâché

Utilise `Ordering::Relaxed` partout où c'est safe, avec des `fence()` explicites uniquement aux points critiques.

## API

### Création

```rust
// Capacité doit être puissance de 2
let ring = UltraFastRing::new(256);

// Ou avec capacité par défaut (256)
let ring = UltraFastRing::with_default_capacity();
```

### Envoi Rapide

```rust
// Envoi inline (≤40 octets) - Target: 80-100 cycles
ring.send_fast(data, PriorityClass::Normal)?;

// Envoi avec priorité
ring.send_fast(urgent_data, PriorityClass::RealTime)?;
```

### Réception Rapide

```rust
let mut buffer = [0u8; 64];

// Retourne (taille, priorité, latence_cycles)
let (size, priority, latency) = ring.recv_fast(&mut buffer)?;

println!("Reçu {} octets, priorité {:?}, latence {} cycles", 
         size, priority, latency);
```

### Opérations Batch

```rust
// Envoi batch - amortit l'overhead
let messages = [b"msg1", b"msg2", b"msg3"];
let sent = ring.send_batch(&messages, PriorityClass::Normal)?;

// Réception batch
let mut buffers = [[0u8; 64]; 8];
let received = ring.recv_batch(&mut buffers)?;
```

## Statistiques

```rust
let stats = ring.stats();

println!("Envoyés: {}", stats.sent);
println!("Reçus: {}", stats.received);
println!("Latence moyenne: {} cycles", stats.avg_latency);
```

## Flux d'Exécution - send_fast()

```
1. rdtsc() → Timestamp de départ
2. Vérifier taille ≤ 40B
3. credits.try_consume(1) → Flow control
4. coalesce.record_arrival() → Stats coalescing
5. claim_produce_slot() → Réserver séquence (CAS)
6. prefetch_write(next_slot) → Précharger
7. wait_slot_ready() → Attendre slot libre
8. slot.write(data, priority) → Écrire données
9. commit_produce() → Publier
10. lane_stats.record() → Statistiques
```

## Flux d'Exécution - recv_fast()

```
1. rdtsc() → Timestamp de départ
2. claim_consume_slot() → Réserver séquence (CAS)
3. prefetch_read(next_slot) → Précharger
4. wait_data_ready() → Attendre données
5. slot.read(buffer) → Lire + calculer latence
6. commit_consume() → Libérer slot
7. credits.grant(1) → Rendre crédit
8. lane_stats.record() → Statistiques
```

## Comparaison avec Linux

| Aspect | UltraFastRing | Linux pipe |
|--------|---------------|------------|
| Latence inline | 80-100 cycles | ~1200 cycles |
| Syscalls | 0 | 2 (write+read) |
| Copies | 0-1 | 2+ |
| Cache misses | ~1 | ~4-6 |
| Contention | CAS lock-free | spinlock/mutex |
