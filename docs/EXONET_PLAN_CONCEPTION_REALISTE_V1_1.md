# ExoNet v1.1 — conception réaliste, contrats d’autorité et plan de validation

**Statut :** spécification de conception révisée ; elle ne décrit pas une fonctionnalité déjà implémentée.
**Objet :** réseau local et Internet, applications, processus, services Ring 1, administration, observabilité et reprise ExoPhoenix.
**Règle de conception :** aucune promesse de sécurité ou de performance ne vaut sans son mécanisme, son invariant et son test.

Cette version reprend la v1, intègre les retours critiques et corrige un point important découvert dans le code actuel : les chemins réseau demandent encore des allocations DMA avec le drapeau `BYPASS_IOMMU`. Le réseau à capacités ne peut pas être déclaré prêt pour une utilisation de sécurité tant que ce chemin n’est pas remplacé par un domaine IOMMU effectivement attaché et vérifié.

---

## 1. Niveaux de vérité

- **[OBSERVÉ]** : présent dans le checkout ou validé localement.
- **[DÉCISION]** : contrat proposé à implémenter.
- **[ESTIMATION]** : calcul, budget ou objectif ; jamais un résultat déjà acquis.
- **[PORTE]** : test mesurable qui conditionne la suite.
- **[HORS PÉRIMÈTRE]** : ce qu’ExoNet ne prétend pas résoudre dans cette version.

Les nombres de latence proposés par certains retours, par exemple « 5 à 20 µs par `connect()` » ou « 30 fois plus lent que Linux », ne sont pas repris : aucun banc de mesure Exo-OS comparable ne les établit. Ils deviennent des hypothèses à tester, non des faits de conception.

## 2. Décision architecturale

ExoNet est une couche d’autorité réseau à capacités autour d’Ethernet, IPv4, TCP, UDP, ICMP et DNS. Il ne remplace pas Internet par un protocole maison. Il répond aux questions suivantes avant ou pendant un échange :

```text
Quel Sceau demande l’action ?
Quel Pacte précis l’autorise ?
Quel Brin éphémère représente cette ouverture ?
Quelle quantité de réseau, de pages et de file peut-il consommer ?
Qui peut l’observer, le modifier ou le révoquer ?
```

L’expérience utilisateur reste simple : l’application utilise `socket`, `connect`, `send`, `recv`, `bind`, `listen` et `accept` via ExoFS. L’autorisation lui est déléguée avant l’ouverture, sous la forme d’une capacité ; elle n’a ni règles pare-feu globales à écrire ni privilège réseau ambiant à acquérir.

### 2.1 Frontière de responsabilité réelle

ExoNet contrôle le trafic qui traverse ses Portals, ses processus et ses Cercles. Il **ne peut pas** empêcher deux appareils physiques non administrés d’échanger ou de se faire du DHCP/ARP entre eux sur le même commutateur : cela relève du commutateur, du routeur ou d’un bridge qui est effectivement sous son contrôle. Cette limite doit rester visible dans toute interface d’administration.

Les propriétés de sûreté visées sont donc locales à Exo-OS et à ses frontières réseau administrées, pas une promesse de purifier un LAN entier.

## 3. Services Ring 1 et profils de déploiement

| Service | Mission et autorité | Ne détient jamais |
|---|---|---|
| **Portal** (`virtio_net`) | NIC, IRQ, IOMMU, pages DMA, files VirtIO, réserve de contrôle mécanique | Pactes, Enveloppes, décisions sur les utilisateurs |
| **Engine** (`network_server`) | pile IPv4/TCP/UDP, sockets, mise en file, quotas, copie contrôlée, liaison FD→Brin | racine de gouvernance, DMA arbitraire, droit d’élargir une politique |
| **Arbiter** (`net_policy_server`) | Pactes, Brins, générations, révocation, cache d’admission émis, DNS autorisé | NIC, IRQ, domaine IOMMU, accès au contenu réseau |
| **Lens** (`net_observe_server`) | événements filtrés, compteurs, explications de décision | contenu applicatif brut, mutation de politique |
| **Console** (`net_admin_server`) | formulaires, diff, approbations, Enveloppes, export/import signé | descripteurs de paquets, DMA, ouverture de socket hors Pacte |

Les cinq services sont les frontières logiques cibles, pas l’obligation de charger cinq gros démons dès le premier boot.

| Profil | Services résidents | Usage |
|---|---|---|
| **Minimal v1** | Portal, Engine, Arbiter ; Console déclenchée à la demande ; Lens réduit à un anneau d’événements | QEMU, matériel contraint, mise au point |
| **Exploitation** | les cinq services séparés | poste administré, serveur, audit continu |
| **Jamais accepté** | Engine avec une racine de politique, Console avec DMA, Portal avec un catalogue de droits | mélange d’autorité, même si cela semble plus rapide |

