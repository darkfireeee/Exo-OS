# NETWORK STACK REORGANIZATION PLAN

## 🎯 OBJECTIF
Réorganiser `/net` comme `/fs` - propre, modulaire, extensible

## 📁 STRUCTURE CIBLE (inspirée de /fs)

```
kernel/src/net/
├── mod.rs                      # Module principal
├── types.rs                    # Types communs (NetError, NetResult, etc.)
├── constants.rs                # Constantes réseau
│
├── core/                       # Core networking (comme fs/vfs/)
│   ├── mod.rs
│   ├── skb.rs                 ✅ Socket buffer
│   ├── netdev.rs              ✅ Device management
│   ├── buffer.rs              ✅ Buffer pools
│   ├── socket.rs              ✅ Socket core
│   ├── device.rs              ✅ Device abstraction
│   ├── packet.rs              🆕 Packet processing pipeline
│   ├── interface.rs           🆕 Network interface abstraction
│   └── stats.rs               🆕 Statistics collection
│
├── protocols/                  # Protocol implementations (nouveau)
│   ├── mod.rs
│   ├── tcp/                   # TCP protocol
│   │   ├── mod.rs             ✅
│   │   ├── socket.rs          🆕 TCP socket API
│   │   ├── listener.rs        🆕 TCP listener
│   │   ├── connection.rs      ✅ Connection management
│   │   ├── state.rs           ✅ State machine
│   │   ├── segment.rs         ✅ Segment handling
│   │   ├── window.rs          ✅ Window management
│   │   ├── options.rs         ✅ TCP options
│   │   ├── timer.rs           ✅ Timers
│   │   ├── congestion.rs      ✅ Congestion control
│   │   ├── retransmit.rs      ✅ Retransmission
│   │   ├── fastopen.rs        🆕 TCP Fast Open (RFC 7413)
│   │   └── tests.rs           🆕 Unit tests
│   │
│   ├── udp/                   # UDP protocol
│   │   ├── mod.rs             ✅ (fusionner udp.rs + udp/mod.rs)
│   │   ├── socket.rs          🆕 UDP socket API
│   │   ├── multicast.rs       🆕 Multicast support
│   │   └── tests.rs           🆕 Unit tests
│   │
│   ├── ip/                    # IP layer
│   │   ├── mod.rs             ✅
│   │   ├── ipv4.rs            ✅
│   │   ├── ipv6.rs            ✅
│   │   ├── fragmentation.rs   ✅
│   │   ├── routing.rs         ✅
│   │   ├── icmp.rs            🆕 ICMP (déplacer depuis net/icmp.rs)
│   │   ├── icmpv6.rs          ✅
│   │   ├── igmp.rs            🆕 IGMP (multicast)
│   │   └── tunnel.rs          🆕 IP tunneling
│   │
│   ├── ethernet/              # Ethernet layer
│   │   ├── mod.rs             ✅
│   │   ├── vlan.rs            ✅
│   │   ├── arp.rs             🆕 ARP (déplacer depuis net/arp.rs)
│   │   └── bridge.rs          🆕 Ethernet bridging
│   │
│   ├── quic/                  # QUIC protocol (déplacer net/quic.rs)
│   │   ├── mod.rs             🆕
│   │   ├── connection.rs      🆕
│   │   ├── stream.rs          🆕
│   │   ├── crypto.rs          🆕
│   │   └── congestion.rs      🆕
│   │
│   ├── http2/                 # HTTP/2 (déplacer net/http2.rs)
│   │   ├── mod.rs             🆕
│   │   ├── frame.rs           🆕
│   │   ├── stream.rs          🆕
│   │   └── hpack.rs           🆕
│   │
│   └── tls/                   # TLS 1.3 (déplacer net/tls.rs)
│       ├── mod.rs             🆕
│       ├── handshake.rs       🆕
│       ├── record.rs          🆕
│       └── cipher.rs          🆕
│
├── socket/                     # Socket API (comme fs/operations/)
│   ├── mod.rs                 ✅
│   ├── api.rs                 🆕 Socket API public
│   ├── bind.rs                🆕 Bind operations
│   ├── connect.rs             🆕 Connect operations
│   ├── listen.rs              🆕 Listen operations
│   ├── accept.rs              🆕 Accept operations
│   ├── send.rs                🆕 Send operations
│   ├── recv.rs                🆕 Receive operations
│   ├── poll.rs                ✅
│   ├── epoll.rs               ✅
│   ├── select.rs              🆕 Select syscall
│   └── options.rs             🆕 Socket options (SO_*)
│
├── drivers/                    # Network drivers (nouveau)
│   ├── mod.rs                 🆕
│   ├── virtio.rs              🆕 VirtIO network
│   ├── e1000.rs               🆕 Intel E1000
│   ├── rtl8139.rs             🆕 Realtek 8139
│   └── loopback.rs            🆕 Loopback device
│
├── firewall/                   # Firewall (renommer netfilter/)
│   ├── mod.rs                 ✅
│   ├── conntrack.rs           ✅
│   ├── nat.rs                 🆕 NAT
│   ├── rules.rs               🆕 Firewall rules
│   └── tables.rs              🆕 iptables-like
│
├── vpn/                        # VPN protocols
│   ├── mod.rs                 🆕
│   ├── wireguard/             
│   │   ├── mod.rs             ✅
│   │   ├── crypto.rs          ✅
│   │   ├── handshake.rs       ✅
│   │   ├── tunnel.rs          ✅
│   │   ├── peer.rs            🆕
│   │   └── config.rs          🆕
│   ├── ipsec/                 🆕 IPsec
│   │   ├── mod.rs
│   │   ├── esp.rs
│   │   └── ah.rs
│   └── openvpn/               🆕 OpenVPN
│       └── mod.rs
│
├── qos/                        # Quality of Service
│   ├── mod.rs                 🆕 (déplacer net/qos.rs)
│   ├── htb.rs                 🆕 HTB queuing
│   ├── fq_codel.rs            🆕 FQ-CoDel
│   ├── prio.rs                🆕 Priority queuing
│   └── policer.rs             🆕 Rate limiting
│
├── loadbalancer/               # Load balancing
│   ├── mod.rs                 🆕 (déplacer net/loadbalancer.rs)
│   ├── round_robin.rs         🆕
│   ├── least_conn.rs          🆕
│   ├── hash.rs                🆕
│   └── health.rs              🆕 Health checks
│
├── rdma/                       # RDMA support
│   ├── mod.rs                 🆕 (déplacer net/rdma.rs)
│   ├── verbs.rs               🆕 RDMA verbs
│   ├── queue.rs               🆕 Queue pairs
│   └── memory.rs              🆕 Memory regions
│
├── services/                   # Network services (nouveau)
│   ├── mod.rs                 🆕
│   ├── dhcp/                  🆕
│   │   ├── mod.rs             (déplacer net/dhcp.rs)
│   │   ├── client.rs
│   │   └── server.rs
│   ├── dns/                   🆕
│   │   ├── mod.rs             (déplacer net/dns.rs)
│   │   ├── resolver.rs
│   │   ├── cache.rs
│   │   └── server.rs
│   └── ntp/                   🆕 NTP client
│       └── mod.rs
│
├── monitoring/                 # Monitoring & telemetry
│   ├── mod.rs                 🆕 (déplacer net/monitoring.rs)
│   ├── metrics.rs             🆕 Metrics collection
│   ├── tracing.rs             🆕 Packet tracing
│   ├── bpf.rs                 🆕 eBPF support
│   └── prometheus.rs          🆕 Prometheus exporter
│
└── tests/                      # Tests (nouveau)
    ├── mod.rs                 🆕
    ├── tcp_tests.rs           🆕
    ├── udp_tests.rs           🆕
    ├── integration.rs         🆕
    └── benchmarks.rs          🆕
```

