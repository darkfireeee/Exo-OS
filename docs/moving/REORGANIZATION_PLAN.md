# 📦 PLAN DE RÉORGANISATION DES FICHIERS

## Fichiers à déplacer AVANT développement

### 🔴 PRIORITÉ HAUTE - À déplacer immédiatement

#### 1. **ARP** → `protocols/ethernet/`
- ❌ `/net/arp.rs` → ✅ `/net/protocols/ethernet/arp.rs`

#### 2. **ICMP** → `protocols/ip/`
- ❌ `/net/icmp.rs` → ✅ `/net/protocols/ip/icmp.rs`

#### 3. **DHCP** → `services/dhcp/`
- ❌ `/net/dhcp.rs` → ✅ `/net/services/dhcp/client.rs`
- Créer `/net/services/dhcp/mod.rs`

#### 4. **DNS** → `services/dns/`
- ❌ `/net/dns.rs` → ✅ `/net/services/dns/client.rs`
- Créer `/net/services/dns/mod.rs`

#### 5. **QUIC** → `protocols/quic/` (à splitter)
- ❌ `/net/quic.rs` (1200 lignes) → Diviser en:
  - `/net/protocols/quic/mod.rs`
  - `/net/protocols/quic/connection.rs`
  - `/net/protocols/quic/stream.rs`
  - `/net/protocols/quic/crypto.rs`
  - `/net/protocols/quic/congestion.rs`

#### 6. **HTTP/2** → `protocols/http2/` (à splitter)
- ❌ `/net/http2.rs` (850 lignes) → Diviser en:
  - `/net/protocols/http2/mod.rs`
  - `/net/protocols/http2/frame.rs`
  - `/net/protocols/http2/stream.rs`
  - `/net/protocols/http2/hpack.rs`

#### 7. **TLS** → `protocols/tls/` (à splitter)
- ❌ `/net/tls.rs` (900 lignes) → Diviser en:
  - `/net/protocols/tls/mod.rs`
  - `/net/protocols/tls/handshake.rs`
  - `/net/protocols/tls/record.rs`
  - `/net/protocols/tls/cipher.rs`

#### 8. **QoS** → `qos/` (à splitter)
- ❌ `/net/qos.rs` (800 lignes) → Diviser en:
  - `/net/qos/mod.rs`
  - `/net/qos/htb.rs`
  - `/net/qos/fq_codel.rs`
  - `/net/qos/prio.rs`

#### 9. **Load Balancer** → `loadbalancer/` (à splitter)
- ❌ `/net/loadbalancer.rs` (700 lignes) → Diviser en:
  - `/net/loadbalancer/mod.rs`
  - `/net/loadbalancer/round_robin.rs`
  - `/net/loadbalancer/least_conn.rs`
  - `/net/loadbalancer/health.rs`

#### 10. **RDMA** → `rdma/` (à splitter)
- ❌ `/net/rdma.rs` (1400 lignes) → Diviser en:
  - `/net/rdma/mod.rs`
  - `/net/rdma/verbs.rs`
  - `/net/rdma/queue.rs`
  - `/net/rdma/memory.rs`

#### 11. **Monitoring** → `monitoring/` (à splitter)
- ❌ `/net/monitoring.rs` (650 lignes) → Diviser en:
  - `/net/monitoring/mod.rs`
  - `/net/monitoring/metrics.rs`
  - `/net/monitoring/tracing.rs`

#### 12. **Routing** → `ip/` OU garder racine
- ❌ `/net/routing.rs` → ✅ `/net/protocols/ip/routing.rs` OU garder à la racine

### 🟡 DOUBLONS à résoudre

#### 1. **UDP** - DOUBLON
- ❌ `/net/udp.rs` (346 lignes) 
- ❌ `/net/udp/mod.rs` (350 lignes)
- ✅ **DÉJÀ RÉSOLU**: `/net/protocols/udp/` créé avec 3 fichiers

➡️ **ACTION**: Supprimer `/net/udp.rs` et `/net/udp/mod.rs` (remplacés)

#### 2. **Buffer** - DOUBLON?
- ❌ `/net/buffer.rs`
- ❌ `/net/core/buffer.rs`