Les instantanés de politique et les anneaux d’événements sont mappés en lecture seule là où c’est possible ; ils ne sont pas dupliqués par processus sans nécessité. La consommation mémoire de chaque profil est une métrique obligatoire de la phase 0, particulièrement pour une cible embarquée.

## 4. Point de départ du dépôt et précondition IOMMU

### 4.1 État observé

**[OBSERVÉ]** Le dépôt contient déjà :

- `network_server` avec `smoltcp`, DHCP, routage, ICMP, TCP/UDP, une table de 64 sockets, et la liaison au pilote ;
- `virtio_net` avec la gestion de VirtIO et un suivi RX de 256 buffers ;
- `net_bridge` avec une charge utile inline maximale de 128 octets ;
- un pool de 256 pages RX et 256 pages TX de 4 Kio, soit 1 Mio par direction ;
- un `CapToken` typé, générationnel et ABI de 24 octets ;
- des tests locaux validés pour DHCP, routage, ICMP, handles de sockets et le modèle `exonet_stress`.

Ces tests établissent des comportements unitaires et de modèle. Ils ne mesurent ni un débit E2E QEMU, ni une propriété IOMMU matérielle, ni une résurrection Phoenix pendant trafic.

### 4.2 Blocage de sûreté à traiter avant BufferGrant

**[OBSERVÉ]** `network_server` et `virtio_net` utilisent actuellement `DMA_MAP_FLAGS_BYPASS_IOMMU`, et le code du pilote indique que les adresses de files VirtIO restent physiques en attendant le câblage de contextes IOMMU traduits.

**[DÉCISION]** Le modèle v1.1 inverse la possession DMA :

1. le Portal alloue les pages et les files VirtIO dans **son** domaine IOMMU traduit ;
2. seul ce domaine rend ces pages visibles au périphérique ;
3. le noyau peut mapper temporairement une page du Portal dans l’Engine, mais ne transfère jamais le domaine IOMMU au moteur ;
4. une application ne mappe jamais une page DMA du Portal ;
5. l’Engine n’envoie au Portal que des capacités de prêt et des indices bornés, jamais une adresse physique choisie par lui.

**[PORTE DMA-0]** avant tout test de performance ou annonce de confinement matériel :

- aucun chemin Portal/VirtIO utilisé en production ne porte `BYPASS_IOMMU` ;
- le domaine de la NIC ne mappe que le pool et les files du Portal ;
- un accès DMA hors pool est injecté et bloqué ;
- la destruction d’un prêt prouve le retrait de la visibilité DMA avant réutilisation de la page.

Cette porte ne prétend pas que le code actuel est vulnérable dans toutes les configurations ; elle constate une incompatibilité à résoudre entre une promesse de confinement IOMMU et le chemin de contournement visible dans le code.

## 5. Amorçage : Couronne, Sceaux et absence d’autorité ambiante

### 5.1 Chaîne de démarrage

Le premier droit réseau ne peut pas surgir d’un PID ou du nom d’un binaire. La chaîne proposée est :

```text
Couronne hors ligne
       ↓ signe
Manifeste de démarrage immuable
       ↓ hash/validation au boot
Kernel B après SECURITY_READY
       ↓ délégation bornée
Arbiter initial
       ↓ publication contrôlée
Pactes, Enveloppes et Sceaux opérationnels
```

Le manifeste signé contient l’identité de la Couronne publique, les Sceaux des services de démarrage, le premier Arbiter, les Cercles initiaux, l’empreinte de la politique de départ et une politique de récupération locale. Kernel B vérifie la continuité du manifeste et ne libère au premier Arbiter qu’une capacité de publication bornée par ce manifeste.

En cas de manifeste invalide, de divergence ou d’absence de politique, le système démarre dans `RestrictedRecovery` : Console locale autorisée avec une Enveloppe de secours ; aucune ouverture Internet ou LAN applicative par défaut.

### 5.2 Enrôlement ultérieur

Un nouvel utilisateur, service ou appareil reçoit un Sceau seulement à partir d’une Enveloppe valide, d’une approbation enregistrée et d’un événement de journal. Une adresse MAC, une IP DHCP, un PID ou un nom de processus peut être un attribut de transport ; aucun n’est une identité suffisante.

La Couronne ne s’utilise pas pour l’exploitation quotidienne. Elle crée des Enveloppes limitées par Cercle, durée, opérations et plafonds. Une personne appelée « administrateur » ou « chef d’entreprise » ne peut pas dépasser son Enveloppe ; un titre humain n’a aucune sémantique dans le noyau.

## 6. Objets d’autorité et compatibilité CapToken

| Objet | Durée | Rôle |
|---|---|---|
| **Sceau** | persistant | identité d’un utilisateur, service, appareil ou administrateur |
| **Pacte** | durable, versionné | règle d’accès, de destination, de priorité et de quota |
| **Brin** | éphémère | autorisation d’une session/socket concret |
| **Lentille** | déléguée, révocable | visibilité filtrée sur événements et métriques |
| **Cercle** | durable | frontière logique entre groupes et services |
| **Enveloppe** | durable, bornée | capacité d’administration atténuée |
| **Couronne** | racine | autorité de gouvernance, normalement hors ligne |

