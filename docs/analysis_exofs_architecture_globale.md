# Analyse ciblée de `docs/ExoOS_Architecture_v6.md` pour la construction d'ExoFS

## Portée

Cette note extrait **uniquement** ce que l'architecture globale impose ou influence pour **ExoFS**, sans dériver vers les autres sous-systèmes sauf lorsqu'ils conditionnent directement son design ou ses interfaces.

Axes couverts :
- place d'ExoFS dans l'architecture ExoOS ;
- interactions kernel / drivers / serveurs / userspace ;
- contraintes `no_std` / ring0 ;
- persistance et abstraction block device ;
- sécurité / capabilities ;
- IPC et médiation ;
- boot / recovery / epochs ;
- performance / observabilité ;
- checklist finale des interfaces ExoFS requises par l'architecture.

---

## 1. Place d'ExoFS dans l'architecture globale

## 1.1 ExoFS est explicitement un composant Ring 0

Le document est très clair dès l'introduction :

- **Ring 0** contient : primitives matérielles, IPC **et ExoFS** ;
- **Ring 1** contient les services système `no_std` ;
- **Ring 3** contient les applications avec un POSIX partiel.

Conséquence immédiate :

- ExoFS n'est **pas** un serveur userspace ;
- ExoFS est un **sous-système noyau** au même niveau architectural que `memory/`, `scheduler/`, `ipc/`, `security/` ;
- le choix est motivé par la **performance**.

Donc, pour la construction de `kernel/src/fs/exofs/`, il faut raisonner en termes de :
- API internes noyau ;
- contraintes ring0 ;
- couplage minimal mais réel avec `memory`, `scheduler`, `security`, `syscall`.

## 1.2 Dans la hiérarchie des couches, `fs/` dépend de `memory + scheduler + security`

Le tableau §2.1 impose :

- couche `fs/` dépend de :
  - `memory/`
  - `scheduler/`
  - `security/`
- `fs/` est appelé par userspace via syscalls `0-519`.

Implications très concrètes pour ExoFS :

1. ExoFS **peut utiliser** des services de mémoire/scheduling/sécurité du noyau ;
2. ExoFS **ne doit pas dépendre** d'IPC pour ses chemins critiques internes ;
3. ExoFS est une frontière système appelée via syscalls, donc ses erreurs et ses contrats doivent être stables.

## 1.3 ExoFS est le socle persistant du système, pas seulement un backend de fichiers

Plusieurs parties du document montrent qu'ExoFS stocke ou supporte :

- les binaires ELF enregistrés via `PHX-03` ;
- la persistance du `ipc_broker` ;
- les états isolés/flushés de `init_server`, `vfs_server`, `memory_server`, `crypto_server` ;
- les objets exécutables via `exec(object_id)` ;
- la base de persistance utilisée lors de l'isolation ExoPhoenix.

Donc architectoniquement ExoFS sert de :
- **stockage persistant système** ;
- **référentiel d'objets noyau/serveurs** ;
- **base de reprise après crash/gel**.

Cela veut dire que son rôle dépasse un simple FS POSIX : il doit offrir des primitives adaptées à des **objets identifiés durablement**, à des flushs cohérents, et à la reprise.

---

## 2. Interactions ExoFS avec kernel, drivers, serveurs et userspace

## 2.1 Interaction ExoFS ↔ kernel

ExoFS vit dans le noyau et s'insère dans plusieurs séquences critiques :

- boot : étape **17** = `Monter ExoFS + boot_recovery_sequence` ;
- sécurité : les accès ExoFS doivent passer par la vérification de capabilities ;
- process : `exec()` manipule des `ObjectId` ExoFS ;
- syscall layer : `fs/exofs/syscall/` expose les appels 500–518.

Conséquences :
- ExoFS doit exposer un **point d'entrée de montage** utilisable tôt au boot ;
- ExoFS doit intégrer une **séquence de recovery au boot** ;
- ExoFS doit avoir des APIs internes consommables par la couche syscall ;
- ExoFS doit coopérer avec `security/verify()` et `check_access()`.

