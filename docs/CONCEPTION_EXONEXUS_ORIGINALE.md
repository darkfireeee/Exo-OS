# ExoNexus — réseau comme tissu d’autorité

**Document de conception original.** Il ne s’appuie sur aucune recherche Internet et ne reprend pas l’architecture du plan précédent. Il part uniquement des contraintes constitutives d’Exo-OS : noyau à capacités, IPC Ring 1, ExoShield, ExoPhoenix, pilotes matériels et façade POSIX.

## 1. Thèse

Le réseau d’un OS ne doit pas être une collection de cartes, d’adresses, de ports et de listes de règles. Ce sont seulement des détails de transport. Le vrai objet que l’OS doit administrer est une **relation autorisée de communication** : qui peut parler à quoi, pour quel but, par quel chemin, pendant combien de temps, avec quelle priorité, et qui peut constater que cela s’est produit.

ExoNexus transforme donc chaque communication en un **Pacte**. Un Pacte n’est ni un socket, ni une règle de pare-feu, ni un rôle global. C’est une autorisation compacte, bornée et traçable de faire exister une relation de communication.

L’application ne voit jamais la complexité du tissu : elle demande « joindre le service paie », « publier mon service », « voir l’état de mes connexions ». L’OS, lui, exécute un protocole d’autorité beaucoup plus strict et beaucoup plus riche.

## 2. Vocabulaire fondateur

| Terme | Définition |
|---|---|
| **Royaume** | Domaine d’autorité étanche : entreprise, filiale, équipe, production, invités, quarantaine, administration. |
| **Cellule** | Entité qui agit : application, service Ring 1, utilisateur, appareil ou portail réseau. |
| **Pacte** | Contrat d’autorisation de communication entre cellules ou royaumes. Il exprime l’intention, le périmètre, les limites et la durée. |
| **Sceau de route** | Autorité éphémère créée à l’ouverture réelle d’une session à partir d’un Pacte. Il est invalide hors de son sujet, de sa génération de politique et de son chemin. |
| **Portail** | Unique frontière entre le tissu interne et un réseau physique ou un protocole externe. Il possède le matériel, jamais les applications. |
| **Lentille** | Droit de lecture séparé du droit de communication : santé, métadonnées, audit ou contenu. |
| **Couronne** | Autorité de gouvernance maximale, volontairement indisponible à l’usage quotidien et fragmentée entre plusieurs dépositaires. |
| **Tissage** | Compilation d’une politique humaine en graphe de Pactes, plafonds et Sceaux admissibles. |

L’usage d’adresses IP, de ports, de VLAN, de DNS, de TCP ou d’UDP est permis aux Portails, mais aucun de ces éléments ne constitue une identité ou un droit dans le cœur d’ExoNexus.

## 3. Les lois du tissu

Ces lois sont les contraintes de conception. Une fonctionnalité qui les contredit est rejetée, même si elle est pratique.

1. **Aucune connectivité sans Pacte.** Une Cellule ne peut pas demander un accès réseau générique. Elle doit désigner une intention et posséder l’autorité correspondante.
2. **Aucune autorité sans ascendance.** Chaque Pacte a un parent identifiable jusqu’à la Couronne ; tout droit sans ascendance est invalide.
3. **La descendance ne peut que rétrécir.** Un enfant peut raccourcir une durée, réduire un débit, retirer une destination ou une opération ; il ne peut jamais les augmenter.
4. **L’observation est un pouvoir.** Voir les flux, leurs métadonnées ou leurs contenus est protégé par des Lentilles distinctes.
5. **Le matériel ne connaît pas les utilisateurs.** Une carte réseau ne connaît que le Portail qui la possède. Le Portail ne décide pas seul qui peut utiliser la carte.
6. **Le réseau physique n’est pas digne de confiance.** Un appareil local, une adresse, une annonce ou une trame n’acquièrent jamais d’autorité par leur simple présence.
7. **Une reprise ne doit pas élargir.** Après une résurrection ExoPhoenix, seul ce qui est encore valide dans le présent peut reprendre ; aucun état ancien ne recrée de droit.
8. **Le confort n’est pas une permission.** Toute facilité utilisateur est une vue simplifiée d’un Pacte, jamais un raccourci autour de lui.

## 4. Architecture complète

