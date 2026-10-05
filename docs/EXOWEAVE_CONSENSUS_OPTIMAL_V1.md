# ExoWeave v1 — synthèse de consensus pour le réseau d’Exo-OS

**Statut :** proposition de référence à discuter avant toute implémentation.  
**Méthode :** synthèse des sept documents fournis localement ; aucune recherche Internet effectuée pour ce document.  
**But :** obtenir le meilleur compromis global entre sécurité, simplicité d’usage, fiabilité et performance, sans promettre ce qui n’a pas encore été démontré sur Exo-OS.

## Décision en une phrase

ExoWeave fait du réseau un **tissu de relations autorisées** : une application demande une relation métier nommée, la politique la transforme en une autorité limitée, puis le moteur réseau ouvre une session éphémère vers un transport local ou LAN ; ni l’adresse, ni le pilote, ni un privilège global ne deviennent l’autorité de l’application.

La conception est volontairement riche. L’usage se limite à cinq noms : **Cercle, Service, Pacte, Brin, Lentille**.

```text
Administrateur : « Le cercle Finance peut joindre le service Banque pour payer. »
Application    : « Ouvre un brin vers weave://banque/paiement. »
Système        : « Accordé ou refusé ; voici le motif et l’expiration. »
```

## 1. Ce qui est retenu, corrigé ou écarté

Les contributions convergent fortement sur le modèle à capacités, la séparation matériel/politique, les sessions bornées, les anneaux SPSC et la façade POSIX. Le plan conserve ces acquis, mais ne mélange pas leurs contradictions.

| Idée rencontrée | Décision ExoWeave | Raison |
|---|---|---|
| Unifier IPC local, appareil et réseau distant sous un Conduit | **Retenue au niveau de l’API**, pas comme réécriture immédiate de tous les ABI. | L’utilisateur raisonne avec une seule relation ; les transports existants restent spécialisés et migrent progressivement. |
| Pacte durable + brin/session éphémère | **Retenue.** | Permet de fermer une session suspecte sans retirer toute l’autorisation, et de révoquer un Pacte pour fermer tous ses brins. |
| Identité logique plutôt qu’adresse IP interne | **Retenue.** | Les adresses deviennent une traduction de Portail ; les services ne dépendent plus de la topologie. |
| Annuaire « aveugle » | **Retenue.** | Une Cellule ne découvre que les Services qu’elle est autorisée à découvrir ; un refus ne confirme pas l’existence d’une cible. |
| Cercles et langage déclaratif d’intention | **Retenue.** | C’est l’interface adaptée aux humains ; le compilateur absorbe la complexité des capacités. |
| Effilochage automatique de confiance | **Retenu, modifié.** | L’autorité doit expirer et se réduire en cas d’anomalie ou de perte d’attestation, mais jamais sur la seule inactivité d’un service légitime. |
| Budget unique CPU + mémoire + réseau | **Écarté.** | Une monnaie unique rend les ressources difficilement prévisibles. Les budgets réseau restent distincts, tout en étant portés par le même Pacte. |
| Prêt de bande passante entre autorisations | **Retenu pour le surplus élastique seulement.** | Les réserves de survie, contrôle et trafic engagé ne sont jamais prêtées. |
| Vessels/rings persistants en Ring 0 et reprise TCP transparente | **Écarté du socle.** | Cela agrandirait fortement le TCB noyau et promettrait une transparence de reprise non démontrée. La reprise est sûre avant d’être transparente. |
| ExoFabric, protocole LAN propriétaire immédiat | **Différé.** | Le premier déploiement doit rester interopérable avec le LAN existant. Une Fabric native peut devenir une phase ultérieure, derrière des Portails. |
| Zéro-copie de bout en bout dès le départ | **Objectif conditionnel, non fondation.** | La première obligation est la propriété mémoire/DMA sans ambiguïté ; les `BufferGrant` ne viennent qu’après mesures et modèle de sûreté. |
| Vérificateur structurel indépendant | **Retenu sous le nom WeaveVigie.** | Il contrôle périodiquement le graphe réel de délégation, avec travail borné, sans remplacer le contrôle noyau du chemin d’exécution. |

