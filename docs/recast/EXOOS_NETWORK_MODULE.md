# ExoOS — Module Réseau `network_server`
> Architecture · Fonctionnalités · Performance · Intégration  
> Avril 2026 · v1.0 · Spécification définitive

---

## 0. Contexte et périmètre

Le `network_server` est un **server Ring 1** démarré à l'étape 10 de la séquence canonique
d'initialisation, après `virtio-net` (étape 8) et avant toute application Ring 3.

Il tourne dans son propre espace d'adressage, isolé du kernel Ring 0 par les capabilities
ExoOS. Un crash de la stack réseau ne provoque aucun kernel panic — le `init_server` relance
le `network_server` via la politique de restart définie dans `service_table.rs`.

**Stack TCP/IP sous-jacente :** smoltcp (Rust pur, `no_std`, 0-BSD).  
Chiffres mesurés smoltcp sur tap0 (Intel Core i5-13500H, Linux 6.9) :
- Reader throughput : **3.67 Gbps**
- Writer throughput : **7.90 Gbps**
- Loopback interne : **15.4 Gbps**

Ces chiffres sont le plancher, pas le plafond — ExoOS supprime les couches
de traduction POSIX et les copies inutiles qui pénalisent smoltcp dans un contexte Linux standard.

---

## 1. Architecture générale

```
Ring 3 (Applications)
        │
        │  ExoSocket API
        │  syscalls 547–550
        ▼
┌─────────────────────────────────────────────┐
│         network_server  (Ring 1, PID dyn)   │
│                                             │
│  ┌────────────┐  ┌──────────────────────┐  │
│  │ ExoSocket  │  │  TcpStateStore       │  │
│  │ cap_table  │  │  (snapshot Phoenix)  │  │
│  └─────┬──────┘  └──────────────────────┘  │
│        │                                    │
│  ┌─────▼──────────────────────────────┐    │
│  │  smoltcp Interface (poll-loop)     │    │
│  │  TCP · UDP · ICMP · ARP · IPv4/6  │    │
│  └─────┬──────────────────────────────┘    │
│        │                                    │
│  ┌─────▼──────────────────────────────┐    │
│  │  NetBufPool  (pages DMA pinnées)   │    │
│  │  RX_POOL[256] · TX_POOL[256]      │    │
│  └─────┬──────────────────────────────┘    │
└────────┼────────────────────────────────────┘
         │  SpscRing<NetBufRef>
         │  (IPC existant, zero alloc)
         ▼
┌─────────────────────────────────────────────┐
│         virtio-net driver  (Ring 1)         │
│                                             │
│  vring RX descriptors ←── DMA ←── NIC      │
│  vring TX descriptors ──→ DMA ──→ NIC      │
│                                             │
│  Mode : interrupt (faible charge)           │
│       → polling  (forte charge)             │
└─────────────────────────────────────────────┘
```

---

## 2. Fonctionnalités

### 2.1 NetBufPool — Zero-copy RX/TX

**Principe.** Le driver `virtio-net` ne gère pas de buffers propres. Au boot, le
`network_server` alloue via le `memory_server` un pool de pages physiques contiguës,
marquées `NO_COW + PINNED`. Les adresses physiques de ces pages sont inscrites dans les
descripteurs vring avant toute réception.

Quand un paquet arrive, le NIC (QEMU virtio) écrit directement dans la page du pool par DMA.
Le driver signale au `network_server` via `SpscRing<NetBufRef>` — une référence (index dans
le pool + longueur), pas une copie. Le `network_server` passe cette référence à smoltcp.

Pour les applications Ring 3, la page est partagée via **SHM CapToken read-only** :
l'application lit le payload dans la même page que le DMA. Aucun `memcpy` sur le chemin
critique pour les paquets ≥ 256 octets.

**Exception small-packet.** Pour les paquets < 240 octets, la copie inline dans un
`IpcMessage` (240 B max, déjà dans le protocole IPC existant) est **plus rapide** que la
gestion de référence de page. Le seuil est configurable à la compilation.

```
┌──────────────┐     DMA      ┌──────────────┐
│   NIC QEMU   │ ──────────→  │ NetBufPool   │
└──────────────┘              │ page[i]      │
                              └──────┬───────┘
                                     │ NetBufRef{idx, len}
                              ┌──────▼───────┐
                              │ SpscRing     │ ← zéro copie
                              └──────┬───────┘
                                     │
                              ┌──────▼───────┐
                              │ smoltcp      │
                              └──────┬───────┘
                                     │ SHM CapToken (RO)
                              ┌──────▼───────┐
                              │ Application  │
                              └──────────────┘
```

