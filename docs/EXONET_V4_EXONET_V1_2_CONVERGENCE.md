# ExoNet — convergence du module V4 et de l’architecture v1.2

**Statut :** décision d’architecture et plan de refonte incrémentale.
**But :** conserver la compatibilité et les acquis fonctionnels de V4, tout en remplaçant les frontières qui empêchent la sûreté par capacités, le confinement DMA et la performance mesurable d’ExoNet v1.2.

Ce document ne juxtapose pas deux piles réseau. Il définit **une seule pile**, découpée en deux responsabilités complémentaires :

- **monde V4 :** compatibilité applicative, IPC déjà opérationnel, pile TCP/IP et invariants de transport ;
- **monde ExoNet v1.2 :** autorité, confinement DMA, administration, observabilité, révocation et reprise honnête.

La fusion est une superposition de contrats, pas une moyenne entre deux idées.

---

## 1. Conclusion exécutive

Une réécriture complète de tous les modules réseau et du noyau en une seule livraison serait le choix le plus risqué : elle supprimerait les tests V4, casserait l’ABI sockets déjà active et rendrait tout échec difficile à localiser.

Conserver V4 inchangé serait aussi un mauvais choix : la propriété DMA est du mauvais côté de la frontière de confiance et le chemin actif utilise encore `BYPASS_IOMMU`.

La décision est donc une **refonte de châssis à compatibilité conservée** :

```text
     MONDE V4 CONSERVÉ                     MONDE EXONET AJOUTÉ / REMPLACÉ

 applications / ExoFS                Sceau, Pacte, Lease, Brin, Lentille
 ABI socket et net_bridge             autorité, révocation, administration
 smoltcp / SocketTable                quotas, QoS, diagnostic
 IPC existant                         Portal propriétaire DMA/IOMMU
 tests V4 de propriété                DmaLease, epochs et politique A/B
```

L’application ne voit pas la transition. Elle continue à appeler les sockets POSIX. Le noyau ne reçoit pas une pile réseau complète ni une SHM générique : il reçoit seulement des contrats réseau spécialisés et bornés.

## 2. Méthode et éléments vérifiés deux fois

Les décisions de ce document reposent sur le plan V4, sur ExoNet v1.2 et sur une vérification du code actuel. Les faits marqués **[OBSERVÉ]** proviennent de deux sources concordantes : le document V4/audit V4 et le code vivant.

| Élément | Vérification documentaire | Vérification code actuelle | Conclusion |
|---|---|---|---|
| SHM POSIX | V4 signale l’absence de handlers | `SYS_SHMGET`, `SYS_SHMAT`, `SYS_SHMCTL`, `SYS_SHMDT` vont à `sys_enosys` dans `kernel/src/syscall/table.rs` | pas de SHM générique dans le plan |
| Anneau SPSC | V4 indique un `SpscRing` concret | `kernel/src/ipc/ring/spsc.rs` définit `pub struct SpscRing`, non générique | ne pas concevoir `SpscRing<u16>` |
| ABI sockets | V4 prévoit `net_bridge` | les syscalls 41–55 appellent effectivement `kernel/src/syscall/net_bridge.rs` | ABI à figer et réutiliser |
| DMA V4 | V4 place le pool dans `network_server` et transmet les IOVA | `NetBufPool::init()` alloue RX/TX ; `DriverInitMsg` contient `rx_base_iova`, `tx_base_iova` | frontière DMA à remplacer |
| Contournement IOMMU | signalé par ExoNet v1.2 | les deux modules passent `DMA_MAP_FLAGS_BYPASS_IOMMU`; le mapper retourne alors l’adresse physique | DMA-0 est bloquant |
| Invariants V4 | audit V4 et TLA de stress | `cargo test -p exo-network-server --no-default-features` a réussi : 7 tests, dont DHCP, routage, ICMP, sockets et stress | acquis de transport à préserver |

Les chiffres de performance de V4 — notamment ~1,3 µs ou 3/6 Gbit/s — restent des **estimations historiques**, non des mesures. Ils ne sont pas des critères d’acceptation de la nouvelle architecture.

## 3. Les deux mondes, définis correctement

