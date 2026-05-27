# MASTER-CORRECTIONS-V0.2 — Synthèse des Erreurs Identifiées
## Contre-audit claude-beta + Réconciliation claude-gamma → Corrections claude-alpha

**Auteur :** claude-alpha  
**Date :** 2026-05-16  
**Sources :** `ANALYSE-RESOLUTION-V0_2_0_claude-beta.md` + `RECONCILIATION_README_V010_CLAUDE_GAMMA.md`  
**Statut :** DOCUMENT DE CORRECTION — Invalide et remplace les sections erronées du corpus initial

---

## Préambule

Trois instances ont analysé ExoOS v0.2.0 de manière indépendante.  
**claude-beta** a audité les 16 documents du corpus resolutionV0_2_0 contre le code kernel réel.  
**claude-gamma** a réconcilié le README v0.1.0, les images QEMU, et les audits précédents.  
**claude-alpha** (ce document) intègre toutes les corrections.

Verdict global : la vision architecturale est correcte, mais **11 erreurs techniques** dont **2 qui rendent le système non compilable ou non bootable** doivent être corrigées avant toute implémentation.

---

## BLOC 1 — Bugs Kernel v0.1.0 Non Adressés dans le Corpus (claude-beta)

Ces bugs existent dans le code kernel actuel. Sans les corriger, **rien du corpus ne peut s'exécuter**.

### CRIT-01 — `map_physmap()` jamais appelée → physmap 1 GiB max

**Impact :** Toute machine avec > 1 GiB de RAM → panique kernel au boot.  
**Symptôme :** `phys_to_virt()` sur adresse > 1 GiB → accès mémoire invalide.  
**Priorité dans ROADMAP :** Phase 0, avant tout le reste.  
**Correction :** `CORR-76.md`

### CRIT-02 — `cgroup::init()` omis → root cgroup invalide

**Impact :** Le scheduler ne peut pas attacher les processus Ring1 au root cgroup.  
**Symptôme :** Crash ou comportement indéfini lors du démarrage des serveurs Ring1.  
**Correction :** `CORR-77.md`

### HIGH-01 — Injection PID via magic number `len == 128`

**Impact :** Un processus Ring3 peut forger un PID en envoyant un message de 128 octets exactement.  
**Priorité :** Sécurité critique — à corriger avant ExoShield.  
**Correction :** `CORR-78.md`

### HIGH-02 — Service non-critique bloque `exosh`

**Impact :** Si `network_server` timeout au démarrage, `exosh` ne démarre jamais.  
**Correction :** `CORR-79.md`

### HIGH-03 — `USER_ELF_BASE_MIN` = 1 TiB rejette tous les ELF standards

**Impact :** Aucun binaire ELF standard (compilé pour base 0x400000) ne peut être chargé.  
**Symptôme :** `exo compat install calendar` installe mais le lancement échoue immédiatement.  
**Correction :** `CORR-80.md`

---

## BLOC 2 — Erreurs Techniques dans le Corpus claude-alpha (claude-beta)

### ERR-01 🔴 CRITIQUE — SSR struct dépasse 4 KiB de 250%

**Document original :** `SPEC-EXOPHOENIX-V0.2.md` §3.2  
**Problème :** La struct `SystemStateRecord` déclarée comme "4 KiB" contient en réalité :

```
Header               : ~61 octets
cap_table refs        : 44 octets
[ProcessRecord; 64]  : 64 × ~116 = 7 424 octets  ← DÉPASSE À LUI SEUL
[EndpointRecord; 128]: 128 × ≥20 = ≥2 564 octets
Timing               : 16 octets
TOTAL MINIMUM        : ~10 109 octets  ← 2.5× la page de 4 KiB
```

**Conséquence :** Corruption mémoire lors de toute bascule ExoPhoenix.  
**Correction :** `CORR-81.md` — Réduire les limites + supprimer le `align(4096)` trompeur.

---

### ERR-02 🔴 CRITIQUE — ExoSeal Phase 0 avant la mémoire : impossible

**Document original :** `SPEC-EXO-SECURITY-ACTIVATION.md` §5  
**Problème :** La séquence proposée était :
```
Phase 0: ExoSeal verify_boot_chain()  ← IMPOSSIBLE : besoin de heap + APIC
Phase 1: ExoCage
Phase 2: ExoNMI (watchdog NMI)        ← IMPOSSIBLE : LAPIC non initialisé
Phase 3: memory_init()
```

