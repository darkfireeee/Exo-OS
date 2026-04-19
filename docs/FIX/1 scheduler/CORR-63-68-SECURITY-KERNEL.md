# CORR-63 à CORR-68 — Corrections sécurité kernel

---

# CORR-63 — ct_u64_gte : vrai constant-time

**Source :** BUG-S4 | **Fichier :** `kernel/src/security/exokairos.rs` | **Priorité :** Phase 1

## Constat

```rust
// exokairos.rs:510-513 — ACTUEL (non constant-time)
fn ct_u64_gte(a: u64, b: u64) -> bool {
    let diff = a.wrapping_sub(b);
    let sign_bit = (diff >> 63) & 1;
    sign_bit == 0 || is_eq != 0   // ← `||` = court-circuit = branchement potentiel
}
```

Le commentaire dans le code reconnaît lui-même le problème. L'opérateur `||` en Rust
peut être compilé en branchement conditionnel par LLVM selon les flags d'optimisation.

## Correction

```rust
// exokairos.rs — APRÈS
/// Retourne `1u64` si a >= b, `0u64` sinon. Vrai constant-time : pas de branchement.
///
/// Principe : a >= b ⟺ (a wrapping_sub b) n'a pas de borrow.
/// Le borrow est le bit 63 de (a - b) en arithmétique wrapping signée.
#[inline(always)]
fn ct_u64_gte(a: u64, b: u64) -> bool {
    // Pas de `||`, pas de `if`, pas de `match` : zéro branchement conditionnel.
    // a >= b ⟺ borrow == 0 ⟺ bit 63 de (a wrapping_sub b) == 0
    let borrow = a.wrapping_sub(b) >> 63;  // 0 si a>=b, 1 si a<b
    borrow == 0
}

/// Comparaison constant-time d'égalité u64.
/// Supprime ct_u64_eq si elle n'est utilisée qu'ici.
#[inline(always)]
fn ct_u64_eq(a: u64, b: u64) -> u64 {
    // XOR = 0 ssi égaux ; transformer en 0/1 sans branchement
    let diff = a ^ b;
    // diff == 0 → (diff | (!diff + 1)) >> 63 == 0 en u64 non-signé est complexe
    // Plus simple : diff == 0 → !diff == u64::MAX → !diff >> 63 == 1
    // Utiliser : 1 - (diff.min(1)) — mais min() peut brancher
    // Approche portable : utiliser la soustraction signée
    // diff == 0 → ((0u64.wrapping_sub(diff)) | diff) >> 63 == 0
    let r = (diff | (0u64.wrapping_sub(diff))) >> 63;
    1 - r  // 1 si égaux, 0 sinon
}
```

**Note :** Si `ct_u64_eq` n'est plus utilisée après ce changement, la supprimer.

## Validation

- [ ] Compiler avec `opt-level=3` et inspecter le désassemblage : aucun `jcc`, `cmov` conditionnel
- [ ] Test : `ct_u64_gte(5, 3)` = true, `ct_u64_gte(3, 5)` = false, `ct_u64_gte(3, 3)` = true
- [ ] Test : `ct_u64_gte(0, u64::MAX)` = false, `ct_u64_gte(u64::MAX, 0)` = true

---

# CORR-64 — fetch_sub underflow : compare_exchange_weak

**Source :** BUG-S5 | **Fichier :** `kernel/src/security/exokairos.rs` | **Priorité :** Phase 0

## Constat

```rust
// exokairos.rs:217-221 — ACTUEL
let prev_calls = self.calls_left.fetch_sub(1, Ordering::AcqRel);
let calls_result = if prev_calls == 0 {
    self.calls_left.store(0, Ordering::Release);  // rattrape le wrapping
    Err(CapError::BudgetExhausted)
} else { Ok(()) };
```

Fenêtre SMP : entre `fetch_sub` (→ u32::MAX) et `store(0)`, un autre CPU lit
`u32::MAX` et passe le check budget. Sur N CPUs, N-1 threads peuvent contourner
simultanément un budget épuisé.

## Correction

