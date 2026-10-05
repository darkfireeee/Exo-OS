# ExoOS — module réseau V5 : transport conservé, autorités et DMA corrigés

**Statut :** spécification de remplacement de V4 pour une mise en œuvre incrémentale.
**Décision :** une seule pile réseau. V4 reste la base de compatibilité et de transport ; ExoNet apporte les frontières de confiance, l’administration et la révocation. V5 ne crée ni une seconde pile IP ni un sous-système de mémoire partagée générique.
**Périmètre :** connexions LAN et Internet, applications et services locaux, administration d’entreprise, DMA/IOMMU, résilience et performance mesurée.
**Ce document ne modifie pas le code.** Les sections marquées `[OBSERVÉ]` ont été relues dans le checkout local ; les choix sont marqués `[DÉCISION]`, les grandeurs non mesurées `[ESTIMATION]`, et les critères bloquants `[PORTE]`.

---

## 1. Décision exécutive

Exo-OS ne doit ni réécrire le réseau en bloc, ni conserver V4 sans changer ses frontières de confiance.

Une réécriture totale détruirait l’ABI sockets, le travail sur `smoltcp`, les tests de routage/DHCP/ICMP et la possibilité d’isoler une régression. À l’inverse, le chemin V4 actif laisse `network_server` allouer des pages DMA et lui donne des IOVA ; il utilise en outre le drapeau `BYPASS_IOMMU`. Un moteur IP qui analyse du trafic hostile ne doit pas disposer de cette autorité matérielle.

La V5 est donc une **refonte de châssis à compatibilité conservée** :

```text
                       façade inchangée pour applications
 Applications ─────────────────────────────────────────────────────────
    socket/connect/send/recv, descripteurs et erreurs POSIX existants
                                  │
 Ring 0                           ▼
 ┌───────────────────────────────────────────────────────────────────┐
 │ net_bridge : validation de pointeurs, fd, identite et NetGrant     │
 │ CapTable + NetAuthorityTable + AuthorityView : contrats bornés     │
 └───────────────────────────────────────────────────────────────────┘
                    │ IPC de contrôle authentifié, jamais IOVA brute
 Ring 1             ▼
 ┌───────────────────────┐      ┌────────────────────────────────┐
 │ Engine                 │      │ Arbiter                        │
 │ network_server         │      │ net_policy_server              │
 │ smoltcp, SocketTable   │      │ Pactes, leases, révocation     │
 └───────────┬───────────┘      └───────────────┬────────────────┘
             │ DmaLease                           │ Console / Lens / Ledger
             ▼                                    ▼
 ┌───────────────────────┐      ┌────────────────────────────────┐
 │ Portal                 │      │ Administration                  │
 │ virtio_net / e1000     │      │ Enveloppes, Couronne, audits    │
 │ NIC, IRQ, IOMMU, DMA   │      │ sans lecture de charge utile    │
 └───────────┬───────────┘      └────────────────────────────────┘
             ▼
          LAN / Internet
```

L’application conserve l’usage simple d’un socket. La complexité reste volontairement dans les services et les contrats vérifiables, pas dans l’expérience utilisateur.

---

## 2. Base locale et corrections apportées par V5

| Fait ou faiblesse | État V4 / convergence | Décision V5 |
|---|---|---|
| ABI sockets | Les appels 41–55 arrivent dans `net_bridge`. | Gelée : aucun nouvel appel applicatif obligatoire. |
| IPC inter-services | L’IPC existe ; `SpscRing` noyau est concret et non générique. | Aucun anneau SPSC partagé fictif entre Engine et Portal. Le contrôle passe par l’IPC existant ; un anneau de données n’est étudié qu’après mesure. |
| `smoltcp` | Le dépôt dépend de la copie locale `libs/vendors/smoltcp-upstream`. | `poll_ingress_single` est accepté uniquement parce que cette copie le définit ; un test de contrat bloque toute mise à jour qui le retirerait. |
| DMA | `NetBufPool` est alloué par Engine et `DriverInitMsg` contient deux IOVA. | `PortalPool` est la propriété exclusive de Portal. Aucun IOVA, aucune adresse CPU inter-processus dans les messages de contrôle. |
| IOMMU | Les chemins `network_server`, `virtio_net` et `e1000` contiennent `BYPASS_IOMMU`. | La suppression effective sur le chemin cible est DMA-0 : une porte de sécurité, pas une promesse. |
| retours RX | Le tableau `released_buf` courant peut saturer et les erreurs IPC sont ignorées. | Journal de retour acquitté, capacité au moins égale au pool RX, sans perte silencieuse ni recyclage avant accusé. |
| Phoenix TCP | Un grand `TcpStateStore` pourrait suggérer une reprise transparente. | Les sessions TCP externes sont rompues ; l’état ne sert qu’au diagnostic borné. |
| administration | Enveloppes/Couronne existaient mais le cérémonial était sous-décrit. | Compilation de politique, simulation d’impact, quorum/délai selon le risque, publication atomique et ExoLedger. |