## 2.2 Interaction ExoFS ↔ drivers de stockage

Le document montre deux choses importantes :

1. côté kernel :
   - `fs/exofs/storage/virtio_adapter.rs` existe comme adaptateur de stockage ;
2. côté Ring 1 :
   - `drivers/virtio-block/`
   - `exofs_backend.rs` : « enregistrement comme backend ExoFS ».

Cela révèle une architecture mixte :

- ExoFS est **kernel-side** ;
- le backend bloc concret peut venir d'un driver virtio block séparé ;
- il faut un **contrat d'enregistrement / abstraction backend** entre ExoFS et la source de blocs.

Le document mentionne aussi que `virtio_adapter.rs` est `#[cfg(qemu_only)]`, ce qui suggère :
- un backend temporaire/bootstrapping pour QEMU ;
- une séparation souhaitée entre logique FS et implémentation matérielle.

Conclusion :
ExoFS doit être construit autour d'une **abstraction de block device**, non autour de VirtIO directement.

## 2.3 Interaction ExoFS ↔ services Ring 1

ExoFS n'est pas exposé brut aux applications ; plusieurs serveurs Ring 1 reposent dessus :

- `vfs_server` :
  - dépend de « ExoFS kernel monté » ;
  - fait `PathBuf -> ObjectId` en wrapant `SYS_EXOFS_PATH_RESOLVE` ;
  - maintient FD table et mount namespace ;
- `ipc_broker` :
  - `persistence.rs` : dump registry → ExoFS (`ObjectId`) ;
- `init_server`, `memory_server`, `crypto_server`, `vfs_server` :
  - lors d'isolation, flushent leur état vers ExoFS et renvoient un `PrepareIsolationAck`.

Cela impose que côté ExoFS il existe ou doive exister :
- des syscalls/path APIs pour résolution de chemins ou noms ;
- des opérations de lecture/écriture d'objets ;
- des opérations de flush/sync suffisamment fortes pour l'isolation et le crash recovery ;
- une stabilité des `ObjectId` sur laquelle les services peuvent s'appuyer.

## 2.4 Interaction ExoFS ↔ userspace / POSIX partiel

Le userspace Ring 3 passe :
- soit par `vfs_server`,
- soit par la libc/syscalls ExoOS.

La librairie `libs/exo-syscall/src/exofs.rs` wrappe les syscalls `500-518`, `519` étant réservé.

Donc ExoFS doit fournir :
- une interface syscall de bas niveau et stable ;
- des primitives exploitables par la translation layer/VFS ;
- des erreurs convertibles vers une sémantique partiellement POSIX.

Mais le document d'architecture précise aussi un identifiant natif central :
- `ObjectId([u8;32])` opaque,
- avec contrainte stricte de validité (`bytes[8..32] == 0`),
- utilisé comme **ID global unique ExoFS**.

Donc l'architecture globale indique que la ressource native d'ExoFS est l'**objet persistant adressé par ObjectId**, pas le chemin POSIX.

---

## 3. Contraintes `no_std` et ring0 applicables à ExoFS

## 3.1 ExoFS est en environnement noyau, donc pas de runtime userspace

Le document ne le dit pas explicitement sous la forme « fs est no_std », mais tout le contexte Ring 0 l'impose :
- Rust noyau ;
- dépendances contrôlées ;
- environnements critiques ;
- pas de confort userspace.

Le fait que les crates Ring 1 soient explicitement `#![no_std]` renforce encore que le noyau l'est aussi par nature.

Donc ExoFS doit être :
- compatible environnement noyau ;
- sans dépendances userspace supposant OS hôte, allocateur standard, threads standard, etc.

## 3.2 Toute corruption ou bug ExoFS en ring0 est critique

Parce qu'ExoFS réside dans le TCB noyau :
- un parseur on-disk fragile peut compromettre le noyau ;
- une corruption d'offset/bloc peut mener à corruption mémoire ou deadlock ;
- des appels bloquants mal placés peuvent geler le système.