### 3.1 Monde A — V4 : le contrat de transport et de compatibilité

V4 apporte une réponse pragmatique à une contrainte réelle d’Exo-OS : les services Ring 1 disposent d’IPC, mais pas d’une SHM générique stable. Son architecture est donc pertinente pour :

- garder les syscalls BSD sockets 41–55 et ExoFS ;
- transporter les opérations de contrôle par messages fixes ;
- réutiliser `smoltcp`, SocketTable, routage, DHCP, ICMP et TCP/UDP ;
- maintenir des limites statiques (`no_std`, pas d’allocation cachée dans le chemin chaud) ;
- préserver l’invariant RX : le pilote ne remet un buffer en circulation qu’après la libération explicite du moteur ;
- conserver les tests de saturation, handles de socket, routage et Phoenix déjà écrits.

V4 n’est donc pas « à jeter ». C’est la **façade compatible** et une partie du moteur de transport.

### 3.2 Monde B — ExoNet v1.2 : le contrat d’autorité et de sûreté

ExoNet ajoute ce que V4 ne tente pas de fournir :

- aucun accès réseau ambiant ;
- identité durable du sujet (**Sceau**) ;
- politique durable et délégable (**Pacte**) ;
- droit de session temporaire (**Brin**) ;
- capacité d’ouverture compilée (**AdmissionLease**) ;
- visibilité sans lecture de charge utile (**Lentille**) ;
- administration par Enveloppes et Couronne, pas par super-utilisateur réseau ;
- propriété DMA dans un **Portal**, pas dans la pile IP ;
- révocation par génération et états de page ;
- époques A/B, ExoLedger et reprise ExoPhoenix sans promesse de résurrection TCP.

ExoNet n’est pas une seconde pile TCP/IP. Il encadre le moteur V4 et remplace précisément ses frontières de confiance insuffisantes.

### 3.3 Ce qui ne doit surtout pas être fusionné

| Tentation | Pourquoi elle est refusée |
|---|---|
| deux piles IP en parallèle | deux états TCP, deux sources de vérité, diagnostic impossible |
| SHM générique pour « accélérer V4 » | non disponible, élargit le noyau et introduit un mécanisme réutilisable hors réseau |
| conserver les IOVA brutes V4 dans ExoNet | laisse l’Engine choisir ou recevoir la mémoire exposée à la NIC |
| faire décider le Portal des Pactes | mélange matériel et politique ; *confused deputy* |
| faire vérifier chaque paquet par l’Arbiter | goulot de performance et point de défaillance inutile |
| restaurer l’état TCP V4 après Phoenix comme s’il était valide | le pair distant n’a pas été restauré ; la session doit être rompue et réouverte |

## 4. Architecture convergée : une pile, cinq frontières

```text
 Ring 3
 ┌────────────────────────────────────────────────────────────────────┐
 │ Applications : API ExoFS/POSIX, aucun privilège réseau ambiant     │
 └──────────────┬─────────────────────────────────────────────────────┘
                │ ABI V4 stable : socket/connect/send/recv/...
 Ring 0         ▼
 ┌────────────────────────────────────────────────────────────────────┐
 │ net_bridge : copie/valide les arguments, associe le contexte cap   │
 │ CapTable + NetAuthorityTable + NetGrant : contrats étroits         │
 └──────────────┬─────────────────────────────────────────────────────┘
                │ contrôle IPC V4 (messages fixes, IDs/caps, jamais data brute)
 Ring 1         ▼
 ┌───────────────────┐      ┌──────────────────┐
 │ Engine            │<---->│ Arbiter          │
 │ network_server    │      │ net_policy_server│
 │ smoltcp, sockets  │      │ Pactes/leases    │
 └───────┬───────────┘      └────────┬─────────┘
         │ DmaLease / PortalCmd                │ Lens / Console / ExoLedger
         ▼                                     ▼
 ┌───────────────────┐                  ┌──────────────────┐
 │ Portal            │                  │ administration   │
 │ virtio_net        │                  │ et observation   │
 │ NIC/IOMMU/DMA     │                  └──────────────────┘
 └─────────┬─────────┘
           ▼
         NIC / LAN / Internet
```