### 2.1 Faits observés retenus

- `[OBSERVÉ]` `SYS_SHMGET`, `SYS_SHMAT`, `SYS_SHMCTL` et `SYS_SHMDT` sont routés vers `sys_enosys` dans `kernel/src/syscall/table.rs`. V5 ne s’appuie donc pas sur une SHM POSIX à terminer plus tard.
- `[OBSERVÉ]` `kernel/src/ipc/ring/spsc.rs` définit un `SpscRing` concret ; il ne justifie pas un hypothétique `SpscRing<T>` trans-processus.
- `[OBSERVÉ]` les messages V4 actuels sont transportés par l’IPC ; le `PacketRing` de `network_server` est local au processus.
- `[OBSERVÉ]` la dépendance `smoltcp` est un chemin local. Cette version expose `Interface::poll_ingress_single` et `poll_egress`, toutes deux documentées dans son propre code comme bornées. L’affirmation « smoltcp 0.12 » seule ne suffit pas à le garantir hors de ce checkout.
- `[OBSERVÉ]` `NET_INLINE_DATA_MAX` vaut 128 dans `net_bridge`. Les charges utiles longues ne doivent pas être cachées dans le contrôle IPC.
- `[OBSERVÉ]` les CapTokens ont un format fil de 20 octets et les envois IPC authentifiés vérifient le propriétaire, le destinataire, les droits et le type. Cette propriété est réemployée pour les objets réseau ; aucun secret partagé Engine/Arbiter n’est introduit.

Les commandes de relecture locale sont conservées dans le plan de migration. Elles doivent être réexécutées avant chaque implémentation, car un checkout évolue.

---

## 3. Responsabilités et frontières qui ne se recouvrent pas

| Composant | Possède | Peut faire | Ne peut jamais faire |
|---|---|---|---|
| Application | données et FD POSIX | demander un flux dans son Pacte | découvrir le graphe de droits, programmer la NIC, créer une lease |
| `net_bridge` | validation ABI, contexte appelant, durée de NetGrant | copier/mapper temporairement pour un appel socket | décider une destination ou publier du DMA |
| Engine | sockets, `smoltcp`, tables de flux, quotas de chemin chaud | parser L3/L4, ordonnancer un Brin déjà admis | obtenir une IOVA, appeler DMA, modifier une politique |
| Portal | PCI, IRQ, VirtIO/e1000, IOMMU, pages DMA | allouer/publier/quarantiner une page, filtrer L2 mécanique | décider quels utilisateurs ou services peuvent communiquer |
| Arbiter | Pactes, générations, leases, décisions d’ouverture | compiler une politique et déléguer un droit atténué | lire la charge utile, recevoir une IRQ, mapper DMA |
| Console | parcours administratifs et approbations | soumettre une mutation dans son Enveloppe | s’accorder elle-même un nouveau plafond |
| Lens | vues filtrées et explications | lire les événements autorisés | ouvrir un socket, modifier un Pacte, lire le contenu par défaut |
| Couronne | racine hors ligne et récupération | autoriser une opération exceptionnelle définie | devenir un compte réseau quotidien |

### 3.1 Raccourcis explicitement interdits

