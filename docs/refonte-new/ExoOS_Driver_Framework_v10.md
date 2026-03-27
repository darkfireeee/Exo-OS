# ExoOS Driver Framework — v10
**Architecture · Création · Organisation · Erreurs silencieuses**
Analyse croisée v8 + double passe finale — ExoOS Phase 8
> Révision v10 basée sur audit croisé de 6 modèles (Z-AI, Gemini, Kimi, MiniMax, Grok4, Copilote)
> sur v8 et v9 + double passe d'analyse propre pour éliminer les bugs restants.
> Types canoniques dans `ExoOS_Kernel_Types_v10.md` — source unique de vérité.

---

## ∆ Changelog v8 → v10

### Bugs corrigés dans v10

| ID | Sévérité | Source | Localisation | Problème | Correction |
|----|----------|--------|----|----------|------------|
| FIX-103 | 🔴 Critique | Z-AI COMPIL-01 | `time.rs` | `static BOOT_TSC_KHZ: u64` immutable → compile error | → `AtomicU64` *(dans Kernel_Types)* |
| FIX-104 | 🟠 Majeur | KIMI/MINIMAX | `fault_queue.rs` | `compare_exchange_weak` → spurious failures ARM/RISC-V | → `compare_exchange` strong *(dans Kernel_Types)* |
| FIX-108 | 🔴 Critique | KIMI v8 / GROK4 | `routing.rs` dispatch_irq | EOI LAPIC manquant pour IRQ Level blacklistée → bit ISR LAPIC bloqué → système gelé | `lapic_send_eoi()` toujours dans early-return blacklist |
| FIX-109 | 🔴 Critique | Z-AI ARCH-01 | `routing.rs` dispatch_irq | `scheduler::yield_current_thread()` dans ISR context → corruption pile noyau | Remplacé par drop-after-SPIN_THRESHOLD en ISR |
| FIX-110 | 🟡 Mineur | GROK4 | `routing.rs` syscall table | Comment `bdf` SYS_IRQ_REGISTER dit "requis" mais `Option<PciBdf>` → incohérence | Commentaire corrigé : "optionnel" |
| FIX-111 | 🟡 Mineur | COPILOTE | `routing.rs` dispatch_irq | `masked_since` pas reset à 0 après fin de storm → CAS 0→now ambigu | Reset masked_since=0 dans ack_irq(remaining==0) et watchdog |
| FIX-112 | 🟡 Mineur | double-passe | `routing.rs` sys_irq_register | `handled_count` non reset dans `!is_new` path → ghost IRQ detection cassée après driver restart post-watchdog | Reset handled_count quand pending_acks==0 && !is_new |
| FIX-113 | 🟡 Mineur | COPILOTE | `routing.rs` dispatch_irq | `masked_since.store()` en cas de race deux CPUs → CAS-based 0→now | `compare_exchange(0, now)` pour masked_since |

---

## 2 — Architecture en couches

### 2.2 — Syscalls (table complète v10)

```rust
SYS_IRQ_REGISTER    = 530  // (irq: u8, endpoint: IpcEndpoint, kind: IrqSourceKind,
                            //  bdf: Option<PciBdf>)
                            //   → Result<reg_id: u64, IrqError>
                            //   FIX-110 v10 : bdf est OPTIONNEL.
                            //   None si IRQ non-PCI (timer HPET, ACPI GPE, etc.).
                            //   Some(bdf) REQUIS pour les devices PCI (do_exit PCIe reset).

SYS_IRQ_ACK         = 531  // (irq: u8, reg_id: u64, handler_gen: u64,
                            //   wave_gen: u64, result: IrqAckResult)
                            //   handler_gen : génération du handler (anti ghost-handler)
                            //   wave_gen    : génération de la vague (anti stale-ACK FIX-56)
                            //   OBLIGATION : appeler même si result=NotMine (DRV-08)
                            //   → Result<(), IrqError>

SYS_MMIO_MAP        = 532  // (phys: PhysAddr, size: usize) → Result<*mut u8, MmioError>
SYS_MMIO_UNMAP      = 533  // (virt: *mut u8, size: usize) → Result<(), MmioError>

SYS_DMA_ALLOC       = 534  // (size: usize, dir: DmaDirection)
                            //   → Result<(virt: *mut u8, iova: IoVirtAddr), DmaError>
SYS_DMA_FREE        = 535  // (iova: IoVirtAddr, size: usize) → Result<(), DmaError>
SYS_DMA_SYNC        = 536  // (iova: IoVirtAddr, size: usize, dir: DmaDirection)
                            //   → Result<(), DmaError>

SYS_PCI_CFG_READ    = 537  // (offset: u16) → Result<u32, PciError>
SYS_PCI_CFG_WRITE   = 538  // (offset: u16, val: u32) → Result<(), PciError>
SYS_PCI_BUS_MASTER  = 539  // (enable: bool) → Result<(), PciError>

SYS_PCI_CLAIM       = 540  // (phys: PhysAddr, size: usize, driver_pid: u32,
                            //   bdf: Option<PciBdf>)              ← FIX-95 v8
                            //   → Result<(), ClaimError>  [capability SysDeviceAdmin]
                            //   bdf stocké dans DeviceClaim pour bdf_of_pid() (polling drivers)

SYS_DMA_MAP         = 541  // (vaddr: usize, size: usize, dir: DmaDirection)
                            //   → Result<IoVirtAddr, DmaError>
SYS_DMA_UNMAP       = 542  // (domain_id: u32, iova: IoVirtAddr)
                            //   → Result<(), DmaError>

SYS_MSI_ALLOC       = 543  // (count: u16) → Result<handle: u64, MsiError>
SYS_MSI_CONFIG      = 544  // (handle: u64, vector_idx: u16) → Result<(), MsiError>
SYS_MSI_FREE        = 545  // (handle: u64) → Result<(), MsiError>

SYS_PCI_SET_TOPOLOGY = 546 // (child_bdf: PciBdf, parent_bdf: PciBdf)
                            //   → Result<(), PciError>  [capability SysDeviceAdmin]
                            //   Appelé par device_server après scan PCI
                            //   Requis pour wait_link_retraining dans do_exit()
```

---

## 3 — Ring 0 : Infrastructure kernel driver

### 3.0 — Types fondamentaux

*Définis dans `ExoOS_Kernel_Types_v10.md` — source autoritaire.*

### 3.1 — IRQ routing

#### Structs IRQ v10

```rust
// kernel/src/arch/x86_64/irq/routing.rs — VERSION v10

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

pub struct IrqRoute {
    irq_line:            u8,
    source_kind:         IrqSourceKind,
    handlers:            Vec<IrqHandler>,
    pending_acks:        AtomicU32,
    handled_count:       AtomicU32,
    dispatch_generation: AtomicU64,  // génération de vague (anti stale-ACK FIX-56)
    masked:              AtomicBool,
    /// Timestamp en ms du début du BLOCAGE COURANT.
    /// Initialisé à l'entrée en blocage, reset à 0 à la fin du blocage.
    /// FIX-66 v7 : figé au premier dispatch de la vague (pas rafraîchi en storm).
    /// FIX-111 v10 : reset à 0 en fin de vague pour permettre CAS 0→now au prochain blocage.
    masked_since:        AtomicU64,
    soft_alarmed:        AtomicBool,
    overflow_count:      AtomicU32,  // compteur de storms (FIX-80 v8, FIX-99 v8)
    pci_bdf:             Option<PciBdf>, // BDF pour wait_link_retraining (FIX-85 v9)
}

const MAX_PENDING_ACKS: u32 = 4096;
const MAX_OVERFLOWS:    u32 = 5;
/// Nombre de spins CAS avant abandon en contexte ISR (FIX-109 v10).
/// En ISR, yield est interdit — on doit drop l'IRQ plutôt que spinner indéfiniment.
const SPIN_THRESHOLD:   u32 = 8;
```

