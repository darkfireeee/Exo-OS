# Analyse Critique des Roadmaps d'Adaptation des Librairies
## ExoOS v0.2.0 — Audit Complet

**Auteur :** claude-alpha  
**Date :** 2026-05-14  
**Cible :** `libs/vendors/` + roadmaps v1 et v2 fournis par ChatGPT  
**Statut :** Document fondateur — à lire avant tout autre document de cette série

---

## 1. Résumé Exécutif

Les deux roadmaps existantes (v1 et v2) sont des documents **techniquement sérieux mais architecturalement incorrects** pour ExoOS. Elles traitent ExoOS comme *"Linux avec des capabilities"* alors qu'ExoOS est un système fondamentalement différent. Ce biais produit des recommandations qui, si elles étaient suivies telles quelles, auraient trois effets négatifs :

1. **Diluer** les avantages compétitifs uniques d'ExoOS (ExoPhoenix, ExoFS, IPC fastpath, ExoShield)
2. **Introduire** des dépendances architecturalement incompatibles (systemd, PAM, libsodium FFI, D-Bus via zbus)
3. **Rater** des opportunités majeures non couvertes : pile graphique complète (iced/wgpu/winit), observabilité native, ExoPhoenix-safety des libs

Ce document recense chaque problème identifié, le justifie par référence au code noyau réel, et oriente vers les documents de spécification correctifs.

---

## 2. Problèmes Transversaux aux Deux Roadmaps

### 2.1 Le Biais "Linux-Compatible d'Abord"

Les deux roadmaps commencent toujours par *"comment porter cette lib Linux sur ExoOS"* au lieu de *"quelle valeur cette lib apporte-t-elle aux primitives ExoOS existantes"*. 

**Symptôme concret :** La roadmap v1 recommande de créer `exo-libc` comme fork de musl/relibc et d'implémenter 241 syscalls manquants. C'est une stratégie correcte pour la compatibilité applicative, mais elle est présentée comme *la* priorité numéro un — avant même que les crates natives ExoOS (`exo-net`, `exo-crypto`) soient spécifiées.

**Conséquence :** Les libs natives ExoOS (qui exploitent l'IPC SPSC à 50M msgs/s, les capabilities, l'ExoFS) restent vides pendant qu'on empile des couches de compatibilité coûteuses.

**Correction :** Inverser la priorité. Les libs natives d'abord, la compatibilité POSIX ensuite comme couche d'émulation.

### 2.2 Aucune Mention d'ExoPhoenix-Safety

Le noyau implémente un mécanisme de résurrection dual-kernel (ExoPhoenix). Les libs userland doivent être **resurrection-safe** : elles ne peuvent pas conserver d'état qui devient invalide après une bascule A→B ou B→A.

**Ce que les roadmaps ignorent :**
- Les sockets smoltcp conservent un état interne (numéros de séquence TCP, fenêtres). Que se passe-t-il lors d'une bascule ExoPhoenix pendant une connexion active ?
- Les arènes d'allocateur (snmalloc, jemalloc) maintiennent des pointeurs vers des pages physiques. Ces pointeurs survivent-ils à une résurrection ?
- Les clés dans le `crypto_server` ont-elles un cycle de vie aligné sur les epochs ExoFS ?

**Correction :** Chaque spec de lib doit inclure une section *ExoPhoenix-Safety* définissant les invariants à maintenir.

### 2.3 Le Ring0/Ring1 Boundary Est Ignoré

ExoOS sépare strictement :
- **Ring0** : kernel (mémoire, scheduler, sécurité, IPC fondamental)
- **Ring1** : serveurs système (`network_server`, `crypto_server`, `vfs_server`, etc.)
- **Ring3** : applications userland

Les roadmaps intègrent des libs indifféremment sans préciser dans quel ring elles s'exécutent. Exemple : `smoltcp` peut légitimement s'exécuter dans `network_server` (Ring1) **ou** dans une application (Ring3) selon le modèle choisi. Les deux ont des implications de sécurité et de performance radicalement différentes.

**Règle DRV-ARCH-01 (architecture ExoOS) :** Zéro logique de driver en Ring0. Si une lib gère du matériel, elle est Ring1. Si elle fournit une abstraction applicative, elle est Ring3.

