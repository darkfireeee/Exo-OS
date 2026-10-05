# ExoNet V5 — ABI `NetGrant` v0, budget noyau et migration contrôlée

**Compagnon de :** [EXOOS_NETWORK_MODULE_V5.md](EXOOS_NETWORK_MODULE_V5.md).
**Statut :** contrat d’implémentation proposé. Aucun numéro ou format n’est stable avant C0 et la revue des preuves CapToken.
**But :** fermer les trous laissés par les formulations « mapping temporaire » et « changement noyau minimal » : opérations exactes, bornes, erreurs, transfert, `fork`, révocation, tests et portes de décision.

---

## 1. Contraintes de départ et principes ABI

1. L’ABI BSD sockets 41–55 et `NetMsg`/`NetReply` V4 restent inchangés.
2. `SYS_SHMGET`/`SHMAT` restent non implémentés : `NetGrant` ne les active pas indirectement.
3. Les CapTokens conservent leur format fil de 20 octets et leur contrôle propriétaire/destinataire existant.
4. Aucun utilisateur ne reçoit de page DMA ; aucun Engine ne reçoit une IOVA.
5. Toute extension noyau est bornée, typée réseau et doit s’accompagner de tests de génération/révocation.
6. Les appels longs `send*`/`recv*` sont découpés en fenêtres synchrones ; ils ne créent pas une relation mémoire durable entre deux processus.

### 1.1 Budget noyau fermé : quatre extensions, pas davantage

| Extension | Objet exact | Pourquoi elle est nécessaire | Ce qu’elle ne devient pas |
|---|---|---|---|
| K1 — types CapToken réseau | `NetworkPact`, `NetworkLease`, `NetworkSession`, `PortalDevice`, `ObservationScope` | la vérification de type et de détenteur doit rester dans la source de vérité CapTable | un deuxième système de jetons |
| K2 — `NetAuthorityTable` | métadonnées compactes de Pacte, lease, Brin, révocation et continuité | validité O(1), générations, saturation déterministe | le graphe complet de politique ou un moteur de règles général |
| K3 — `NetGrant`/`DmaLease` | prêt/mapping temporaire et états de page | données supérieures aux 128 octets inline sans DMA applicatif | SHM persistante, mapping arbitraire ou prêt hors réseau |
| K4 — `AuthorityView` | page RO et notification d’époque | invalidation fiable d’un cache même si une notification est perdue | stockage de charge utile ou base de données de politique |

Toute proposition ultérieure doit se rattacher explicitement à K1–K4. Sinon elle est hors périmètre et exige une décision d’architecture. Cette règle évite qu’un « petit détail » devienne progressivement un sous-système générique.

### 1.2 Surface de syscall proposée

Le bloc des drivers s’arrête actuellement à 549 et la table système est dimensionnée à 550. V5 réserve donc, sous condition de mise à jour atomique de `numbers.rs`, `syscall_abi`, dispatcher et taille de table :

```text
550  SYS_NET_CONTROL   # K1, K2 et publication/consultation K4
551  SYS_NET_GRANT     # K3 : mapping temporaire et DmaLease
SYSCALL_TABLE_SIZE = 552
```

Ce sont deux portes ABI, non deux mécanismes génériques. Elles ne sont accessibles qu’avec des capacités réseau appropriées. `SYS_DMA_ALLOC` reste disponible pour les autres drivers, mais **ne peut plus être la voie de l’Engine réseau** après C1b : l’allocation des pages réseau passe par `SYS_NET_GRANT(PortalAllocatePool)` et exige `PortalDeviceCap`.

---

## 2. `SYS_NET_CONTROL` : autorité et transfert non forgeable

`SYS_NET_CONTROL(op, request_ptr, request_len, cap_wire_ptr, reply_ptr, 0)` est un multiplexeur étroit ; le noyau valide les pointeurs, la longueur, la version et le token avant de lire la requête. Une requête inconnue retourne `ENOTSUP`, jamais un comportement permissif.