#### `sys_irq_register` v10

```rust
// kernel/src/arch/x86_64/irq/routing.rs — sys_irq_register VERSION v10

pub fn sys_irq_register(
    irq:         u8,
    endpoint:    IpcEndpoint,
    source_kind: IrqSourceKind,
    bdf:         Option<PciBdf>,
) -> Result<u64, IrqError> {
    let _irq_guard = arch::irq_save();
    let mut table  = IRQ_TABLE.write();

    let is_new = table[irq as usize].is_none();

    let route = table[irq as usize].get_or_insert_with(|| IrqRoute {
        irq_line:            irq,
        source_kind,
        handlers:            Vec::new(),
        pending_acks:        AtomicU32::new(0),
        handled_count:       AtomicU32::new(0),
        dispatch_generation: AtomicU64::new(0),
        masked:              AtomicBool::new(false),
        masked_since:        AtomicU64::new(0),
        soft_alarmed:        AtomicBool::new(false),
        overflow_count:      AtomicU32::new(0),
        pci_bdf:             bdf,
    });

    if !is_new {
        // FIX-67 v7 : vérifier compatibilité trigger type
        if route.source_kind != source_kind {
            return Err(IrqError::KindMismatch {
                existing:  route.source_kind,
                requested: source_kind,
            });
        }

        // FIX-99 v8 : reset overflow_count sur re-registration.
        // Le driver redémarre → ardoise vierge pour les storms futurs.
        route.overflow_count.store(0, Ordering::Relaxed);

        // FIX-112 v10 : reset handled_count si pas de storm en cours.
        // POURQUOI : si le watchdog hard reset s'est produit, il reset pending_acks=0
        //   mais NE reset PAS handled_count. La valeur héritée de handled_count
        //   peut fausser la détection "ghost IRQ" (all_not_mine) lors du prochain storm.
        //   Si pending_acks==0, il n'y a pas de storm en cours → reset propre.
        //   Si pending_acks>0, un storm est toujours en cours → ne pas toucher
        //   (handled_count compte la vague courante).
        if route.pending_acks.load(Ordering::Acquire) == 0 {
            route.handled_count.store(0, Ordering::Relaxed);
        }

        // FIX-99 v8 + FIX-102 v8 : si le driver redémarre après une blacklist
        // (IOAPIC masqué par overflow), et qu'il n'y a plus de storm en cours
        // (pending_acks==0), démasquer l'IOAPIC pour que le nouveau driver reçoive ses IRQs.
        // DANGER : ne pas démasquer si pending_acks>0 (storm encore en cours).
        if route.masked.load(Ordering::Acquire)
            && route.pending_acks.load(Ordering::Relaxed) == 0
            && route.source_kind.needs_ioapic_mask()
        {
            ioapic_unmask(irq);
            route.masked.store(false, Ordering::Release);
        }

        route.soft_alarmed.store(false, Ordering::Relaxed);
        if let Some(b) = bdf { route.pci_bdf = Some(b); }
        // NE PAS toucher pending_acks, masked_since, dispatch_generation (FIX-67 v7)
        // masked_since : reset à 0 si pas de storm en cours
        //   (ack_irq(remaining==0) et watchdog hard reset le font déjà — FIX-111)
    }

    let generation  = GLOBAL_GEN.fetch_add(1, Ordering::Relaxed);
    let reg_id      = new_reg_id();
    let calling_pid = current_process::pid();

    route.handlers.retain(|h| h.owner_pid != calling_pid);
    route.handlers.push(IrqHandler { reg_id, generation, owner_pid: calling_pid, endpoint });

    Ok(reg_id)
}
```

#### `dispatch_irq` v10