### 2.4 L'IPC Fastpath n'Est Jamais Exploité

Le noyau implémente un SPSC ring optimisé avec séparation de cache-line :

```rust
// kernel/src/ipc/ring/spsc.rs
// PERFORMANCE CIBLE : > 50 millions de msgs/s en SPSC par canal @ 3 GHz.
pub struct SpscRing {
    head: CachePad,  // [AtomicU64 + 56 bytes padding = 64 bytes = 1 cache line]
    ...
}
```

Et un `fastcall_asm.s` pour les IPC synchrones ultra-rapides. **Aucune des deux roadmaps ne mentionne ce chemin rapide** pour les libs qui nécessitent une latence minimale (cryptographie in-process, allocation mémoire, I/O courte).

### 2.5 ExoFS Sous-Exploité

ExoFS n'est pas "un autre système de fichiers". Ses propriétés uniques sont :
- **Identifiants de blobs content-addressed** : déduplication automatique
- **Epochs atomiques** : snapshots O(1), rollback garanti
- **Relations typées** : graphe d'objets, pas une hiérarchie d'inodes
- **Moves O(1)** quelle que soit la localisation dans le namespace
- **Chiffrement par objet** (xchacha20 + clé par capability)
- **fsck 4 phases** avec récupération en ligne

Les roadmaps traitent ExoFS comme un "backend VFS parmi d'autres". La lib `exo-fs` devrait au contraire **exposer ces primitives uniques** aux applications.

---

## 3. Problèmes Spécifiques — Roadmap v1

### 3.1 Réseau

| Problème | Référence code | Gravité |
|----------|---------------|---------|
| Suggère de créer un "thin layer exo-libc" exposant `socket()` | Contradictoire avec le modèle IPC capability | Élevée |
| Ne mentionne pas `SYS_DMA_ALLOC=534` pour les transferts réseau DMA | `kernel/src/syscall/numbers.rs` | Élevée |
| `rtnetlink` suppose l'existence de l'API Netlink Linux | Ring1 n'a pas de Netlink | Élevée |
| Boucle `smoltcp.poll()` sans mention du `sched_yield` ExoOS | Risque de busy-loop en Ring1 | Moyenne |

### 3.2 Cryptographie

| Problème | Référence code | Gravité |
|----------|---------------|---------|
| Recommande `libsodium` (bibliothèque C) | ExoOS est `no_std` Rust — FFI C = risque de panic, pas de `std::alloc` | Critique |
| Pas de mention du `TRNG` (Random Number Generator matériel) | Le noyau a `security/crypto/rng.rs` avec RDRAND/RDSEED | Élevée |
| `ring` utilise `SystemRandom::new()` qui appelle `getrandom()` | `getrandom` doit être câblé vers le `crypto_server` | Élevée |
| Constante-time mentionnée mais sans référence à `subtle` crate | `kernel/src/security/crypto/` utilise déjà des ops CT | Moyenne |

### 3.3 Systèmes de Fichiers

| Problème | Référence code | Gravité |
|----------|---------------|---------|
| Suggère de porter `libfuse` (C) comme option valide | Incompatible avec le modèle de sécurité capability | Élevée |
| `ext4_lwext4` est un wrapper C — même problème que libsodium | Préférer une impl Rust pure ou un bridge minimal | Élevée |
| Mappage `uid/gid` vers capabilities décrit comme "à décider" | Cette décision doit être prise maintenant — la réponse est : on les ignore | Moyenne |

### 3.4 Allocateurs

| Problème | Référence code | Gravité |
|----------|---------------|---------|
| L'allocateur hybride kernel (SLUB + vmalloc) est ignoré | `kernel/src/memory/heap/allocator/hybrid.rs` | Élevée |
| La séparation nette kernel/userland des arènes n'est pas discutée | Critère de sécurité fondamental | Élevée |

### 3.5 Bibliothèques Système

