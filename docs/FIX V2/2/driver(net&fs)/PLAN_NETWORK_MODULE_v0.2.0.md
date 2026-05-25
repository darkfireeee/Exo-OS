# ExoOS v0.2.0 — Plan de création du module Network
**Auteur:** claude-alpha  
**Date:** 2026-05-21  
**Périmètre:** `drivers/network/` complet + `servers/network_server/` complété  
**Référence architecture:** ExoOS Architecture v7, Ring1 startup V4 canonique (PID 11)

---

## 1. Diagnostic de l'état actuel

### Ce qui existe et est fonctionnel
| Fichier | État |
|---|---|
| `servers/network_server/src/main.rs` | ✅ Complet (V4) |
| `servers/network_server/src/protocol.rs` | ✅ Complet — NetMsg/NetReply 48B |
| `servers/network_server/src/socket_table.rs` | ✅ Complet — 64 sockets, gestion, génération |
| `servers/network_server/src/smoltcp_iface.rs` | ✅ Complet — poll_ingress_single + poll_egress |
| `servers/network_server/src/driver_link.rs` | ✅ Complet — handshake DriverInitMsg + flush RxReleaseMsg |
| `servers/network_server/src/buf_pool.rs` | ✅ Complet — DMA pool RX/TX |
| `servers/network_server/src/virtio_device.rs` | ✅ Complet |
| `servers/network_server/src/isolation.rs` | ✅ Complet — ExoPhoenix drain/restore |
| `servers/network_server/src/tcp_store.rs` | ✅ Complet |
| `kernel/src/syscall/net_bridge.rs` | ✅ Complet — 16 syscalls BSD routés |
| `drivers/network/loopback/src/main.rs` | ✅ Stub fonctionnel |
| `drivers/network/virtio_net/src/net.rs` | ⚠️ Statemachine logicielle uniquement — aucun accès MMIO |
| `drivers/network/virtio_net/src/main.rs` | ⚠️ IPC loop présente mais sans init PCI ni ISR |

### Ce qui est vide (à créer intégralement)
- `drivers/network/e1000/` — driver Intel e1000 (PCI 0x8086:0x100E) entièrement absent
- `drivers/network/virtio_net/src/pci.rs` — découverte PCI + MMIO mapping — ABSENT
- `drivers/network/virtio_net/src/virtqueue.rs` — ring DMA réel — ABSENT
- `drivers/network/virtio_net/src/interrupt.rs` — ISR + registration SYS_IRQ_REGISTER — ABSENT
- `drivers/network/evdev/` — driver evdev input (scope futur, hors réseau)
- `servers/network_server/src/dhcp.rs` — client DHCP (manquant pour IP dynamique)

### Lacune critique dans `virtio_net` actuel
Le fichier `net.rs` existant est un **automate logiciel correct** pour l'IPC et la gestion des buffers, mais il ne fait aucune des choses suivantes :
- Lire/écrire les registres MMIO VirtIO (Device Status, Feature bits, Queue Select, etc.)
- Allouer de la mémoire DMA pour les descriptor tables
- Notifier l'hôte via `QueueNotify` (offset 0x50 MMIO legacy)
- Lire la MAC depuis `NetConfig` (offset 0x100+ dans config space)
- Enregistrer un ISR pour l'IRQ VirtIO

---

## 2. Arborescence complète du module Network