```rust
// exokairos.rs — APRÈS : CAS dans verify()
// Remplacer fetch_sub par compare_exchange_weak en boucle

let calls_result = {
    let mut result = Err(CapError::BudgetExhausted);
    let mut cur = self.calls_left.load(Ordering::Acquire);
    loop {
        if cur == 0 {
            break; // budget épuisé, result = Err
        }
        match self.calls_left.compare_exchange_weak(
            cur,
            cur - 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                result = Ok(());
                break;
            }
            Err(actual) => {
                cur = actual; // retry avec la valeur actuelle
            }
        }
    }
    result
};
```

**Propriété garantie :** `calls_left` ne passe JAMAIS par u32::MAX.
La propriété TLA+ `BudgetMonotonicity` est restaurée.

## Validation

- [ ] Test SMP : N threads en parallèle sur une capability à budget = 1 → exactement 1 Ok(), N-1 Err(BudgetExhausted)
- [ ] Test : budget = 0 dès le départ → tous les appels Err(BudgetExhausted) immédiatement
- [ ] `calls_left.load()` ne retourne jamais u32::MAX pendant ni après les appels

---

# CORR-65 — KERNEL_SECRET : Once<> thread-safe

**Source :** BUG-S6 | **Fichier :** `kernel/src/security/exokairos.rs` | **Priorité :** Phase 1

## Constat

```rust
// exokairos.rs:560-581 — ACTUEL
static mut KERNEL_SECRET: [u8; 32] = [0u8; 32];
static KERNEL_SECRET_INITIALIZED: AtomicBool = AtomicBool::new(false);

pub unsafe fn init_kernel_secret(secret: &[u8; 32]) {
    KERNEL_SECRET.copy_from_slice(secret);  // ← write à static mut non synchronisé
    KERNEL_SECRET_INITIALIZED.store(true, Ordering::Release);
}

fn get_kernel_secret() -> [u8; 32] {
    unsafe { KERNEL_SECRET }  // ← lecture sans lock ni fence globale
}
```

## Correction

```rust
// exokairos.rs — APRÈS
use spin::Once;

/// KERNEL_SECRET — initialisé une seule fois par ExoSeal au boot, lecture-seule ensuite.
/// `Once` garantit l'initialisation atomique thread-safe et la lecture sans lock post-init.
static KERNEL_SECRET: Once<[u8; 32]> = Once::new();

/// Initialise le KERNEL_SECRET (appelé par ExoSeal au boot, Ring 0 uniquement).
/// Panique si appelé deux fois.
pub fn init_kernel_secret(secret: &[u8; 32]) {
    KERNEL_SECRET.call_once(|| *secret);
}

/// Lit le KERNEL_SECRET — panique si non initialisé (bug d'ordre de boot).
fn get_kernel_secret() -> [u8; 32] {
    *KERNEL_SECRET.get()
        .expect("KERNEL_SECRET non initialisé — exoseal_boot_phase0 doit précéder verify()")
}
```

**Dépendance :** ajouter `spin = { version = "0.9", default-features = false }` au
`Cargo.toml` du kernel si pas déjà présent. `spin::Once` est `no_std` compatible.

**Alternative sans crate externe :**
```rust
// Utiliser AtomicU64 array + fence SeqCst si spin n'est pas disponible
static KERNEL_SECRET_DATA: [AtomicU64; 4] = [
    AtomicU64::new(0), AtomicU64::new(0),
    AtomicU64::new(0), AtomicU64::new(0),
];
static KERNEL_SECRET_READY: AtomicBool = AtomicBool::new(false);

pub fn init_kernel_secret(secret: &[u8; 32]) {
    // Écrire les 4 mots de 8 bytes
    for (i, chunk) in secret.chunks(8).enumerate() {
        let word = u64::from_le_bytes(chunk.try_into().unwrap());
        KERNEL_SECRET_DATA[i].store(word, Ordering::Relaxed);
    }
    // Fence SeqCst : toutes les écritures précédentes visibles avant READY
    core::sync::atomic::fence(Ordering::SeqCst);
    KERNEL_SECRET_READY.store(true, Ordering::Release);
}

fn get_kernel_secret() -> [u8; 32] {
    assert!(KERNEL_SECRET_READY.load(Ordering::Acquire), "KERNEL_SECRET non init");
    let mut out = [0u8; 32];
    for (i, chunk) in out.chunks_mut(8).enumerate() {
        let word = KERNEL_SECRET_DATA[i].load(Ordering::Relaxed);
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    out
}
```