Le document insiste justement sur :
- vérification de capabilities ;
- séquence de boot ordonnée ;
- contraintes de lock ordering ;
- séparation claire des responsabilités.

Pour ExoFS cela impose un style d'implémentation défensif :
- validation stricte des métadonnées on-disk ;
- refus des `ObjectId` invalides avant logique profonde ;
- bornage des lectures/écritures/offsets ;
- distinction claire entre erreur d'IO, corruption, violation de droits.

## 3.3 Contraintes de locking : FS est dernier niveau et doit relâcher ses locks avant IPC

Le tableau §2.2 est capital pour ExoFS :

- niveau 5 = `FS`
- règle : **FS doit relâcher ses locks avant tout appel IPC** car l'IPC est bloquant.

C'est une contrainte architecturale forte.

Implications directes :
- ExoFS ne doit pas faire d'IPC tout en tenant des spinlocks internes ;
- si ExoFS dépend d'un backend ou d'un service accessible par IPC, l'architecture interne doit découper :
  - préparation sous lock,
  - relâchement,
  - appel externe,
  - reprise/validation.

Pour la construction d'ExoFS, cela milite pour :
- backend bloc accessible idéalement sans IPC sur chemin critique, ou via frontière très contrôlée ;
- code interne structuré pour éviter l'inversion de priorité ;
- aucune attente bloquante sous verrou de métadonnées ExoFS.

---

## 4. Persistance et abstraction de block device

## 4.1 Le boot impose un montage et une recovery ExoFS précoces

Étape 17 du boot :
- **Monter ExoFS + `boot_recovery_sequence`**

Donc ExoFS doit fournir au minimum :
- détection du support ;
- chargement de sa structure persistante ;
- recovery après arrêt non propre ;
- état monté exploitable avant démarrage complet des services Ring 1.

Comme `vfs_server` dépend d'« ExoFS kernel monté », ExoFS est une dépendance de démarrage structurante.

## 4.2 ExoFS doit reposer sur une abstraction backend bloc

Indices explicites :
- `fs/exofs/storage/virtio_adapter.rs`
- `drivers/virtio-block/exofs_backend.rs`
- priorité élevée accordée au driver `virtio-block`.

Lecture architecturale :
- ExoFS ne doit pas mélanger format persistant et code de pilote matériel ;
- il lui faut un **backend de blocs interchangeable** ;
- le système doit pouvoir brancher au moins :
  - un backend VirtIO,
  - éventuellement un adaptateur QEMU-only,
  - potentiellement un mock/RAM backend de test.

L'architecture n'impose pas le trait exact, mais impose très fortement les capacités suivantes :
- lecture de blocs/secteurs ;
- écriture ;
- flush / garantie de persistance ;
- découverte capacité/taille ;
- erreurs d'IO distinguées.

## 4.3 Le device bloc concret est 512B côté VirtIO block

Le document précise pour `drivers/virtio-block/block.rs` :
- `VIRTIO_BLK_T_IN/OUT`
- lecture/écriture de **secteurs 512B**.

Même si ExoFS peut avoir une granularité logique supérieure, il doit architectoniquement :
- tolérer un backend à 512B ;
- gérer alignements, agrégations ou write amplification lui-même ;
- ne pas supposer une granularité plus large offerte par le matériel.

## 4.4 Persistance système : ExoFS est utilisé comme dépôt de vérité durable

Exemples imposés par l'architecture :
- enregistrement des hashes d'ELF (`PHX-03`) ;
- persistence registry `ipc_broker` ;
- flush des tables `mount_table`, `fd_table`, `region_table`, états chiffrés, etc. ;
- lecture par `exec(object_id)`.

Donc ExoFS doit garantir :
- stabilité durable des objets ;
- cohérence après reboot ;
- support des écritures de métadonnées système ;
- possibilité de relecture par ID global.

---

## 5. Sécurité, capabilities et contrôle d'accès

