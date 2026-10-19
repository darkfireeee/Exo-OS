# 03 — PHASE 2 : Services Réseau & Cryptographie

## 1. État Réel dans le Dépôt
* **`crypto_server`** : Tel que révélé par `servers/phase5-tests/src/lib.rs`, `crypto_server` utilise encore une fonction `derive_key_stub` (hachage FNV/XOR jouet) et retourne `CRYPTO_ERR_ARGS` sur `CRYPTO_ENCRYPT` (2) et `CRYPTO_DECRYPT` (3)[cite: 18]. Or, la crate `drivers/storage/fscrypt` (`exo-fscrypt`) contient déjà une implémentation `#![no_std]` auditée de XChaCha20-BLAKE3 AEAD (`aead_seal`, `aead_open`) et d'Argon2id (`derive_kek`)[cite: 17].
* **`network_server` & `net_bridge.rs`** :
  * `servers/network_server` intègre déjà `smoltcp` (TCP, UDP, ICMP), un client DHCP complet (`src/dhcp.rs`), une table de routage LPM (`src/routing.rs`), un pool DMA de 256 pages RX + 256 pages TX (`src/buf_pool.rs`) et le support d'isolation ExoPhoenix (`src/isolation.rs`)[cite: 18].
  * **Goulot d'étranglement bloquant** : Dans `kernel/src/syscall/net_bridge.rs` et `servers/network_server/src/protocol.rs`, `NET_INLINE_DATA_MAX` est égal à `RAW_MSG_SIZE (200) - RAW_CALL_HEADER_SIZE (24) - sizeof::<NetMsg>() (48) = 128 octets`. Tout appel `sendto` ou `sendmsg` avec `len > 128` échoue avec `EMSGSIZE` (`-90`), et `recvfrom`/`recvmsg` tronque chaque lecture à 128 octets[cite: 7, 18].
  * Dans `servers/network_server/src/smoltcp_iface.rs`, `TCP_BUFFER_SIZE` et `UDP_BUFFER_SIZE` sont fixés à `2048` octets, ce qui est trop faible pour des chaînes de certificats TLS 1.3[cite: 18].

---

## 2. Procédure d'Implémentation Sans Ambiguïté

