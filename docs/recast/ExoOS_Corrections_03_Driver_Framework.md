# ExoOS — Corrections Driver Framework
**Couvre : CORR-04, CORR-08, CORR-16, CORR-19, CORR-23**  
**Sources IAs : Kimi (§4 CORR-02), Z-AI (INCOH-06), MiniMax (spin_count), Claude**

---

## CORR-04 🔴 — `Vec<IpcEndpoint>` dans `dispatch_irq` : allocation heap en ISR

### Problème
Dans `Driver_Framework_v10.md §3.1`, `dispatch_irq` contient :

```rust
let eps: Vec<IpcEndpoint> = route.handlers.iter()
    .map(|h| h.endpoint.clone())
    .collect();
```

**`dispatch_irq` s'exécute en ISR context (Ring 0, interrupt stack).**  
`Vec::collect()` = allocation heap = **INTERDIT en ISR**.

Cette ligne est entourée de commentaires corrects ("Ne jamais bloquer") mais le code lui-même viole la règle.

**Source** : Kimi §4 (CORR-02), confirmé par Claude

### Correction — `kernel/src/arch/x86_64/irq/routing.rs`

```rust
// dispatch_irq ISR — CORR-04 : Remplacement Vec par tableau fixe
//
// RÈGLE ISR : JAMAIS d'allocation heap en ISR context.
// Solution : tableau fixe sur la pile [Option<IpcEndpoint>; MAX_HANDLERS_PER_IRQ].
//
// MAX_HANDLERS_PER_IRQ = 8 : limite pratique.
// Si un IRQ a >8 handlers, les handlers au-delà sont ignorés (log critique).
// En pratique, aucun IRQ en production ne devrait avoir >4 handlers.

const MAX_HANDLERS_PER_IRQ: usize = 8;

pub fn dispatch_irq(irq: u8) {
    let table = IRQ_TABLE.read();
    let Some(route) = &table[irq as usize] else {
        drop(table);
        lapic_send_eoi();
        return;
    };

    // [... vérification blacklist, EOI, masked_since CAS ...]

    // CORR-04 : Tableau fixe sur la pile — ZÉRO allocation heap
    let mut eps:    [Option<IpcEndpoint>; MAX_HANDLERS_PER_IRQ] = [None; MAX_HANDLERS_PER_IRQ];
    let mut eps_n:  usize = 0;
    let mut generations: [u64; MAX_HANDLERS_PER_IRQ] = [0u64; MAX_HANDLERS_PER_IRQ];

    for h in route.handlers.iter() {
        if eps_n >= MAX_HANDLERS_PER_IRQ {
            // Handlers supplémentaires perdus — situation critique à éviter par design
            log::error!(
                "IRQ {} : {} handlers > MAX({}) — handlers [{}..] ignorés",
                irq, route.handlers.len(), MAX_HANDLERS_PER_IRQ, MAX_HANDLERS_PER_IRQ
            );
            break;
        }
        // Clone de IpcEndpoint doit être O(1) sans allocation (Copy ou inline)
        // Vérifier que IpcEndpoint implémente Copy (exigence architecturale)
        eps[eps_n]        = Some(h.endpoint.clone());
        generations[eps_n] = h.generation;
        eps_n += 1;
    }

    let n = eps_n as u32;

    // [... mise à jour pending_acks, dispatch_generation ...]

    // Envoi IPC aux endpoints (n max = MAX_HANDLERS_PER_IRQ)
    let wg = route.dispatch_generation.fetch_add(1, Ordering::AcqRel) + 1;
    for i in 0..eps_n {
        if let Some(ep) = &eps[i] {
            ipc::send_irq_notification(ep, irq, wg);
        }
    }
}
```

**Exigence architecturale ajoutée** :
```rust
// IpcEndpoint DOIT être Copy (pas d'allocation cachée)
#[derive(Clone, Copy, Debug)]
pub struct IpcEndpoint {
    pub pid:      u32,
    pub chan_idx: u32,
}
// Si IpcEndpoint ne peut pas être Copy, utiliser un index dans une table statique.
```