### 4.1 Responsabilités sans recouvrement

| Frontière | Possède | Ne peut pas faire |
|---|---|---|
| `net_bridge` | ABI, copie/validation de pointeurs, contexte d’appel | décider une destination, allouer DMA, parser des paquets |
| Engine | sockets, smoltcp, flux, quotas au fil de l’eau | choisir une politique, programmer l’IOMMU, posséder la NIC |
| Arbiter | Sceaux, Pactes, Leases, générations, décisions d’ouverture | recevoir une IRQ, lire les paquets, produire une adresse DMA |
| Portal | PCI/VirtIO, IRQ, IOMMU, pages DMA et états de descripteurs | savoir quel utilisateur a le droit d’aller sur Internet |
| Lens/Console | consultation filtrée, approbation et administration | émettre des paquets ou écrire une file VirtIO |

Un bug de l’Engine reste grave pour les flux qu’il traite ; il ne doit toutefois pas devenir un bug qui lui permet de reprogrammer la NIC, de déléguer de nouveaux droits ou de contourner une révocation.

## 5. Ce qui est gelé, ce qui est refactoré, ce qui est nouveau

### 5.1 Contrats gelés

| Élément V4 | Décision | Justification |
|---|---|---|
| syscalls sockets 41–55 | gel de l’ABI externe | protège ExoFS, applications et autres services |
| `net_bridge` comme façade | conserver le module et ses codes d’erreur | limite le changement noyau à l’intérieur de la façade |
| IPC SPSC/Raw RPC existant | conserver pour le contrôle | mécanisme existant, testé, sans SHM générique |
| `NetMsg` / `NetReply` V4 | conserver version 1 | compatibilité des appels ; les nouveaux objets passent par IDs/caps |
| `smoltcp` et SocketTable | conserver à la première étape | éviter une réécriture TCP non motivée par une mesure |
| tests V4 et modèle de stress | conserver et enrichir | ils couvrent déjà RX/TX, sockets et Phoenix |

### 5.2 Contrats refactorés

| Élément V4 | Limite actuelle | Cible convergée |
|---|---|---|
| `NetBufPool` de `network_server` | Engine alloue DMA et connaît IOVA | `PortalPool` alloué et mappé dans le seul domaine Portal |
| `DriverInitMsg` | donne `rx_base_iova` et `tx_base_iova` au pilote | `PortalReady` ne transporte que pool-capacité, génération et tailles |
| `RxReleaseMsg` | indices de pool Engine-propriétaire | `ReleaseLeaseBatch` : identifiants de leases Portal, même message de contrôle compact |
| `TxSubmitMsg` | index dans pool Engine | `PublishTxLease` : Portal valide lease, état et longueur |
| `ExoNetDevice` | tampon sous propriété Engine | adaptateur `smoltcp::phy::Device` au-dessus de leases Portal |
| `driver_link` | transmet des IOVA brutes | client Portal, requiert/rend des leases |
| `tcp_store` Phoenix | risque de faire croire à une reprise TCP | diagnostic/local only ; sessions externes sont signalées rompues |
| boucle un-paquet | utile pour bornage, insuffisant pour haut débit | budget de paquets explicite, mesuré, jamais boucle illimitée |

### 5.3 Nouvelles briques, limitées au réseau

| Brique | Emplacement privilégié | Rôle exact |
|---|---|---|
| `NetworkPact`, `NetworkLease`, `NetworkSession`, `ObservationScope` | CapTable existante | types de capacités, générations, droits atténués |
| `NetAuthorityTable` | noyau, mémoire bornée | métadonnées compactes de lease/Brin, pas le graphe complet de politique |
| `AuthorityView` | page noyau RO, réseau seulement | `authority_epoch`, état et séquence pour invalidation O(1) |
| `NetGrant` / `DmaLease` | DMA + capacité noyau | prête/mappe temporairement un buffer sous état contrôlé |
| `net_policy_server` | Ring 1 | compilation des Pactes et décisions, sans NIC |
| `net_observe_server` / `net_admin_server` | Ring 1 | Lentilles, Console et Enveloppes ; peuvent être chargés à la demande |

