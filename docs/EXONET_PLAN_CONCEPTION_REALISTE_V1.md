# ExoNet v1 — plan de conception, d’administration et de validation

**Statut :** proposition d’architecture à implémenter et à vérifier.  
**Portée :** réseau local, Internet, applications, processus, services Ring 1, administration et observabilité.  
**Principe directeur :** la simplicité doit être visible pour l’utilisateur, pas obtenue en cachant une autorité implicite dans le noyau ou dans un service.

---

## 1. Comment lire ce document

Trois niveaux sont volontairement séparés :

- **[OBSERVÉ]** : constat dans le dépôt Exo-OS actuel ou résultat de test local.
- **[DÉCISION]** : choix d’architecture proposé pour ExoNet v1.
- **[ESTIMATION]** : modèle de coût ou objectif de benchmark. Ce n’est pas une mesure obtenue.
- **[PORTE]** : condition vérifiable à franchir avant d’activer l’étape suivante.

Cette séparation évite de confondre une bonne idée, une propriété prouvée et une performance mesurée.

## 2. Décision de synthèse

ExoNet n’est ni une pile TCP/IP entièrement dans Ring 0, ni un réseau propriétaire qui remplacerait Internet. C’est une **couche d’autorité réseau à capacités** placée autour d’une pile IP standard, répartie entre services Ring 1. Elle décide qui peut joindre quoi, avec quelle limite, pendant combien de temps, et qui peut le constater. La pile IP exécute ensuite cette décision.

L’interface simple exposée aux utilisateurs est :

1. un administrateur publie un service ou autorise une destination ;
2. une application reçoit un droit précis ;
3. elle ouvre une connexion normale via ExoFS/POSIX ;
4. ExoNet applique le droit, la limite, la priorité et l’audit automatiquement ;
5. une révocation ferme ou bloque les nouvelles opérations sans donner d’autorité supplémentaire à l’application.

Les cinq services Ring 1 cibles sont les suivants.

| Service | Rôle exclusif | Ne possède jamais |
|---|---|---|
| `virtio_net` / **Portal** | IRQ, DMA, files VirtIO et trames Ethernet | politiques utilisateur, droits d’administration |
| `network_server` / **Engine** | IPv4, DHCP, ARP, TCP/UDP/ICMP, tables de sockets et transfert de données | racine de gouvernance, droit de se déléguer des politiques |
| `net_policy_server` / **Arbiter** | compile les politiques en capacités, révocation, quotas et résolution autorisée | IRQ, DMA, accès direct à la NIC |
| `net_observe_server` / **Lens** | métriques, événements et journaux filtrés | données applicatives brutes, droits de modification |
| `net_admin_server` / **Console** | interface d’administration, approbations et import/export de politique | DMA, sockets arbitraires, contournement de l’Arbiter |

Cette séparation est une cible de migration : `network_server` existe déjà et porte aujourd’hui une part importante de ces fonctions.

## 3. Corrections apportées aux propositions antérieures