## 2. Le modèle mental d’usage

### Cercle

Un **Cercle** est une réalité de l’entreprise : `Finance`, `Production`, `Invités`, `Administration`, `Partenaire-Banque`. Il contient des membres et définit une enveloppe maximale de relations admissibles. Un Cercle n’est ni un VLAN, ni un CIDR, ni un groupe Unix ; ces détails peuvent être utilisés sous le capot par un Portail.

### Service

Un **Service** est une Cellule publiable dans un Cercle. Son nom est stable et lisible :

```text
weave://finance/paie
weave://banque/paiement
weave://imprimerie/laser-3
```

Un nom n’est résolu que si le demandeur détient un droit de découverte. Une même intention peut être résolue vers une instance locale, une autre machine LAN ou un adaptateur externe sans modifier l’application.

### Pacte

Le **Pacte** est l’autorisation durable, immuable et auditée : « qui peut établir quelle relation, dans quelle direction, avec quelles limites ». Il ne transporte aucune donnée.

### Brin

Le **Brin** est la session vivante dérivée d’un Pacte : un flux local, TCP, UDP ou un transport ultérieur. Il est court, lié à son sujet, à une génération de politique et à un budget. Il est le seul objet consulté sur le chemin chaud.

### Lentille

Une **Lentille** est un droit de lecture autonome : santé, topologie autorisée, métadonnées ou contenu. Posséder un Pacte n’accorde aucune Lentille ; posséder une Lentille n’accorde aucune action.

## 3. Architecture de référence

```text
                                  Couronne
                          (gouvernance hors ligne)
                                      │
                          approbations et signatures
                                      │
  exosh / UI ───────────────► weave_admin ─────► weave_control
                                  │                  │
                             journal scellé     compiler Pactes,
                                                annuaire, révocation
                                                       │
Application ─► façade Conduit/POSIX ─► noyau ─► weave_engine ─► weave_portal
          CapToken + demande             │        données          pilote/NIC
                                        Brin
                                                       │
                                                weave_observe
                                                (Lentilles seules)
```

### 3.1 Le noyau : petit, déterministe, non négociable

Le noyau ne traite ni IP, ni TCP, ni politique métier. Il ajoute seulement les objets de sécurité dont les autres services ont besoin :

| Objet noyau | Usage |
|---|---|
| `PactCap` | Référence à l’autorisation durable et à sa chaîne de parents. |
| `BrinCap` | Session éphémère liée à un sujet, un Pacte, une génération et des limites. |
| `LentilleCap` | Droit de lire une vue déterminée du système. |
| `EnvelopeCap` | Plafond administratif d’un opérateur ou propriétaire de service. |
| `BufferGrantCap` | Prêt de mémoire directionnel, borné et révocable ; optionnel avant la phase performance. |

Le noyau est la seule autorité qui crée ces objets. Ring 1 peut demander une création mais ne peut jamais fabriquer, réécrire ou élargir une capacité. La délégation compare des ensembles explicites : opérations, cibles, direction, priorité maximale, budgets, durée, visibilité et droit de redélégation.

### 3.2 Les services Ring 1

| Service | Responsabilité | Ce qu’il ne peut pas faire |
|---|---|---|
| `weave_portal` | Possède la capacité matérielle du pilote, les IRQ et les buffers DMA autorisés ; traduit entre NIC et descripteurs de paquets. | Décider qu’un Service peut communiquer, publier une politique, créer une identité métier. |
| `weave_engine` | Fait vivre les Brins, les protocoles L3/L4 et les files de priorité ; utilise les Sceaux de Brin déjà vérifiés. | Émettre un Pacte, modifier une Enveloppe ou posséder la racine d’administration. |
| `weave_control` | Compile les Pactes, maintient l’annuaire autorisé, alloue les Brins et publie les générations de politique. | Accéder au matériel, envoyer des paquets ou lire le contenu des flux. |
| `weave_admin` | Reçoit les intentions humaines, calcule l’impact, coordonne le quorum et soumet un bundle atomique. | Appliquer seul une politique ou s’augmenter lui-même. |
| `weave_observe` | Reçoit événements et métriques filtrés, prépare des vues pour les Lentilles. | Injecter, modifier, ralentir ou autoriser du trafic. |
| `WeaveVigie` | Vérifie par périodes bornées la cohérence du graphe effectif des capacités et des générations. | Émettre une autorité ; il ne peut que alerter ou demander une contraction de sécurité. |