| Tentation | Refus V5 | Raison |
|---|---|---|
| Deux piles IP concurrentes | interdite | deux vérités TCP et des pertes impossibles à expliquer |
| Engine qui garde `SYS_DMA_ALLOC` | interdit après C1b | le parseur de paquets deviendrait une autorité matérielle |
| IOVA dans l’IPC | interdite | une adresse d’accès matériel n’est pas une donnée de contrôle |
| SHM générale « juste pour le réseau » | interdite | élargit le noyau et crée un canal inter-processus universel |
| HMAC partagé pour une lease | interdit | le vérificateur posséderait le pouvoir de fabriquer le MAC |
| Arbiter à chaque paquet | interdit | ralentit le chemin chaud et crée une dépendance inutile |
| restauration TCP externe | interdite | le pair distant n’est pas restauré avec ExoPhoenix |
| Console avec accès DMA ou Portal avec catalogue de Pactes | interdite | mélange autorité politique et mécanisme matériel |

Cette liste est un garde-fou de revue : une optimisation ne peut l’assouplir sans une nouvelle décision d’architecture.

---

## 4. Contrat applicatif : même simplicité, autorité explicable

### 4.1 Chemin sortant

```text
socket/connect
  -> net_bridge : fd, Sceau appelant, destination normalisée
  -> Engine : recherche d’une AdmissionLease bornée
       -> hit valide : demande de matérialisation du Brin
       -> miss : OpenIntent compact vers Arbiter
  -> Arbiter : compile et atténue le Pacte en NetworkLease
  -> noyau : émet une lease liée à l’instance Engine destinataire
  -> Engine : matérialise le Brin, crée le socket smoltcp
  -> application : fd normal ou errno + cause `exonet explain`
```

Le contrôle est fait à l’ouverture et aux transitions de génération, pas par inspection du contenu à chaque paquet. Le chemin chaud lit l’état compact du Brin : actif, génération, quotas, destination déjà admise.

### 4.2 Réception entrante et services publiés

Une application serveur n’obtient pas une exposition générale parce qu’elle appelle `bind` ou `listen`.

1. Elle possède un **Pacte de publication** qui fixe protocole, adresse/port, Cercle, limite de connexions et profil de pair.
2. L’ouverture de `listen` matérialise un **Brin de listener**. Le Portal ne laisse passer que ce qui correspond aux règles L2/L3 mécaniques associées ; Engine reste responsable du TCP.
3. Un `accept` crée un **Brin enfant** descendant du listener. Il consomme le budget de connexions et hérite de la même génération ; il ne demande pas un nouveau droit général.
4. Après révocation ou changement de génération, aucun enfant nouveau n’est admis. Les sessions existantes suivent la règle de drain du Pacte ou reçoivent une erreur.

Cette règle est importante pendant une panne de l’Arbiter : un listener déjà autorisé peut encore accepter dans son plafond ; il ne peut pas devenir une porte vers un nouveau service ou une autre destination.

### 4.3 LAN, Internet, DNS et chiffrement

V5 ne remplace pas IPv4/IPv6, Ethernet, DNS, TCP ou UDP par un protocole propriétaire. Elle ajoute l’autorité au-dessus des API et de la pile existante.

| Profil de Pacte | Usage | Règle vérifiable |
|---|---|---|
| `PinnedAddress` | bootstrap, DNS, test, pair LAN fixe | tuple protocole/adresse/port exact |
| `DeviceBound` | appareil LAN connu | identité d’appareil enrôlée plus frontière réseau explicite ; MAC seule jamais racine de confiance |
| `NameBoundTls` | service Internet ou LAN chiffré | résolution sous Pacte, port fixé et validation du nom/certificat par l’application |
| `NameBoundPlaintextException` | héritage interne exceptionnel | CIDR privé borné, durée, justification, approbation et audit obligatoires |

`NameBoundTls` ne lit pas le SNI dans le trafic. Cela resterait fragile avec le chiffrement du ClientHello et mélangerait politique et analyse L7. L’Engine autorise l’adresse produite par une résolution indexée par la génération de Pacte ; la bibliothèque applicative valide le certificat et le nom. Un changement de TTL ne redirige jamais une session déjà établie.

