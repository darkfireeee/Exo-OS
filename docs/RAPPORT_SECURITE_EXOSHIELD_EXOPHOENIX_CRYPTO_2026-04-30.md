# Rapport d’audit croisé — Sécurité / ExoShield / ExoPhoenix / Crypto

**Date** : 30 avril 2026  
**Périmètre** : `docs/recast`, `docs/Exo-OS-TLA+`, `kernel/src/security`, `kernel/src/exophoenix`, `servers/exo_shield`, `servers/crypto_server`, `libs/exo-phoenix-ssr`  
**Méthode** : lecture croisée des documents recast, des modèles TLA+ et du code actif. Je n’ai retenu que les écarts observables dans les fichiers lus. Quand un point n’était pas explicitement visible, je l’ai noté comme zone à vérifier plutôt que de l’inventer.

## Résumé exécutif

### Ce qui est bon
- Le noyau de sécurité `kernel/src/security/` est globalement aligné avec les invariants TLA et les docs recast sur le boot sûr, `SECURITY_READY`, PKS, CET et le journal d’audit.
- Le `crypto_server` centralise bien les primitives crypto côté Ring 1, ce qui colle à la règle SRV-02/SRV-04.
- Le protocole ExoPhoenix de haut niveau est bien présent dans le code : `stage0`, SSR partagée, freeze IPI, handoff, watchdog, et barrières mémoire Release/Acquire.

### Ce qui ne colle pas
- **ExoPhoenix est le plus désaligné** : la spec v6 dit `MAX_CORES = 64`, alors que la lib partagée est déjà passée à **256** cœurs, et plusieurs modules kernel gardent encore une limite dure à **64**.
- **ExoShield serveur** (`servers/exo_shield`) n’est pas l’“ExoShield v1.0” de la spec de boot : c’est un serveur PID 10 de confinement/process monitoring. En plus, son moteur de signatures contient encore une implémentation locale d’Ed25519 / hash au lieu de déléguer au `crypto_server`.
- **Le reseed post-Phoenix** demandé par la documentation n’apparaît pas câblé dans le code visible.
- **CAP-01 dans `crypto_server`** est mentionné, mais aucun appel effectif de vérification de capability n’est visible dans le fichier examiné.

## Lecture par module

## 1) Module sécurité noyau (`kernel/src/security/*`)

### Ce qui est cohérent
- `kernel/src/security/mod.rs` regroupe bien les sous-modules annoncés par la spec de sécurité : `capability`, `access_control`, `zero_trust`, `crypto`, `isolation`, `integrity_check`, `exploit_mitigations`, `audit`, puis les modules ExoShield v1.0 (`exocage`, `exoveil`, `exoledger`, `exokairos`, `exoargos`, `exoseal`, `exonmi`).
- L’ordre d’initialisation du code correspond à l’esprit des docs : `exoseal_boot_phase0()` d’abord, puis intégrité, capabilities, crypto, audit, access control, ledger, secrets kernel, ExoNmi, ExoCage, puis `exoseal_boot_complete()`.
- `SECURITY_READY` est bien un `AtomicBool` publié en `Release` et relu en `Acquire`, ce qui correspond au contrat TLA `SmpBoot.tla` et `Memory.tla`.
- `exoseal.rs` met bien en œuvre la logique “boot inversé” : verrouillage IOMMU NIC, CET global, PKS par défaut révoqué, puis retour à l’état normal à la fin du boot sécurité.
- `exoveil.rs` correspond bien à la philosophie documentée : PKS avec domaines `Default`, `Caps`, `Credentials`, `TcbHot`, révocation O(1), et restauration partielle à la fin du boot.
- `exocage.rs` implémente bien la logique CET + shadow stack + token au sommet, avec sauvegarde des flags TCB dans `_cold_reserve`.
- `exoledger.rs` implémente bien le journal append-only P0 et le chaînage Blake3.
- `exokairos.rs` garde bien la deadline cachée hors TCB, avec budget atomique et vérification inline.