`network_server` est la base de migration naturelle de `weave_engine`. Les pilotes VirtIO/e1000 deviennent l’implémentation initiale de `weave_portal`. Cette migration garde le code de transport et isole progressivement la politique, au lieu de jeter une pile réseau déjà présente.

### 3.3 Les quatre plans séparés

1. **Plan de données** : `weave_engine ↔ weave_portal`. Il traite des descripteurs, buffers, compteurs et Sceaux de Brin ; aucune règle humaine n’y est interprétée.
2. **Plan de contrôle** : `weave_control`. Il résout les noms, compile les Pactes, ouvre les Brins et propage des versions atomiques.
3. **Plan de gouvernance** : `weave_admin` et Couronne. Il formule une intention, la simule, l’approuve et la signe.
4. **Plan d’observation** : `weave_observe`. Il est hors du chemin de transmission et incapable d’action.

Cette séparation est plus importante que le nombre exact de processus. Des services peuvent temporairement être regroupés durant un prototype, à condition que leurs capacités restent séparées et que le regroupement ne devienne pas une autorité implicite.

## 4. Structure d’un Pacte et d’un Brin

```text
Pacte {
  origine             : Cellule ou Cercle porteur
  cible               : Service, Cercle, Portail ou adaptateur de frontière
  action              : appeler | publier | répondre | relayer | découvrir
  sens                : sortie | entrée | bidirectionnel
  transports          : local | TCP | UDP | adaptateur POSIX
  frontières          : Portails et Royaumes autorisés
  budgets             : débit, burst, connexions, volume, nombre de Brins
  priorité_max        : jamais plus haute que celle du parent
  fenêtre             : début, expiration, calendrier optionnel
  découverte          : invisible | nom seul | nom et santé
  exigences_pair      : aucune | identité de Service | canal protégé
  visibilité          : identifiants de Lentilles possibles
  parent              : référence noyau au Pacte parent
  génération          : version de politique
  délégation          : interdite | réduction seulement
}

Brin {
  pacte                : référence au Pacte
  sujet                : Cellule/PID auquel il est lié
  chemin               : local ou Portail choisi
  budget_restant       : compteurs propres au Brin
  expiration           : au plus égale à celle du Pacte
  mode_fin             : drainer | arrêter immédiatement
  génération           : génération observée à l’ouverture
}
```

Le Pacte peut autoriser une famille finie de cibles ; le Brin ne concerne qu’une relation effective. Ainsi, un service peut être autorisé à joindre `weave://banque/paiement`, mais le Brin concrètement ouvert est lié à l’instance sélectionnée et à son Portail.

## 5. Chemins de fonctionnement

### 5.1 Appel local entre services

1. `service_A` demande un Brin vers `weave://production/facturation`.
2. `weave_control` vérifie son droit de découverte puis l’existence d’un Pacte compatible.
3. Le noyau crée un `BrinCap` lié à `service_A`, au Pacte et à la génération actuelle.
4. `weave_engine` résout la cible vers l’IPC local existant ; aucun paquet IP ni accès NIC ne sont nécessaires.
5. À l’expiration ou à la révocation, les nouvelles écritures sont refusées et le Brin est drainé ou arrêté selon le Pacte.

### 5.2 Sortie LAN ou Internet

1. L’application demande un Service logique, jamais une carte réseau brute.
2. Le Contrôle sélectionne un Portail autorisé et une traduction IP/port si le Pacte la permet.
3. Le Moteur associe le Brin à ce chemin ; le Portail reste l’unique détenteur du droit matériel.
4. Les données traversent les files de priorité puis le pilote ; l’application ne peut ni choisir une source arbitraire, ni émettre un paquet brut hors du Brin.