| `op` | Appelant autorisé | Entrée principale | Résultat |
|---|---|---|---|
| `RegisterPortal` | init/boot avec `PortalDeviceCap` | instance pilote, NIC et budget de pool | association Portal/NIC/domaine IOMMU |
| `PublishPolicy` | Arbiter avec `NetworkPactCap` | projection compacte d’un Pacte compilé | transaction A/B, `policy_sequence` et epoch |
| `IssueLeaseForEngine` | Arbiter avec droit `ISSUE` | décision exacte et `ServiceInstanceCap` Engine | token de lease installé chez le destinataire |
| `MaterializeBrin` | Engine détenteur de lease | lease + tuple/socket/listener | Brin ou erreur atomique |
| `RevokePact` | Arbiter/Couronne dans Enveloppe | slot, génération et mode drain | révocation indexée et publication d’époque |
| `MapAuthorityView` | Engine/Arbiter/Lens autorisés | token d’instance | page RO déjà filtrée |
| `AcknowledgeRecovery` | Couronne/recovery | snapshot, signatures et portée | sortie restreinte ou refus |

Les structures de politique riches restent dans l’Arbiter. `PublishPolicy` ne fournit au noyau qu’une projection fixe : slot, parent, génération, plafonds, empreinte, statut, expiration et classes autorisées.

### 2.1 Livraison sûre d’une `NetworkLease`

Le résultat de `IssueLeaseForEngine` est une mutation de CapTable du destinataire, pas un token au choix de l’appelant.

```text
Arbiter --PactCap/ISSUE--> noyau
  noyau vérifie : Pacte, Enveloppe, sujet, génération, EngineInstance
  noyau crée : NetAuthorityTable[lease_id]
  noyau installe : NetworkLeaseCap propriétaire = EngineInstance
  noyau notifie : {lease_id, pact_generation, authority_epoch}
Engine --NetworkLeaseCap/MATERIALIZE--> noyau
```

Le message contient un identifiant opaque mais la permission reste un CapToken possédé par la bonne instance. Un Engine compromis peut employer ses propres leases dans leurs plafonds ; il ne peut pas convertir l’identifiant d’une autre instance en droit utilisable. C’est précisément le test d’attaque T3.

### 2.2 `AuthorityView`

La vue RO contient seulement :

```text
{ abi_version, authority_epoch, authority_state,
  policy_sequence, root_digest_hint, portal_epoch }
```

La lecture est Acquire ; la publication d’un nouvel état est Release. Une notification IPC est un accélérateur, jamais une condition de sécurité. Une lecture qui voit une autre époque invalide immédiatement le cache d’ouverture local.

---

## 3. `SYS_NET_GRANT` v0 : contrat complet

### 3.1 Rôles et bornes

| Objet | Source -> destination | Droits CPU | Taille | Durée |
|---|---|---|---:|---|
| `AppTxGrant` | page application -> Engine | Engine RO | 1 à 16 pages, max 64 KiB | jusqu’à copie confirmée dans l’appel `send*` |
| `AppRxGrant` | page application -> Engine | Engine RW | 1 à 16 pages, max 64 KiB | jusqu’à copie confirmée dans l’appel `recv*` |
| `DmaLeaseTx` | page Portal -> Engine | Engine RW | 1 page, MTU v1 <= 1500 | jusqu’à `ReadyTx`/annulation |
| `DmaLeaseRx` | page Portal -> Engine | Engine RO | 1 page, MTU v1 <= 1500 | jusqu’au retour acquitté |

La fenêtre applicative n’est jamais sauvegardée au-delà du retour du syscall socket. TCP peut être segmenté en fenêtres ; un datagramme dépassant la limite de fenêtre V1 retourne `EMSGSIZE` plutôt que d’être fragmenté de façon cachée. Jumbo frame est explicitement hors V1 et exige une révision de l’ABI/états.

### 3.2 Formes fil proposées

```rust
#[repr(C)]
struct NetGrantRequestV0 {
    abi_version: u16,  // 0
    op: u16,
    flags: u32,
    session_or_portal: u32,
    peer_instance: u32,
    address: u64,      // adresse applicative seulement pour App{Tx,Rx} interne
    length: u32,       // 1..=65536
    _reserved: u32,
} // 32 octets

#[repr(C)]
struct NetGrantReplyV0 {
    grant_id: u32,
    generation: u32,
    mapped_addr: u64,  // adresse seulement dans l’espace du détenteur
    mapped_len: u32,
    state: u16,
    abi_version: u16,
    authority_epoch: u64,
} // 32 octets
```

