# Scheduler Energy — C-States, P-States, Profils d'alimentation

> **Sources** : `kernel/src/scheduler/energy/`  
> **Règles** : CSTATE-01

---

## Table des matières

1. [c_states.rs — États d'inactivité du processeur](#1-c_statesrs--états-dinactivité-du-processeur)
2. [frequency.rs — P-States et MSR](#2-frequencyrs--p-states-et-msr)
3. [power_profile.rs — Profils d'alimentation](#3-power_profilers--profils-dalimentation)

---

## 1. c_states.rs — États d'inactivité du processeur

### CState enum

```rust
pub enum CState {
    C0,  // Actif (CPU exécute des instructions)
    C1,  // Halt (HLT — arrêt de l'horloge, réveil rapide)
    C2,  // Stop-Clock (latence ~80 ns sur x86 moderne)
    C3,  // Sleep (caches vidés, latence ~200 ns)
}
```

### Latences de sortie

```rust
impl CState {
    pub fn exit_latency_ns(self) -> u64 {
        match self {
            C0 => 0,
            C1 => 1_000,    // 1 µs
            C2 => 80_000,   // 80 µs
            C3 => 200_000,  // 200 µs
        }
    }
}
```

### Initialisation

```rust
pub unsafe fn init(nr_cpus: usize)
```
- Initialise `MAX_CSTATE[0..nr_cpus]` à `CState::C3` (tous états autorisés par défaut).
- Ces entrées sont des `AtomicU8` dans un tableau statique.

### Contrainte RT (CSTATE-01)

La règle CSTATE-01 exige que `max_allowed_cstate()` utilise **`fetch_min` atomique** pour garantir la cohérence sans lock.

```rust
// Contraint le CPU à C1 max (threads RT actifs)
pub fn constrain_rt(cpu: usize) {
    MAX_CSTATE[cpu].fetch_min(CState::C1 as u8, SeqCst);
}

// Relâche la contrainte RT (retour à C3 max)
pub fn release_rt_constraint(cpu: usize) {
    MAX_CSTATE[cpu].store(CState::C3 as u8, Release);
}

// Lit le C-state maximum autorisé (fetch_min garantit cohérence CSTATE-01)
pub fn max_allowed_cstate(cpu: usize) -> CState {
    match MAX_CSTATE[cpu].load(Acquire) {
        0 => CState::C0,
        1 => CState::C1,
        2 => CState::C2,
        _ => CState::C3,
    }
}
```

**Pourquoi `fetch_min`** : si deux threads RT se terminent simultanément et appellent `constrain_rt()`, `fetch_min` garantit que la contrainte la plus restrictive (C1) est toujours appliquée, sans race.

### Sélection du C-state optimal

```rust
pub fn select_cstate(cpu: usize, idle_ns: u64) -> CState
```

Algorithme :
```
max = max_allowed_cstate(cpu)

Si idle_ns < C2.exit_latency_ns()  → CState::C1 (trop court pour C2)
Si idle_ns < C3.exit_latency_ns()  → min(CState::C2, max)
Sinon                              → max
```

Heuristique : si le CPU va être idle moins de 80 µs, HLT (C1) est plus efficace que d'entrer en C2 (dont la latence de sortie annulerait le gain).

### Entrée en C-state

```rust
pub unsafe fn enter_cstate(cs: CState)
```

```
C0 → rien (ne devrait pas être appelé)
C1 → asm!("hlt")                          # Arrêt horloge, réveil par IRQ
C2 → asm!("mwait" /* hint=C2 */)          # MWAIT avec hint niveau C2
C3 → asm!("mwait" /* hint=C3, flush */)   # MWAIT + flush cache
```

L'IPI de reschedule (`sched_ipi_reschedule`) ou toute IRQ périphérique réveille le CPU.

---

## 2. frequency.rs — P-States et MSR

### FFI vers arch/

```rust
extern "C" {
    // Écrit le P-state dans IA32_PERF_CTL (MSR 0x199)
    fn arch_set_cpu_pstate(cpu: u32, pstate: u32);
}
```

### Table de fréquences

```rust
// Initialisée par init_server ou acpi_server au boot
pub unsafe fn set_pstate_table(freqs_mhz: &[u32])

// Retourne la fréquence (MHz) pour un P-state donné
pub fn pstate_freq_mhz(p: usize) -> u32
```

Exemple typique (CPU 4 cœurs) :
```
P0 = 3600 MHz  (Turbo boost)
P1 = 3200 MHz  (Max nominal)
P2 = 2400 MHz  (Balanced)
P3 = 1600 MHz  (Power save)
P4 =  800 MHz  (Idle)
```

### État courant

```rust
static CURRENT_PSTATE: [AtomicU32; MAX_CPUS]  // P-state actuel par CPU

pub fn current_pstate(cpu: usize) -> u32
pub fn current_freq_mhz(cpu: usize) -> u32
```

### Changement de fréquence

```rust
pub unsafe fn set_pstate(cpu: usize, p: u32)
```

1. Valide `p < pstate_table.len()`.
2. Si `p == CURRENT_PSTATE[cpu]` → return (pas de changement inutile).
3. `arch_set_cpu_pstate(cpu as u32, p)` → écriture dans IA32_PERF_CTL.
4. `CURRENT_PSTATE[cpu].store(p)`.
5. `PSTATE_CHANGES++`.

### Scaling EDF

```rust
pub fn scale_budget_ns(budget_ns: u64, cpu: usize) -> u64
```

Pour SCHED_DEADLINE : ajuste le budget en fonction du P-state actuel.
```
budget_scaled = budget_ns × MAX_FREQ / current_freq
```
Si le CPU tourne à 50 % de sa fréquence max, le budget compte double en temps réel.

### Compteur

```rust
pub static PSTATE_CHANGES: AtomicU64
```

---

## 3. power_profile.rs — Profils d'alimentation

### Profils disponibles

```rust
pub enum PowerProfile {
    Performance,   // P-state max en permanence
    Balanced,      // P-state dynamique selon charge
    PowerSave,     // P-state minimal, C3 max
}
```

### Profil actif

```rust
static ACTIVE_PROFILE: AtomicU8 = AtomicU8::new(PowerProfile::Balanced as u8)

pub fn current_profile() -> PowerProfile
pub fn set_profile(p: PowerProfile)
```

### maybe_update_pstate

```rust
pub unsafe fn maybe_update_pstate(
    cpu: usize,
    cpu_util_pct: u32,  // Utilisation CPU 0-100%
)
```

Appelé depuis `scheduler_tick()` toutes les 4 ticks.

**Logique par profil** :

```
Performance :
    set_pstate(cpu, 0)  ← P0 toujours (Turbo/max)

Balanced :
    Si util > 80% → set_pstate(cpu, P0 ou P1)
    Si util > 40% → set_pstate(cpu, P2)
    Sinon         → set_pstate(cpu, P3)

PowerSave :
    Si util > 90% → set_pstate(cpu, P2)  ← jamais P0/Turbo
    Sinon         → set_pstate(cpu, P4)  ← fréquence min
    constrain_rt → C3 privilégié
```

### Interaction avec C-states

| Profil | enter_cstate cible | max_allowed_cstate |
|--------|--------------------|--------------------|
| Performance | C1 (HLT) | C1 |
| Balanced | select_cstate() | C3 |
| PowerSave | C3 (MWAIT) | C3 |

Avec le profil `Performance`, on évite C2/C3 pour minimiser les latences de réveil (serveurs web, temps réel souple).
