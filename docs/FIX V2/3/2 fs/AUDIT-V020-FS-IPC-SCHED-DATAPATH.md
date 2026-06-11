# AUDIT-V020 — Chemin de données FS / IPC / Scheduler / Process

| Champ | Valeur |
|---|---|
| Projet | ExoOS v0.2.0 "Strata" |
| Dépôt | `github.com/darkfireeee/Exo-OS` |
| HEAD audité | `601f445` |
| Date | 2026-06-10 |
| Méthode | Analyse statique croisée Python — lecture du chemin de données réel |
| Périmètre | `fs/exofs`, `servers/vfs_server`, `syscall/fs_bridge` (5073 l.), `ipc/`, `scheduler/`, `process/` |
| Auditeur | claude-alpha |

> **Objet de la passe :** identifier les incohérences qui empêchent (1) une **fonctionnalité FS réelle**, (2) le **chargement/remplacement de binaires**, et (3) la **rapidité des processus** (zéro-copie, latence). Les conclusions ci-dessous proviennent du suivi pas-à-pas du flux `userspace → syscall → fs_bridge → exofs`, et non d'une recherche de motifs.

---

## 0. Architecture réelle du chemin de données (telle qu'observée)

Le flux d'un `read()`/`write()`/`open()` userspace est :

```
Ring3  →  SYSCALL  →  table.rs (routage)  →  fs_bridge::fs_read/fs_write/fs_open
                                                   │
                                                   ▼
                                  crate::fs::exofs::*  (BLOB_CACHE, OBJECT_TABLE, …)
                                                   │  IN-KERNEL, Ring0
                                                   ▼
                                  storage / virtio_adapter
```

**Constat fondateur :** `fs_bridge.rs` importe et appelle **directement** `crate::fs::exofs::*` (cache, object table, path index). **Aucun IPC n'est émis vers `vfs_server`** dans le chemin d'I/O. Le routage est confirmé dans `table.rs:171-345` (`fs_bridge::fs_read`, `fs_write`, `fs_open`, …).

En parallèle, `servers/vfs_server` (PID 3, Ring1) existe avec une pile POSIX complète (`translation_layer/`, `ops/`, `compat/`) — mais son en-tête (`main.rs:7-21`) déclare lui-même que les opérations POSIX `VFS_CLOSE..FSYNC (4..14)` sont **« déléguées au kernel »**, et il ne traite réellement que `VFS_MOUNT` / `VFS_RESOLVE`.

**Conséquence directe sur les 3 axes** : tout le moteur ExoFS s'exécute en **Ring0**, le serveur Ring1 est un quasi-doublon, et le chemin d'I/O est entièrement synchrone, copiant, et non-bloquant. Détails ci-dessous.

---

## 1. AXE FONCTIONNALITÉ RÉELLE DU FS

### F1 · P1 — Double chemin FS redondant : le design microkernel n'est pas réalisé

**Fichiers :** `kernel/src/syscall/fs_bridge.rs` (tout) vs `servers/vfs_server/src/**`

Deux implémentations FS complètes coexistent :

| Chemin | Localisation | Ce qu'il fait réellement |
|---|---|---|
| **Kernel direct** | `fs_bridge.rs` → `fs/exofs` | **100 % des I/O** : open, read, write, lseek, dup, fcntl, pipe, socket, eventfd, poll… Tout est servi ici, en Ring0. |
| **vfs_server Ring1** | `servers/vfs_server` | Seulement `MOUNT`/`RESOLVE`. Les ops POSIX 4..14 sont marquées « déléguées au kernel » (`main.rs:21`) — donc redondantes ou jamais empruntées. |

**Impact :** le principe « ExoFS en Ring1 » de la spec Strata n'est pas tenu — le moteur complet (≈54 % des LOC kernel) tourne en Ring0, dans le même espace de confiance que l'ordonnanceur et la mémoire. Cela contredit le modèle de sécurité capability/Ring séparé du projet. La présence des deux mondes crée aussi un risque de **divergence d'état** (table de montages dans vfs_server vs `MountTable` kernel) et une confusion de maintenance.