```
drivers/network/
├── Cargo.toml                          ← workspace member declaration
│
├── virtio_net/
│   ├── Cargo.toml                      ← dépendances: exo-syscall-abi, spin
│   └── src/
│       ├── main.rs                     ← _start: register endpoint, IPC loop, dispatch
│       ├── net.rs                      ← [EXISTANT] statemachine logicielle RX/TX pool
│       ├── pci.rs                      ← [NOUVEAU] découverte PCI + MMIO BAR0 mapping
│       ├── virtqueue.rs                ← [NOUVEAU] Vring DMA split-ring, descriptor table
│       ├── interrupt.rs                ← [NOUVEAU] ISR registration + handler used-ring poll
│       ├── mac.rs                      ← [NOUVEAU] lecture MAC depuis NetConfig space
│       └── config.rs                   ← [NOUVEAU] constantes registres VirtIO 1.2 legacy/modern
│
├── e1000/
│   ├── Cargo.toml                      ← dépendances: exo-syscall-abi, spin
│   └── src/
│       ├── main.rs                     ← _start: register endpoint, IPC loop
│       ├── e1000.rs                    ← driver principal: init, RX/TX descriptors
│       ├── pci.rs                      ← découverte PCI 0x8086:0x100E, BAR0 MMIO
│       ├── regs.rs                     ← constantes registres e1000 (CTRL, STATUS, RCTL, etc.)
│       ├── rx.rs                       ← ring RX: descripteurs, refill, used poll
│       ├── tx.rs                       ← ring TX: descripteurs, submission, flush
│       ├── interrupt.rs                ← ISR: ICR read, IMS, IMC
│       └── mac.rs                      ← lecture MAC depuis RAL/RAH registers
│
├── loopback/
│   ├── Cargo.toml                      ← [EXISTANT]
│   └── src/
│       ├── main.rs                     ← [EXISTANT] IPC loop avec echo accounting
│       ├── state.rs                    ← [NOUVEAU] LoopbackState étendu pour stats ICMP/TCP
│       └── echo.rs                     ← [NOUVEAU] echo réel de paquets RX → TX (127.0.0.1)
│
└── common/
    ├── Cargo.toml                      ← lib partagée (no_std, no alloc)
    └── src/
        ├── lib.rs                      ← re-exports
        ├── ether.rs                    ← EthernetHeader parsing (src/dst MAC, ethertype)
        ├── ipv4.rs                     ← IPv4Header parsing (src/dst IP, proto, checksum)
        ├── pci_scan.rs                 ← itérateur PCI bus/device/function (0-255/0-31/0-7)
        └── dma_buf.rs                  ← DmaBuffer wrapper (phys_addr, virt_ptr, size)

servers/network_server/
├── Cargo.toml                          ← [EXISTANT] smoltcp workspace dep
├── EXONET_V4_AUDIT.md                  ← [EXISTANT]
├── src/
│   ├── main.rs                         ← [EXISTANT] NetworkService, bootstrap, dispatch loop
│   ├── protocol.rs                     ← [EXISTANT] NetMsg/NetReply/DriverInitMsg/RxReleaseMsg
│   ├── socket_table.rs                 ← [EXISTANT] 64 sockets, état, génération
│   ├── smoltcp_iface.rs                ← [EXISTANT] Interface smoltcp, poll_ingress/egress
│   ├── driver_link.rs                  ← [EXISTANT] IPC handshake + RxRelease flush
│   ├── buf_pool.rs                     ← [EXISTANT] DMA pool pages RX/TX
│   ├── virtio_device.rs                ← [EXISTANT] ExoNetDevice smoltcp phy adapter
│   ├── isolation.rs                    ← [EXISTANT] ExoPhoenix state
│   ├── tcp_store.rs                    ← [EXISTANT] TCP state store
│   ├── dhcp.rs                         ← [NOUVEAU] client DHCP minimal (DISCOVER/OFFER/REQUEST/ACK)
│   ├── routing.rs                      ← [NOUVEAU] table de routage statique + gateway default
│   ├── stats.rs                        ← [NOUVEAU] compteurs atomiques TX/RX bytes/packets/drops
│   └── icmp.rs                         ← [NOUVEAU] réponses ICMP Echo (ping) via smoltcp raw socket
└── tests/
    ├── exonet_stress.rs                ← [EXISTANT] stress test TLA state machine
    ├── dhcp_state.rs                   ← [NOUVEAU] test automate DHCP 4 phases
    └── routing_table.rs                ← [NOUVEAU] test insertion/lookup route

kernel/src/syscall/
├── net_bridge.rs                       ← [EXISTANT COMPLET] 16 syscalls BSD → IPC network_server
└── (aucun fichier supplémentaire nécessaire pour la partie réseau)
```

---

## 3. Instructions de création par module

### 3.1 `drivers/network/virtio_net/src/config.rs` [NOUVEAU]

Définir toutes les constantes de registres VirtIO 1.2 en mode legacy MMIO (QEMU par défaut) et moderne PCI.