### Écarts / tensions détectés

#### 1. `security_init()` est documenté comme une séquence unique, mais le code est scindé en deux phases
- **Docs** : `kernel/src/security/mod.rs` décrit `security_init()` comme l’orchestrateur unique.
- **Code** : le boot sécurité est en pratique réparti entre `exoseal_boot_phase0()`, `security_init()`, puis `exoseal_boot_complete()`.
- **Gravité** : mineure à moyenne. Ce n’est pas forcément un bug, mais la narration documentaire ne reflète pas la structure réelle du code.

#### 2. Le module “ExoShield v1.0” du noyau et le serveur `exo_shield` ne sont pas le même composant
- **Docs** : `docs/recast/ExoShield_v1_Production.md` présente ExoShield comme une architecture de boot et d’invariants matériels (ExoSeal, ExoCage, ExoVeil, ExoKairos, ExoCordon, ExoLedger, ExoArgos, ExoNmi), avec un “orchestrateur léger” qui démarre après `SECURITY_READY`.
- **Code** : `kernel/src/security/*` implémente ces modules de boot/invariants, tandis que `servers/exo_shield/src/main.rs` est un **PID 10 AI/Process Containment Security Server**.
- **Gravité** : majeure sur le plan de la nomenclature et de la responsabilité. Les deux “ExoShield” sont conceptuellement liés, mais ce ne sont pas les mêmes objets de code.

#### 3. Le chemin d’audit de la sécurité est cohérent avec les TLA, mais les modèles TLA sont plus abstraits que le code
- `docs/Exo-OS-TLA+/SmpBoot.tla` impose notamment : `security_init()` avant `SECURITY_READY`, APs bloqués tant que le flag n’est pas publié, `gs:[0x20]` écrit avant syscall, etc.
- Le code suit bien ces jalons, mais la preuve TLA ne couvre pas les détails d’implémentation réels des modules ExoShield v1.0 (par exemple le découpage `exoseal` / `exoledger` / `exokairos`).
- **Conclusion** : cohérence globale, mais niveau d’abstraction différent.

## 2) ExoShield côté serveur (`servers/exo_shield`)

### Ce qui est cohérent
- Le serveur est bien structuré comme un service de confinement/process security avec `engine`, `behavioral`, `network`, `sandbox`, `forensics`, `signatures`.
- La couche `ipc_gate` et les politiques d’IPC existent, ce qui colle à l’idée d’une barrière de sécurité Ring 1.
- Les fichiers de signatures et de règles montrent une intention claire de validation de politiques et de détection.

### Écarts importants

#### 1. Scope différent de la spec ExoShield v1.0
- **Docs** : `ExoShield_v1_Production.md` décrit un système de boot/invariants matériels, observateur Kernel B, CET, PKS, IOMMU et handoff.
- **Code** : `servers/exo_shield` est un serveur PID 10 de confinement de processus, pas le bootstrap matériel ExoShield décrit par la spec.
- **Gravité** : majeure. Il y a un vrai décalage de responsabilité entre le document et l’implémentation du serveur.

#### 2. La gestion crypto/signatures est encore locale dans `exo_shield`
- `servers/exo_shield/src/signatures/update.rs` contient encore une implémentation locale de vérification Ed25519 et un hash simplifié.
- Le fichier le dit lui-même : **“En production, cela serait remplacé par le vrai SHA-512 via le crypto_server.”**
- Cela contredit l’intention d’architecture centralisée vue dans les docs recast (`SRV-02`, `SRV-04`) où les opérations crypto Ring 1 doivent passer par `crypto_server`.
- **Gravité** : majeure. C’est l’écart le plus clair entre ExoShield serveur et la séparation crypto voulue.

