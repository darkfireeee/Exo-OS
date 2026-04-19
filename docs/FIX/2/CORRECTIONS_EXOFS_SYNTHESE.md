# Synthèse d'Audit ExoFS — Verdict sur Rapport Kimi + Bugs Nouveaux
## Commit de référence : 93616537 · 19 avril 2026

---

## 1. Tableau de vérité : 22 findings Kimi

| ID Kimi | Titre | Verdict | Sévérité réelle |
|---------|-------|---------|-----------------|
| FS-CRIT-01 | Checksum XOR naïf | ✅ Confirmé | P1 (flag optionnel — impact moindre qu'annoncé) |
| FS-CRIT-02 | POSIX layer absente | ⚠️ Exagéré | P0/P1 — partiellement stub, pas vide |
| FS-CRIT-03 | InodeNumber = [u8;32] | ❌ Faux | Aucun — `ObjectIno = u64` dans le code |
| FS-CRIT-04 | AtomicU64 dans structs on-disk | ❌ Faux scope | `epoch_record.rs` est correct ; stats = in-memory |
| FS-CRIT-05 | `const fn` atomique | ⚠️ Partiellement | La vraie critique est l'absence de validation, pas le `const fn` |
| FS-HIGH-01 | INLINE\_DATA\_MAX 512 vs 256 | ✅ Confirmé | P1 |
| FS-HIGH-02 | GC\_MIN\_EPOCH\_DELAY double def | ✅ Confirmé + Pire | **3 définitions** trouvées, pas 2 |
| FS-HIGH-03 | Superblock taille sans vérif | ⚠️ 50% Faux | `format()` OK, `mount()` sans garde (P1-06) |
| FS-HIGH-04 | align\_up overflow | ❌ Faux | `checked_add` déjà présent |
| FS-HIGH-05 | PathCache sans limite | ❌ Faux | `max` + `evict_one_lru()` existent |
| FS-MED-01 | Ordering::Relaxed insuffisant | ✅ Confirmé | P2 |
| FS-MED-02 | Symlinks non résolus | ✅ Confirmé | P2 — `resolve_symlink_chain()` existe mais non branchée |
| FS-MED-03 | Quotas non implémentés | ✅ Confirmé | P2 — syscall stub |
| FS-MED-04 | NUMA partiel | ✅ Confirmé | P2 |
| FS-MED-05 | Export ExoAR sans tests | ✅ Confirmé | P2 |
| FS-MED-06 | SYS\_EXOFS\_EPOCH\_META TODO | ✅ Confirmé | P2 |
| FS-MED-07 | Crypto shredding non branché | ✅ Confirmé | P2 |
| FS-MED-08 | Cache éviction dirty/clean | ✅ Confirmé | P0 (flush\_all supprime les dirty — voir P0-05) |
| FS-LOW-01 | Typos commentaires | ✅ Confirmé | Low |
| FS-LOW-02 | Compression no\_std non testée | ✅ Confirmé | Low |
| FS-LOW-03 | Nonce XChaCha20 non vérifié | ✅ Confirmé | Low |
| FS-LOW-04 | io\_uring stub | ✅ Confirmé | Low |

**Score Kimi : 14/22 corrects (64 %), 4/22 faux (18 %), 4/22 partiels (18 %)**

---

## 2. Bugs critiques manqués par Kimi (découverts à froid)

| ID | Description | Sévérité | Document |
|----|-------------|----------|----------|
| MISS-P0-01 | `vfs_read()` retourne des zéros silencieux | **P0** | P0_P1 § P0-01 |
| MISS-P0-02 | `vfs_write()` ne persiste rien dans BLOB\_CACHE | **P0** | P0_P1 § P0-02 |
| MISS-P0-03 | `flush_dirty_blobs()` marque dirty au lieu de flusher | **P0** | P0_P1 § P0-03 |
| MISS-P0-04 | Offsets de désérialisation du journal d'epoch incorrects | **P0** | P0_P1 § P0-04 |
| MISS-P0-05 | `flush_all()` supprime les données dirty sans les écrire | **P0** | P0_P1 § P0-05 |
| MISS-P1-01 | `vfs_rmdir()` sans vérification répertoire vide | P1 | P0_P1 § P1-01 |
| MISS-P1-02 | `vfs_readdir()` ne liste que `.` et `..` | P1 | P0_P1 § P1-02 |
| MISS-P1-03 | `COMMIT_STATE` non réinitialisé sur panique | P1 | P0_P1 § P1-03 |
| MISS-P1-04 | `GC_MIN_EPOCH_DELAY` en 3 exemplaires (pas 2) | P1 | P0_P1 § P1-04 |
| MISS-P1-05 | `exofs_init(0)` — disk\_size nul au boot | P1 | P0_P1 § P1-07 |
| MISS-P2-01 | `vfs_rename()` perd le contenu du blob | P2 | P2 § P2-03 |
| MISS-P2-02 | `PathCache::new_const()` max hardcodé à 16384 ≠ 10000 | P2 | P2 § P2-02 |

---

## 3. Ordre de correction recommandé

### Sprint 1 — Bloquant pour tout test E2E (1–3 jours)

1. **P0-04** Corriger les offsets de désérialisation du journal d'epoch
   → Sans ça, `verify_epoch_journal()` est toujours fausse
2. **P0-01** Connecter `vfs_read()` à `BLOB_CACHE`
3. **P0-02** Connecter `vfs_write()` à `BLOB_CACHE`
4. **P0-03** Corriger la sémantique de `flush_dirty_blobs()`
5. **P0-05** Renommer `flush_all()` → `drop_all()` + créer vrai `flush_all()`
6. **P1-07** Passer une `disk_size` réelle à `exofs_init()`

### Sprint 2 — Cohérence des données et POSIX minimal (3–7 jours)

7. **P1-01** `vfs_rmdir()` vérification répertoire vide
8. **P1-02** `vfs_readdir()` lister les vrais enfants
9. **P1-03** Guard RAII pour `COMMIT_STATE`
10. **P1-04** Unifier `GC_MIN_EPOCH_DELAY` (3 → 1 définition)
11. **P1-05** Unifier `INLINE_DATA_MAX` (512 → 256)
12. **P1-06** `mount()` avec vérification `MIN_DISK_SIZE`
13. **P2-03** `vfs_rename()` avec transfert de blob

### Sprint 3 — Renforcement qualité (1–2 jours)

14. **P1-08** Checksum XOR → Blake3 dans `epoch_commit.rs`
15. **P2-01** `Ordering::Relaxed` → `Ordering::Acquire` sur getters de config
16. **P2-02** `PathCache::new_const()` utiliser `PATH_CACHE_CAPACITY`
17. **P2-05** `reclaim_bytes()` corriger le calcul de bytes libérés
18. **FS-LOW-03** Validation taille nonce XChaCha20

---

## 4. Métriques de l'audit

| Métrique | Valeur |
|----------|--------|
| Fichiers audités | 292 (.rs dans fs/exofs/) |
| Findings P0 (crash/perte données) | **5** (tous manqués par Kimi) |
| Findings P1 (comportement incorrect) | **8** (dont 6 manqués par Kimi) |
| Findings P2 (qualité/cohérence) | **5** |
| Findings Kimi confirmés | 14/22 (64 %) |
| Findings Kimi faux | 4/22 (18 %) |
| Findings Kimi partiels/exagérés | 4/22 (18 %) |
| Bugs nouveaux découverts | **12** |

---

## 5. Évaluation révisée du module

| Dimension | Kimi | Audit croisé | Delta |
|-----------|------|--------------|-------|
| Qualité du code | B+ | B | Stub I/O = perte silencieuse |
| Conformité specs | C+ | C | vfs\_read/write non fonctionnels |
| Sécurité | B | B- | journal désérialisé incorrectement |
| Intégrité données | — | **D** | flush\_all détruit des dirty |
| Testabilité | C | C+ | Tests unitaires présents et corrects |
| Global | B- | **C+** | 5 P0 non détectés par Kimi |

> Le module n'est pas en état d'être utilisé en production. Les 5 bugs P0
> rendent toute opération I/O soit silencieusement fausse (`vfs_read` retourne
> des zéros), soit silencieusement perdue (`vfs_write`, `flush_dirty_blobs`,
> `flush_all`). Le journal d'epoch est désérialisé avec des offsets incorrects.