**Recommandation :** trancher l'intention. Soit (a) vfs_server devient un simple détenteur de la table de montages et la doc cesse de prétendre servir les ops POSIX ; soit (b) à terme, déplacer réellement le moteur exofs en Ring1 (refonte lourde). À court terme, au minimum **supprimer le code mort de délégation POSIX dans vfs_server** et documenter que le FS est Ring0 en v0.2.0.

### F2 · P1 — `read()`/`write()` bloquants ne bloquent jamais (violation POSIX + busy-poll)

**Fichier :** `kernel/src/syscall/fs_bridge.rs` — `fs_read` (1696), `fs_write` (1791)

Sur un pipe / socket / eventfd **vide**, le chemin retourne `WouldBlock` → `EAGAIN` **inconditionnellement**, sans jamais consulter le flag `O_NONBLOCK` ni appeler de primitive de blocage :

```rust
fs_bridge.rs:1720
    if is_pseudo_blob(&entry.blob_id, PSEUDO_PIPE_TAG) {
        let data = BLOB_CACHE.get(...)...;
        if data.is_empty() {
            return Err(FsBridgeError::WouldBlock);   // <-- pas de test O_NONBLOCK
        }
```

Les seules occurrences de `O_NONBLOCK` dans `fs_bridge.rs` sont dans la **gestion de flags** (`fcntl`, ouverture), **jamais** dans la décision de blocage de `fs_read`/`fs_write` (vérifié : refs 229, 545, 2451-2474, 3121…). De plus `block_current_thread()` n'est **jamais** appelé depuis `fs_bridge` (voir Z4).

**Impact :**
- **Correctness :** un `read()` POSIX bloquant sur un FIFO vide doit **bloquer** le thread. Ici il renvoie `EAGAIN` même sans `O_NONBLOCK`. Un programme standard (`cat fifo`, shell pipeline) reçoit `EAGAIN` de façon inattendue.
- **Performance :** comme rien ne bloque, le userspace est forcé de **busy-poller** (`while read()==EAGAIN`), brûlant du CPU et saturant le dispatch syscall. C'est la cause structurelle de la « latence haute, architecture polling synchrone » notée dans le snapshot.

**Recommandation :** dans `fs_read`/`fs_write`, si la ressource est vide **et** `O_NONBLOCK` absent, transitionner le thread et appeler `scheduler::block_current_thread()`, avec réveil sur écriture (`wake_on` côté `fs_write`). Sinon, retourner `EAGAIN` seulement quand `O_NONBLOCK` est positionné.

### F3 · P2 — Écriture sur pipe en O(n) : recopie du tampon entier à chaque `write`

**Fichier :** `fs_bridge.rs:1824-1836` (chemin `PSEUDO_PIPE_TAG`)

```rust
let mut data = BLOB_CACHE.get(&entry.blob_id).map(|b| b.to_vec()).unwrap_or_default();
data.extend_from_slice(&input);
BLOB_CACHE.insert(entry.blob_id, data)...;
```

Chaque `write()` sur un pipe : (1) `.to_vec()` **copie tout le contenu existant** du pipe, (2) ajoute les nouveaux octets, (3) réinsère le tout. Pour un pipe rempli en N petits writes, coût total **O(N²)**. Idem la lecture pipe (`fs_read:1720-1735`) reconstruit le reste via `data[read_len..].to_vec()`.