| Problème | Référence code | Gravité |
|----------|---------------|---------|
| `linux-pam` : "potentiellement inutilisable" — c'est un euphémisme | PAM est **définitivement incompatible** avec le modèle capability | Critique |
| `systemd-upstream` + `launchd-upstream` : "trop lourds mais inspirants" | Ils doivent être rejetés, pas "inspirés". L'`init_server` suffit | Élevée |
| `shadow-rs` (gestion /etc/shadow) : sans équivalent dans ExoOS | À rejeter — il n'y a pas d'utilisateurs dans ExoOS | Critique |
| `libudev-rs` via Netlink : impossible sans Netlink | Port via IPC `device_server` discuté mais incomplet | Moyenne |

### 3.6 Runtimes

| Problème | Référence code | Gravité |
|----------|---------------|---------|
| Tokio "sans son runtime" = 90% du travail perdu | Tokio **est** son runtime. Sans lui, c'est juste des types async | Critique |
| `rayon` via `std::thread` supposé non disponible — incorrect | ExoOS supporte `clone()` — rayon peut fonctionner via le scheduler | Moyenne |
| `async-std` et `tokio` traités comme équivalents | async-std est plus simple à porter mais moins performant | Faible |

---

## 4. Problèmes Spécifiques — Roadmap v2

La v2 améliore considérablement la v1 avec des exemples de code concrets. Cependant :

### 4.1 Réseau v2

Le code `ExoDevice` pour smoltcp est techniquement correct mais incomplet :

```rust
// v2 propose ceci :
Err(IpcError::WouldBlock) => None,
Err(e) => panic!("Erreur réseau : {:?}", e),  // ← INTERDIT en Ring1
```

Un `panic!` dans un serveur Ring1 tue le serveur. Il faut `Err(IpcError::...)` propagé vers l'appelant pour décision de relance.

### 4.2 Cryptographie v2

L'approche `ipc_crypto(request)` est juste conceptuellement mais :
- Les `Vec<u8>` passés en IPC supposent une heap disponible. En Ring1 strict, on préfère des buffers de taille fixe ou un SHM pré-alloué.
- `ring::SystemRandom::new()` dans le `crypto_server` est correct, mais le TRNG matériel ExoOS (`security/crypto/rng.rs`) doit être la source primaire.

### 4.3 FS v2

Le `ExoBlockDevice` avec `ipc_block_read` est correct mais :
- Il manque la gestion des erreurs d'epoch ExoFS (si le volume est en cours de snapshot, les écritures doivent être barrées)
- La stratégie d'invalidation du cache lors d'une bascule ExoPhoenix n'est pas mentionnée

### 4.4 Alloc v2

Le code `ExoSnmalloc` est un exemple valide, mais critique majeure :

```rust
unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
    let size = layout.size().max(layout.align());  // ← Bug : .max() est incorrect
    // La taille doit être alignée sur layout.align(), pas le max des deux
    match exo_mmap(size) { ... }
}
```

La taille correcte est `layout.size()` arrondie au multiple supérieur de `layout.align()`, ce qui est différent du `max`.

---

## 5. Bibliothèques Absentes des Deux Roadmaps

Les bibliothèques suivantes sont présentes dans `vendors/` mais **totalement ignorées** par les deux roadmaps :

| Lib | Rôle | Urgence |
|-----|------|---------|
| `iced-upstream` | Framework GUI déclaratif | Haute — premier shell graphique |
| `wgpu-upstream` | GPU/WebGPU abstraction | Haute — rendu hardware-accéléré |
| `winit-upstream` | Gestion fenêtres et événements | Haute — nécessaire pour iced/wgpu |
| `zbus-upstream` | IPC D-Bus (Freedesktop) | Critique — **à REJETER** : conflict avec IPC ExoOS |

Ces quatre bibliothèques définissent la **pile graphique complète** d'ExoOS. Leur absence des roadmaps est une lacune majeure.

---

## 6. Tableau de Verdict par Bibliothèque