```rust
// kernel/src/arch/x86_64/irq/routing.rs — dispatch_irq VERSION v10
//
// ⚠️ CONTEXTE D'EXÉCUTION : ISR (Interrupt Service Routine) — Ring 0
//    dispatch_irq est appelé depuis le vecteur d'interruption CPU.
//    RÈGLES ISR STRICTES :
//      1. Ne jamais appeler scheduler::yield_current_thread() [FIX-109 v10]
//      2. Ne jamais bloquer (spin limité, puis drop de l'IRQ)
//      3. Toujours envoyer LAPIC EOI si l'IRQ a été reçue par le LAPIC [FIX-108 v10]
//      4. Retourner rapidement

pub fn dispatch_irq(irq: u8) {
    let (endpoints, wave_gen) = {
        let table = IRQ_TABLE.read();
        match &table[irq as usize] {
            None => {
                // IRQ sans handler enregistré — EOI et sortie propre
                drop(table);
                lapic_send_eoi();
                return;
            }
            Some(route) => {

                // ═══════════════════════════════════════════════════════════════
                // ÉTAPE 1 : VÉRIFICATION BLACKLIST EN PREMIER (FIX-92 v8)
                //
                // POURQUOI en premier :
                //   Si on faisait le CAS pending_acks d'abord puis vérifiait la blacklist,
                //   on mettrait pending_acks=n SANS envoyer les IPC → watchdog fire spurieux.
                //   La vérification AVANT toute mutation empêche cet état incohérent.
                //
                // FIX-108 v10 : EOI LAPIC OBLIGATOIRE pour TOUS types (Level ET Edge/MSI).
                //
                //   POURQUOI Level nécessite un EOI ici (KIMI v8 / GROK4) :
                //     Le LAPIC a reçu l'IRQ Level → bit ISR LAPIC SET.
                //     Sans EOI, l'ISR bit reste bloqué → toutes les IRQs de même/inférieure
                //     priorité vectorielle ne seront plus délivrées jusqu'au reboot.
                //     Scénario typique : IOAPIC était masqué (blacklist), un glitch
                //     électrique ou un race de démasquage a quand même livré l'IRQ au LAPIC.
                //     Dans tous les cas : si le LAPIC nous a appelés, il faut EOI.
                //
                //   Note : pour Level, l'EOI ici NE démasque PAS l'IOAPIC (il reste masqué
                //   car la blacklist l'a masqué). C'est correct — l'IOAPIC ne doit pas
                //   re-livrer des IRQs blacklistées.
                // ═══════════════════════════════════════════════════════════════
                if route.overflow_count.load(Ordering::Relaxed) >= MAX_OVERFLOWS {
                    // FIX-108 v10 : EOI TOUJOURS (Level et Edge/MSI).
                    // Le LAPIC a reçu l'IRQ → bit ISR SET → doit être acquitté.
                    lapic_send_eoi();
                    return; // Pas de modification de pending_acks
                }

                // ═══════════════════════════════════════════════════════════════
                // ÉTAPE 2 : PROTOCOLE EOI SELON SOURCE KIND
                // ═══════════════════════════════════════════════════════════════
                match route.source_kind {
                    IrqSourceKind::IoApicLevel => {
                        // Level : masquer IOAPIC AVANT de traiter (re-trigger prévenu)
                        // EOI LAPIC différé → envoyé par ack_irq(remaining==0)
                        ioapic_mask(irq);
                        // FIX-113 v10 : CAS 0→now pour masked_since (seul premier écrivain gagne)
                        // Garantit que masked_since est fixé atomiquement au début du PREMIER
                        // blocage, même si deux CPUs entrent simultanément (race sur Level IRQ
                        // partagée entre deux CPUs avant que le masquage IOAPIC soit visible).
                        let now = current_time_ms();
                        let _ = route.masked_since.compare_exchange(
                            0, now, Ordering::Relaxed, Ordering::Relaxed
                        );
                        route.masked.store(true, Ordering::Release);
                    }
                    IrqSourceKind::IoApicEdge | IrqSourceKind::Msi | IrqSourceKind::MsiX => {
                        // Edge/MSI : EOI immédiat (le device ne re-levera pas la ligne)
                        lapic_send_eoi();
                        // FIX-113 v10 : CAS 0→now pour masked_since
                        // Avant : `if prev_pending == 0 { masked_since.store(now) }` — race si
                        // deux CPUs voient tous les deux prev_pending==0 et stockent masked_since.
                        // Avec CAS : seul le premier écrivain (prev==0) réussit.
                        let now = current_time_ms();
                        let _ = route.masked_since.compare_exchange(
                            0, now, Ordering::Relaxed, Ordering::Relaxed
                        );
                        route.masked.store(false, Ordering::Release);
                    }
                }

                // ═══════════════════════════════════════════════════════════════
                // ÉTAPE 3 : COLLECTER LES ENDPOINTS ET INCRÉMENTER LA GÉNÉRATION
                // ═══════════════════════════════════════════════════════════════
                let eps: Vec<IpcEndpoint> = route.handlers.iter()
                    .map(|h| h.endpoint.clone())
                    .collect();
                let n = eps.len() as u32;

                // FIX-56 v7 : incrémenter génération de vague avant dispatch
                let wg = route.dispatch_generation.fetch_add(1, Ordering::AcqRel) + 1;

                // ═══════════════════════════════════════════════════════════════
                // ÉTAPE 4 : MISE À JOUR pending_acks SELON TYPE DE SOURCE
                // ═══════════════════════════════════════════════════════════════
                match route.source_kind {
                    IrqSourceKind::IoApicLevel => {
                        // Level : store atomique (1 dispatch = N acks attendus)
                        route.pending_acks.store(n, Ordering::Release);
                    }
                    _ => {
                        // Edge/MSI : CAS loop avec plafond MAX_PENDING_ACKS.
                        //
                        // FIX-83 v9 : overflow_count.fetch_add DANS Ok() (CAS winner only).
                        // FIX-92 v8 : dans la branche overflow, reset pending_acks=0 si blacklist.
                        // FIX-109 v10 : PLUS DE yield_current_thread() EN ISR CONTEXT.
                        //
                        // POURQUOI yield est interdit ici (Z-AI ARCH-01) :
                        //   dispatch_irq s'exécute en ISR context (appelé depuis le vecteur CPU).
                        //   Un ISR "emprunte" la pile et le contexte du thread interrompu.
                        //   Appeler yield_current_thread() depuis un ISR tente de préempter
                        //   ce thread, ce qui corrompt sa pile noyau et crashe le scheduler.
                        //   SOLUTION : après SPIN_THRESHOLD tentatives CAS infructueuses,
                        //   dropper l'IRQ. Le watchdog détectera une réactivité dégradée
                        //   si la situation est récurrente. C'est le comportement correct
                        //   pour un ISR : "fail fast" plutôt que "spin forever".
                        //
                        // CONTRASTE avec wait_link_retraining : cette fonction s'exécute en
                        // THREAD context (do_exit via watchdog kernel thread) → yield OK.

                        let mut current    = route.pending_acks.load(Ordering::Relaxed);
                        let mut spin_count = 0u32;

                        loop {
                            if current > MAX_PENDING_ACKS {
                                // Storm : overflow détecté
                                // FIX-98 v8 : initialiser masked_since si pas encore fait
                                // (cas très rare : premier dispatch est déjà en overflow)
                                // FIX-113 v10 : CAS 0→now (idempotent si déjà set)
                                let now = current_time_ms();
                                let _ = route.masked_since.compare_exchange(
                                    0, now, Ordering::Relaxed, Ordering::Relaxed
                                );

                                match route.pending_acks.compare_exchange(
                                    current, n, Ordering::Release, Ordering::Relaxed
                                ) {
                                    Ok(_) => {
                                        // FIX-83 v9 : UNIQUEMENT ici (CAS winner)
                                        let ov = route.overflow_count
                                            .fetch_add(1, Ordering::Relaxed) + 1;
                                        log::error!(
                                            "IRQ {} Edge pending_acks overflow ({}), storm #{}/{}",
                                            irq, current, ov, MAX_OVERFLOWS
                                        );
                                        if ov >= MAX_OVERFLOWS {
                                            log::error!(
                                                "IRQ {} : {} storms → blacklist définitive",
                                                irq, ov
                                            );
                                            ioapic_mask(irq);
                                            // FIX-92 v8 : reset pending_acks=0 — personne ne fera ACK
                                            route.pending_acks.store(0, Ordering::Release);
                                            device_server_ipc::notify_irq_blacklisted(irq as u8);
                                            return; // sans IPC handlers
                                        }
                                        device_server_ipc::notify_driver_stall(irq as u8);
                                        break;
                                    }
                                    Err(actual) => {
                                        current = actual;
                                        spin_count += 1;
                                        if spin_count >= SPIN_THRESHOLD {
                                            // FIX-109 v10 : ISR context → JAMAIS yield.
                                            // Contention extrême → dropper cette vague.
                                            log::warn!(
                                                "IRQ {} CAS contention extreme en ISR ({} spins) \
                                                 → vague droppée (watchdog détectera si récurrent)",
                                                irq, spin_count
                                            );
                                            return;
                                        }
                                        core::hint::spin_loop();
                                        continue;
                                    }
                                }
                            } else {
                                match route.pending_acks.compare_exchange(
                                    current, current + n, Ordering::AcqRel, Ordering::Relaxed
                                ) {
                                    Ok(_prev) => {
                                        // masked_since déjà géré par CAS 0→now ci-dessus (FIX-113)
                                        if _prev == 0 {
                                            // Nouveau blocage frais → ardoise vierge pour overflow
                                            route.overflow_count.store(0, Ordering::Relaxed);
                                        }
                                        break;
                                    }
                                    Err(actual) => {
                                        current = actual;
                                        spin_count += 1;
                                        if spin_count >= SPIN_THRESHOLD {
                                            // FIX-109 v10 : ISR — drop plutôt que yield
                                            log::warn!(
                                                "IRQ {} CAS spin limit en ISR → vague droppée",
                                                irq
                                            );
                                            return;
                                        }
                                        core::hint::spin_loop();
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                }

                (eps, wg)
            }
        }
    }; // read lock libéré

    for ep in endpoints {
        ipc::send_irq_notification(ep, irq, wave_gen);
    }
}
```

#### `ack_irq` v10