---

## CORR-08 🟠 — `masked_since` CAS : success ordering Release

### Problème
Driver Framework v10 FIX-113 corrige la race sur `masked_since` avec un CAS mais utilise :

```rust
let _ = route.masked_since.compare_exchange(
    0, now, Ordering::Relaxed, Ordering::Relaxed  // ← INSUFFISANT
);
```

**Sur architectures ARM/RISC-V** (mémoire faible), `Relaxed` success ne garantit pas que l'écriture de `now` soit visible par les autres CPUs lisant `masked_since` avec `Relaxed`.

Le watchdog utilise `masked_since.load(Ordering::Relaxed)`. Sur ARM, il pourrait voir 0 longtemps après que le CAS a réussi → watchdog aveugle temporairement.

**Source** : Z-AI INCOH-06 (ordering incorrect, proposait Acquire — incorrect), Claude (correction : Release)

### Justification technique
- CAS **success** ordering = ordering du *store* si CAS réussit
- On veut que `now` soit visible par les autres CPUs → il faut **Release** (publish)
- Les CPUs liseurs font `load(Relaxed)` → sur x86_64 TSO, Relaxed suffit, mais pour portabilité ARM, un Release permet à un lecteur avec Acquire de voir `now` immédiatement
- CAS **failure** ordering = Relaxed suffit (on ne modifie rien si CAS échoue)

> Z-AI propose Acquire pour success — **incorrect** : Acquire signifie "acquérir" les writes d'autres threads, pas "publier" le nôtre. Release publie.

### Correction — tous les endroits où masked_since.compare_exchange est appelé

```rust
// AVANT (v10) — Relaxed success = non portable
let _ = route.masked_since.compare_exchange(
    0, now, Ordering::Relaxed, Ordering::Relaxed
);

// APRÈS (CORR-08) — Release success = portable ARM/RISC-V
let _ = route.masked_since.compare_exchange(
    0, now,
    Ordering::Release,  // ← Publie `now` vers les lecteurs Acquire
    Ordering::Relaxed,  // ← Failure : pas besoin de barrière
);
```

**Occurrences à corriger dans `routing.rs`** (3 endroits) :
1. Branche `IoApicLevel` dans `dispatch_irq` — masquage IOAPIC
2. Branche `IoApicEdge | Msi | MsiX` dans `dispatch_irq` — après EOI
3. Branche overflow dans `dispatch_irq` — FIX-98

**Le watchdog `load(Ordering::Relaxed)` reste valide** sur x86_64. Pour portabilité, le passage à `Acquire` serait mieux mais n'est pas critique pour Phase 8 (cible x86_64).

---

## CORR-19 🟠 — `spin_count` dans CAS loop : reset par tentative, pas global

### Problème
Dans `dispatch_irq` pour les IRQ Edge/MSI, la boucle CAS sur `pending_acks` utilise :

```rust
let mut spin_count = 0u32;
loop {
    if current > MAX_PENDING_ACKS {
        match route.pending_acks.compare_exchange(...) {
            Err(_) => {
                spin_count += 1;
                if spin_count >= SPIN_THRESHOLD { return; }
                // ...
            }
        }
    } else {
        match route.pending_acks.compare_exchange(...) {
            Err(actual) => {
                current = actual;
                // spin_count n'est PAS incrémenté ici ← PROBLÈME
                // ...
            }
        }
    }
}
```

**Problème** : `spin_count` compte uniquement les échecs dans la branche overflow, pas dans la branche normale. En contention extrême sur la branche normale, le CAS peut échouer des centaines de fois sans jamais atteindre SPIN_THRESHOLD.

**Source** : MiniMax simulation pas-à-pas (§ spin_count correction)

### Correction — `routing.rs dispatch_irq`