---

## 5. Portal V5 : nouveau propriétaire des pages et du périphérique

### 5.1 Arborescence cible

```text
drivers/network/virtio_net/ ou drivers/network/e1000/
├── main.rs                 # bootstrap Portal, only driver executable
├── portal_pool.rs          # pages DMA, états, compteurs, aucune IOVA exportée
├── portal_iommu.rs         # association NIC/domaine, invalidation, DMA-0
├── portal_control.rs       # IPC de contrôle V2, versionné et authentifié
├── portal_queue.rs         # Virtqueues/descripteurs, complétions et reset
├── portal_leases.rs        # DmaLease et registre de retours acquittés
└── portal_tests.rs         # simulation d’états, reset et injections de faute

servers/network_server/
├── smoltcp_iface.rs        # conservé, avec contrat de version vendored
├── socket_table.rs         # conservé et enrichi par Brin
├── driver_link.rs          # devient portal_client.rs, sans IOVA ni DMA
├── net_grant_client.rs     # copies synchrones via NetGrant/DmaLease
├── return_journal.rs       # acquittements ReleaseLeaseBatch
└── tcp_diagnostics.rs      # snapshots bornés, jamais restauration TCP
```

Le terme **Portal** désigne le rôle, pas exclusivement le pilote VirtIO. Il doit donc exister pour e1000 et tout futur NIC, avec le même contrat de sûreté.

### 5.2 États DMA — la sûreté l’emporte sur le délai

Une page DMA a exactement un état. Le fait que l’application a dépassé son délai ne prouve jamais que la NIC a cessé d’y accéder.

| État | CPU autorisé | NIC DMA | Sorties autorisées |
|---|---|---|---|
| `Free` | Portal | non | réserve TX ou publication RX |
| `MappedEngineTx` | Engine RW, Portal contrôle | non | `ReadyTx`, annulation |
| `ReadyTx` | aucun Engine | non | `PublishedTx` |
| `PublishedTx` | Portal | lecture possible | complétion ou `Quarantine` |
| `PublishedRx` | Portal | écriture possible | complétion ou `Quarantine` |
| `MappedEngineRx` | Engine RO, Portal contrôle | non | `ReturnPending` |
| `ReturnPending` | Portal seulement | non | accusé Portal puis publication RX |
| `Quarantine` | récupération contrôlée | aucune nouvelle utilisation | preuve de quiescence, puis `Free` |

**Règles :**

- Engine ne possède jamais une page pendant que la NIC la voit.
- Portal retire le mapping CPU d’Engine avant `PublishedTx` et avant toute nouvelle publication RX.
- un timeout produit une erreur logique, retire éventuellement un budget et place la page en quarantaine ; il ne la remet pas dans `Free` ;
- seule une complétion observée **ou** un reset de file/périphérique suivi de l’invalidation IOMMU confirmée autorise la sortie de `Quarantine`.

### 5.3 Retour RX fiable : remplacer le tableau perdable

V4 agrège les indices RX dans `released_buf: [u16; 64]`. Lorsque le tableau est plein ou qu’un envoi IPC échoue, l’indice peut être oublié. V5 remplace ce comportement par un `ReturnJournal` :

```text
slot RX termine dans Engine
    -> état MappedEngineRx vers ReturnPending
    -> bit `pending[slot] = 1` dans un bitmap de 256 bits
    -> ReleaseLeaseBatch contient jusqu’à 16 slots + generation
    -> Portal valide et accuse le batch
    -> seulement l’accusé efface les bits correspondants
    -> Portal remet les pages autorisées dans PublishedRx
```

- `[DÉCISION]` le journal a une entrée par page RX du pool ; un retour unique ne peut donc pas déborder.
- `[DÉCISION]` l’échec d’envoi conserve l’état `ReturnPending`. Cela peut réduire le débit ou provoquer `ENOBUFS`, mais jamais faire disparaître un droit de recyclage.
- `[DÉCISION]` un accusé est idempotent grâce à `{portal_epoch, lease_id, lease_generation}`. Une répétition réseau/interne ne crée pas une double remise en file.
- `[PORTE DMA-0]` une simulation force la saturation, des échecs IPC et des accusés dupliqués : aucune page n’est perdue, publiée deux fois ou réutilisée trop tôt.