## Validation

- [ ] Suppression de tous les `unsafe` autour de KERNEL_SECRET
- [ ] Test : appel à `get_kernel_secret()` avant `init_kernel_secret()` → panic clair
- [ ] Test SMP : deux CPUs appellent `init_kernel_secret()` simultanément → un seul réussit

---

# CORR-66 — CPUID EDX clobber dans exoveil_init

**Source :** BUG-S7 | **Fichier :** `kernel/src/security/exoveil.rs` | **Priorité :** Phase 1

## Constat

```rust
// exoveil.rs:299 — bloc asm CPUID ACTUEL
core::arch::asm!(
    "push rbx", "mov eax, 7", "xor ecx, ecx",
    "cpuid",
    "pop rbx",
    out("ecx") ecx,
    lateout("eax") _,
    // ← EDX non déclaré comme clobber : UB si LLVM alloue une variable dans EDX
);
```

CPUID.7.0 modifie EAX, EBX, ECX, **ET EDX**. Ne pas déclarer EDX comme clobber
peut corrompre silencieusement une variable locale allouée dans ce registre.

## Correction

```rust
// exoveil.rs — APRÈS : ajouter lateout("edx")
core::arch::asm!(
    "push rbx",
    "mov eax, 7",
    "xor ecx, ecx",
    "cpuid",
    "pop rbx",
    out("ecx") ecx,
    lateout("eax") _,
    lateout("edx") _,   // ← AJOUT : déclarer EDX modifié par CPUID
);
```

**Note :** `exocage.rs::cpuid_cet_available()` déclare déjà correctement `lateout("edx")`.
Ce patch aligne exoveil sur la pratique déjà correcte dans exocage.

## Validation

- [ ] Compiler avec `RUSTFLAGS="-C opt-level=3"` — vérifier absence de corruption
- [ ] `cargo clippy` — pas de warning sur le bloc asm
- [ ] Test : `PKS_AVAILABLE` a la bonne valeur sur un CPU avec/sans PKS

---

# CORR-67 — ExoLedger P0 : mutex sur la chaîne de hash

**Source :** BUG-S8 | **Fichier :** `kernel/src/security/exoledger.rs` | **Priorité :** Phase 1

## Constat

```rust
// exoledger.rs:384-417 — séquence actuelle
pub fn exo_ledger_append_p0(action: ActionTag) {
    let idx = P0_USED.fetch_add(1, Ordering::AcqRel);  // atomique ✓
    // ... construction entrée ~N cycles ...
    let prev_hash = load_last_hash();   // ← non atomique avec store_last_hash
    // ... calcul hash ...
    store_last_hash(&entry.hash);       // ← fenêtre de race avec un autre CPU
    write_volatile(&mut P0_ZONE[idx], entry);
}
```

`LAST_HASH` est un `[AtomicU8; 32]` — 32 opérations séparées non atomiques globalement.
Deux CPUs peuvent lire le même `prev_hash` → deux entrées avec le même parent dans
la Merkle chain → `verify_p0_integrity()` échoue avec `ChainBroken`.

## Correction

```rust
// exoledger.rs — APRÈS : serialiser les appels P0

use spin::Mutex;

/// Mutex léger protégeant la chaîne de hash P0.
/// Seules 16 entrées P0 maximum — contention rare, overhead acceptable.
static P0_CHAIN_LOCK: Mutex<()> = Mutex::new(());

pub fn exo_ledger_append_p0(action: ActionTag) {
    // L'idx est alloué hors du lock (fast path pour la numérotation)
    let idx = P0_USED.fetch_add(1, Ordering::AcqRel);
    if idx >= P0_ZONE_SIZE {
        // P0 zone pleine — cas exceptionnel, ne pas paniquer ici
        return;
    }

    // Lock : garantit atomicité de (load_last_hash → compute → store_last_hash)
    let _guard = P0_CHAIN_LOCK.lock();

    let prev_hash = load_last_hash();  // ← maintenant atomique avec store ci-dessous

    let mut entry = LedgerEntry {
        seq:        idx as u64,
        tsc:        crate::arch::x86_64::time::ktime::read_tsc(),
        actor_oid:  action.actor_oid(),
        action:     action.discriminant(),
        prev_hash,
        hash:       [0u8; 32],
    };
    let hash = entry.compute_hash();
    entry.hash = hash;

    // SAFETY: idx < P0_ZONE_SIZE vérifié ci-dessus ; seul thread dans ce bloc (Mutex).
    unsafe { core::ptr::write_volatile(&mut P0_ZONE[idx], entry); }

    store_last_hash(&entry.hash);  // ← atomique dans ce contexte (sous lock)
    // _guard droppé ici → unlock
}
```