```rust
// kernel/src/arch/x86_64/irq/routing.rs — ack_irq VERSION v10
// v8 conservé intégralement + FIX-111 v10 (reset masked_since=0 en fin de vague).

pub fn ack_irq(
    irq:         u8,
    reg_id:      u64,
    handler_gen: u64,  // génération du handler (anti ghost-handler)
    wave_gen:    u64,  // génération de la vague (anti stale-ACK)
    result:      IrqAckResult,
) -> Result<(), IrqError> {
    let table = IRQ_TABLE.read();
    let route = table[irq as usize].as_ref().ok_or(IrqError::NotRegistered)?;

    route.handlers.iter()
        .find(|h| h.reg_id == reg_id && h.generation == handler_gen)
        .ok_or(IrqError::NotOwner)?;

    let current_wave = route.dispatch_generation.load(Ordering::Acquire);
    let is_stale     = wave_gen != current_wave;

    // FIX-65 v7 : logique différenciée Level vs Edge
    if is_stale && !route.source_kind.is_cumulative() {
        return Ok(());
    }

    if !is_stale && matches!(result, IrqAckResult::Handled) {
        route.handled_count.fetch_add(1, Ordering::AcqRel);
    }

    // FIX-65 v7 : décrémenter TOUJOURS pour Edge/MSI (stale ou non)
    let prev = route.pending_acks.fetch_sub(1, Ordering::AcqRel);
    if prev == 0 {
        route.pending_acks.store(0, Ordering::Release);
        log::warn!("IRQ {} ack_irq underflow (reg_id={}, wave={})", irq, reg_id, wave_gen);
        return Ok(());
    }

    if is_stale { return Ok(()); }

    let remaining = prev - 1;
    if remaining == 0 {
        let all_not_mine = route.handled_count.load(Ordering::Acquire) == 0;

        // FIX-71 v7 : reset handled_count AVANT action hardware
        route.handled_count.store(0, Ordering::Release);

        // FIX-88 v9 : reset overflow_count — fin de vague normale = ardoise vierge
        route.overflow_count.store(0, Ordering::Relaxed);

        // FIX-111 v10 : reset masked_since à 0 — fin de vague = fin du blocage courant.
        // POURQUOI : masked_since représente le début du BLOCAGE COURANT.
        // Quand remaining==0, le blocage est terminé. Au prochain dispatch (nouveau blocage),
        // on veut que CAS(0 → now) réussisse pour marquer le début du nouveau blocage.
        // Sans ce reset : masked_since garde l'ancienne valeur → CAS échoue silencieusement
        // → masked_since reste à l'ancien temps → watchdog mesure elapsed = now - ancien_time
        // → watchdog peut fire prématurément sur un storm légèrement tardif.
        route.masked_since.store(0, Ordering::Relaxed);

        // FIX-52 v6 : NotMine Storm Level
        if all_not_mine && route.source_kind == IrqSourceKind::IoApicLevel {
            log::warn!(
                "IRQ {} : ghost IRQ (tous NotMine, level) — LAPIC EOI envoyé, IOAPIC masqué",
                irq
            );
            lapic_send_eoi();
            device_server_ipc::notify_unhandled_irq(irq);
            return Ok(());
        }

        match route.source_kind {
            IrqSourceKind::IoApicLevel => {
                lapic_send_eoi();
                ioapic_unmask(irq);
            }
            _ => { /* EOI déjà dans dispatch_irq — FIX-41 v5 */ }
        }

        route.masked.store(false, Ordering::Release);
        route.soft_alarmed.store(false, Ordering::Relaxed);
    }
    Ok(())
}
```

#### Watchdog v10

```rust
// kernel/src/arch/x86_64/irq/watchdog.rs — VERSION v10
// FIX-111 v10 : reset masked_since=0 dans hard reset (cohérence avec ack_irq).

fn irq_watchdog_tick() {
    let now   = current_time_ms();
    let table = IRQ_TABLE.read();

    for irq in 0usize..256 {
        let Some(route) = &table[irq] else { continue };

        let pending = route.pending_acks.load(Ordering::Relaxed);
        if pending == 0 { continue; }

        let elapsed = now.saturating_sub(route.masked_since.load(Ordering::Relaxed));
        let cfg     = watchdog_config_for_irq(irq as u8);

        if elapsed > cfg.soft_ms && !route.soft_alarmed.swap(true, Ordering::Relaxed) {
            log::warn!(
                "IRQ {} ({:?}) soft watchdog ({} ms), pending_acks={}",
                irq, route.source_kind, elapsed, pending
            );
        }

        if elapsed > cfg.hard_ms {
            log::error!(
                "IRQ {} ({:?}) hard watchdog ({} ms) → force reset",
                irq, route.source_kind, elapsed
            );

            // FIX-77 v8 : dispatch_generation incrémenté AVANT reset de pending
            route.dispatch_generation.fetch_add(1, Ordering::AcqRel);

            if route.source_kind == IrqSourceKind::IoApicLevel {
                lapic_send_eoi();
                ioapic_unmask(irq as u8);
            }

            route.pending_acks.store(0, Ordering::Release);
            route.handled_count.store(0, Ordering::Release);
            route.masked.store(false, Ordering::Release);
            route.soft_alarmed.store(false, Ordering::Relaxed);

            // FIX-111 v10 : reset masked_since=0 après hard reset.
            // Cohérence avec ack_irq(remaining==0) qui reset aussi masked_since.
            // Permet au prochain dispatch d'utiliser CAS(0→now) correctement.
            route.masked_since.store(0, Ordering::Relaxed);

            // NE PAS reset overflow_count ici (intentionnel — anomalie persistante).
            // Reset uniquement via : dispatch(prev==0), ack_irq(remaining==0), re-register.

            device_server_ipc::notify_driver_stall(irq as u8);
        }
    }
}
```

---

### 3.2 — MMIO Capabilities

*(Inchangé depuis v5 — FIX-26 conservé)*

---

### 3.3 — DMA management + IOMMU

#### `sys_dma_map` v10

*(Identique à v8/v7 FIX-68 — ordre COW avant query_perms. Conservé intégralement.)*

```rust
// kernel/src/drivers/dma.rs — SYS_DMA_MAP VERSION v10

pub fn sys_dma_map(vaddr: usize, size: usize, dir: DmaDirection) -> Result<IoVirtAddr, DmaError> {
    let requesting_pid = current_process::pid();
    let page_count = (size + PAGE_SIZE - 1) / PAGE_SIZE;
    let mut pinned_pages: Vec<PinnedPage> = Vec::with_capacity(page_count);

    for i in 0..page_count {
        let vpage = vaddr + i * PAGE_SIZE;

        // FIX-68 v7 : COW AVANT query_perms
        if matches!(dir, DmaDirection::FromDevice | DmaDirection::Bidirectional) {
            page_tables::resolve_cow_or_fault(requesting_pid, vpage, PageProtection::WRITE)
                .map_err(|e| { for p in &pinned_pages { p.unpin(); }
                    match e { CowError::OutOfMemory => DmaError::OutOfMemory,
                               _                    => DmaError::InvalidVaddr } })?;
        }

        let perms = page_tables::query_perms_single(requesting_pid, vpage)
            .ok_or_else(|| { for p in &pinned_pages { p.unpin(); } DmaError::InvalidVaddr })?;

        if matches!(dir, DmaDirection::FromDevice | DmaDirection::Bidirectional)
            && !perms.is_writable()
        {
            for p in &pinned_pages { p.unpin(); }
            return Err(DmaError::PermissionDenied);
        }

        let pinned = page_tables::pin_user_page(requesting_pid, vpage)
            .ok_or_else(|| { for p in &pinned_pages { p.unpin(); } DmaError::InvalidVaddr })?;
        pinned_pages.push(pinned);
    }

    let domain_id = iommu::domain_of_pid(requesting_pid).map_err(|_| DmaError::IommuError)?;
    let iova_base = iommu::alloc_iova_range(domain_id, page_count)
        .map_err(|_| DmaError::IovaSpaceExhausted)?;

    for (i, pinned) in pinned_pages.iter().enumerate() {
        let iova_page = IoVirtAddr(iova_base.0 + (i * PAGE_SIZE) as u64);
        if let Err(_) = iommu::map_page(domain_id, iova_page, pinned.phys_addr(), dir) {
            for j in 0..i {
                iommu::unmap_page(domain_id, IoVirtAddr(iova_base.0 + (j * PAGE_SIZE) as u64));
            }
            for p in &pinned_pages { p.unpin(); }
            return Err(DmaError::IommuError);
        }
    }

    dma_map_table::register(requesting_pid, domain_id, iova_base, pinned_pages, size);
    Ok(iova_base)
}
```

