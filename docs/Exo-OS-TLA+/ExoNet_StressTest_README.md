# ExoNet_StressTest — Guide complet

**ExoOS Network Module v4 — Modèle TLA+ haute contrainte**  
**Mai 2026 · Claude Sonnet 4.6 (claude-beta)**

---

## Ce que ce modèle prouve (au-delà d'ExoNet_IPC et HardMode)

| Propriété | ExoNet_IPC | ExoNet_HardMode | **ExoNet_Stress** |
|---|---|---|---|
| Safety_NoDoubleRxUse | ✅ BFS | 🟡 sim | ✅ BFS |
| Safety_NoRxBufferLeak | ❌ | 🟡 sim | ✅ BFS |
| Safety_NoDoubleTxUse | ❌ | ❌ | ✅ BFS |
| Safety_NoTxBufferLeak | ❌ | ❌ | ✅ BFS |
| Safety_NoTrafficBeforeBoot | ❌ | ❌ | ✅ BFS |
| Safety_SocketBound | ❌ | ❌ | ✅ BFS |
| Phoenix_DrainCompleteness | ❌ | ❌ | ✅ BFS |
| Liveness_RxRefill | ❌ | ❌ | ✅ LTL+WF |
| Liveness_TxCompletion | ❌ | ❌ | ✅ LTL+WF |
| Liveness_PhoenixCompletes | ❌ | ❌ | ✅ LTL+WF |
| Liveness_TxPoolRecovery | ❌ | ❌ | ✅ LTL+WF |
| DDoS drop path | ❌ | 🟡 sim | ✅ BFS |
| TX saturation (NS_TxSaturated) | ❌ | ❌ | ✅ BFS |
| Multi-burst (MAX_BURST paquets) | ❌ | ❌ | ✅ BFS |

---

## Nouvelles actions modélisées

### Boot handshake
`NS_SendDriverInit` → `VN_AckDriverInit` : séquence obligatoire avant tout trafic.  
`Safety_NoTrafficBeforeBoot` prouve qu'aucun paquet ne peut entrer dans `ipc_rx_ring` avant que `boot_phase = "ready"`.

### TX path complet
`NS_SendPacket` : `tx_alloc` + écriture smoltcp + push `ipc_pkt_ring`.  
`VN_FlushTx` : envoi NIC + `tx_free` → retour dans `tx_pool_free`.  
`Safety_NoDoubleTxUse` : un slot TX ne peut pas être simultanément libre ET en vol.  
`Safety_NoTxBufferLeak` : tout slot TX est localisable à tout instant.

### TX saturation
`NS_TxSaturated` : modélise `receive()` retournant `None` quand `tx_alloc()` échoue.  
Le slot RX est immédiatement remis dans `ns_released_buf` (pas de corruption).  
La propriété `Liveness_TxPoolRecovery` prouve que le pool TX se libère toujours.

### Multi-burst RX
`VN_HandleIRQ_Burst(burst)` : N paquets (`1 ≤ N ≤ MAX_BURST`) traités en un tick.  
Modélise la boucle `while pop_used()` de `handle_rx_irq()`.  
TLC explore toutes les tailles de burst possibles et toutes les combinaisons de slots.

### Socket lifecycle
`NS_SocketAlloc` / `NS_SocketClose` : alloc et libération de handles TCP.  
`Safety_SocketBound` : `|socket_slots| ≤ MAX_SOCKETS` toujours vrai.

### Phoenix isolation complète
Cycle `normal → draining → serialized → normal` avec 5 actions.  
`Phoenix_DrainCompleteness` : quand `phoenix_phase = "serialized"`, `ipc_rx_ring` est vide.  
`Liveness_PhoenixCompletes` : le cycle s'achève toujours (sous fairness).  
`Safety_PhoenixNoNewRx` : aucun nouveau paquet accepté pendant drain/serialize.

### Propriétés de vivacité (LTL + WF)
`Spec = Init ∧ [][Next]_vars ∧ Fairness`  
Fairness = `WF_vars` sur toutes les actions de consommation/libération.  