`address` n’est admis que par l’appel interne de `net_bridge`, qui connaît le PID source et le Brin associé au FD. Une application ne peut pas appeler `SYS_NET_GRANT` pour mapper une page chez un arbitraire : le handler exige un `NetworkSessionCap` déjà associé au FD et la paire autorisée application↔Engine. Les opérations Portal n’acceptent jamais une adresse fournie par Engine.

| Opération | Appelant | Transition | Résultat |
|---|---|---|---|
| `BeginAppTx` | `net_bridge` interne | libre -> Engine RO | grant et mapping Engine |
| `BeginAppRx` | `net_bridge` interne | libre -> Engine RW | grant et mapping Engine |
| `PortalAllocatePool` | Portal avec `PortalDeviceCap` | aucun pool -> pages Portal `Free` | pages DMA attachées au seul domaine Portal |
| `AcquirePortalTx` | Engine | `Free -> MappedEngineTx` | DmaLease TX |
| `SealPortalTx` | Engine | `MappedEngineTx -> ReadyTx` | CPU Engine retiré |
| `PublishPortalTx` | Portal | `ReadyTx -> PublishedTx` | descripteur NIC publié |
| `AcquirePortalRx` | Engine après complétion Portal | `PublishedRx -> MappedEngineRx` | DmaLease RX RO |
| `ReturnPortalRx` | Engine | `MappedEngineRx -> ReturnPending` | entrée journalisée |
| `AckReturnPortalRx` | Portal | `ReturnPending -> PublishedRx` | accusé idempotent |
| `CompleteApp` / `Cancel` | détenteur ou noyau | grant -> retiré | unmap/unpin sûr |
| `PortalReset` | Portal | publié -> `Quarantine` | arrêt, invalidation et reprise contrôlée |

### 3.3 États, révocation et quiescence

Un `NetGrant` applicatif est consommé une fois : `Free -> Mapped -> Copying -> Complete` ou `Cancelled`. Il ne peut être remonté, élargi, re-délégué ou réactivé.

Une `DmaLease` suit la machine d’états du module V5. L’expiration d’un Brin empêche une nouvelle publication mais ne détruit pas un mapping qui pourrait encore être vu par le matériel. La séquence est toujours : retirer le droit logique, annuler/terminer l’opération, observer complétion **ou** reset + invalidation IOMMU, puis seulement retirer/réutiliser la page.

### 3.4 Erreurs stables

| Situation | Erreur visible | Effet sûr |
|---|---|---|
| token/type/propriétaire invalide | `EACCES` | aucun mapping |
| version ou op inconnue | `ENOTSUP` | aucune compatibilité implicite |
| epoch/génération obsolète | `EAGAIN` | cache invalidé, aucun FD/grant partiel |
| taille, alignement, sens invalide | `EINVAL` | aucun pinning |
| fenêtre/lease/pool saturé | `ENOBUFS` ou `EAGAIN` | état existant inchangé |
| datagramme V1 trop grand | `EMSGSIZE` | aucune fragmentation cachée |
| Portal en reset/quarantaine | `ENETDOWN` ou `EAGAIN` | page non réutilisée |
| révocation/Phoenix d’un flux | `EPIPE` ou `ECONNRESET` | l’application décide de se reconnecter |

### 3.5 `fork`, `exec`, transfert de FD et sortie processus

- `NetGrant` n’est jamais hérité : sa durée est interne à l’appel socket synchrone et son mapping dans Engine est retiré avant le retour vers l’application.
- Les FD réseau sont invalides dans un fils par défaut. Avec `inherit_fork`, père et fils pointent vers le **même** Brin, quotas et génération communs ; aucun Brin nouveau n’est créé.
- `exec` réévalue le Sceau de l’exécutable ; les sessions incompatibles sont fermées.
- transmettre un FD à un Sceau/instance distinct exige une délégation de Pacte explicite et crée une référence bornée, jamais une extension des droits.
- crash/exit d’Engine ou d’application révoque les grants non terminés ; les pages DMA suivent `Quarantine` jusqu’à quiescence.

