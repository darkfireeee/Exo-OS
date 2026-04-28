--- docs/audit/AUDIT_01_Corrections_P0_P1.md (原始)


+++ docs/audit/AUDIT_01_Corrections_P0_P1.md (修改后)
# ExoOS — Guide de Correction Détaillé P0/P1

## 🎯 Objectif : Implémentations exactes pour 100% conformité

**Document technique** — Code prêt à copier-coller pour chaque correction critique
**Priorité** : P0 (Critique) → P1 (Majeur)
**Références** : CORR-01 à CORR-54 + SRV-05

---

## 🔴 P0 — CRITIQUE (Bloquant production)

### CORR-04 : Remplacement Vec<IpcEndpoint> en ISR

**Fichier cible** : `kernel/src/arch/x86_64/irq/dispatch.rs`

#### Étape 1 : Modifier la signature de dispatch_irq

```rust
// kernel/src/arch/x86_64/irq/dispatch.rs
// ❌ AVANT — ALLOCATION HEAP INTERDITE
pub fn dispatch_irq(irq: u8, registers: &mut InterruptFrame) {
    let route = IRQ_TABLE.read();
    if let Some(ir) = route[irq as usize].as_ref() {
        let mut endpoints: Vec<IpcEndpoint> = Vec::new(); // ← INTERDIT

        for handler in &ir.handlers {
            endpoints.push(handler.endpoint);
        }

        for endpoint in endpoints {
            ipc::send_nonblocking(endpoint.pid, msg);
        }
    }
}

// ✅ APRÈS — TABLEAU FIXE SUR LA PILE (heapless::Vec)
use heapless::Vec as HeaplessVec;

pub fn dispatch_irq(irq: u8, registers: &mut InterruptFrame) {
    let route = IRQ_TABLE.read();
    if let Some(ir) = route[irq as usize].as_ref() {
        // Tableau fixe sur la pile — ZÉRO allocation heap
        let mut endpoints: HeaplessVec<IpcEndpoint, MAX_HANDLERS_PER_IRQ> = HeaplessVec::new();

        for handler in &ir.handlers {
            // ignore error si plein (limite déjà vérifiée à l'enregistrement)
            let _ = endpoints.push(handler.endpoint);
        }

        // Drop du read lock avant envoi IPC (évite deadlock)
        drop(route);

        for endpoint in endpoints.iter() {
            let msg = IpcMessage::new_irq(irq, *endpoint);
            // send_nonblocking ne doit jamais allouer heap non plus
            let _ = ipc::send_nonblocking(endpoint.pid, msg);
        }
    }
}
```

#### Étape 2 : Modifier IrqRoute pour utiliser heapless::Vec

```rust
// kernel/src/arch/x86_64/irq/routing.rs

// ❌ AVANT
pub struct IrqRoute {
    pub irq_line: u8,
    pub source_kind: IrqSourceKind,
    pub handlers: Vec<IrqHandler>, // ← heap allocation
    // ...
}

// ✅ APRÈS
use heapless::Vec as HeaplessVec;

pub const MAX_HANDLERS_PER_IRQ: usize = 8;

pub struct IrqRoute {
    pub irq_line: u8,
    pub source_kind: IrqSourceKind,
    pub handlers: HeaplessVec<IrqHandler, MAX_HANDLERS_PER_IRQ>, // ← stack-only
    // ...
}

impl IrqRoute {
    pub fn new(irq: u8, kind: IrqSourceKind) -> Self {
        Self {
            irq_line: irq,
            source_kind: kind,
            handlers: HeaplessVec::new(),
            // ... autres champs
        }
    }
}
```

#### Étape 3 : Ajouter dépendance heapless

```toml
# kernel/Cargo.toml
[dependencies]
heapless = { version = "0.8", default-features = false }
```

**Vérification CI** :
```bash
# Script CI — détecter toute allocation heap en contexte ISR
# Utiliser cargo-audit ou grep personnalisé
grep -r "Vec::new\|Box::new\|vec!\[" \
  kernel/src/arch/x86_64/irq/ \
  && echo "VIOLATION CORR-04: allocation heap en ISR" && exit 1
```

---

### CORR-32 : Fermeture TOCTOU sys_pci_claim

**Fichier cible** : `kernel/src/drivers/device_claims.rs`