**[DÉCISION]** ExoNet étend le système `CapToken` existant au lieu d’en construire un second. Les types proposés sont `NetworkPact`, `NetworkSession`, `ObservationScope` et, si nécessaire, `DmaLease`. Leur ajout conserve la taille ABI de 24 octets : seul le tag de type est étendu ; toute incompatibilité ABI impose une version explicite du protocole, jamais un changement silencieux.

Les droits standards reçoivent une sémantique par objet :

| Objet | `READ` | `EXEC` / `WRITE` | `DELEGATE` | `REVOKE` |
|---|---|---|---|---|
| Pacte | consulter les champs non secrets | ouvrir un Brin dans son périmètre | seulement un sous-ensemble | invalider descendants |
| Brin | recevoir dans les limites | émettre/agir dans les limites | interdit par défaut | fermer le Brin |
| Lentille | lire les événements filtrés | aucun | filtre plus étroit seulement | retirer la visibilité |
| DmaLease | aucun accès direct à l’application | accès Engine temporaire et borné | interdit | termine le prêt |

Un Pacte est immuable après publication. Modifier une destination, un débit, une classe, un Cercle ou une exigence de chiffrement produit une nouvelle génération.

## 7. Admission : Arbiter source de vérité, Engine sans goulot d’ouverture

### 7.1 Pacte et Brin

Un Pacte contient au minimum :

```text
Pacte {
  pact_id, policy_epoch, generation, émetteur, sujet_ou_groupe,
  direction: sortie | entrée | interne,
  destination_profile, transport, ports, cercles,
  classe_max, bytes_per_s, packets_per_s, burst_bytes, burst_packets,
  pages_max, connexions_max, expiration,
  règle_dns, exigence_tls, journalisation, règle_de_reprise
}
```

Un Brin lie un Sceau, une génération de Pacte, une session, les endpoints réellement retenus, les quotas et un état :

```text
Closed -> Opening -> Active -> Draining -> Closed
                     |            |
                     v            v
                 Restricted ----> Revoked
```

### 7.2 Cache d’admission sûr et borné

Un aller-retour vers l’Arbiter pour chaque `connect()` serait coûteux et ferait de ce service un point chaud. L’Engine maintient donc un cache de décisions, mais **pas** une deuxième base de politiques :

```text
clé = (sceau, destination_profile, transport, port, policy_epoch)
valeur = AdmissionLease signée/typée, limites compilées, expiration courte
```

L’Arbiter émet l’`AdmissionLease` sous forme d’autorité atténuée. Lors d’un hit, l’Engine vérifie localement : validité du token, Sceau, requête exacte, `policy_epoch`, expiration, quota de connexions et `DenySet`. Il peut alors demander au noyau de matérialiser un Brin descendant. Le cache ne peut ni ajouter une destination, ni élargir une classe, ni survivre à une révocation de génération.

Le noyau invalide le parent lorsque l’Arbiter révoque le Pacte ; `DenySet` est une structure compacte consultée par l’Engine sur chaque admission et chaque point de reprise de lot. Ce double mécanisme évite la fenêtre d’un cache devenu ancien.

L’Arbiter émet un battement de vie. Si l’Engine ne le reçoit plus pendant la fenêtre configurée, il passe en `AuthorityFrozen` :

- aucune nouvelle ouverture, y compris via cache ; retour `EAGAIN` ou erreur ExoNet documentée ;
- les Brins déjà actifs continuent seulement selon leur échéance existante ;
- une révocation d’urgence reste possible via une capacité de coupure distincte détenue par la Couronne ;
- ExoPhoenix relance l’Arbiter, recharge l’instantané de politique, publie une nouvelle époque et débloque ensuite les ouvertures.

**[PORTE A-1]** une panne de l’Arbiter pendant `connect()` (B5bis) ne doit ni bloquer indéfiniment, ni créer de FD partiel, ni accepter une ouverture sur cache après passage `AuthorityFrozen`.

### 7.3 Publication A/B sans arrêt du chemin de données

Une publication normale est en deux temps :

1. l’Arbiter produit `PolicySnapshot(n, root_digest)` immuable ;
2. Kernel A et Kernel B reçoivent le résumé hors chemin de paquets ;
3. après accusés cohérents, l’Engine rend `n` actif pour les nouvelles admissions ;
4. les assouplissements sont refusés tant que les deux accusés ne sont pas présents.

Une révocation est différente : `DenySet`/génération est appliqué immédiatement sur l’Engine, puis répliqué vers Kernel B. Une divergence n’autorise jamais davantage : elle mène à `Restricted` ou `AuthorityFrozen` suivant sa nature.