## 5.1 ExoFS est explicitement branché sur le sous-système `security`

Dans la hiérarchie des couches, `fs/` dépend de `security/`.

Le document précise :
- `security/capability/verify.rs` : `verify()` = unique point décision ;
- `security/access_control/check.rs` : `check_access()` wrappe `verify()` ;
- `ipc/check_access()` appelle `verify()` en interne ;
- **S-01** : `verify_cap()` avant tout accès ExoFS dans `fs/exofs/syscall/*`.

C'est une exigence directe, pas une simple recommandation.

Conséquences :
- toute syscall ExoFS doit commencer par une vérification de capability/droits ;
- ExoFS ne peut pas supposer que la sécurité est entièrement gérée ailleurs ;
- l'intégration avec `security::verify()` est un contrat architectural fort.

## 5.2 Vérification préalable de `ObjectId::is_valid()`

Le document définit précisément `ObjectId` :
- opaque `[u8;32]`,
- valide seulement si `bytes[8..32] == 0`.

Et la checklist impose :
- **S-33** : `ObjectId::is_valid()` vérifié avant `verify()`.

Donc pour ExoFS :
- toute entrée publique prenant un `ObjectId` doit d'abord valider son format ;
- cette validation est structurelle et doit être très bon marché ;
- les API noyau et syscall doivent préserver cet ordre.

## 5.3 Capabilities : ExoFS doit prendre en compte des droits fins

Le document ne liste pas ici tous les droits, mais mentionne :
- `Rights bitflags 14 droits`
- `CapToken { gen, oid, rights }`
- révocation instantanée
- `verify()` O(1) constant-time à implémenter.

Implications pour ExoFS :
- ses opérations doivent être séparables par droits :
  - lecture,
  - écriture,
  - mutation métadonnées,
  - exécution,
  - administration,
  - potentiellement hash/secret/quotas.
- les syscalls ExoFS doivent pouvoir transporter ou résoudre les droits nécessaires.

## 5.4 Secrets et pipeline crypto côté ExoFS kernel

Le document attribue explicitement à `fs/exofs/crypto/` la crypto Ring 0, avec exigences précises :

- `RustCrypto no_std Ring 0 (Cargo.toml ✅)`
- **S-05** : Blake3 avant compression
- **S-06** : nonce = `NONCE_COUNTER` + HKDF(object_id)
- **S-07** : pipeline données → Blake3 → LZ4 → XChaCha20 → disque
- **S-09** : `ObjectKind::Secret` : `BlobId` jamais retourné
- **S-16** : `key_storage` Argon2id

Architecturalement cela signifie :
- ExoFS n'est pas un FS neutre vis-à-vis de la sécurité ; il porte des objets secrets ;
- certains traitements cryptographiques doivent être **dans ExoFS kernel-side**, pas déportés à Ring 1 ;
- il existe une distinction d'objet (`ObjectKind::Secret`) qui influence les interfaces exposées.

Pour la construction d'ExoFS, cela impose des hooks ou modules pour :
- hashing de contenu ;
- chiffrement des secrets ;
- interdiction de certaines opérations d'inspection sur objets secrets ;
- audit de certaines opérations sensibles.

## 5.5 Audit obligatoire des opérations sensibles

La structure inclut :
- `audit/ring_buffer.rs   AUDIT-RING-SEC lock-free`

Checklist :
- **S-14** : toutes opérations loggées ;
- **S-15** : `GET_CONTENT_HASH` toujours auditée.

Donc ExoFS doit intégrer nativement une capacité d'audit/traçage sécurité, pas juste des logs opportunistes.

---

## 6. IPC éventuel et médiation

## 6.1 ExoFS est appelé par syscall, mais coopère avec des services IPC au-dessus

L'architecture montre deux chemins :
- accès direct via syscalls ExoFS 500–518 ;
- accès indirect via `vfs_server` et autres services Ring 1 via IPC.

