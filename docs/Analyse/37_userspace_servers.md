# Rapport d'Analyse : Serveurs Système Userspace (servers/)

## 1. Vue d'Ensemble

L'architecture d'Exo-OS repose sur un micro-noyau où la grande majorité des fonctionnalités traditionnellement intégrées au Ring 0 sont déportées dans l'espace utilisateur (Ring 1 et Ring 3). Le répertoire `servers/` regroupe l'ensemble des démons et serveurs fondamentaux du système d'exploitation.

### Inventaire des Serveurs :
1. `init_server` (PID 1) : Superviseur racine, init micro-noyau, gestionnaire de dépendances de services et relance automatique (fail-safe).
2. `ipc_router` (PID 2) : Routeur IPC central, validation des tokens de capability et distribution des messages.
3. `device_server` : Plan de contrôle GI-03 pour l'énumération PCI, les revendications de périphériques (*claims*), l'IOMMU et l'alimentation.
4. `vfs_server` : Système de fichiers virtuel, arborescence unifiée, descripteurs de fichiers et ponts POSIX.
5. `memory_server` : Gestionnaire de mémoire utilisateur, pagination à la demande et mémoire partagée.
6. `scheduler_server` : Contrôle des priorités, quotas cgroups et affinités CPU.
7. `network_server` : Pile réseau IP (`smoltcp`), interfaces ethernet et sockets.
8. `crypto_server` : Services cryptographiques, gestion de clés et certificats.
9. `exo_shield` : Sentinelle de sécurité userspace, surveillance d'intégrité et audit.
10. `fb_server`, `input_server`, `tty_server` : Sous-système graphique, saisie clavier/souris et terminaux virtuels.
11. `syscall_abi` : Couche canonique d'ABI syscall partagée (registres SysV AMD64, instructions `syscall`).

---

## 2. Analyse Détaillée des Composants Critiques

### 2.1 `init_server` (PID 1)

- **Flux d'Amorçage Séquentiel :**
  - Exécuté en tant que tout premier processus userspace (`/sbin/exo-init-server`).
  - Démarre immédiatement `ipc_router` (PID 2) pour activer les canaux de communication inter-processus.
  - Ordonne ensuite le démarrage des serveurs matériels et d'infrastructure (`device_server`, `memory_server`, `vfs_server`).
  - Stabilise les services pendant un délai de grâce (`SERVICE_STABLE_MS = 5000 ms`) avant de démarrer le shell interactif (`exosh`).

- **Politique de Résilience et Supervision :**
  - **Invariant :** PID 1 ne se termine jamais. Il boucle indéfiniment sur la réception des événements noyau et des signaux `SIGCHLD`.
  - **Backoff Exponentiel :** Tout service interrompu anormalement est redémarré avec un délai progressif (1 tick, 2 ticks... jusqu'à 32 secondes) pour éviter les tempêtes d'erreurs en boucle (*crash loops*).
  - Gestion des signaux non-masquables : Respecte l'isolation stricte des signaux SIGKILL/SIGSTOP assurée par le noyau (DOC1 RÈGLE SIGNAL-01).

---

### 2.2 `device_server` (Plan de Contrôle GI-03)

- **Gestion des Revendications Matérielles (`Claims`) :**
  - Valide les autorisations des pilotes Ring 1 souhaitant accéder aux registres MMIO ou aux ports I/O d'un périphérique PCI.
  - Émet l'appel système d'administration `SYS_PCI_SET_TOPOLOGY` vers le Ring 0 avec vérification de l'identité du demandeur (`sender_pid == 1` lors de la configuration initiale).
  - Associe chaque périphérique PCI à son domaine IOMMU dédié pour bloquer tout accès DMA direct à la mémoire noyau ou à d'autres processus.

- **Hotplug et Événements Matériels :**
  - Maintient une file d'événements `HotplugQueue` protégée par `spin::Mutex`.
  - Distribue les notifications de connexion/déconnexion de périphériques aux serveurs abonnés.

---

### 2.3 `syscall_abi` (Interface Système Universelle)

- **Primitives d'Appel Système :**
  - Écrit en assembleur inline pur `core::arch::asm!` pour x86-64 :
    - `syscall1`, `syscall2`, `syscall3`, `syscall4`, `syscall5`, `syscall6`.
    - Place le numéro d'appel système dans `RAX`.
    - Arguments passés dans `RDI`, `RSI`, `RDX`, `R10`, `R8`, `R9` selon l'ABI standard x86-64 Linux/POSIX.
    - Clobbers : `RCX` et `R11` (écrasés matériellement par l'instruction `syscall`).
    - Valeur de retour lue depuis `RAX`.

- **Conformité no_std :**
  - Zéro dépendance externe, types primitifs stricts, codes d'erreur conformes à l'ABI POSIX (ex. `EPERM = 1`, `ENOENT = 2`, `ESRCH = 3`, `EINTR = 4`, `EIO = 5`).

---

## 3. Synthèse de Sûreté et Sécurité

| Sous-système | Risque Identifié | Mécanisme de Protection Micro-Noyau |
| :--- | :--- | :--- |
| **Superviseur Init** | Crash d'un démon système critique | Supervision proactive avec relance exponentielle et isolation des fautes. |
| **Accès Périphériques** | Pilote non privilégié accédant au matériel | Contrôle d'accès par token capability et domaine IOMMU imposé par `device_server`. |
| **Syscall ABI** | Corruption de registres lors de l'appel | Préservation stricte de la pile et des registres callee-saved dans les stubs `asm!`. |
| **Routage IPC** | Usurpation d'identité d'un service | Vérification matérielle et cryptographique du `Pid` injecté par le noyau dans chaque message. |

---

## 4. Statut de Conformité

- **Architecture :** Conforme au modèle micro-noyau et aux spécifications GI-03.
- **Sécurité :** Isolation Ring 1/Ring 3 étanche, principe du moindre privilège appliqué à tous les démons.