### 5.4 Contrôle Portal V2

Les messages V4 ne sont pas supprimés du jour au lendemain, mais le nouveau canal porte toujours une version et n’échange jamais des IOVA. Toutes les tailles sont des assertions Rust dans la future implémentation.

```rust
#[repr(C)]
struct PortalCtrlHeaderV2 {
    ctrl_version: u16,  // 2
    opcode: u16,
    portal_epoch: u32,
    request_id: u32,
    flags: u32,
} // 16 octets

#[repr(C)]
struct ReleaseLeaseBatchV2 {
    hdr: PortalCtrlHeaderV2,
    count: u16,         // 0..=16
    _pad: u16,
    lease: [u32; 16],   // slots opaques, jamais indices/I/OVA bruts
} // 84 octets
```

Le transport IPC local observé supporte un payload inline de 192 octets ; il n’existe donc aucune raison technique de conserver artificiellement une limite de 48 octets pour le canal Portal V2. La limite V4 de 48 octets reste pour `NetMsg`/`NetReply`, c’est-à-dire le contrat ABI utilisé par `net_bridge`. V5 choisit **84 octets, versionnés et vérifiés**, au lieu d’un faux format de 48 octets qui omettrait génération ou accusé.

Les opérations V2 sont : `PortalHello`, `PortalReady`, `AcquireTxLease`, `PublishTxLease`, `RxLeaseReady`, `ReleaseLeaseBatch`, `ReleaseAck`, `Quiesce`, `QuiesceAck`, `ResetNotice` et `ProtocolError`. Chaque opération est liée à l’époque Portal ; une réponse ancienne est ignorée, journalisée et ne modifie pas la page.

---

## 6. Engine V5 : pile IP conservée et budget de travail explicite

### 6.1 Contrat `smoltcp` local

`Interface::poll_ingress_single` est une API de la copie vendored actuelle, pas une hypothèse sur toutes les versions publiées de `smoltcp`. C0 ajoute un test qui compile une utilisation minimale de `poll_ingress_single` et `poll_egress`. Toute mise à jour du vendor doit soit conserver ce contrat, soit modifier explicitement V5 et refaire les mesures.

Le choix n’est pas « un paquet pour toujours ». Le contrat est :

```text
à chaque tour Engine :
  ingress <= ingress_budget paquets ET <= ingress_budget_cycles
  egress  <= egress_budget  paquets ET <= egress_budget_cycles
  puis contrôle, sockets et retour vers l’ordonnanceur
```

| Profil initial | `ingress_budget` | `egress_budget` | Statut |
|---|---:|---:|---|
| Petit | 4 | 4 | `[ESTIMATION]` à valider par C0 |
| Standard | 16 | 16 | `[ESTIMATION]` à valider par C0 |
| Dense | 64 | 64 | `[ESTIMATION]` à valider par C0 |

Une limite de cycles protège le système si un paquet devient coûteux ; un budget de paquets seul ne suffit pas. Les deux limites sont réglées par profil au boot et visibles dans `exonet status`. Aucune boucle de drainage illimitée ne peut être introduite comme optimisation locale.

### 6.2 Données : une copie contrôlée, pas de DMA applicatif

```text
TX : mémoire application --NetGrant RO--> Engine --copie--> page Portal
     page Portal prête -> CPU Engine retiré -> NIC DMA

RX : NIC DMA -> page Portal -> DmaLease RO Engine --copie--> NetGrant RX
     fin copie -> ReturnPending -> accusé Portal -> réemploi possible
```

`NetGrant` n’est pas un mapping persistant. Il est borné, attaché à un socket/Brin précis, à un sens et à un appel en cours. L’application ne reçoit jamais une page Portal et la NIC ne DMA jamais dans une page applicative.

### 6.3 Coût assumé et métriques correctes

La copie Engine→Portal (TX) ou Portal→Engine→application (RX) est le prix conscient de la révocation et du confinement DMA. Elle est mesurée, pas masquée par un chiffre de latence V4 historique.