### 5.3 Entrée depuis le LAN

1. Tout trafic entrant arrive au Portail dans l’état **non fiable**.
2. Le Portail ne route que vers une Promesse de Service explicitement publiée par un Pacte d’entrée.
3. Le pair est représenté comme une Cellule distante limitée au Brin entrant ; il ne reçoit aucune découverte interne ni capacité d’administration.
4. Les paquets sans Promesse, hors limite ou durant une génération révoquée sont rejetés avant d’atteindre un Service.

### 5.4 Compatibilité POSIX

La façade ExoFS garde `socket`, `connect`, `bind`, `listen`, `send` et `recv` comme interface de compatibilité. Elle ne crée pas d’autorité :

```text
connect(fd, destination) → demande de Brin → vérification Pacte → fd lié au BrinCap
```

Une duplication ou un passage de descripteur ne propage pas implicitement le droit. Le noyau impose soit un transfert explicite de capacité atténuée, soit l’échec du nouvel usage.

## 6. LAN, Cercles et découverte

Les Cercles initiaux d’une entreprise peuvent être :

```text
Couronne entreprise
├── Administration      consoles, récupération et gouvernance
├── Production          services métier publiés
├── Développement       CI, essais et environnements temporaires
├── Collaboration       postes et outils de travail
├── Partenaires         relations limitées vers des tiers connus
├── Invités             sortie publique contrôlée
└── Quarantaine         appareil inconnu, diagnostic et enrôlement seulement
```

Les Cercles ne sont pas une topologie. Ils sont des frontières d’autorité. Un Portail peut les matérialiser en adresses, segments, files ou règles de traduction, mais une configuration physique ne peut jamais créer une autorisation logique.

### Annuaire autorisé

Le Registre maintient les Promesses de Service, mais répond selon la Lentille de découverte du demandeur :

- sans droit : même réponse que pour un nom absent ;
- droit minimal : nom et type de Service ;
- droit de connexion : nom, cibles admissibles et Pacte applicable ;
- droit d’exploitation : santé et capacité agrégée ;
- droit d’administration : graphes de dépendance dans son Enveloppe seulement.

Une application ne peut donc pas scanner l’espace des services ni confondre découverte et droit de connexion.

### Enrôlement d’appareils

Un appareil local est **quarantiné par défaut**. Son apparition sur un câble, son adresse MAC, une réponse DHCP ou une trame ne suffisent pas à le placer dans un Cercle. Un opérateur propose son enrôlement ; une autorité adéquate l’approuve ; un Pacte d’appareil limite ensuite sa découverte, ses Brins et ses budgets.

## 7. Priorité, quotas et équité

La priorité est une propriété d’un Pacte, attribuée par son parent. Une application ne peut ni réclamer une priorité plus haute, ni transformer un budget de fond en trafic de contrôle.

| Classe | Usage | Règle |
|---|---|---|
| Survie | signaux de sûreté, libération DMA, coordination de reprise | Réserve non prêtée. |
| Contrôle | révocation, administration, annuaire, politique | Réserve non prêtée ; volume borné. |
| Interactif | console, assistance, UX sensible à la latence | Budget court et plafonné. |
| Engagé | service métier avec objectif de disponibilité | Part réservée par Pacte. |
| Élastique | trafic normal susceptible d’emprunter du surplus | Emprunt seulement dans le même Cercle et jusqu’à une échéance courte. |
| Fond | sauvegarde, synchronisation, mise à jour | Cède aux classes précédentes. |
| Quarantaine | appareil non approuvé ou diagnostic | Plafond strict et aucun emprunt. |

`weave_engine` applique des files bornées, une équité par Brin dans une classe, et un budget réseau distinct pour chaque Pacte. Le surplus élastique peut être prêté, mais une révocation ou le retour d’un besoin réservé le reprend immédiatement. Cette règle conserve l’intuition d’ExoFlux sans rendre le trafic critique imprévisible.