Deux impossibilités :
- `blake3_hash_kernel_image()` nécessite un buffer de travail → heap non disponible
- Le watchdog NMI 200ms nécessite le LAPIC → non initialisé avant `arch_init()`

**Séquence correcte :**
```
Phase 0: memory_init()           ← buddy, physmap (CRIT-01 corrigé)
Phase 1: arch_init() + APIC      ← registres hardware
Phase 2: ExoCage (CR4, MSR)      ← pas de heap nécessaire
Phase 3: ExoNMI watchdog         ← LAPIC disponible
Phase 4: scheduler_init()
Phase 5: security_init() complet ← CapToken, ZeroTrust, ExoLedger
Phase 6: ExoSeal verify_chain()  ← heap disponible, physmap OK
Phase 7: ExoShield IOMMU         ← avant drivers Ring1
```

**Correction :** `CORR-82.md`

---

### ERR-03 🟠 HAUTE — wgpu ne compile pas en no_std

**Document original :** `SPEC-EXO-GRAPHICS.md` §6.2  
**Problème :** `wgpu` utilise `std::thread`, `std::sync::Arc`, `std::time::Instant`. Pas de mode `no_std`. De plus, `Backends::GL` suppose un driver OpenGL inexistant dans ExoOS v0.2.0.

**Correction :** Retirer wgpu de v0.2.0. Rendu texte direct sur GOP framebuffer via `fontdue` (no_std). wgpu passe en v0.3.0 avec musl-exo complet.

**Correction :** `CORR-83.md`

---

### ERR-04 🟠 HAUTE — `META_FLAG_IMMUTABLE` jamais vérifié dans le chemin d'écriture

**Document original :** `SPEC-EXO-SECURITY-ACTIVATION.md` §3.6  
**Problème :**
```bash
grep -rn "is_immutable" kernel/src/fs/exofs/syscall/
→ 0 résultats
```
Le flag `META_FLAG_IMMUTABLE` existe dans `objects/object_meta.rs` mais n'est **jamais vérifié** dans `blob_write.rs`. ExoLedger peut être modifié par un processus Ring3 avec la capability d'écriture.

**Fix immédiat :**
```rust
// kernel/src/fs/exofs/syscall/blob_write.rs
pub fn vfs_write_at(blob_id: BlobId, offset: u64, data: &[u8], pid: u32) -> ExofsResult<usize> {
    let meta = blob_meta_cache_get(blob_id)?;
    if meta.is_immutable() {
        exoledger_append(pid, LedgerEvent::WriteOnImmutable { blob_id });
        return Err(ExofsError::AccessDenied(AccessDeniedReason::Immutable));
    }
    // ... suite inchangée
}
```

**Correction :** `CORR-84.md`

---

### ERR-05 🟠 HAUTE — Données réseau IPC > MAX_MSG_SIZE (240 octets)

**Document original :** `SPEC-EXO-CRATES.md` §2.2  
**Problème :** `MAX_MSG_SIZE = 240` octets dans le kernel. Un paquet TCP = 1460 octets (MSS Ethernet). L'API proposée `NetRequest::Write { data: data.to_vec() }` ne peut pas fonctionner pour des transferts > 240 octets.

**Protocole correct — deux niveaux :**
```rust
// Données courtes (≤ IPC_INLINE_MAX = 200 octets) → inline
NetRequest::WriteInline { handle, data: [u8; 200] }

// Données longues (> 200 octets) → SHM
// 1. Allouer SHM : SYS_SHM_CREATE → shm_cap
// 2. Copier données → SHM
// 3. IPC : NetRequest::WriteSHM { handle, shm_cap, offset, len }
// 4. network_server lit SHM → envoie via smoltcp
// 5. network_server libère le slot SHM
```

**Correction :** `CORR-85.md`

---

### ERR-06 🟡 MOYENNE — Syntaxe Rust invalide `[u8; _]`

**Document original :** `SPEC-EXOPHOENIX-V0.2.md`  
```rust
_reserved: [u8; /* reste de la page */ _],  // ← NE COMPILE PAS
```

**Fix :** Avec ERR-01 corrigé (SSR multi-pages), ce champ disparaît. Si une page unique est visée, la taille doit être une constante calculée explicitement.

---

### ERR-07 🟡 MOYENNE — ExoKairos : budget sans réinitialisation de fenêtre

**Document original :** `SPEC-EXO-SECURITY-ACTIVATION.md` §3.5  
**Problème :** `budget.used_ns` s'incrémente sans jamais être remis à 0 → kill inévitable sur tout processus long-lived.