**Contrainte implémentation.** La taille du pool détermine le nombre de paquets en vol
simultanés. Valeur recommandée : `RX_POOL_SIZE = 256` pages × 4 Ko = 1 Mo réservé au
boot. Le `memory_server` alloue ce bloc via buddy avant le démarrage du `network_server`.
Le pool n'est jamais restitué au buddy — il est permanent, comme la SSR.

---

### 2.2 ExoSocket API — Sockets capability-gated

**Problème avec BSD sockets.** `socket()` + `bind()` + `listen()` = autorité ambiante.
N'importe quel processus avec les droits suffisants peut ouvrir un socket sur n'importe
quel port. Le filtrage est rajouté après coup (iptables, nftables).

**Approche ExoOS.** Un socket est créé en présentant un `CapToken` réseau. Ce token encode
directement la policy autorisée :

```
CapToken {
    gen:    u32,          // génération — révocation instantanée
    oid:    ObjectId,     // identifiant unique du socket
    rights: NetRights,    // proto | port_range | dst_cidr | bandwidth_budget
}

NetRights {
    proto:            Proto,      // TCP | UDP | ICMP | ANY
    local_port_min:   u16,
    local_port_max:   u16,
    remote_cidr:      IpCidr,    // 0.0.0.0/0 = tout, sinon restreint
    bandwidth_budget: u32,       // octets/s max, 0 = illimité
}
```

Le `network_server` maintient une `cap_table : [Option<ExoSocketEntry>; MAX_SOCKETS]`
(tableau statique, pas de heap). Chaque entrée est vérifiée en O(1) via `verify()` constant-
time sur le token (même primitive que le reste de l'OS).

**Nouveaux syscalls :**

| Numéro | Nom | Description |
|--------|-----|-------------|
| 547 | `sys_exo_socket_open` | Ouvrir un socket, présenter le CapToken réseau |
| 548 | `sys_exo_socket_connect` | Connecter (TCP) ou fixer l'adresse distante (UDP) |
| 549 | `sys_exo_socket_send` | Envoyer données (zero-copy si ≥ 256 B) |
| 550 | `sys_exo_socket_recv` | Recevoir données (référence SHM ou copie inline) |

**Ce qui change structurellement.** Un processus sans CapToken réseau valide est **muet**
par construction — pas parce qu'une règle le bloque, mais parce que le syscall 547 retourne
`ExoError::CapabilityDenied` en première instruction, avant toute allocation. Aucune règle
de filtrage n'est nécessaire. La surface d'attaque réseau d'un processus est entièrement
définie par son CapToken, accordé à la création par `init_server`.

---

### 2.3 Polling adaptatif — Hybrid interrupt/poll

**Problème.** Les interruptions hardware ont un overhead fixe (~1–2 µs par IRQ sur x86_64
APIC). À faible charge c'est acceptable. À haute charge (millions de paquets/s), cet
overhead domine et effondre le débit.

**Solution.** Le driver `virtio-net` implémente un mode hybride :

```
Faible charge  →  mode IRQ   : interruption par paquet, CPU libéré entre les paquets
Forte charge   →  mode POLL  : spin-wait sur le used ring, zero overhead IRQ
```

La bascule se fait sur seuil de paquets consécutifs reçus en une seule ISR. Si l'IRQ
handler traite `N ≥ POLL_THRESHOLD` paquets d'affilée, il masque l'IRQ (IOAPIC mask) et
passe en polling. Quand le used ring est vide, il démasque et repasse en IRQ.

```rust
// Dans irq_handler (virtio-net Ring 1)
fn handle_rx_irq(&mut self) {
    let mut count = 0;
    while let Some(buf_ref) = self.consume_used_ring() {
        self.spsc_tx.push(buf_ref);  // vers network_server
        count += 1;
        if count >= POLL_THRESHOLD {
            ioapic_mask(self.rx_irq);
            self.mode = DriverMode::Polling;
            return;  // le poll-loop prend le relais
        }
    }
    // Reste en mode IRQ si count < POLL_THRESHOLD
}

fn poll_tick(&mut self) {
    if self.consume_used_ring().is_none() {
        ioapic_unmask(self.rx_irq);
        self.mode = DriverMode::Interrupt;
    }
}
```

`POLL_THRESHOLD` recommandé : **8 paquets**. Au-delà du seuil le polling prend le relais.
En dessous, chaque paquet déclenche une IRQ — latence basse garantie à faible charge.

**Conformité ExoOS.** `ioapic_mask()` et `ioapic_unmask()` sont des opérations atomiques
lock-free, autorisées dans un handler IRQ Ring 1. Pas de spinlock. Pas de yield.

---

### 2.4 ExoPhoenix Net — Survie TCP à travers un cycle Phoenix

**Contexte.** Lors d'un cycle ExoPhoenix, le `network_server` reçoit un IPC
`PrepareIsolation` (règle PHX-01). Il doit retourner `PrepareIsolationAck` avant que Kernel
B ne gèle les cores. Le `network_server` est ensuite arrêté et redémarré sur un nouvel espace
d'adressage.

**Problème.** Avec une stack TCP classique, toutes les connexions actives meurent avec le
processus. L'application reçoit un RST ou un timeout.

**Solution ExoPhoenix Net.** Le `network_server` maintient en parallèle un `TcpStateStore`
sérialisable. À chaque modification d'état TCP significative (changement d'état de la machine
à états, mise à jour des numéros de séquence, modification de la fenêtre), le store est mis à
jour de façon synchrone. `PrepareIsolation` déclenche la sérialisation complète dans une zone
mémoire partagée avec Kernel B (région SSR étendue ou segment dédié).

