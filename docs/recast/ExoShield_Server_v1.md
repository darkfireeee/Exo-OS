# ExoShield Server v1

> **Périmètre** : `servers/exo_shield/` uniquement  
> **Type** : service Ring 1, endpoint IPC `10`  
> **Statut** : spécification runtime alignée sur le code actuel et les corrections sécurité 2026-05

---

## 1. Séparation de responsabilité

Le serveur `exo_shield` (PID 10) n'est **pas** la couche kernel ExoShield v1.0 décrite dans `ExoShield_v1_Production.md`.

Les deux composants sont liés mais distincts :

- **ExoShield kernel** : invariants de boot, IOMMU, CET, PKS, audit chaîné, budgets, TLA de sûreté de plateforme.
- **ExoShield serveur** : confinement applicatif Ring 1, scan de signatures, heuristiques, forensics, quarantaine, traitement d'événements.

Les modèles TLA+ `ExoShield.tla` et `ExoShield_v1.tla` couvrent la **couche kernel**. Ils ne modélisent pas le moteur de scan ni le protocole métier du serveur PID 10.

---

## 2. Dépendances de sécurité

Le serveur dépend explicitement des composants suivants :

- `kernel/src/security/*` pour les invariants de plateforme et le graphe d'autorité IPC.
- `servers/crypto_server` pour toute opération cryptographique Ring 1.
- `kernel/src/exophoenix/*` pour les cycles d'isolation / restore de la plateforme.

Règles actives :

- **SRV-02 / SRV-04** : aucune crypto from-scratch en Ring 1 ; la vérification de signature doit passer par `crypto_server`.
- **IPC policy** : `exo_shield -> crypto_server` et `crypto_server -> exo_shield` doivent être explicitement autorisés par le noyau.
- **ExoPhoenix** : le réveil post-restore et le reseed crypto sont gérés côté noyau / `crypto_server`, pas dans le serveur `exo_shield`.

---

## 3. Protocole IPC du serveur

Endpoint public : `10`

Types de messages entrants :

| Msg | Nom | Rôle |
|-----|-----|------|
| `0` | `SCAN_REQUEST` | analyse d'un buffer ou d'un processus cible |
| `1` | `EVENT_REPORT` | remontée d'événement temps réel |
| `2` | `QUARANTINE_CMD` | confinement, libération, interrogation d'état |
| `3` | `THREAT_QUERY` | consultation des évaluations de menace |
| `4` | `POLICY_UPDATE` | mise à jour des politiques de scan |
| `5` | `HEARTBEAT` | liveness / surveillance |

Le message reçu contient actuellement `sender_pid`, `msg_type` et `payload[120]`.
Cette ABI reflète l'implémentation existante ; elle ne doit pas être confondue avec un modèle capability complet.

Contrat sécurité actif du protocole :

- Toutes les requêtes passent par `ipc_gate` avec audit et rate limiting.
- `QUARANTINE_CMD` et `POLICY_UPDATE` sont toujours capability-gated.
- `SCAN_REQUEST` devient capability-gated dès qu'un appelant tente de scanner un autre PID que lui-même.
- `EVENT_REPORT` devient capability-gated pour signaler un événement sur un autre PID que l'émetteur.
- `THREAT_QUERY` devient capability-gated pour les requêtes détaillées (`by_id`, `stats`) et pour toute consultation d'un PID tiers.
- Quand une requête est classée comme privilégiée, `payload[100..120]` transporte un `ExoCapTokenWire` vérifié par le noyau avant exécution.
- Le token attendu porte le droit `IPC_SEND` vers l'endpoint `10` ; un simple `sender_pid` ne suffit donc plus pour piloter une action inter-processus ou muter l'état du serveur.

Cette séparation aligne le serveur avec l'intention des modèles TLA d'autorité IPC bornée et avec le durcissement capability déjà appliqué à `crypto_server`.

---

## 4. Surface de sécurité du module signatures

Le sous-module `signatures/update.rs` applique désormais les contraintes suivantes :

- les clés de confiance restent maintenues localement dans `exo_shield`,
- la signature d'une mise à jour est vérifiée via `crypto_server`,
- l'implémentation locale Ed25519 n'est plus le chemin actif de validation,
- le rollback de base de signatures reste local au serveur.

Le flux de vérification est :

1. vérifier que `publisher_key` appartient à l'ensemble de confiance local,
2. reconstruire le message signé (`header` avec champ `signature` neutralisé + `payload`),
3. déléguer la vérification Ed25519 en streaming à `crypto_server`,
4. n'appliquer la mise à jour qu'en cas de succès.

---

## 5. Politique IPC active

Le serveur initialise maintenant effectivement `ipc_gate/policy.rs` et `ipc_gate/audit.rs` au démarrage.

Règles opérationnelles par défaut :

- `ipc_router` et `init_server` peuvent joindre `exo_shield` sur tous les types de messages.
- `QUARANTINE_CMD` et `POLICY_UPDATE` sont refusés à tout autre émetteur sans capability valide, même si le message atteint l'endpoint public.
- `SCAN_REQUEST`, `EVENT_REPORT` et `THREAT_QUERY` restent disponibles en self-service seulement pour les opérations portant sur l'émetteur lui-même ; les variantes inter-processus basculent sur le même contrôle capability noyau.
- `HEARTBEAT` reste public, journalisé et borné par quota.

Ce point est important pour la cohérence documentaire : la politique IPC n'est plus seulement architecturale, elle est branchée sur le chemin runtime du serveur PID 10.

---

## 6. Limites connues

Les points suivants ne sont pas couverts intégralement par cette spécification :

- la preuve formelle TLA+ du serveur Ring 1 lui-même,
- un protocole capability utilisateur complet de bout en bout pour `crypto_server`,
- un protocole TLS sessionnel Ring 1 entre services.

En particulier, `CAP-01` côté `crypto_server` reste une dette d'architecture tant qu'un ABI capability utilisateur complet n'est pas propagé à tous les appelants.

---

## 7. Contrat de cohérence

Pour considérer le serveur `exo_shield` cohérent avec l'architecture Exo-OS :

- `ExoShield_v1_Production.md` doit rester la source de vérité de la couche kernel,
- le présent document doit rester la source de vérité du serveur PID 10,
- toute opération crypto Ring 1 nouvelle doit être implémentée dans `crypto_server`,
- toute extension IPC du serveur doit être recoupée avec le graphe d'autorité noyau.