```rust
// CORR-19 : spin_count incrémenté dans TOUTES les branches CAS Err
// et reset lors d'un changement de branche (overflow → normal).

let mut spin_count = 0u32;
let mut current    = route.pending_acks.load(Ordering::Relaxed);

loop {
    if current > MAX_PENDING_ACKS {
        // Branche overflow
        match route.pending_acks.compare_exchange(
            current, n, Ordering::Release, Ordering::Relaxed
        ) {
            Ok(_) => {
                // [... FIX-83 overflow_count.fetch_add ...]
                break;
            }
            Err(actual) => {
                current = actual;
                spin_count += 1;               // ← INCRÉMENTÉ ICI AUSSI
                if spin_count >= SPIN_THRESHOLD {
                    log::warn!("IRQ {} CAS overflow contention extrême ({} spins)", irq, spin_count);
                    return; // Drop de la vague
                }
                core::hint::spin_loop();
            }
        }
    } else {
        // Branche normale
        match route.pending_acks.compare_exchange(
            current, current + n, Ordering::AcqRel, Ordering::Relaxed
        ) {
            Ok(prev) => {
                if prev == 0 {
                    route.overflow_count.store(0, Ordering::Relaxed);
                }
                break;
            }
            Err(actual) => {
                current = actual;
                spin_count += 1;               // ← INCRÉMENTÉ ICI AUSSI
                if spin_count >= SPIN_THRESHOLD {
                    log::warn!("IRQ {} CAS normal contention extrême ({} spins)", irq, spin_count);
                    return; // Drop de la vague en ISR — jamais yield
                }
                core::hint::spin_loop();
            }
        }
    }
}
```

---

## CORR-16 ⚠️ — `iommu::domain_of_pid()` : spécification manquante

### Problème
`domain_of_pid(pid: u32)` est appelée dans `sys_dma_map` et `do_exit()` mais n'est jamais spécifiée dans aucun document.

**Source** : Claude (analyse de cohérence), ChatGPT5 §1.5

### Spécification — `kernel/src/drivers/iommu/domain_registry.rs`