**Sortie de `Restricted` :** uniquement une Console locale de récupération, avec Enveloppe Couronne/secours ; chargement d’un instantané dont séquence et empreinte concordent ; accusé A/B ; journal de cause ; validation explicite avant de rouvrir de nouvelles admissions. Le réseau de production ne sert pas à « réparer » son propre désaccord.

## 8. DNS, CDN et destination Internet

Une politique d’adresse IP seule est adaptée aux services internes stables, mais insuffisante pour l’Internet moderne. À l’inverse, une autorisation par AS ou par grand préfixe est trop large : changement BGP, multi-hébergement et réutilisation d’adresses n’en font pas une identité fiable.

ExoNet v1.1 propose deux profils explicites :

| Profil | Usage | Règle |
|---|---|---|
| `PinnedAddress` | service LAN/infra stable | CIDR et ports exacts, éventuellement adresse unique |
| `NameBound` | API/CDN Internet | FQDN exact ou wildcard explicitement borné, résolveur autorisé, ports, TTL, et TLS avec nom serveur requis |

Pour `NameBound`, l’Engine possède un cache DNS contrôlé par `policy_epoch` ; tout cache DNS interne de pile activé sans cette clé doit être désactivé ou contourné. La clé de cache est :

```text
(policy_epoch, pact_generation, fqdn, resolver_identity, transport, port)
```

Le TTL est plafonné par le Pacte. À l’ouverture, l’ensemble résolu est gelé dans le Brin :

- une connexion déjà établie ne migre pas silencieusement quand le TTL expire ;
- une nouvelle ouverture résout à nouveau selon la politique courante ;
- une réponse interdite fait échouer l’ouverture ;
- le changement de génération purge le cache et invalide les AdmissionLeases ;
- `NameBound` exige une validation TLS applicative du nom attendu ; sans TLS, le Pacte doit être `PinnedAddress` ou porter une exception explicitement acceptée et journalisée.

Ainsi, un CDN ne force pas une recompilation de Pacte à chaque TTL, mais ne donne pas non plus à l’Engine une autorisation vague pour un AS entier.

## 9. Données et DMA : contrat Engine ↔ Portal

### 9.1 Plans séparés

| Plan | Support | Sert à |
|---|---|---|
| contrôle | IPC/SPSC existant, messages courts | open, close, révocation, publication, réservation, statistiques |
| données v1a | `BufferGrant` en lots bornés | valider la propriété et sortir de la limite inline de 128 o |
| données v1b, seulement si mesure nécessaire | anneau SPSC de descripteurs et doorbell amortie | petits paquets et charges fréquentes sans RPC synchrone par envoi |

Le chemin de données ne consulte jamais l’Arbiter par paquet. Il ne fait qu’une vérification de Brin, de génération, de `DenySet`, de quotas, de bornes et d’état de file.

### 9.2 Propriété des pages

Le **Portal** est propriétaire physique des pages DMA et des descripteurs. L’Engine est propriétaire logique de la décision de les utiliser pour un Brin, jamais de leur domaine IOMMU.

```text
application -- MemoryRegion, non-DMA --> Engine
                                       |
                                       | copie contrôlée
                                       v
                              DmaLease temporaire
                                       |
                                       v
Portal (pages DMA + IOMMU + VirtIO) -- NIC
```

L’application ne voit pas la page DMA. L’Engine peut lire une région de l’application ou y écrire dans les limites de `MemoryRegion`, puis copier vers/depuis une page Portal prêtée. Il n’existe donc ni transfert de domaine IOMMU entre services, ni DMA direct application→NIC en v1.

### 9.3 Machine d’états de page

Chaque page a un `page_id`, une génération de prêt et un propriétaire de transition. Une transition est atomique et journalisable.

| État | Propriétaire d’accès CPU | Périphérique DMA | Transition autorisée |
|---|---|---|---|
| `Free` | Portal | non publiée | `ReserveTx`, `PublishRx` |
| `ReservedTx` | Portal | non publiée | `MapEngineTx`, `Cancel` |
| `MappedEngineTx` | Engine RW temporaire | non publiée | `ReadyTx`, `Cancel` |
| `ReadyTx` | aucun Engine | non publiée | `PublishTx` |
| `PublishedTx` | Portal | lecture matériel possible | `CompletedTx`, `Quarantine` |
| `PublishedRx` | Portal | écriture matériel possible | `CompletedRx`, `Quarantine` |
| `MappedEngineRx` | Engine RO temporaire | non publiée | `ReleaseRx` |
| `Quarantine` | Portal/Kernel de récupération | accès normal interdit | `DmaQuiesced -> Free` |

Les chemins sont :

```text
TX: Free -> ReservedTx -> MappedEngineTx -> ReadyTx -> PublishedTx -> CompletedTx -> Free
RX: Free -> PublishedRx -> CompletedRx -> MappedEngineRx -> Free
```

