# Analyse Approfondie — Module `kernel/src/arch`

**Chemin :** `Exo-OS/kernel/src/arch`  
**Fichiers analysés :**
1. `constants.rs`
2. `mod.rs`
3. `time.rs`

---

## 1. Vue d'Ensemble Architecturale

Le module `arch` constitue la couche transverse matérielle d'Exo-OS. Selon les règles d'architecture du noyau :
- **Transversalité :** `arch` peut appeler n'importe quelle couche de bas niveau (mémoire, structures de base) et est appelé par les sous-systèmes de haut niveau (`scheduler/`, `process/`, `memory/`).
- **Isolation d'exécution :** `arch` est responsable de l'orchestration des signaux matériels/exceptions, de l'isolation d'espace mémoire (CR3 / KPTI) et de l'initialisation des descripteurs matériels (GDT, IDT, TSS).
- **Règle de sûreté :** Tout bloc `unsafe` doit impérativement comporter une justification explicite `// SAFETY: ...`.

---

## 2. Analyse Ligne par Ligne et Cohérence des Fichiers

### 2.1. `kernel/src/arch/constants.rs`

#### Contenu & Rôles
Ce fichier regroupe les constantes fondamentales validées lors de la phase de boot v0.2.0.

- **Direct-physmap (Lignes 3-7) :**
  ```rust
  pub const PHYSMAP_INITIAL_COVERAGE: usize = 1 << 30; // 1 GiB
  pub const PHYSMAP_INITIAL_COVERAGE_BYTES: u64 = PHYSMAP_INITIAL_COVERAGE as u64;
  ```
  *Analyse :* Définit une couverture physique initiale de 1 GiB pour la table des pages d'amorçage. Ceci garantit que la RAM basse et le noyau ELF sont immédiatement adressables sans re-pagination prématurée.

- **Zone SSR (ExoPhoenix Shared State Region) (Lignes 9-16) :**
  ```rust
  pub const SSR_PHYS_BASE: u64 = 0x0100_0000; // 16 MiB
  pub const SSR_PHYS_SIZE: u64 = 0x1_0000;    // 64 KiB
  pub const SSR_PHYS_END: u64 = SSR_PHYS_BASE + SSR_PHYS_SIZE;
  ```
  *Cohérence inter-modules :* L'adresse 16 MiB (`0x0100_0000`) est située au-dessus de la mémoire conventionnelle (1 MiB) et de la plage basse réservée au firmware BIOS/ACPI/DMA. Elle correspond exactement aux hypothèses de la crate `exo-phoenix-ssr`.

- **Limites CPU / Multicœur (Lignes 18-28) :**
  ```rust
  pub const MAX_CORES_LAYOUT: usize = 256;
  pub const MAX_CORES_RUNTIME: usize = 64;
  pub const SSR_MAX_CORES_LAYOUT: usize = MAX_CORES_LAYOUT;
  pub const CORE_MASK_WORDS: usize = (MAX_CORES_LAYOUT + 63) / 64; // = 4
  ```
  *Cohérence :* Le bitmask des cœurs utilise des mots de 64 bits (`u64`). Pour 256 cœurs, `(256 + 63) / 64 = 4` mots.

- **Limites IPC & Tables Noyau (Lignes 30-40) :**
  ```rust
  pub const MAX_MSG_SIZE: usize = 240;
  pub const IPC_INLINE_MAX: usize = 200;
  pub const MAX_PROCESSES: usize = 32_768;
  pub const MAX_ENDPOINTS: usize = 8_192;
  ```
  *Analyse de sûreté :* `IPC_INLINE_MAX` (200 octets) réserve 40 octets au sein de `MAX_MSG_SIZE` pour les en-têtes et métadonnées du protocole IPC. `MAX_PROCESSES` plafonné à 32 768 évite les dépassements de mémoire dans la table de processus et les descripteurs SSR.

- **Espace Virtuel User / Kernel (Lignes 42-49) :**
  ```rust
  pub const USER_ELF_BASE_MIN: u64 = 0x0040_0000;
  pub const USER_STACK_TOP: u64 = crate::memory::core::layout::USER_STACK_TOP.as_u64();
  pub const KERNEL_BASE: u64 = crate::memory::core::layout::PHYS_MAP_BASE.as_u64();
  ```
  *Liaison inter-modules :* Importation directe depuis `crate::memory::core::layout`. `USER_STACK_TOP` et `KERNEL_BASE` sont statiquement ancrés dans le canonical high-half (`>= 0xFFFF_8000_0000_0000`).

- **Comptabilité Budgétaire ExoKairos (Lignes 51-58) :**
  ```rust
  pub const KAIROS_WINDOW_NS: u64 = 1_000_000_000; // 1 seconde
  pub const KAIROS_THROTTLE_PCT: u64 = 100;
  pub const KAIROS_KILL_PCT: u64 = 200;
  ```

