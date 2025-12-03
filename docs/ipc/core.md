# 🔧 IPC Core - Primitives Fondamentales

## CoalesceController - Coalescing Adaptatif

Le `CoalesceController` optimise dynamiquement le batching des messages basé sur la charge.

### Modes de Coalescing

```rust
pub enum CoalesceMode {
    Immediate,   // Pas de batching - latence minimale
    Light,       // Batch de 4 messages max
    Moderate,    // Batch de 16 messages max
    Aggressive,  // Batch de 64 messages max
}
```

### Algorithme EMA

Le contrôleur utilise une **Moyenne Mobile Exponentielle** (EMA) pour calculer l'intervalle moyen entre les arrivées de messages:

```
new_ema = old_ema - (old_ema >> 3) + (interval >> 3)
```

- **Intervalle court** (< 1000 cycles) → Mode Aggressive
- **Intervalle moyen** (1000-10000 cycles) → Mode Moderate
- **Intervalle long** (10000-100000 cycles) → Mode Light
- **Intervalle très long** (> 100000 cycles) → Mode Immediate

### Utilisation

```rust
let coalesce = CoalesceController::new();

// Enregistrer une arrivée
coalesce.record_arrival(rdtsc());

// Obtenir le mode actuel
let mode = coalesce.current_mode();

// Vérifier si on doit flush le batch
if coalesce.should_flush() {
    coalesce.flush_batch();
}
```

---

## CreditController - Flow Control

Empêche un producteur rapide de submerger un consommateur lent.

### Mécanisme

```rust
pub struct CreditController {
    available: AtomicU64,    // Crédits disponibles
    total: u64,              // Crédits totaux
    low_water: u64,          // Seuil bas (défaut: 25%)
    high_water: u64,         // Seuil haut (défaut: 75%)
}
```

### API

```rust
let credits = CreditController::new(256); // 256 slots

// Producteur: consommer un crédit
if credits.try_consume(1) {
    // Envoi autorisé
} else {
    // Backpressure - attendre
}

// Consommateur: libérer un crédit
credits.grant(1);

// Vérifier les seuils
if credits.is_low() {
    // Réveiller les producteurs bloqués
}
```

---

## PriorityClass - 5 Niveaux de Priorité

```rust
pub enum PriorityClass {
    RealTime = 0,  // Latence minimale, préempte tout
    High = 1,      // Interactive/UI
    Normal = 2,    // Défaut
    Low = 3,       // Background
    Bulk = 4,      // Transferts massifs
}
```

### Politique de Service

Les messages sont servis par **ordre de priorité strict**:
1. Tous les messages RealTime d'abord
2. Puis High, Normal, Low, Bulk

---

## LaneStats - Statistiques par Priorité

```rust
#[repr(C, align(64))]  // Évite false sharing
pub struct LaneStats {
    pub sent: AtomicU64,
    pub received: AtomicU64,
    pub bytes: AtomicU64,
    pub avg_latency: AtomicU64,  // EMA en cycles
}
```

---

## TimestampedSlot - Slot avec Timestamp

Chaque slot de 64 octets contient:

```
┌────────────────────────────────────────────────────────────────┐
│ sequence (8B) │ send_tsc (8B) │ pri (1B) │ flags (1B) │ size (2B) │ reserved (4B) │
├────────────────────────────────────────────────────────────────┤
│                        payload (40 bytes)                       │
└────────────────────────────────────────────────────────────────┘
```

Le timestamp TSC permet de calculer la latence exacte:
```rust
let latency = recv_tsc - send_tsc; // En cycles CPU
```

---

## Fonctions de Prefetch

```rust
// Prefetch pour lecture
prefetch_read(ptr);

// Prefetch pour écriture
prefetch_write(ptr);

// Prefetch une plage
prefetch_range(ptr, len);
```

Ces fonctions utilisent `_mm_prefetch` pour charger les données en cache L1 avant leur utilisation.

---

## IpcPerfCounters - Compteurs Globaux

```rust
pub static GLOBAL_PERF_COUNTERS: IpcPerfCounters;

// Métriques disponibles:
counters.total_sends.load(Ordering::Relaxed);
counters.total_recvs.load(Ordering::Relaxed);
counters.total_bytes.load(Ordering::Relaxed);
counters.inline_sends.load(Ordering::Relaxed);
counters.zerocopy_sends.load(Ordering::Relaxed);
counters.batch_sends.load(Ordering::Relaxed);
counters.spin_iterations.load(Ordering::Relaxed);
counters.cas_retries.load(Ordering::Relaxed);
```