---

## 4. Portes de migration C0–C5

Le dual-run compare les **décisions** d’autorité, jamais deux transmissions sur la même NIC. À tout instant, une seule pile possède les descripteurs et les pages du périphérique.

| Phase | Travail | Sortie obligatoire | Estimation de travail |
|---|---|---|---:|
| C0 | figer ABI, tracer le chemin réel, exécuter la base de tests, mesurer B0/B2 | sources/versions/commandes et `exonet explain` minimal publiés | 3–6 sem.-pers. |
| C1a | Portal alloue un pool de test, attache un domaine IOMMU, effectue mapping/fault contrôlé ; trafic V4 inchangé | Go/no-go matériel avant toute refonte Engine | 6–10 |
| C1b | chemin boot exclusif Portal ; supprimer IOVA/`SYS_DMA_ALLOC` Engine et faire passer contrôle par IPC V2 | Engine refusé au DMA, V4 transport adapté conserve son comportement | 8–14 |
| C1c | quiescence, reset VirtIO/e1000, journal de retour et tests de double propriété | **DMA-0** : aucun bypass sur chemin cible | 6–10 |
| C2 | K1/K2/K4, Pactes `PinnedAddress`, shadow échantillonné puis complet | aucune divergence non expliquée sur Cercle test | 10–18 |
| C3 | enforcement, Console/Lens, listeners/continuité, `fork`/FD | aucun open non admis après révocation ou Frozen | 8–14 |
| C4 | K3 `NetGrant`, copies, QoS, DNS contrôlé | états DMA et B2/B3/B4 publiés | 14–24 |
| C5 | Phoenix, panne Arbiter/Portal/Engine, déploiement par Cercle | B5/B5bis et recovery administrative passent | 10–18 |

**Total C0–C5 : 65–114 semaines-personnes.** C’est une enveloppe de planification, non un engagement calendaire. C1 est volontairement la zone de variance la plus élevée ; l’optimisation DataRing/multi-queue est hors total et ne démarre qu’après goulot mesuré.

### 4.1 Conditions de C1a, C1b et C1c

**C1a — faire fonctionner le Portal sans toucher au trafic.** Le test de mapping IOMMU utilise une page et un périphérique/domaine de test, puis une faute contrôlée. Il prouve la chaîne de base ou arrête le programme avant d’empiler politique et service sur une fondation inconnue.

**C1b — basculer une fois, jamais en parallèle.** Au boot, `legacy-v4` et `portal-v5` sont deux modes exclusifs. Dès `portal-v5`, Engine échoue explicitement à `SYS_DMA_ALLOC` pour une allocation réseau. Les tests V4 attachés aux anciens types peuvent être réécrits, mais leurs propriétés fonctionnelles — DHCP/routage/ICMP/sockets, pas de double RX/TX, pas de trafic avant prêt — doivent être reroulées dans l’adaptateur V5.

**C1c — passer DMA-0.** Les pannes durant DMA, retour dupliqué, queue bloquée, reset et invalidation sont injectés. Une page ne revient jamais `Free` parce qu’un délai a expiré. Une machine sans IOMMU opérationnelle ne démarre pas le réseau V5 ; elle peut éventuellement démarrer sans réseau, avec une cause auditée. Il n’existe pas de mode « sûr mais bypass ».

---

## 5. Tests, benchmarks et critères de refus

### 5.1 Tests adversariaux minimaux

| ID | Injection | Oracle |
|---|---|---|
| T1 | Engine appelle `SYS_DMA_ALLOC` après C1b | refus ; aucune allocation/pin/IOMMU nouveau |
| T2 | Engine forge une IOVA, un `lease_id` ou un message V2 ancien | `EACCES`/erreur de protocole ; aucun descripteur changé |
| T3 | Engine présente la lease d’une autre instance | échec de vérification propriétaire/destinataire |
| T4 | saturation ReturnJournal + échec IPC + ack répété | aucune fuite, aucune double publication RX |
| T5 | expiration pendant TX/RX en vol | erreur logique puis quarantaine jusqu’à quiescence |
| T6 | révocation pendant `connect`, listener et trafic | pas de Brin enfant/new send illicite ; erreurs explicables |
| T7 | crash Arbiter | seuls listeners/tickets `ContinuityOpen` préexistants survivent |
| T8 | `fork`/`exec`/transfert FD | pas de création ni amplification de Brin |