Transitions interdites :

- `MappedEngine* -> Published*` sans retrait confirmé du mapping Engine ;
- `Published* -> Free` sans complétion VirtIO ou mise au repos DMA prouvée ;
- deux `DmaLease` sur la même page ;
- mapping applicatif d’une page Portal ;
- libération de page à partir d’un simple timeout.

Le Portal réserve `N` pages pour un Brin par IPC ; l’Engine reçoit des `DmaLease` bornés ; il les remplit ou les lit ; le Portal valide l’indice, retire le mapping Engine, publie le descripteur VirtIO et signale la complétion via SPSC/IRQ. Aucun tiers ne peut mapper les pages.

### 9.4 Révocation, drain et matériel bloqué

La révocation est en deux phases :

1. **logique** : nouvelle admission et nouveau `ReadyTx` refusés dès la génération révoquée ;
2. **physique** : le Portal attend complétion, puis restitue les pages. Si elle n’arrive pas, les pages passent `Quarantine`, la file ou le périphérique est réinitialisé et le domaine DMA est mis au repos avant toute réutilisation.

**Invariant N11 — libération sûre et bornée.** Un Brin `Draining` a un délai logique maximum par classe, configuré et mesuré. Valeurs initiales de test : 100 ms pour classes 0–1, 500 ms pour interactif, 2 s pour service, 5 s pour arrière-plan/quarantaine. À l’échéance, l’application reçoit l’échec ; le Brin cesse de retenir le quota logique. **Mais une page n’est jamais rendue au pool avant preuve de quiescence DMA.** Un périphérique suspendu peut donc provoquer un reset de file ou de Portal : la disponibilité cède à la sûreté mémoire.

Cette distinction corrige l’idée dangereuse « timeout = page libre ». Le timeout borne l’attente de l’utilisateur ; seul un accusé de complétion ou un retrait DMA confirmé autorise la réutilisation physique.

### 9.5 Éviter l’impôt IPC sans introduire une course de mémoire

v1a commence par des lots de 1 à 16 pages et mesure B2. Si plus de 25 % du temps CPU de B2 est imputable au contrôle IPC ou si les petits messages ratent l’objectif de latence relatif, v1b introduit un `DataRing` SPSC **par paire application→Engine**, pas un anneau par Brin.

Un descripteur contient `{brin_index, window_id, offset, len, seq, flags}`. La fenêtre `MemoryRegion` est enregistrée une fois, bornée, et son contenu est copié par l’Engine au moment de la consommation — sémantique compatible avec un tampon d’envoi dont l’application ne doit pas modifier la zone sans synchronisation. Les doorbells sont groupés. La saturation retourne `EAGAIN`; elle ne déclenche ni allocation dynamique ni élargissement de quota.

## 10. QoS, trafic de contrôle et quarantaine

### 10.1 Classes et unités

Chaque Pacte porte **octets/s, paquets/s, rafale en octets, rafale en paquets, pages maximum et connexions maximum**. Une délégation ne peut qu’abaisser ces valeurs et sa classe.

| Classe | Finalité | Traitement |
|---:|---|---|
| 0 survie | battement Phoenix et récupération minimale | réserve minuscule, plafonnée, non applicative |
| 1 contrôle | administration active, DHCP/ARP nécessaires au host | priorité bornée, réserve de buffers |
| 2 interactif | shell, UI, RPC courts | objectif faible p99 |
| 3 service | API métier | partage équitable par Pacte |
| 4 arrière-plan | sauvegarde, mises à jour | surplus, abandonnable |
| 5 quarantaine | diagnostic d’un contexte non approuvé | plafond dur et visibilité renforcée |

L’Engine utilise un seau de jetons par Pacte puis un round-robin à déficit par classe 2–5. Les classes 0–1 reçoivent une priorité stricte **et** un plafond en octets/s et paquets/s ; elles ne deviennent pas une voie de famine.

### 10.2 Réserve de contrôle portable

Une virtqueue séparée n’est pas toujours offerte par le matériel ou négociée. v1 réserve donc dans le Portal un nombre fixe de descripteurs et de buffers RX/TX à destination des classes 0–1, invisibles aux files applicatives. Une virtqueue dédiée est une optimisation de phase 6 seulement si la fonctionnalité VirtIO est réellement négociée.

Le Portal peut appliquer un garde mécanique L2 à table bornée : plafond global et par MAC source pour ARP/DHCP entrant, avant de livrer à l’Engine. Il ne décide pas si l’utilisateur a le droit d’ouvrir une application ; il évite seulement qu’une tempête de contrôle épuise les réserves.

Pour ce host, seul le Sceau `net_bootstrap` reçoit le Pacte DHCP client. Un contexte/guest Exo-OS en quarantaine ne reçoit pas de DHCP automatique ; il utilise une adresse préattribuée, un relay administré ou reste sans réseau. Pour un appareil externe sur le même LAN, ExoNet ne prétend pas contrôler son DHCP tant qu’il ne traverse pas un Portal/bridge administré.