➡️ **ACTION**: Analyser et garder le meilleur, supprimer l'autre

#### 3. **Socket** - DOUBLON?
- ❌ `/net/socket.rs`
- ❌ `/net/socket/mod.rs`

➡️ **ACTION**: Analyser et fusionner si nécessaire

### 🟢 FICHIERS OK (déjà bien placés)

- ✅ `/net/core/*` - Bien organisé
- ✅ `/net/protocols/tcp/*` - Bien organisé
- ✅ `/net/protocols/udp/*` - Bien organisé (nouveau)
- ✅ `/net/protocols/ip/*` - Bien organisé (nouveau)
- ✅ `/net/tcp/*` - OK (référencé par protocols/tcp)
- ✅ `/net/ip/*` - OK (référencé par protocols/ip)
- ✅ `/net/ethernet/*` - OK (mais besoin d'ajouter arp.rs)
- ✅ `/net/wireguard/*` - OK
- ✅ `/net/netfilter/*` - OK
- ✅ `/net/socket/*` - OK (epoll, poll)
- ✅ `/net/stack.rs` - OK à la racine (core du stack)
- ✅ `/net/mod.rs` - OK

---

## 🎯 ORDRE D'EXÉCUTION

### Phase 1: Déplacements simples (10 min)
1. Créer les répertoires manquants
2. Déplacer ARP → ethernet
3. Déplacer ICMP → ip
4. Créer services/ et déplacer DHCP/DNS

### Phase 2: Résolution des doublons (5 min)
5. Analyser et supprimer doublons UDP
6. Analyser et supprimer doublons buffer
7. Analyser et supprimer doublons socket

### Phase 3: Splits des gros fichiers (30 min)
8. Split QUIC en 5 fichiers
9. Split HTTP/2 en 4 fichiers
10. Split TLS en 4 fichiers
11. Split QoS en 4 fichiers
12. Split LoadBalancer en 4 fichiers
13. Split RDMA en 4 fichiers
14. Split Monitoring en 3 fichiers

### Phase 4: Mise à jour des imports (10 min)
15. Mettre à jour tous les `use` statements
16. Mettre à jour net/mod.rs
17. Compiler et vérifier

---

## 📊 RÉSULTAT ATTENDU

### Avant
```
/net/
├── *.rs (16 fichiers à la racine) ❌
├── core/
├── tcp/
├── udp/ (doublon)
├── ip/
├── ethernet/
└── protocols/ (nouveau, incomplet)
```

### Après
```
/net/
├── mod.rs
├── stack.rs (core)
├── core/ (9 fichiers)
├── protocols/
│   ├── tcp/ (13 fichiers)
│   ├── udp/ (3 fichiers)
│   ├── ip/ (11 fichiers) ← +icmp, +routing
│   ├── ethernet/ (4 fichiers) ← +arp, +bridge
│   ├── quic/ (5 fichiers)
│   ├── http2/ (4 fichiers)
│   └── tls/ (4 fichiers)
├── services/
│   ├── dhcp/ (2 fichiers)
│   └── dns/ (2 fichiers)
├── qos/ (4 fichiers)
├── loadbalancer/ (4 fichiers)
├── rdma/ (4 fichiers)
├── monitoring/ (3 fichiers)
├── firewall/ (netfilter)
├── vpn/ (wireguard)
├── socket/ (epoll, poll)
└── drivers/ (dans drivers/net/)
```

**Total**: ~80 fichiers bien organisés ✅

---

## 🚀 COMMANDES

```bash
# Phase 1: Créer répertoires
mkdir -p services/{dhcp,dns,ntp}
mkdir -p qos loadbalancer rdma monitoring

# Phase 2: Déplacer fichiers
mv arp.rs protocols/ethernet/
mv icmp.rs protocols/ip/
mv routing.rs protocols/ip/
mv dhcp.rs services/dhcp/client.rs
mv dns.rs services/dns/client.rs

# Phase 3: Supprimer doublons
rm udp.rs  # Remplacé par protocols/udp/

# Phase 4: Analyser avant suppression
diff buffer.rs core/buffer.rs
diff socket.rs socket/mod.rs
```

---

**PRIORITÉ**: Faire Phase 1 et 2 MAINTENANT avant tout développement !