### A. Mise en Production de `servers/crypto_server` (`2.1`–`2.6`)
1. Ajouter en dépendances de `servers/crypto_server/Cargo.toml` : `exo-fscrypt = { path = "../../drivers/storage/fscrypt" }`, `blake3`, `sha2` (`force-soft`), `ed25519-dalek` et `hkdf`[cite: 7, 17].
2. Remplacer `derive_key_stub` et implémenter dans `handle_request` :
   * `CRYPTO_DERIVE_KEY (0)` : Dériver une clé de 32 octets via `exo_fscrypt::derive_kek` (Argon2id) ou `blake3::derive_key`, la stocker dans `Keystore.table[idx]` et retourner uniquement le `key_handle = idx + 1` en laissant `reply.data` à zéro (conformément à l'invariant `SRV-04` testé par `test_srv04_cle_opaque_pas_dans_reply`)[cite: 17, 18].
   * `CRYPTO_RANDOM (1)` : Appeler `SYS_GETRANDOM` (318) pour remplir `reply.data` depuis le TRNG matériel (`RDRAND`/`RDSEED`)[cite: 7, 11, 18].
   * `CRYPTO_ENCRYPT (2)` / `CRYPTO_DECRYPT (3)` : Chiffrer/déchiffrer via `exo_fscrypt::aead_seal` et `exo_fscrypt::aead_open` en utilisant la clé indexée par `key_handle`[cite: 17, 18].
   * `CRYPTO_HASH (4)` : Calculer `blake3::hash(&req.payload)`[cite: 7, 18].

### B. Suppression du Plafond de 128 Octets dans `net_bridge.rs` et `network_server` (`2.7`–`2.12`)
1. **Option 1 (Segmentation dans `kernel/src/syscall/net_bridge.rs`)** :
   * Dans `net_sendto()` et `net_sendmsg()`, remplacer `if len > NET_INLINE_DATA_MAX { return Err(EMSGSIZE); }` par une boucle découpant le buffer utilisateur en tranches de `NET_INLINE_DATA_MAX` (128 octets) pour les sockets `SOCK_STREAM` (TCP), ou passer par la copie inter-processus pour les datagrammes UDP > 128 octets[cite: 7].
2. **Option 2 (Copie inter-processus modèle `vfs_server`, recommandée pour TCP et UDP)** :
   * Dans `kernel/src/syscall/table/ipc/remote_memory.rs`, ajouter `process_name_eq(&name, b"network_server")` dans `caller_can_copy_target_memory()` (qui n'autorise actuellement que `PID 1`, `memory_server` et `vfs_server`)[cite: 7].
   * Dans `net_bridge.rs`, passer directement `buf_ptr` (adresse virtuelle utilisateur) et `len` dans `NetMsg.arg1` et `NetMsg.arg2` lorsque `len > NET_INLINE_DATA_MAX`[cite: 7, 18].
   * Dans `servers/network_server/src/main.rs` (`handle_sendto_data` et `handle_recvfrom_data`), utiliser `SYS_EXO_MEM_COPY_FROM_PID` (307) et `SYS_EXO_MEM_COPY_TO_PID` (308) par tranches de 4096 octets (comme dans `servers/vfs_server/src/main.rs`)[cite: 7, 18].
3. Dans `servers/network_server/src/smoltcp_iface.rs`, porter `TCP_BUFFER_SIZE` de `2048` à `16384` (16 Kio)[cite: 18].

### C. Résolveur DNS et Client TLS 1.3 (`2.9`, `2.13`, `2.14`)
1. Dans `servers/network_server/src/main.rs` (`tick`), lorsque `self.dhcp.ingest()` retourne un `DhcpLease` en phase `DhcpPhase::Bound`, appliquer dynamiquement `lease.ip`, `lease.prefix_len`, `lease.gateway` et enregistrer `lease.dns`[cite: 18].
2. Ajouter un module `servers/network_server/src/dns.rs` (ou dans `libs/exo-network`) résolvant les enregistrements `A`, `AAAA` et `CNAME` via UDP port 53 vers `lease.dns` (fallback `1.1.1.1` / `10.0.2.3` sous QEMU)[cite: 11, 18].
3. Dans `libs/exo-network`, intégrer `rustls` en mode `no_std` + `alloc` avec uniquement la version `TLS13` activée (TLS 1.2 désactivé, critère `2.14`)[cite: 10, 11].

---

## 3. Vigilance et Invariants pour ne pas Perturber le Noyau
* **Parité du Timeout IPC (`IPC_RECV_TIMEOUT_MS`)** : Comme documenté dans `servers/network_server/src/protocol.rs`, le flag `IPC_FLAG_TIMEOUT` (`0x0001`) partage le même mot de 64 bits que la valeur du timeout en millisecondes (`flags & !1`)[cite: 7, 18]. **Toujours utiliser une valeur paire pour `IPC_RECV_TIMEOUT_MS` (ex: `2`)** combinée avec `IPC_FLAG_TIMEOUT | IPC_RECV_TIMEOUT_MS`, sinon le bit 0 est masqué et un timeout de `1 ms` devient `0 ms` (expiration immédiate)[cite: 7, 18].
* **Invariant de libération des buffers TX (`smoltcp_iface.rs`)** : Dans `SmoltcpIface::poll_egress()`, ne **jamais** appeler `pool.tx_free()` immédiatement après `iface.poll_egress()`[cite: 18]. Les slots TX sont mis en file dans `ExoNetDevice.tx_ring` et ne doivent être libérés qu'à la réception du message IPC `NET_CTRL_TX_COMPLETE` (`0x4F06`) envoyé par `virtio_net_driver` ou `e1000_driver`[cite: 17, 18].
* **Invariant `SRV-04` (`crypto_server`)** : Une clé privée ou symétrique stockée dans `crypto_server` ne doit **jamais** être copiée dans `CryptoReply.data` lors d'un `CRYPTO_DERIVE_KEY`[cite: 11, 15, 18].