| Propriété | Signification |
|---|---|
| `Liveness_RxRefill` | Tout slot RX libéré (released_buf) est éventuellement refillé dans le vring |
| `Liveness_TxCompletion` | Tout slot TX en vol est éventuellement libéré |
| `Liveness_PhoenixCompletes` | Le drain Phoenix se termine toujours |
| `Liveness_TxPoolRecovery` | Pool TX épuisé → éventuellement non-vide |
| `Liveness_RxDelivery` | Ring RX non-vide → éventuellement consommé |

---

## Comment exécuter

### Profil 1 — BFS exhaustif (preuve formelle)

```bash
java -Xmx4g -XX:+UseParallelGC -cp tla2tools.jar tlc2.TLC \
  -workers auto \
  -config ExoNet_StressTest.cfg \
  ExoNet_StressTest
```

Constantes : `POOL_SIZE=6, RING_SIZE=3, MAX_SOCKETS=2, MAX_BURST=2, BATCH_SIZE=2`  
Estimation : ~500K–2M états, 30–120s, 8 cœurs.

### Profil 2 — Simulation grande échelle (confiance sous paramètres réels)

```bash
java -Xmx4g -XX:+UseParallelGC -cp tla2tools.jar tlc2.TLC \
  -simulate -depth 500 -workers auto \
  -config ExoNet_StressTest_Sim.cfg \
  ExoNet_StressTest
```

Constantes : `POOL_SIZE=32, RING_SIZE=16, MAX_SOCKETS=8, MAX_BURST=8, BATCH_SIZE=4`  
Objectif : 50M états simulés sans violation.

---

## Relation avec le code V4

| Action TLA | Code V4 correspondant |
|---|---|
| `NS_SendDriverInit` | `driver_link::connect_virtio_net()` → envoi `DriverInitMsg` |
| `VN_AckDriverInit` | `virtio_net::main` → réception `NET_CTRL_DRIVER_INIT` |
| `VN_HandleIRQ_Burst(burst)` | `VirtioNet::handle_rx_irq()` boucle `pop_used()` |
| `VN_HandleIRQ_Drop(idx)` | guard `Len >= RING_SIZE` → drop DDoS |
| `NS_PollOne` | `SmoltcpIface::poll_one()` → `ExoRxToken::consume()` |
| `NS_FlushReleased` | `driver_link::flush_released()` → `RxReleaseMsg` IPC |
| `VN_ProcessReleases` | `VirtioNet::process_rx_releases()` → refill vring |
| `NS_SendPacket` | `ExoNetDevice::transmit()` → `ExoTxToken::consume()` |
| `NS_TxSaturated(idx)` | `tx_alloc()` retourne `None` → `receive()` retourne `None` |
| `VN_FlushTx` | `VirtioNet::flush_tx()` → `tx_free()` |
| `NS_SocketAlloc` | `SmoltcpIface::alloc_tcp_socket()` |
| `NS_SocketClose` | `dispatch(NET_MSG_CLOSE)` |
| `Phoenix_StartDrain` | Réception `PrepareIsolation` depuis exo_shield |
| `Phoenix_DrainOne` | `SmoltcpIface::drain_all()` — un paquet |
| `Phoenix_Serialize` | `TcpStateStore::serialize_into()` |
| `Phoenix_Restore` | Kernel B restaure, network_server redémarre |

---

## Abstractions et limites

**Ce que le modèle abstrait correctement :**
- `ipc_rx_ring` comme `Sequence` : abstrait l'encodage binaire `[pool_idx:u16, len:u16]` du `SpscRing`
- `ipc_tx_ring` comme `Sequence of Set` : abstrait le batchage par `RxReleaseMsg`
- Actions atomiques : chaque action TLA correspond à une section critique du code
- `burst` comme sous-ensemble : TLC explore toutes les combinaisons de slots possibles

**Ce que le modèle ne prouve pas :**
- Latence (pas de notion de temps dans ce modèle)
- Throughput (pas de compteurs de paquets traités)
- Comportement TLS (délégué à `crypto_server`, hors scope)
- Sessions UDP persistantes à travers Phoenix (Phase 2)
- `ipc_broker::get_spsc_ring()` (non implémenté, problème identifié dans l'audit V4)

---

*ExoOS Network Module v4 — Stress Test TLA+ · Mai 2026*  
*Claude Sonnet 4.6 (claude-beta)*
