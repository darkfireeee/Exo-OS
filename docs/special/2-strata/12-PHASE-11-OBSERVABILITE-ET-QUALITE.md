## Fichier 12 : `12-PHASE-11-OBSERVABILITE-ET-QUALITE.md`

```markdown
# 12 — PHASE 11 : Observabilité (`monitor_server`) & Qualité Système

## 1. État Réel dans le Dépôt
* Côté noyau, les compteurs de télémétrie sont déjà implémentés et exposés sans verrou via le syscall `SYS_EXO_PERF_READ = 330` (`kernel/src/syscall/table/ipc/system_ops.rs`) :
  * `EXO_PERF_SYSCALL_COUNT (1)` : Compteur d'appels par numéro de syscall (`0..550`)[cite: 7, 18].
  * `EXO_PERF_IPC_MESSAGES_SENT (2)`, `RECEIVED (3)`, `DROPPED (4)`, `RPC_CALLS (5)`, `RPC_TIMEOUTS (6)`, `FUTEX_WAITS (7)`, `FUTEX_WAKES (8)`[cite: 7, 18].
  * `EXO_PERF_DISPATCH_TOTAL (9)`, `FAST_PATH (10)`, `SLOW_PATH (11)`, `ENOSYS (12)`[cite: 7, 18].
* De plus, `SYS_SYSINFO = 99` (`kernel/src/syscall/handlers/misc.rs`) expose déjà `uptime`, `totalram`, `freeram` et `procs`, et `SYS_EXO_PROCESS_LIST = 351` (`system_ops.rs`) exporte la table complète des processus (`ExoProcessInfo` : `pid`, `ppid`, `state`, `threads`, `name`, `utime_ns`, `stime_ns`)[cite: 7, 18].
* La bibliothèque `libs/exo-observability` est déclarée dans `libs/Cargo.toml`[cite: 19].
* **Écarts restants** :
  1. Le serveur Ring 1 `servers/monitor_server/` n'existe pas encore dans `servers/` (`11.1`)[cite: 5, 10, 11, 18].
  2. Les sous-commandes utilisateur `exo log --filter <tag>` (`11.2`), `exo metrics` (`11.3`) et `exo audit --verify-chain` (`11.10`) ne sont pas encore câblées dans `exosh`[cite: 10, 11].

---

## 2. Procédure d'Implémentation Sans Ambiguïté

### A. Création de `servers/monitor_server` (`11.1`)
1. Créer `servers/monitor_server/Cargo.toml` et `src/main.rs` (`#![no_std]`, `#![no_main]`)[cite: 10, 11].
2. Enregistrer l'endpoint `MONITOR_SERVER_ENDPOINT` (`b"monitor_server"`) auprès d'`ipc_router` et ajouter la classe de service correspondante dans `kernel/src/security/ipc_policy.rs`, `kernel/src/syscall/table/ipc/endpoints.rs` et `servers/ipc_router/src/exocordon.rs`[cite: 7, 18].
3. Implémenter 3 opérations IPC dans `monitor_server` :
   * `MONITOR_MSG_LOG_APPEND (0)` : Reçoit une entrée de log structurée (`sender_pid`, `timestamp_ns`, `level`, `tag`, `message`), la conserve dans un anneau en mémoire et la persiste par lots dans `/var/log/system.log` sur ExoFS (`11.1`)[cite: 10, 11, 13].
   * `MONITOR_MSG_LOG_QUERY (1)` : Filtre les entrées par tag (ex: `shield`, `kernel`, `net`, `usb`) pour servir `exo log --filter shield` (`11.2`)[cite: 10, 11].
   * `MONITOR_MSG_METRICS_SNAPSHOT (2)` : Agrège en une seule structure `SystemMetricsWire` :
     * Les compteurs kernel lus via `SYS_EXO_PERF_READ` (330) : total syscalls, ratio fast-path/slow-path, messages IPC envoyés/reçus/droppés (`11.9`)[cite: 7, 10, 18].
     * L'état mémoire lu via `SYS_SYSINFO` (99) (`totalram`, `freeram`) et `memory_server` (`MEMORY_MSG_QUOTA_QUERY = 9`) (`11.8`)[cite: 7, 10, 18].
     * Le nombre de processus et temps CPU lus via `SYS_EXO_PROCESS_LIST` (351)[cite: 7, 18].

### B. Implémentation de `exo log`, `exo metrics` et `exo audit --verify-chain` (`11.2`, `11.3`, `11.10`)
1. Dans `servers/exosh`, câbler :
   * `exo log [--filter <tag>]` $\to$ interroge `monitor_server` (`MONITOR_MSG_LOG_QUERY`) et affiche les journaux horodatés (`11.2`)[cite: 10, 11].
   * `exo metrics` $\to$ interroge `monitor_server` (`MONITOR_MSG_METRICS_SNAPSHOT`) et affiche l'utilisation CPU, les pages mémoire libres/utilisées, les statistiques IPC (0 drop) et les statistiques réseau (`11.3`)[cite: 10, 11].
   * `exo audit [--verify-chain]` $\to$ lit le journal scellé d'`ExoLedger` et recalcule chaque maillon de la chaîne `BLAKE3(prev_hash || entry_bytes)` depuis l'entrée 0 jusqu'à la tête courante, affichant `0 rupture de chaîne` (`1.21`, `11.10`)[cite: 10, 11].

---

## 3. Vigilance et Invariants pour ne pas Perturber le Noyau
* **Allocation sur pile dans `sys_exo_log` (350)** : Dans `kernel/src/syscall/table/ipc/system_ops.rs`, `sys_exo_log` alloue `let mut kbuf = [0u8; 4096];` sur la pile kernel pour demeurer `NO-ALLOC`[cite: 7]. Les serveurs Ring 1 doivent envoyer des messages de log concis (`<= 512` octets) et utiliser `monitor_server` pour l'archivage disque afin de ne pas saturer le port de debug `0xE9` / console série[cite: 7, 18].
* **Vérification de `EXO_PERF_IPC_MESSAGES_DROPPED` (`11.9`)** : Le critère `11.9` exige que `syscall2(SYS_EXO_PERF_READ, EXO_PERF_IPC_MESSAGES_DROPPED, 0)` reste égal à `0` après 1 heure de stress IPC[cite: 7, 10, 11, 18].