#### IOMMU fault handler v10

```rust
// kernel/src/drivers/iommu/fault_handler.rs — VERSION v10
// Utilise IommuFaultQueue CAS-based strong (ExoOS_Kernel_Types_v10.md §7).

pub fn iommu_fault_isr(domain_id: u16, iova: IoVirtAddr, reason: IommuFaultReason) {
    // 1. Désactiver le domaine atomiquement (ISR-safe)
    iommu::disable_domain_atomic(domain_id as u32);

    // 2. Logger sans allocation
    log::error!("IOMMU FAULT ISR domain={} iova={:?} reason={:?}", domain_id, iova, reason);

    // 3. Push dans la queue CAS-based strong (FIX-91 + FIX-104)
    let pushed = IOMMU_FAULT_QUEUE.push(IommuFaultEvent {
        domain_id, iova, reason, timestamp: current_time_ms(),
    });
    if !pushed {
        log::error!("IOMMU_FAULT_QUEUE saturée — faute domain={} perdue", domain_id);
    }

    lapic_send_eoi();
}

pub fn iommu_fault_worker_tick() {
    let dropped = IOMMU_FAULT_QUEUE.drain_dropped();
    if dropped > 0 {
        log::error!("IOMMU FAULT QUEUE : {} fautes perdues (contention ou queue pleine)", dropped);
    }

    while let Some(event) = IOMMU_FAULT_QUEUE.pop() {
        log::error!(
            "IOMMU FAULT (t={}ms) domain={} iova={:?} reason={:?}",
            event.timestamp, event.domain_id, event.iova, event.reason
        );
        // FIX-78 v8 : kill différé via IPC — jamais depuis ISR
        if let Some(pid) = iommu::pid_of_domain(event.domain_id as u32) {
            device_server_ipc::notify_iommu_fault_kill(pid, event.iova, event.reason);
        }
    }
}
```

---

### 3.3 (suite) — `bdf_of_pid` v10

```rust
// kernel/src/drivers/device_claims.rs — bdf_of_pid VERSION v10
// Identique à v8 FIX-95 — référence canonique.

pub fn bdf_of_pid(pid: u32) -> Option<PciBdf> {
    // 1. IRQ_TABLE (priorité — handler actif = BDF fiable)
    {
        let table = IRQ_TABLE.read();
        for slot in table.iter() {
            if let Some(route) = slot {
                if route.handlers.iter().any(|h| h.owner_pid == pid) {
                    if let Some(bdf) = route.pci_bdf {
                        return Some(bdf);
                    }
                }
            }
        }
    }

    // 2. DEVICE_CLAIMS (polling drivers sans IRQ — FIX-95 v8)
    {
        let claims = DEVICE_CLAIMS.read();
        for claim in claims.iter() {
            if claim.owner_pid == pid {
                if let Some(bdf) = claim.bdf {
                    return Some(bdf);
                }
            }
        }
    }

    None
}

pub fn sys_pci_claim(
    phys_base:   PhysAddr,
    size:        usize,
    driver_pid:  u32,
    bdf:         Option<PciBdf>,
    calling_pid: u32,
) -> Result<(), ClaimError> {
    if !process::has_capability(calling_pid, Capability::SysDeviceAdmin) {
        return Err(ClaimError::PermissionDenied);
    }
    if !MMIO_WHITELIST.contains(phys_base, size) {
        return Err(ClaimError::NotInHardwareRegion);
    }
    if memory_map::is_ram_region(phys_base, size) {
        return Err(ClaimError::PhysIsRam);
    }
    let mut claims = DEVICE_CLAIMS.write();
    if claims.iter().any(|c| c.overlaps(phys_base, size)) {
        return Err(ClaimError::AlreadyClaimed);
    }
    let gen = process::get_generation(driver_pid);
    claims.push(DeviceClaim { phys_base, size, owner_pid: driver_pid, generation: gen, bdf });
    Ok(())
}
```

---

### 3.3 (suite) — `wait_link_retraining` v10

```rust
// servers/device_server/src/pci/link_retraining.rs — VERSION v10
// Identique à v8 — référence canonique.
// NOTE : cette fonction s'exécute en THREAD CONTEXT (do_exit via watchdog kernel thread).
//        Le yield_current_thread() ici EST correct (contrairement à dispatch_irq).

pub fn wait_link_retraining(device_bdf: PciBdf, timeout_ms: u64) -> Result<(), PciError> {
    // FIX-94 v8 : Fallback quarantaine si bridge inconnu.
    let bridge_bdf = match PCI_TOPOLOGY.parent_bridge(device_bdf) {
        Some(b) => b,
        None => {
            log::warn!(
                "wait_link_retraining: bridge inconnu pour {:?} (topology non peuplée ?) \
                 → quarantaine conservatrice 250ms avant unmap IOMMU",
                device_bdf
            );
            // 250ms > T_rhfa (100ms spec PCIe §6.6.1) → bus stable garantit
            timer::sleep_ms(250);
            return Ok(());
        }
    };

    let pcie_cap = pci_find_capability(bridge_bdf, PCI_CAP_ID_EXP)
        .ok_or(PciError::LinkTrainingTimeout)?;

    // FIX-86 v9 : délai obligatoire spec PCIe §6.6.1 avant lecture registres
    timer::sleep_ms(100);

    let lnksta_offset = pcie_cap + PCI_EXP_LNKSTA;
    let deadline      = current_time_ms() + timeout_ms;

    loop {
        // FIX-96 v8 : pci_cfg_read16 via helper read32+shift
        let lnksta = pci_cfg_read16(bridge_bdf, lnksta_offset);
        if lnksta & PCI_EXP_LNKSTA_DLLLA != 0 {
            return Ok(());
        }
        if current_time_ms() >= deadline {
            log::error!(
                "wait_link_retraining timeout ({}ms) bridge={:?} — link non actif",
                timeout_ms, bridge_bdf
            );
            return Err(PciError::LinkTrainingTimeout);
        }
        // NOTE : yield ici EST CORRECT — on est en thread context, pas en ISR.
        core::hint::spin_loop();
        scheduler::yield_current_thread(); // FIX-86 v9 — thread context uniquement
    }
}
```

---

### 3.4 — `do_exit()` v10