### 5.2 B0–B6

| Banc | Mesure ou propriété |
|---|---|
| B0 | ABI, taille des structures, versions vendor, profils mémoire, états |
| B1 | connect/listen/accept, TCP/UDP, refus et `exonet explain` |
| B2 | 64/512/1400 o, pps, p50/p95/p99, copies, cycles IPC et pools |
| B3 | quotas/fairness de plusieurs Pactes, réserves survie/contrôle |
| B4 | révocation pendant B2/B3, générations et quarantaine |
| B5 | panne Engine/Portal/Kernel A, reset et DMA quiescent |
| B5bis | panne Arbiter pendant open, listener et ticket de continuité |
| B6 | token périmé/forgé, DNS interdit, fork, double lease, tempête L2 |

Chaque résultat précise révision, compilation, CPU/RAM, QEMU/TAP ou matériel réel, MTU, nombre de files, durée, taille/flux, p50/p95/p99, pps, débit utile, CPU, pertes, erreurs et mémoire. Les estimations V4 de microsecondes ou Gbit/s ne sont pas des critères d’acceptation.

### 5.3 Commandes de revalidation locale C0

```bash
rg -n "pub struct SpscRing" kernel/src/ipc/ring/spsc.rs
rg -n "SYS_SHMGET|SYS_SHMAT" kernel/src/syscall/table.rs
rg -n "BYPASS_IOMMU" servers/network_server/src drivers/network
rg -n "fn poll_ingress_single" libs/vendors/smoltcp-upstream/src
wsl.exe -e bash -lc 'cd /mnt/c/Users/xavie/Desktop/Exo-OS && cargo test -p exo-network-server --no-default-features'
```

Un test de service vert ne remplace pas un boot QEMU avec journal debugcon/série ni le test IOMMU réel. C0 enregistre donc les trois niveaux séparément : compilation/test, image ISO, exécution instrumentée.

---

## 6. Décisions de performance après, non avant, la mesure

V5a utilise IPC de contrôle + `NetGrant`/copie contrôlée. Un `DataRing` n’est considéré que si B2 attribue plus de 25 % des cycles au contrôle/mapping/doorbell **et** montre backlog/pertes avec un lien sous-utilisé. Toute variante doit conserver les mêmes états, le Release/Acquire portable et le même mécanisme de révocation.

Une optimisation ne peut ni réintroduire le DMA applicatif, ni exporter une IOVA, ni convertir `NetGrant` en mapping durable. Si elle requiert cela, elle échoue à V5 et devient un nouveau projet.

---

## 7. Définition honnête des livrables

| Niveau | Signification |
|---|---|
| pilot | C0 et C1a : chemin mesuré, pas de promesse de réseau sûr utilisable |
| réseau DMA sûr | C1b/C1c : Portal propriétaire, DMA-0, transport non régressé |
| v1 contrôlée | C2/C3 : autorité et administration en application sur Cercle test |
| flux contrôlés | C4 : `NetGrant`/QoS/DNS et mesures B2–B4 |
| résilience réseau | C5 : pannes injectées, recovery et déploiement progressif |

Cette granularité évite de dire « réseau sécurisé » parce que seule une interface socket ou un test unitaire fonctionne. La première action utile reste C0 ; C1a est le premier stop/go matériel.

## Références locales

- [EXOOS_NETWORK_MODULE_V5.md](EXOOS_NETWORK_MODULE_V5.md) — architecture, états DMA, administration et invariants V5.
- `kernel/src/security/capability/token.rs` et `kernel/src/security/capability/mod.rs` — format et vérification de CapToken à étendre avec les preuves associées.
- `kernel/src/syscall/numbers.rs`, `kernel/src/syscall/table.rs`, `servers/syscall_abi/src/lib.rs` — plage/dispatcher ABI actuels.
- `servers/network_server/src/buf_pool.rs`, `servers/network_server/src/driver_link.rs`, `drivers/network/virtio_net/src/virtqueue.rs` — chemin V4 à retirer du chemin cible.