**Fix :**
```rust
fn update_kairos_budget(tcb: &mut Tcb, elapsed_ns: u64, now_ns: u64) {
    let budget = &mut tcb.kairos_budget;
    // Reset fenêtre si expirée (fenêtre = 1 seconde = 1_000_000_000 ns)
    if now_ns.saturating_sub(budget.window_start_ns) >= KAIROS_WINDOW_NS {
        budget.used_ns      = 0;
        budget.window_start_ns = now_ns;
    }
    budget.used_ns += elapsed_ns;
    if budget.used_ns > budget.limit_200pct_ns { kairos_kill(tcb); }
    else if budget.used_ns > budget.limit_ns   { kairos_throttle(tcb); }
}
```

---

### ERR-08 🟡 MOYENNE — snmalloc-rs requiert std

**Document original :** `SPEC-EXO-CRATES.md` §1.2  
**Problème :** `snmalloc-rs` dépend de `std` (threads OS internes).

**Table corrigée :**
| Backend | Condition | Notes |
|---------|-----------|-------|
| `dlmalloc` | **Principal v0.2.0** | no_std pur, aucune dépendance |
| `snmalloc-rs` | v0.3.0+ (musl-exo pthreads) | Haute perf multithread |
| `jemallocator` | Jamais Ring1 | Maintenu |

---

### ERR-09 🟡 MOYENNE — Zero Trust sur CHAQUE IPC = -17% perf

**Document original :** `SPEC-EXO-SECURITY-ACTIVATION.md` §3.3  
**Problème :** `check_ipc()` complet sur 50M msgs/s = 500M cycles/s ≈ 17% d'un core.

**Solution : deux niveaux de vérification :**
```
Fast path (Ring1↔Ring1 connu) :
    → bitmask précompilé au démarrage serveur (O(1), 1-2 cycles)
    → PAS de lookup table, PAS d'ExoLedger
    
Slow path (Ring3→Ring1, Ring3→Ring3) :
    → check_ipc() complet
    → ExoLedger si refus
```

---

### ERR-10 🔵 INFO — Limite 64 processus SSR non documentée

La politique de priorisation lors d'une bascule avec > 64 processus doit être explicite :
```
Priorité 1 : Ring1 servers (toujours restaurés)
Priorité 2 : Ring3 avec cap PERSISTENT
Priorité 3 : Ring3 standard (FIFO par PID croissant)
Au-delà de 64 : abandon documenté + log ExoLedger
```

---

### ERR-11 🔵 INFO — ExoPhoenix < 500ms : démarrage Ring1 doit être parallèle

La spec doit préciser que les serveurs Ring1 sont démarrés **en parallèle** après la bascule, pas séquentiellement. Sinon 5 serveurs × ~80ms = 400ms laissant 100ms pour le reste.

---

## BLOC 3 — Corrections claude-gamma (v0.1.0 → v0.2.0)

### C-GAMMA-01 — ExoFS est RAM-only en v0.1.0 : déblocage requis avant musl-exo

**Découverte critique :** L'adresse VirtIO hardcodée `0x10000000` = borne haute de la RAM avec `-m 256M`. Le BAR PCI réel de `virtio-blk-pci` est autour de `0xC000_0000`. ExoFS ne persiste **rien** sur disque en v0.1.0.

**Conséquence pour le plan v0.2.0 :** L'ordre d'implémentation doit être modifié :

```
AVANT (corpus claude-alpha) :
  Phase 0 → exo-alloc → musl-exo → crypto → net → fs → exo-pkg

APRÈS (ordre correct) :
  Phase 0.0 → Corriger VirtIO BAR (lire PCI config space)
  Phase 0.1 → Valider ExoFS sur disque (monter exofs-root.img, lire depuis disque)
  Phase 0.2 → Test : exosh:/$ cat /sbin/exo-ipc-router depuis disque → OK
  Phase 0.3 → SEULEMENT ALORS : exo-alloc, musl-exo, etc.
```

Sans cette correction, `exo compat install calendar` installe dans la RAM — tout est perdu au reboot.

**Correction :** `CORR-86.md`

---

### C-GAMMA-02 — ExoPhoenix : 0 tests unitaires dédiés

Les 2 975 tests sont des tests ExoFS/workspace entier. ExoPhoenix n'a aucun test propre. La checklist `MASTER-CHECKLIST-V0.2.md` P-01 à P-11 suppose des tests qui n'existent pas encore.

