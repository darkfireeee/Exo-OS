# 🚀 Advanced Channels - Patterns de Communication Avancés

## Vue d'ensemble

Les canaux avancés fournissent des patterns de communication sophistiqués au-delà du simple point-à-point.

## PriorityChannel - 5 Niveaux de Priorité

### Concept

Un canal avec **5 queues séparées**, une par niveau de priorité. La réception sert toujours la priorité la plus haute d'abord.

```
┌─────────────────────────────────────────────────────────────┐
│                    PriorityChannel                           │
├─────────────────────────────────────────────────────────────┤
│  RealTime Ring ──► ┌────────────────┐                       │
│  High Ring     ──► │   Réception    │ → Message le plus     │
│  Normal Ring   ──► │   Prioritaire  │   prioritaire         │
│  Low Ring      ──► └────────────────┘                       │
│  Bulk Ring     ──►                                          │
└─────────────────────────────────────────────────────────────┘
```

### API

```rust
// Créer un canal prioritaire
let channel = PriorityChannel::new(256)?;

// Envoi avec priorité
channel.send(data, PriorityClass::RealTime)?;  // Urgent
channel.send(data, PriorityClass::Normal)?;    // Normal
channel.send(data, PriorityClass::Bulk)?;      // Background

// Réception (toujours la plus haute priorité disponible)
let (size, priority, latency) = channel.recv(&mut buffer)?;
```

### Cas d'usage

- **RealTime**: Interruptions, événements critiques
- **High**: UI, réponses utilisateur
- **Normal**: Tâches standard
- **Low**: Logging, monitoring
- **Bulk**: Transferts de fichiers, backups

---

## MulticastChannel - Un vers Plusieurs

### Concept

Un émetteur envoie à **N récepteurs** simultanément. Chaque récepteur a son propre buffer pour éviter le blocage mutuel.

```
┌──────────┐       ┌────────────┐
│  Sender  │──────►│ Receiver 1 │  Ring dédié
│          │──────►│ Receiver 2 │  Ring dédié
│          │──────►│ Receiver 3 │  Ring dédié
└──────────┘       └────────────┘
```

### Gestion des Récepteurs Lents

Si un récepteur prend du retard (lag > max_lag), ses messages sont **droppés** pour ne pas bloquer les autres.

```rust
// Créer avec max_lag de 64 messages
let channel = MulticastChannel::new(256, 64)?;

// Ajouter des récepteurs
let receiver1 = channel.add_receiver()?;
let receiver2 = channel.add_receiver()?;

// Envoi à tous
channel.send(data, PriorityClass::Normal)?;

// Chaque récepteur lit indépendamment
let (size, _, _) = receiver1.recv(&mut buffer)?;
```

### États des Récepteurs

```rust
pub struct MulticastReceiverState {
    pub id: u64,
    pub lag: u64,           // Messages en retard
    pub dropped: u64,       // Messages droppés (lag trop grand)
    pub received: u64,      // Total reçus
}
```

---

## AnycastChannel - Load Balancing

### Concept

Un émetteur envoie à **un seul** récepteur parmi N, choisi selon une politique de load balancing.

```
┌──────────┐       ┌────────────┐
│  Sender  │──┬───►│ Receiver 1 │  Sélection
│          │  └───►│ Receiver 2 │  selon
│          │  └───►│ Receiver 3 │  politique
└──────────┘       └────────────┘
```

### Politiques

```rust
pub enum AnycastPolicy {
    RoundRobin,    // Tour à tour
    LeastLoaded,   // Moins chargé
    Random,        // Aléatoire
    AffinityFirst, // Préfère même CPU (NUMA)
}
```

### API

```rust
// Créer avec politique Round Robin
let channel = AnycastChannel::new(256, AnycastPolicy::RoundRobin)?;

// Ajouter des workers
let worker1 = channel.add_receiver()?;
let worker2 = channel.add_receiver()?;
let worker3 = channel.add_receiver()?;

// Envoi - sera routé vers UN worker
channel.send(task_data, PriorityClass::Normal)?;

// Chaque worker traite ses tâches
loop {
    if let Ok((size, _, _)) = worker1.recv(&mut buffer) {
        process_task(&buffer[..size]);
    }
}
```

### États des Récepteurs

```rust
pub struct AnycastReceiverState {
    pub id: u64,
    pub pending: u64,    // Messages en attente
    pub processed: u64,  // Messages traités
    pub load_factor: f32, // Charge (0.0 - 1.0)
}
```

---

## RequestReplyChannel - Pattern RPC

### Concept

Canal bidirectionnel pour **requête-réponse** avec corrélation automatique. Idéal pour les appels RPC.

```
┌──────────┐  Request   ┌──────────┐
│  Client  │───────────►│  Server  │
│          │◄───────────│          │
└──────────┘  Response  └──────────┘
         correlation_id
```

### API

```rust
// Créer le canal
let channel = RequestReplyChannel::new(256)?;

// Client: envoyer requête et attendre réponse
let correlation_id = channel.send_request(request_data)?;
let (response, latency) = channel.recv_response(correlation_id, &mut buffer)?;

// Server: recevoir requête et envoyer réponse
let (request, correlation_id) = channel.recv_request(&mut buffer)?;
// ... traitement ...
channel.send_response(correlation_id, response_data)?;
```

### Tracking de Latence

```rust
// Obtenir statistiques
let stats = channel.stats();
println!("Latence moyenne: {} cycles", stats.avg_latency);
println!("Latence P99: {} cycles", stats.p99_latency);
```

---

## Comparaison des Patterns

| Pattern | Producteurs | Consommateurs | Cas d'usage |
|---------|-------------|---------------|-------------|
| Point-to-Point | 1-N | 1-N | Communication standard |
| Priority | 1-N | 1-N | QoS, préemption |
| Multicast | 1 | N | Broadcast, pub/sub |
| Anycast | 1-N | N (1 actif) | Load balancing |
| Request-Reply | 1 | 1 | RPC, services |