Pour un flux utile de `R` octets/s, une direction impose au minimum une copie de `R` octets/s ; le plein duplex impose environ `2R` de trafic de copie CPU. À 1 Gbit/s utile, `R` vaut 125 MB/s et un paquet de 1500 octets représente environ 83 333 paquets/s. À 10 Gbit/s, une direction demanderait déjà environ 1,25 GB/s de copie avant les coûts de pile, d’IPC ou de cache. Ce sont des calculs, pas des mesures de cette machine.

Les critères sont donc p50/p95/p99, paquets/s, débit utile, pertes, cycles/paquet, part de cycles IPC, occupation de pool et délai de révocation — pour 64, 512 et 1400 octets, en 1 puis plusieurs flux. Un débit de salon sans p99 sous charge ne valide rien.

---

## 7. Autorité : du Sceau au Brin, sans élévation possible

| Objet | Durée | Détenteur | Finalité |
|---|---|---|---|
| Sceau | durable | utilisateur, programme, service ou appareil | identité stable, pas une permission réseau vague |
| Pacte | versionné | Arbiter | destinations, protocoles, quotas, écoute, DNS et observabilité |
| AdmissionLease | courte | Engine destinataire | décision compilée d’ouvrir une relation exacte |
| NetworkLease | courte | Engine destinataire | capacité noyau non forgeable, liée à un Pacte et une instance |
| Brin | session | Engine + noyau | droit concret d’un socket ou d’un listener |
| Enveloppe | durable | administrateur | plafond non augmentable de ses mutations |
| Lentille | déléguée | opérateur/auditeur | lecture filtrée, sans droit d’action |

### 7.1 Trois compteurs, trois rôles

| Compteur | Change lors de | Effet |
|---|---|---|
| `authority_epoch` | reprise, rollback, état de sûreté global | invalide tous les caches de lease |
| `policy_sequence` | publication atomique | ordonne audit et snapshots, n’invalide pas seul tous les Brins |
| `pact_generation` | modification/révocation d’un Pacte | invalide ses leases et Brins descendants |

Un Brin est valable si son `authority_epoch`, sa génération de Pacte, sa date d’expiration, son Sceau et son endpoint restent valides. La `RevocationTable` est indexée, bornée et sans éviction d’une révocation active. Une saturation retourne `ENOSPC` lors d’une publication ; elle ne réanime jamais un droit supprimé.

### 7.2 Transfert de lease sûr

Arbiter ne transmet pas une chaîne d’octets qu’Engine pourrait réutiliser pour n’importe quel processus. Lors de `issue_lease_for_engine`, le noyau :

1. vérifie le `PactCap` de l’Arbiter, son droit d’émission, le sujet et la spécification compacte ;
2. vérifie l’instance Engine cible — pas seulement un PID réutilisable — et son `ServiceInstanceCap` ;
3. crée l’entrée `NetworkLease` avec le détenteur Engine, l’époque et la génération ;
4. installe directement un CapToken **possédé par cette instance Engine** ;
5. donne seulement un `lease_id` opaque dans le message de contrôle.

À la matérialisation, le noyau vérifie type, propriétaire, destinataire, génération et droits. Une copie d’un token vers un autre service échoue parce que son propriétaire enregistré ne correspond pas. Ce mécanisme réutilise la vérification de propriété existante des CapTokens ; il exige néanmoins des tests spécifiques V5.

### 7.3 Arbiter indisponible : continuité limitée, pas une autorité cachée

L’état par défaut reste `AuthorityFrozen` : aucune nouvelle ouverture arbitraire. Il protège mieux l’intégrité qu’un cache d’Engine signé avec une clé ambiguë.

V5 ajoute deux exceptions étroites, toutes deux déjà autorisées avant la panne :

- un listener actif peut créer des Brins enfants dans son plafond et sa génération courants ;
- un Pacte peut contenir des `ContinuityOpen` exacts : destination fixée, nombre maximal d’ouvertures, période, expiration courte et pas de DNS/délégation.