**Action :** Créer `kernel/src/exophoenix/tests/` avec au minimum :
```rust
#[test] fn test_ssr_bitmask_256_cores()
#[test] fn test_handoff_roundtrip()
#[test] fn test_cap_survival_empty()
#[test] fn test_phoenix_safe_trait_stateless()
```

---

### C-GAMMA-03 — README SSR Layout typo

`0x1000000..0x110000` → corriger en `0x1000000..0x1100000` (zone 16–17 MiB).

---

### C-GAMMA-04 — POSIX 95% est une cible, pas un état

Ajouter dans toutes les specs qui mentionnent "POSIX ~95%" :  
> *"Ce chiffre est la cible architecturale de ExoFS Translation Layer v5. L'état actuel de musl-exo est 0/127 syscalls implémentés."*

---

## BLOC 4 — Tableau de Priorité des Corrections

| ID | Erreur | Gravité | Bloque | CORR |
|----|--------|---------|--------|------|
| CRIT-01 | physmap 1 GiB max | 🔴 CRITIQUE | Tout | CORR-76 |
| CRIT-02 | cgroup::init() omis | 🔴 CRITIQUE | Ring1 boot | CORR-77 |
| HIGH-01 | Injection PID len==128 | 🔴 SÉCURITÉ | Sécurité | CORR-78 |
| HIGH-02 | Service bloque exosh | 🟠 HAUTE | UX | CORR-79 |
| HIGH-03 | ELF_BASE_MIN=1TiB | 🔴 CRITIQUE | Tout binaire ELF | CORR-80 |
| ERR-01 | SSR overflow 4KiB | 🔴 CRITIQUE | ExoPhoenix | CORR-81 |
| ERR-02 | ExoSeal avant mémoire | 🔴 CRITIQUE | Boot | CORR-82 |
| ERR-03 | wgpu no_std impossible | 🟠 HAUTE | Graphics | CORR-83 |
| ERR-04 | is_immutable() non vérifié | 🟠 HAUTE | ExoLedger | CORR-84 |
| ERR-05 | IPC réseau > 240B | 🟠 HAUTE | exo-net | CORR-85 |
| C-GAMMA-01 | ExoFS RAM-only | 🔴 CRITIQUE | Persistance | CORR-86 |
| ERR-07 | Kairos sans reset fenêtre | 🟡 MOYENNE | Scheduler | CORR-82 (inclus) |
| ERR-08 | snmalloc ≠ no_std | 🟡 MOYENNE | exo-alloc | CORR-81 (inclus) |
| ERR-09 | ZeroTrust perf | 🟡 MOYENNE | IPC perf | CORR-82 (inclus) |
| ERR-10 | SSR 64 procs undoc | 🔵 INFO | Doc | CORR-81 (inclus) |
| ERR-11 | Phoenix Ring1 séquentiel | 🔵 INFO | Perf | CORR-81 (inclus) |
| C-GAMMA-02 | 0 tests ExoPhoenix | 🟠 HAUTE | CI | Nouveau |
| C-GAMMA-03 | SSR typo README | 🔵 INFO | Doc | README fix |

---

## BLOC 5 — Ce qui a Été Validé par claude-beta

Les points suivants du corpus claude-alpha sont **techniquement corrects** et ne nécessitent pas de correction :

- ✅ Rejet libsodium (FFI C no_std + entropie non câblable)
- ✅ Rejet rtnetlink (Netlink inexistant dans ExoOS)
- ✅ Rejet zbus/D-Bus (daemon incompatible avec IPC SpscRing)
- ✅ Rejet jemalloc en Ring1 (arènes globales incompatibles fork)
- ✅ MSR CET : `MSR_IA32_U_CET` (Ring3) + `MSR_IA32_S_CET` (Ring0) — correct SDM Intel
- ✅ Règle DRV-ISR-01 : ISR = acquitter + flag + EOI uniquement
- ✅ `align_up()` exo-alloc : `(size + align - 1) & !(align - 1)` — correct
- ✅ hickory-dns (renommage trust-dns pris en compte)
- ✅ smoltcp Ring1 + exo-net Ring3 — bon découpage
- ✅ PhoenixSafe trait `on_pre_switch` / `on_post_switch`
- ✅ Priorisation natif ExoOS avant compat POSIX
- ✅ Inversion de priorité roadmaps ChatGPT justifiée

---

*claude-alpha — ExoOS v0.2.0 — MASTER-CORRECTIONS-V0.2.md*
