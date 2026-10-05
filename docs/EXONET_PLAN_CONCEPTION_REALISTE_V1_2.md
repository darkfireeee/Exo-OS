# ExoNet v1.2 — spécification finale de conception, sûreté et mise en œuvre

**Statut :** cible de conception. Rien dans ce document ne vaut comme fonctionnalité livrée sans sa porte de validation.
**But :** fournir un réseau LAN/Internet utilisable par les applications Exo-OS, administrable sans privilège ambiant, sûr face aux erreurs de service et mesurable avant toute promesse de performance.

La v1.2 conserve les décisions structurelles justes des versions antérieures et ferme les dernières ambiguïtés : nature de l’autorité mise en cache, granularité des époques, récupération hors ligne, bornes des tables, intégrité des instantanés Kernel B, exploitation de l’audit existant, et séquencement réaliste.

---

## 1. Discipline de vérité

- **[OBSERVÉ]** : constat dans le checkout actuel ou résultat de test local daté.
- **[DÉCISION]** : contrat qui doit être implémenté.
- **[ESTIMATION]** : objectif, taille candidate ou budget ; jamais une mesure acquise.
- **[PORTE]** : condition Go/No-Go vérifiable.
- **[RÉSIDU]** : limite connue qui n’est pas couverte par la propriété annoncée.

Cette notation s’applique aussi aux nombres secondaires : temps de drain, taille d’anneau, nombre de Brins et durée de récupération sont des paramètres initiaux à mesurer, jamais des constantes magiques.

## 2. Décision centrale et frontières honnêtes

ExoNet est une **couche d’autorité réseau à capacités** autour d’Ethernet, IPv4, TCP, UDP, ICMP et DNS. Il ne remplace pas les standards LAN/Internet. Il associe chaque opération réseau à un sujet, une politique, un budget et une trace :

```text
Sceau (qui) -> Pacte (quoi/vers où/combien) -> Brin (session concrète)
                                   |
                         Lentille (qui peut constater)
```

L’application garde une expérience simple : `socket`, `connect`, `send`, `recv`, `bind`, `listen` et `accept` via ExoFS. Elle ne reçoit ni un droit « réseau global », ni une liste de règles à gérer elle-même.

ExoNet contrôle le trafic qui passe par ses Portals, ses processus et ses Cercles. Il ne peut pas empêcher deux appareils externes, sur un commutateur non administré, de communiquer directement, d’émettre des ARP ou du DHCP. Une Console ne doit jamais laisser croire l’inverse.

## 3. Décisions confirmées et corrections v1.2

