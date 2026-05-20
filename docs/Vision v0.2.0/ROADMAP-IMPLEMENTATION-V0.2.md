# ROADMAP-IMPLEMENTATION-V0.2 — Plan d'Implémentation Détaillé
## ExoOS v0.2.0 — Phases, Ordre, Dépendances

**Auteur :** claude-alpha  
**Date :** 2026-05-14  
**Statut :** DOCUMENT DE TRAVAIL — Mis à jour à chaque milestone

---

## 1. Vue d'Ensemble des Phases

```
PHASE 0 — Fondations Critiques        (Bloquant tout le reste)
PHASE 1 — Sécurité Complète          (Bloquant toute app)
PHASE 2 — Services Réseau & Crypto   (Bloquant les apps réseau)
PHASE 3 — Stockage & FS              (Bloquant l'installeur)
PHASE 4 — Processus & POSIX Compat   (Bloquant exo compat)
PHASE 5 — Installeur & PKG           (Bloquant les apps POSIX)
PHASE 6 — Graphisme & Shell          (Expérience utilisateur)
PHASE 7 — Observabilité & Qualité    (Stabilité ~98%)
PHASE 8 — Validation Finale          (Release candidate)
```

---

## 2. PHASE 0 — Fondations Critiques

**Objectif :** Sans cette phase, rien d'autre ne peut démarrer.

### 0.1 — Fix SSR Bitmask 256-core [BLOQUANT]

**Priorité :** P0  
**Fichiers :** `forge.rs`, `handoff.rs`, `isolate.rs`, `ssr.rs`  
**Description :** Migrer `u64` → `[u64; 4]` pour le masque de cores actifs dans le SSR ExoPhoenix

```rust
// Fichiers affectés :
// kernel/src/exophoenix/forge.rs
// kernel/src/exophoenix/handoff.rs
// kernel/src/exophoenix/isolate.rs
// kernel/src/exophoenix/ssr.rs

// AVANT :
pub active_cores: u64,

// APRÈS :
pub active_cores: [u64; 4],  // 256 cores max
```

**Tests :** `phoenix_test::ssr_bitmask_256_cores` → PASS  
**Dépend de :** Rien  
**Bloque :** ExoPhoenix sous charge multi-core

---

### 0.2 — exo-alloc (snmalloc + mmap) [BLOQUANT]

**Priorité :** P0  
**Fichiers :** `libs/exo-alloc/src/`  
**Description :** Allocateur userland standard basé sur snmalloc avec hooks mmap ExoOS

**Checklist :**
- [ ] `ExoAllocator` implémente `GlobalAlloc`
- [ ] `align_up()` correct (multiply supérieur de align, PAS max)
- [ ] `exo_mmap_anon()` appelle `SYS_MMAP` avec `MAP_ANON | MAP_PRIVATE`
- [ ] `exo_mremap()` pour `realloc` quand possible
- [ ] Feature flag pour backend dlmalloc (fallback)
- [ ] Tests : allocation 8B, 64B, 4KiB, 2MiB, déallocation, realloc

**Tests requis :**
```
exo_alloc_test::alloc_basic_sizes      PASS
exo_alloc_test::dealloc_correct        PASS
exo_alloc_test::realloc_grow           PASS
exo_alloc_test::alignment_respected    PASS
exo_alloc_test::concurrent_alloc_free  PASS
```

**Dépend de :** SYS_MMAP kernel (déjà implémenté)  
**Bloque :** Toutes les crates Ring3 qui allouent de la mémoire

---

### 0.3 — generic-rt : TLS + TCB Access [BLOQUANT]

**Priorité :** P0  
**Fichiers :** `libs/generic-rt/src/`  
**Description :** Infrastructure runtime Thread-Local Storage correctement câblée vers le TCB ExoOS (GS base)

**Checklist :**
- [ ] `__tls_get_addr` implémenté correctement pour les ELF dynamiques
- [ ] `gs:[0x20]` current TCB accessible depuis userland Ring3
- [ ] TLS variables initialisées avant le `main()` du processus
- [ ] TLS survit à un `clone()` (chaque thread a son propre TLS)

**Dépend de :** kernel/percpu (déjà implémenté)  
**Bloque :** musl-exo (pthread, errno), exo-runtime

---

## 3. PHASE 1 — Sécurité Complète