```text
                         GOUVERNANCE
  Couronne ──► Tisseur ──► catalogue de Pactes ──► journal scellé
       │              │
       │              └──────────► Consoles d'administration
       │
       ▼
                     PLAN DE DÉCISION
               Registre de Cellules et Services
                         │
 Application ─► Façade ─► Arbitre de Pacte ─► Sceau de route
                         │                          │
                         ▼                          ▼
                     PLAN D'EXÉCUTION          Moteur de flux
                        Lentilles                 │
                                                 Portails
                                           LAN / appareils / Internet
```

ExoNexus est scindé en sept composants Ring 1, volontairement spécialisés. Cette séparation n’est pas destinée à rendre la conception simple ; elle empêche une défaillance unique de se transformer en contrôle total.

| Composant | Mission | Autorité possédée | Autorité explicitement absente |
|---|---|---|---|
| `nexus_registry` | Enregistrer les Cellules, Services, Royaumes et leurs identités stables. | Écriture sur le catalogue d’identités avec validation. | Ouvrir un flux ou modifier une politique. |
| `nexus_weaver` | Compiler les politiques et le graphe d’autorité en Pactes admissibles. | Capacité de proposer un nouveau tissage. | Possession de NIC, transport de paquets, émission directe d’un Sceau. |
| `nexus_arbiter` | Vérifier un Pacte, l’ascendance et les limites ; demander un Sceau de route au noyau. | Capacité de décision ponctuelle. | Modifier son propre plafond, lire les payloads ou piloter un Portail. |
| `nexus_engine` | Faire circuler les données, appliquer quotas, priorités et Sceaux. | Accès aux flux déjà autorisés. | Inventer un Pacte ou conserver une autorité après révocation. |
| `nexus_portal` | Traduire le tissu interne vers un pilote, un LAN ou un réseau standard. | Capacité pilote/DMA strictement nécessaire. | Décider d’une communication métier ou exposer le matériel aux applications. |
| `nexus_lens` | Produire santé, traces et vues d’observation filtrées. | Émission de données auxquelles une Lentille autorise l’accès. | Créer une Lentille ou donner un accès à un contenu non autorisé. |
| `nexus_console` | Offrir les parcours administratifs et le protocole de quorum. | Session d’administration dans un Royaume. | Appliquer une mutation sans Tisseur, Arbitre et journal. |

Le noyau reste le gardien de quatre primitives minimales : type de capacité, lien sujet-capacité, compteur/budget non forgeable et génération de révocation. Les services Ring 1 forment des décisions ; le noyau rend impossible leur contournement.

## 5. Le Pacte : objet central

Un Pacte est immuable une fois publié. Il possède une version, une ascendance et une empreinte. Sa forme conceptuelle est la suivante :

```text
Pacte {
  origine        : Cellule ou groupe de Cellules
  cible          : Service, Cellule, Royaume ou Portail déclaré
  action         : appeler | publier | répondre | relayer | observer | capturer
  sens           : sortie | entrée | bidirectionnel
  transport      : interne | TCP | UDP | datagramme | flux fiable
  frontières     : Royaumes et Portails franchissables
  limites        : durée, sessions, octets, débit, burst, taille de message
  classe         : survie | contrôle | interactif | métier | fond | non-fiable
  confidentialité: aucune | canal protégé | identité mutuelle requise
  visibilité     : Lentilles admissibles
  parent         : identifiant du Pacte plus puissant
  génération     : génération de politique qui le rend valable
}
```

Le Pacte ne contient jamais une permission vague comme « réseau », « administrateur » ou « tous les ports ». Toute étendue est représentée comme une cible nommée, une frontière explicite ou une liste finie de destinations traduites par le Portail.

### 5.1 Du Pacte au Sceau de route

Le Pacte est une permission potentielle. Lorsqu’une application veut vraiment échanger des données, elle demande un Sceau de route. L’Arbitre effectue alors, une seule fois pour l’ouverture :

1. vérification de la Cellule appelante et de son CapToken ;
2. vérification de l’ascendance complète du Pacte ;
3. calcul des limites réellement disponibles après budgets parents ;
4. choix d’un Portail et d’un chemin autorisé ;
5. création noyau d’un Sceau de route lié au sujet, au Pacte, à la génération, à une expiration et aux quotas ;
6. inscription de l’événement dans le journal.