Conclusion :
- ExoFS doit être **IPC-agnostique** en interne ;
- mais ses interfaces doivent être suffisamment stables pour servir de fondation à des services IPC.

## 6.2 `vfs_server` dépend explicitement de `SYS_EXOFS_PATH_RESOLVE`

C'est un point architectural majeur :
- `path_resolver.rs` : `PathBuf -> ObjectId` wrappe `SYS_EXOFS_PATH_RESOLVE`.

Donc, même si la translation layer détaillera davantage la sémantique, le document d'architecture impose déjà qu'ExoFS fournisse :
- une **syscall de résolution de chemin** ;
- retournant un `ObjectId` ;
- consommable par le `vfs_server`.

Autrement dit, ExoFS n'est pas purement object-store sans namespace : il doit au minimum offrir une primitive de résolution de chemin ou un équivalent déjà matérialisé comme syscall.

## 6.3 Isolation ExoPhoenix : ExoFS est la cible des flushs de services

Serveurs concernés :
- `init_server`
- `vfs_server`
- `memory_server`
- `crypto_server`

Tous ont un `isolation.rs` avec schéma :
- `PrepareIsolation`
- flush état vers ExoFS
- `PrepareIsolationAck`

ExoFS doit donc fournir des garanties pour :
- écrire des snapshots/états de service ;
- assurer qu'ils soient durables avant ack final ;
- supporter ce rôle dans une phase système sensible.

Même si le mécanisme d'IPC d'isolation n'est pas dans ExoFS, ExoFS doit offrir un `sync/commit` suffisamment fort pour être l'ancre persistante de cette séquence.

---

## 7. Boot, recovery, epochs

## 7.1 Boot : ExoFS est monté avant l'ouverture complète du système

L'étape 17 arrive après :
- mémoire prête ;
- scheduler prêt ;
- IPC + SHM pool initialisés.

Mais avant :
- le déblocage de sécurité global à l'étape 18 ;
- le démarrage complet des services Ring 1 dépendants d'ExoFS.

ExoFS doit donc être capable d'opérer dans une fenêtre où :
- le noyau est fonctionnel mais encore en bootstrap avancé ;
- l'environnement global n'est pas totalement stabilisé ;
- la recovery doit être fiable et simple.

## 7.2 `boot_recovery_sequence` est une exigence explicite

Le document ne détaille pas ici tout l'algorithme, mais l'appel est imposé :
- `Monter ExoFS + boot_recovery_sequence`

Donc la construction d'ExoFS doit inclure :
- un point d'entrée recovery séparé ou intégré au mount ;
- des états de montage distinguant volume propre / recovery exécutée / échec / mode dégradé ;
- un chemin compatible boot kernel.

## 7.3 Les epochs sont un sous-module explicite d'ExoFS

Arborescence :
- `fs/exofs/epoch/`
  - `epoch_record.rs    TODO:517 métadonnées rollback`
  - `gc/                GC -> create_kernel_thread() Phase 4`

Ce point est très important : l'architecture globale confirme que le concept d'**epoch** n'est pas accessoire mais structurel.

Implications :
- ExoFS doit gérer des enregistrements d'epoch/rollback ;
- des métadonnées de recovery associées ;
- un GC dédié, futur, lancé comme kernel thread.

Donc le noyau attend au minimum :
- une représentation des epochs ;
- des opérations de commit/publication ;
- des métadonnées de rollback ;
- une base pour GC ultérieur.

## 7.4 Un verrou de commit d'epoch existe déjà dans les contraintes globales

Dans les problèmes P1 :
- **LOCK-05** : `reserve_for_commit(n)` AVANT `EPOCH_COMMIT_LOCK`

Cela révèle des informations architecturales utiles :
- ExoFS a ou doit avoir un **chemin de commit synchronisé** ;
- il existe une notion de réservation avant commit ;
- la contention et l'ordre de locking autour du commit sont déjà une préoccupation système.

Même sans implémentation détaillée, cela impose :
- une API ou un mécanisme de réservation pour commit ;
- un verrou/état de commit explicite ;
- une attention particulière aux sections critiques du commit.