```rust
// kernel/src/process/lifecycle.rs — do_exit() VERSION v10
// Identique à v8 FIX-101 — référence canonique.

pub fn do_exit(pid: u32) {
    // ORDRE CRITIQUE v10 (identique v8)

    // 1. Désactiver le Bus Mastering
    pci::disable_bus_master_for_pid(pid);

    // 2. Attendre quiescence PCIe
    let needs_reset = match pci::wait_bus_master_quiesced(pid, 100) {
        Ok(())                               => false,
        Err(PciError::BusMasterQuiesceTimeout) => true,
        Err(_)                               => false,
    };

    if needs_reset {
        if let Some(bdf) = bdf_of_pid(pid) {
            log::error!("PID {} ({:?}) : non quiescent → secondary bus reset", pid, bdf);
            if let Ok(()) = pci::secondary_bus_reset_bdf(bdf) {
                // FIX-69 v7 + FIX-101 v8 : attendre link retraining AVANT unmap IOMMU.
                // FIX-94 v8 : si bridge inconnu → quarantaine 250ms dans wait_link_retraining.
                match wait_link_retraining(bdf, 200) {
                    Ok(()) => { /* link stable, safe pour unmap */ }
                    Err(PciError::LinkTrainingTimeout) => {
                        // FIX-101 v8 : timeout → forcer désactivation domaine IOMMU
                        if let Ok(domain_id) = iommu::domain_of_pid(pid) {
                            iommu::force_disable_domain(domain_id);
                            log::error!(
                                "PID {} : link training timeout (200ms) — domaine IOMMU {} \
                                 désactivé de force. Bus bridge={:?} instable.",
                                pid, domain_id, bdf
                            );
                        }
                        // Continuer : revoke_all_for_pid loggera les erreurs unmap
                    }
                    Err(e) => {
                        log::error!("PID {} : wait_link_retraining erreur {:?}", pid, e);
                    }
                }
            }
        } else {
            log::warn!("PID {} : BDF inconnu pour secondary reset → attente 250ms", pid);
            timer::sleep_ms(250);
        }
    }

    // 3. Révoquer les mappings DMA temporaires (TLB flush unique par domaine)
    dma_map_table::revoke_all_for_pid(pid);

    // 4. Révoquer les buffers DMA alloués (SYS_DMA_ALLOC)
    dma::revoke_all_alloc_for_pid(pid);

    // 5. Révoquer les mappings MMIO
    mmio_cap::revoke_all_mmio(pid);

    // 6. Désenregistrer les handlers IRQ
    irq::revoke_all_irq(pid);

    // 7. Libérer les claims PCI
    device_claims::revoke_claims_for_pid(pid);
}
```

---

### 3.5 — Séquence d'initialisation boot

```rust
// Ordre boot STRICT pour ExoOS Phase 8 :

// Étape 1 : Initialisation de la queue IOMMU (AVANT activation des IRQs IOMMU)
IOMMU_FAULT_QUEUE.init();  // FIX-100 v8 : Ordering::Release dans init()

// Étape 2 : Calibration de la source temporelle (AVANT enable_interrupts)
calibrate_tsc_khz();  // FIX-103 v10 : écrit dans AtomicU64 (plus de static u64 immutable)

// Étape 3 : Activer les interruptions CPU
// La barrière mémoire de enable_interrupts() garantit la visibilité de
// BOOT_TSC_KHZ (Ordering::Relaxed store) sur tous les CPUs.
arch::enable_interrupts();

// Étape 4 : Démarrer device_server Ring 1

// Séquence device_server STRICTE (servers/device_server/src/main.rs) :
// 4.1 : scan_pci_bus()               → découverte de tous les devices
// 4.2 : sys_pci_set_topology()       → peupler PCI_TOPOLOGY pour TOUS les devices
//                                       (AVANT sys_irq_register et match_and_spawn)
// 4.3 : sys_pci_claim(bdf=Some())    → claim des ressources + stockage BDF dans DeviceClaim
// 4.4 : sys_irq_register(bdf=Some()) → enregistrement IRQs avec BDF
// 4.5 : match_and_spawn()            → démarrage des drivers

// GARANTIES :
//   → PCI_TOPOLOGY complète avant tout sys_irq_register
//   → wait_link_retraining trouvera le bridge parent dans do_exit()
//   → BDF dans DeviceClaim via sys_pci_claim
//   → polling drivers trouvables dans bdf_of_pid()
//   → IOMMU_FAULT_QUEUE prête avant tout ISR IOMMU
//   → BOOT_TSC_KHZ valide avant tout appel current_time_ms()
```

---

## 5 — Generic Driver Interface (GDI)

*(Identique à v8/v7 FIX-75 et FIX-58)*

---

## 7 — Linux Shim

*(Identique à v8/v5 — KmallocHeader avec `_pad[11]`, sizeof=32, FIX-37)*

---

## 9 — Catalogue des erreurs silencieuses (v10 — exhaustif)

