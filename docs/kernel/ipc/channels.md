# Canaux IPC

Les canaux sont les abstractions de haut niveau fournissant des API orientées communication. Ils s'appuient sur les ring buffers de `ring/`.

## Vue d'ensemble

```
channel/
├── sync.rs       — Rendezvous synchrone (émetteur bloque jusqu'à réception)
├── async.rs      — Canal asynchrone (Futures, Waker)
├── mpmc.rs       — Multi-Producteurs Multi-Consommateurs
├── broadcast.rs  — Un émetteur → N récepteurs (max 16)
├── typed.rs      — Canal type-safe générique
└── streaming.rs  — DMA — gros volumes continus (> 50 GB/s)
```

Chaque canal est identifié par un `ChannelId` opaque alloué à la création.

---

## Canal synchrone — `channel/sync.rs`

### Description

Canal de rendezvous : l'émetteur **bloque** jusqu'à ce que le récepteur ait reçu le message. Garantit la livraison point-à-point avant de retourner.

### Cas d'usage

- Communications latence-critique entre deux processus connus
- Protocoles requête/réponse sans état côté serveur (correlé par `Cookie`)
- IPC entre threads du même processus nécessitant une synchronisation forte

### Fonctionnement

```
Émetteur                    Récepteur
─────────────────────────────────────
sync_channel_send(ch, msg)
  → alloue slot dans SpscRing
  → pose sur wait_queue (Woken=false)
  → bloque (futex_wait)
                            sync_channel_recv(ch, buf)
                              → prend slot depuis SpscRing
                              → copie dans buf
                              → futex_wake → débloque émetteur
  → retourne Ok(())
```

### API

```rust
pub fn sync_channel_send(ch: ChannelId, msg: &IpcMessage) -> Result<(), IpcError>
pub fn sync_channel_recv(ch: ChannelId, buf: &mut IpcMessage) -> Result<(), IpcError>
pub fn sync_channel_close(ch: ChannelId) -> Result<(), IpcError>
```

### Timeout

Le timeout par défaut est `SYNC_CHANNEL_TIMEOUT_NS` (100 ms). En cas d'expiration, `sync_channel_send` retourne `Err(IpcError::Timeout)`.

---

## Canal asynchrone — `channel/async.rs`

### Description

Canal non bloquant basé sur les futures Rust. Compatible avec un éventuel runtime async kernel.

### Cas d'usage

- I/O drivers asynchrones
- Notifications d'événements système sans bloquer le thread courant
- Pipeline de traitements asynchrones

### Modèle

```rust
pub struct IpcAsyncSender { channel_id: ChannelId, ... }
pub struct IpcAsyncReceiver { channel_id: ChannelId, ... }

impl IpcAsyncSender {
    pub async fn send(&self, msg: IpcMessage) -> Result<(), IpcError>
}

impl IpcAsyncReceiver {
    pub async fn recv(&mut self) -> Result<IpcMessage, IpcError>
}
```

Le waker est enregistré dans `IpcWaitQueue` et déclenché par `wake_one()` lors de la disponibilité d'un message.

---

## Canal MPMC — `channel/mpmc.rs`

### Description

Wrapper haute-performance sur `ring::MpmcRing`. Permet N producteurs et M consommateurs simultanés, sans verrou central.

### Cas d'usage

- File de travaux partagée entre workers
- Dispatcher d'événements vers plusieurs handlers
- Pipeline producteur/consommateur à débit élevé

### Limites

- Capacité : `RING_SIZE` = 16 slots (statique)
- N producteurs et M consommateurs possibles, mais la progression n'est pas FIFO stricte entre consommateurs

### API

```rust
pub struct MpmcChannel { inner: MpmcRing, id: ChannelId }

impl MpmcChannel {
    pub fn new() -> Self
    pub fn send(&self, msg: &RingSlot) -> bool
    pub fn recv(&self, dst: &mut RingSlot) -> bool
    pub fn id(&self) -> ChannelId
}
```

---

## Canal Broadcast — `channel/broadcast.rs`

### Description