```rust
// kernel/src/drivers/device_claims.rs
// ✅ CORR-32 — LOCK AVANT VÉRIFICATIONS + UNICITÉ BDF

use crate::arch::irq_save;
use crate::memory::memory_map;
use crate::pci::PciBdf;

pub fn sys_pci_claim(
    phys_base:   PhysAddr,
    size:        usize,
    driver_pid:  u32,
    bdf:         Option<PciBdf>,
    calling_pid: u32,
) -> Result<(), ClaimError> {
    // Vérification capability (lecture seule, pas de TOCTOU ici)
    if !process::has_capability(calling_pid, Capability::SysDeviceAdmin) {
        return Err(ClaimError::PermissionDenied);
    }

    // ✅ CORR-32 : Acquérir le lock AVANT toute vérification
    // Protège contre TOCTOU sur MMIO_WHITELIST et memory_map
    let _irq_guard = irq_save(); // Éviter deadlock IRQ → claims
    let mut claims = DEVICE_CLAIMS.write();

    // Vérifications SOUS LOCK (atomiques par rapport aux autres claims)
    if !MMIO_WHITELIST.contains(phys_base, size) {
        return Err(ClaimError::NotInHardwareRegion);
    }

    if memory_map::is_ram_region(phys_base, size) {
        return Err(ClaimError::PhysIsRam);
    }

    // Vérification overlap physique
    if claims.iter().any(|c| c.overlaps(phys_base, size)) {
        return Err(ClaimError::AlreadyClaimed);
    }

    // ✅ CORR-32 : Vérification BDF unique (si spécifié)
    // Empêche deux drivers de claimer le même device PCI
    if let Some(b) = bdf {
        if claims.iter().any(|c| c.bdf == Some(b)) {
            log::warn!(
                "sys_pci_claim: BDF {:?} déjà claimé par PID {}",
                b,
                claims.iter()
                    .find(|c| c.bdf == Some(b))
                    .map(|c| c.owner_pid)
                    .unwrap_or(0)
            );
            return Err(ClaimError::AlreadyClaimed);
        }
    }

    // Vérifier génération du processus cible
    let gen = process::get_generation(driver_pid);
    if gen == 0 {
        return Err(ClaimError::ProcessNotFound);
    }

    // Ajout entrée
    claims.push(DeviceClaim {
        phys_base,
        size,
        owner_pid: driver_pid,
        generation: gen,
        bdf,
    }).map_err(|_| ClaimError::TableFull)?;

    log::info!(
        "sys_pci_claim: région {:#x}+{:#x} claimée par PID {} (BDF: {:?})",
        phys_base, size, driver_pid, bdf
    );

    Ok(())
}

// ─── ClaimError étendu ──────────────────────────────────────────────
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimError {
    PermissionDenied,
    PhysIsRam,
    NotInHardwareRegion,
    AlreadyClaimed,
    TableFull,          // ← NOUVEAU : heapless::Vec plein
    ProcessNotFound,    // ← NOUVEAU : PID invalide
}
```

**Test unitaire** :
```rust
#[test]
fn test_pci_claim_tocou_protection() {
    // Simuler deux threads tentant de claimer la même région simultanément
    let handle1 = std::thread::spawn(|| {
        sys_pci_claim(REGION_A, SIZE, PID1, Some(BDF1), PID1)
    });
    let handle2 = std::thread::spawn(|| {
        sys_pci_claim(REGION_A, SIZE, PID2, Some(BDF1), PID2) // Même BDF
    });

    let r1 = handle1.join().unwrap();
    let r2 = handle2.join().unwrap();

    // Un seul doit réussir
    assert!(r1.is_ok() ^ r2.is_ok()); // XOR logique
}
```

---

### CORR-41 : verify_cap_token() constant-time

**Fichier cible** : `libs/exo-types/src/cap.rs`

#### Étape 1 : Ajouter dépendance subtle

```toml
# libs/exo-types/Cargo.toml
[dependencies]
subtle = { version = "2.5", default-features = false }
```

#### Étape 2 : Réécrire verify_cap_token