| ID | Module | Erreur silencieuse | Symptôme tardif | Correction |
|---|---|---|---|---|
| DRV-01 | irq/routing.rs | Driver crashe, `pending_acks` jamais à 0 | Device muet | Watchdog `pending_acks > 0` **FIX-53 v6** |
| DRV-02 | mmio_cap.rs | MMIO non révoqué, PID recyclé | Exploit trivial | `revoke_all_mmio()` ordre strict |
| DRV-03 | dma.rs | DMA after-free | Heap kernel corrompu | Drop + quiescence + link retraining |
| DRV-04 | dma.rs + pci | Bus mastering non désactivé | DMA arbitraire | `disable_bus_master` + quiescence |
| DRV-05 | irq/routing.rs | Ghost handler après restart | Commandes dupliquées | `reg_id` + `generation` + `retain` |
| DRV-06 | pci/bar.rs | BAR size sans save/restore | Device muet | save→write→read→restore |
| DRV-07 | linux_shim | `kfree()` IOVA manquante | Crash DMA | `KmallocHeader.iova` |
| DRV-08 | irq/routing.rs | IRQ partagée sans ACK `NotMine` | EOI bloqué | ACK obligatoire même sur NotMine |
| DRV-09 | pci/msi.rs | Table MSI-X écrasée | Wrong IRQ vectors | `SYS_MSI_CONFIG` opaque |
| DRV-10 | drivers/lifecycle | Driver spawné avant controller | Probe silencieux | Sort topologique |
| DRV-11 | linux_shim | `kmalloc` non aligné 16 | GPF silencieux | `_pad[11]` **FIX-37 v5** |
| DRV-12 | dma.rs | DMA write page RO via IOMMU | Corruption silencieuse | Perms par page **FIX-44 v5** |
| DRV-13 | dma.rs | Mapping IOMMU page unique | DMAR fault | Scatter-gather **FIX-27 v5** |
| DRV-14 | dma.rs | DMA map sur COW Zero Page | Corruption globale | `resolve_cow_or_fault` **FIX-42 v5** |
| DRV-15 | irq/routing.rs | Double EOI edge/MSI-X | Comportement indéfini | EOI dans `dispatch_irq` uniquement |
| DRV-16 | irq/routing.rs | `ioapic::is_level_triggered` sur MSI | EOI raté | `IrqSourceKind` **FIX-40 v5** |
| DRV-17 | dma.rs | Race translate/pin | IOMMU cible stale | `pin_user_page()` atomique |
| DRV-18 | dma.rs | TLB IOMMU stale après unmap | Accès post-libération | Flush groupé **FIX-54 v6** |
| DRV-19 | dma.rs | `requesting_pid` forgeable | Escalade IOMMU | `current_process::pid()` |
| DRV-20 | irq/routing.rs | NotMine Storm level → re-trigger | Boucle infinie | Garder masqué **FIX-49 v5** |
| DRV-21 | irq/routing.rs | LAPIC ISR bloqué ghost IRQ | Système gelé | `lapic_send_eoi()` NotMine Storm |
| DRV-22 | irq/routing.rs | Watchdog aveugle Edge/MSI | Crash non détecté | `pending_acks > 0` **FIX-53 v6** |
| DRV-23 | irq/routing.rs | `pending_acks` overflow u32 | EOI prématuré | CAS plafond `MAX_PENDING_ACKS` |
| DRV-24 | irq/routing.rs | ACK stale Edge non décrémenté | Fuite → watchdog abusif | Décrémenter stale Edge **FIX-65 v7** |
| DRV-25 | dma.rs | PCIe in-flight au moment unmap | Faute IOMMU ou MCE | `wait_link_retraining` |
| DRV-26 | pci/bar.rs | `bar_phys(N)` sur partie haute 64-bit | Adresse MMIO fantaisiste | Détection BAR 64-bit **FIX-58 v6** |
| DRV-27 | iommu/ | Faute IOMMU sans handler | Driver fautif non tué | `iommu_fault_isr` async |
| DRV-28 | dma_map_table | N flushes TLB pour N buffers | Latence crash | Flush groupé unique **FIX-54 v6** |
| DRV-29 | irq/routing.rs | `masked_since` rafraîchi sur storm | Watchdog timer blindé | Figé au premier blocage **FIX-66 v7** |
| DRV-30 | irq/routing.rs | `handled_count` cumulatif entre vagues | NotMine Storm Guard contourné | Reset `remaining==0` **FIX-71 v7** |
| DRV-31 | irq/routing.rs | Reset atomics sur route IRQ active | Corruption état | Flag `is_new` **FIX-67 v7** |
| DRV-32 | dma.rs | `query_perms` avant COW → PermissionDenied | DMA write impossible | COW avant `query_perms` **FIX-68 v7** |
| DRV-33 | pci/bar.rs | `raw==0xFFFF_FFFF` confondu BAR valide | Mapping MMIO sur erreur PCIe | `PciError::DeviceAbsent` |
| DRV-34 | irq/routing.rs | `masked_since` race load-séparé vs CAS | Watchdog timer décalé | CAS return value **FIX-76** |
| DRV-35 | irq/watchdog.rs | `dispatch_generation` non incrémenté hard reset | Stale ACKs non reconnus | `fetch_add` watchdog |
| DRV-36 | iommu/ | `process::kill()` dans worker IOMMU | Deadlock possible | IPC async |
| DRV-37 | pci/ | `wait_link_retraining` sans topologie bridge | Panique immédiate | `SYS_PCI_SET_TOPOLOGY` + fallback 250ms **FIX-94** |
| DRV-38 | irq/routing.rs | Storm DoS non plafonné | CPU kernel saturé | Blacklist après `MAX_OVERFLOWS` |
| DRV-39 | irq/routing.rs | `overflow_count.fetch_add` avant CAS | Fausse blacklist SMP | `fetch_add` dans `Ok(_)` uniquement |
| DRV-40 | iommu/fault_queue | ABA + torn read ou orphaned slots | Kill mauvais process / queue inutilisable | Queue CAS-based **FIX-91 v8** |
| DRV-41 | pci/link_retraining | Pas de délai 100ms post-reset | CA matériel → MCE → panic | `sleep_ms(100)` initial |
| DRV-42 | irq/routing.rs | `overflow_count` non reset fin de vague | Blacklist sur storms séparés | Reset dans `ack_irq` |
| DRV-43 | dma_types.rs | `DmaError::Overflow` hors scope DMA | Confusion API | `IovaSpaceExhausted` |
| DRV-44 | irq/routing.rs | `pending_acks=n` dans blacklist sans IPC | Watchdog hard reset abusif | Check blacklist avant CAS + reset=0 **FIX-92 v8** |
| DRV-45 | pci/topology | `spin::RwLock` sans `irq_save` → deadlock IRQ | Deadlock de cœur CPU | `irq_save` obligatoire **FIX-93 v8** |
| DRV-46 | do_exit/pci | `bdf_of_pid` aveugle aux polling drivers | Pas de reset PCIe pour GPU/crypto | BDF dans DeviceClaim **FIX-95 v8** |
| DRV-47 | pci/topology | heapless::Vec<256> saturé sur serveurs SR-IOV | Topology incomplète → MCE | Capacité 1024 **FIX-96 v8** |
| DRV-48 | irq/routing.rs | `overflow_count` héritée après crash driver | Blacklist injuste sur redémarrage | Reset sur re-registration **FIX-99 v8** |
| DRV-49 | irq/routing.rs | IOAPIC reste masqué après restart driver blacklisté | Driver ne reçoit plus d'IRQ | Unmask conditionnel **FIX-102 v8** |
| DRV-50 | irq/routing.rs | `masked_since` = 0 si premier dispatch overflow | Watchdog fire immédiat | Init dans branche overflow **FIX-98 v8** |
| DRV-51 | irq/routing.rs | EOI LAPIC manquant pour Level blacklistée | Bit ISR LAPIC bloqué → plus aucune IRQ délivrée → système gelé | `lapic_send_eoi()` toujours dans early return **FIX-108 v10** |
| DRV-52 | irq/routing.rs | `yield_current_thread()` dans dispatch_irq ISR | Corruption pile noyau / crash scheduler | Drop-after-SPIN_THRESHOLD en ISR **FIX-109 v10** |
| DRV-53 | time.rs | `static BOOT_TSC_KHZ: u64` immutable → compile error | Échec de compilation | `AtomicU64` **FIX-103 v10** |
| DRV-54 | iommu/fault_queue | `compare_exchange_weak` → spurious drops ARM/RISC-V | Perte d'événements IOMMU légitimes | `compare_exchange` strong **FIX-104 v10** |
| DRV-55 | irq/routing.rs | `handled_count` non reset après watchdog hard reset + driver restart | Ghost IRQ detection faux-négatif | Reset handled_count si pending==0 **FIX-112 v10** |

---

## Annexe A — Invariants atomiques v10

### Invariant overflow_count (v10)

```
overflow_count réinitialisé à 0 dans EXACTEMENT quatre cas :

1. dispatch_irq : CAS Ok, prev==0 → nouveau blocage frais = ardoise vierge
2. ack_irq : remaining==0 → fin de vague normale = ardoise vierge  [FIX-88 v9]
3. sys_irq_register : !is_new → driver redémarre = ardoise vierge    [FIX-99 v8]
4. JAMAIS dans watchdog hard reset (anomalie persistante = compteur conservé)

overflow_count.fetch_add() uniquement dans CAS Ok(_) → 1 incrément / overflow réel [FIX-83 v9]
Vérification AVANT toute mutation : si >= MAX_OVERFLOWS → EOI + return sans modifier état [FIX-92 v8 + FIX-108 v10]
```

### Invariant masked_since (v10)

```
masked_since représente le timestamp du DÉBUT DU BLOCAGE COURANT.
Unité : millisecondes (current_time_ms()).

INITIALISATION (FIX-113 v10) :
  CAS(0 → current_time_ms()) dans dispatch_irq :
  → Level : dans le match block (masquage IOAPIC)
  → Edge/MSI : dans le match block (après EOI immédiat)
  → overflow path : CAS(0 → now) (FIX-98 v8)
  Seul le PREMIER écrivain réussit (CAS garantit l'atomicité).

RESET à 0 dans TROIS cas (FIX-111 v10) :
  1. ack_irq : remaining==0 → fin de vague normale
  2. watchdog : hard reset → force reset
  3. JAMAIS dans dispatch_irq normal (resterait bloqué si reset entre deux ACKs)

INVARIANT : masked_since == 0 ⟺ pas de blocage en cours (pending_acks==0).
```

### Invariant IommuFaultQueue v10 (CAS strong, FIX-91 + FIX-104)