## 8. Administration, délégation et impossibilité d’auto-élévation

### 8.1 La Couronne et les Enveloppes

La **Couronne** est la racine de gouvernance. Ce n’est ni un utilisateur connecté ni une capacité disponible dans une application. Elle est fractionnée entre plusieurs dépositaires définis par l’entreprise.

Une **Enveloppe** est le plafond d’un administrateur : Cercles administrables, opérations, budgets, priorités maximales, Lentilles et durée de délégation. L’administrateur peut travailler dans son Enveloppe ; il ne peut ni la modifier, ni créer un pair, ni se déléguer une Enveloppe plus large.

| Acteur | Peut | Ne peut jamais |
|---|---|---|
| Couronne | définir les plafonds, désigner les signataires, récupération d’urgence sous quorum | opérer quotidiennement avec une autorité générale. |
| Responsable sécurité | approuver l’élargissement inter-Cercle, révoquer globalement, lire l’audit mandaté | créer seul une nouvelle Couronne. |
| Administrateur réseau | appliquer une politique incluse dans son Enveloppe, exploiter les Portails autorisés | s’élargir, augmenter sa priorité plafond ou lire le contenu sans Lentille. |
| Propriétaire de Service | publier et déléguer des Pactes plus étroits pour son Service | franchir une frontière ou administrer un autre Cercle. |
| Auditeur | consulter les événements couverts par une Lentille | créer, modifier, injecter ou bloquer un Brin. |
| Utilisateur / workload | utiliser ses Brins et, exceptionnellement, les réduire | créer un Pacte ou voir hors de son périmètre. |

### 8.2 Un langage qui ne sait pas élever

L’administrateur ne modifie pas un tableau de permissions brut. Il soumet une intention déclarative qui est compilée sous une Enveloppe donnée :

```text
autoriser Finance/paie
  à appeler Banque/paiement
  via Partenaires
  pour règlement
  avec classe Engagé, débit 2 MiB/s, expiration 90 jours
```

Le compilateur n’a pas d’opération « augmenter », « devenir Couronne » ou « prendre les droits de X ». Il ne sait produire qu’une intersection entre l’intention et l’Enveloppe signée du demandeur. Une intention qui dépasse l’Enveloppe ne se compile pas ; elle ne devient jamais une règle partiellement active.

### 8.3 Cycle d’un changement

1. **Proposition** : un opérateur ou propriétaire de Service déclare son intention.
2. **Simulation** : `weave_admin` calcule la différence d’autorité, les Brins affectés, les nouvelles frontières et le rayon d’impact.
3. **Approbation** : une ou plusieurs signatures sont exigées selon la sensibilité du changement.
4. **Compilation** : `weave_control` prouve l’inclusion dans les Enveloppes et produit un bundle de Pactes immuables.
5. **Activation atomique** : une nouvelle génération devient visible en une fois ; l’ancienne reste cohérente jusqu’au basculement.
6. **Journal et révision** : le changement, son ascendance et ses approbations deviennent observables via Lentille.

Une hausse d’autorité, une ouverture Internet, une capture de contenu, un changement de Cercle de production ou une capacité de Portail nécessitent un quorum plus fort. Un changement de réduction de droits peut être immédiat et unilatéral dans l’Enveloppe adéquate.

### 8.4 Urgence sans porte dérobée

Le mode urgence crée un Pacte de secours, jamais une autorité générale. Il est limité à une cible, une durée, un volume, une justification et un quorum ; il s’autodétruit. Le compte qui l’utilise ne conserve aucune augmentation de droit après l’échéance.

## 9. Les défenses contre l’escalade

Le but n’est pas de prétendre aujourd’hui à une impossibilité mathématique non prouvée. Le but est de construire une propriété qui pourra être prouvée et testée à quatre niveaux indépendants :