Le Moteur de flux ne revalide pas le graphe entier par paquet : il vérifie le Sceau et ses compteurs. Cela conserve une voie chaude courte sans déplacer la décision de sécurité hors de la voie d’ouverture.

### 5.2 Les trois fins d’un flux

Un flux ne peut se terminer que de trois façons : fermeture volontaire, expiration des limites, ou révocation. Une révocation ne détruit pas aveuglément la mémoire : elle place le flux en **drain contrôlé**, empêche tout nouvel envoi, libère les buffers selon le protocole du Portail, puis ferme. Pour une action sensible, le mode est **arrêt immédiat**.

## 6. Un cœur sans adresses

Le noyau et les applications internes ne se parlent pas par IP. Ils se parlent par :

```text
Royaume / Service / Instance / Intention
```

Exemple : `entreprise/finance/paie/consulter` est une cible d’autorité ; le Registre peut la résoudre vers une instance locale, une autre machine du LAN, ou plusieurs instances. Cette résolution ne modifie pas le Pacte initial : si la nouvelle instance est hors du Royaume autorisé, elle est refusée.

Les Portails sont les seuls à posséder la correspondance vers des adresses, ports et formats externes. Ainsi :

- déplacer un service ne force pas les applications à connaître une nouvelle adresse ;
- une adresse usurpée ne devient pas une identité interne ;
- le routage inter-services est un problème de graphe d’autorité avant d’être un problème de tables IP ;
- la façade POSIX peut rester compatible en traduisant `socket/connect/send/recv` en demande de Pacte, sans faire de l’API POSIX la source de vérité.

## 7. Fonctionnement sur un réseau local

### 7.1 Les Royaumes LAN

Le LAN est composé de Royaumes, non d’un grand réseau plat :

```text
Couronne entreprise
├── administration        : consoles, clés de gouvernance, récupération
├── production            : services métier publiés
├── développement         : CI, sandbox et environnements temporaires
├── collaboration         : postes et services utilisateurs
├── invités               : accès externe limité, aucun accès interne implicite
└── quarantaine           : appareils inconnus ou suspects, diagnostic seulement
```

Un Portail peut desservir plusieurs Royaumes physiques ou logiques, mais la traduction ne crée aucune traversée par défaut. Un appareil nouvellement vu est placé en quarantaine. Le passage vers collaboration ou production nécessite un Pacte d’enrôlement explicite et une identité d’appareil validée par la politique de l’entreprise.

### 7.2 Publication et découverte

Un service ne « s’ouvre pas sur le réseau ». Son propriétaire publie une **Promesse de service** : nom, Royaume, interface, niveau de santé, intentions acceptées, limites et exigences d’identité. Le Registre fabrique ensuite des annonces uniquement pour les Cellules ayant le droit de les recevoir.

La découverte est donc un résultat d’autorisation. Une application non admise ne voit pas un service interdit ; elle ne reçoit pas seulement un refus de connexion après l’avoir découvert.

### 7.3 Entrée depuis le LAN

Un pair externe rencontre un Portail, pas directement un service. Le Portail associe l’entrée à une Promesse publiée, crée une Cellule distante éphémère et exige le Pacte d’entrée correspondant. Une destination qui n’est pas publiée n’est pas routée vers l’intérieur, même si le pair connaît son adresse ou son port de traduction.

### 7.4 Priorité et équité

La classe de priorité est attribuée uniquement par le Tisseur. Les applications ne peuvent ni choisir une classe plus haute ni modifier leur quota. Le Moteur de flux réserve des files distinctes et des budgets indépendants :

| Classe | Usage | Garantie de conception |
|---|---|---|
| Survie | ExoPhoenix, arrêt sûr, libération DMA. | Ne dépend pas de la santé du trafic métier. |
| Contrôle | Révocation, gouvernance, identité, service registry. | Passe avant les tâches de données ordinaires. |
| Interactif | Console, interface utilisateur, assistance. | Latence protégée par une limite de volume. |
| Métier | Services explicitement critiques. | Quota garanti mais jamais illimité. |
| Fond | Synchronisation, mise à jour, indexation. | Cède lorsque les classes précédentes sont actives. |
| Non-fiable | Invités et quarantaine. | Ne peut pas affamer le reste du système. |