## 6. Changement minimal et justifié du noyau

Le refus d’une grande refonte du noyau ne signifie pas « zéro changement ». Il signifie que chaque changement doit être spécifique au réseau, attestable et non réutilisable comme autorité générique par un autre sous-système.

### 6.1 Ce qui ne change pas dans le noyau

```text
scheduler/                 aucun ordonnancement de paquets dans Ring 0
ipc/ring/                  aucune SHM, aucun nouvel anneau générique
fs/exofs/                  ABI sockets conservée
memory_server SHM          aucune tentative de rendre SYS_SHMGET générique
exoPhoenix général         aucune modification de sa sémantique pour sauver TCP
```

### 6.2 Les quatre extensions autorisées

1. **Types et vérification de capacité réseau.** Étendre `CapObjectType` et les preuves associées. La source de vérité reste `CapTable` ; aucun deuxième jeton n’est créé.
2. **Table d’autorité bornée.** Le noyau stocke uniquement ce qui est nécessaire à la vérification d’une lease et à la révocation : slots, générations, liens vers Brins, expiration et limites compilées. Les règles d’entreprise complètes restent dans l’Arbiter.
3. **Prêt mémoire réseau spécialisé.** `NetGrant` mappe temporairement une région application dans l’Engine (lecture TX / écriture RX) ou une page Portal dans l’Engine. Il ne crée ni segment SHM persistant, ni mapping libre entre deux processus arbitraires, ni DMA direct d’application.
4. **Vue d’époque et notifications.** `AuthorityView` est une page RO et une notification de changement ; elle ne porte pas des données applicatives et ne devient pas une base de politique en mémoire partagée.

Ces extensions peuvent être fournies par un petit nombre d’opérations réseau internes ou de syscalls dédiés, suivant le framework de pilotes existant. L’ABI POSIX reste inchangée.

### 6.3 Pourquoi `NetGrant` n’est pas une SHM déguisée

| SHM générique refusée | `NetGrant` accepté |
|---|---|
| deux processus choisissent librement quoi partager | une capacité réseau déjà vérifiée désigne une région précise |
| mapping durable et réutilisable | mapping temporaire, révocable, attaché à une opération/Brin |
| peut devenir un canal clandestin universel | sens, droits, taille et durée imposés par le noyau |
| peut être mappée par n’importe quel service autorisé | seulement application↔Engine ou Engine↔Portal |
| ne connaît ni IOMMU ni état matériel | prêt Portal lié à l’état DMA et à la quiescence |

Cette distinction est la clé pour obtenir des gros transferts sans ouvrir le chantier risqué d’un sous-système SHM global.

## 7. Contrats d’ouverture : de POSIX au Brin

### 7.1 Ouverture sortante

```text
1. Application appelle socket/connect via ABI V4.
2. net_bridge valide les pointeurs et récupère le contexte de capacité du Sceau.
3. Engine cherche une AdmissionLease valide dans son cache borné.
4. Hit : Engine demande au noyau de matérialiser le Brin exact.
   Miss : Engine envoie un OpenIntent compact à l’Arbiter.
5. Arbiter compile/valide le Pacte et demande au noyau une NetworkLease.
6. Le noyau vérifie type, génération, époque, sujet et endpoint, puis crée le Brin.
7. Engine crée le socket smoltcp et lie fd V4 <-> Brin.
8. L’application reçoit le fd habituel ou une erreur POSIX explicable.
```

La lease est une capacité noyau typée, non une signature Ed25519/HMAC vérifiée par l’Engine. Une clé HMAC partagée donnerait à l’Engine le pouvoir de produire le même MAC qu’il devrait vérifier ; elle est donc rejetée.

### 7.2 Admission et invalidation

| Compteur | Signification | Effet sur cache |
|---|---|---|
| `authority_epoch` | récupération, redémarrage d’Arbiter, compaction/racine | invalide toutes les leases |
| `policy_sequence` | ordre de publication et audit | n’invalide pas seul les leases non concernées |
| `pact_generation` | modification/révocation d’un Pacte | invalide lease et Brins de son slot |

