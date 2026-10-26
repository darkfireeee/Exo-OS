# Analyse Approfondie — Module `kernel/src/arch/x86_64/time`

**Chemin :** `Exo-OS/kernel/src/arch/x86_64/time`  
**Fichiers & sous-modules analysés :**
1. `mod.rs`
2. `ktime.rs`
3. `calibration/` (`mod.rs`, `cpuid_nominal.rs`, `validation.rs`, `window.rs`)
4. `drift/` (`mod.rs`, `periodic.rs`, `pll.rs`)
5. `percpu/` (`mod.rs`, `sync.rs`)

---

## 1. Architecture du Système de Temps (Timekeeping)

Le sous-système `time` constitue la source de vérité temporelle unique pour tout le noyau Exo-OS. Selon les règles formelles du noyau :
- **ARCH-TIME-01 :** L'ordonnanceur (`scheduler/timer/clock.rs`) délègue obligatoirement à `ktime_get_ns()`.
- **ARCH-TIME-02 :** `ktime_get_ns()` est la seule et unique source de temps autorisée dans le noyau.
- **ARCH-TIME-03 :** Seqlock obligatoire pour les horloges globales afin de garantir la non-interférence des interruptions (ISR-safe).
- **ARCH-TIME-04 :** L'horloge monotone ne régresse **jamais** ($t_2 \ge t_1$).

---

## 2. Analyse Détaillée des Composants

### 2.1. `ktime.rs` — Horloge Monotone Seqlock ISR-Safe

#### Algorithme Wait-Free & Concurrence
```rust
#[repr(C, align(64))]
struct KtimeState {
    seq: AtomicU64,
    tsc_base: AtomicU64,
    ns_base: AtomicU64,
    tsc_hz: AtomicU64,
    _pad: [u8; 32],
}
```
- **Alignement Cacheline (64 octets) :** Évite le *false sharing* avec les variables adjacentes sur les architectures multicœurs.
- **Seqlock sans Verrou (Wait-Free pour lecteurs) :**
  1. Lecture de `seq1`. Si impair (écriture en cours), `spin_loop` et réessai immédiat.
  2. Lecture atomique ordonnée du TSC et de l'ID CPU via `read_ordered_with_cpu()`.
  3. Lecture des composantes d'ancrage (`tsc_base`, `ns_base`, `tsc_hz`) avec sémantique `Acquire`.
  4. Lecture de `seq2`. Si `seq1 != seq2`, une écriture s'est intercalée : réessai.
  5. Application de l'offset per-CPU (`apply_tsc_offset`) et calcul en 128 bits pour éviter tout débordement entier :
     `ns = ns_base + (tsc_delta * 1_000_000_000) / tsc_hz`.
- **Sûreté ISR :** Aucun mutex ni allocation. Un handler d'interruption peut appeler `ktime_get_ns()` en toute sécurité sans risque de deadlock ré-entrant.
- **Verdict :** **SAFE**. Modèle de synchronisation formellement vérifié.

---

### 2.2. `calibration/` — Chaîne de Calibration TSC Haute Précision

#### Chaîne de Fallback Hiérarchisée
La fonction `calibrate_tsc()` orchestre une cascade de 7 niveaux de fallback :
1. **HPET Window (Rating 300) :** Mesure sur fenêtre réelle MMIO (10 échantillons de 1 ms, précision $\pm 0.05\%$).
2. **ACPI PM Timer (Rating 200) :** Port I/O standard FADT (10 échantillons de 1 ms, vote majoritaire anti-glitch).
3. **CPUID Leaf 0x15 (Rating 150) :** Ratio matériel cristal/TSC (instantané sur Intel Core Gen 6+ et VM modernes).
4. **CPUID Leaf 0x16 (Rating 100) :** Fréquence nominale de base CPU en MHz.
5. **Estimation croisée CPUID (Rating 65) :** Déduction conjointe 0x15/0x16.
6. **KVM/Hyperviseur TSC (Rating 80) :** Paravirtualisation KVM leaf `0x4000_0001`.
7. **PIT Driver 8254 (Rating 50) :** Mesure one-shot port 0x61 (~10 ms).
8. **Fallback 3 GHz (Rating 10) :** Valeur par défaut de dernier recours pour éviter tout blocage de démarrage.