## 8. Administration : pouvoir sans super-utilisateur

### 8.1 Les enveloppes d’autorité

Chaque administrateur possède une **Enveloppe**, elle-même un Pacte administratif. L’Enveloppe définit le maximum non négociable de ce que cet administrateur peut modifier : Royaumes, types de Pactes, limites de priorité, budgets et durée de validité.

Un administrateur ne peut :

- ni modifier sa propre Enveloppe ;
- ni créer une Enveloppe de même niveau ou de niveau supérieur ;
- ni approuver seul une extension qui touche une frontière supérieure ;
- ni lire une donnée parce qu’il peut la gérer ;
- ni transformer une capacité de dépannage en droit permanent.

Le Tisseur refuse structurellement les cycles où A élève B et B élève A. Il refuse aussi tout Pacte enfant qui ne démontre pas son inclusion stricte dans le parent.

### 8.2 La Couronne n’est pas un compte

La Couronne est une racine de gouvernance, fragmentée entre plusieurs dépositaires désignés par l’entreprise. Elle ne possède pas de session réseau ordinaire. Une action de Couronne est un cérémonial : proposition, approbation de quorum, délai de réflexion configurable, signature, application atomique et audit immuable.

Le dirigeant de l’entreprise peut être l’un des dépositaires, mais son titre humain ne crée aucune capacité technique. Cela répond au problème du « chef d’entreprise tout-puissant » : la gouvernance définit le plafond, alors que l’opération quotidienne utilise des droits plus petits et réversibles.

### 8.3 Secours sans porte dérobée

Le mode urgence produit un **Pacte de secours** : périmètre minimal, objectif identifié, expiration courte, quorum supérieur à l’ordinaire, journal obligatoire, et suppression automatique à l’échéance. Il ne remet jamais une autorité générale au compte de l’administrateur. Le rapport de clôture est lui-même une condition pour qu’un nouveau Pacte de secours soit disponible.

## 9. La lecture, l’audit et la preuve

ExoNexus sépare quatre niveaux de vision :

| Lentille | Ce qu’elle révèle | Cas d’usage |
|---|---|---|
| Santé | disponibilité, saturation, erreurs agrégées. | Exploitation quotidienne. |
| Topologie | Royaumes, Services publiés et frontières autorisées. | Administration de périmètre. |
| Métadonnées | qui a demandé quel Pacte, volumes, durée, décision et motif. | Audit, incident, capacité. |
| Contenu | payload ou capture ponctuelle. | Diagnostic exceptionnel, jamais implicite. |

Une Lentille ne suit pas un rôle par défaut. Un administrateur de production peut gérer les Pactes de production sans lire leur contenu ; un auditeur peut lire les décisions sans ouvrir de session ; un opérateur peut voir la santé sans connaître l’architecture complète.

Le journal est une succession d’événements scellés : publication, délégation, refus, émission de Sceau, révocation, changement de Portail, franchissement de frontière et action de Couronne. Chaque événement porte l’empreinte de son parent logique. L’audit peut donc reconstruire non seulement « qui a fait quoi », mais « avec quelle ascendance ce droit existait ».

## 10. Parcours d’usage

La simplicité est ici un produit de l’interface, jamais une simplification de la sécurité sous-jacente.

### Employé ou application

```text
exo connect finance/paie
→ « Vous êtes dans collaboration. Le service paie accepte consulter, lecture seule,
   jusqu’à 10 minutes. Voulez-vous ouvrir la session ? »
```

L’utilisateur ne choisit ni IP, ni VLAN, ni route, ni privilège. Il reçoit une explication et peut lire l’expiration de sa session.

### Propriétaire de service

```text
exo publish paie --from production --accept consulter --for collaboration
→ aperçu : personnes concernées, frontière franchie, débit maximal, Lentilles autorisées
→ soumettre pour approbation
```

L’interface fabrique une Promesse et une proposition de Pacte. Elle ne permet pas d’entrer une règle arbitraire impossible à raisonner.

### Administrateur réseau

```text
exo inspect pacte production/paie/consulter
→ ascendance, limites, flux vivants, motif de chaque refus, prochaine expiration

exo revoke pacte production/paie/consulter --mode drain
→ impact prévisible, approbation requise, événement journalisé
```