`AuthorityView` permet à l’Engine de voir un changement d’époque par une lecture Acquire, même si la notification est perdue. L’Arbiter absent mène à `AuthorityFrozen` : aucune nouvelle ouverture, y compris sur cache ; les Brins déjà actifs suivent leur durée et leur règle de reprise.

## 8. Contrats de données : de l’application à la NIC

### 8.1 Les trois chemins, à ne pas confondre

| Chemin | Taille et mécanisme | Utilisation |
|---|---|---|
| contrôle V4 | `NetMsg`/IPC fixe | open, close, politiques, leases, indices, erreurs |
| données v1a | `NetGrant` + copie unique Engine↔Portal | charge utile en lots bornés, première version sûre |
| données v1b | anneau SPSC de descripteurs par application↔Engine | seulement si B2 montre que le contrôle IPC est le goulot |

Le contrôle V4 reste utile. Il ne doit jamais être utilisé pour transporter les charges utiles longues, car le bridge actif est limité à 128 octets inline.

### 8.2 TX

```text
Application MemoryRegion
      | NetGrant RO temporaire
      v
Engine copie vers DmaLease Portal RW
      | Engine rend l'accès CPU
      v
Portal vérifie lease + Brin + état, publie VirtIO
      | DMA dans le domaine IOMMU du Portal
      v
NIC
```

### 8.3 RX

```text
NIC DMA -> page Portal PublishedRx
      | complétion VirtIO
      v
Portal accorde DmaLease RO à Engine
      | Engine copie vers NetGrant RX applicatif
      v
application, puis ReleaseLeaseBatch -> Portal
```

Les identifiants de lease tiennent dans les messages V4 existants. Aucune adresse virtuelle inter-processus ni IOVA brute n’est nécessaire dans l’IPC de contrôle.

### 8.4 Machine d’états qui étend l’invariant V4

| État | CPU autorisé | DMA NIC | Transition suivante |
|---|---|---|---|
| `Free` | Portal | non | réserve TX ou publication RX |
| `ReservedTx` | Portal | non | mapping Engine TX / annulation |
| `MappedEngineTx` | Engine RW | non | prêt TX / annulation |
| `ReadyTx` | aucun Engine | non | publication Portal |
| `PublishedTx` | Portal | lecture possible | complétion / quarantaine |
| `PublishedRx` | Portal | écriture possible | complétion / quarantaine |
| `MappedEngineRx` | Engine RO | non | libération |
| `Quarantine` | récupération | aucune réutilisation | quiescence DMA puis `Free` |

Les propriétés V4 « pas de double RX, pas de fuite RX, pas de double TX, pas de fuite TX » sont conservées et renforcées. Une page ne retourne jamais à `Free` sur le seul écoulement d’un timeout.

## 9. DMA, IOMMU et Phoenix : ordre de sûreté

### 9.1 Règle non négociable

Une page publiée à la NIC peut encore être lue ou écrite par matériel. Si un driver, une VM ou une NIC ne répond plus, le délai borne l’attente de l’application, non le droit de recycler la mémoire.

```text
expiration du drain
     -> erreur applicative et retrait du budget logique
     -> Quarantine de la page
     -> complétion OU reset de file/périphérique + invalidation IOMMU
     -> seulement alors Free
```

Le reset de Portal/NIC est exceptionnel et peut rompre d’autres flux de la même interface. C’est un choix conscient : sûreté mémoire avant disponibilité locale.

### 9.2 Phoenix

V4 peut conserver un `TcpStateStore` pour diagnostic, comptage ou décisions de nettoyage. Il ne doit pas restaurer une session TCP distante comme si la machine n’avait pas redémarré.

| Défaillance | Comportement convergé |
|---|---|
| Engine | les sockets touchées reçoivent `ECONNRESET`/`EPIPE`; Portal récupère les leases |
| Portal/NIC | Engine cesse l’émission ; file/domaine mis au repos avant réemploi |
| Arbiter | `AuthorityFrozen`, nouveaux opens refusés ; Phoenix republie une époque cohérente |
| désaccord Kernel A/B | `Restricted`; récupération Console locale seulement |
| ancien Brin | génération invalide ; aucun envoi nouveau admis |