Ces tickets sont matérialisés par le noyau avant la panne, attachés au Pacte et à l’instance Engine. Ils ne sont pas des signatures qu’Engine interprète seul. Sans ticket, le résultat est `EAGAIN`/`ENETDOWN` explicable. Après le retour de l’Arbiter, l’époque est republiée et les tickets anciens sont invalidés.

---

## 8. Administration : un administrateur ne peut pas se fabriquer plus de droits

Une commande « admin réseau » ne doit jamais signifier « tout est permis ». Console transforme une intention humaine en mutation canonique de Pacte, dans l’Enveloppe de l’opérateur.

```text
proposition déclarative
  -> compilation statique et normalisation
  -> preuve d’inclusion dans l’Enveloppe et le parent
  -> simulation d’impact et diff lisible
  -> approbation(s) requise(s) selon le risque
  -> délai de réflexion si Couronne / portée élevée
  -> publication A/B atomique
  -> ExoLedger obligatoire et diffusion AuthorityView
```

### 8.1 Politique qui « ne compile pas »

Le langage de Pacte est déclaratif et borné ; ce n’est pas un script général. Le compilateur refuse avant toute publication :

- destination, protocole ou port non bornés ;
- enfant qui n’est pas un sous-ensemble strict du Pacte parent ;
- cycle de délégation ou tentative d’élévation d’Enveloppe ;
- quota, priorité, fenêtre de continuité ou Lentille au-dessus du parent ;
- publication entrante sans service/Sceau destinataire ;
- exception en clair sans justification, durée, Cercle et approbation ;
- changement qui dépasse le budget mémoire de son profil ;
- mutation non journalisable.

La console présente alors une erreur de conception et un diff, pas un échec tardif au milieu d’un flux.

### 8.2 Couronne, récupération et séparation des lectures

Une mutation de Couronne suit `proposition -> quorum -> délai -> signatures -> application atomique -> audit`. Elle est hors ligne ou via une cérémonie locale indépendante du réseau de production. Une `RecoveryEnvelope` est courte, minimale et ne peut qu’isoler/révoquer, restaurer un snapshot signé ou relancer les composants essentiels ; elle ne donne pas d’accès Internet général ni le pouvoir de créer une Enveloppe plus puissante.

Une Lentille sépare l’administration de la lecture : santé, décisions, volumes et motifs sont consultables selon filtre ; le contenu applicatif reste inaccessible par défaut. Sans `net_observe_server`, Engine conserve un `exonet explain` minimal et ExoLedger reçoit les événements de sûreté. L’absence de Lens réduit le confort, jamais les contrôles.

---

## 9. Mémoire bornée, sécurité et limites honnêtes

Les métadonnées noyau ne doivent pas devenir une allocation cachée dans le chemin chaud. Le tableau suivant est un budget de conception ; il sera remplacé par des `size_of` et des tests d’initialisation avant stabilisation.

| Profil | Pactes x 48 o | leases x 64 o | Brins x 64 o | NetGrants x 48 o | DmaLease x 32 o | ordre de grandeur métadonnées |
|---|---:|---:|---:|---:|---:|---:|
| Petit | 32 | 64 | 64 | 128 | 512 | ~34 KiB |
| Standard | 128 | 256 | 256 | 512 | 512 | ~84 KiB |
| Dense | 512 | 1024 | 1024 | 2048 | 2048 | ~324 KiB |

Les tailles de slots sont `[ESTIMATION]` et incluent leurs générations mais pas les pages de données, la CapTable existante ou les journaux. Un profil trop grand échoue explicitement au boot. Les pools pleins renvoient `ENOBUFS`, `EAGAIN` ou `ENOSPC` selon l’objet ; aucun slot actif n’est évincé.

### 9.1 Résidu assumé

Un résumé `{authority_epoch, policy_sequence, root_digest}` ne détecte pas en continu toute corruption mémoire d’un Pacte déjà activé si ni l’objet ni son digest n’est relu. V5 ne prétend pas le contraire. Ses mitigations sont les pages RO/copy-on-write, les générations, la vérification à la matérialisation, ExoLedger et le scrubbing de fond. Une garantie plus forte demanderait une redondance ou une vérification matérielle supplémentaire et reste hors v1.

---