L’administrateur travaille sur des relations lisibles, pas sur des milliers de règles ordonnées et de masques réseau.

## 11. Cas de fonctionnement importants

### Une application compromise

Elle peut uniquement consommer les Sceaux qui lui sont liés. Elle ne peut pas explorer les Services cachés, se connecter à une destination hors Pacte, monter une priorité, capturer le trafic ou déléguer un droit supérieur. La révocation de son Pacte coupe les nouvelles actions et clôt ses flux selon la classe de risque.

### Un administrateur compromis

Il reste enfermé dans son Enveloppe. Il peut causer des dommages dans son périmètre — ce risque doit être assumé et observé — mais ne peut pas élargir ce périmètre, modifier la Couronne, créer une autre Enveloppe puissante ou accéder au contenu sans Lentille séparée.

### Un Portail compromis ou instable

Il n’a pas le graphe des droits ni la clé pour créer un Sceau. Les flux concernés sont suspendus ou migrés vers un Portail alternatif si le Pacte le permet. Les autres Royaumes et Portails restent indépendants.

### Perte de `nexus_weaver` ou `nexus_arbiter`

Les Sceaux déjà valides continuent jusqu’à leur expiration et dans leurs limites ; aucune nouvelle relation ni extension de droit n’est créée. Le système favorise l’intégrité sur la disponibilité des nouvelles autorisations.

### Résurrection ExoPhoenix

Les Sceaux ne sont pas repris tels quels. L’état restauré indique seulement quels flux demandaient à revivre. Chaque candidat est ré-arbitré contre le tissage actuel ; à défaut, il est fermé. Les buffers et files du Portail sont drainés avant remise en service.

## 12. Intégration à Exo-OS

| Élément Exo-OS | Évolution ExoNexus proposée |
|---|---|
| CapTokens | Ajouter des types dédiés : `PactCap`, `RouteSealCap`, `LensCap`, `EnvelopeCap`, `CrownShareCap`. Les règles de dérivation résident dans le noyau. |
| IPC/SPSC | Véhicule exclusif de contrôle entre composants Nexus ; messages fixes, bornés, typés et sans adresse virtuelle transférable. |
| `network_server` | Devient progressivement le noyau de `nexus_engine` : il garde les protocoles et buffers, mais n’est plus l’autorité de politique. |
| Pilotes VirtIO/e1000 | Deviennent l’implémentation d’un `nexus_portal` ; une seule Cellule Portail possède le CapToken de périphérique et le DMA associé. |
| ExoShield | Verrouille les Portails admissibles et atteste l’état de leurs capacités matérielles ; aucune politique métier n’assouplit ce verrou matériel. |
| ExoPhoenix | Orchestre drain, gel, restauration candidate et ré-arbitrage des Sceaux. |
| ExoFS/POSIX | Fournit une façade de compatibilité, sans transformer les sockets POSIX en droits d’autorité. |
| `crypto_server` | Protège les Sceaux, les signatures de tissage, les sessions d’administration et les identités de Service. |

## 13. Construction par étapes

### Étape I — Fondation d’autorité

Définir les types de capacités, le format immuable du Pacte, l’Enveloppe, la Couronne et les règles de dérivation. Produire le premier modèle formel de l’ascendance et de la révocation avant toute interface graphique ou LAN.

**Condition de sortie :** il est impossible, dans le modèle et les tests de propriétés, de fabriquer, élargir, cycler ou ressusciter une autorité.

### Étape II — Tissu intra-hôte

Implémenter Registre, Tisseur, Arbitre et Moteur de flux pour des services d’un seul hôte. Les communications passent déjà par Pacte, mais aucun Portail physique n’est requis.

**Condition de sortie :** une application peut joindre un Service autorisé ; le même appel échoue sans Pacte, après révocation ou depuis une Cellule différente.

### Étape III — Portail LAN

Connecter le Moteur de flux aux pilotes existants derrière un Portail. Ajouter Royaumes, Promesses de services, enrôlement d’appareils, quarantaine et traduction vers le LAN. Mesurer d’abord le transport copié ; ne créer un mécanisme de buffer partagé contrôlé par capacité qu’après preuve de propriété et besoin mesuré.