## 10. Administration, observations et audit

### 10.1 Administration sans réseau omnipotent

La Console traduit des tâches humaines en Pactes et Enveloppes :

```text
Service : inventaire
Cercle : production
Écoute : TCP 8443
Clients : groupe employés
Budget : 20 Mbit/s, 2 000 pps, 100 connexions
Internet : interdit
Audit : métadonnées et refus
```

Une Enveloppe ne peut qu’être atténuée. Un administrateur ne peut pas se donner un Cercle, une durée, une priorité ou un droit absent de son autorité. La Couronne hors ligne crée les racines et les récupérations, pas les règles quotidiennes.

### 10.2 ExoLedger et Lentilles

Les actes irréversibles — publication, révocation, divergence A/B, échec IOMMU, reset matériel et récupération — utilisent ExoLedger/P0 suivant leur criticité. Ils ne reposent pas sur un nouveau journal réseau.

Les événements fréquents sont des métriques best-effort dans un anneau borné ; s’il est plein, les événements de télémétrie sont comptés puis écrasés, sans bloquer le chemin de données. Une mutation de politique qui exige un audit durable échoue si ExoLedger ne peut pas l’enregistrer.

```text
exonet status
exonet explain <fd|request>
exonet trace <cercle>
exonet policy diff <ancien..nouveau>
```

`exonet explain` doit pouvoir donner Sceau, Pacte, Brin, génération, lease, état de pool et cause d’erreur, sans exposer le contenu applicatif à une Lentille non autorisée.

## 11. Comparaison : ce qui est préservé et ce qui est amélioré

| Axe | V4 seule | ExoNet v1.2 seul sans V4 | Convergence retenue |
|---|---|---|---|
| compatibilité sockets | forte | à reconstruire | forte, ABI gelée |
| IPC inter-services | concret | abstrait | IPC V4 conservé pour contrôle |
| gros transferts | limité par inline/ownership | `NetGrant` prévu | NetGrant spécialisé, pas SHM globale |
| DMA/IOMMU | Engine propriétaire, bypass actuel | Portal défini | Portal réel, DMA-0 préalable |
| capacités | non structurées | forte | insertion sous l’ABI existante |
| administration | absente | complète | progressive après transport stable |
| Phoenix TCP | V4 peut tenter de sérialiser | rupture honnête | diagnostic conservé, TCP non ressuscité |
| débogage | logs V4 techniques | Lentilles prévues | `exonet explain` + ExoLedger |
| risque | perf/sûreté matérielle | réécriture théorique massive | migration mesurable, une frontière à la fois |

## 12. Plan de migration sans régression

### Phase C0 — figer et mesurer V4

- ne modifier aucune ABI sockets ni format `NetMsg`/`NetReply` V1 ;
- exécuter les tests actuels ;
- tracer le chemin application→bridge→Engine→driver ;
- ajouter `exonet explain` en lecture seule ;
- relever pool, files, appels IPC, pps, CPU et pertes avec QEMU/TAP.

**Porte C0 :** les tests V4 restent verts et tout refus connu possède une cause observable.

### Phase C1 — Portal sous propriété correcte

- extraire l’allocation de pool et des Virtqueues de `network_server` vers `virtio_net` ;
- remplacer `DriverInitMsg` IOVA par `PortalReady` à capacité ;
- attacher la NIC au domaine Portal et supprimer le bypass du chemin cible ;
- conserver temporairement l’adaptateur `smoltcp`/SocketTable V4 au-dessus du nouveau Portal.

Il n’existe qu’un seul pilote actif par NIC. Le basculement ancien/nouveau se fait au boot ou après arrêt contrôlé de l’interface, jamais en faisant conduire deux drivers en parallèle.

**Porte C1 :** DMA-0, plus tests d’absence de double propriété et de reset/quiescence.

### Phase C2 — Autorité en observation

- ajouter les types CapToken réseau et le `NetAuthorityTable` ;
- déployer Arbiter avec Pactes `PinnedAddress` ;
- calculer les décisions ExoNet en *shadow* sur un Cercle test ;
- le chemin V4 actuel reste seul à agir ; les divergences sont journalisées, pas appliquées.