**Impact :** débit pipe effondré sur les flux fragmentés (cas typique d'un shell). Combiné au busy-poll de F2, les pipelines userspace seront très lents.

**Recommandation :** remplacer le blob-cache pipe par un véritable ring-buffer FIFO (tête/queue), sans recopie du contenu résiduel.

---

## 2. AXE CHARGEMENT / REMPLACEMENT DE BINAIRES

### B1 · ✅ Câblé et correct — `execve` charge bien depuis ExoFS, en zéro-copie cache

**Fichiers :** `process/lifecycle/exec.rs` (`do_execve` 212), `fs/elf_loader_impl.rs` (`ExoFsElfLoader::load_elf` 347)

Le chemin est **fonctionnel et propre** :

```
do_execve  →  ELF_LOADER (ExoFsElfLoader, enregistré lib.rs:327)
           →  resolve_blob_id(path)                 (path ExoFS → BlobId)
           →  read_blob_from_cache → BLOB_CACHE.get → Arc<[u8]>   (ZÉRO-COPIE)
           →  validate_elf_header + PT_INTERP        (interpréteur dynamique géré)
           →  install_elf_image (segments → nouvel AS)
```

Points positifs vérifiés :
- `read_blob_from_cache` (elf_loader_impl.rs:554) retourne **`Arc<[u8]>`** — pas de copie sur cache-hit, fallback `object_store::load_blob_data_if_available` sur cache-miss.
- Gestion de l'**interpréteur dynamique** (PT_INTERP) avec second blob + handoff (`build_dynamic_handoff`).
- Remplacement d'espace d'adressage, KPTI shadow, reset signaux, AT_* auxv.

**Conclusion axe binaires :** le **loader n'est pas le blocage**. Conformément au snapshot, le seul obstacle au lancement de `exosh` est sa **présence dans le rootfs ExoFS** (le blob doit être résoluble par `resolve_blob_id`). Le mécanisme de chargement/remplacement d'image est prêt.

### B2 · P1 — Vérification de signature neutralisée → n'importe quel binaire peut être substitué

**Fichier :** `exec.rs:272-289` (déjà signalé dans `AUDIT-V020-INCOHERENCES-KERNEL §P1-1`)

La branche stricte est gardée par `#[cfg(feature = "strict_exec_signatures")]` — **feature déclarée dans aucun `Cargo.toml`**. Donc en pratique, un binaire dont la chaîne de confiance ED25519 échoue s'exécute **toujours** avec un simple warning.

**Impact spécifique à cet axe :** le « changement de binaire » n'est soumis à **aucun contrôle d'intégrité effectif**. On peut remplacer l'image d'un service par un blob non signé : `execve` l'exécutera. C'est cohérent avec ton modèle ExoSeal/chaîne de confiance, mais actuellement **désactivé par construction**.

**Recommandation :** déclarer `strict_exec_signatures` dans `[features]` du kernel et l'activer dans le profil de production (cf. fix de l'audit précédent).

### B3 · P2 — `sys_fork` / `sys_execve` tripliqués (risque de divergence)

**Fichiers :** `syscall/table.rs:2157,2302` · `syscall/handlers/process.rs:24,48` · `syscall/dispatch.rs:240-276`

Trois implémentations coexistent : les stubs morts de `table.rs` (commentés « code mort »), les handlers de `handlers/process.rs`, et le **vrai** traitement in-place de `dispatch.rs` (`handle_fork_like_inplace`, `handle_execve_inplace`) — seul ce dernier est réellement emprunté, car `dispatch.rs:240-276` court-circuite la table.

**Impact :** aucun bug actif, mais trois sources pour la même sémantique de création de processus. Une correction appliquée au mauvais exemplaire serait silencieusement inopérante (exactement le motif « wired but not connected » récurrent du projet).

**Recommandation :** supprimer les stubs de `table.rs` et le doublon de `handlers/process.rs`, garder le chemin in-place de `dispatch.rs`.

---

## 3. AXE RAPIDITÉ DES PROCESSUS / ZÉRO-COPIE

### Z1 · P1 — Lecture de fichier : 1 allocation heap + 2 copies évitables par `read()`

**Fichier :** `fs_bridge.rs:1772-1786` (chemin fichier régulier)

```rust
let data = BLOB_CACHE.read_at(&entry.blob_id, start, count)?;   // -> Vec<u8> : ALLOC + COPIE #1
copy_to_user(buf_ptr, data.as_ptr(), read_len)?;                // COPIE #2 vers Ring3
```

`BLOB_CACHE.read_at` (blob_cache.rs:386) a pour signature **`-> ExofsResult<Vec<u8>>`** : il **alloue un `Vec` et recopie** depuis la page de cache. Puis `copy_to_user` recopie ce `Vec` vers l'espace utilisateur.

**Incohérence frappante :** le **même cache** expose `BLOB_CACHE.get() -> Arc<[u8]>` (zéro-copie, refcompté), et c'est **précisément ce que le chargeur ELF utilise** (B1). Le chemin `read()` régulier pourrait **emprunter une tranche de l'`Arc`** et faire un unique `copy_to_user`, mais utilise la variante `read_at` copiante. Résultat : **chaque lecture de fichier paie une allocation heap + une copie de plus que nécessaire**.