```
Push (ISR) — head avance SEULEMENT si compare_exchange (strong) réussit :
  pos = head.load(Relaxed)                  ← lecture, pas fetch_add
  Si slot[pos%CAP].seq != pos → dropped++ ; return false (queue pleine)
  CAS strong head : (pos → pos+1) ?         ← tentative atomique, sans spurious failure
  CAS échoue → dropped++ ; return false     ← pas d'orphaned slot (head non avancé)
  CAS réussit → écrire event, slot.seq.store(pos+1, Release)

GARANTIE : head n'avance QUE pour des pushes réussis → pas de slot orphelin.
GARANTIE : compare_exchange strong → pas de false drops sur ARM/RISC-V.

Pop (worker thread) :
  pos = tail.load(Relaxed)
  Si slot[pos%CAP].seq != pos+1 → return None
  event = lire slot ; slot.seq.store(pos+CAP, Release) ; tail++
  return Some(event)
```

### Invariant PCI Topology v10 (FIX-92/94 + FIX-105)

```
register() :
  irq_save OBLIGATOIRE avant write lock → prévient deadlock IRQ (pas NMI)
  CLI ne masque PAS les NMI (limitation documentée — FIX-105 v10)
  NMI handlers Phase 8 : NE DOIVENT PAS appeler parent_bridge()
  Phase 9+ : SeqLock pour NMI-safety complète

parent_bridge() retourne None → FIX-94 :
  wait_link_retraining applique sleep_ms(250) ("quarantaine aveugle")
  250ms > T_rhfa PCIe = bus stable garanti dans tous les cas réels

Ordre init device_server STRICT :
  1. scan_pci_bus()
  2. sys_pci_set_topology()    ← AVANT tout sys_irq_register
  3. sys_pci_claim(bdf=Some()) ← BDF dans DeviceClaim
  4. sys_irq_register(bdf=Some())
  5. match_and_spawn()
```

### Invariant dispatch_irq context (FIX-109 v10)

```
dispatch_irq s'exécute en CONTEXTE ISR (Ring 0, interrupt stack).

RÈGLES ISR ABSOLUES :
  ✓ lapic_send_eoi() → OK (écriture registre LAPIC)
  ✓ ioapic_mask/unmask → OK (écriture registre IOAPIC)
  ✓ AtomicXxx operations → OK
  ✓ core::hint::spin_loop() → OK (instruction PAUSE x86)
  ✓ log::error!/warn! → OK (ring buffer sans lock)
  ✓ ipc::send_irq_notification() → OK (enqueue non-bloquant)
  ✗ scheduler::yield_current_thread() → INTERDIT (corruption pile)
  ✗ blocking operations → INTERDIT
  ✗ heap allocation → INTERDIT

wait_link_retraining : THREAD context (do_exit) → yield OK.
iommu_fault_worker_tick : THREAD context (kernel worker) → yield OK.
```

### Invariant BOOT_TSC_KHZ (FIX-103 v10)

```
Type : AtomicU64 (non `static u64`)
Raison : Rust refuse l'assignation sur un static immutable → compile error.

WRITE : une seule fois dans calibrate_tsc_khz() — Ordering::Relaxed
READ  : dans current_time_ms() — Ordering::Relaxed
ORDRE : calibrate_tsc_khz() AVANT enable_interrupts()
        enable_interrupts() = barrière mémoire implicite = visibilité garantie sur tous CPUs.
```

---

## Annexe B — Matrice de tests v10

| Scénario | Composant | Condition | Résultat attendu |
|---|---|---|---|
| 64 fautes IOMMU simultanées | `iommu_fault_isr` | 64 CPUs push en même temps | Pas d'orphaned slot, dropped++ si queue pleine **FIX-91** |
| Blacklist + IRQ arrive encore | `dispatch_irq` | overflow_count=5, nouvelle IRQ Level | EOI LAPIC envoyé, return sans pending_acks **FIX-108 v10** |
| Blacklist Level - ISR bit LAPIC | `dispatch_irq` | overflow_count=5, Level IRQ | lapic_send_eoi() toujours → pas de gel système **FIX-108 v10** |
| Blacklist CAS multithreaded | `dispatch_irq` | 8 CPUs, CAS race, ov atteint 5 | 1 seul overflow_count++ par CAS winner, reset pending=0 **FIX-92** |
| Contention CAS extreme ISR | `dispatch_irq` | >SPIN_THRESHOLD tentatives | Drop propre de la vague, log warn → pas de yield **FIX-109 v10** |
| PciTopology irq_save | `pci_topology` | IRQ normale pendant write lock | Pas de deadlock (irq_save) **FIX-93** |
| Bootstrap race topology | `wait_link_retraining` | device_server crash avant set_topology | fallback sleep_ms(250) + Ok() retourné **FIX-94** |
| Polling driver crash | `do_exit` | GPU sans IRQ registered | bdf_of_pid trouve le BDF via DeviceClaim **FIX-95** |
| Serveur 300 devices SR-IOV | `sys_pci_set_topology` | 300 appels | Topology accepte (capacité 1024) **FIX-96** |
| Driver restart après crash | `sys_irq_register` | re-registration, overflow_count=4 | overflow_count reset à 0, handled_count reset si pending==0 **FIX-99 + FIX-112 v10** |
| Driver restart post-watchdog | `sys_irq_register` | watchdog hard reset, puis re-register | handled_count reset → ghost IRQ detection correcte **FIX-112 v10** |
| Driver restart après blacklist | `sys_irq_register` | IOAPIC masqué, pending_acks=0 | overflow_count + handled_count reset + IOAPIC unmasked **FIX-102 v8 + FIX-112** |
| wait_link_retraining timeout | `do_exit` | link ne revient pas en 200ms | force_disable_domain + log explicite + continuer **FIX-101** |
| masked_since reset fin de vague | `ack_irq` | remaining==0 | masked_since.store(0) → CAS 0→now correct au prochain storm **FIX-111 v10** |
| masked_since reset watchdog | `irq_watchdog` | hard reset | masked_since.store(0) → cohérent avec ack_irq **FIX-111 v10** |
| Queue init() Relaxed → Release | `fault_queue` | boot multi-CPU | Visibilité garantie entre CPUs **FIX-100** |
| BOOT_TSC_KHZ AtomicU64 | `time.rs` | compilation Rust | Pas de "cannot assign to immutable static" **FIX-103 v10** |
| compare_exchange weak→strong | `fault_queue` | ARM/RISC-V (portabilité) | Pas de spurious drops **FIX-104 v10** |
| ACK stale Edge storm | `ack_irq` | wave précédente, Edge | pending_acks décrémenté, pas de watchdog abusif **FIX-65 v7** |
| Ghost IRQ Level | `ack_irq` | tous NotMine | LAPIC EOI envoyé, IOAPIC masqué, notify **FIX-52 v6** |
| DMA write buffer malloc | `sys_dma_map` | page COW standard | COW résolu, mapping réussi **FIX-68 v7** |
| `sizeof(KmallocHeader)` | linux_shim | GCC + Clang | _Static_assert passe, offsetof(data)==32 **FIX-37 v5** |
| PCI BAR partie haute 64-bit | `bar_phys` | bar_phys(1) avec BAR0 64-bit | `PciError::IsHighPartOf64BitBar` **FIX-58 v6** |
| PCI device absent | `bar_phys` | raw==0xFFFF_FFFF | `PciError::DeviceAbsent` **FIX-75 v7** |