## 7.5 Le GC d'epochs doit être autonome et kernel-side

Feuille de route Phase 4 :
- `GC kthread autonome via create_kernel_thread() + timeout`

Donc l'architecture prévoit que certaines tâches ExoFS de maintenance seront :
- exécutées en **kernel thread** ;
- asynchrones ;
- séparées des syscalls foreground.

Cela influence déjà la structuration du code ExoFS :
- maintenance task séparée du plan de données ;
- état partagé synchronisé ;
- interfaces internes de scan / reclaim / cleanup.

---

## 8. Performance et observabilité

## 8.1 ExoFS est en ring0 pour la performance

Le document l'affirme explicitement en 1.1 :
- Ring 0 contient IPC et ExoFS **pour la performance**.

Donc les choix de design doivent minimiser :
- transitions inutiles de privilège ;
- médiations coûteuses ;
- copies excessives ;
- dépendances à des services externes sur chemin critique.

## 8.2 Path index : optimisation explicitement prévue

L'arborescence mentionne :
- `path/path_index.rs     SipHash-2-4 keyed (mount_secret_key depuis rng)`

Checklist :
- **S-12** : PathIndex = SipHash-2-4 keyed depuis `rng::fill_random()`.

Cela indique que l'architecture attend :
- un index de chemins/noms ;
- protégé par clé de montage ;
- optimisé ;
- résistant à certaines attaques de collision.

ExoFS doit donc intégrer ou préparer une vraie structure d'indexation des chemins, pas une résolution naïve linéaire.

## 8.3 Audit ring buffer lock-free : observabilité/sécurité en temps réel

La présence de `audit/ring_buffer.rs` avec mention lock-free implique :
- instrumenter sans bloquer fortement le chemin critique ;
- conserver une trace des opérations sensibles ;
- supporter surcharge partielle avec mécanismes de perte comptée.

La feuille de route ajoute :
- « sticky entries + compteur logs perdus ».

Donc l'observabilité ExoFS doit être pensée comme un sous-système natif :
- buffer d'audit ;
- événements structurés ;
- compteurs de pertes ;
- utilisable dans contexte ring0.

## 8.4 La performance de commit est une préoccupation explicite

Le problème `LOCK-05` montre que :
- l'ordre `reserve_for_commit(n)` avant `EPOCH_COMMIT_LOCK` est nécessaire.

Cela suggère :
- risque de blocage ou de contention ;
- besoin de préparer les ressources avant d'entrer dans la section critique du commit ;
- design commit path très important pour les performances.

## 8.5 Observabilité minimale attendue au moins sur sécurité et recovery

Même si le document ne définit pas une API de stats ExoFS complète, il impose de fait :
- audit de toutes opérations ;
- suivi des epochs/rollback ;
- visibilité sur le montage et la recovery au boot ;
- possibilité d'appuyer ExoPhoenix sur des persistences fiables.

Il faut donc prévoir des statuts/compteurs au minimum pour :
- mount state ;
- recovery state ;
- commit/epoch current ;
- audit dropped count ;
- erreurs d'IO / corruption / permission.

---

## 9. Contraintes spécifiques supplémentaires visibles dans la checklist globale

## 9.1 Syscalls ExoFS : plage 500–518, 519 réservé

Arborescence :
- `fs/exofs/syscall/            500-518 ExoFS + 519=réservé`

et `libs/exo-syscall/src/exofs.rs` confirme :
- wrappers des syscalls 500–518 ;
- 519 réservé.

Donc l'architecture impose une surface syscall ExoFS propre, bornée, et déjà réservée numériquement.

## 9.2 `verify_cap()` avant tout accès ExoFS

Checklist :
- **S-01** : obligatoire dans `fs/exofs/syscall/*`

C'est un invariant d'entrée de toutes interfaces publiques ExoFS.

## 9.3 Exécution d'objets ExoFS

