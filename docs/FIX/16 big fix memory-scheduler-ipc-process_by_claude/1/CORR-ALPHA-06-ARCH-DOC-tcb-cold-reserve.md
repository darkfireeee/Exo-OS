# CORR-ALPHA-06 — Architecture v7 : Tableau TCB incomplet — sous-champs `_cold_reserve` manquants

> **Auteur :** claude-alpha  
> **Date :** 2026-05-04  
> **Classe :** 🟡 DOC — Documentation incomplète  
> **Fichier :** `docs/recast/ExoOS_Architecture_v7.md §3.2`  
> **Section :** Tableau TCB Layout GI-01  
> **Sévérité :** Mineure-Haute — opacité totale sur 88 octets critiques du TCB pour audit externe

---

## 1. Description du bug

### 1.1 Le tableau TCB dans Architecture v7 §3.2

```markdown
| `_cold_reserve` | [144] | 88 B | Réservé extensions futures (144+88=232) |
| `fpu_state_ptr` | [232] | 8 B  | `*mut XSaveArea` — null si FPU jamais utilisée **HARDCODÉ ExoPhoenix** |
```

Le champ `_cold_reserve` est présenté comme un bloc opaque de 88 octets "réservé extensions futures". 

### 1.2 La réalité du code

Le champ `_cold_reserve: [u8; 88]` (offsets [144..232]) contient en réalité **9 sous-champs actifs** utilisés en production :

| Offset abs. | Champ | Taille | Utilisé par |
|-------------|-------|--------|-------------|
| [144..152] | `shadow_stack_token` | 8 B | ExoShield v1.0 — PKS domain TcbHot |
| [152] | `cet_flags` | 1 B | ExoShield — CET_EN, IBT, TOKEN_VALID |
| [153] | `threat_score_u8` | 1 B | ExoArgos — score menace 0..100 |
| [154..160] | _(réservé)_ | 6 B | — |
| [160..168] | `pt_buffer_phys` | 8 B | Phase 4 — LBR/Intel PT (futur) |
| [168..176] | `creation_tsc` | 8 B | Anti-réutilisation PID / audit |
| [176..184] | `kstack_top` | 8 B | **TSS.RSP0** — sommet stable de pile kernel |
| [184..192] | _(réservé)_ | 8 B | — |
| [192..200] | `pl0_ssp` | 8 B | **CET** — Shadow Stack Pointer Ring 0 (FIX-CET-01) |
| [200..208] | `affinity_hi[0]` | 8 B | Affinité CPU 64..127 (CORR-57) |
| [208..216] | `affinity_hi[1]` | 8 B | Affinité CPU 128..191 |
| [216..224] | `affinity_hi[2]` | 8 B | Affinité CPU 192..255 |
| [224..232] | _(réservé)_ | 8 B | — |

Total utilisé : 72 octets sur 88. Disponible : 16 octets (réservé).

### 1.3 Champs critiques non documentés

**`kstack_top` à [176]** est particulièrement critique : c'est la valeur écrite dans `TSS.RSP0` à chaque context switch (V7-C-03). Si Kernel B (ExoPhoenix) ou un outil d'audit externe cherche `kstack_top` dans le TCB sur la base du tableau Architecture v7, il ne le trouvera pas.

**`pl0_ssp` à [192]** : le Shadow Stack Pointer Ring 0 est sauvegardé/restauré à chaque switch (FIX-CET-01). Son absence du tableau rend le comportement CET opaque.

**`affinity_hi[0..2]` à [200..224]** : ces champs complètent `cpu_affinity` à [48] pour former un masque 256 bits. Sans cette information, on croirait que l'affinité CPU est limitée à 64 CPUs (1 seul u64).

---

## 2. Correctif

### Fichier : `docs/recast/ExoOS_Architecture_v7.md §3.2`

**Remplacer la ligne `_cold_reserve` par le tableau étendu suivant :**

```markdown
| `run_time_acc` | [128] | 8 B | Temps CPU cumulé (ns) |
| `switch_count` | [136] | 8 B | Nombre de context switches |
| **`_cold_reserve`** | **[144]** | **88 B** | **Extensions actives (voir sous-tableau)** |
| ↳ `shadow_stack_token` | [144] | 8 B | ExoShield PKS — token domaine TcbHot |
| ↳ `cet_flags` | [152] | 1 B | ExoShield CET : bit 0=CET_EN, bit 1=IBT, bit 2=TOKEN_VALID |
| ↳ `threat_score_u8` | [153] | 1 B | ExoArgos — score menace 0-100 |
| ↳ _(réservé)_ | [154] | 6 B | — |
| ↳ `pt_buffer_phys` | [160] | 8 B | Intel PT/LBR buffer (Phase 4) |
| ↳ `creation_tsc` | [168] | 8 B | TSC création — anti-réutilisation PID |
| ↳ **`kstack_top`** | **[176]** | **8 B** | **Sommet stable de pile kernel → écrit dans TSS.RSP0 (V7-C-03)** |
| ↳ _(réservé)_ | [184] | 8 B | — |
| ↳ **`pl0_ssp`** | **[192]** | **8 B** | **CET Shadow Stack Pointer Ring 0 — sauvegardé/restauré par context switch (FIX-CET-01)** |
| ↳ **`affinity_hi[0]`** | **[200]** | **8 B** | **Affinité CPU 64..127 — étend cpu_affinity@[48] → masque 256 bits total** |
| ↳ **`affinity_hi[1]`** | **[208]** | **8 B** | **Affinité CPU 128..191** |
| ↳ **`affinity_hi[2]`** | **[216]** | **8 B** | **Affinité CPU 192..255** |
| ↳ _(réservé)_ | [224] | 8 B | — |
| `fpu_state_ptr` | [232] | 8 B | `*mut XSaveArea` — null si FPU jamais utilisée **HARDCODÉ ExoPhoenix** |
| `rq_next` | [240] | 8 B | RunQueue intrusive — null si BLOCKED |
| `rq_prev` | [248] | 8 B | RunQueue intrusive — null si BLOCKED |
```

### Note à ajouter sous le tableau

```markdown
> **Note `_cold_reserve`** : Les sous-champs de `_cold_reserve` sont accédés exclusivement via 
> les helpers `tcb_write_cold_u64` / `tcb_read_cold_u64` dans `security/exocage.rs`. Jamais 
> via accès direct au tableau d'octets. Les offsets sont vérifiés par des assertions compile-time 
> dans `scheduler/core/task.rs`.
>
> **Affinité 256 bits** : `cpu_affinity` à [48] (u64) + `affinity_hi[0..2]` à [200-216] (3×u64) 
> forment ensemble un masque de 256 bits via la méthode `cpu_affinity_mask() → CpuSet`.
> Ne pas lire uniquement `cpu_affinity@[48]` pour tester l'affinité sur un système > 64 CPUs.
```

---

## 3. Impact scope

- **Fichier modifié :** `docs/recast/ExoOS_Architecture_v7.md §3.2` uniquement
- **Nature :** enrichissement de documentation sans changement de code
- **Bénéficiaires directs :** Kernel B (ExoPhoenix) qui inspecte le TCB, outils d'audit, futurs développeurs
- **Cohérence :** `GI-01_Types_TCB_SSR.md` — vérifier si ce doc doit être mis à jour aussi

---

*— claude-alpha*