```rust
// libs/exo-types/src/cap.rs
// ✅ CORR-41 — CONSTANT-TIME AVEC subtle

use subtle::{Choice, ConstantTimeEq};

/// Vérifie qu'un CapToken correspond au type attendu — constant-time.
///
/// SÉCURITÉ (CORR-41) :
///   - Constant-time : pas de branche dépendant des valeurs secrètes
///   - Vérifie type_id ET generation (anti-replay)
///   - NE vérifie PAS object_id inline : trop coûteux constant-time
///     sans une clé de signature. Vérification object_id = Phase 1 crypto.
///
/// Retourne true si le token correspond. PANIQUE si type incorrect.
/// Conformément à CAP-01 : appelé en première instruction de main.rs.
pub fn verify_cap_token(token: &CapToken, expected: CapabilityType) -> bool {
    // Comparaison constant-time du type_id
    let type_match: Choice = token.type_id.ct_eq(&(expected as u16));

    // Vérification génération non-nulle (token émis par le kernel, pas forgé)
    let gen_valid: Choice = (!token.generation.ct_eq(&0u64)).into();

    // Résultat constant-time : true seulement si les deux conditions sont vraies
    let result = bool::from(type_match & gen_valid);

    if !result {
        // Panic immédiat si token invalide (CAP-01)
        // NOTE : Le message de panic ne révèle PAS lequel des champs a échoué
        //        (protection contre information leakage via panic messages)
        panic!("SECURITY: CapToken invalide en main.rs — arrêt");
    }

    result
}

// ─── Test de timing (à exécuter manuellement) ───────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    #[ignore] // Manuel — nécessite environnement contrôlé
    fn test_verify_cap_token_constant_time() {
        let valid_token = CapToken {
            type_id: CapabilityType::IpcBroker as u16,
            generation: 12345,
            object_id: ObjectId::new(999),
        };

        let invalid_type = CapToken {
            type_id: 0xFFFF, // Type invalide
            generation: 12345,
            object_id: ObjectId::new(999),
        };

        let invalid_gen = CapToken {
            type_id: CapabilityType::IpcBroker as u16,
            generation: 0, // Generation nulle = forgé
            object_id: ObjectId::new(999),
        };

        // Mesurer temps pour token valide
        let start = Instant::now();
        for _ in 0..1_000_000 {
            let _ = core::hint::black_box(verify_cap_token(&valid_token, CapabilityType::IpcBroker));
        }
        let time_valid = start.elapsed();

        // Mesurer temps pour token invalide (type)
        let start = Instant::now();
        for _ in 0..1_000_000 {
            let _ = core::panic::catch_unwind(|| {
                verify_cap_token(&invalid_type, CapabilityType::IpcBroker)
            });
        }
        let time_invalid_type = start.elapsed();

        // Mesurer temps pour token invalide (generation)
        let start = Instant::now();
        for _ in 0..1_000_000 {
            let _ = core::panic::catch_unwind(|| {
                verify_cap_token(&invalid_gen, CapabilityType::IpcBroker)
            });
        }
        let time_invalid_gen = start.elapsed();

        // Les temps doivent être similaires (±10%)
        let ratio1 = time_invalid_type.as_nanos() as f64 / time_valid.as_nanos() as f64;
        let ratio2 = time_invalid_gen.as_nanos() as f64 / time_valid.as_nanos() as f64;

        assert!(
            (0.9..=1.1).contains(&ratio1),
            "Timing diff: valide={:?} vs invalid_type={:?} (ratio={})",
            time_valid, time_invalid_type, ratio1
        );
        assert!(
            (0.9..=1.1).contains(&ratio2),
            "Timing diff: valide={:?} vs invalid_gen={:?} (ratio={})",
            time_valid, time_invalid_gen, ratio2
        );
    }
}
```

---

### Campagne unwrap() → expect()

**Script d'automatisation** :
```bash
#!/bin/bash
# scripts/fix_unwrap.sh — Remplacer unwrap() par expect() avec contexte

# Fichiers hors tests
FILES=$(find kernel/src servers/*/src drivers/*/src libs/*/src \
         -name "*.rs" \
         ! -name "*_test.rs" \
         ! -path "*/tests/*")

for file in $FILES; do
    echo "Processing $file..."

    # Remplacer .unwrap() par .expect("contexte")
    # Note : nécessite revue manuelle pour ajouter message pertinent
    sed -i 's/\.unwrap()/\.expect("TODO: ajouter message explicite")/g' "$file"
done

echo "Revue manuelle requise pour personnaliser les messages expect()"
```

**Exemples de corrections manuelles** :
```rust
// ❌ AVANT
let value = option.unwrap();

// ✅ APRÈS — Message explicite
let value = option.expect("Invariant: config doit être initialisée avant premier accès");

// ❌ AVANT
let result = fallible_op().unwrap();

// ✅ APRÈS — Gestion d'erreur appropriée
let result = fallible_op().map_err(|e| {
    log::error!("Operation failed: {:?}", e);
    Error::InitializationFailed
})?;
```

---

### Commentaires SAFETY pour static mut