```rust
// kernel/src/drivers/iommu/domain_registry.rs — NOUVEAU fichier (CORR-16)
//
// Maintient le mapping bidirectionnel PID ↔ DomainID pour les drivers Ring 1.
//
// LIFECYCLE :
//   Création  : device_server appelle assign_domain() lors de sys_pci_claim()
//               AVANT de spawner le driver.
//   Destruction : do_exit() appelle release_domain() en dernier.
//
// CONTRAINTE hardware : max ~256 domaines IOMMU par contrôleur VT-d.
// heapless::FnvIndexMap garantit no_alloc.

use heapless::FnvIndexMap;

const MAX_IOMMU_DOMAINS: usize = 256;

pub struct IommuDomainRegistry {
    /// PID → DomainID (pour les drivers ayant un domaine isolé)
    pid_to_domain: spin::Mutex<FnvIndexMap<u32, u32, MAX_IOMMU_DOMAINS>>,

    /// DomainID → PID (pour notify_iommu_fault_kill dans worker)
    domain_to_pid: spin::Mutex<FnvIndexMap<u32, u32, MAX_IOMMU_DOMAINS>>,

    /// Compteur de DomainID (simple incrémentation, jamais recyclé en Phase 8)
    next_domain_id: core::sync::atomic::AtomicU32,
}

impl IommuDomainRegistry {
    pub const fn new() -> Self {
        IommuDomainRegistry {
            pid_to_domain:  spin::Mutex::new(FnvIndexMap::new()),
            domain_to_pid:  spin::Mutex::new(FnvIndexMap::new()),
            next_domain_id: core::sync::atomic::AtomicU32::new(1), // 0 = domaine kernel
        }
    }

    /// Crée et assigne un nouveau domaine IOMMU isolé au driver PID.
    /// Appelé par sys_pci_claim() dans device_server AVANT spawn du driver.
    /// Retourne DomainId alloué.
    pub fn assign_domain(&self, pid: u32) -> Result<u32, DmaError> {
        let domain_id = self.next_domain_id
            .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if domain_id as usize >= MAX_IOMMU_DOMAINS {
            return Err(DmaError::IovaSpaceExhausted);
        }

        // Créer le domaine hardware VT-d
        iommu::hardware::create_domain(domain_id).map_err(|_| DmaError::IommuError)?;

        // Enregistrer les mappings bidirectionnels
        self.pid_to_domain.lock()
            .insert(pid, domain_id)
            .map_err(|_| DmaError::IommuError)?;
        self.domain_to_pid.lock()
            .insert(domain_id, pid)
            .map_err(|_| DmaError::IommuError)?;

        Ok(domain_id)
    }

    /// Retourne le DomainID du driver identifié par PID.
    /// Utilisé dans sys_dma_map et do_exit().
    pub fn domain_of_pid(&self, pid: u32) -> Result<u32, DmaError> {
        self.pid_to_domain
            .lock()
            .get(&pid)
            .copied()
            .ok_or(DmaError::IommuError)
    }

    /// Retourne le PID du driver identifié par DomainID.
    /// Utilisé dans iommu_fault_worker_tick() pour notify_iommu_fault_kill.
    pub fn pid_of_domain(&self, domain_id: u32) -> Option<u32> {
        self.domain_to_pid.lock().get(&domain_id).copied()
    }

    /// Libère le domaine IOMMU d'un driver. Appelé en dernier dans do_exit().
    pub fn release_domain(&self, pid: u32) {
        let mut p2d = self.pid_to_domain.lock();
        let mut d2p = self.domain_to_pid.lock();

        if let Some(domain_id) = p2d.remove(&pid) {
            d2p.remove(&domain_id);
            // Désactiver le domaine hardware (déjà fait dans do_exit étape 3)
            // iommu::hardware::destroy_domain(domain_id);
        }
    }
}

pub static IOMMU_DOMAIN_REGISTRY: IommuDomainRegistry = IommuDomainRegistry::new();

// Fonctions helper pour compatibilité avec le code existant
pub fn domain_of_pid(pid: u32) -> Result<u32, DmaError> {
    IOMMU_DOMAIN_REGISTRY.domain_of_pid(pid)
}
pub fn pid_of_domain(domain_id: u32) -> Option<u32> {
    IOMMU_DOMAIN_REGISTRY.pid_of_domain(domain_id)
}
```

**Intégration dans do_exit() — ajout de l'étape 8** :
```rust
// kernel/src/process/lifecycle.rs — do_exit() v8
// Étapes 1-7 inchangées (Driver Framework v10 §3.4)

// Étape 8 : Libérer le domaine IOMMU (CORR-16 — NOUVEAU)
// NOTE : appelé APRÈS revoke_all_for_pid (étape 3) qui a déjà détruit les mappings.
iommu_domain_registry::release_domain(pid);
```

---

## CORR-23 ⚠️ — Documentation `domain_of_pid` dans Driver Framework

La section §3.3 de Driver Framework v10 doit référencer le nouveau fichier :

```markdown
### 3.3 (ajout) — IommuDomainRegistry

// Voir kernel/src/drivers/iommu/domain_registry.rs
// Créé par CORR-16 : spécification manquante de domain_of_pid()
//
// INTÉGRATION dans sys_pci_claim() (device_server) :
//   Avant spawn du driver, appeler assign_domain(driver_pid).
//   Le DomainID retourné est configuré dans l'IOMMU hardware.
//
// INTÉGRATION dans sys_dma_map() :
//   domain_id = IOMMU_DOMAIN_REGISTRY.domain_of_pid(requesting_pid)?
//   ← remplace l'appel actuel à iommu::domain_of_pid()
//
// do_exit() :
//   Étape 8 : IOMMU_DOMAIN_REGISTRY.release_domain(pid)
```

---

*ExoOS — Corrections Driver Framework — Mars 2026*