## 11. Sockets, `fork`, erreurs et reprise applicative

### 11.1 Héritage sans amplification

Une `NetworkSession` est un objet référencé, non une permission copiée en secret avec un FD. Lors d’un `fork()` :

- le noyau peut créer une seconde référence vers **le même** Brin seulement si le Pacte autorise `inherit` et que le fils porte le même Sceau de famille ;
- les quotas et la révocation restent communs : le fils ne reçoit pas un Brin supplémentaire ;
- sinon, le FD réseau du fils est marqué non utilisable et toute opération renvoie une erreur documentée ;
- `exec()` réévalue le Sceau du nouvel exécutable ; les sessions non autorisées sont fermées ou révoquées ;
- transmettre un FD à un autre Sceau exige une délégation explicite et atténuée, jamais un simple `SCM_RIGHTS` implicite.

Cette règle préserve l’usage POSIX raisonnable pour un même programme sans créer de chemin d’escalade par héritage.

### 11.2 Matrice d’erreurs utile au développeur

| Cas | Résultat |
|---|---|
| Pacte absent ou destination hors profil | `EACCES`/code ExoNet explicatif |
| cache invalide ou Arbiter indisponible | `EAGAIN`, aucun FD partiel |
| quota de débit ou de paquets | attente, `EAGAIN` avec `MSG_DONTWAIT`, ou délai déclaré |
| pool Portal saturé | `ENOBUFS` ou `EAGAIN`, compteur et cause Lens |
| envoi partiel | longueur réellement acceptée, jamais succès fictif |
| Brin révoqué | nouvelles écritures refusées ; fermeture selon protocole |
| Engine/Portal/Phoenix redémarré | `ECONNRESET` ou `EPIPE`; reconnexion applicative |
| DNS `NameBound` invalide | échec d’ouverture, aucune bascule silencieuse |