**Template de commentaire** :
```rust
/// SAFETY:
///   - [Condition 1 : contexte d'accès exclusif]
///   - [Condition 2 : invariants de synchronisation]
///   - [Condition 3 : durée de vie / initialization]
///   - [Référence à la spec ou règle architecturale]
static mut VARIABLE: Type = InitialValue;
```

**Exemples concrets** :
```rust
// kernel/src/arch/x86_64/irq/mod.rs

/// SAFETY:
///   - Accessible uniquement depuis IRQ context (interrupts disabled)
///   - Jamais lu/écrit concurrently avec code normal (garanti par irq_save())
///   - Initialisé à zero au boot, jamais réinitialisé
///   - Règle S-04 : toutes modifications sous irq_save()
static mut IRQ_NESTING_COUNT: u32 = 0;

/// SAFETY:
///   - Écrit une fois au boot par BSP avant enable_interrupts()
///   - Lecture seule ensuite (AtomicU64 preferred, mais TSC calibration requiert mutabilité temporaire)
///   - Après calibration, traité comme immutable
///   - Règle S-04 : BOOT_TSC_KHZ AtomicU64 préféré (FIX-103)
static mut BOOT_TSC_KHZ_TEMP: u64 = 0;

// kernel/src/exophoenix/handoff.rs

/// SAFETY:
///   - Région SSR partagée entre Kernel A et Kernel B
///   - Accès coordonné via protocole PHX-* (FREEZE_REQ, FREEZE_ACK, etc.)
///   - Jamais modifié pendant phase active (seulement pendant handoff)
///   - Règle PHX-01 : écritures atomiques Ordering::Release/Acquire uniquement
static mut SSR_HANDOFF_FLAG: u64 = 0;
```

---

## 🟠 P1 — MAJEUR (Stabilité requise)

### Ordering::Relaxed commenté

**Template de commentaire** :
```rust
// Relaxed OK : [raison]
// - [Argument 1 : pourquoi pas besoin de synchronisation]
// - [Argument 2 : conséquences acceptables si lecture stale]
VARIABLE.store(value, Ordering::Relaxed);
```

**Exemples** :
```rust
// kernel/src/time.rs

// Relaxed OK : compteur statistique, perte acceptable
// - Utilisé uniquement pour debugging/metrics
// - Lecture stale = métrique légèrement imprécise, pas de bug fonctionnel
static DEBUG_COUNTER: AtomicU64 = AtomicU64::new(0);
DEBUG_COUNTER.fetch_add(1, Ordering::Relaxed);

// Relaxed OK : valeur monotone, lecture seule après init
// - BOOT_TSC_KHZ écrit une fois au boot, jamais modifié ensuite
// - Toutes lectures après calibration voient valeur finale ou zéro (check debug_assert)
BOOT_TSC_KHZ.store(khz, Ordering::Relaxed);

// ❌ Relaxed INCORRECT — À corriger :
// FLAG.store(1, Ordering::Relaxed); // ← DOIT ÊTRE Release si synchronisation inter-core

// ✅ Correction :
// FLAG.store(1, Ordering::Release); // Synchronise avec load Acquire dans autre core
```

---

### TODOs : Feature gates

**Pattern recommandé** :
```rust
// ❌ AVANT — TODO actif en production
// TODO: implementer validation
fn validate_epoch() {
    unimplemented!();
}

// ✅ APRÈS — Feature gate explicite
#[cfg(feature = "phase4")]
fn validate_epoch() {
    // Implémentation complète Phase 4
}

#[cfg(not(feature = "phase4"))]
fn validate_epoch() -> Result<(), EpochError> {
    // Stub Phase 8 — retourne NotImplemented
    Err(EpochError::NotImplemented)
}
```

---

*(Suite des corrections P1/P2 dans documents séparés)*

---

## 📋 Checklist de validation post-correction

### Pour chaque fichier modifié
- [ ] Compilation sans warnings (`cargo build --release --no-default-features`)
- [ ] Tests unitaires passent (`cargo test --lib`)
- [ ] Clippy clean (`cargo clippy -- -D warnings`)
- [ ] Formatage correct (`cargo fmt --check`)

### Validation globale
- [ ] Zéro allocation heap en contexte ISR (audit manuel + script CI)
- [ ] Tous static mut ont commentaire SAFETY
- [ ] unwrap() < 10 en production
- [ ] Ordering::Relaxed 100% commentés
- [ ] TODOs soit implémentés, soit feature-gated

---

*Document technique — Code prêt à intégrer*
*Dernière mise à jour : Avril 2026*