**Porte C2 :** aucune divergence inexpliquée pour le jeu de scénarios de test, saturation des pools et révocation simulée.

### Phase C3 — Enforcement contrôlé

- matérieliser `NetworkLease` et Brin à l’ouverture ;
- passer un Cercle test en enforcement ;
- valider `fork`, `exec`, transfert de FD, `AuthorityFrozen` et erreurs POSIX ;
- introduire Console/Lens et l’audit de mutation ExoLedger.

**Porte C3 :** chaque ouverture est associée à un Brin ; aucun open après révocation ou perte d’Arbiter.

### Phase C4 — Données longues et QoS

- introduire `NetGrant` application↔Engine puis `DmaLease` Engine↔Portal ;
- garder une copie unique, pas de DMA direct d’application ;
- mesurer B2 avant d’envisager un `DataRing` ;
- appliquer les classes QoS, réserves de contrôle et politiques DNS `NameBoundTls`.

**Porte C4 :** aucun buffer publié n’est réutilisé sans quiescence ; débit et p99 sont publiés avec la configuration QEMU/matériel.

### Phase C5 — Phoenix et déploiement par Cercle

- injecter les pannes Engine, Portal et Arbiter pendant trafic ;
- vérifier les générations, la quarantaine de pages et la reprise administrative ;
- déployer Cercle par Cercle, jamais tous les services à la fois ;
- optimiser DataRing/multi-queue uniquement si les profils le justifient.

**Porte C5 :** B4, B5 et B5bis passent ; la reprise ne donne pas de droit ni de page supplémentaire.

## 13. Tests obligatoires et non-régression

| Famille | Acquis V4 à conserver | Ajout convergence |
|---|---|---|
| transport | DHCP, routage, ICMP, socket handles | État de Portal, compatibilité ABI et version de protocole |
| mémoire | RX/TX sans double propriété/leak | états `DmaLease`, retrait mapping, quiescence IOMMU |
| politiques | n/a | Pacte/Lease/Brin, revocation, cache, `AuthorityFrozen` |
| POSIX | opérations socket existantes | `fork`, `exec`, FDs partagés ou invalides, erreurs atomiques |
| Phoenix | drain/serialize V4 | rupture TCP assumée, recovery A/B, ancien Brin refusé |
| administration | n/a | Enveloppe, Couronne, diff, ExoLedger, Lentilles |
| performance | aucun chiffre accepté sans mesure | B1/B2/B3 avec backend réseau et CPU publiés |

Le modèle de stress V4 devient la base de la spécification d’états. Les variables `rx_submitted`, `tx_inflight`, libérations de pool, sockets et Phoenix restent présentes ; il est étendu par `pact_generation`, `authority_epoch`, `lease_state`, `page_state` et `portal_state`.

## 14. Invariants de la pile convergée

| ID | Invariant |
|---|---|
| V4-1 | un buffer RX est possédé soit par Portal, soit par Engine, jamais par les deux |
| V4-2 | un TX soumis ne revient au pool qu’après complétion/quiescence |
| V4-3 | sockets et messages restent bornés, même sous saturation |
| X1 | aucun paquet applicatif sans Brin actif |
| X2 | toute lease est un descendant atténué d’un Pacte valide |
| X3 | tout changement de Pacte révoque les leases/Brins de sa génération |
| X4 | Portal n’a aucune autorité de politique ; Arbiter n’a aucun accès NIC/DMA |
| X5 | application n’accède jamais aux pages DMA Portal |
| X6 | une page publiée ne redevient pas libre sans quiescence DMA confirmée |
| X7 | un changement d’`authority_epoch` invalide tous les caches d’ouverture |
| X8 | une divergence A/B conduit à `Restricted`, jamais à un assouplissement |
| X9 | une Lentille ne révèle pas des données hors de son filtre |
| X10 | les classes de survie/contrôle gardent une réserve bornée sans famine du reste |

Les propriétés V4-1 à V4-3 sont non négociables pendant toute migration. Les invariants X1 à X10 s’ajoutent ; aucun n’autorise à affaiblir une propriété V4 existante.