**Données sérialisées par socket TCP :**

```rust
#[repr(C)]
pub struct TcpSocketSnapshot {
    // Identité
    local_addr:       IpAddress,
    local_port:       u16,
    remote_addr:      IpAddress,
    remote_port:      u16,
    cap_token_gen:    u32,      // pour re-valider après restore

    // État machine TCP
    state:            TcpState, // Established | CloseWait | FinWait1 | etc.

    // Numéros de séquence (getters publics smoltcp)
    local_seq_no:     u32,
    remote_seq_no:    u32,
    remote_win_len:   u32,
    remote_win_scale: Option<u8>,

    // Timers
    rtt_estimate_ms:  u32,
    retransmit_ms:    u32,

    // Données en transit (octets en attente de ACK)
    tx_pending_len:   u32,
    rx_pending_len:   u32,
    // Données bufferiées copiées (seule copie du protocole Phoenix Net)
    tx_pending:       [u8; TCP_SNAPSHOT_BUF],
    rx_pending:       [u8; TCP_SNAPSHOT_BUF],
}

pub const TCP_SNAPSHOT_BUF: usize = 65536;  // 64 Ko par socket
pub const MAX_SNAPSHOT_SOCKETS: usize = 64; // 64 connexions survivantes max
```

**Limite smoltcp.** smoltcp n'expose pas nativement ses buffers internes en lecture directe.
Le `TcpStateStore` contourne cela en interceptant les données à la couche `send()` / `recv()`
du `network_server` — les données non-ACKées sont conservées dans le store, pas seulement dans
smoltcp. Cela implique une **copie supplémentaire** sur le TX path pour les données non-ACKées
(TX path normal reste zero-copy pour les données ACKées).

**Séquence PrepareIsolation :**

```
network_server reçoit PrepareIsolation
        │
        ├─ 1. Sérialiser TcpStateStore → région partagée Kernel B
        ├─ 2. Drainer le SpscRing TX (flush paquets en attente)
        ├─ 3. Nettoyer le NetBufPool (remettre toutes les pages en pool)
        ├─ 4. Retourner PrepareIsolationAck { checkpoint_id: epoch }
        │
        ▼
Kernel B gèle les cores (IPI 0xF3)
        │
        ▼
Cycle Phoenix (snapshot RAM + restore)
        │
        ▼
network_server redémarre sur nouveau PID
        ├─ 1. Lire le TcpStateStore depuis la région partagée
        ├─ 2. Re-créer les sockets smoltcp avec l'état sérialisé
        ├─ 3. Envoyer window update aux pairs TCP (signale reprise)
        └─ 4. Reprendre le poll-loop normalement
```

**Résultat visible pour l'application.** Un micro-lag ≤ 100 ms selon la charge, puis les
connexions TCP reprennent. Aucun RST envoyé. Aucun timeout côté pair si la fenêtre de
retransmission n'est pas expirée (~RTO initial = 1 s). En pratique : transferts de fichiers,
connexions SSH, HTTP keep-alive survivent à un cycle Phoenix complet.

---

## 3. Performance

### 3.1 Cibles mesurables