Un émetteur envoie à **N récepteurs simultanément**. Chaque récepteur dispose de sa propre file entrante pour éviter le head-of-line blocking.

### Constantes

```rust
pub const MAX_BROADCAST_SUBSCRIBERS:     usize = 16;
pub const BROADCAST_CHANNEL_TABLE_SIZE:  usize = 16;
```

### Schéma

```
Émetteur
    │
    ├──► Récepteur 0  (SpscRing individuel)
    ├──► Récepteur 1
    ├──► Récepteur 2
    │    ...
    └──► Récepteur N-1 (max 15)
```

### API

```rust
pub struct BroadcastChannel {
    id:          ChannelId,
    sub_rings:   [Option<SpscRing>; MAX_BROADCAST_SUBSCRIBERS],
    sub_count:   usize,
}

impl BroadcastChannel {
    pub fn subscribe(&mut self) -> Option<usize>  // retourne l'index du récepteur
    pub fn unsubscribe(&mut self, index: usize)
    pub fn broadcast(&self, msg: &RingSlot)  // envoie à tous les abonnés
    pub fn recv(&self, index: usize, dst: &mut RingSlot) -> bool
}
```

### Comportement sur file pleine

Si la file d'un abonné est pleine lors du broadcast, le message est **ignoré** pour cet abonné (pas de blocage de l'émetteur). Un compteur de drops est incrémenté dans `IPC_STATS`.

---

## Canal typé — `channel/typed.rs`

### Description

Wrapper générique apportant la vérification de types à la compilation. Évite les erreurs de sérialisation/désérialisation à l'exécution.

### Modèle

```rust
pub struct TypedSender<T: Copy>   { inner: SpscRing, _t: PhantomData<T> }
pub struct TypedReceiver<T: Copy> { inner: SpscRing, _t: PhantomData<T> }

impl<T: Copy> TypedSender<T> {
    pub fn send(&self, value: T) -> Result<(), IpcError>
}

impl<T: Copy> TypedReceiver<T> {
    pub fn recv(&self) -> Result<T, IpcError>
}
```

### Contrainte de taille

`size_of::<T>() <= MAX_MSG_SIZE` (240 octets). Les types dépassant cette limite doivent utiliser le canal streaming avec zero-copy SHM.

---

## Canal Streaming — `channel/streaming.rs`

### Description

Canal pour les transferts de gros volumes de données (fichiers, buffers vidéo, flux réseau DMA). Utilise le zero-copy SHM pour atteindre > 50 GB/s.

### Cas d'usage

- Transfert de frames vidéo entre driver GPU et serveur d'affichage
- Buffer de paquets réseau entre driver et stack TCP/IP
- Lecture de blocs de stockage NVMe → processus userland

### Modèle de données

```
Producteur
  1. shm_alloc(size)          → ptr SHM physiquement contigu
  2. Écrit données dans ptr
  3. ZcRing::push_ref({phys, len, flags})

Consommateur
  4. ZcRing::pop_ref()        → ZeroCopyRef
  5. Lit via physmap (zero copie)
  6. shm_free(ptr)            → retour au pool
```

### API

```rust
pub struct StreamingChannel {
    id:      ChannelId,
    zc_ring: ZcRing,
}

impl StreamingChannel {
    pub fn send_zc(&self, buffer: ZeroCopyRef) -> Result<(), IpcError>
    pub fn recv_zc(&self) -> Result<ZeroCopyRef, IpcError>
}
```

---

## Tableau comparatif

| Canal | Modèle | Copie | Bloquant | Usage |
|---|---|---|---|---|
| `sync` | 1:1 rendezvous | Oui | Oui | Requête/réponse stricte |
| `async` | 1:1 future | Oui | Non | I/O asynchrone |
| `mpmc` | N:M | Oui | Non | Workers / dispatch |
| `broadcast` | 1:N | Oui | Non | Notifications |
| `typed<T>` | 1:1 | Oui (T) | Configurable | Type-safe interne |
| `streaming` | 1:1 | Non (ZC) | Non | Gros volumes DMA |