**Recommandation :** réimplémenter `fs_read` sur `BLOB_CACHE.get() → &Arc<[u8]>` puis `copy_to_user(&arc[start..start+n])`. Une seule copie (inévitable, Ring0→Ring3), zéro allocation.

### Z2 · P1 — Écriture de fichier : copie intermédiaire systématique

**Fichier :** `fs_bridge.rs:1859-1869`

```rust
let input = read_user_bytes(buf_ptr, count)?;     // ALLOC Vec + COPIE depuis Ring3
BLOB_CACHE.write_at(entry.blob_id, start, &input)?; // COPIE vers la page de cache
```

`read_user_bytes` alloue un `Vec` tampon, puis `write_at` recopie dans le cache. La copie Ring3→cache pourrait être directe (copier depuis le pointeur user vers la page de cache après validation), évitant le `Vec` intermédiaire.

**Recommandation :** chemin d'écriture direct user-ptr → page de cache, sans tampon `Vec`.

### Z3 · P2 — Infrastructure zéro-copie complète mais **non branchée** sur le chemin de données

Trois sous-systèmes zéro-copie sont **définis, exportés, documentés… et instanciés nulle part dans le hot path** :

| Composant | Défini dans | Exporté | Utilisé dans le chemin I/O ? |
|---|---|---|---|
| `ZeroCopyReader` / `ZeroCopySlice` (`read_at → &[u8]`) | `fs/exofs/io/zero_copy.rs:141` | oui (`io/mod.rs:80`) | **Non** — `fs_read` utilise `read_at→Vec` (Z1) |
| `ZeroCopyRing` / `StreamChannel` (IPC streaming) | `ipc/ring/zerocopy.rs`, `ipc/channel/streaming.rs` | oui (`channel/mod.rs:110`) | **Non** — aucun serveur ni FS n'instancie `StreamChannel` |
| `shared_memory` pool (pages partagées) | `ipc/shared_memory/`, init `ipc/mod.rs:158` | oui | **Partiel** — seul `memory_server` (`attach_shared_region`) l'utilise ; **aucun syscall FS** |

**Impact :** l'effort d'ingénierie zéro-copie est présent mais **inerte**. Le chemin réel reste copiant (Z1/Z2). C'est la principale incohérence « zéro-copie » : l'API existe, le câblage manque.

**Recommandation :** brancher `ZeroCopyReader` dans `fs_read` (Z1) ; pour les gros transferts inter-serveurs (FS↔réseau, FS↔userspace volumineux), router via `StreamChannel`/`shared_memory` au lieu des payloads IPC inline de 192 octets.

### Z4 · P1 — Modèle fondamentalement non-bloquant + spin-poll (pas d'event-driven)

**Fichiers :** `ipc/sync/futex.rs:18`, `scheduler/core/switch.rs:119`, `scheduler/timer/sleep.rs:220`

- `futex_wait` est documenté comme **« WAIT avec spin-poll sur `waiter.woken` »** (futex.rs:18) — la primitive de synchronisation de plus bas niveau **tourne en boucle** au lieu de dé-scheduler.
- `block_current_thread()` (switch.rs:119) **existe** mais ses **seuls appelants** sont `timer/sleep.rs:220` (sommeil temporisé) et le hook IPC (`lib.rs:299`). Il n'est **jamais** invoqué depuis `fs_bridge` (pipes/sockets/poll/select) ni depuis le futex.

**Impact :** l'attente sur ressource (FS, IPC, futex) repose sur du **busy-wait** plutôt que sur une mise en sommeil + réveil événementiel. Sous charge ou avec plusieurs threads en attente, cela gaspille des cycles, dégrade la latence des autres tâches, et empêche le CPU d'entrer en C-states. C'est la racine commune de la lenteur perçue.

**Recommandation :** convertir `futex_wait` et les attentes FS/IPC en blocage réel via `block_current_thread()` + file d'attente de réveil (`scheduler/sync/wait_queue.rs` existe déjà mais est sous-utilisé). Réserver le spin aux sections ultra-courtes (verrous).