| Métrique | Valeur cible | Base de référence | Méthode de mesure |
|----------|-------------|-------------------|-------------------|
| Throughput TCP RX (QEMU virtio) | ≥ 4 Gbps | 3.67 Gbps smoltcp/tap | iperf3 loopback QEMU |
| Throughput TCP TX (QEMU virtio) | ≥ 8 Gbps | 7.90 Gbps smoltcp/tap | iperf3 loopback QEMU |
| Latence RTT loopback | ≤ 50 µs | ~100 µs Linux loopback | ping QEMU interne |
| Overhead IRQ (mode polling) | 0 cycles/pkt | ~2 µs/IRQ Linux | perf stat |
| Allocations heap RX path | 0 | N allocs Linux sk_buff | valgrind/heaptrack |
| Temps PrepareIsolation réseau | ≤ 5 ms | N/A (aucun OS existant) | TSC delta |
| Restore TCP après Phoenix | ≤ 100 ms lag | N/A (connexion morte ailleurs) | time delta connect |

**Note importante.** Les chiffres "loopback interne smoltcp" (15.4 Gbps) ne sont pas
pertinents en contexte QEMU car le bottleneck est le vring virtio, pas smoltcp. Les cibles
≥ 4 / ≥ 8 Gbps sont réalistes sur matériel moderne avec QEMU KVM accelerated.

### 3.2 Analyse du chemin critique RX

```
NIC écrit DMA dans page pool         →   0 µs CPU (hardware)
virtio-net détecte used ring entry   →   ~200 ns (check atomique)
SpscRing push(NetBufRef)             →   ~50 ns (Release store)
network_server SpscRing pop()        →   ~50 ns (Acquire load)
smoltcp process_ingress()            →   ~500 ns (TCP state machine)
Application reçoit SHM ref          →   ~100 ns (CapToken verify O(1))
─────────────────────────────────────────────────────────────────
Total chemin RX (grands paquets)     →   ~900 ns = ~0.9 µs
```

Comparaison Linux TCP stack : ~5–15 µs pour le même chemin (sk_buff alloc + copy + netfilter
+ socket buffer + syscall).

### 3.3 Consommation mémoire

| Composant | Taille fixe | Dynamique |
|-----------|------------|-----------|
| NetBufPool RX (256 pages) | 1 Mo | Non |
| NetBufPool TX (256 pages) | 1 Mo | Non |
| cap_table (MAX_SOCKETS=256) | ~16 Ko | Non |
| TcpStateStore (64 sockets × ~130 Ko) | ~8 Mo | Non |
| smoltcp SocketSet | ~10 Ko par socket | Non (statique) |
| **Total network_server** | **~11 Mo** | **0** |

Zéro allocation dynamique en steady-state. Toute la mémoire est réservée au démarrage via
`memory_server`. Pas d'OOM possible sur le chemin réseau une fois le serveur démarré.

---

## 4. Intégration dans ExoOS

### 4.1 Dépendances et règles Ring 1

Le `network_server` respecte strictement les règles Ring 1 de la spec v7 :

| Règle | Application réseau |
|-------|-------------------|
| `PHX-01` | `isolation.rs` implémenté : sérialise `TcpStateStore` avant d'envoyer `PrepareIsolationAck` |
| `PHX-02` | `#![no_std]` + `panic = "abort"` dans `Cargo.toml` |
| `PHX-03` | `Blake3(ELF network_server)` enregistré dans ExoFS via `build/register_binaries.sh` |
| `SRV-02` | Pas d'import `blake3` ou `chacha20poly1305` — crypto déléguée à `crypto_server` via IPC |
| `CAP-01` | `verify_cap_token()` en première instruction de `main.rs` |
| `IPC-01` | `SpscRing` avec `#[repr(C, align(64))]` sur head/tail |
| `IPC-02` | `NetBufRef` et `ExoSocketEntry` sont `Sized`, taille fixe, pas de `Vec`/`String`/`Box` |

### 4.2 Position dans la séquence de boot

```
Étape  7 : virtio-block  ← déjà démarré (P1 CRITIQUE)
Étape  8 : virtio-net    ← PRÉREQUIS : alloue NetBufPool, configure vring
Étape  9 : virtio-console
Étape 10 : network_server ← démarre ici, attend virtio-net via ipc_broker lookup
Étape 11 : scheduler_server
Étape 12 : exo_shield    (Phase 3 seulement)
```

Le `network_server` effectue son lookup `ipc_broker::lookup("virtio-net")` en boucle
avec backoff exponentiel (max 10 tentatives, 100 ms entre chacune). Si virtio-net n'est
pas disponible, le `network_server` logue une erreur et se termine — `init_server` le
relance via `supervisor.rs` avec la politique `RestartPolicy::OnFailure`.