- **Assertions Statiques à la Compilation (Lignes 60-115) :**
  11 assertions `const _: () = assert!(...);` vérifient formellement :
  1. `PHYSMAP_INITIAL_COVERAGE == 0x4000_0000` (1 GiB)
  2. `SSR_PHYS_END - SSR_PHYS_BASE == SSR_PHYS_SIZE`
  3. `CORE_MASK_WORDS * 64 == MAX_CORES_LAYOUT`
  4. `MAX_CORES_RUNTIME <= MAX_CORES_LAYOUT`
  5. `SSR_MAX_CORES_LAYOUT == MAX_CORES_LAYOUT`
  6. `IPC_INLINE_MAX < MAX_MSG_SIZE`
  7. `USER_ELF_BASE_MIN < USER_STACK_TOP`
  8. `KERNEL_BASE >= 0xFFFF_8000_0000_0000`
  9. `MAX_PROCESSES <= 65_536`
  10. `KAIROS_WINDOW_NS > 0`
  11. `KAIROS_THROTTLE_PCT == 100` et `KAIROS_KILL_PCT == 200`

#### Diagnostic de Sûreté & Vulnérabilité
- **Vulnérabilités / Bugs :** Aucun bug détecté dans `constants.rs`. Les assertions statiques bloquent toute régression à la compilation.
- **Verdict :** **SAFE**. Fichier robuste, typé et exempt d'effets de bord.

---

### 2.2. `kernel/src/arch/mod.rs`

#### Contenu & Rôles
- Déclare les sous-modules conditionnels (`x86_64`, `aarch64`) et les modules communs (`constants`, `time`).
- Exporte la structure d'information système `ArchInfo` et son implémentation `Default`.
- Re-exporte `CpuFeatures`, `CPU_FEATURES`, `read_tsc`, `halt_cpu`.

```rust
#[derive(Debug, Clone, Copy)]
pub struct ArchInfo {
    pub cpu_count: u32,
    pub has_apic: bool,
    pub has_x2apic: bool,
    pub has_acpi: bool,
    pub page_size: usize,
}
```

#### Diagnostic Inter-modules
- `ArchInfo` est instancié par `crate::arch::x86_64::arch_info()` et consommé par le noyau d'initialisation (`kernel_init`).
- L'implémentation par défaut (`Default::default()`) fournit un état dégradé sûr : mono-CPU (1), pas d'APIC, pas d'ACPI, page de 4096 octets.
- **Verdict :** **SAFE**. Pas de code `unsafe`.

---

### 2.3. `kernel/src/arch/time.rs`

#### Contenu & Rôles
Fournit la fonction fondamentale d'accès aux ticks processeur de manière agnostique :
```rust
pub fn read_ticks() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::x86_64::cpu::tsc::read_tsc()
    }
    #[cfg(target_arch = "aarch64")]
    {
        let count: u64;
        // SAFETY: CNTVCT_EL0 est lisible depuis EL0 quand CNTKCTL_EL1.EL0VCTEN=1.
        // En kernel (EL1/EL2), l'accès est toujours autorisé.
        unsafe {
            core::arch::asm!("mrs {}, cntvct_el0", out(reg) count, options(nostack, nomem));
        }
        count
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        0u64
    }
}
```

#### Diagnostic de Sûreté & Concurrence
- **x86_64 :** Utilise `read_tsc()` (`rdtsc`). C'est une instruction non-privilégiée sans effet de bord mémoire (`nomem, nostack`).
  - *Note sur le réordonnancement :* `rdtsc` n'étant pas sérialisant par défaut, deux lectures très proches peuvent être réordonnées par le processeur spéculatif. Pour les mesures nécessitant une sérialisation stricte, le noyau fournit `read_tsc_begin()` (avec `lfence`) et `read_tsc_end()` (avec `rdtscp` + `lfence`).
- **aarch64 :** La lecture du registre système `cntvct_el0` est valide en anneau privilégié EL1/EL2. Les options `nostack, nomem` garantissent l'absence de corruption de pile ou de barrière mémoire inopportune.
- **Fallback :** Retourne `0u64` pour les cibles non prises en charge.
- **Verdict :** **SAFE**.

---

## 3. Matrice de Liaison Inter-Modules

| Symbole | Défini dans | Utilisé par | Justification de Sûreté |
| :--- | :--- | :--- | :--- |
| `PHYSMAP_INITIAL_COVERAGE` | `arch::constants` | `arch::x86_64::boot`, `memory` | Garanti à 1 GiB par assertion statique. |
| `SSR_PHYS_BASE` / `SSR_PHYS_SIZE` | `arch::constants` | `exo_phoenix_ssr`, `memory::vmm` | Plage 16 MiB - 16 MiB + 64 KiB sans collision noyau. |
| `USER_STACK_TOP` / `KERNEL_BASE` | `arch::constants` | `arch::x86_64::syscall`, `process` | Pointage statique strict sur le canonical high-half. |
| `read_ticks()` | `arch::time` | `time::monotonic`, `scheduler` | Non-bloquant, O(1), `nomem`, `nostack`. |
| `ArchInfo` | `arch::mod` | `arch::x86_64`, `kernel::init` | Struct POD simple avec `Copy`, sans pointeur brut. |

---

## 4. Synthèse et Prochaines Étapes
Le dossier racine `kernel/src/arch` est entièrement sain, cohérent et sans vulnérabilité.
L'analyse se poursuit maintenant sur le sous-dossier `kernel/src/arch/x86_64/` et ses fichiers directs ainsi que ses sous-modules (`time`, `sources`, etc.).