| Sujet | Décision finale | Justification concrète |
|---|---|---|
| Vocabulaire ExoWeave/ExoLoom | Conserver seulement **Sceau, Pacte, Brin, Lentille, Cercle, Enveloppe, Couronne**. | Les objets ont un rôle distinct et visible. Ajouter davantage de termes créerait une couche d’administration difficile à auditer. |
| Protocole LAN propriétaire | Ne pas créer ExoFabric en v1. Ethernet + IPv4 + TCP/UDP/ICMP restent le transport LAN/Internet. | Interopérabilité immédiate avec routeurs, DNS, outils et autres machines ; la politique reste ExoNet. |
| Reprise TCP « transparente » après Phoenix | Refusée. Le Brin est ré-arbitré ; l’application reçoit une rupture et se reconnecte si son protocole le permet. | Une connexion TCP a un état partagé entre les deux hôtes. Après perte de cet état, la norme TCP prévoit fermeture/réinitialisation, pas une résurrection locale magiquement transparente. Voir [RFC 9293](https://www.rfc-editor.org/rfc/rfc9293.html). |
| Gestionnaire de paquets dans Ring 0 | Refusé en v1. Ring 0 vérifie, lie, révoque et prête des ressources ; il ne planifie pas les paquets. | Réduit le TCB, la surface temps réel et l’effort de preuve du noyau. |
| Vérificateur complet de graphe dans Kernel B | Différé. Kernel B ne vérifie initialement qu’un résumé immuable de génération/empreinte de politique et les invariants de capacités qu’il sait déjà vérifier. | Une traversée complète de graphe dans Kernel B augmenterait fortement le code critique et les latences sans preuve préalable de bornage. |
| Zéro-copie | Non revendiquée en v1. Le premier chemin de gros messages copie une fois vers un tampon DMA appartenant à l’Engine. | Un DMA direct depuis la mémoire d’une application demande une preuve stricte de propriété, d’épinglage, d’IOMMU et de révocation. Il ne doit pas être introduit comme simple optimisation. |
| Une monnaie unique CPU/mémoire/réseau | Refusée. | Les unités et mécanismes de contention ne sont pas comparables. ExoNet ne gère que débit, rafales, paquets, files et pages de réseau. |
| « Admin » comme super-utilisateur implicite | Refusé. Toute administration est une Enveloppe de capacités, bornée et traçable. | Un titre humain ne doit jamais devenir une autorité ambiante dans le noyau. |

## 4. Point de départ réel du dépôt

### 4.1 Ce qui est déjà présent

**[OBSERVÉ]** Le checkout actuel n’est pas un terrain réseau vierge :

- `servers/network_server` utilise une version locale de `smoltcp` et contient DHCP, routage, ICMP, TCP/UDP, table de sockets et liaison au pilote ;
- `drivers/network/virtio_net` possède le contrôle de base du pilote VirtIO et un suivi de 256 buffers RX ;
- `kernel/src/syscall/net_bridge.rs` fait le pont entre l’ABI de sockets et `network_server` ;
- le pool actuel contient 256 pages RX et 256 pages TX de 4 Kio, soit 1 Mio par direction et 2 Mio au total ;
- la table de sockets actuelle est bornée à 64 entrées ;
- le `CapToken` existant est typé, porte une génération, et sa taille ABI est 24 octets ;
- le 29 juillet 2026, `cargo test -p exo-network-server --no-default-features` a validé 7 tests : DHCP, routage, ICMP, génération de handles de sockets et le modèle `exonet_stress`.

La pile `smoltcp` est adaptée au contexte bare metal : sa documentation la décrit comme sans allocation de tas et utilisable hors OS ([documentation smoltcp 0.12](https://docs.rs/crate/smoltcp/0.12.0)). Ce choix doit néanmoins être réévalué à chaque montée de version et non supposé correct par principe.

### 4.2 Limite qui bloque toute ambition de débit

**[OBSERVÉ]** Le bridge courant limite le message de données en ligne à `NET_INLINE_DATA_MAX = 128` octets. Il convient aux appels de contrôle et aux petits datagrammes ; il ne convient pas à un flux applicatif normal.

Le calcul ci-dessous est une borne arithmétique, sans compter les en-têtes, IPC, copies ou acquittements :

| Chemin | Unité utile | Soumissions minimales pour 1 Gbit/s | pour 10 Gbit/s |
|---|---:|---:|---:|
| Bridge inline actuel | 128 o | 976 563/s | 9 765 625/s |
| Une page | 4 Kio | 30 518/s | 305 176/s |
| Lot de 16 pages | 64 Kio | 1 908/s | 19 074/s |

**[DÉCISION]** Les 128 octets restent le canal de contrôle. Un chemin `BufferGrant` à pages prêtées est requis avant toute revendication de performance réseau.

La profondeur actuelle d’un pool par direction donne aussi une réserve théorique de `256 × 4096 = 1 048 576` octets : environ **8,39 ms à 1 Gbit/s**, **3,36 ms à 2,5 Gbit/s** et **0,84 ms à 10 Gbit/s**. C’est une profondeur utile pour le démarrage ; elle est trop faible pour promettre une absorption de rafales à 10 Gbit/s.

## 5. Les objets d’autorité

Tous les objets suivants ont un identifiant stable, une génération et une durée de vie définie. Un PID, une adresse MAC ou une adresse IP ne sont jamais, seuls, une identité d’autorité.

### 5.1 Sceau

Un **Sceau** est l’identité persistante d’un sujet : utilisateur, application signée, service déclaré, appareil administré ou administrateur. Il est résolu depuis un manifeste de démarrage ou un registre signé ; le PID courant n’est qu’une instance temporaire de ce sujet.

Un appareil LAN n’est pas rendu digne de confiance par son adresse MAC : elle sert à joindre la machine, non à l’authentifier. Tant qu’il ne présente pas une identité vérifiable par la politique locale, il reste dans le Cercle `quarantaine`.

### 5.2 Pacte

Un **Pacte** est une politique durable, versionnée et immuable après publication. Le changement produit une nouvelle génération, pas une modification silencieuse. Son contenu minimal est :

```text
Pacte {
  pact_id, generation, émetteur, sujet_ou_groupe,
  direction: sortie | entrée | transit_interne,
  destination_logique, transport, ports,
  contraintes_adresses, contraintes_dns_et_ttl,
  cercle_source, cercle_destination,
  classe_qos_max, débit, rafale, pages_max,
  connexions_max, expiration, exigences_de_chiffrement,
  journalisation, règle_de_reprise
}
```

`destination_logique` est ce que l’humain administre, par exemple `catalogue://erp` ou `external://api.exemple.tld:443`. La traduction vers une IP et un port est faite par l’Engine seulement après validation du Pacte. Une réponse DNS n’est acceptée que si elle appartient aux préfixes/ports permis, avec une génération et une durée de validité enregistrées ; cela évite qu’une résolution ultérieure ne change implicitement l’autorité.

### 5.3 Brin

Un **Brin** est le droit éphémère associé à une ouverture concrète : socket, flux, datagramme ou écoute. Il lie :

`Sceau du demandeur + Pacte + génération + endpoint local + endpoint distant + quotas + état`.

Il ne peut pas être réutilisé par un autre processus, même s’il obtient un descripteur de fichier copié. La duplication de FD doit transférer explicitement un Brin atténué et enregistré, ou échouer. Les états sont :

```text
Closed -> Opening -> Active -> Draining -> Closed
                     |            |
                     v            v
                 Restricted ----> Revoked
```

`Revoked` interdit immédiatement toute nouvelle émission et nouvelle ouverture. Les données déjà confiées au matériel suivent une règle explicite : terminer le descripteur déjà publié ou le retirer s’il n’est pas encore visible au matériel ; jamais une troisième sémantique ambiguë.

### 5.4 Lentille

Une **Lentille** est une capacité de lecture filtrée : une équipe peut voir les compteurs de son Cercle, sans lire les destinations d’un autre Cercle ni le contenu des flux. Les événements contiennent des identifiants pseudonymisés par défaut ; la résolution nominative est une autre capacité, délivrée seulement aux rôles autorisés.

### 5.5 Cercle, Enveloppe et Couronne

- Un **Cercle** est une zone nommée : `poste-utilisateur`, `production`, `administration`, `invité`, `quarantaine`, `internet`. Il sert à déclarer les frontières, pas à donner des privilèges automatiques.
- Une **Enveloppe** est une délégation administrative bornée : Cercles, objets, durée, actions et plafond de priorité autorisés.
- La **Couronne** est la racine de gouvernance, idéalement hors ligne ou protégée par une approbation de seuil. Elle délègue ; elle ne s’emploie ni pour exécuter une application ni pour administrer chaque flux quotidien.

## 6. Capacités et droits : intégration minimale au noyau

### 6.1 Principe

**[DÉCISION]** Ne pas inventer un deuxième système de jetons. Étendre le `CapToken` existant avec les types sémantiques `NetworkPact`, `NetworkSession` et `ObservationScope`, après extension de la preuve associée. Les droits existants sont interprétés selon le type :

| Objet | Lecture | Exécution/écriture | Délégation | Révocation |
|---|---|---|---|---|
| `NetworkPact` | consulter sa politique non secrète | `EXEC` ouvre un Brin conforme | seulement sous-ensemble | ferme les descendants |
| `NetworkSession` | recevoir dans les limites du Brin | émettre dans les limites du Brin | interdite par défaut | ferme le Brin |
| `ObservationScope` | lire les événements filtrés | aucune | uniquement une Lentille plus étroite | retire la visibilité |

Les droits ne doivent pas être codés dans le PID, le nom d’un service, la classe QoS ou la mémoire d’un processus. Le noyau impose les propriétés structurelles : type correct, génération valide, chaîne de délégation atténuée et révocation. L’Arbiter interprète le contenu riche du Pacte.

### 6.2 Chemin d’ouverture

```text
Application (Sceau) --PactCap/EXEC--> Engine --requête--> Arbiter
       ^                                      |              |
       |<------- BrinCap + fd lié ------------|<-- validation-|
                                              |
Portal <--- DMA/IRQ uniquement <------------- Engine
```

1. L’application demande une ouverture par l’API socket/ExoFS ou par l’API native.
2. Le bridge retrouve le `PactCap` attaché au Sceau et au contexte de l’application ; l’application ne choisit jamais librement une politique par son identifiant brut.
3. L’Arbiter contrôle le sujet, la destination, les limites et la génération courante ; il produit un Brin.
4. L’Engine crée le socket et lie le FD à ce Brin.
5. Chaque opération de données vérifie l’état du Brin et débite les quotas avant mise en file.

L’ABI de contrôle existante suffit à transporter un petit identifiant de requête et les métadonnées. Le contenu de Pacte, les chaînes de capacités et les listes de pages ne passent jamais dans la limite inline de 128 octets.

### 6.3 Ce que Kernel B vérifie réellement

Kernel B reçoit un enregistrement compact et immuable : `{policy_epoch, root_digest, commit_sequence}`. Il vérifie monotonicité, cohérence d’époque et validité des capacités sous sa responsabilité. Il ne recalcule pas un graphe d’entreprise ni ne prend de décision QoS sur le chemin de paquets.

Une divergence A/B force un état **Restricted** : nouvelles ouvertures suspendues, trafic d’administration d’urgence très borné, audit prioritaire. La reprise exige une génération de politique cohérente, jamais une simple suppression d’alerte.

## 7. Chemin de données : d’abord sûr, ensuite rapide

### 7.1 Deux plans, jamais confondus

| Plan | Transport | Taille | But |
|---|---|---|---|
| Contrôle | anneaux SPSC/IPC existants | petit message fixe, actuellement 128 o de données inline | open, close, bind, décision, révocation, statistiques |
| Données | `BufferGrant` fondé sur `MemoryRegion` | 1 à 16 pages, listes bornées | envoyer/recevoir des charges utiles sans une IPC par 128 o |

### 7.2 `BufferGrant` v1

Le chemin sûr de départ est le suivant :

1. l’application fournit une capacité `MemoryRegion` bornée en taille et en durée ;
2. le noyau confirme propriété, alignement, droits et absence de révocation ;
3. l’Engine obtient une vue transitoire de lecture (TX) ou écriture (RX) ;
4. l’Engine copie vers/depuis son pool DMA propriétaire ;
5. seul le Portal expose ces pages DMA au périphérique via son domaine IOMMU ;
6. la fin de l’opération libère le prêt, débite le quota et notifie l’application.

Le maximum initial est de 16 pages (64 Kio) par soumission et un nombre de prêts fixé par Pacte. Cela borne les listes, la mémoire épinglée et le travail de révocation.

**[PORTE]** Un DMA direct application→NIC ne peut être proposé qu’après preuve de non-réutilisation de page, retrait IOMMU terminé, annulation de descripteur et test de révocation concurrente. Avant cette porte, « zéro-copie » est un terme interdit dans la documentation produit.

### 7.3 VirtIO et batching

VirtIO expose des files de buffers ; les options de plusieurs files et de coalescence sont négociées avec l’appareil, elles ne doivent donc jamais être supposées présentes. La spécification VirtIO décrit notamment les conditions de coalescence des notifications et l’exigence de négociation de fonctionnalités ([VirtIO 1.3](https://docs.oasis-open.org/virtio/virtio/v1.3/virtio-v1.3.html)).

La progression est : une file sûre et instrumentée, puis lots de descripteurs, puis négociation des fonctionnalités utiles, puis plusieurs files seulement si les mesures indiquent que le verrou/IRQ/Engine est le goulot. Une option matérielle absente doit dégrader le débit, jamais les garanties d’autorité.

## 8. LAN, Internet, applications et services

### 8.1 Réseau local

Un service interne se publie sous une destination logique, par exemple `exo://production/inventaire`. Le catalogue associe ce nom à un ou plusieurs endpoints IP/port, mais seul un Pacte permet à un Sceau de le joindre.

Le réglage par défaut est :

- aucune découverte automatique entre Cercles ;
- aucun service entrant exposé sans Pacte d’entrée ;
- tout nouvel appareil arrive en `quarantaine` ;
- un administrateur peut l’enrôler dans un Cercle par identité vérifiée, puis seulement lui déléguer une Enveloppe ;
- ARP/DHCP sont des fonctions réseau nécessaires, non une permission de contacter des applications.

### 8.2 Internet

L’Internet utilise les protocoles standard. ExoNet agit à la frontière :

- sortie : `external://nom:port` est autorisé par un Pacte précis ;
- DNS : l’Engine résout au nom du Pacte, contrôle adresse, TTL et port avant ouverture ;
- entrée : une écoute TCP/UDP doit avoir un Pacte d’entrée, une limite de connexions et un Cercle de réception ;
- chiffrement : le Pacte peut exiger TLS applicatif, mais ExoNet v1 ne prétend pas transformer une connexion non chiffrée en connexion sûre. La vérification de l’identité distante doit être réalisée par la couche TLS/protocole concernée ou par une future intégration avec le service crypto.

Le moteur n’offre ni accès IP brut aux applications ordinaires, ni autorisation large de type « tout Internet ». Les outils de diagnostic reçoivent un Pacte séparé, temporaire, journalisé et révocable.

### 8.3 Compatibilité applicative

ExoFS expose la façade POSIX : `socket`, `connect`, `bind`, `listen`, `accept`, `send`, `recv` continuent d’exister. Leur comportement est enrichi par les erreurs explicites suivantes :

| Situation | Résultat pour l’application |
|---|---|
| aucun Pacte applicable | refus d’ouverture (`EACCES`/erreur ExoNet documentée) |
| limite de débit atteinte | attente non bloquante, `EAGAIN`, ou délai du Pacte |
| Brin révoqué | écritures refusées, lecture terminée conformément à la règle du Brin |
| Engine/Phoenix redémarré | socket rompue (`ECONNRESET`/`EPIPE`) ; la bibliothèque peut reconnecter si le protocole est idempotent |
| politique changée | les nouvelles ouvertures utilisent la nouvelle génération ; les Brins existants suivent la règle explicitement choisie |

L’application qui désire une reprise robuste doit posséder un protocole applicatif avec identifiants de requêtes, accusés de réception et réexécution sûre. ExoNet fournit une notification fiable de rupture, pas une fausse garantie de continuité TCP.

## 9. Priorités et quotas

### 9.1 Classes

Un Pacte fixe une classe maximale. Une délégation ne peut qu’abaisser sa priorité ou réduire ses limites.

| Classe | Exemples | Politique de service |
|---:|---|---|
| 0 — survie | récupération minimale, battement Phoenix | petite réserve stricte, plafond très bas pour ne jamais monopoliser le lien |
| 1 — contrôle | administration active, DHCP/ARP indispensable, révocation | priorité forte mais débit plafonné |
| 2 — interactif | shell, interface utilisateur, appels courts | faible latence recherchée |
| 3 — service | API de production, synchronisation métier | partage équitable entre Pactes |
| 4 — arrière-plan | mise à jour, sauvegarde, indexation | utilise le surplus, abandonnable |
| 5 — quarantaine | appareil non enrôlé, diagnostic limité | plafond dur et visibilité maximale |

### 9.2 Algorithme initial

Chaque Pacte possède un seau de jetons `{débit, rafale}` et une file bornée. Le principe est conforme au modèle de trafic à débit engagé et taille de rafale décrite par [RFC 2697](https://www.rfc-editor.org/rfc/rfc2697/). ExoNet l’emploie comme mécanisme local de contrôle, sans prétendre implémenter tout DiffServ.

L’ordonnanceur Engine applique :

1. contrôle de jetons par Pacte à l’admission ;
2. service strictement prioritaire seulement pour les classes 0 et 1, avec plafond ;
3. round-robin à déficit entre Pactes à l’intérieur des classes 2 à 5 ;
4. abandon contrôlé des classes de plus faible priorité lorsque les files sont pleines ;
5. compteurs séparés pour octets, paquets, abandons, délai de file et refus de capacité.

L’équité est testée par Pacte, non seulement par PID : un service ne doit pas contourner une limite en créant cent processus.

## 10. Administration d’entreprise sans escalade de privilèges

### 10.1 Rôles exprimés comme capacités

| Acteur | Enveloppe typique | Ce qu’il ne peut pas faire |
|---|---|---|
| Couronne / direction | crée les racines de politique, impose une approbation de seuil | exécuter quotidiennement avec la racine, contourner le journal |
| administrateur réseau | édite des Pactes dans ses Cercles et plafonds | se donner un Cercle, une priorité ou un droit absent de son Enveloppe |
| responsable de service | demande/publie un service dans un Cercle précis | autoriser d’autres Cercles ou élargir Internet |
| utilisateur | utilise les Pactes qui lui sont délégués | créer, déléguer, révoquer ou observer hors Lentille |
| appareil inconnu | accès de quarantaine éventuellement | voir les services, gagner une identité ou un droit par son adresse MAC |

Une Enveloppe contient explicitement la liste ou le préfixe de Cercles, les opérations (`proposer`, `approuver`, `révoquer`, `observer`), les plafonds de QoS et l’expiration. Chaque délégation est une atténuation vérifiée par le mécanisme de capacités : aucun sous-administrateur ne peut créer des droits qui ne sont pas contenus dans les siens.

Pour les modifications sensibles — exposition Internet, hausse de classe, changement de Couronne, suppression de journal — le flux est `proposition -> seconde approbation -> publication`. En mode développeur solo, la même personne peut signer les deux rôles, mais le journal doit marquer cette exception. Ce mode ne doit jamais être présenté comme une séparation de fonctions d’entreprise.

### 10.2 Administration quotidienne

La Console présente une vue orientée tâche :

```text
Créer un service « inventaire »
  Cercle : production
  Écoute : TCP 8443
  Clients : groupe « employés »
  Priorité : service
  Limite : 20 Mbit/s, rafale 512 Kio, 100 connexions
  Internet : interdit
  Journal : métadonnées et refus
```

La Console compile ce formulaire en Pacte lisible, affiche le diff de politique, demande les approbations nécessaires, puis transmet la publication à l’Arbiter. Elle ne parle jamais directement au pilote ou aux descripteurs DMA.

### 10.3 Audit

Chaque décision importante génère : `epoch, pact_id, génération, sujet, action, résultat, motif, compteur`. L’audit est append-only du point de vue de la Console ; sa conservation hors mémoire vive relève de la politique de stockage existante.

Une Lentille de direction visualise agrégats et alertes ; une Lentille de sécurité peut retrouver les identités selon son Enveloppe ; une Lentille de service ne voit que son Cercle. Les octets utiles des applications ne sont pas intégrés au journal réseau v1.

## 11. Résilience ExoPhoenix

Le contrat de reprise est volontairement honnête :

1. les Pactes publiés et les Enveloppes sont restaurés depuis leur état durable validé ;
2. les Brins sont invalidés ou revalidés par génération ;
3. les ressources DMA et files VirtIO sont reconstruites dans un ordre documenté ;
4. les sockets TCP en cours sont signalées comme rompues ;
5. les applications redemandent un Brin si leur Pacte est encore valide ;
6. Kernel B compare l’époque/résumé de politique avant de rouvrir les nouvelles connexions.

**[PORTE]** Une démonstration Phoenix réseau exige une panne injectée pendant trafic, l’absence de fuite de buffer/capacité, l’impossibilité d’émettre avec un ancien Brin, puis la reconnexion contrôlée. Un simple redémarrage à vide ne valide pas ce contrat.

## 12. Invariants à prouver et à tester

| ID | Invariant |
|---|---|
| N1 | Aucune émission ou réception applicative sans Brin actif. |
| N2 | Tout Brin est lié à un Sceau, un Pacte et une génération valides. |
| N3 | Une délégation est un sous-ensemble de l’autorité parente : Cercles, destinations, droits, durée, débit et priorité. |
| N4 | Révoquer un Pacte interdit toutes les nouvelles opérations de ses descendants. |
| N5 | Un FD dupliqué ne contourne pas la liaison sujet/Brin. |
| N6 | Une page DMA n’appartient simultanément qu’à un état autorisé : libre, prêtée à l’Engine, publiée au Portal ou complétée. |
| N7 | Portal ne possède aucun pouvoir de politique ; Arbiter ne possède ni IRQ ni DMA. |
| N8 | Un conflit d’époque A/B place le réseau dans un état plus restrictif, jamais plus permissif. |
| N9 | Une Lentille ne révèle pas d’événement hors de son filtre. |
| N10 | Les classes basses ne privent jamais les classes 0/1 de leur réserve, et les classes 0/1 ne peuvent monopoliser le lien au-delà de leur plafond. |

Le modèle TLA+ doit représenter `Pacte`, `Brin`, `BufferGrant`, `generation`, `owner`, `queue`, `PhoenixState` et `PolicyEpoch`. Les preuves déjà associées à `CapToken` sont étendues aux nouveaux types ; aucune phrase « formellement vérifié » ne peut couvrir l’Arbiter avant que son modèle et ses obligations soient effectivement intégrés au périmètre de preuve.

## 13. Stratégie de benchmark et estimations

### 13.1 Règle de mesure

Chaque résultat publie : révision Git, options de compilation, modèle CPU, fréquence, RAM, QEMU/version, backend réseau, MTU, nombre de files, affinité CPU, taille de message, nombre de flux, durée, p50/p95/p99, débit utile, paquets/s, CPU, abandons et erreurs de capacités. Sans ces métadonnées, un chiffre n’est pas comparable.

Le benchmark QEMU utilise un backend TAP/bridge contrôlé ; le NAT utilisateur QEMU est acceptable pour vérifier DHCP/connexion, pas pour conclure sur le débit. Les mesures matérielles doivent utiliser le même scénario, en séparant le débit de la NIC, celui de l’Engine et le coût de la politique.

### 13.2 Scénarios obligatoires

| ID | Scénario | Mesures et succès |
|---|---|---|
| B0 | tests de modèles et ABI | invariants N1–N10, structures de taille bornée, aucune allocation cachée |
| B1 | écho TCP/UDP, 1 flux puis 64 | connexion, fermeture, erreurs et absence de fuite de socket/page |
| B2 | flux soutenu 64, 512 et 1400 octets | débit, p99, CPU, appels/s ; comparer inline et `BufferGrant` |
| B3 | quatre Pactes concurrents, classes 1–5 | débit attribué, ratio d’équité, p99 interactif et abandon arrière-plan |
| B4 | révocation pendant B2/B3 | temps de décision, aucune nouvelle émission après révocation, nettoyage des pages |
| B5 | panne Engine et panne Kernel A pendant trafic | fermeture explicite, réinitialisation des buffers, nouvel open seulement après ré-arbitrage |
| B6 | adversaire | token forgé, génération obsolète, FD transmis, page réutilisée, DNS hors Pacte, saturation de files |

### 13.3 Objectifs chiffrés, sans les vendre comme résultats

| Étape | Objectif de planification | Lecture correcte |
|---|---|---|
| Inline courant | valider B1, pas viser le débit | l’arithmétique montre qu’un Gbit/s demanderait près d’un million de soumissions/s de 128 o avant surcoûts |
| `BufferGrant` v1 en QEMU/TAP | au moins 25 000 paquets/s de 1500 o utiles, soit ~300 Mbit/s utiles, sans violation N1–N10 | objectif de mise au point, dépendant fortement de l’hôte QEMU |
| `BufferGrant` v1 matériel 1 GbE | cible de 70 000 paquets/s de 1500 o utiles, soit ~840 Mbit/s utiles | objectif ambitieux de réception/transmission soutenue ; il ne constitue pas une promesse avant mesure |
| équité | aucun Pacte de même classe ne reçoit moins de 90 % de sa part théorique sur une fenêtre stabilisée | seuil à interpréter avec tailles de paquets et limites de seaux publiées |
| coût de sécurité | débit utile >= 90 % du même chemin avec politique pré-résolue, p99 augmenté de <= 10 % | objectif relatif, plus robuste qu’une microseconde absolue dépendante du CPU |
| révocation | aucune soumission admise après l’observation de la nouvelle génération | propriété de sûreté ; le délai observé est publié plutôt que masqué par un SLA arbitraire |

Le coût à chaud attendu d’un paquet ne comporte pas de signature cryptographique : comparaison de génération, état de Brin, compteur de quotas et mise en file. La cryptographie est réservée à la publication/validation de politique et aux protocoles applicatifs. Cette décision est essentielle : une signature par paquet détruirait à la fois les performances et la prédictibilité sans améliorer l’autorité déjà apportée par la capacité locale.

## 14. Plan d’implémentation

Les durées sont des ordres de grandeur pour une personne expérimentée, après prise en main du dépôt, avec accès à QEMU et à une machine de test. Elles comprennent tests unitaires et documentation de l’étape, pas une certification externe. L’incertitude est volontairement élevée car aucun débit de base E2E n’est encore mesuré.

| Phase | Livrable | Estimation | Porte de sortie |
|---|---|---:|---|
| 0 | état de référence : boot, QEMU/TAP, B0/B1, carte des capacités et du bridge | 2–3 semaines-personnes | scénario réseau réel reproductible et métriques vides documentées |
| 1 | types `NetworkPact`/`NetworkSession`/`ObservationScope`, extension de preuve et tests N1–N5 | 4–6 sem.-pers. | aucune escalade par délégation, PID ou FD dans les tests adversariaux |
| 2 | `net_policy_server`, modèle de Pacte, Brin, Cercle et API ExoFS de contrôle | 4–6 sem.-pers. | application autorisée/refusée selon Pacte ; révocation fonctionnelle |
| 3 | `net_admin_server`, Enveloppes, approbation, Lens et journal filtré | 4–6 sem.-pers. | administration sans accès direct au Portal ni au DMA |
| 4 | `BufferGrant` copié une fois, pages bornées, compteurs et B2/B3 | 6–10 sem.-pers. | amélioration mesurée contre inline, N6 maintenu sous charge |
| 5 | Phoenix réseau, résumé Kernel B, B4/B5, modèle TLA+ et campagnes adversariales | 5–8 sem.-pers. | panne injectée sans ancien Brin ni fuite de ressource |
| 6 | multi-files VirtIO, batching avancé et optimisation guidée par profils | 4–8 sem.-pers. | gain mesuré ; aucune régression de sécurité ou d’équité |

**Estimation totale : 25 à 47 semaines-personnes.** Pour un seul développeur, il est réaliste de prévoir des mois plutôt que des semaines calendaires, car la validation QEMU/matérielle et la preuve formelle imposent des boucles de correction. Les phases 4 et 6 ne doivent pas commencer avant que la phase 2 soit validée.

### 14.1 Fichiers et frontières de modification prévus

| Zone | Modification proposée |
|---|---|
| `kernel/src/security/capability/` | nouveaux types d’objet, règles de vérification/délégation/révocation et obligations de preuve |
| `kernel/src/syscall/net_bridge.rs` | conserver le contrôle inline ; ajouter un protocole de prêts de `MemoryRegion`, sans contourner l’ABI de capacité |
| `servers/network_server/` | migration progressive en Engine : liaison Brin↔socket, QoS, BufferGrant, reprise explicite |
| `drivers/network/virtio_net/` | Portal strict : négociation de fonctionnalités, lots, libération RX/TX et instrumentation ; aucune politique |
| `servers/net_policy_server/` | nouveau service Arbiter, compilation Pacte→Brin et registre de génération |
| `servers/net_admin_server/` | nouveau service Console, Enveloppes, approbations et diff de politiques |
| `servers/net_observe_server/` | nouveau service Lens, filtres et export de métriques sans contenu applicatif |
| `docs/` et tests | ABI, modèle TLA+, guide d’administration, benchmark reproductible et matrice d’invariants |

## 15. Ce qui doit rester hors de v1

- remplacement de TCP/IP par un protocole ExoNet sur le LAN ;
- migration transparente de connexions TCP après panne ;
- DMA direct depuis la mémoire applicative ;
- chiffrement magique du trafic applicatif ;
- politique exprimée par adresse MAC, PID ou nom de processus seulement ;
- découverte LAN qui attribue des droits par défaut ;
- « root réseau » interactif et omnipotent ;
- objectif de 10 GbE annoncé sans mesures B2/B3/B5 sur matériel représentatif.

## 16. Résultat attendu pour chaque public

| Public | Expérience obtenue |
|---|---|
| utilisateur | son application fonctionne par sockets habituelles ; les droits déjà reçus suffisent, aucun réglage IP ou pare-feu à comprendre |
| développeur de service | déclare une destination logique, les clients autorisés et un budget ; reçoit des erreurs de politique explicites |
| administrateur | administre des règles lisibles par Cercles, voit le diff, délègue dans son périmètre et ne peut pas s’auto-augmenter |
| direction | conserve la Couronne et peut imposer approbation, visibilité et révocation sans devenir un super-utilisateur quotidien |
| noyau | ne devient pas une pile réseau géante : il conserve la vérification des capacités, la propriété mémoire/DMA et la révocation |

## 17. Décision de démarrage

La première modification utile n’est pas une optimisation VirtIO. C’est la **phase 0** : établir un test QEMU/TAP de trafic réel, figer les métriques de base et documenter précisément la liaison actuelle `socket -> net_bridge -> network_server -> virtio_net`. Ensuite, la phase 1 introduit les types de capacité et les tests d’autorité ; seulement après, `BufferGrant` remplace la limite de 128 octets pour les données.

Ainsi, ExoNet devient un réseau utilisable pour LAN et Internet, compatible avec les applications, administrable dans une entreprise, et fidèle au modèle de capacités d’Exo-OS sans promettre des propriétés ou des performances qui n’ont pas encore été démontrées.

## Références de conception

- [RFC 9293 — Transmission Control Protocol](https://www.rfc-editor.org/rfc/rfc9293.html) : état et remise à zéro des connexions TCP.
- [RFC 2697 — Single Rate Three Color Marker](https://www.rfc-editor.org/rfc/rfc2697/) : modèle de débit engagé et rafale pour le policing local.
- [VirtIO 1.3](https://docs.oasis-open.org/virtio/virtio/v1.3/virtio-v1.3.html) : files de buffers, négociation et coalescence de notifications.
- [smoltcp 0.12](https://docs.rs/crate/smoltcp/0.12.0) : pile réseau sans allocation de tas pour environnement bare metal.
