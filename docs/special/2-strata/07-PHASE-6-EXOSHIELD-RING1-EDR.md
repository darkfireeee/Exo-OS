# 07 — PHASE 6 : ExoShield Ring 1 — Intégration Complète EDR

## 1. État Réel dans le Dépôt
* Côté noyau (`kernel/`), le flux d'événements de sécurité `shield_feed` est déjà actif : `kernel/src/security/shield_feed.rs` enregistre les événements (`push_event` appelé notamment lors des violations IPC dans `channel.rs` et des tentatives de `SYS_REBOOT` non autorisées dans `system_misc.rs`), et le syscall `SYS_EXO_SHIELD_DRAIN` (325) dans `kernel/src/syscall/table/ipc/security_ops.rs` permet exclusivement au serveur de classe `ServiceClass::ExoShield` de drainer jusqu'à 64 `ShieldEvent` par appel[cite: 7].
* Côté routeur IPC (`servers/ipc_router/src/exocordon.rs`), `ServiceId::ExoShield = 10` est défini et le graphe `AUTHORIZED_GRAPH` comporte déjà les 51 arêtes canoniques synchronisées avec `kernel/src/security/ipc_policy.rs`[cite: 18].
* Côté `servers/exo_shield`, l'architecture modulaire (`behavioral`, `engine`, `forensics`, `hooks`, `ipc_gate`, `ml`, `network`, `sandbox`, `signatures`) est en place[cite: 5, 10, 11].
* **Écarts restants** :
  1. Le chargement initial de `/etc/exoshield/signatures.ydb` depuis ExoFS au boot et le scan initial des binaires Ring 1 (`PID 1..16`) avant d'émettre `SHIELD_READY` vers `init_server` (`6.3`, `6.15`)[cite: 8, 10, 11].
  2. Le déclenchement des alertes sonores vers `audio_server` sur menace `HIGH` (3 bips courts) et `CRITICAL` (1 ton long) (`6.17`, `6.18`).
  3. L'application automatique des profils de sandbox (`syscall_filter`, `fs_restriction`, `net_isolation`) lors du lancement d'une application via `exo compat run` (`6.6`–`6.9`, `6.16`).

---

## 2. Procédure d'Implémentation Sans Ambiguïté

### A. Boucle de Drain `SYS_EXO_SHIELD_DRAIN` et Chargement YARA au Boot (`6.1`–`6.5`, `6.15`)
1. Dans `servers/exo_shield/src/main.rs`, s'enregistrer en premier sous le nom `b"exo_shield"` via `SYS_IPC_REGISTER` (304) afin que `kernel/src/syscall/table/ipc/endpoints.rs` (`service_class_for_endpoint_name`) attribue `ServiceClass::ExoShield` au PID du serveur[cite: 7, 18].
2. Ouvrir et lire `/etc/exoshield/signatures.ydb` via `SYS_EXOFS_OPEN_BY_PATH` (519) et `SYS_READ` (0), peupler `signatures/database.rs`, puis scanner la liste des processus actifs retournée par `SYS_EXO_PROCESS_LIST` (351) pour vérifier l'intégrité des serveurs Ring 1 (`PID 1..16`)[cite: 7, 8, 10, 18].
3. Si 0 menace n'est détectée (`6.15`), envoyer le signal IPC `SHIELD_READY` à `init_server` (PID 1), puis entrer dans la boucle principale drainant les événements kernel via `syscall2(SYS_EXO_SHIELD_DRAIN, events_buf.as_mut_ptr() as u64, 64)`[cite: 7, 8, 10, 18].
4. Persister chaque violation dans l'audit ring buffer (4096 entrées circulaires, `6.5`) et dans `ExoLedger`[cite: 10, 11].

### B. Sandbox Automatique pour `exo compat` (`6.6`–`6.9`, `6.16`)
1. Dans `servers/exo_shield/src/sandbox/`, associer chaque manifeste de paquet installé dans `/etc/exoshield/` à un masque de promesses `SYS_EXO_PLEDGE` (324) et d'accès FS/réseau[cite: 7, 8, 10, 11].
2. Lorsqu'un processus est lancé via `exo compat run`, vérifier que `sys_exo_pledge(promises)` est appliqué avant `execve` : toute tentative d'appel système hors manifeste déclenche un événement `SYSCALL` dans `shield_feed` et bloque l'évasion (`security_test::exoshield_sandbox_escape_blocked`, `6.16`)[cite: 7, 10, 11].

### C. Alertes Sonores vers `audio_server` et Callbacks `PhoenixSafe` (`6.13`, `6.14`, `6.17`, `6.18`)
1. Sur détection d'une menace dans `engine/core` :
   * **Niveau `HIGH` (`6.17`)** : Envoyer 3 requêtes IPC non-bloquantes `AudioMsgType::Beep { freq_hz: 300, duration_ms: 150 }` espacées de 50 ms vers `audio_server` (avec fallback silencieux si `audio_server` est absent)[cite: 10, 11, 12, 14].
   * **Niveau `CRITICAL` (`6.18`)** : Envoyer `AudioMsgType::PlaySystemSound(SoundId::SecurityAlert)` (ton saturé 200 Hz de 1000 ms) vers `audio_server`[cite: 10, 11, 12, 14].
2. Implémenter `PhoenixSafe` (`on_pre_switch` / `on_post_switch`, `6.13` et `6.14`) pour flusher le buffer d'alertes vers ExoFS avant une bascule ExoPhoenix et réabonner les hooks après la bascule sans perdre d'événements[cite: 10, 11].

---

## 3. Vigilance et Invariants pour ne pas Perturber le Noyau
* **Ordre d'enregistrement de `ServiceClass::ExoShield`** : Dans `kernel/src/syscall/table/ipc/security_ops.rs`, `sys_exo_shield_drain` vérifie `service_class_of(Pid(caller_pid)) == ServiceClass::ExoShield` et retourne `EACCES` sinon[cite: 7]. Or, cette classe est attribuée par le noyau uniquement lorsque le processus appelle `SYS_EXO_IPC_CREATE` (304) avec le nom exact `b"exo_shield"` (`endpoints.rs`)[cite: 7]. **`exo_shield` doit donc impérativement appeler `SYS_IPC_REGISTER` (`b"exo_shield"`) AVANT son premier appel à `SYS_EXO_SHIELD_DRAIN` (325)**[cite: 7, 18].
* **Zéro blocage dans les hooks kernel (`shield_feed`)** : Les hooks du noyau (`push_event` dans `channel.rs`, `system_misc.rs`, etc.) s'exécutent sur le chemin chaud des syscalls[cite: 7]. Ils ne doivent jamais attendre une réponse synchrone d'`exo_shield` : la communication reste strictement asynchrone via l'anneau `shield_feed` drainé par `SYS_EXO_SHIELD_DRAIN`[cite: 7, 11].
* **Invariant de démarrage Vague 5 (`BOOT_SEQUENCE_STRATA.md`)** : `exo_shield` doit toujours être démarré en **dernier serveur Ring 1** (Vague 5, juste avant `exosh` en Vague 6) afin de pouvoir inspecter tous les serveurs `PID 1..16` déjà actifs[cite: 8, 9, 10, 11]. Si le scan initial détecte une menace sur un serveur Ring 1, le comportement exigé par `BOOT_SEQUENCE_STRATA.md` est un arrêt immédiat avec dump forensique[cite: 8].