### 4.3 Arborescence des fichiers

```
servers/network_server/
├── Cargo.toml          # no_std, panic=abort, smoltcp features sélectifs
├── src/
│   ├── main.rs         # verify_cap_token() → ipc_broker lookup → poll_loop
│   ├── buf_pool.rs     # NetBufPool : alloc via memory_server, vring setup
│   ├── cap_table.rs    # ExoSocket cap_table[MAX_SOCKETS], syscalls 547-550
│   ├── poll.rs         # smoltcp Interface + poll_loop + mode bascule IRQ/POLL
│   ├── tcp_store.rs    # TcpStateStore : suivi état TCP sérialisable
│   ├── isolation.rs    # PrepareIsolation : sérialise store + flush + Ack
│   └── protocol.rs     # IPC types : NetBufRef, ExoSocketEntry, SocketSnapshot

drivers/virtio-net/
├── Cargo.toml
├── src/
│   ├── main.rs         # claim PCI → setup vring → loop IRQ/POLL
│   ├── virtio.rs       # Négociation features VirtIO 1.1
│   ├── queue.rs        # vring RX/TX, populate descriptors depuis NetBufPool
│   ├── net.rs          # handle_rx_irq(), poll_tick(), bascule mode
│   └── protocol.rs     # NetBufRef{pool_idx: u16, len: u16} = 4 octets
```

Deux fichiers principaux nouveaux (`buf_pool.rs`, `tcp_store.rs`) + extensions des fichiers
existants. Aucune nouvelle dépendance workspace.

### 4.4 Interaction avec crypto_server (règle SRV-02)

Si une application demande une connexion TLS (futur), le `network_server` ne fait aucune
crypto lui-même. Il délègue via IPC au `crypto_server` (PID 4) :

```
network_server → IPC → crypto_server : ChaCha20-Poly1305 encrypt(payload, key)
                                     ← IPC ← retourne payload chiffré
network_server → TX path → NIC
```

Cette délégation est conforme à `SRV-02` et `SRV-04`. Le `network_server` ne voit jamais
de clés en clair — seulement des payloads chiffrés.

### 4.5 Interaction avec exo_shield (Phase 3)

Quand `exo_shield` est actif (Phase 3), Kernel B peut déclencher un cycle Phoenix si une
anomalie réseau est détectée (ex. : corruption du `cap_table`, tentative de bind hors
`NetRights`). Le `network_server` reçoit `PrepareIsolation` via `exo_shield`, exécute la
séquence décrite en §2.4, et le cycle se déroule normalement.

---

## 5. Ce qui n'est PAS dans ce document

Les éléments suivants ont été explicitement exclus après analyse technique :

| Feature | Raison d'exclusion |
|---------|-------------------|
| ExoNIC hardware flow filtering | virtio-net ne supporte pas Intel Flow Director. Feature matérielle physique, indisponible en QEMU. |
| eBPF compatibility | Nécessite un JIT compiler séparé — projet indépendant de plusieurs mois. |
| QUIC natif kernel | QUIC est conçu pour le userspace. L'intégrer dans le kernel est architecturalement incorrect. |
| MPTCP Smart Bonding | Non supporté par smoltcp. Réécriture partielle de la stack TCP requise. |
| AI Traffic Shaping | Dépendance ML — hors scope kernel. |
| InterplanetaryTCP, BioInspiredRouting, NeuralInterface | Fantasy marketing. |
| ExoMesh P2P (STUN/TURN/ICE) | Couche applicative, pas kernel. |

---

## 6. Résumé

Le `network_server` ExoOS est structurellement différent des stacks classiques sur
**trois points non-reproductibles** dans Linux, Windows ou macOS sans refonte complète :

1. **Zero-copy garanti** — les pages DMA du NIC sont partagées directement avec les
   applications via CapTokens. Aucun `memcpy` sur le chemin critique pour les paquets ≥ 256
   octets. Zéro allocation heap en steady-state.

2. **Isolation réseau par construction** — un processus sans CapToken réseau valide est
   structurellement muet. Pas de règles de filtrage. Pas d'autorité ambiante. La policy est
   dans le token, vérifiée en O(1) constant-time.

3. **Connexions TCP survivent à un crash kernel** — grâce à `TcpStateStore` +
   `PrepareIsolation` + ExoPhoenix, les sessions TCP actives perdurent à travers un cycle
   complet de recovery kernel. Aucun autre OS ne propose cette garantie.

---

*ExoOS · Network Module Specification v1.0 · Avril 2026*