**Contenu requis :**
```
// Offsets registres MMIO legacy (VirtIO 0.9.x — QEMU -device virtio-net-pci sans version moderne)
VIRTIO_MMIO_MAGIC_VALUE         = 0x000  // doit lire 0x74726976 ("virt")
VIRTIO_MMIO_VERSION             = 0x004  // 1 = legacy, 2 = modern
VIRTIO_MMIO_DEVICE_ID           = 0x008  // 1 = net, 2 = blk
VIRTIO_MMIO_VENDOR_ID           = 0x00C  // 0x554D4551 = QEMU
VIRTIO_MMIO_DEVICE_FEATURES     = 0x010
VIRTIO_MMIO_DEVICE_FEATURES_SEL = 0x014
VIRTIO_MMIO_DRIVER_FEATURES     = 0x020
VIRTIO_MMIO_DRIVER_FEATURES_SEL = 0x024
VIRTIO_MMIO_QUEUE_SEL           = 0x030
VIRTIO_MMIO_QUEUE_NUM_MAX       = 0x034
VIRTIO_MMIO_QUEUE_NUM           = 0x038
VIRTIO_MMIO_QUEUE_ALIGN         = 0x03C  // legacy uniquement
VIRTIO_MMIO_QUEUE_PFN           = 0x040  // legacy: adresse physique >> PAGE_SHIFT
VIRTIO_MMIO_QUEUE_READY         = 0x044  // moderne uniquement
VIRTIO_MMIO_QUEUE_NOTIFY        = 0x050
VIRTIO_MMIO_INTERRUPT_STATUS    = 0x060
VIRTIO_MMIO_INTERRUPT_ACK       = 0x064
VIRTIO_MMIO_STATUS              = 0x070
VIRTIO_MMIO_CONFIG              = 0x100  // début de la zone config device

// Status bits
VIRTIO_STATUS_ACKNOWLEDGE       = 1
VIRTIO_STATUS_DRIVER            = 2
VIRTIO_STATUS_DRIVER_OK         = 4
VIRTIO_STATUS_FEATURES_OK       = 8
VIRTIO_STATUS_FAILED            = 128

// Feature bits réseau
VIRTIO_NET_F_MAC                = 1u64 << 5
VIRTIO_NET_F_STATUS             = 1u64 << 16
VIRTIO_NET_F_MRG_RXBUF          = 1u64 << 15
VIRTIO_NET_F_CSUM               = 1u64 << 0
VIRTIO_NET_F_GUEST_CSUM         = 1u64 << 1

// PCI IDs VirtIO-net
VIRTIO_PCI_VENDOR               = 0x1AF4
VIRTIO_NET_PCI_DEVICE_LEGACY    = 0x1000
VIRTIO_NET_PCI_DEVICE_MODERN    = 0x1041

// Tailles
VIRTIO_NET_HDR_SIZE_LEGACY      = 10   // struct virtio_net_hdr sans merge flag
VIRTIO_NET_HDR_SIZE_MRG         = 12   // avec num_buffers
VRING_QUEUE_SIZE                = 256  // doit être puissance de 2
```

**Règle architecturale:** Toutes les lectures/écritures MMIO se font via `core::ptr::read_volatile` et `core::ptr::write_volatile`. Ne jamais utiliser un load/store ordinaire sur une adresse MMIO.

---

### 3.2 `drivers/network/virtio_net/src/pci.rs` [NOUVEAU]

**Responsabilité:** Trouver le device VirtIO-net sur le bus PCI, lire BAR0 pour l'adresse MMIO, activer Bus Master + Memory Space dans la commande PCI.

**Logique de création :**

1. Itérer bus 0-255, device 0-31, function 0-7 via les ports I/O PCI (0xCF8/0xCFC) ou via ACPI MCFG (ECAM). Sur QEMU en mode x86_64, le bus PCI est accessible par I/O.

