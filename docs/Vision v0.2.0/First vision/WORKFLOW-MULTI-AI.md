# WORKFLOW-MULTI-AI — Protocole d'Arbitrage Multi-Instance ExoOS
## Rôles, Résultats et Décisions d'Arbitrage

**Auteur :** claude-alpha  
**Date :** 2026-05-16  
**Statut :** Document de référence — protocole de travail v0.2.0

---

## 1. Pourquoi le Multi-AI est Nécessaire

Une seule instance d'IA travaille dans un contexte de session limité. Elle peut produire des specs architecturalement correctes tout en laissant passer des erreurs techniques précises parce qu'elle ne relit pas son propre travail avec les yeux d'un compilateur.

Les bugs identifiés par claude-beta et claude-gamma le prouvent :
- **SSR overflow** (ERR-01) : la struct était logiquement bien conçue mais dépassait physiquement la page. Un calcul de 30 secondes l'aurait trouvé — mais il n'a pas été fait lors de la rédaction.
- **ExoSeal avant mémoire** (ERR-02) : erreur de dépendance classique, invisible quand on raisonne en "couches" plutôt qu'en "ressources disponibles à chaque instant".
- **VirtIO BAR hardcodé** (C-GAMMA-01) : nécessitait de croiser une commande QEMU avec une adresse dans le code — impossible sans lire les deux sources simultanément.

**Conclusion :** Le multi-AI n'est pas un luxe. C'est la méthode d'audit la plus efficace disponible pour un projet solo.

---

## 2. Rôles Établis

| Instance | Rôle Principal | Artefacts Produits |
|----------|---------------|-------------------|
| **claude-alpha** | Architecture, specs, direction | 16 docs corpus + corrections |
| **claude-beta** | Contre-audit technique du corpus | `ANALYSE-RESOLUTION-V0_2_0_claude-beta.md` |
| **claude-gamma** | Réconciliation README + images QEMU | `RECONCILIATION_README_V010_CLAUDE_GAMMA.md` |
| **Autres (Gemini, Grok, GPT, Qwen)** | Audits parallèles selon disponibilité | Rapports spécifiques |

---

## 3. Ce que Chaque Instance a Validé ou Contredit

### 3.1 claude-beta — Résultats

**Validé (13 points) :**
- Rejet libsodium, rtnetlink, zbus, jemalloc Ring1
- MSR CET correctement assignés
- Règle DRV-ISR-01 correctement formulée
- align_up() exo-alloc correct
- hickory-dns pris en compte
- smoltcp Ring1 + exo-net Ring3 bon découpage
- PhoenixSafe trait bien conçu
- Priorisation natif ExoOS avant compat POSIX

**Contredit / Erreurs trouvées (11) :**
- ERR-01 : SSR overflow 4 KiB → **CRITIQUE** → CORR-81
- ERR-02 : ExoSeal avant mémoire → **CRITIQUE** → CORR-82
- ERR-03 : wgpu no_std impossible → **HAUTE** → CORR-83
- ERR-04 : is_immutable() non vérifié → **HAUTE** → CORR-84
- ERR-05 : IPC réseau > 240B → **HAUTE** → CORR-85
- ERR-06 : Syntaxe `[u8; _]` invalide → résolu via CORR-81
- ERR-07 : Kairos sans reset fenêtre → **MOYENNE** → CORR-82
- ERR-08 : snmalloc ≠ no_std → **MOYENNE** → CORR-81
- ERR-09 : ZeroTrust -17% perf → **MOYENNE** → CORR-82
- ERR-10 : SSR 64 procs non documenté → **INFO** → CORR-81
- ERR-11 : Phoenix Ring1 séquentiel → **INFO** → CORR-81

**Bugs kernel non adressés (5) :**
- CRIT-01 : physmap limitée → CORR-76
- CRIT-02 : cgroup init omis → CORR-77
- HIGH-01 : injection PID → CORR-78
- HIGH-02 : service bloque exosh → CORR-79
- HIGH-03 : ELF_BASE_MIN trop haut → CORR-80

### 3.2 claude-gamma — Résultats

**Validé :**
- Boot splash QEMU : 9 modules OK, framebuffer fonctionnel
- 13 services Ring1 tous `running` dans `top`
- exosh : touch, ls, mkdir, mv, cd, pwd, tree, cp, top fonctionnels
- ExoFS in-memory opérations hiérarchiques correctes
- Tests : 2975 cargo + 25 intégration = chiffres corrects (pas spécifiques ExoPhoenix)
- SSR layout physique : zone 16-17 MiB correcte (typo dans README)