## 10. Invariants de V5

| ID | Invariant |
|---|---|
| V5-1 | une page RX/TX a un seul état et un seul détenteur CPU actif |
| V5-2 | aucune page publiée à la NIC ne revient libre sans complétion ou quiescence confirmée |
| V5-3 | Engine ne peut ni allouer, ni mapper, ni programmer du DMA réseau après C1b |
| V5-4 | aucun IOVA ou pointeur CPU inter-processus n’apparaît dans le contrôle Portal V2 |
| V5-5 | un retour RX n’est effacé qu’après accusé idempotent de Portal |
| V5-6 | aucun paquet applicatif ne sort ni n’entre sans Brin actif descendant d’un Pacte valide |
| V5-7 | une copie de lease vers une autre instance ne donne pas une capacité utilisable |
| V5-8 | révocation, epoch et génération ferment toute action descendante nouvelle |
| V5-9 | `fork`, `exec` et transfert de FD n’augmentent pas l’autorité |
| V5-10 | la panne d’Arbiter ne permet que listener/enveloppes de continuité préexistants |
| V5-11 | Console/Lens ne donnent ni DMA ni lecture de charge utile hors Lentille |
| V5-12 | les budgets Engine sont à la fois en paquets et en cycles ; aucune boucle de drainage n’est infinie |

---

## 11. Ce que V5 prouve, ce qu’elle doit encore démontrer

V5 est une conception, pas une affirmation que l’IOMMU fonctionne aujourd’hui sur tout matériel. Les preuves attendues sont séparées :

| Niveau | Exigence |
|---|---|
| compilation | assertions de tailles ABI, aucun `BYPASS_IOMMU` sur la cible V5, API vendored présente |
| tests de modèle | V5-1 à V5-5, générations, retours dupliqués, saturation et transitions Phoenix |
| tests adversariaux | Engine appelle `SYS_DMA_ALLOC` après C1b ; token de lease d’une autre instance ; page réutilisée après timeout seul ; message V2 d’ancienne époque |
| intégration | sockets V4, DHCP, routage, ICMP, TCP/UDP, listener, DNS et erreurs POSIX |
| runtime | QEMU avec logs puis matériel IOMMU réel, faute DMA/IOMMU et reset de file |
| performance | protocole B0–B6 : pps, p99, CPU, copies, pools, perte et révocation durant trafic |

Le résultat de tests de compilation ou de simulation ne vaut jamais preuve de boot ou de comportement IOMMU matériel. Les deux doivent être publiés séparément.

---

## 12. Décision finale

V5 conserve ce qui est réel : ABI sockets, `net_bridge`, IPC de contrôle, `smoltcp`, SocketTable et les tests de transport. Elle remplace ce qui ne peut pas être sécurisé par un réglage : propriété DMA, IOVA exposées, bypass IOMMU, retours RX perdables et promesse TCP de Phoenix.

La suite n’est plus une nouvelle refonte conceptuelle : c’est l’ABI `NetGrant` v0 et une migration C0–C5 avec portes. Elles sont définies dans [EXONET_V5_NETGRANT_ABI_AND_MIGRATION.md](EXONET_V5_NETGRANT_ABI_AND_MIGRATION.md).

## Références locales

- `docs/recast/EXOOS_NETWORK_MODULE_V4.md` — transport et ABI V4 d’origine ; V5 en corrige les frontières DMA et le brouillon `RxReleaseMsg`.
- `docs/EXONET_PLAN_CONCEPTION_REALISTE_V1_2.md` — autorité, révocation, DNS, Phoenix et invariants ExoNet.
- `docs/recast/EXONET_V4_EXONET_V1_2_CONVERGENCE.md` — décision de convergence antérieure.
- `servers/network_server/src/buf_pool.rs`, `drivers/network/virtio_net/src/virtqueue.rs`, `drivers/network/e1000/src/main.rs` — bypass et ownership V4 observés.
- `kernel/src/syscall/net_bridge.rs`, `kernel/src/ipc/ring/spsc.rs`, `libs/vendors/smoltcp-upstream/src/iface/interface/mod.rs` — contrats locaux revalidés.
