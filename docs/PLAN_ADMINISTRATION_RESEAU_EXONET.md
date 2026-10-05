# Plan ExoNet — administration réseau par capacités

**Statut :** proposition d’architecture et feuille de route, à soumettre au consensus technique avant implémentation.  
**Périmètre :** connectivité LAN, applications et services ; administration, délégation, observabilité, résilience et interopérabilité.  
**Hors promesse :** ce document ne déclare pas la pile réseau actuellement opérationnelle sur matériel réel. Cette preuve devra venir des jalons de validation ci-dessous.

## Décision directrice

Construire **ExoNet Control Plane v1** autour de l’autorité par capacité déjà présente dans Exo-OS, et non autour d’un super-utilisateur réseau, d’ACL globales ou d’un second stack IP. Une application demande une connexion à un *service* ; elle ne reçoit qu’une capacité de flux bornée qui autorise cette opération précise. Le `network_server` reste le plan de données, le noyau reste le point de contrôle irréductible pour l’identité du processus et les capacités, et deux petits services Ring 1 ajoutent la politique et l’administration.

Le principe est compatible avec les modèles à capacités : une capacité relie un objet à des droits précis et des copies moins puissantes peuvent être produites à partir d’une capacité initiale. [Manuel seL4](https://sel4.com/Info/Docs/seL4-manual-latest.pdf) Il répond aussi au principe « ne pas accorder de confiance implicite à cause de l’emplacement réseau » de la [Zero Trust Architecture du NIST](https://csrc.nist.gov/pubs/sp/800/207/final).

## Point de départ vérifié dans l’arbre local

La copie de travail contient déjà une base ExoNet V4 :

- `servers/network_server/` contient un serveur `no_std`, une interface `smoltcp`, une table bornée de sockets, DHCP, routage, isolation Phoenix et des tests de modèle ; `smoltcp 0.12` est fourni par le workspace. Cette bibliothèque est explicitement conçue pour du bare metal sans allocation de tas. [Documentation smoltcp 0.12](https://docs.rs/crate/smoltcp/0.12.0)
- `kernel/src/syscall/net_bridge.rs` copie et valide les données utilisateur, puis fait des RPC IPC vers `network_server` pour les opérations de sockets ; les pilotes réseau `virtio_net` et `e1000` existent sous `drivers/network/`.
- `servers/network_server/EXONET_V4_AUDIT.md` et `tests/exonet_stress.rs` décrivent la propriété de possession des buffers RX/TX et le cycle Phoenix, mais ils ne remplacent ni un test QEMU récent ni une validation sur une carte réelle.

Conséquence : la première étape n’est **pas** une réécriture complète. Il faut d’abord stabiliser et mesurer ce chemin existant, puis y insérer l’autorisation de flux. Le protocole virtio est une contrainte matérielle réelle : un périphérique réseau possède des files RX/TX et le pilote ne peut modifier un buffer encore exposé au périphérique. [Spécification VirtIO 1.3, §5.1 et §3.3.1](https://docs.oasis-open.org/virtio/virtio/v1.3/virtio-v1.3.html)

## Architecture cible

```text
                         plan d’administration, jamais le plan de données
 Console locale / mTLS ───────────────► net_admin_server ─────────────┐
                                                   │ bundle signé       │
                                                   ▼                    │
                                            net_policy_server           │
                                      (PDP, délégation, révocation)     │
                                                   │ NetFlowCap          │
                                                   ▼                     │
 Application Ring 3 ─► net_bridge Ring 0 ─► network_server Ring 1 ─► virtio/e1000
       ServiceId            PEP n°1              PEP n°2             PEP DMA
       + CapToken        contrôle appel       contrôle du flux       IOMMU / driver cap
                                                   │
                                           LAN / services externes
```

### Rôles des composants

| Composant | Responsabilité | Ne doit jamais pouvoir faire |
|---|---|---|
| `net_bridge` (noyau) | Associer l’appel au PID et à son espace de capacités ; valider pointeurs, taille et référence de capacité ; créer un descripteur de flux opaque. | Interpréter une politique métier, détenir des clés d’administration ou transmettre un pointeur virtuel à Ring 1. |
| `network_server` | TCP/UDP/IP, gestion de flux, routage effectif, quotas et PEP d’exécution ; cache de décision borné. | Émettre une autorité nouvelle, modifier une politique, autoriser un DMA sans capacité pilote. |
| `net_policy_server` **(nouveau)** | Compiler une politique déclarative en décisions bornées ; demander au noyau la création, l’atténuation ou la révocation de `NetFlowCap`. | Transmettre des paquets, posséder les files virtio ou accepter une commande d’administration non authentifiée. |
| `net_admin_server` **(nouveau)** | Session d’administration, contrôle de rôle, validation des bundles signés, approbations à plusieurs personnes, journal d’audit. | Exécuter directement une mutation de driver ou de routage hors du chemin `net_policy_server`. |
| `crypto_server` | Opérations de signature, vérification, clés de service et, plus tard, primitives TLS. | Décider seul qu’un sujet peut accéder à un service. |

`net_policy_server` et `net_admin_server` sont les seuls nouveaux services proposés. Cela limite la surface de confiance et conserve les anneaux SPSC/IPC et les CapTokens comme mécanismes d’intégration.

## Le modèle d’autorisation de flux

### Une capacité, pas une permission ambiante

Le noyau introduit un objet opaque `NetFlowCap`. Il est infalsifiable, lié à un sujet, à une version de politique et à un budget. Sa charge logique est :

```text
subject = WorkloadId / PID lié
tenant  = entreprise ou unité
service = ServiceId ou zone autorisée
ops     = connect | bind | listen | resolve | observe | capture
match   = protocole, CIDR/port ou ServiceId, direction
limits  = expiration, débit, burst, connexions, octets, priorité maximale
epoch   = version de politique et génération de révocation
parent  = capacité parente facultative, profondeur de délégation
```

Le champ `match` peut être exprimé en attributs (sujet, ressource, opération et contexte), ce qui correspond au modèle ABAC décrit par le [NIST SP 800-162](https://csrc.nist.gov/pubs/sp/800/162/upd2/final). En revanche, la règle finale doit être compilée en structures **bornées** avant d’arriver dans le chemin chaud ; le noyau et le serveur réseau ne doivent pas évaluer du JSON ni un langage généraliste par paquet.

### Règles non négociables contre l’escalade

1. **Création réservée.** Seul le noyau crée un `NetFlowCap`, sur demande authentifiée de `net_policy_server` qui possède un `PolicyAuthorityCap` explicitement remis au démarrage. Aucun PID, administrateur réseau ou `network_server` ne peut fabriquer un jeton équivalent.
2. **Délégation uniquement par intersection.** Une capacité fille doit avoir un ensemble de droits, de destinations, de quotas et une durée strictement inclus dans ceux de son parent. La vérification est faite par le noyau et répliquée par tests de propriétés ; un champ libre « admin=true » n’existe pas.
3. **Pas d’héritage de privilège par descripteur.** Un descripteur POSIX ne transporte pas l’autorité à un autre processus. Après `fork`, passage IPC ou restauration Phoenix, le noyau lie de nouveau le flux au sujet et à la capacité autorisée ; sinon le flux est fermé.
4. **Révocation effective.** Chaque connexion cache `epoch` et son bail. Une hausse d’epoch invalide immédiatement les nouvelles opérations ; les flux ouverts sont soit drainés selon une règle explicitement choisie, soit arrêtés au prochain point de contrôle. Les droits administratifs critiques ne bénéficient pas d’une période de grâce.
5. **Contrôle aux quatre frontières.** L’ouverture est filtrée par `net_bridge`, le flux par `network_server`, le DMA par la capacité pilote/IOMMU, et la mutation de politique par `net_admin_server`. Une compromission Ring 1 ne doit donc pas devenir une émission d’autorité noyau.

Ces règles définissent exactement ce que signifie « un utilisateur X ne peut jamais monter au-dessus de son administrateur ». Elles ne prétendent pas empêcher un propriétaire humain autorisé de déléguer excessivement ; ce risque est traité par la gouvernance, les signatures et l’audit ci-dessous.

## Réseau local : usage quotidien et routage de services

### Espaces et identité

Un hôte possède un ou plusieurs `NetZone` : `management`, `production`, `build`, `guest` et `quarantine` sont les valeurs initiales recommandées. Une zone est liée à une interface/port/VLAN seulement par une politique racine ; une application n’obtient jamais un accès brut au réseau de niveau 2.

Les applications demandent `connect(ServiceId, port, protocol)` plutôt que « atteindre n’importe quelle IP ». Le répertoire de services résout un `ServiceId` en une ou plusieurs destinations locales ou LAN et fournit le `NetFlowCap` associé. Les accès IP/CIDR restent possibles pour l’interopérabilité, mais sont explicites et plus restrictifs.

Sur le LAN, Exo-OS démarre avec IPv4, TCP et UDP compatibles, et conserve DHCP uniquement comme mécanisme de configuration d’adresse. DHCP, ARP, adresse MAC, VLAN et adresse IP ne sont jamais des preuves d’identité ni des sources d’autorité. L’identité de service est fournie par la capacité et, lorsque la confidentialité ou l’authentification du pair est requise, par TLS 1.3 et des identités de service gérées par `crypto_server`. TLS 1.3 est le standard IETF conçu pour protéger contre l’écoute, l’altération et la contrefaçon de messages. [RFC 8446, remplacé par RFC 9846](https://www.rfc-editor.org/info/rfc8446/)

### Priorisation sans privilège caché

La priorité est un attribut de politique, jamais un paramètre contrôlé par l’application. `network_server` maintient des files bornées et une équité par `WorkloadId` :

1. contrôle de sûreté et messages Phoenix/driver ;
2. administration authentifiée et DNS/service-directory ;
3. services métier marqués critiques par la politique ;
4. trafic standard ;
5. invité, bulk et quarantaine.

Chaque classe a un plafond et un quota par sujet. Une application de classe basse ne peut ni se déclarer urgente ni affamer une classe haute. Les marques réseau externes, comme DSCP, sont optionnelles et posées seulement par une règle privilégiée ; elles ne sont jamais interprétées comme une autorisation reçue du LAN.

### Lecture, diagnostic et capture

- `NetObserveCap` donne des métadonnées filtrées : état du lien, compteurs, latence agrégée, erreurs et flux visibles dans les zones autorisées.
- La lecture de contenu, la capture et l’export de PCAP requièrent une `NetCaptureCap` séparée, à durée et volume bornés, avec justification et audit. Aucun rôle d’observation standard ne voit les payloads d’un autre tenant.
- Les applications n’obtiennent que leurs propres compteurs et erreurs ; elles ne peuvent ni lister les autres flux ni deviner les services internes par les messages d’erreur.

## Administration d’entreprise et gouvernance

La hiérarchie technique ne doit pas reprendre naïvement l’organigramme. Le « CEO » n’est pas un compte root connecté au LAN ; il peut être détenteur d’une clé de gouvernance, mais la racine opérationnelle demeure hors ligne ou protégée par un quorum.

| Rôle | Peut faire | Ne peut pas faire |
|---|---|---|
| **Conseil propriétaire** | Initialiser la racine, déléguer les limites maximales, récupération d’urgence à M-sur-N signatures. | Administrer les flux quotidiens depuis le réseau. |
| **Responsable sécurité** | Approuver une extension de périmètre, révoquer, lire l’audit global. | Créer seul une racine ou contourner le quorum. |
| **Administrateur réseau** | Appliquer une politique déjà dans son enveloppe, gérer les zones et diagnostics. | S’accorder un périmètre, un budget ou une durée supérieurs à son `AdminCap`. |
| **Propriétaire de service** | Déléguer à ses workloads des droits plus petits vers ses services. | Modifier une zone globale, observer ou capturer les autres tenants. |
| **Opérateur / auditeur** | Exploiter ou lire les métadonnées permises. | Changer les politiques, clés ou capacités. |
| **Workload** | Utiliser les flux qui lui sont remis. | Émettre, déléguer ou inspecter des capacités. |

Les changements sensibles — ouverture Internet, changement de zone de production, capacité de capture, hausse de quota administratif, modification de racine — exigent deux approbations distinctes, une fenêtre de validité et une entrée d’audit. Le bundle accepté est signé, versionné, lié à son hash et appliqué atomiquement : soit tout le nouveau graphe de délégation est cohérent, soit l’ancien reste actif.

L’administration distante utilise un chemin dédié `management`, une identité de machine et une authentification mutuelle. Le mode secours est une console locale ou un canal hors bande, pas une règle « allow any » sur le réseau de production. Cette séparation concrétise le modèle où décision et point d’application sont distincts, recommandé comme cadre de déploiement par [NIST SP 800-207](https://csrc.nist.gov/pubs/sp/800/207/final).

## Chemin de données et performance

Le chemin actuel transmet de petites requêtes de sockets par RPC IPC. Le plan ne prétend pas que ce format suffit aux gros `send`/`recv` : `net_bridge` limite aujourd’hui les données inline. La progression doit être la suivante :

1. **Sémantique correcte d’abord :** copie bornée et fragmentée par le noyau ; aucune adresse virtuelle d’un processus n’est envoyée à Ring 1.
2. **Puis `NetBufferGrant` si les mesures le justifient :** objet noyau temporaire, directionnel, de taille bornée et révocable. Le noyau mappe séparément la mémoire vers le demandeur et le serveur réseau, transmet un identifiant de capacité — jamais un pointeur — puis détruit le grant quand RX/TX est terminé.
3. **DMA conservé sous capacités :** `network_server` et le pilote ne reçoivent que les IOVA et buffers explicitement autorisés ; la possession RX/TX reste à un seul acteur jusqu’au retour de libération. Cette discipline est exigée par virtio pour les buffers exposés.

La décision de passer au grant se prend sur mesures : débit, p50/p99 de latence, pertes, CPU, saturation des anneaux et erreurs de propriété. Il n’y a ni objectif Gb/s annoncé ni promesse « zero-copy » avant un benchmark QEMU puis matériel reproductible.

QUIC est volontairement différé. Il apporte des flux multiplexés, contrôle de flux et migration de chemin, mais aussi un état de transport et une surface de test supplémentaires. [RFC 9000](https://www.rfc-editor.org/info/rfc9000/) Il sera un consommateur du modèle `NetFlowCap`, au-dessus d’UDP, seulement après TCP/UDP, révocation et Phoenix validés.

## Résilience ExoPhoenix

Le cycle existant `Normal → Draining → Serialized → Normal` est conservé, avec deux compléments :

- avant sérialisation, interdire les nouvelles ouvertures, borner le drain des files et enregistrer les identifiants de flux avec leur `epoch`, sans sérialiser de secrets de capacité en clair ;
- après reprise, revalider la politique et l’identité du sujet avant de réactiver un flux. En cas de doute, fermer proprement et forcer la reconnexion plutôt que créer une connexion « vivante » sans autorisation prouvée.

Le pilote ne doit pas réutiliser un buffer RX/TX tant que le propriétaire précédent n’a pas confirmé sa libération. C’est le point de jonction entre la sûreté mémoire/DMA et la fiabilité du réseau.

## Feuille de route avec portes de sortie

### J0 — Baseline factuelle et contrat d’interface

- Cartographier les chemins réellement construits : `net_bridge`, `network_server`, `virtio_net`, `e1000`, boot Ring 1, IOMMU et ExoPhoenix.
- Exécuter sous WSL les contrôles ciblés déjà présents, puis `make iso` et un smoke QEMU séparé pour le réseau. Consigner distinctement compilation, tests de modèle et trafic réellement observé.
- Écrire trois ADR : modèle de flux, révocation, et transport des gros buffers. Fixer les limites initiales (taille, socket, débit, délai de drain) avant toute optimisation.

**Sortie :** un test reproductible montre l’ouverture, l’échange, la fermeture et l’échec propre quand `network_server` ou le pilote est indisponible. Aucun changement d’architecture n’est accepté sur la seule base d’un document ancien.

### J1 — Capacité de flux minimale

- Ajouter les objets noyau `NetFlowCap`, `AdminCap` et les opérations `mint`, `attenuate`, `revoke`, sans attribuer de nouveaux numéros de syscall avant audit de la table actuelle.
- Étendre le message `net_bridge → network_server` avec un identifiant opaque de capacité, un sujet et l’epoch, puis appliquer le PEP au moment de `socket`, `connect`, `bind`, `listen` et `send`.
- Introduire un format de politique minimal : `tenant`, `subject`, `service/destination`, `opérations`, `limites`, `durée`, `priorité`. Tout le reste est refusé par défaut.

**Sortie :** les tests négatifs prouvent qu’un workload sans capacité, une copie falsifiée, une délégation élargie et un descripteur transféré échouent tous.

### J2 — Politique, administration et observabilité

- Créer `net_policy_server` et `net_admin_server` avec leurs CapTokens de démarrage minimaux.
- Compiler et signer des bundles atomiques ; implémenter la révocation par epoch et le journal d’événements chaîné/haché dans le mécanisme d’audit déjà retenu par le projet.
- Livrer une CLI locale simple : `net policy check`, `net flow list`, `net revoke`, `net audit verify`. Les commandes affichent toujours le sujet, la justification, les limites et l’expiration.
- Mettre en place `NetObserveCap` et `NetCaptureCap`, avec redaction par tenant et quotas de capture.

**Sortie :** un administrateur peut gérer son périmètre et voir son audit ; il ne peut pas accroître ce périmètre ni lire le trafic d’un autre tenant.

### J3 — LAN et services

- Déployer les zones `management`, `production`, `build`, `guest`, `quarantine`, avec routage `ServiceId → destinations` et règles d’entrée/sortie explicites.
- Démarrer avec IPv4/DHCP de configuration et TCP/UDP ; valider le DNS/service-directory contrôlé. Ajouter IPv6, 802.1X, multicast ou routage multi-hôte uniquement avec un modèle de menace et des tests dédiés.
- Ajouter les files de priorité et quotas ; comparer les mesures avant/après, sans régression des flux de contrôle.

**Sortie :** matrice e2e de communications autorisées et refusées entre zones, y compris un hôte LAN malveillant et une application compromise.

### J4 — Buffers performants et Phoenix

- Évaluer le transport copié ; n’implémenter `NetBufferGrant` qu’après modèle de propriété, revue IOMMU et tests de double-libération/fuite.
- Tester interruption du pilote, perte de lien, saturation, révocation pendant transfert, redémarrage de `network_server`, et résurrection ExoPhoenix.

**Sortie :** aucune réutilisation DMA prématurée, aucune élévation après restauration, et comportement de fermeture/reconnexion documenté.

### J5 — Assurance de sécurité et publication

- Ajouter un modèle TLA+ dédié aux graphes de délégation, epochs, buffers et Phoenix. Invariants : `NoAmbientNet`, `DelegationSubset`, `RevokedCannotOpen`, `SingleBufferOwner`, `NoPhoenixSplitBrain` et `PolicyApplyAtomic`.
- Tests de propriété/fuzzing pour parseurs Ethernet/IP/TCP/UDP, formats IPC, bundles et transitions de révocation ; test de non-régression sur QEMU et, ensuite, matériel NIC sélectionné.
- Révision externe du format de capacité, de la racine de confiance et de l’interface d’administration avant d’affirmer une propriété de non-escalade au-delà du modèle.

**Sortie :** preuves, limites de modèle, résultats de build, QEMU et matériel sont publiés séparément. Une réussite de compilation ne vaut jamais preuve de sécurité ou de résilience réseau.

## Confrontation aux quatre axes

| Axe | Choix | Risque traité |
|---|---|---|
| Sécurité | Capacité de flux infalsifiable, délégation par intersection, PEP multiples, révocation, IOMMU/DMA séparés. | Escalade, autorité ambiante, lecture inter-tenant, abus d’un pilote ou d’un admin. |
| Simplicité d’usage | `ServiceId` plutôt que règles IP pour l’application ; quelques rôles lisibles ; CLI qui explique la décision. | Réseau administrable seulement par experts, règles opaques. |
| Fiabilité | Files et budgets bornés, refus par défaut, reprise Phoenix qui revalide, décision atomique. | Blocage global, état de flux incohérent, reprise plus permissive que l’état initial. |
| Performance | Autorisation lors de l’ouverture/caches par epoch, pas de RPC politique par paquet, buffers grants seulement après mesure. | Le contrôle d’accès devient un goulot d’étranglement ou l’optimisation casse l’isolation. |

## Décisions explicitement reportées

- QUIC/HTTP/3, IPv6, Wi-Fi, 802.1X, BGP, routage multi-hôte et interception TLS ne font pas partie de v1.
- Aucun DPDK, XDP/eBPF ou mécanisme Linux n’est importé dans le chemin bare-metal `no_std`.
- Aucun accès brut aux sockets, au packet capture ou au DMA n’est accordé pour « dépanner » sans capacité, expiration et audit.
- Une politique de secours ne signifie jamais `allow all` ; elle réduit le système à la console locale et aux flux de récupération explicitement signés.

## Références externes vérifiées

- [Dépôt Exo-OS — architecture, capacités, ExoShield et ordre de démarrage](https://github.com/darkfireeee/Exo-OS)
- [VirtIO 1.3 — périphérique réseau, virtqueues et propriété des buffers](https://docs.oasis-open.org/virtio/virtio/v1.3/virtio-v1.3.html)
- [smoltcp 0.12 — pile bare-metal sans heap](https://docs.rs/crate/smoltcp/0.12.0)
- [NIST SP 800-207 — Zero Trust Architecture](https://csrc.nist.gov/pubs/sp/800/207/final)
- [NIST SP 800-162 — Attribute Based Access Control](https://csrc.nist.gov/pubs/sp/800/162/upd2/final)
- [seL4 manual — droits et atténuation de capacités](https://sel4.com/Info/Docs/seL4-manual-latest.pdf)
- [RFC 8446 / TLS 1.3](https://www.rfc-editor.org/info/rfc8446/)
- [RFC 9000 / QUIC](https://www.rfc-editor.org/info/rfc9000/)