TCP n’est pas « ressuscité » localement. La norme TCP définit une machine d’états et des cas de reset qui suppriment l’état de contrôle de connexion ; ExoNet signale proprement la rupture et laisse le protocole applicatif décider d’une reprise idempotente ([RFC 9293](https://www.rfc-editor.org/rfc/rfc9293.html)).

## 12. Résilience, Phoenix et erreurs de composant

| Défaillance | Comportement v1.1 |
|---|---|
| Arbiter | `AuthorityFrozen`, nouveaux opens refusés, Brins existants bornés ; Phoenix relance et republie une époque cohérente |
| Engine | sockets signalées rompues ; Portal récupère/quarantine les leases ; Engine repart puis ré-arbitre |
| Portal/NIC | Engine cesse les envois ; reset de file/périphérique et retrait DMA avant réemploi ; sockets selon impact rompues |
| Kernel A/B désaccord | `Restricted`, aucun assouplissement ; récupération locale Couronne seulement |
| congestion | admission/queues limitent, les classes faibles sont rejetées avant que les réserves ne disparaissent |

La capacité de coupure d’urgence de la Couronne peut fermer les Brins d’un Cercle ou d’une interface même si l’Arbiter est indisponible. Elle ne confère aucun droit d’ouverture ou de modification de politique : urgence et administration ordinaire sont séparées.

## 13. Observabilité avant déploiement, pas après l’incident

La complexité des capacités est acceptable uniquement si le refus devient explicable. Avant l’activation obligatoire des Pactes, une Lentille de développement fournit :

```text
exonet status                 # interfaces, époques, pool, queues, classes
exonet explain <fd|request>   # Sceau, Pacte, Brin, génération, étape qui refuse
exonet trace <cercle>         # événements filtrés sans contenu applicatif
exonet policy diff <n..n+1>   # changement signé et approbations
```

Un événement de décision contient `request_id, sceau, pact_id, génération, brin_id, étape, résultat, errno, queue, page_state, policy_epoch`. Il ne contient ni charge utile, ni secret de l’application. La Lentille filtre ces événements avant exposition.

**[PORTE O-1]** tout refus dans B1 à B6 doit être relié à un code de cause stable et à l’objet responsable. Une architecture à capacités sans `explain` serait plus sûre sur le papier mais inutilisable en exploitation.

## 14. Invariants et vérification formelle progressive

| ID | Invariant |
|---|---|
| N1 | aucune émission/réception applicative sans Brin actif |
| N2 | tout Brin lie Sceau, Pacte, génération et limites valides |
| N3 | toute délégation est un sous-ensemble de son parent |
| N4 | une révocation interdit les nouvelles opérations descendantes |
| N5 | `fork`, `exec` et transfert de FD n’amplifient pas l’autorité |
| N6 | une page Portal n’a qu’un état et un prêt actif autorisés |
| N7 | Portal n’a pas de politique ; Arbiter n’a ni IRQ ni DMA |
| N8 | une divergence A/B devient plus restrictive, jamais plus permissive |
| N9 | une Lentille ne lit rien hors de son filtre |
| N10 | la QoS protège les réserves sans famine par les classes hautes |
| N11 | le timeout borne l’attente logique ; la page n’est libérée qu’après quiescence DMA |

La preuve complète de N1–N11 n’est pas une condition réaliste de la première livraison. Le découpage est :

| Version | Obligation |
|---|---|
| **v1** | modèle TLA+ et extension de preuves capacités pour N1–N5 ; tests de propriété/fuzz ciblés pour N6–N11 |
| **v1.5** | modèle DMA/Portal, réinitialisation, Phoenix et `DenySet` pour N6, N8, N11 |
| **v2** | équité sous charge, confidentialité Lens et multi-queue dans un modèle plus complet |

Cette réduction de périmètre n’affaiblit pas N6–N11 : elle interdit simplement de les appeler « formellement prouvés » avant que leurs modèles existent.

## 15. Benchmarks, métriques et estimations

### 15.1 Calculs utiles dès aujourd’hui

Le canal inline de 128 octets exigerait, sans aucun surcoût, 976 563 soumissions/s pour 1 Gbit/s utile et 9 765 625/s pour 10 Gbit/s. Une page de 4 Kio réduit ces valeurs à 30 518/s et 305 176/s ; 16 pages à 1 908/s et 19 074/s.

Les 256 pages de 4 Kio par direction actuelles offrent une réserve théorique de 1 Mio : 8,39 ms à 1 Gbit/s, 3,36 ms à 2,5 Gbit/s et 0,84 ms à 10 Gbit/s. Ce sont des calculs de capacité, pas la latence ou le débit du système.

Les pools sont par interface, pas par Brin. Passer de 64 à 1 000 connexions exige un budget explicite de tables de sockets, brins, fenêtres et files ; le système doit retourner `ENOBUFS`/`EAGAIN` plutôt que sur-allouer. Aucune cible « 1 000 connexions » n’est annoncée avant ce test.

### 15.2 Protocole de mesure

Chaque résultat publie révision Git, compilation, CPU/RAM, topologie, version QEMU, backend TAP/bridge, MTU, nombre de files, affinité, durée, taille de messages, flux, débit utile, paquets/s, p50/p95/p99, CPU, drops, erreurs d’autorité et mémoire par profil.

Le NAT utilisateur QEMU suffit à vérifier une connectivité simple ; les conclusions de débit utilisent TAP/bridge contrôlé. Les comparaisons Linux utilisent le même hôte, MTU, backend, protocole, tailles et nombre de flux ; sinon elles sont informationnelles, pas un verdict.

| ID | Scénario |
|---|---|
| B0 | tests ABI, CapToken, états de page et modèle N1–N5 |
| B1 | TCP/UDP écho, fermeture, refus, erreurs POSIX et `exonet explain` |
| B2 | 64/512/1400 octets, 1 puis 64 flux ; inline puis lots `BufferGrant` |
| B3 | quatre Pactes des classes 1–5 ; débit, perte, fairness et p99 interactif |
| B4 | révocation pendant B2/B3 ; aucune admission après `DenySet`, N11 sous lien arrêté |
| B5 | panne Engine, Portal, Kernel A ; quiescence DMA et reconnexion |
| B5bis | panne Arbiter pendant open et pendant trafic existant |
| B6 | adversaire : token périmé/forgé, FD hérité, page doublement prêtée, DNS hors profil, tempête ARP/DHCP |

### 15.3 Objectifs de travail, à confirmer par B2–B6

| Étape | Objectif | Interprétation correcte |
|---|---|---|
| inline actuel | fonctionnalité et instrumentation, pas débit | la limite 128 o rend un débit élevé structurellement peu crédible |
| lots BufferGrant QEMU/TAP | 25 000 paquets/s de 1500 o utiles (~300 Mbit/s) sans violation N1–N11 | seuil de mise au point dépendant de l’hôte |
| matériel 1 GbE | 70 000 paquets/s de 1500 o utiles (~840 Mbit/s) avec drops expliqués | objectif ambitieux, pas promesse commerciale |
| coût de politique | débit ≥ 90 % du même chemin pré-résolu ; p99 +≤ 10 % | budget relatif, non une microseconde universelle |
| multi-cœur | commencer seulement si un Engine monofile est >80 % CPU avec backlog/drops et lien sous-utilisé | évite une réécriture avant preuve du goulot |

`smoltcp` convient au démarrage bare-metal sans allocation de tas selon sa documentation ([smoltcp 0.12](https://docs.rs/crate/smoltcp/0.12.0)). Sa conservation pour le multi-queue n’est pas décidée d’avance : le résultat B2/B3 et le profil monofile déterminent si l’Engine doit être découpé en shards par file/cœur ou si la pile doit évoluer.

Les limites par débit/rafale suivent un modèle de seaux de jetons à débit engagé et rafale, adapté comme mécanisme local de contrôle ([RFC 2697](https://www.rfc-editor.org/rfc/rfc2697/)).

## 16. Migration sûre depuis le réseau actuel

| Étape | Changement | Coexistence / porte |
|---|---|---|
| M0 | carte de flux réelle, QEMU/TAP, trace et correction IOMMU | aucune politique nouvelle ; DMA-0 requis avant prêt de pages |
| M1 | nouveaux tags CapToken et N1–N5 | bridge et `network_server` existants continuent en mode compatibilité mesuré |
| M2 | Arbiter à côté de l’Engine, Pacte statique, Brin pour `connect` sortant | dual-run : décision ancienne/journal + décision ExoNet comparée avant enforcement |
| M3 | enforcement pour un Cercle de test, Console/Lens, `explain` | retour arrière par politique, pas par désactivation cachée des contrôles |
| M4 | Portal propriétaire DMA, BufferGrant v1a, N6/N11 tests | aucun zéro-copie application→NIC |
| M5 | DNS `NameBound`, QoS, AuthorityFrozen, Phoenix/B5bis | seulement après observations de M3/M4 |
| M6 | `DataRing` ou multi-queue, uniquement si mesures justifient | benchmark avant/après et test d’invariants inchangé |

Le dual-run compare les décisions, il ne duplique pas le trafic applicatif ni les droits. Toute divergence est journalisée et bloque le passage à l’enforcement pour le Cercle concerné.

## 17. Budget de réalisation et décisions Go/No-Go

| Lot | Estimation réaliste |
|---|---:|
| M0 : runtime, IOMMU, trace, QEMU/TAP | 3–6 semaines-personnes |
| M1–M3 : capacités, Arbiter, Console/Lens, migration/dual-run | 12–20 sem.-pers. |
| M4–M5 : Portal DMA, BufferGrant, DNS, QoS, Phoenix | 13–24 sem.-pers. |
| preuve N1–N5 et modèle TLA+ dédié | 4–6 sem.-pers. |
| fuzz, B2–B6, matériel et stabilisation | 4–10 sem.-pers. |

**Total v1 de travail : 36 à 66 semaines-personnes**, hors certification/audit externe. Avec 30–50 % de marge de découverte — justifiée par le DMA, la validation matérielle et les preuves — une personne seule doit prévoir plutôt **11 à 18 mois calendaires** qu’un calendrier optimiste. v1.5 (N6/N8/N11 formels et optimisation multi-queue) est un lot distinct.

**Go M1 :** DMA-0, B0/B1 et O-1 verts.
**Go M4 :** N1–N5 validés, dual-run sans divergence non expliquée.
**Go M6 :** B2/B3 montrent un goulot mesuré ; pas seulement l’envie d’un chiffre élevé.
**No-Go immédiat :** Bypass IOMMU actif sur le chemin cible, page `Published` réutilisée sans quiescence, ouverture après révocation, ou incapacité à expliquer un refus.

## 18. Ce que v1.1 refuse explicitement

- protocole LAN propriétaire pour contourner Ethernet/IP ;
- reprise TCP transparente après crash ;
- DMA app→NIC ou « zéro-copie » avant preuve de retrait IOMMU ;
- libération d’une page DMA uniquement parce qu’un timeout expire ;
- autorisation CDN par AS ou préfixe trop large ;
- cache d’Arbiter qui accepte des ouvertures après perte de vie du service ;
- cinq gros services obligatoirement chargés dans une cible mémoire contrainte ;
- super-utilisateur réseau quotidien ;
- chiffres de performance présentés comme acquis sans B2–B6 ;
- promesse de contrôler les appareils externes d’un LAN qui ne traversent pas ExoNet.

## 19. Décision de départ

La prochaine action n’est ni un nouveau protocole, ni l’optimisation de VirtIO : c’est M0. Il faut établir le chemin réel `application -> net_bridge -> network_server -> virtio_net`, le faire circuler sur QEMU/TAP, mesurer B1, mettre en place `exonet explain`, puis fermer le contournement IOMMU du chemin cible. Cette base rend possibles les capacités, l’administration, les performances et la preuve sans que l’un serve d’alibi aux autres.

## Références de conception

- [RFC 9293 — Transmission Control Protocol](https://www.rfc-editor.org/rfc/rfc9293.html) : états et réinitialisation TCP.
- [RFC 2697 — Single Rate Three Color Marker](https://www.rfc-editor.org/rfc/rfc2697/) : paramètres de débit engagé et rafale.
- [VirtIO 1.3](https://docs.oasis-open.org/virtio/virtio/v1.3/virtio-v1.3.html) : files de buffers et fonctions négociées.
- [smoltcp 0.12](https://docs.rs/crate/smoltcp/0.12.0) : pile réseau adaptée au bare metal sans allocation de tas.