### Z5 · P1 — Anneau IPC à 16 slots : saturation sous charge (bug réseau connu, racine confirmée)

**Fichiers :** `ipc/core/constants.rs:68` (`RING_SIZE = 16`), `ipc/channel/mpmc.rs:207-234`

L'anneau MPMC a une **capacité de 16 messages**. `send()` retourne `IpcError::QueueFull` lorsqu'il est plein (mpmc.rs:228-234) ; l'appelant doit retenter. C'est la cause structurelle de la « saturation IPC par retry 1 ms » et de l'`ETIMEDOUT` notés dans le snapshot réseau.

**Impact :** sous le moindre débit (réseau, FS via IPC, multi-clients), 16 slots se remplissent immédiatement → boucle de retry → latence et timeouts. La doc historique annonçait 4096 ; la réalité est 16.

**Recommandation :** porter `RING_SIZE` IPC à une valeur réaliste (≥ 256, idéalement 1024–4096 selon budget mémoire), et coupler avec un réveil bloquant (Z4) plutôt qu'un retry temporisé.

---

## 4. Synthèse transversale IPC / Scheduler / Process

| Sous-système | Incohérence | Réf. | Effet |
|---|---|---|---|
| **IPC** | Anneau 16 slots → QueueFull/retry | Z5 | Saturation, timeouts réseau/FS |
| **IPC** | Zéro-copie (`StreamChannel`, `ZeroCopyRing`, SHM) inerte | Z3 | Gros transferts passent par copies + payloads 192 o |
| **Scheduler** | `block_current_thread` quasi inutilisé ; futex en spin-poll | Z4 | Busy-wait, latence, pas de C-states |
| **Scheduler/FS** | `read`/`write` ne bloquent pas (pas de test O_NONBLOCK) | F2 | Busy-poll userspace, viole POSIX |
| **Process** | `fork`/`execve` tripliqués | B3 | Risque de divergence silencieuse |
| **Process** | Signature binaire neutralisée (feature fantôme) | B2 | Substitution de binaire non contrôlée |
| **FS arch.** | Double chemin Ring0/Ring1 redondant | F1 | Moteur FS entièrement en Ring0 |
| **FS perf** | `read`=2 copies+alloc, `write`=copie interm., pipe O(n²) | Z1/Z2/F3 | Débit FS dégradé |

---

## 5. Priorisation (impact réel / coût)

**Bloquants de fonctionnalité réelle :**
1. **F2 — read/write bloquants** : sans blocage réel, les pipes/sockets POSIX sont inutilisables proprement et le système busy-poll. Prérequis à un shell fluide.
2. **F1 — clarifier le double chemin FS** : décider Ring0 assumé vs Ring1 cible ; supprimer le code mort vfs_server pour éviter la divergence.

**Gains de rapidité majeurs (faible coût) :**
3. **Z1 — `fs_read` sur `Arc<[u8]>`** : supprime 1 alloc + 1 copie par lecture, en réutilisant l'API déjà employée par le loader. Quelques dizaines de lignes.
4. **Z5 — `RING_SIZE` IPC ≥ 256** : une constante. Élimine la saturation/retry.
5. **Z4 — blocage réel (futex + FS)** : remplace le spin-poll par `block_current_thread` + wait_queue. Effort moyen, impact latence global.

**Sécurité du remplacement de binaire :**
6. **B2 — activer `strict_exec_signatures`** : déclarer la feature + l'activer en prod.

**Dette / propreté :**
7. **Z2 / F3** (chemins d'écriture et pipe), **B3** (dédup fork/execve), **Z3** (brancher le streaming zéro-copie pour gros transferts).

**Bonne nouvelle (axe binaires) :** le chargement/remplacement d'image via `ExoFsElfLoader` est **complet et zéro-copie au niveau cache** (B1). Le lancement de `exosh` ne dépend que de sa présence dans le rootfs ExoFS, pas du loader.

---

*AUDIT-V020-FS-IPC-SCHED-DATAPATH.md — ExoOS v0.2.0 Strata — HEAD `601f445` — 2026-06-10 — claude-alpha*