**Condition de sortie :** aucune application n’accède directement à la NIC, aucun flux ne traverse une frontière sans Pacte et les buffers DMA ont toujours un propriétaire unique.

### Étape IV — Administration gouvernée

Livrer Console, Enveloppes, Couronne partagée, Lentilles et journal scellé. Concevoir les parcours autour de « demander », « publier », « comprendre », « révoquer », plutôt qu’autour de paramètres de réseau traditionnels.

**Condition de sortie :** une démonstration montre qu’un admin de périmètre peut opérer son Royaume mais ne peut ni augmenter sa propre autorité ni lire une donnée hors Lentille.

### Étape V — Résilience et assurance

Brancher le protocole Phoenix, injecter pannes de Portail, saturation, révocation concurrente, perte de services Nexus et reprise. Étendre le modèle formel aux transitions de flux, budgets, buffers et journal.

**Condition de sortie :** la reprise ne restaure jamais un flux plus autorisé qu’avant l’incident ; toute incertitude conduit à la fermeture sûre.

### Étape VI — Compatibilité et optimisation

Construire la façade POSIX, les ponts TCP/UDP externes, puis seulement les transports avancés. Optimiser sans changer les invariants : cache de Sceaux, batching, files par classe et Portails multiples.

**Condition de sortie :** les gains de performance sont accompagnés de tests de non-régression d’autorité, de propriété de buffers et de saturation.

## 14. Propriétés à prouver

| Propriété | Formulation attendue |
|---|---|
| Absence d’autorité ambiante | Toute sortie de paquet possède un Sceau valide et une ascendance de Pactes. |
| Non-escalade | Un enfant n’est jamais plus puissant que chaque parent de sa chaîne. |
| Non-interférence entre Royaumes | Une Cellule ne peut ni découvrir ni joindre un Royaume sans Pacte de frontière. |
| Lecture bornée | Toute donnée observée est couverte par une Lentille valide et contextualisée. |
| Révocation cohérente | Après révocation, aucun nouveau Sceau ni envoi non permis ne réussit. |
| Possession unique | Un buffer de Portail est détenu par une seule étape à la fois. |
| Reprise sûre | Après Phoenix, un flux réactivé satisfait la politique et les budgets actuels. |
| Gouvernance finie | Aucun cycle d’Enveloppes ne permet de reconstituer une Couronne. |

## 15. Arbitrages assumés

| Axe | Choix ExoNexus | Coût accepté |
|---|---|---|
| Sécurité | Tout flux est une relation scellée à ascendance vérifiable. | Plus de composants et un cycle de décision à l’ouverture. |
| Simplicité d’usage | L’utilisateur manipule des Services et des verbes, pas des adresses et des ACL. | La traduction vers ce langage demande un Registre et une Console soignés. |
| Fiabilité | État immuable, Sceaux courts, Portails isolés et reprise par ré-arbitrage. | Une perte du plan de décision bloque les nouvelles autorisations. |
| Performance | Décision coûteuse à l’ouverture, vérification courte sur la voie de données. | Les optimisations mémoire sont reportées jusqu’à preuve de nécessité. |

## 16. Ce qui est volontairement refusé

- un compte « root réseau » permanent ;
- des règles réseau dont l’ordre implicite change le sens ;
- une découverte de services mondiale et non filtrée ;
- l’accès direct d’une application à une NIC, au DMA, à un socket brut ou à une capture ;
- une restauration Phoenix qui fait confiance à l’état antérieur sans ré-arbitrage ;
- une fonction d’urgence qui contourne Couronne, Enveloppes, durée et audit ;
- l’optimisation qui introduit des pointeurs inter-processus non autorisés ou une propriété de buffer ambiguë.

## Conclusion

ExoNexus ne cherche pas à rendre le réseau moins complexe. Il déplace sa complexité là où elle peut être contrôlée, vérifiée et expliquée : dans un graphe d’autorité, des Sceaux courts, des Portails isolés et une gouvernance incapable de s’élever elle-même.

Pour l’utilisateur, le réseau devient une action simple : demander un service, publier une intention, comprendre une décision, retirer une relation. Pour l’OS et l’entreprise, chaque paquet devient la conséquence vérifiable d’un Pacte qui avait le droit d’exister.