#### 3. Les docs TLA ExoShield modélisent des invariants de boot et d’IPC, pas ce serveur concret
- `docs/Exo-OS-TLA+/ExoShield.tla` modélise : `WatchdogMissed`, `HandoffFlag`, whitelist IOMMU NIC, graphe IPC autorisé, quotas.
- `docs/Exo-OS-TLA+/ExoShield_v1.tla` modélise : `SecurityReady`, `NetworkEnabled`, `MutableFS`, `BudgetMap`, `P0Log`.
- Le serveur `exo_shield` réel expose plutôt un moteur de scan, des politiques, du forensics et du sandboxing applicatif.
- **Gravité** : moyenne à majeure, surtout si l’on s’attend à une correspondance “module = un modèle TLA = un composant runtime”. Ici, on a plutôt un empilement de plusieurs abstractions.

## 3) ExoPhoenix (`kernel/src/exophoenix/*` + `libs/exo-phoenix-ssr`)

### Ce qui est cohérent
- Le noyau a bien les briques attendues : `stage0`, `ssr`, `interrupts`, `handoff`, `sentinel`, `forge`, `isolate`.
- `stage0.rs` construit la table `apic_to_slot`, détecte le mode APIC, calibre `TICKS_PER_US`, prépare les gardes IOMMU et les garde-fous de boot.
- `interrupts.rs` implémente les handlers `0xF1`, `0xF2`, `0xF3` avec des actions lock-free et des ACKs SSR.
- `handoff.rs` utilise bien `SSR_HANDOFF_FLAG` avec `Release/Acquire`, et collecte les ACKs dans une fenêtre de timeout.
- Le modèle TLA `ExoPhoenixHandoff.tla` correspond bien à cette logique de freeze / ACK / snapshot / restore.

### Écarts majeurs

#### 1. `MAX_CORES` a divergé entre spec v6 et code partagé
- **Spec** : `docs/recast/ExoPhoenix_Spec_v6.md` dit explicitement `MAX_CORES = 64`.
- **Code partagé** : `libs/exo-phoenix-ssr/src/lib.rs` dit `SSR_MAX_CORES_LAYOUT = 256`.
- **Impact** : le layout SSR n’est plus celui de la spec v6. Le code a clairement migré vers CORR-02.
- **Gravité** : majeure. La documentation v6 n’est plus la source de vérité du layout.

#### 2. Offsets SSR différents entre la spec v6 et la lib actuelle
- **Spec v6** : `SSR_PMC` à `0x1080`, `SSR_LOG_AUDIT` à `0x8000`, `SSR_METRICS_PUSH` à `0xC000`.
- **Lib actuelle** : `SSR_PMC_OFFSET = 0x4080`, `SSR_LOG_AUDIT_OFFSET = 0xC000`, `SSR_METRICS_OFFSET = 0xE000`.
- **Impact** : la spec v6 ne décrit plus le layout réellement utilisé par le code partagé.
- **Gravité** : majeure. C’est un drift de layout compile-time, donc potentiellement silencieux si un lecteur suit encore la spec v6.

#### 3. Les modules kernel gardent une limite dure à 64 cœurs
- Dans `kernel/src/exophoenix/isolate.rs`, `handoff.rs` et `forge.rs`, on voit encore des gardes du style `slot >= 64`.
- Cela veut dire que, malgré la lib SSR à 256 cœurs, plusieurs chemins runtime ignorent encore tout slot au-delà de 63.
- **Gravité** : majeure, voire bloquante pour du SMP > 64 cœurs. Le système peut devenir partiellement non isolé au-delà de cette borne.

#### 4. Le format des ACKs de freeze a changé par rapport à la spec v6
- **Spec v6** : un ACK par cœur paddé sur 64 bytes, pour éviter le false sharing SMP.
- **Lib actuelle** : `freeze_ack_offset(apic_id) = base + apic_id * 4`, et le code lit/écrit via `AtomicU32`.
- **Impact** : le runtime est cohérent avec lui-même, mais la spec n’est plus à jour.
- **Gravité** : moyenne à majeure selon qu’on considère la spec ou la lib comme source de vérité.