## 🔄 ACTIONS À FAIRE

### 1. Nettoyer les doublons
- [ ] Fusionner `/net/udp.rs` et `/net/udp/mod.rs`
- [ ] Fusionner `/net/buffer.rs` et `/net/core/buffer.rs`
- [ ] Fusionner `/net/socket.rs` et `/net/socket/mod.rs`
- [ ] Vérifier `/net/routing.rs` vs `/net/ip/routing.rs`

### 2. Créer nouveaux modules
- [ ] `protocols/` - Regrouper tous les protocoles
- [ ] `drivers/` - Drivers réseau
- [ ] `services/` - DHCP, DNS, etc.
- [ ] `tests/` - Tests unitaires et intégration

### 3. Déplacer fichiers
- [ ] `net/quic.rs` → `protocols/quic/mod.rs` + split
- [ ] `net/http2.rs` → `protocols/http2/mod.rs` + split
- [ ] `net/tls.rs` → `protocols/tls/mod.rs` + split
- [ ] `net/arp.rs` → `protocols/ethernet/arp.rs`
- [ ] `net/icmp.rs` → `protocols/ip/icmp.rs`
- [ ] `net/dhcp.rs` → `services/dhcp/mod.rs`
- [ ] `net/dns.rs` → `services/dns/mod.rs`
- [ ] `net/qos.rs` → `qos/mod.rs` + split
- [ ] `net/loadbalancer.rs` → `loadbalancer/mod.rs` + split
- [ ] `net/rdma.rs` → `rdma/mod.rs` + split
- [ ] `net/monitoring.rs` → `monitoring/mod.rs` + split
- [ ] `netfilter/` → `firewall/`

### 4. Créer fichiers manquants
Voir liste détaillée ci-dessus (marqués 🆕)

## 📊 STATISTIQUES CIBLES

- **Avant**: ~48 fichiers, organisation chaotique
- **Après**: ~120+ fichiers, organisation modulaire
- **Gain**: 3x plus de fichiers, mais 10x plus organisé