1. **Noyau** : une capacité fille est un sous-ensemble matériellement vérifié de son parent ; les capacités sont liées à un sujet et non utilisables après changement de sujet.
2. **Compilateur** : la grammaire et le contrôleur d’Enveloppe rejettent toute intention hors plafond, tout cycle de délégation et tout changement d’auto-élévation.
3. **Exécution** : le Brin porte l’identifiant du Pacte, du sujet et de la génération ; `weave_engine` refuse ce qui ne correspond pas.
4. **Surveillance** : WeaveVigie vérifie en arrière-plan, par fragments bornés, que le graphe réellement chargé conserve l’ascendance, la monotonie, les budgets non négatifs et l’absence de cycles. Une anomalie demande immédiatement la contraction/révocation, puis l’investigation ou la réinitialisation ciblée.

Le confinement matériel IOMMU et les capacités pilote sont une cinquième frontière : même un composant de données compromis ne reçoit pas la capacité de décider la politique de l’entreprise ni d’accéder à une ressource matérielle non autorisée.

## 10. Lentilles, audit et confidentialité

L’observation est fournie à partir d’événements de contrôle et de métriques, jamais par une capture globale implicite.

| Lentille | Visible | Exclu par défaut |
|---|---|---|
| Santé | disponibilité, erreurs, pression de files agrégée | identité détaillée, contenu. |
| Opération | Brins de son Service, quotas et motifs de refus | autres Cercles, contenu. |
| Audit | Pactes, ascendance, changements et décisions dans le périmètre mandaté | payload, secrets de session. |
| Capture exceptionnelle | échantillon de contenu ou en-têtes définis par le Pacte | tout hors cible, toute persistance au-delà de l’expiration. |

L’audit inscrit : proposition, approbation, compilation, activation, ouverture de Brin, révocation, transition Phoenix, violation et fin. Les données de contenu ne sont pas journalisées par défaut ; une capture doit être un Pacte séparé, borné dans le temps et le volume.

Le plan de données n’attend jamais l’écriture d’un journal. Il pousse des événements dans une file bornée ; en cas de saturation, la politique choisit explicitement entre agrégation, pression sur le flux ou refus de la nouvelle opération. Il n’existe pas de perte silencieuse d’un événement de sécurité critique.

## 11. Effilochage et résilience

### 11.1 ExoFray, version sûre

L’effilochage ne dépend pas de l’inactivité normale. Il s’active seulement sur : expiration contractuelle, perte d’identité/attestation attendue, détection d’anomalie, dépassement répété de budget ou retrait d’un Cercle.

Il suit une pente déterministe définie dans le Pacte :

```text
Normal → restreint (plus de nouveaux Brins) → drain (réponses seulement) → révoqué
```

Le système automatique peut uniquement **réduire** une autorité ; tout élargissement nécessite une nouvelle génération signée. Un faux positif peut donc réduire la disponibilité, mais ne peut jamais ouvrir un nouveau chemin d’accès.

### 11.2 Panne de contrôle, moteur ou Portail

| Panne | Comportement |
|---|---|
| `weave_control` indisponible | Aucun nouveau Brin ni renouvellement ; les Brins actifs continuent seulement jusqu’à leur bail court et leurs limites existantes. |
| `weave_engine` indisponible | Les Brins affectés passent à `drain` ; la reprise ne suppose pas que l’état TCP est récupérable. |
| `weave_portal` indisponible | Les Brins de ce Portail sont notifiés puis arrêtés ou migrés vers un Portail déjà admis par leur Pacte. |
| `weave_observe` indisponible | Les données ne deviennent pas plus permissives ; la politique de journalisation critique décide si les nouveaux changements sont suspendus. |
| ExoPhoenix | Les candidats à reprise sont ré-arbitrés avec la génération courante ; en cas d’ambiguïté, la session est fermée et doit se reconnecter. |

Cette règle choisit la sûreté et la cohérence plutôt qu’une fausse promesse de reprise transparente de toutes les connexions.

## 12. Performance : ordre des décisions