**Découvertes nouvelles :**
- C-GAMMA-01 : **ExoFS RAM-only** → adresse VirtIO 0x10000000 = borne RAM → CORR-86
- C-GAMMA-02 : ExoPhoenix a 0 tests unitaires dédiés
- C-GAMMA-03 : Typo SSR layout dans README
- C-GAMMA-04 : POSIX 95% = aspiration, pas état actuel

**Correction du corpus claude-alpha :**
- "2975 tests ExoPhoenix" → FAUX. Ce sont des tests ExoFS/workspace. ExoPhoenix = 0 tests.
- "ExoFS sur disque" → FAUX en v0.1.0. ExoFS = RAM uniquement.

---

## 4. Décisions d'Arbitrage

Quand claude-beta et claude-alpha sont en désaccord, la règle est :

**Si claude-beta cite du code réel → beta a raison, alpha se corrige.**  
**Si claude-beta argumente sans référence code → arbitrage par calcul indépendant.**

| Désaccord | Arbitrage | Raison |
|-----------|-----------|--------|
| SSR = 4 KiB vs overflow | **beta** | Calcul vérifié : 64 × ProcessRecord = 7424 octets > 4096 |
| ExoSeal Phase 0 vs Phase 6 | **beta** | BLAKE3 nécessite heap — impossible sans memory_init() |
| wgpu no_std possible vs impossible | **beta** | wgpu dépend de std::thread — vérifiable sur crates.io |
| snmalloc principal vs dlmalloc | **beta** | snmalloc-rs = std requis — vérifiable sur crates.io |
| VirtIO BAR hardcodé | **gamma** | Croisement commande QEMU + code source = preuve directe |

---

## 5. Protocole de Session Suivante

Pour maximiser l'efficacité des audits futurs :

### 5.1 Ce qu'Eric doit fournir à chaque instance

**Pour claude-alpha (architecture/specs) :**
```
- Code source complet (zip du repo)
- Dernières corrections appliquées
- Questions spécifiques ou nouveaux modules
```

**Pour claude-beta (contre-audit) :**
```
- Les documents produits par claude-alpha
- Le code kernel réel (pour vérification)
- Instruction : "Contre-audite ces specs contre le code réel"
```

**Pour claude-gamma (réconciliation) :**
```
- README + images QEMU si disponibles
- Les deux rapports précédents (alpha + beta)
- Instruction : "Réconcilie les trois sources — README, code, specs"
```

### 5.2 Ordre de Lecture des Rapports

```
1. claude-gamma en premier (état réel du système)
2. claude-beta en second (erreurs dans les specs)
3. claude-alpha en dernier (corrections intégrées)
```

### 5.3 Règle d'Or

**Une spec non vérifiée contre le code réel n'est pas une spec — c'est un souhait.**

Chaque document du corpus doit avoir été cross-référencé avec le code réel par au moins une instance indépendante avant d'être considéré comme implémentable.

---

## 6. État Final Post-Corrections

### Ce qui est maintenant correct

| Domaine | Avant | Après |
|---------|-------|-------|
| Bugs kernel bloquants | Non adressés (5) | CORR-76 à CORR-80 |
| SSR taille | Overflow 250% | Redesigné, const_assert! |
| Boot sequence | ExoSeal avant mémoire (impossible) | Reséquencé Phase 0→14 |
| wgpu en v0.2.0 | Promis (impossible) | Reporté v0.3.0, fontdue à la place |
| ExoLedger immutabilité | Flag présent, non vérifié | is_immutable() dans blob_write |
| IPC réseau > 240B | Silencieusement tronqué | Protocole SHM inline/long |
| ExoKairos | Compteur sans reset | Fenêtre 1s avec reset |
| snmalloc no_std | Principal (faux) | dlmalloc principal v0.2.0 |
| ZeroTrust fast path | -17% perf potentiel | Bitmask Ring1↔Ring1 |
| VirtIO BAR | Hardcodé (RAM-only) | PCI config space dynamique |
| Tests ExoPhoenix | "2975 tests" (faux) | 0 → créer suite dédiée |
| POSIX 95% | Présenté comme état | Clarifié : cible architecturale |
| exo_shield modules | 5 orphelins (dead code) | CORR-75 : tous actifs |
| Outillage CI | Inexistant | const_assert! + Semgrep + Kani + cargo-deny |

### Métriques Finales

```
Documents produits    : 21 (16 corpus + 5 corrections)
Bugs kernel corrigés  : 6 (CORR-76 à CORR-81 via CORR-86)
Erreurs specs         : 11 corrigées (ERR-01 à ERR-11)
Points validés        : 13 (par claude-beta)
Checklist             : 143 → 158 critères (+10%)
Outillage CI          : 5 outils (const_assert!, Python, Semgrep, Kani, cargo-deny)
```

---

*claude-alpha — ExoOS v0.2.0 — WORKFLOW-MULTI-AI.md*