#### 5. Le contrat de reseed post-Phoenix n’est pas câblé dans le code visible
- **Docs** : `GI-05_ExoPhoenix.md` et les corrections associées exigent l’envoi d’un `PhoenixWakeEntropy` vers `crypto_server` avant tout autre IPC au réveil.
- **Code** : je n’ai trouvé aucun symbole ni appel `PhoenixWakeEntropy`/reseed correspondant dans le code noyau ou serveur.
- `servers/crypto_server/src/xchacha20.rs` et `src/main.rs` utilisent bien un nonce compteur + sel, mais cela ne remplace pas le contrat de réensemencement post-restore demandé par les docs.
- **Gravité** : majeure, car elle touche directement à la non-réutilisation des nonces et à la reprise post-restore.

### Point de cohérence partielle
- Le TLA `ExoPhoenixHandoff.tla` est bien aligné sur le schéma général : `HandoffFlag`, `FreezeAckBitmap`, `KernelBState`, `EpochID`, `NonceSeed`, `FreezeTimer`.
- Le code reprend l’esprit, mais pas toutes les hypothèses opérationnelles de la doc (notamment le reseed crypto explicite après restore et la borne 64 vs 256).

## 4) Crypto (`kernel/src/security/crypto`, `servers/crypto_server`)

### Ce qui est cohérent
- Le noyau `kernel/src/security/crypto/mod.rs` expose bien les primitives attendues : `blake3`, `xchacha20_poly1305`, `rng`, `kdf`, `x25519`, `ed25519`, `aes_gcm`.
- `servers/crypto_server` centralise bien les opérations crypto Ring 1 : dérivation HKDF-Blake3, chiffrement XChaCha20-Poly1305, hash Blake3, signatures Ed25519, etc.
- Le serveur retourne des **handles opaques** pour les clés, au lieu de faire sortir des octets bruts, ce qui est cohérent avec les docs recast et la règle SRV-02/SRV-04.
- `servers/crypto_server/src/xchacha20.rs` utilise un nonce construit à partir d’un compteur monotone + sel, ce qui évite la réutilisation triviale des nonces dans le serveur.

### Écarts / risques

#### 1. CAP-01 est annoncé, mais pas démontré dans le fichier examiné
- Le commentaire en tête de `servers/crypto_server/src/main.rs` mentionne explicitement : **“CAP-01 : vérification de capability token en première instruction”**.
- Dans le fichier lu, je n’ai trouvé **aucun appel effectif** à une fonction du type `verify_cap_token()` / `capability verify` avant l’enregistrement IPC ou la boucle principale.
- **Gravité** : majeure. Si le commentaire est vrai mais l’implémentation absente, la première barrière de sécurité du serveur n’est pas garantie.

#### 2. Le chemin “Phoenix restore → reseed crypto_server” n’apparaît pas
- `servers/crypto_server` fait du nonce management interne ; le code ne montre pas de réception d’un message de réveil Phoenix ni d’un reseed déclenché par le kernel B.
- Cela croise l’écart ExoPhoenix ci-dessus : la doc demande un évènement explicite, le code visible ne le montre pas.
- **Gravité** : majeure pour l’invariance de nonces après restore.

### À ne pas confondre
- Le module crypto du noyau n’est pas un problème en soi : la règle SRV-02 vise les cratés Ring 1, pas les primitives Ring 0 du noyau.
- Autrement dit, `kernel/src/security/crypto/*` est compatible avec l’architecture, alors que l’implémentation locale d’Ed25519 dans `servers/exo_shield` l’est beaucoup moins.

## Écarts croisés les plus importants