2. Pour chaque combinaison, lire le registre `vendor_id:device_id` (offset 0x00 dans l'espace de configuration PCI). Cibler :
   - `0x1AF4:0x1000` (virtio-net legacy PCI)
   - `0x1AF4:0x1041` (virtio-net modern PCIe)

3. Une fois trouvé, lire `BAR0` (offset 0x10). Si le bit 0 est 0 → MMIO, masquer les 4 bits bas pour obtenir l'adresse physique. Si le bit 0 est 1 → I/O port, adapter.

4. Activer Bus Master (bit 2) et Memory Space (bit 1) dans le registre Command PCI (offset 0x04).

5. Retourner l'adresse physique de BAR0 pour que `pci.rs` la passe à la HAL kernel (`SYS_PHYSMAP_MAP` ou équivalent `CAP_PHYSMAP`) pour obtenir une adresse virtuelle accessible.

**Attention :** Le driver tourne en Ring1. Il doit utiliser `SYS_PCI_CLAIM` (syscall 540) suivi de `SYS_MMIO_MAP` pour mapper le BAR dans son espace d'adressage. Il ne peut pas accéder directement aux ports physiques en Ring1 sans CAP.

**Structure de retour requise :**
```rust
pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub bar0_phys: u64,        // adresse physique BAR0 MMIO
    pub bar0_virt: *mut u8,    // adresse virtuelle mappée par le kernel
    pub irq_line: u8,          // depuis PCI config offset 0x3C
    pub irq_pin: u8,           // depuis PCI config offset 0x3D
}
```

---

### 3.3 `drivers/network/virtio_net/src/virtqueue.rs` [NOUVEAU]

**Responsabilité:** Implémenter un split virtqueue DMA réel conforme VirtIO 1.2 §2.7.

**Structure de la mémoire d'un virtqueue split-ring (layout linéaire) :**
```
[ Descriptor Table ][ Available Ring ][ padding ][ Used Ring ]
  N × 16 bytes        2 + N × 2 bytes   (align)   2 + 2 + N × 8 bytes
```

Pour N=256 (VRING_QUEUE_SIZE) :
- Descriptor table : 256 × 16 = 4096 bytes
- Available ring   : 4 + 256 × 2 = 516 bytes → aligné à 2
- (padding jusqu'à page boundary = 4096)
- Used ring        : 4 + 2 + 256 × 8 = 2054 bytes

**Allocation DMA :** Appeler `SYS_DMA_ALLOC` (syscall 534) pour obtenir des pages physiquement contiguës. La taille totale pour N=256 est 2 pages (8192 bytes). Stocker l'adresse physique `phys_addr` et l'adresse virtuelle `virt_ptr`.

**Membres de la structure `Virtqueue` :**
```rust
pub struct Virtqueue {
    pub phys_base: u64,            // adresse physique page 0 (descriptor table)
    pub virt_base: *mut u8,        // adresse virtuelle correspondante
    pub queue_size: u16,           // N (256)
    pub desc:  *mut VirtqDesc,     // virt_base + 0
    pub avail: *mut VirtqAvail,    // virt_base + N*16
    pub used:  *mut VirtqUsed,     // virt_base + USED_RING_OFFSET (page aligné)
    pub free_head: u16,            // première desc libre
    pub free_count: u16,           // nombre de descs libres
    pub last_used_idx: u16,        // index du dernier used entry consommé
}
```

**Opérations obligatoires :**
- `init(queue_size: u16) -> Result<Self, Error>` — alloue DMA, configure pointeurs, crée free list chaînée dans `desc[i].next`
- `add_buffer(addr: u64, len: u32, flags: u16) -> Result<u16, Error>` — alloue une desc, écrit addr/len/flags, met à jour avail ring, retourne l'index
- `add_chain(bufs: &[(u64, u32, u16)]) -> Result<u16, Error>` — enchaîne plusieurs descs avec VIRTQ_DESC_F_NEXT
- `notify(mmio: *mut u8, queue_idx: u16)` — écrit queue_idx dans VIRTIO_MMIO_QUEUE_NOTIFY
- `poll_used() -> Option<(u16, u32)>` — lit `used.ring[last_used_idx % N]`, retourne (desc_id, bytes_written)
- `recycle_desc(head: u16)` — suit la chaîne next pour libérer toutes les descs

**Règle critique (issue historique ExoOS) :** Tous les accès à `avail.idx` (incrementé par le driver) et `used.idx` (écrit par le device) doivent utiliser un fence mémoire (`core::sync::atomic::fence(Ordering::Release)` avant notify, `Ordering::Acquire` après poll_used). Sans fence, le device peut voir des descripteurs corrompus.

---

### 3.4 `drivers/network/virtio_net/src/interrupt.rs` [NOUVEAU]

**Responsabilité:** Enregistrer un ISR auprès du kernel via `SYS_IRQ_REGISTER` (syscall 530), et implémenter le handler qui poll le used ring RX.

**Séquence d'initialisation :**
1. Récupérer `irq_line` depuis `pci.rs`
2. Appeler `SYS_IRQ_REGISTER(irq_line, endpoint_id, IRQ_FLAG_SHARED)` — le kernel envoie un message IPC quand l'IRQ se déclenche
3. Dans la boucle IPC de `main.rs`, traiter les messages de type `MSG_IRQ_NOTIFY` en appelant `handle_irq()`

**Handler `handle_irq()` :**
```
1. Lire VIRTIO_MMIO_INTERRUPT_STATUS (offset 0x60) via read_volatile
2. Si bit 0 set → Used Ring Update (des buffers ont été traités)
   a. Poll la RX used ring : pour chaque entry (desc_id, total_len) :
      - Calculer pool_idx depuis desc_id
      - Appeler VirtioNet::handle_rx_used(pool_idx, total_len)
   b. Envoyer RxReleaseMsg au network_server si des slots sont disponibles
3. Si bit 1 set → Configuration Changed (MAC a changé, rare)
4. Écrire VIRTIO_MMIO_INTERRUPT_ACK (offset 0x64) = valeur lue en (1)
```

**Règle ISR (FIX-108/109 ExoOS) :** Le handler ISR ne doit JAMAIS allouer de mémoire, ni appeler yield. Il met à jour des drapeaux atomiques et retourne immédiatement. Le traitement réel (poll used ring) se fait dans la boucle IPC principale, déclenchée par le message IRQ.

---

### 3.5 `drivers/network/virtio_net/src/mac.rs` [NOUVEAU]

**Responsabilité:** Lire l'adresse MAC depuis l'espace de configuration du device VirtIO.

**Procédure :**
- L'adresse MAC est stockée à `BAR0 + VIRTIO_MMIO_CONFIG + 0` (6 octets, un par un)
- Lire 6 fois `u8` via `read_volatile(config_base.add(i))`
- Vérifier que `VIRTIO_NET_F_MAC` est négocié (sinon MAC aléatoire)

**Réponse au network_server :**
Quand le driver reçoit `NET_CTRL_MAC_QUERY` (0x4F02) du network_server, il doit répondre avec un message `NET_CTRL_MAC_REPLY` (0x4F03) contenant les 6 octets MAC dans le payload. C'est le seul cas actuellement non géré dans `main.rs` existant (`_ => {}` sans reply).

---

### 3.6 `drivers/network/virtio_net/src/main.rs` — mise à jour [MODIFICATION]

Réécrire le `_start` pour intégrer la séquence d'initialisation VirtIO complète :

**Séquence d'init VirtIO 1.2 §3.1 (ordre obligatoire) :**
```
1. Reset device : écrire 0 dans STATUS
2. Écrire STATUS |= ACKNOWLEDGE
3. Écrire STATUS |= DRIVER
4. Négocier features :
   a. Lire DEVICE_FEATURES (features sel = 0, puis sel = 1 pour bits 32-63)
   b. Écrire DRIVER_FEATURES = features négociées (garder: F_MAC | F_MRG_RXBUF si dispo)
5. Écrire STATUS |= FEATURES_OK
6. Relire STATUS, vérifier FEATURES_OK toujours set (sinon: device reject)
7. Configurer les virtqueues (QUEUE_SEL=0 pour RX, QUEUE_SEL=1 pour TX) :
   a. Lire QUEUE_NUM_MAX
   b. Écrire QUEUE_NUM = min(QUEUE_NUM_MAX, VRING_QUEUE_SIZE)
   c. Allouer DMA pour le vring
   d. Écrire QUEUE_PFN = phys_addr >> PAGE_SHIFT (legacy)
   e. Pré-remplir la RX queue avec RX_POOL_SIZE buffers vides
8. Lire MAC depuis NetConfig
9. Enregistrer IRQ via SYS_IRQ_REGISTER
10. Écrire STATUS |= DRIVER_OK
11. Envoyer NET_CTRL_MAC_REPLY au network_server
12. Entrer dans la boucle IPC
```

**Après init, la boucle IPC traite :**
- `NET_CTRL_DRIVER_INIT` → `apply_driver_init` (met à jour pool IOVAs depuis network_server)
- `NET_CTRL_RX_RELEASE` → `process_rx_releases` + repopuler les descs RX libérées
- `NET_CTRL_MAC_QUERY` → `send_mac_reply`
- `MSG_IRQ_NOTIFY` → `handle_irq` → poll used ring, créer RxReleaseMsg vers network_server
- TX : quand `tx_from_network` non vide, ajouter bufs dans TX queue + `notify`

---

### 3.7 `drivers/network/e1000/` — driver complet [NOUVEAU]

Le driver e1000 cible le NIC Intel 82540EM émulé par QEMU (`-device e1000`), PCI ID `0x8086:0x100E`.

**`src/regs.rs`** — constantes registres MMIO e1000 :
```
CTRL    = 0x0000  // Device Control
STATUS  = 0x0008  // Device Status
EECD    = 0x0010  // EEPROM/Flash Control
EERD    = 0x0014  // EEPROM Read
CTRL_EXT= 0x0018
ICR     = 0x00C0  // Interrupt Cause Read (lecture clear)
IMS     = 0x00D0  // Interrupt Mask Set
IMC     = 0x00D8  // Interrupt Mask Clear
RCTL    = 0x0100  // Receive Control
TCTL    = 0x0400  // Transmit Control
TIPG    = 0x0410  // Transmit IPG
RDBAL   = 0x2800  // RX Desc Base Low
RDBAH   = 0x2804  // RX Desc Base High
RDLEN   = 0x2808  // RX Desc Ring Length (bytes)
RDH     = 0x2810  // RX Desc Head
RDT     = 0x2818  // RX Desc Tail
TDBAL   = 0x3800  // TX Desc Base Low
TDBAH   = 0x3804  // TX Desc Base High
TDLEN   = 0x3808  // TX Desc Ring Length
TDH     = 0x3810  // TX Head
TDT     = 0x3818  // TX Tail
RAL     = 0x5400  // Receive Address Low
RAH     = 0x5404  // Receive Address High
MTA     = 0x5200  // Multicast Table Array (128 × 32bit)
```

**`src/rx.rs`** — ring RX e1000 :
- Descriptor RX e1000 : `{ addr: u64, length: u16, csum: u16, status: u8, errors: u8, special: u16 }` (16 bytes)
- Ring size = 256 descripteurs → allouer via SYS_DMA_ALLOC
- `status & 0x01` = DD (Descriptor Done) → packet disponible
- `status & 0x02` = EOP (End Of Packet)
- Après consommation: remettre addr physique dans le descriptor, écrire RDT = head-1

**`src/tx.rs`** — ring TX e1000 :
- Descriptor TX e1000 : `{ addr: u64, length: u16, csum_offset: u8, cmd: u8, status: u8, csum_start: u8, special: u16 }` (16 bytes)
- CMD = `IFCS | EOP | RS` (0x0B) pour un paquet complet
- Vérifier `status & 0x01` (DD) avant réutilisation
- Écrire TDT pour notifier le device

**`src/e1000.rs`** — init e1000 :
```
1. Reset : CTRL |= RST, attendre ~10μs, relire CTRL jusqu'à RST clear
2. Désactiver interruptions : IMC = 0xFFFFFFFF
3. Configurer RCTL :
   EN | BAM (broadcast) | SBP | UPE | MPE | RDMTS_HALF | SECRC
   BSIZE = 0b11 (2048 bytes buffer)
4. Configurer TCTL :
   EN | PSP | COLD=0x40 | CT=0x0F
5. Configurer TIPG = 0x00602006 (valeurs Intel pour 1Gbps)
6. Configurer rings RX et TX (base physique + longueur + head/tail = 0)
7. Lire MAC depuis RAL/RAH
8. Activer interruptions : IMS = RXT0|RXO|RXDMT0|RXSEQ|LSC (0x1F)
9. RCTL |= EN (activer réception)
10. Enregistrer IRQ via SYS_IRQ_REGISTER
```

**`src/interrupt.rs`** — ISR e1000 :
```
1. Lire ICR (auto-clear à la lecture)
2. Si ICR & RXT0 → RX packet disponible : poll RDH, consommer descriptors DD
3. Si ICR & TXDW → TX done : recycler descriptors
4. Si ICR & LSC → Link Status Change : log
5. EOI implicite (ICR read suffit pour e1000)
```

---

### 3.8 `servers/network_server/src/dhcp.rs` [NOUVEAU]

**Responsabilité:** Client DHCP minimal permettant à network_server d'obtenir une IP dynamique si `/etc/network.conf` est absent.

**Automate DHCP (RFC 2131) :**
```
INIT → SELECTING → REQUESTING → BOUND → RENEWING → BOUND ...
```

**Implémentation :**
- Utiliser `smoltcp::socket::udp::Socket` via `SmoltcpIface` sur port UDP 68 (client) vers port 67 (server)
- DHCPDISCOVER : broadcast 255.255.255.255, champ client MAC, Option 53 = 1
- DHCPOFFER : parser lease IP, server IP, Options 1 (mask), 3 (gateway), 6 (DNS), 51 (lease time)
- DHCPREQUEST : accepter l'offre, répéter les paramètres
- DHCPACK : confirmer, appliquer IP + prefix + gateway dans `SmoltcpIface`

**Intégration dans `main.rs` :** Appeler `dhcp.poll()` dans le `tick()` de NetworkService, après `iface.poll_one()`. Si `configured_ipv4()` retourne le fallback (car `/etc/network.conf` absent), lancer le DHCP.

**Structure `DhcpState` :**
```rust
pub struct DhcpClient {
    state: DhcpPhase,         // Init / Selecting / Requesting / Bound
    xid: u32,                  // transaction ID aléatoire
    offered_ip: u32,           // IP proposée par le serveur
    server_ip: u32,            // IP du serveur DHCP
    lease_until_tick: u64,     // tick à partir duquel renouveler
    mac: [u8; 6],              // MAC du device
}
```

---

### 3.9 `servers/network_server/src/routing.rs` [NOUVEAU]

Table de routage statique minimaliste pour smoltcp.

**Structure :**
```rust
pub struct RouteTable {
    entries: [RouteEntry; MAX_ROUTES],   // MAX_ROUTES = 8
    count: usize,
}

pub struct RouteEntry {
    pub dest_net: u32,     // réseau destination
    pub prefix_len: u8,    // longueur du masque
    pub gateway: u32,      // 0 = on-link, sinon adresse gateway
    pub metric: u8,        // priorité (plus bas = prioritaire)
}
```

**Opérations :**
- `add(dest, prefix, gateway, metric)` — ajouter une route
- `lookup(dst_ip: u32) -> Option<u32>` — longest prefix match, retourne le prochain saut
- `default_gateway() -> Option<u32>` — retourne la route 0.0.0.0/0

**Intégration smoltcp :** Après obtention d'une IP (DHCP ou statique), appeler `iface.update_ip_addrs()` et `iface.routes_mut().add_default_ipv4_route(gateway)`.

---

### 3.10 `drivers/network/loopback/src/echo.rs` [NOUVEAU]

Implement un vrai echo loopback : les paquets reçus sur 127.0.0.1 sont réinjectés comme paquets TX.

**Comportement attendu :**
- Recevoir un paquet RX avec IP dest = 127.0.0.1
- Swap src/dst MAC (identique, 00:00:00:00:00:00 pour loopback)
- Swap src/dst IP
- Pour TCP : swap ports + gérer les flags
- Réinjecter dans la TX queue

**Note :** Le driver loopback existant est un stub comptable. Cette extension le rend fonctionnel pour le ping local (127.0.0.1) et les connexions localhost, requis pour les tests unitaires exosh.

---

## 4. Relations IPC et séquence de démarrage Ring1

```
Boot kernel (step 18: DRIVER_OK)
    │
    ├── [PID 2]  ipc_broker    → register endpoint "ipc_broker"
    ├── [PID 3]  memory_server → register endpoint "memory_server"
    ├── [PID 1]  init_server   → orchestre la suite
    ├── [PID 4]  vfs_server    → monte ExoFS (nécessaire pour /etc/network.conf)
    │
    ├── [PID 11] network_server (Ring1):
    │       1. register_endpoint("network_server", ep=7)
    │       2. bootstrap():
    │          a. NetBufPool::init() → SYS_DMA_ALLOC
    │          b. DriverLink::connect_virtio_net() → SYS_IPC_LOOKUP("virtio_net")
    │          c. Si trouvé: envoyer NET_CTRL_DRIVER_INIT avec pool IOVAs
    │          d. Envoyer NET_CTRL_MAC_QUERY
    │          e. SmoltcpIface::init(mac, ip, prefix)
    │          f. configured_ipv4() → lire /etc/network.conf → si absent: DHCP
    │
    ├── [PID 12] exo-virtio-net-driver (Ring1, ou Ring3):
    │       1. register_endpoint("virtio_net", ep=14)
    │       2. PCI scan → trouver 0x1AF4:0x1000
    │       3. SYS_PCI_CLAIM(bus, dev, fn)
    │       4. SYS_MMIO_MAP(bar0_phys, size) → bar0_virt
    │       5. Séquence init VirtIO 1.2 §3.1
    │       6. Lire MAC
    │       7. SYS_IRQ_REGISTER(irq_line, ep=14)
    │       8. Attendre NET_CTRL_DRIVER_INIT de network_server
    │       9. apply_driver_init() → mapper pool IOVAs dans RX queue
    │       10. Répondre NET_CTRL_MAC_REPLY
    │       11. Boucle IPC
    │
    └── [PID 13] exo-loopback-driver (Ring1):
            1. register_endpoint("loopback_net", ep=15)
            2. Boucle IPC echo
```

**Message flow RX complet (paquet réseau arrivant) :**
```
QEMU virtio-net device → IRQ → ISR virtio_net driver
    → poll used RX ring
    → handle_rx_used(pool_idx, len)
    → boucle IPC: envoyer RxReleaseMsg vers network_server (ep=7)
    → network_server: ExoNetDevice::push_rx_for_stack(pool_idx, len)
    → SmoltcpIface::poll_one() → iface.poll_ingress_single()
    → smoltcp traite le paquet → met à jour l'état du socket
    → application userspace lit via syscall SYS_RECVFROM → net_bridge → NetMsg
```

**Message flow TX complet :**
```
Application → SYS_SENDTO → net_bridge → NetMsg NET_OP_SENDTO
    → network_server: handle_sendto() → socket_table.send_to()
    → ExoNetDevice::submit_tx(pool, len)
    → SmoltcpIface::poll_egress() → ExoTxToken::consume()
    → ExoNetDevice::queue_tx_idx(pool_idx, len)
    → DriverLink::flush_released() → envoyer vers virtio_net (pas NET_CTRL_TX_SUBMIT car le driver le fait via notify)
    → virtio_net: ajouter desc dans TX queue → QueueNotify
    → QEMU envoie le paquet sur le réseau
```

---

## 5. Contraintes et règles techniques

### Règles de base obligatoires
- **DRV-ARCH-01** : Zéro logique driver en Ring0. Tout accès MMIO se fait depuis le processus driver en Ring1 après mapping via `SYS_MMIO_MAP`.
- **FIX-108** : L'ISR ne fait pas d'allocation mémoire.
- **FIX-109** : L'ISR ne peut pas yield ni bloquer.
- **SRV-01** : network_server ne démarre qu'après ipc_broker.
- **SRV-02** : network_server ne fait pas d'appel bloquant sans timeout.
- **IPC-01** : Tous les IPC utilisent SYS_IPC_SEND/RECV avec IPC_FLAG_TIMEOUT.

### Contraintes mémoire virtio
- Les pool IOVAs communiqués par network_server via `DriverInitMsg` sont des adresses physiques DMA (obtenues par `SYS_DMA_ALLOC`). Le driver virtio_net ne doit PAS les convertir — il les donne directement aux descripteurs VirtIO tel quel.
- La `buf_pool.rs` du network_server garantit déjà l'alignement page (4096 bytes par buffer) requis par VirtIO.

### Contrainte smoltcp (issue historique V4)
- `SmoltcpIface::poll_ingress_single()` retourne `PacketProcessed` ou `SocketStateChanged` quand il y a du travail. La boucle dans `tick()` doit appeler `poll_one()` **en boucle** jusqu'à retour `NothingToProcess`, sinon des paquets restent dans la queue device.
- `poll_egress()` doit être appelé après chaque `poll_one()` pour vider les ACKs TCP générés par smoltcp.

### Versions de dépendances (workspace)
```toml
smoltcp = { version = "0.12", default-features = false, features = [
    "medium-ethernet", "proto-ipv4", "socket-tcp", "socket-udp",
    "socket-raw", "proto-dhcpv4"
] }
virtio-drivers = { version = "0.7.2", default-features = false }
spin = { version = "0.9.8", features = ["spin_mutex", "once"] }
```

---

## 6. Tests requis pour valider v0.2.0 Network

| Test | Fichier | Critère |
|---|---|---|
| VirtIO init séquence | `virtio_net/tests/init.rs` | STATUS lit DRIVER_OK après init |
| RX pool refill | `exonet_stress.rs` (existant) | 0 leak après 1000 cycles |
| MAC reply | `virtio_net/tests/mac.rs` | MAC lue = MAC répondue |
| Ping 127.0.0.1 | `network_server/tests/loopback.rs` | ICMP echo reply en < 1ms |
| TCP connect localhost | `network_server/tests/tcp_local.rs` | connect + send + recv + close |
| DHCP 4 phases | `dhcp_state.rs` | BOUND atteint, IP assignée |
| Route lookup | `routing_table.rs` | LPM correct sur 4 entrées |
| e1000 init | `e1000/tests/init.rs` | RCTL|EN set, IMS configuré |