| Bibliothèque | Verdict Roadmaps | Verdict Correct | Motif |
|---|---|---|---|
| `smoltcp` | ✅ Conserver | ✅ Conserver | Excellent fit no_std |
| `hickory-dns` | ✅ Conserver | ✅ Conserver | Async DNS, no_std possible |
| `dhcp4r` | ✅ Conserver | ✅ Conserver | Léger, no_std |
| `rtnetlink` | ✅ Adapter | ❌ Rejeter | Dépend de Netlink Linux |
| `hyper` | ✅ Adapter | ⚠️ Post-v0.2 | Dépend de tokio runtime |
| `axum` | ✅ Adapter | ⚠️ Post-v0.2 | Dépend de hyper + tokio |
| `rustls` | ✅ Conserver | ✅ Conserver | TLS pur Rust, adaptable |
| `ring` | ✅ Conserver | ✅ Conserver | Primitives ASM rapides |
| `libsodium` | ✅ Adapter | ❌ Rejeter | Lib C, FFI non sûre en no_std |
| `rustcrypto-*` | ✅ Conserver | ✅ Conserver | Excellents pour no_std |
| `ext4-rs` | ✅ Adapter | ⚠️ Wrapper C risqué | Évaluer ext4 pure Rust |
| `rust-fatfs` | ✅ Conserver | ✅ Conserver | Pur Rust, no_std |
| `redoxfs` | ✅ Adapter | ✅ Conserver | Proche d'ExoFS conceptuellement |
| `libfuse` | ✅ Adapter | ❌ Rejeter | Lib C, modèle incompatible |
| `dlmalloc` | ✅ Fallback | ✅ Fallback | Acceptable comme repli |
| `jemallocator` | ✅ Conserver | ⚠️ Seulement Ring3 | OK en userland, pas en Ring1 |
| `snmalloc-rs` | ✅ Recommandé | ✅ Recommandé | Meilleur choix |
| `musl` | ✅ Forker | ✅ Forker | Base solide pour musl-exo |
| `relibc` | ✅ Conserver | ⚠️ Redondant avec musl | Un seul fork suffira |
| `linux-pam` | ⚠️ Adapter | ❌ Rejeter | Incompatible capability model |
| `shadow-rs` | ⚠️ Adapter | ❌ Rejeter | Pas d'utilisateurs dans ExoOS |
| `libudev-rs` | ✅ Adapter | ✅ Adapter via IPC | Port difficile mais utile |
| `log` | ✅ Conserver | ✅ Conserver | Trait standard, facile à router |
| `tracing` | ✅ Conserver | ✅ Conserver | Instrumentation structurée |
| `tokio` | ✅ Adapter runtime | ❌ Runtime rejeté | Porter tokio::sync/io uniquement |
| `async-std` | ✅ Adapter | ⚠️ Post-v0.2 | Redondant avec exo-rt |
| `rayon` | ✅ Adapter | ✅ Adapter | Port faisable via clone() |
| `cargo-chef` | ✅ Tooling | ✅ Tooling | Pas besoin d'adaptation |
| `pkgcraft` | ✅ Adapter | ⚠️ Post-v0.2 | Trop complexe pour v0.2 |
| `systemd` | ⚠️ Inspiration | ❌ Rejeter | Trop lourd, model incompatible |
| `launchd` | ⚠️ Inspiration | ❌ Rejeter | Même motif |
| `iced` | ❌ Non couvert | ✅ Priorité haute | Stack GUI v0.2 |
| `wgpu` | ❌ Non couvert | ✅ Priorité haute | Rendu GPU |
| `winit` | ❌ Non couvert | ✅ Priorité haute | Événements fenêtre/input |
| `zbus` | ❌ Non couvert | ❌ Rejeter | D-Bus = IPC alternative incompatible |

---

## 7. Conclusion

Les roadmaps v1 et v2 constituent un bon point de départ mais **ne doivent pas être suivies telles quelles**. La direction correcte pour ExoOS v0.2.0 est de :

1. **Rejeter fermement** : PAM, shadow-rs, systemd, launchd, libsodium, libfuse, rtnetlink, zbus
2. **Prioriser** les libs natives ExoOS (`exo-net`, `exo-crypto`, `exo-alloc`, `exo-runtime`) avec intégration deep dans l'IPC fastpath et le modèle capability
3. **Ajouter** la pile graphique complète (`iced` + `wgpu` + `winit`) absente des deux roadmaps
4. **Garantir** l'ExoPhoenix-safety de toutes les libs à état
5. **Exploiter** les primitives uniques d'ExoFS au lieu de les traiter comme un backend générique

Les documents de spécification suivants développent chaque point.

---

*claude-alpha — ExoOS Library Direction — ANALYSE-CRITIQUE-ROADMAPS.md*