1. **Correctitude et possession** : descripteurs fixes, files bornées, un unique propriétaire de chaque buffer, retour explicite avant réutilisation.
2. **Chemin établi court** : le Brin validé est consulté ; aucune analyse d’intention, signature de politique ou recherche d’annuaire par paquet.
3. **Batching et priorité** : opérés dans le Moteur et le Portail, sous les limites du Pacte.
4. **`BufferGrantCap`** : seulement après benchmark. Il mappe un buffer par direction et durée, sans transfert de pointeur virtuel entre processus ; son cycle de vie est vérifié comme une capacité.
5. **Optimisations avancées** : multiqueue, offloads, polling, Fabric native ou filtres spécialisés seulement après preuve de gain et conservation des invariants.

Cette séquence retient l’ambition des propositions zéro-copie, mais refuse de faire de l’optimisation une prémisse de sécurité.

## 13. Invariants formels prioritaires

| Nom | Énoncé |
|---|---|
| `NoAmbientBrin` | Toute émission ou réception de données correspond à un Brin valide. |
| `ChildSubsetParent` | Les droits, cible, budget, durée et priorité d’un enfant sont inclus dans ceux de chaque parent. |
| `NoSelfExpansion` | Aucun acteur ne peut modifier l’Enveloppe dont il dépend, créer un pair ou fermer un cycle de délégation. |
| `HiddenMeansHidden` | Sans Lentille de découverte, un Service interdit est indistinguable d’un nom absent. |
| `RevokeStops` | Après transition de révocation, aucune nouvelle opération interdite n’est acceptée ; le drainage respecte le mode du Pacte. |
| `OneBufferOwner` | Un buffer RX/TX/Grant n’a qu’un propriétaire actif ; il ne peut être réutilisé avant confirmation de libération. |
| `RecoveryDoesNotExpand` | Après Phoenix, aucun Brin ne possède plus de droits, durée ou budget que le Pacte actuellement valide. |
| `ObserveCannotAct` | Une Lentille ne permet jamais création, modification ou injection de données. |
| `PolicyActivateAtomic` | Une génération de politique est entièrement ancienne ou entièrement nouvelle, jamais mixte pour un Brin. |

Les modèles doivent couvrir les courses entre délégation, révocation, ouverture, retour de buffer, perte de Portail et résurrection. Les tests de code sont nécessaires, mais ne remplacent pas ces modèles.

## 14. Feuille de route avec portes de validation

### P0 — Contrat de base et modèle

- Figer le vocabulaire, les types de capacités et les invariants ci-dessus.
- Écrire le modèle de délégation/révocation et refuser toute API qui le contourne.
- Cartographier le chemin réellement construit : noyau, bridge réseau, `network_server`, pilotes, DMA, IOMMU, IPC et Phoenix.

**Porte :** modèle sans contre-exemple pour l’ascendance, l’auto-élévation, la révocation et la possession de buffer dans les bornes définies.

### P1 — Tissu intra-hôte

- Implémenter `PactCap`, `BrinCap`, `LentilleCap` et Enveloppes sans NIC.
- Ajouter annuaire autorisé et `weave_control` minimal.
- Faire communiquer deux Services via Brin sur l’IPC existant ; produire les premières Lentilles de santé et d’audit.

**Porte :** le même appel réussit avec Pacte, échoue sans Pacte, après révocation, après transfert non autorisé ou depuis un autre sujet.

### P2 — Moteur et Portail LAN

- Isoler la propriété du pilote dans `weave_portal` et les sessions dans `weave_engine`.
- Brancher le chemin LAN sur le transport actuellement pris en charge, sans protocole propriétaire obligatoire.
- Ajouter l’entrée non fiable, la Promesse de Service, la quarantaine et les classes de trafic.

**Porte :** une application compromise ne peut ni envoyer hors de son Pacte, ni usurper une source, ni utiliser la NIC ; la matrice inter-Cercles autorisé/refusé est observée dans QEMU.

### P3 — Administration et gouvernance

- Créer l’interface d’intentions, la simulation d’impact, le bundle atomique, les Enveloppes et le protocole de quorum.
- Distinguer les Lentilles opération, audit et capture ; ajouter le journal scellé.