## 15. Budget, risques et arbitrages

### 15.1 Coût évité par la convergence

Conserver ABI, IPC contrôle, smoltcp, SocketTable et tests V4 évite une seconde réécriture de transport. Une réécriture intégrale ajouterait une période longue sans connectivité ni diagnostic, sans résoudre plus vite la sécurité DMA.

La convergence ne rend pas le projet petit : DMA-0, `NetGrant`, politiques et récupération restent des travaux profonds. Elle réduit le risque en limitant le nombre de variables qui changent simultanément.

| Lot | Estimation de travail | Risque principal |
|---|---:|---|
| C0 mesure/diagnostic | 3–6 sem.-pers. | environnement QEMU/TAP insuffisant |
| C1 Portal/IOMMU | 8–16 sem.-pers. | détail VirtIO/IOMMU et récupération de device |
| C2 capacités/ombre | 8–14 sem.-pers. | sémantique de cache et de révocation |
| C3 enforcement/admin | 6–10 sem.-pers. | compatibilité POSIX/fork/FD |
| C4 grants/QoS/DNS | 12–20 sem.-pers. | états mémoire et pression IPC |
| C5 Phoenix/perf/déploiement | 10–16 sem.-pers. | panne matérielle et comportements non déterministes |

**Estimation totale : 47–82 semaines-personnes**, hors certification ou audit externe. C’est comparable à v1.2, mais une réécriture complète depuis zéro ajouterait une incertitude significative sans supprimer les portes DMA, capacités ou Phoenix.

### 15.2 Risques assumés

| Risque | Réponse | Déclencheur de stop |
|---|---|---|
| IOMMU difficile à intégrer | C1 isolé, tests avant politique | DMA-0 échoue |
| IPC limite petits paquets | mesurer B2, DataRing seulement après preuve | optimisation avant mesure |
| smoltcp monofile limite | profilage puis shards/multi-queue optionnels | réécriture sans goulot démontré |
| politique trop complexe | commencer `PinnedAddress` + Cercle test | divergence shadow inexpliquée |
| ROM/RAM cible trop faible | profil Petit, refus explicite à saturation | allocation dynamique cachée |
| Phoenix interfère avec réseau | pannes injectées et rupture TCP honnête | ancien Brin/page encore utilisable |

## 16. Décision finale

Le meilleur résultat ne consiste pas à choisir V4 *ou* ExoNet v1.2.

Il consiste à :

1. **préserver V4 là où il est réel et utile** : ABI, IPC de contrôle, smoltcp, SocketTable, tests, bornes ;
2. **remplacer V4 là où il ne peut pas devenir sûr par simple configuration** : possession DMA, IOVA brutes, bypass IOMMU, reprise TCP implicite ;
3. **ajouter ExoNet autour du moteur**, jamais comme une seconde pile ;
4. **modifier le noyau seulement par contrats réseau étroits** : capacité, lease, mapping temporaire, époque ;
5. **faire progresser le système par portes**, avec une seule source de paquets et une seule source de vérité à chaque étape.

La première action est C0 : documenter et mesurer exactement le chemin V4 actif, renforcer `exonet explain`, puis réussir DMA-0. À partir de là, la refonte est assez ambitieuse pour atteindre un vrai réseau à capacités, mais assez contenue pour ne pas transformer Exo-OS tout entier en chantier réseau.

## Références locales

- `docs/recast/EXOOS_NETWORK_MODULE_V4.md` — contrat et hypothèses V4.
- `servers/network_server/EXONET_V4_AUDIT.md` — mapping V4/TLA et invariants déjà couverts.
- `docs/EXONET_PLAN_CONCEPTION_REALISTE_V1_2.md` — contrat d’autorité, DMA, administration et vérification.
- `kernel/src/syscall/net_bridge.rs` et `kernel/src/syscall/table.rs` — ABI sockets active.
- `servers/network_server/src/buf_pool.rs`, `drivers/network/virtio_net/src/virtqueue.rs`, `kernel/src/memory/dma/core/mapping.rs` — allocation DMA et bypass actuel.