**Objectif :** Tous les composants de sécurité actifs en production.

### 1.1 — ExoSeal Activation [P0]

**Fichier :** `kernel/src/security/exoseal.rs`

**Checklist :**
- [ ] `exoseal_verify_boot_chain()` appelé en Phase 0 (avant tout)
- [ ] Hash du kernel binaire vérifié au boot
- [ ] Hash des serveurs Ring1 vérifiés
- [ ] Mode QEMU : hash stocké en mémoire (pas TPM requis)
- [ ] `EXOSEAL_DEV_BYPASS` loggé dans ExoLedger quand actif

---

### 1.2 — ExoCage : Tous les Mécanismes Hardware [P0]

**Fichier :** `kernel/src/security/exocage.rs`

**Checklist :**
- [ ] SMEP activé (CR4.SMEP) sur BSP + tous APs
- [ ] SMAP activé (CR4.SMAP) sur BSP + tous APs
- [ ] KPTI actif sur toutes les transitions kernel↔user
- [ ] CET Shadow Stack activé (MSR_IA32_U_CET)
- [ ] CET IBT activé (MSR_IA32_S_CET)
- [ ] NX/XD actif (EFER.NXE)
- [ ] IBRS + SSBD actifs (Spectre mitigations)
- [ ] `exocage_verify_active()` appelé après Phase 5, panic si incomplet

---

### 1.3 — Zero Trust Layer sur IPC [P0]

**Fichier :** `kernel/src/security/zero_trust/`

**Checklist :**
- [ ] `ZeroTrustLabel` attaché à chaque message SpscRing
- [ ] `zero_trust::check_ipc()` sur chaque `ipc_send` du fast path
- [ ] Ring3 → Ring3 direct IPC bloqué (sauf SHM autorisé)
- [ ] IPC non conforme : bloqué + ExoLedger + ExoKairos débit
- [ ] Tests d'intrusion : bypass tentative → 0 succès

---

### 1.4 — CapToken : Couverture Complète [P0]

**Fichier :** `kernel/src/security/capability/`

**Checklist :**
- [ ] Chaque accès FS passe par `capability::verify()`
- [ ] Chaque IPC vers Ring1 vérifié
- [ ] Chaque accès réseau vérifié
- [ ] Révocation propagée immédiatement à tous les tokens dérivés
- [ ] Test : accès avec cap révoquée → `EXO-0410` instantané

---

### 1.5 — ExoKairos : Budgets en Production [P1]

**Fichier :** `kernel/src/security/exokairos.rs`

**Checklist :**
- [ ] Budget initialisé à la création de chaque processus Ring3 (100ms/s par défaut)
- [ ] `update_kairos_budget()` appelé à chaque context switch
- [ ] Throttle à 100% du budget (pas kill)
- [ ] Kill à 200% cumulé
- [ ] ExoLedger entrée sur dépassement

---

### 1.6 — ExoLedger : Persistance & Chaîne [P0]

**Fichier :** `kernel/src/security/exoledger.rs`

**Checklist :**
- [ ] Journal persisté dans ExoFS (objet sealed)
- [ ] Chaîne BLAKE3 vérifiée au boot
- [ ] Toutes les catégories d'événements auditées (voir section 3.6 SPEC-SECURITY)
- [ ] `exo audit` fonctionnel avec vérification de chaîne
- [ ] Impossible de modifier/supprimer une entrée passée

---

### 1.7 — ExoShield : IOMMU Statique [P0]

**Fichier :** `kernel/src/security/iommu/`

**Checklist :**
- [ ] IOMMU activé avant tout démarrage de driver Ring1
- [ ] Domaines IOMMU séparés : NET / BLOCK / BLACKHOLE
- [ ] `SYS_DMA_ALLOC=534` retourne (virt, iova) dans le bon domaine
- [ ] `IommuFaultQueue` actif (CAS-strong)
- [ ] Test DMA hors plage → fault détectée + DMA stoppé

---

### 1.8 — ExoNMI : Watchdog Armé [P1]

**Fichier :** `kernel/src/security/exonmi.rs`

**Checklist :**
- [ ] NMI watchdog armé toutes les 200ms
- [ ] Heartbeat incrémenté → visible par ExoPhoenix sentinel
- [ ] Canaries stack kernel vérifiés dans le handler NMI
- [ ] IDT integrity check au NMI