Dans `process/exec.rs` :
- `verify_cap(EXEC) + is_valid(object_id) + ObjectKind != Secret`
- `load_elf(object_id)`

Et checklist :
- **S-10** : `exec()` sur Secret = `Err(NotExecutable)`

Donc ExoFS doit au minimum pouvoir :
- identifier le type/kind d'objet ;
- fournir le contenu ELF lié à un `ObjectId` ;
- refuser certains kinds.

## 9.4 Quotas

Checklist :
- **S-13** : quota vérifié AVANT allocation
- `quota_enforcement.rs`

Même si le module n'apparaît pas dans l'arborescence courte, l'architecture globale impose déjà pour ExoFS :
- une étape de vérification de quota avant toute allocation persistante ;
- donc un point d'intégration entre allocator espace disque et politique de quota.

---

## 10. Ce qui est architecturalement imposé vs ce qui reste ouvert

## 10.1 Clairement imposé par `ExoOS_Architecture_v6.md`

- ExoFS est **dans le noyau Ring 0**.
- `fs/` dépend de `memory`, `scheduler`, `security`.
- ExoFS est monté au boot à l'étape **17** avec `boot_recovery_sequence`.
- ExoFS possède une interface syscall **500–518**.
- Le `vfs_server` dépend d'ExoFS monté et utilise `SYS_EXOFS_PATH_RESOLVE`.
- Les accès ExoFS doivent faire `ObjectId::is_valid()` puis `verify_cap()/verify()`.
- ExoFS doit supporter `ObjectId` comme identifiant global natif.
- ExoFS doit intégrer :
  - crypto kernel-side,
  - path index SipHash,
  - audit ring buffer,
  - epochs / rollback metadata / commit path.
- ExoFS doit relâcher ses locks avant tout appel IPC.
- ExoFS doit servir de stockage persistant à plusieurs services système.

## 10.2 Restant ouvert à l'implémentation

Le document d'architecture ne fixe pas précisément :
- la structure exacte du superblock ;
- le format on-disk détaillé ;
- l'algorithme précis d'allocateur ;
- la représentation exacte des arbres/index ;
- la granularité logique bloc/page/extent côté ExoFS ;
- la sémantique détaillée des 19 syscalls ExoFS ;
- le degré exact de responsabilité namespace entre ExoFS et translation layer.

Autrement dit, l'architecture fixe surtout :
- les **frontières**,
- les **invariants de sécurité**,
- les **points d'intégration système**,
- les **contraintes de cycle de vie**.

---

## 11. Checklist finale — interfaces ExoFS requises par l'architecture globale

## 11.1 Interface bloc / backend stockage
- [ ] Un contrat backend bloc découplé du matériel concret.
- [ ] Lecture de secteurs/blocs.
- [ ] Écriture de secteurs/blocs.
- [ ] `flush` / persistance forcée.
- [ ] Découverte de taille/capacité du device.
- [ ] Compatibilité avec backend VirtIO block.
- [ ] Possibilité d'un adaptateur spécial (`virtio_adapter.rs`, `qemu_only`) sans contaminer le cœur ExoFS.

## 11.2 Interface cycle de vie ExoFS
- [ ] `mount()` utilisable au boot étape 17.
- [ ] `boot_recovery_sequence()` ou équivalent intégré.
- [ ] Chargement d'état persistant avant démarrage de `vfs_server`.
- [ ] `sync` / `flush_all` pour persistance forte.
- [ ] `unmount` ou arrêt propre si prévu.

## 11.3 Interface syscalls ExoFS
- [ ] Surface syscall dans la plage 500–518.
- [ ] 519 explicitement non utilisé/réservé.
- [ ] Vérification systématique `ObjectId::is_valid()` avant traitement.
- [ ] Vérification systématique `verify_cap()` / `check_access()` avant tout accès.