| Sujet | Décision v1.2 | Raison |
|---|---|---|
| Protocole LAN | Ethernet/IP standard ; aucune ExoFabric propriétaire en v1 | interopérabilité et surface de preuve réduite |
| Reprise TCP | rupture explicite puis reconnexion applicative | TCP conserve un état aux deux extrémités ; aucune reprise locale « magique » ([RFC 9293](https://www.rfc-editor.org/rfc/rfc9293.html)) |
| Planificateur Ring 0 | refusé | Ring 0 vérifie, lie, révoque et mappe ; il ne planifie pas les paquets |
| Zéro-copie app→NIC | refusé en v1 | retrait IOMMU, annulation de descripteur et révocation concurrente ne sont pas encore prouvés |
| `AdmissionLease` | capacité noyau typée, non une signature/HMAC par ouverture | aucune cryptographie sur le chemin d’ouverture ou de paquet ; un HMAC partagé avec l’Engine lui donnerait aussi le pouvoir de forger |
| `DenySet` | remplacé par une table de générations indexée | coût O(1), aucune éviction d’une révocation active, comportement de saturation défini |
| DNS/CDN | `PinnedAddress` ou `NameBound`, jamais AS/CIDR vague | un AS ou grand préfixe n’est pas une identité de destination sûre |
| TLS et ECH | l’Engine ne lit pas le SNI ni le ClientHello | l’autorisation L3/L4 est séparée de la validation TLS applicative ; ECH ne casse donc pas ExoNet |
| Audit | réutiliser ExoLedger et sa zone P0, pas créer un journal concurrent | continuité avec ExoShield et moins de mécanismes critiques |

## 4. Point de départ réel et porte DMA-0

### 4.1 État observé du checkout

**[OBSERVÉ]** Le réseau actuel contient :

- `network_server` avec `smoltcp`, DHCP, routage, ICMP, TCP/UDP, liaison pilote et table de 64 sockets ;
- `virtio_net` avec contrôle VirtIO et suivi de 256 buffers RX ;
- `net_bridge` avec au plus 128 octets de données inline ;
- un pool actuel de 256 pages RX et 256 pages TX de 4 Kio, soit 1 Mio par direction ;
- un `CapToken` typé et générationnel de 24 octets ABI ;
- un ExoLedger avec audit chaîné et zone P0 immuable ;
- des tests validés pour DHCP, routage, ICMP, handles de sockets et le modèle `exonet_stress`.

Ils ne prouvent pas le trafic E2E QEMU, le débit, la quiescence DMA ou la résurrection Phoenix pendant charge.

### 4.2 Chemin DMA effectivement trouvé

**[OBSERVÉ]** `network_server::bootstrap()` appelle `NetBufPool::init()`. Ce pool RX/TX transmet `DMA_MAP_FLAGS_BYPASS_IOMMU` aux deux appels `SYS_DMA_ALLOC`. `virtio_net::Virtqueue::init()` transmet le même drapeau pour les files. Dans l’allocateur DMA, ce drapeau choisit `IOVA == adresse physique` et n’insère pas le mapping dans la table IOMMU.

Le noyau met bien en place une politique NIC/IOMMU au boot. Mais le chemin réseau utilisateur ci-dessus contourne la traduction IOMMU. Cette tension doit être résolue par l’exécution, pas masquée par un document.

### 4.3 Porte bloquante

**[DÉCISION]** Le Portal devient le seul propriétaire des pages visibles à la NIC et des files VirtIO. L’Engine ne possède ni adresse physique choisie par lui, ni domaine IOMMU, ni droit de contourner le Portal.

**[PORTE DMA-0]** avant `BufferGrant`, benchmark de performance ou annonce de confinement matériel :

1. le chemin Portal/VirtIO actif ne passe plus `BYPASS_IOMMU` ;
2. la NIC est attachée au seul domaine du Portal ;
3. ce domaine ne mappe que le pool Portal et les descripteurs autorisés ;
4. une tentative DMA hors plage est rejetée et journalisée ;
5. retrait de page, reset de file et invalidation IOMMU sont testés avant toute réutilisation.

Un échec DMA-0 bloque les jalons suivants. Cela n’affirme pas que chaque configuration actuelle est exploitable ; cela interdit simplement de lui attribuer une garantie de confinement qu’elle n’a pas encore démontrée.

## 5. Services Ring 1 et profils mémoire

| Service | Autorité exacte | Ne détient jamais |
|---|---|---|
| **Portal** (`virtio_net`) | NIC, IRQ, domaine IOMMU, pages DMA, files VirtIO, garde L2 mécanique | Pactes, Enveloppes, politique utilisateur |
| **Engine** (`network_server`) | pile L3/L4, sockets, queues, quotas, Brins, copies contrôlées | racine de politique, DMA arbitraire, changement de Cercle |
| **Arbiter** (`net_policy_server`) | Pactes, générations, leases, révocations, publication de politique | NIC, IRQ, IOMMU, lecture de charges utiles |
| **Lens** (`net_observe_server`) | compteurs et explications filtrés | contenu applicatif, mutation de politique |
| **Console** (`net_admin_server`) | interface d’administration, approbation, Enveloppes | descripteurs, DMA, socket hors Pacte |

| Profil | Pool de Brins / leases / slots de Pacte | Événements rapides | Usage |
|---|---:|---:|---|
| **Petit** | 64 / 128 / 256 | 4 096 | QEMU, cible mémoire contrainte |
| **Standard** | 256 / 512 / 1 024 | 16 384 | poste ou service ordinaire |
| **Dense** | 4 096 / 8 192 / 16 384 | 65 536 | serveur, après mesure mémoire |

Ces tailles sont **[ESTIMATION : profils initiaux]** sélectionnées au boot par un `NetMemoryBudget`, pas des allocations dynamiques dans le chemin de données. Les ratios sont intentionnels : deux leases par Brin couvrent l’ouverture et le churn sans rendre le cache infini. Un profil qui ne tient pas en RAM est refusé au boot ou dégradé explicitement ; jamais ajusté silencieusement à chaud.

| Saturation | Résultat sûr |
|---|---|
| pool de Brins plein | nouvelle ouverture `EAGAIN`, flux existants préservés |
| cache de leases plein | éviction seulement d’entrée expirée et non référencée ; sinon miss Arbiter ou `EAGAIN` |
| slots de Pactes pleins | publication `ENOSPC`, aucun Pacte actif évincé |
| pages Portal pleines | `ENOBUFS`/`EAGAIN`, aucune page réutilisée prématurément |
| audit de politique non inscriptible | nouvelle mutation d’administration refusée, trafic courant non bloqué |

## 6. Amorçage et récupération sans racine en ligne

### 6.1 Démarrage normal

```text
Couronne hors ligne
   -> signe le manifeste de démarrage
   -> Kernel B vérifie continuité après SECURITY_READY
   -> Arbiter initial reçoit une autorité strictement bornée
   -> publication des Pactes et Enveloppes initiaux
```

Le manifeste contient la clé publique de Couronne, les Sceaux de services initiaux, les Cercles, l’empreinte de politique de départ et une autorité de récupération séparée. Un PID, une MAC, une IP ou le nom d’un binaire n’est jamais une racine d’autorité.

### 6.2 Récupération `RestrictedRecovery`

La Couronne reste hors ligne, mais une panne doit être opérable.

**[DÉCISION — solo]** l’opérateur se présente à une Console physique locale avec un support de récupération signé par la Couronne et une authentification locale indépendante. Le support ne contient pas la clé racine copiable par l’OS ; il délivre une `RecoveryEnvelope` à durée courte.

**[DÉCISION — entreprise]** la même Console exige un seuil de détenteurs hors ligne pour produire/présenter une `RecoveryEnvelope`; aucun quorum ne transite par le réseau de production en état restreint.

Une `RecoveryEnvelope` est limitée à : lire les diagnostics, charger un instantané déjà signé, révoquer/isoler un Cercle, relancer Arbiter/Portal, publier la correction qui rétablit la cohérence. Elle ne peut ni ouvrir un accès Internet général, ni créer une délégation persistante, ni transformer la Console en root quotidien. Sa durée est **[ESTIMATION : 15 minutes initiales]** et son expiration ramène l’état restreint tant qu’un commit normal A/B n’a pas réussi.

## 7. Objets, versions et autorité sans crypto à chaud

| Objet | Durée | Fonction |
|---|---|---|
| **Sceau** | persistante | identité de l’utilisateur, service, appareil ou administrateur |
| **Pacte** | durable | règle versionnée d’accès, quotas et destination |
| **AdmissionLease** | courte et non délégable | autorisation compilée d’ouvrir un Brin exactement défini |
| **Brin** | session | droit concret attaché à un socket/flux/dgram |
| **Lentille** | déléguée | lecture filtrée des événements |
| **Cercle** | durable | frontière logique |
| **Enveloppe** | bornée | droit d’administration atténué |
| **Couronne** | racine | gouvernance hors ligne |

### 7.1 Trois compteurs, trois rôles

| Compteur | Avance quand | Effet |
|---|---|---|
| `authority_epoch` global | redémarrage/rotation d’Arbiter, récupération, compaction de sécurité | invalide **toutes** les AdmissionLeases et force un broadcast |
| `policy_sequence` global | chaque transaction publiée | ordre d’audit et empreinte du snapshot ; n’invalide pas tout le cache seul |
| `pact_generation` par slot | modification ou révocation du Pacte | invalide les leases et Brins de ce Pacte |

La validité d’une lease est donc : `authority_epoch` courant **et** `pact_generation` courant **et** expiration **et** sujet/destination exacts. Une modification d’un Pacte ne vide pas inutilement les caches d’autres Pactes ; un événement global de sûreté les invalide tous.

### 7.2 Nature exacte d’`AdmissionLease`

Une lease n’est ni Ed25519, ni HMAC partagé, ni une structure acceptée sur parole par l’Engine. C’est un `CapToken` de type `NetworkLease`, créé par le noyau à la demande d’un Arbiter qui possède l’autorité parent `NetworkPact`.

Le `NetworkAuthorityTable` noyau, de taille bornée, associe le `object_id` de la lease à :

```text
sceau, pact_slot, pact_generation, authority_epoch,
transport, port, ensemble_endpoint_ou_profile, limites, expiration
```

L’Engine présente la lease à `net_materialize_brin()`. Le noyau vérifie type, droits, génération et spécification exacte avant de créer le Brin ; l’Engine ne peut pas fabriquer une lease ou modifier son périmètre. La crypto reste réservée au manifeste, aux commits de politique et au registre d’audit ; le chemin d’ouverture fait des lectures bornées de tables et des comparaisons, pas une vérification cryptographique par `connect()`.

Un HMAC dont l’Engine connaît la clé est explicitement refusé : vérifier et fabriquer un MAC seraient alors le même pouvoir. Une signature asymétrique à chaque ouverture est également refusée : elle est inutile puisque le noyau est déjà l’émetteur de la capacité non forgeable.

### 7.3 Broadcast et cache

Le noyau expose une `AuthorityView` lecture seule aux services réseau :

```text
{ authority_epoch, authority_state, policy_sequence, root_digest_hint }
```

Lors d’un changement global, il publie les champs avec `Release` et envoie une notification ; Engine les lit avec `Acquire`. La notification réduit la latence, mais la lecture de l’époque est la vérité : une notification perdue ne rend pas une lease valide.

L’Engine indexe son cache par :

```text
(sceau, destination_profile, transport, port,
 authority_epoch, pact_slot, pact_generation)
```

Après un changement d’`authority_epoch`, une comparaison suffit à rendre toutes les anciennes entrées inexploitables ; leur nettoyage peut être différé. Après une génération de Pacte, seuls les éléments de son slot deviennent invalides.

## 8. Révocation : table indexée, pas un `DenySet` évictable

Le `DenySet` imprécis est remplacé par une **RevocationTable** noyau indexée par `pact_slot` :

```text
RevocationEntry[pact_slot] = { current_generation, state, active_brin_refs }
```

L’Engine charge `current_generation` avec `Acquire` à l’admission et à chaque frontière de lot ; le noyau la vérifie lors de chaque opération de capacité sensible. Ainsi, une lookup est O(1), ne dépend pas du nombre de révocations et n’utilise aucune éviction LRU.

Une révocation :

1. incrémente la génération du slot et invalide les capacités descendantes ;
2. fait passer les Brins concernés vers `Revoked`/`Draining` ;
3. écrit l’événement ExoLedger ;
4. informe Engine/Portal ;
5. conserve le slot tant que lease ou Brin de l’ancienne génération peut encore y faire référence.

Un slot n’est réutilisable qu’après absence de référence, invalidation de ses leases et barrière d’`authority_epoch` si nécessaire. Si la table de slots est pleine, la Console retourne `ENOSPC`; elle n’écrase jamais une révocation active. Cette sémantique évite le cas dangereux « table de refus pleine, droit ancien ressuscité ».

## 9. Kernel B : couverture précise et résidu explicite

Un `PolicySnapshot` est immuable, copy-on-write et mappé lecture seule dans les services qui le consultent. Chaque publication produit :

```text
{ authority_epoch, policy_sequence, root_digest, changed_pact_digests }
```

Kernel A et Kernel B reçoivent ce commit hors du chemin de données. Aucun assouplissement de politique n’est actif avant accusé cohérent des deux. Kernel B vérifie l’ordre, l’époque, l’empreinte racine et les métadonnées de commit qu’il sait borner ; il ne traverse pas le graphe de l’entreprise et ne planifie aucun paquet.

**[RÉSIDU K-B]** un résumé `{epoch, sequence, digest}` ne détecte pas continuellement une corruption mémoire d’un objet individuel déjà activé si ni l’objet ni son digest ne sont relus. La v1.2 ne prétend pas le contraire.

Les mitigations v1 sont : pages de snapshot RO, copy-on-write, capacités noyau à génération, vérification de spécification de lease au moment de matérialiser un Brin, journal ExoLedger et scrubbing de fond d’objets de politique. Une détection exhaustive de corruption mémoire arbitraire exigerait davantage de redondance, de vérification de feuilles ou de matériel ; elle est hors propriété v1. Toute divergence de commit détectée mène à un état plus restrictif, jamais plus permissif.

## 10. DNS, Internet et chiffrement sans inspection L7

| Profil | Usage | Règle |
|---|---|---|
| `PinnedAddress` | LAN/infra stable | CIDR et ports exacts |
| `NameBoundTls` | Internet/CDN | FQDN borné, résolveur autorisé, TTL plafonné, ports, validation TLS applicative obligatoire |
| `NameBoundPlaintextException` | legacy interne | CIDR privé limité + Cercle déclaré + approbation d’exception + journal explicite |

`NameBoundTls` ne cherche jamais le SNI dans le paquet : l’Engine autorise l’endpoint résolu au moment de `connect`; la bibliothèque applicative, liée au Sceau, valide le certificat et le nom. Cette séparation fonctionne même si le ClientHello protège le SNI.

Le cache DNS contrôlé par ExoNet est indexé par `{authority_epoch, pact_slot, pact_generation, fqdn, resolver, transport, port}`. Le TTL est plafonné par Pacte **[ESTIMATION : plafond choisi par profil]**. Une connexion établie ne bascule jamais à une autre IP à expiration du TTL ; une nouvelle ouverture résout à nouveau. Le cache DNS interne de pile ne peut pas être utilisé sans cette clé de politique.

Une option `tls_required: false` n’est donc pas un défaut de Cercle « trusted ». Elle correspond uniquement à `NameBoundPlaintextException`, visible dans le diff, avec une approbation qui assume explicitement que le nom DNS seul ne prouve pas l’identité distante.

## 11. Contrat DMA et chemin de données

### 11.1 Possession

```text
application -- MemoryRegion non-DMA --> Engine
                                      |
                                      | copie contrôlée
                                      v
                           DmaLease temporaire
                                      |
                                      v
Portal (pages DMA, IOMMU, VirtIO) --> NIC
```

Le Portal alloue les pages DMA et maintient leur mapping IOMMU. L’Engine reçoit un `DmaLease` temporaire, sans transfert de domaine. L’application ne mappe jamais une page Portal et ne soumet jamais une adresse physique.

### 11.2 États de page

| État | CPU autorisé | NIC DMA | Sorties possibles |
|---|---|---|---|
| `Free` | Portal | non publiée | `ReserveTx`, `PublishRx` |
| `ReservedTx` | Portal | non publiée | `MapEngineTx`, `Cancel` |
| `MappedEngineTx` | Engine RW temporaire | non publiée | `ReadyTx`, `Cancel` |
| `ReadyTx` | aucun Engine | non publiée | `PublishTx` |
| `PublishedTx` | Portal | lecture NIC possible | `CompletedTx`, `Quarantine` |
| `PublishedRx` | Portal | écriture NIC possible | `CompletedRx`, `Quarantine` |
| `MappedEngineRx` | Engine RO temporaire | non publiée | `ReleaseRx` |
| `Quarantine` | récupération Portal/noyau | accès normal interdit | `DmaQuiesced -> Free` |

```text
TX: Free -> ReservedTx -> MappedEngineTx -> ReadyTx -> PublishedTx -> CompletedTx -> Free
RX: Free -> PublishedRx -> CompletedRx -> MappedEngineRx -> Free
```

Transitions interdites : publication sans retrait de mapping Engine, réutilisation sans complétion ou quiescence DMA, double `DmaLease`, mapping applicatif d’une page Portal et libération sur timeout seul.

### 11.3 Révocation et N11

La révocation est logique puis physique : aucune nouvelle émission n’est admise après la nouvelle génération ; les descripteurs déjà publiés attendent complétion ou récupération matérielle.

**N11 — libération sûre et bornée.** Le temps d’attente applicatif est borné par classe. Paramètres initiaux **[ESTIMATION]** : 100 ms pour 0–1, 500 ms interactif, 2 s service, 5 s arrière-plan/quarantaine. À échéance, le Brin libère son budget logique et l’application reçoit une erreur. La page reste en `Quarantine` jusqu’à une preuve de quiescence DMA : complétion VirtIO, reset de file/périphérique et invalidation IOMMU confirmés.

Un reset Portal/NIC est une mesure exceptionnelle. Il peut pénaliser temporairement tous les Brins de la NIC ; la sûreté de mémoire prime volontairement sur leur disponibilité. Cette dégradation devient un événement P0 ExoLedger et un motif `exonet explain`.

### 11.4 Données en deux étapes

- **v1a :** lots de 1 à 16 pages **[ESTIMATION : borne initiale]**, contrôlés par IPC, pour valider N6/N11 et dépasser 128 octets ;
- **v1b :** `DataRing` SPSC seulement si B2 montre que plus de 25 % des cycles du chemin de données sont dans IPC/mapping/doorbells **[ESTIMATION : seuil de décision]**.

`DataRing` est par paire application→Engine, pas par Brin. Son descripteur est borné : `{brin_index, window_id, offset, len, seq, flags}`. Producteur : écrit le descripteur puis publie `tail` en `Release`. Consommateur : lit `tail` en `Acquire`, puis le descripteur. Les indices sont atomiques, les entrées ne sont réutilisées qu’après passage de propriété, et la remise à zéro exige l’arrêt/quiescence des deux extrémités. Aucune hypothèse de cohérence x86 ne doit se glisser dans ce protocole : il reste correct sur les architectures à mémoire faible.

## 12. QoS, contrôle L2 et état `AuthorityFrozen`

Chaque Pacte porte `octets/s`, `paquets/s`, rafales en octets et paquets, pages maximum et connexions maximum. Une délégation ne peut que les diminuer.

| Classe | Usage | Garantie |
|---:|---|---|
| 0 survie | battement Phoenix / récupération | réserve minuscule, plafond strict |
| 1 contrôle | DHCP/ARP host et administration | réserve de descripteurs/buffers, plafond strict |
| 2 interactif | shell, UI, RPC | p99 prioritaire |
| 3 service | API métier | fair sharing par Pacte |
| 4 arrière-plan | sauvegarde, mises à jour | surplus abandonnable |
| 5 quarantaine | contexte non approuvé | plafond dur et traçabilité accrue |

L’Engine applique un seau de jetons par Pacte et un round-robin à déficit en classes 2–5. Les classes 0–1 ne sont jamais une voie non limitée ; elles combinent priorité et plafonds. Le modèle débit/rafale s’inspire de la mécanique de contrôle de trafic définie par [RFC 2697](https://www.rfc-editor.org/rfc/rfc2697/), sans prétendre implémenter tout DiffServ.

Le Portal réserve un petit nombre de descripteurs/buffers pour 0–1 **[ESTIMATION : taille issue de B3]**. Cette réserve est une protection mécanique ; elle ne contient aucun Sceau, Pacte, Cercle ou droit utilisateur. Le Portal peut plafonner ARP/DHCP par interface et MAC source dans une table bornée pour éviter l’épuisement des ressources ; il ne devient pas un pare-feu d’applications.

Si l’Arbiter disparaît, l’Engine passe `AuthorityFrozen` après absence de battement : toutes nouvelles ouvertures échouent proprement, y compris sur cache ; les Brins courants continuent jusqu’à leur règle existante. Un nouveau broadcast d’`authority_epoch` après Phoenix invalide l’intégralité des leases anciennes O(1). La Couronne garde une capacité de coupure d’urgence sans droit d’ouverture.

## 13. POSIX, `fork`, erreurs et reprise

`fork()` n’hérite d’aucune `NetworkSession` par défaut : le fils reçoit des FD réseau marqués invalides. Un Pacte peut activer `inherit_fork`; dans ce seul cas, le noyau crée une référence au **même** Brin, avec quotas et révocation partagés. Ce n’est pas un Brin nouveau. `exec()` réévalue toujours le Sceau de l’exécutable et ferme les sessions incompatibles. Passer un FD à un autre Sceau exige une délégation explicite.

| Situation | Résultat applicatif |
|---|---|
| absence de Pacte ou destination hors profil | `EACCES` / cause ExoNet |
| lease expirée entre hit cache et matérialisation | échec atomique `EAGAIN`, aucun FD partiel |
| epoch/génération devenue obsolète | cache invalidé, nouveau passage Arbiter ou `EAGAIN` |
| pool DMA ou Brin saturé | `ENOBUFS` ou `EAGAIN` avec cause Lens |
| débit/paquets dépassés | attente, `EAGAIN` avec `MSG_DONTWAIT`, ou délai déclaré |
| envoi partiel | longueur réellement admise, jamais succès fictif |
| révocation/Phoenix | `EPIPE` ou `ECONNRESET`; reprise laissée au protocole applicatif |

## 14. Observabilité et ExoLedger

ExoNet ne construit pas un second journal. Les actes irréversibles — publication, révocation, divergence A/B, IOMMU/quiescence en échec, récupération — vont vers ExoLedger, y compris sa zone P0 selon leur criticité. Si l’écriture d’audit requise échoue, la mutation de politique est refusée ; elle ne devient jamais un changement non traçable.

Les événements fréquents vont dans un `FastEventRing` borné par profil. C’est un anneau best-effort : il peut écraser les plus anciens événements de télémétrie et incrémente un compteur de perte. Il ne bloque jamais le trafic. Les événements de gouvernance/sûreté ne reposent pas sur cet anneau ; ils exigent ExoLedger.

La Lentille offre :

```text
exonet status                 # interfaces, pools, époques, queues, pertes
exonet explain <fd|request>   # Sceau, Pacte, Brin, génération, motif précis
exonet trace <cercle>         # flux d'événements filtré, sans charge utile
exonet policy diff <a..b>     # commit, approbations, impact
```

**[PORTE O-1]** chaque refus B1–B6 doit exposer un code stable et l’objet responsable. La Console et Lens ne reçoivent pas le contenu applicatif par défaut.

## 15. Invariants et niveau de preuve

| ID | Invariant |
|---|---|
| N1 | aucune émission/réception applicative sans Brin actif |
| N2 | tout Brin lie Sceau, Pacte, génération et limites valides |
| N3 | une délégation est toujours un sous-ensemble du parent |
| N4 | une révocation interdit les opérations descendantes nouvelles |
| N5 | fork/exec/transfert FD n’amplifient aucune autorité |
| N6 | une page Portal n’a qu’un état et un lease actif autorisés |
| N7 | Portal n’a pas de politique ; Arbiter n’a ni IRQ ni DMA |
| N8 | divergence A/B => état plus restrictif |
| N9 | une Lentille ne lit rien hors de son filtre |
| N10 | QoS protège les réserves sans famine durable |
| N11 | timeout logique distinct de quiescence physique DMA |

| Livraison | Vérification exigée |
|---|---|
| **v1-pilot** | modèles/tests N1–N5, DMA-0, B0/B1, O-1 |
| **v1 contrôlée** | TLA+/preuve de capacités N1–N5 ; fuzz et model tests N6–N11 |
| **v1.5** | modèle DMA/Portal/Phoenix pour N6, N8, N11 |
| **v2** | liveness QoS, Lens et multi-queue dans un modèle étendu |

N6–N11 sont des exigences v1, mais ne sont pas qualifiées de formellement prouvées avant v1.5. C’est une distinction de vérité, non un report de sûreté.

## 16. Benchmarks et budgets de performance

### 16.1 Calculs de capacité

**[ESTIMATION arithmétique]** 128 octets inline exigent au minimum 976 563 soumissions/s pour 1 Gbit/s utile, sans inclure IPC, copies, entêtes ou acquittements. Une page de 4 Kio ramène ce nombre à 30 518/s ; un lot de 16 pages à 1 908/s.

**[ESTIMATION arithmétique]** le pool actuel de 1 Mio par direction représente 8,39 ms à 1 Gbit/s, 3,36 ms à 2,5 Gbit/s et 0,84 ms à 10 Gbit/s. Il ne prouve ni absorption de rafale ni p99 réel.

### 16.2 Protocole B0–B6

| Benchmark | Vérifie |
|---|---|
| B0 | ABI, capacités, génération, profils de pools, machines d’états |
| B1 | TCP/UDP écho, erreurs, `exonet explain`, ouverture/refus |
| B2 | 64/512/1400 o, 1/64 flux, inline vs lots, part de cycles IPC |
| B3 | quatre Pactes classes 1–5, fairness, p99, réserve 0–1 |
| B4 | révocation pendant B2/B3, génération et N11 |
| B5 | panne Engine/Portal/Kernel A, récupération et quiescence |
| B5bis | panne Arbiter pendant open et trafic courant |
| B6 | token périmé/forgé, fork, double lease, DNS interdit, tempête L2 |

Chaque résultat publie révision, compilation, CPU/RAM, QEMU/backend TAP, MTU, nombre de files, affinité, durée, tailles, flux, p50/p95/p99, débit utile, paquets/s, CPU, pertes, erreurs d’autorité et mémoire par profil. Le NAT utilisateur QEMU ne sert pas à conclure sur le débit.

| Objectif | Valeur | Statut |
|---|---:|---|
| lots QEMU/TAP | 25 000 pps de 1500 o, ~300 Mbit/s utiles | **[ESTIMATION : objectif de mise au point]** |
| matériel 1 GbE | 70 000 pps de 1500 o, ~840 Mbit/s utiles | **[ESTIMATION : objectif ambitieux]** |
| coût de politique | ≥90 % du débit pré-résolu, p99 +≤10 % | **[ESTIMATION : budget relatif]** |
| DataRing | seulement si IPC/mapping/doorbell >25 % cycles B2 | **[ESTIMATION : seuil de décision]** |

`smoltcp` est approprié au démarrage bare-metal sans allocation de tas selon sa documentation ([smoltcp 0.12](https://docs.rs/crate/smoltcp/0.12.0)). Une décision multi-queue ne se prend qu’après B2/B3 : cœur Engine >80 % CPU avec backlog/pertes et lien sous-utilisé est la porte de profilage, pas une promesse de réécriture.

## 17. Migration sans double trafic

Le dual-run compare deux **décisions**, jamais deux transmissions.

| Mode | Chemin qui agit | Chemin candidat | But |
|---|---|---|---|
| `shadow-sampled` | chemin existant | calcule 1 requête sur N ou seulement Cercle test ; journalise | découverte peu coûteuse |
| `shadow-full-test` | chemin existant | calcule chaque open du Cercle test ; journalise divergences | qualification |
| `enforce-test` | ExoNet du Cercle test | chemin ancien observé seulement si possible | preuve de déploiement |
| `rollout` | ExoNet par Cercle | Lens/ExoLedger suivent | extension progressive |

Une divergence en mode shadow n’interrompt pas le trafic existant ; elle est expliquée, corrigée ou explicitement acceptée avant `enforce-test`. La porte d’enforcement est exhaustive dans le Cercle test, même si la phase exploratoire est échantillonnée.

## 18. Jalons, budget et définition de « v1 »

| Jalon | Contenu | Estimation |
|---|---|---:|
| M0 — pilot matériel | trace réelle, TAP/QEMU, O-1, DMA-0, budget mémoire | 4–8 sem.-pers. |
| M1 — autorité | Sceaux, Pool/slots, `NetworkLease`, N1–N5 et preuve initiale | 10–16 sem.-pers. |
| M2 — politique en ombre | Arbiter, époques, DNS `PinnedAddress`, shadow-sampled/full | 8–12 sem.-pers. |
| M3 — administration contrôlée | Console, Lens, ExoLedger, `enforce-test` | 6–10 sem.-pers. |
| M4 — données sûres | Portal IOMMU, DmaLease, BufferGrant v1a, N6/N11 | 12–20 sem.-pers. |
| M5 — résilience réseau | QoS, NameBound, Phoenix, B4–B6 | 8–14 sem.-pers. |
| M6 — optimisation optionnelle | DataRing/multi-queue uniquement justifiés par profils | 6–14 sem.-pers. |

**Total M0–M5 : 48–80 semaines-personnes.** M6 est volontairement hors total. Pour un seul développeur, la plage réaliste est **14–26 mois calendaires**, hors certification/audit externe. Le travail TLA+ N1–N5 a un budget explicite dans M1 ; il ne doit pas être réduit pour faire tenir une date.

Les jalons ne doivent pas être renommés pour masquer leur état :

- **pilot** : M0 seulement, aucune promesse réseau sécurisé ;
- **v1 contrôlée** : M0–M3, accès de test administré et politique vérifiable ;
- **v1 utilisable pour flux** : M0–M5, BufferGrant et reprise testés ;
- **v1.5** : N6/N8/N11 formalisés ;
- **performance avancée** : M6, uniquement après preuve de goulot.

**Go M1 :** DMA-0, B0/B1 et O-1 sont verts.
**Go M4 :** N1–N5, slots/pools saturés proprement, shadow-full sans divergence inexpliquée.
**Go M5 :** machine DMA et B4/B5/B5bis validés.
**No-Go immédiat :** Bypass IOMMU actif sur le chemin cible, page publiée réutilisée sans quiescence, lease matérialisée hors génération, ou refus inexpliquable.

## 19. Ce que v1.2 refuse définitivement

- protocole LAN propriétaire comme prérequis ;
- root réseau quotidien ou Console omnipotente ;
- signature/HMAC par open ou paquet ;
- HMAC partagé donnant à Engine le pouvoir de fabriquer une lease ;
- admission depuis un cache après perte d’Arbiter ;
- AS/CIDR large comme identité CDN ;
- inspection du SNI par l’Engine ;
- réutilisation DMA après timeout seul ;
- allocation dynamique incontrôlée quand les pools saturent ;
- double-run qui double les paquets réels ;
- promesse de performance ou de contrôle d’un LAN externe non administré.

## 20. Première action

Commencer par M0, dans cet ordre : tracer `application -> net_bridge -> network_server -> virtio_net`; introduire `exonet explain`; établir B1 sur QEMU/TAP ; fermer et tester le contournement IOMMU de la voie cible ; mesurer mémoire et pools. Aucun ajout de capabilities, d’administration ou de performance ne précède DMA-0.

## Références de conception

- [RFC 9293 — Transmission Control Protocol](https://www.rfc-editor.org/rfc/rfc9293.html)
- [RFC 2697 — Single Rate Three Color Marker](https://www.rfc-editor.org/rfc/rfc2697/)
- [VirtIO 1.3](https://docs.oasis-open.org/virtio/virtio/v1.3/virtio-v1.3.html)
- [smoltcp 0.12](https://docs.rs/crate/smoltcp/0.12.0)