| Domaine | Écart | Gravité | Fichiers de preuve |
|---|---|---:|---|
| ExoPhoenix | spec v6 `MAX_CORES=64` vs lib 256 | majeure | `docs/recast/ExoPhoenix_Spec_v6.md`, `libs/exo-phoenix-ssr/src/lib.rs` |
| ExoPhoenix | offsets SSR v6 vs offsets actuels | majeure | `docs/recast/ExoPhoenix_Spec_v6.md`, `libs/exo-phoenix-ssr/src/lib.rs` |
| ExoPhoenix | hard cap runtime `slot >= 64` | majeure | `kernel/src/exophoenix/isolate.rs`, `handoff.rs`, `forge.rs` |
| ExoPhoenix/Crypto | reseed post-restore non câblé | majeure | `docs/recast/GI-05_ExoPhoenix.md`, `servers/crypto_server/src/main.rs`, `kernel/src/exophoenix/*` |
| ExoShield | serveur réel ≠ boot architecture ExoShield v1.0 | majeure | `docs/recast/ExoShield_v1_Production.md`, `servers/exo_shield/src/main.rs` |
| ExoShield/Crypto | Ed25519 / hash local au lieu de crypto_server | majeure | `servers/exo_shield/src/signatures/update.rs` |
| Crypto | CAP-01 non visible dans `crypto_server` | majeure | `servers/crypto_server/src/main.rs` |

## Ce qui est cohérent

- `SECURITY_READY` est bien publié/consommé en `Release/Acquire` dans le noyau.
- Le boot sécurité suit la logique attendue : IOMMU/NIC lock, PKS default-deny, CET global, puis retour à l’état opérationnel.
- Les handlers ExoPhoenix `0xF1/0xF2/0xF3` sont bien présents et lock-free dans l’esprit des docs.
- Le TLA `SmpBoot.tla` est globalement cohérent avec la séquence de boot du noyau.
- Le graphe IPC et les propriétés de base ExoShield TLA (`BootSafety`, `IommuEnforced`, etc.) sont cohérents avec la direction du code.
- `crypto_server` conserve les clés à l’intérieur du processus et ne retourne que des handles opaques.

## Zones encore à vérifier

1. **CAP-01 exact dans `crypto_server`**  
   Le commentaire existe, mais il faut vérifier si la vérification de capability est ailleurs (macro, wrapper IPC, ou code non inspecté).

2. **Chemin exact du reseed post-Phoenix**  
   Je n’ai pas trouvé le symbole `PhoenixWakeEntropy` dans le code lu. S’il existe sous un autre nom, il faut le relever et le relier au restore.

3. **Source de vérité du layout SSR**  
   La lib `exo-phoenix-ssr` semble être la vérité runtime actuelle, alors que `ExoPhoenix_Spec_v6.md` est clairement en retard. Il faut décider si la doc doit être marquée obsolète ou réécrite.

4. **Portée exacte du serveur `exo_shield`**  
   Il faudrait documenter explicitement que le serveur PID 10 n’est pas le même composant que la couche boot/security “ExoShield v1.0”.

5. **Garde `slot >= 64`**  
   Si l’objectif est bien d’exploiter `SSR_MAX_CORES_LAYOUT = 256`, cette borne doit être revisitée dans `isolate.rs`, `handoff.rs` et `forge.rs`.

## Conclusion

Le noyau de sécurité est plutôt bien aligné avec les intentions de la documentation et des modèles TLA. Les plus gros écarts se situent ailleurs :

1. **ExoPhoenix** : drift fort entre spec v6 et implémentation actuelle, surtout sur `MAX_CORES`, les offsets SSR et les bornes runtime.
2. **ExoShield serveur** : scope différent de la spec v1.0 de boot, et crypto encore localement réimplémentée dans `signatures/update.rs`.
3. **Crypto** : `crypto_server` est globalement conforme, mais le contrat CAP-01 n’est pas visible dans le fichier examiné et le reseed post-Phoenix n’est pas câblé dans le code vu.

En l’état, je considérerais le noyau sécurité comme **cohérent mais à documenter**, ExoPhoenix comme **à harmoniser d’urgence**, et ExoShield serveur comme **à re-scoper / clarifier** avant de parler de complétion.