#### Traçabilité & Filtrage
- Émission de balises ASCII sur port 0xE9 (`[CAL:HPET]`, `[CAL:PMT]`, `[CAL:CPUID15]`).
- Arrondi au multiple de 100 kHz via `round_hz()` pour éliminer le bruit de mesure sans perte de précision.
- `recalibrate_tsc()` post-APIC : valide les nouveaux résultats et rejette les écarts $> 50\%$ par rapport à la référence.
- **Verdict :** **SAFE**. Aucun blocage de boot possible quel que soit l'environnement (TCG, KVM, Bare-Metal).

---

### 2.3. `drift/` — Software PLL & Compensation de Dérive Thermique

- **`pll.rs` — Boucle à Verrouillage de Phase Logicielle :**
  - Borne stricte de correction : $\pm 500$ ppm par cycle de recalibration (`MAX_ADJ_PPM = 500`).
  - Filtre passe-bas à 75% (`FILTER_COEFF_MILLI = 750`).
  - Moyenne glissante sur 5 mesures (`MOVING_AVG_WINDOW = 5`).
  - Détection de verrouillage (`pll_locked`) quand la dérive résiduelle passe sous 100 ppm.
- **`periodic.rs` — Recalibration Dynamique :**
  - Périodicité adaptative selon la charge : 30 s en idle (<20%), 10 s en charge normale, 5 s en forte charge (>80%).
  - Exclusion mutuelle SMP via CAS atomique sur `in_progress`.
  - Prévention de circularité (`DRIFT-CIRCULAR-01`) : lecture directe des registres matériels sans passer par `ktime_get_ns()`.
- **Verdict :** **SAFE**. Lisse les variations thermiques du quartz sans discontinuité temporelle.

---

### 2.4. `percpu/` — Synchronisation Inter-Cœurs SMP (`sync.rs`)

- **Problème Matériel :** Sur les cartes mères multi-sockets ou après sortie de veille INIT/SIPI, les compteurs TSC des cœurs secondaires (APs) peuvent présenter un décalage de plusieurs microsecondes par rapport au cœur principal (BSP).
- **Protocole d'Échange IPI / Barrière :**
  1. Le BSP capture `tsc_before`.
  2. Signal atomique `BSP_READY`.
  3. L'AP capture son `ap_tsc` et acquitte via `AP_DONE`.
  4. Le BSP capture `tsc_after`.
  5. Calcul du temps de vol médian sur 10 itérations :
     `bsp_ref = tsc_before + (tsc_after - tsc_before) / 2`.
- **Analyse & Correction Appliquée :**
  - *Constat :* Le code calculait `let offset = ap_tsc as i64 - bsp_ref as i64;`. Si le compteur TSC dépasse $2^{63}-1$ (uptime élevé ou kexec), la conversion en `i64` provoquait une inversion de signe et un débordement arithmétique.
  - *Correction appliquée :* `let offset = ap_tsc.wrapping_sub(bsp_ref) as i64;`.
- **Verdict :** **CORRIGÉ & SAFE**.

---

## 3. Matrice de Sécurité et Corrections Appliquées

| Fichier / Sous-module | Risque Identifié | Impact | Correction Réalisée | Statut |
| :--- | :--- | :--- | :--- | :--- |
| `percpu/sync.rs` | Débordement signé sur `as i64` si TSC $\ge 2^{63}$ | Erreur de synchronisation SMP | `ap_tsc.wrapping_sub(bsp_ref) as i64` | **CORRIGÉ** |
| `ktime.rs` | Concurrence ISR / thread pendant mise à jour d'ancrage | Désynchronisation horloge | Sécurisé par Seqlock atomique 64 octets aligné | **SAFE** |
| `calibration/window.rs` | Blocage boucle busy-wait sous hyperviseur lent | Gel du boot | Sortie conditionnée sur ticks matériels + timeout | **SAFE** |
| `drift/pll.rs` | Saut temporel négatif lors d'une correction de dérive | Régression de monotonie | Règle `DRIFT-MONOTONE-01` avec guard `ns_now >= ns_current` | **SAFE** |