## 11.4 Interface objets persistants
- [ ] Lookup par `ObjectId`.
- [ ] Lecture contenu d'objet.
- [ ] Écriture contenu d'objet.
- [ ] Création/allocation d'objet persistant.
- [ ] Métadonnées de type/kind d'objet.
- [ ] Support de `ObjectKind::Secret`.
- [ ] Refus d'exécution d'objets secrets.
- [ ] Support de chargement ELF par `ObjectId` pour `exec()`.

## 11.5 Interface namespace / résolution de chemin
- [ ] Primitive `PATH_RESOLVE` exposée via syscall.
- [ ] Retour `PathBuf -> ObjectId` pour `vfs_server`.
- [ ] Index de chemin/noms (`PathIndex`) performant.
- [ ] `PathIndex` basé sur SipHash-2-4 keyed par `mount_secret_key`.
- [ ] Base suffisamment stable pour alimenter translation layer et VFS.

## 11.6 Interface epochs / recovery / commit
- [ ] Représentation d'`epoch_record`.
- [ ] Métadonnées de rollback.
- [ ] Publication/commit d'un état cohérent.
- [ ] Chemin `reserve_for_commit(n)` avant prise du verrou de commit.
- [ ] Verrou/section critique de commit d'epoch explicitement structurés.
- [ ] Base pour GC d'epochs.
- [ ] Support futur d'un GC autonome via kernel thread.

## 11.7 Interface sécurité / crypto
- [ ] Intégration native avec `security::verify()`.
- [ ] Contrôle de droits fins par capability.
- [ ] Validation structurelle d'identifiants avant décision de sécurité.
- [ ] Pipeline secret : `Blake3 -> LZ4 -> XChaCha20 -> disque`.
- [ ] Gestion de nonce basée sur compteur atomique + HKDF(object_id).
- [ ] Interdiction d'exposer le `BlobId` des objets secrets.
- [ ] Support `key_storage`/matériel crypto kernel-side si prévu.

## 11.8 Interface quotas / allocation
- [ ] Vérification de quota avant allocation persistante.
- [ ] Point d'intégration clair entre allocation disque et politique de quota.
- [ ] Erreurs distinctes pour quota dépassé / espace insuffisant / accès refusé.

## 11.9 Interface audit / observabilité
- [ ] Audit ring buffer lock-free.
- [ ] Journalisation de toutes les opérations ExoFS sensibles.
- [ ] Audit obligatoire de `GET_CONTENT_HASH`.
- [ ] Compteur de logs perdus / overflow.
- [ ] Statut de mount / recovery / commit observable.
- [ ] Distinction nette des erreurs :
  - IO backend,
  - corruption,
  - droits,
  - quota,
  - invariants/commit.

## 11.10 Contraintes transverses d'architecture
- [ ] Compatible environnement noyau ring0.
- [ ] Ne jamais tenir un lock ExoFS pendant un appel IPC bloquant.
- [ ] Structuration interne compatible boot précoce.
- [ ] Séparation claire entre logique FS et pilote matériel.
- [ ] APIs stables pour consommation par `vfs_server`, `init_server`, `memory_server`, `crypto_server`, `ipc_broker`.

---

## Conclusion synthétique

`ExoOS_Architecture_v6.md` place ExoFS comme **moteur de persistance natif du noyau**, au cœur du TCB Ring 0, avec un double rôle :

1. **backend persistant système** pour les objets, états de services et binaires ;
2. **fournisseur de primitives** pour les syscalls ExoFS et la couche `vfs_server`/translation layer.

Les contraintes architecturales les plus fortes pour la construction d'ExoFS sont :

- intégration noyau stricte ;
- backend bloc abstrait ;
- montage + recovery au boot ;
- contrôle d'accès par capabilities ;
- résolution de chemins au moins partielle côté ExoFS ;
- pipeline crypto kernel-side pour les objets secrets ;
- epochs / rollback / commit structurés ;
- audit et observabilité natifs ;
- interdiction de faire de l'IPC sous locks FS.

Ce document n'impose pas encore tous les détails algorithmiques du format ExoFS, mais il impose très clairement ses **frontières, invariants d'intégration et obligations systémiques**.