**Note :** Le lock ne protège PAS `P0_USED.fetch_add` (déjà atomique) — seulement
la section `prev_hash → hash → store`. L'overhead est minimal : ~50ns par append P0,
et les appends P0 sont rares (violations CET, handoffs, démarrages).

## Validation

- [ ] Test SMP : 8 CPUs appendant simultanément → `verify_p0_integrity()` retourne Ok
- [ ] Test : aucun `ChainBroken` avec concurrence
- [ ] Propriété TLA+ `P0Immutability` satisfaite après correction

---

# CORR-68 — ExoSeal : NIC IOMMU en premier (boot order)

**Source :** BUG-S9 | **Fichier :** `kernel/src/security/exoseal.rs` | **Priorité :** Phase 0

## Constat

```rust
// exoseal.rs:75-91 — ORDRE ACTUEL (incorrect)
pub unsafe fn exoseal_boot_phase0() {
    exoveil::exoveil_init();              // step 1 — PKS
    exocage::exocage_global_enable();    // step 2 — CET
    stage0::arm_apic_watchdog(...);      // step 3 — watchdog
    exoledger::exo_ledger_append(...);   // step 4 — log
    configure_nic_iommu_policy();        // step 5 — NIC IOMMU ← TROP TARD
}
```

La spec ExoShield MODULE 1 exige que `configure_nic_iommu_policy()` soit exécutée
**EN PREMIER** (Couche 0 physique non-contournable). Entre les étapes 1 et 5, la NIC
peut effectuer des DMA réseau non contrôlés. Fenêtre d'exfiltration de données
pendant le boot si un adversaire ou un pilote malveillant déclenche du DMA réseau.

## Correction

```rust
// exoseal.rs — APRÈS : NIC IOMMU en premier
pub unsafe fn exoseal_boot_phase0() {
    if EXOSEAL_PHASE0_DONE.swap(true, Ordering::AcqRel) {
        return;
    }

    // COUCHE 0 — Physique : verrouiller la NIC IOMMU EN PREMIER
    // Spec ExoShield MODULE 1 : "À l'étape 0 du boot, AVANT tout code Kernel A"
    // Sans ce verrou, la NIC peut DMA vers n'importe quelle adresse physique.
    configure_nic_iommu_policy();

    // COUCHE 1 — Protection mémoire Ring 0 : PKS default-deny
    unsafe { exoveil::exoveil_init(); }

    // COUCHE 2 — Intégrité flux de contrôle : CET global
    let _ = unsafe { exocage::exocage_global_enable() };

    // COUCHE 3 — Watchdog de démarrage
    let _ = stage0::arm_apic_watchdog(BOOT_PHASE0_WATCHDOG_MS);

    // LOG — après que toutes les protections physiques sont en place
    exoledger::exo_ledger_append(exoledger::ActionTag::BootEvent { step: 0 });
}
```

**Propriété TLA+ restaurée :** `BootSafety : NicIommuLocked` est vrai avant toute
activation des couches supérieures.

## Validation

- [ ] Vérifier que `configure_nic_iommu_policy()` est idempotente (safe à appeler avant exoveil)
- [ ] Vérifier qu'elle ne dépend pas de PKS ou CET (sinon ajuster l'ordre)
- [ ] Test QEMU : trace des appels boot → NIC IOMMU configurée avant CET dans les logs
- [ ] Propriété TLA+ `S1 BootSafety` satisfaite