---

## 4. PHASE 2 — Services Réseau & Cryptographie

### 2.1 — crypto_server complet [P0]

**Dépend de :** Phase 0 (exo-alloc), Phase 1.4 (CapToken)

**Checklist :**
- [ ] TRNG matériel comme source d'entropie primaire (RDRAND/RDSEED)
- [ ] AES-GCM-256 via `rustcrypto-aeads`
- [ ] ChaCha20-Poly1305 via `rustcrypto-aeads`
- [ ] SHA-256, SHA-3, BLAKE3 via `rustcrypto-hashes`
- [ ] Argon2id via `rustcrypto-password-hashes`
- [ ] ECDSA P-256 + Ed25519 via `rustcrypto-elliptic-curves`
- [ ] RSA-OAEP via `rustcrypto-rsa`
- [ ] HKDF via `rustcrypto-kdfs`
- [ ] Clés privées JAMAIS exportées hors du serveur
- [ ] Interface IPC documentée dans SPEC-EXO-CRATES.md respectée
- [ ] Tests : chaque primitive, round-trip, vecteurs de test NIST

---

### 2.2 — network_server complet (smoltcp + dhcp4r + hickory-dns) [P0]

**Dépend de :** Phase 0, virtio-net driver

**Checklist :**
- [ ] smoltcp IPv4/IPv6 opérationnel dans la boucle principale
- [ ] DHCP via dhcp4r (adresse IP obtenue automatiquement)
- [ ] DNS via hickory-dns (résolution `A`, `AAAA`, `CNAME`)
- [ ] `sys_sched_yield()` dans la boucle principale (pas de busy-loop)
- [ ] Zéro `panic!` dans le serveur (tous les chemins d'erreur propagés)
- [ ] TCP + UDP fonctionnels
- [ ] Interface IPC Ring3 (`exo-net`) documentée respectée
- [ ] TLS via rustls au-dessus de smoltcp

---

### 2.3 — rustls TLS 1.3 [P1]

**Dépend de :** 2.1 (crypto_server), 2.2 (network_server)

**Checklist :**
- [ ] TLS 1.3 uniquement (TLS 1.2 désactivé)
- [ ] Certificats racine depuis le keystore crypto_server
- [ ] Validation de certificats fonctionnelle
- [ ] Test : curl vers https://example.com → 200 OK

---

## 5. PHASE 3 — Stockage & Filesystem

### 3.1 — ExoFS 4-phase fsck en production [P0]

**Checklist :**
- [ ] fsck phase 1 (inode scan) → PASS sur volume test
- [ ] fsck phase 2 (directory check) → PASS
- [ ] fsck phase 3 (connectivity check) → PASS
- [ ] fsck phase 4 (bad block) → PASS
- [ ] fsck en ligne (sans démonter le volume)
- [ ] Recovery automatique après crash simulé

---

### 3.2 — vfs_server : primitives ExoFS natives [P0]

**Checklist :**
- [ ] `open`, `close`, `read_at`, `write_at` fonctionnels
- [ ] `mkdir`, `rmdir`, `unlink`, `rename` (O(1) pour rename dans ExoFS)
- [ ] `stat` retournant les champs ExoFS (pas simulés POSIX)
- [ ] `readdir` / `getdents64`
- [ ] Epochs : commit atomique après chaque écriture
- [ ] Snapshots : `snapshot_create()` opérationnel
- [ ] Relations typées : `relation_create()` opérationnel
- [ ] Content hash accessible : `get_content_hash()`

---

### 3.3 — fat_server (rust-fatfs) [P1]

**Dépend de :** vfs_server

**Checklist :**
- [ ] Montage de volumes FAT32 via virtio-blk
- [ ] Lecture/écriture de fichiers FAT32
- [ ] Exposition via vfs_server (mount point dans ExoFS)
- [ ] Test : monter une image FAT32 QEMU, lire un fichier

---

### 3.4 — ext_server ou nvme_blk_server [P1]

**Dépend de :** nvme driver (Phase 0)

**Checklist :**
- [ ] Accès blocs bruts NVMe via ext_server
- [ ] Montage ext4 (via `ext4-rs` ou lecture directe)
- [ ] Test : lire une partition ext4 depuis QEMU

---

## 6. PHASE 4 — Processus & Compat POSIX

### 4.1 — Fork/CoW fixes P0 [BLOQUANT]

**Fichiers :** `kernel/src/process/lifecycle/fork.rs`, `memory/cow/`

**Bugs connus à corriger :**
- [ ] TLB shootdown deadlock (IPI ACK jamais reçu sur single-CPU) → fix : skip shootdown si single-CPU
- [ ] VMA tree non cloné lors de `fork()` → fix : deep clone de la VMA tree
- [ ] `KERNEL_FAULT_ALLOC` opère sur le mauvais espace d'adressage → fix : vérifier CR3 avant CoW

---

### 4.2 — musl-exo : Syscalls Priorité 1 [P0]

**Dépend de :** Phase 3 (vfs_server), Phase 2 (network_server), fork fix

**Checklist (127 syscalls requis) :**
Voir tableau complet dans `SPEC-EXO-LIBC.md` — Priorités 1 et 2

**Tests d'intégration :**
- [ ] `musl_exo_test::fork_exec_wait` → PASS
- [ ] `musl_exo_test::socket_tcp_connect_send_recv` → PASS
- [ ] `musl_exo_test::getdents64_readdir` → PASS

---

### 4.3 — Syscalls manquants identifiés [P0]

**Fichier :** `kernel/src/syscall/`

**Checklist :**
- [ ] `SYS_GETDENTS64` (N°217) → implémenté dans vfs_server bridge
- [ ] `SYS_GETCWD` (N°79) → implémenté
- [ ] `SYS_CLONE` (N°56) → tous les flags utiles (CLONE_THREAD, CLONE_VM, CLONE_FS)
- [ ] `SYS_FUTEX` (N°202) → partiel (FUTEX_WAIT, FUTEX_WAKE — suffisant pour pthread)

---

## 7. PHASE 5 — Installeur `exo` & PKG

### 5.1 — exo-pkg binaire [P1]

**Dépend de :** Phase 3 (ExoFS), Phase 2 (réseau), Phase 1 (crypto sig)

**Checklist :**
- [ ] `exo install <pkg>` : résolution + téléchargement + vérification sig + injection ExoFS
- [ ] `exo compat install <pkg>` : idem + génération sandbox POSIX
- [ ] `exo remove <pkg>` : révocation caps + nettoyage ExoFS
- [ ] `exo list` : liste des apps installées
- [ ] `exo doctor` : diagnostic complet du système
- [ ] Affichage des capabilities requises avant confirmation

---

### 5.2 — Premier `exo compat install calendar` fonctionnel [MILESTONE]

**Dépend de :** Phase 4 (musl-exo), Phase 5.1 (exo-pkg)

**Critère :** `exo compat install calendar` → `exo compat run calendar` → affiche un calendrier texte.

---

### 5.3 — `exo compat install curl` fonctionnel [MILESTONE]

**Dépend de :** 5.2 + réseau TLS

**Critère :** `curl https://example.com` depuis le shell → 200 OK affiché.

---

## 8. PHASE 6 — Graphisme & Shell

### 6.1 — fb_server stable [P1]

**Checklist :**
- [ ] Framebuffer GOP UEFI opérationnel
- [ ] Blit depuis SHM Ring3 fonctionnel
- [ ] Événements input_server → fb_server → Ring3 routés
- [ ] Pas de tearing visible (double buffering)

---

### 6.2 — wgpu software rasterizer [P1]

**Checklist :**
- [ ] wgpu compile en no_std (ou avec alloc uniquement)
- [ ] Backend software (`wgpu::Backends::empty()` + fallback)
- [ ] Surface basée sur le SHM fb_server
- [ ] Rendu d'un rectangle coloré → visible sur le framebuffer

---

### 6.3 — iced + exosh [P1]

**Checklist :**
- [ ] iced compile avec l'executor exo-runtime
- [ ] Prompt `$ ` visible et interactif
- [ ] `exo ls` dans le shell → affichage format capability natif
- [ ] `exo install` depuis le shell → fonctionnel
- [ ] Pas de crash sur entrée invalide

---

## 9. PHASE 7 — Observabilité & Qualité

### 7.1 — monitor_server [P2]

**Checklist :**
- [ ] Réception de logs depuis tous les serveurs Ring1
- [ ] Réception depuis les apps Ring3
- [ ] Persistance dans ExoFS
- [ ] `exo log` fonctionnel avec filtres
- [ ] `exo metrics` affichant CPU, mémoire, IPC, réseau

---

### 7.2 — tracing dans tous les serveurs Ring1 [P2]

**Checklist :**
- [ ] `network_server` instrumenté (spans par connexion TCP)
- [ ] `crypto_server` instrumenté (spans par opération crypto)
- [ ] `vfs_server` instrumenté (spans par opération FS)
- [ ] `device_server` instrumenté (events IRQ, DMA)

---

### 7.3 — Suite de Tests de Sécurité [P0]

**Checklist :**
```
security_test::exoseal_verify_chain          PASS
security_test::exocage_all_mechanisms        PASS
security_test::zerotrust_ipc_blocked         PASS
security_test::captoken_access_denied        PASS
security_test::captoken_revocation_immediate PASS
security_test::captoken_no_privilege_escalation PASS
security_test::exokairos_throttle_at_100pct  PASS
security_test::exokairos_kill_at_200pct      PASS
security_test::exoledger_chain_integrity     PASS
security_test::exoledger_immutable           PASS
security_test::exoshield_dma_fault           PASS
security_test::exonmi_watchdog_fires         PASS
security_test::full_attack_simulation        PASS
```

---

## 10. PHASE 8 — Validation Finale (Release Candidate)

### Critères de Release ExoOS v0.2.0

| Critère | Seuil | Méthode de validation |
|---------|-------|----------------------|
| Kernel stability | ≥ 98% | Stress test 2h+ sans crash |
| ExoPhoenix | 100% | phoenix_stress_test (1000 bascules) |
| Sécurité | 13/13 tests PASS | security_integration_tests |
| musl-exo syscalls | ≥ 127 | musl_exo_test suite |
| `calendar` POSIX | Fonctionnel | Test manuel |
| `curl https` POSIX | Fonctionnel | Test manuel |
| Tests unitaires | 100% PASS | cargo test --all |
| Tests intégration | 100% PASS | cargo test --test integration |
| ExoLedger integrity | 0 rupture de chaîne | exo audit --verify-chain |
| Memory leaks (2h) | 0 | Valgrind-like ou balloon test |
| IPC drops (1h charge) | 0 | monitor_server metrics |

### Checklist Finale

- [ ] Tous les P0 résolus
- [ ] Tous les P1 résolus
- [ ] Suite de tests complète : 0 FAIL, 0 SKIP (sauf post-v0.2.0 explicitement marqués)
- [ ] CORR-75 à CORR-N générés pour chaque correction de cette phase
- [ ] `VISION-V0.2.0.md` — tous les piliers validés
- [ ] `exo doctor` → 0 erreur critique
- [ ] Git tag : `v0.2.0-rc1` puis `v0.2.0`

---

## 11. Graphe de Dépendances Simplifié

```
[0.1 SSR fix] ──────────────────────────────────► [ExoPhoenix parfait]
[0.2 exo-alloc] ──────────────────────────────┐
[0.3 generic-rt] ─────────────────────────┐   │
                                           │   │
[1.x Sécurité] ────────────────────────┐  │   │
                                        │  │   │
[2.1 crypto_server] ───────────────┐   │  │   │
[2.2 network_server] ──────────┐   │   │  │   │
[2.3 rustls] ──────────────┐   │   │   │  │   │
                            │   │   │   │  │   │
[3.1 ExoFS fsck] ───────┐  │   │   │   │  │   │
[3.2 vfs_server] ───┐   │  │   │   │   │  │   │
                    │   │  │   │   │   │  │   │
[4.1 fork/CoW fix]  │   │  │   │   │   │  │   │
[4.2 musl-exo] ─────┘   │  │   │   │   │  │   │
         │               │  │   │   │   │  │   │
         ▼               ▼  ▼   ▼   ▼  ▼  ▼   ▼
    [5.1 exo-pkg] ──► [5.2 calendar] ──► [5.3 curl]
         │
         ▼
    [6.x graphisme + exosh]
         │
         ▼
    [7.x observabilité]
         │
         ▼
    [8. Release v0.2.0]
```

---

*claude-alpha — ExoOS v0.2.0 — ROADMAP-IMPLEMENTATION-V0.2.md*