**Porte :** un administrateur de périmètre opère son Cercle mais ne peut ni s’élargir, ni lire un autre Cercle, ni appliquer un changement hors Enveloppe.

### P4 — Résilience et effilochage

- Mettre en œuvre les états Normal/restreint/drain/révoqué, WeaveVigie et les reprises Phoenix à ré-arbitrage.
- Tester panne de pilote, saturation de file, perte de contrôle, révocation concurrente et reprise de service.

**Porte :** après chaque scénario, l’état obtenu est aussi restrictif ou plus restrictif que l’état antérieur, jamais plus permissif.

### P5 — Performance et interopérabilité étendue

- Mesurer avant/après chaque optimisation : débit, p50/p99, drops, CPU, mémoire, pression des files et temps de révocation.
- Ajouter `BufferGrantCap`, multiqueue ou offloads seulement si les invariants et les mesures restent satisfaisants.
- Évaluer une Fabric Exo native seulement quand le besoin dépasse les Portails IP compatibles.

**Porte :** un gain chiffré et reproductible n’a pas diminué l’isolation, la capacité de révocation ou la transparence d’audit.

## 15. Confrontation aux quatre axes

| Axe | Choix obtenu | Compromis accepté |
|---|---|---|
| Sécurité | Relations nommées, Pactes atténués, Brins liés au sujet, Lentilles séparées, Portails isolés et ré-arbitrage Phoenix. | Plus de types et de validations au moment de l’ouverture. |
| Simplicité d’usage | Cercles, Services et intentions métier ; simulation d’impact ; erreur explicative ; aucune plomberie IP exposée par défaut. | La conception, le compilateur et la gouvernance sont volontairement plus élaborés. |
| Fiabilité | Plans séparés, génération atomique, bail court, effilochage seulement réducteur, reprise sûre. | Les nouvelles autorisations s’arrêtent si le contrôle est indisponible ; certaines connexions doivent se reconnecter. |
| Performance | Sceau de Brin court, décision coûteuse hors chemin chaud, files bornées et évolution graduelle vers `BufferGrant`. | Zéro-copie intégrale, protocole Fabric et offloads ne sont pas présumés acquis. |

## 16. Ce que ce consensus interdit

- un « root réseau » quotidien ou un administrateur pouvant modifier son propre plafond ;
- un socket, une adresse, une NIC, une capture ou une découverte globale possédés par défaut ;
- une politique avec ordre implicite, règle d’exception cachée ou activation partielle ;
- l’assimilation de la présence sur le LAN à une identité ou un droit ;
- l’augmentation automatique de privilège par une règle de secours, une restauration ou une optimisation ;
- une promesse de performance ou de reprise non soutenue par les tests réels du dépôt.

## Conclusion

Le meilleur des propositions n’est pas de choisir entre ExoNet, ExoFlow, ExoWeave, ExoFabric ou ExoNexus. C’est de prendre leur intuition commune et de la rendre exécutable :

- le **Conduit** devient la relation abstraite ;
- le **Pacte** est l’autorisation durable ;
- le **Brin** est l’exécution courte et performante ;
- le **Cercle** est l’interface humaine ;
- la **Lentille** protège la lecture ;
- le **Portail** contient le monde physique ;
- la **Couronne** fixe un plafond que personne ne peut s’accorder lui-même.

ExoWeave ne transforme donc pas le réseau en une simple pile de paquets filtrés. Il en fait un système de relations d’autorité explicites, vérifiables et administrables par intention.

## Documents synthétisés

- `consensus reseau.txt`
- `ExoOS_Reseau_Proposition_Claude_v2_TableRase.md`
- `ExoOS_Reseau_Proposition_Claude.md`
- `EXOOS_NETWORK_ADMINISTRATION_PROPOSAL_v0.1.md`
- `EXOWEAVE_ORIGINAL_DESIGN_v1.md`
- `CONCEPTION_EXONEXUS_ORIGINALE.md`
- `PLAN_ADMINISTRATION_RESEAU_EXONET.md`
