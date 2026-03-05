# 📋 KERNEL EXO-OS — ARCHITECTURE v6
> Suppression de la preuve formelle Coq/TLA+
> Réorganisation du système de capability
> Nouvelles règles globales cohérentes
> Remplacement : invariants documentés + tests de propriétés + CI strict

---

## POURQUOI CETTE RÉVISION

La preuve formelle Coq/TLA+ constituait un frein majeur au développement.
Elle imposait des contraintes d'architecture qui avaient des répercussions
en cascade sur `ipc/`, `fs/`, `process/` et les règles transversales.

**Ce qui change :**
- `security/capability/` perd son statut de "périmètre TCB prouvé"
- `ipc/capability_bridge/` disparaît — c'était un shim créé pour la preuve
- `security/tcb/` avec `boundary.rs`, `invariants.rs` (annotés Coq) → supprimés
- `PROOF_SECURITY.md`, `PROOF_SCOPE.md` → supprimés
- `proofs/kernel_security/` (dépôt Coq) → supprimé
- `tools/exo-proof/` → supprimé

**Ce qui reste et est renforcé :**
- Le système de capability lui-même reste intact et robuste
- `security/capability/` reste la source de vérité unique
- `ipc/` et `fs/` appellent `security/capability/` **directement** (plus de bridge)
- Les invariants sont documentés dans `INVARIANTS.md` + couverts par `proptest`
- Un check CI garantit qu'aucun module ne bypasse `security/capability/`

---

## MODÈLE DE SÉCURITÉ POST-SUPPRESSION PREUVE

```
AVANT (avec preuve) :
  ipc/ → ipc/capability_bridge/ → security/capability/verify()
  fs/  → security/capability/verify() (appel direct)
  Périmètre Coq : model.rs + token.rs + rights.rs + revocation.rs + delegation.rs

APRÈS (sans preuve) :
  ipc/ → security/capability/verify()   (appel direct, bridge supprimé)
  fs/  → security/capability/verify()   (inchangé)
  process/ → security/capability/verify() (inchangé)
  Assurance : INVARIANTS.md + proptest + CI grep + audit log
```

Le niveau de sécurité réel est identique.
La différence est que la garantie est désormais testée plutôt que prouvée.

---

## ARBORESCENCE KERNEL v6 — COMPLÈTE

```
kernel/
├── Cargo.toml
├── build.rs
├── linker/
│   ├── x86_64.ld
│   ├── aarch64.ld
│   └── sections.ld
│
└── src/
    ├── main.rs
    ├── lib.rs
    │
    ├── arch/                               # ⚙️ COUCHE ARCHITECTURE
    │   ├── mod.rs
    │   ├── x86_64/
    │   │   ├── mod.rs
    │   │   ├── boot/
    │   │   │   ├── multiboot2.rs
    │   │   │   ├── uefi.rs
    │   │   │   ├── early_init.rs
    │   │   │   └── trampoline.s
    │   │   ├── cpu/
    │   │   │   ├── features.rs
    │   │   │   ├── msr.rs
    │   │   │   ├── fpu.rs                  # Instructions ASM brutes : XSAVE/XRSTOR
    │   │   │   ├── tsc.rs
    │   │   │   └── topology.rs
    │   │   ├── gdt.rs
    │   │   ├── idt.rs
    │   │   ├── tss.rs
    │   │   ├── paging.rs
    │   │   ├── syscall.rs
    │   │   ├── exceptions.rs
    │   │   ├── apic/
    │   │   │   ├── local_apic.rs
    │   │   │   ├── io_apic.rs
    │   │   │   ├── x2apic.rs
    │   │   │   └── ipi.rs
    │   │   ├── acpi/
    │   │   │   ├── parser.rs
    │   │   │   ├── madt.rs
    │   │   │   ├── hpet.rs
    │   │   │   └── pm_timer.rs
    │   │   ├── smp/
    │   │   │   ├── init.rs
    │   │   │   ├── percpu.rs
    │   │   │   └── hotplug.rs
    │   │   ├── spectre/
    │   │   │   ├── kpti.rs
    │   │   │   ├── retpoline.rs
    │   │   │   ├── ssbd.rs
    │   │   │   └── ibrs.rs
    │   │   └── virt/
    │   │       ├── detect.rs
    │   │       ├── paravirt.rs
    │   │       └── stolen_time.rs
    │   └── aarch64/
    │       └── mod.rs
    │
    ├── memory/                             # 🟣 COUCHE 0 — dépend de RIEN
    │   ├── mod.rs
    │   ├── core/
    │   │   ├── mod.rs
    │   │   ├── types.rs
    │   │   ├── address.rs
    │   │   ├── layout.rs
    │   │   └── constants.rs
    │   ├── physical/
    │   │   ├── mod.rs
    │   │   ├── allocator/
    │   │   │   ├── mod.rs
    │   │   │   ├── buddy.rs
    │   │   │   ├── slab.rs
    │   │   │   ├── slub.rs
    │   │   │   ├── bitmap.rs
    │   │   │   ├── numa_aware.rs
    │   │   │   └── ai_hints.rs             # Lookup table statique NUMA
    │   │   ├── frame/
    │   │   │   ├── mod.rs
    │   │   │   ├── descriptor.rs
    │   │   │   ├── ref_count.rs
    │   │   │   ├── pool.rs                 # Per-CPU pools + EmergencyPool
    │   │   │   └── reclaim.rs
    │   │   ├── zone/
    │   │   │   ├── mod.rs
    │   │   │   ├── dma.rs
    │   │   │   ├── dma32.rs
    │   │   │   ├── normal.rs
    │   │   │   ├── high.rs
    │   │   │   └── movable.rs
    │   │   └── numa/
    │   │       ├── mod.rs
    │   │       ├── node.rs
    │   │       ├── distance.rs
    │   │       ├── policy.rs
    │   │       └── migration.rs
    │   ├── virtual/
    │   │   ├── mod.rs
    │   │   ├── address_space/
    │   │   │   ├── mod.rs
    │   │   │   ├── kernel.rs
    │   │   │   ├── user.rs
    │   │   │   ├── mapper.rs
    │   │   │   └── tlb.rs
    │   │   ├── page_table/
    │   │   │   ├── mod.rs
    │   │   │   ├── x86_64.rs
    │   │   │   ├── walker.rs
    │   │   │   ├── builder.rs
    │   │   │   └── kpti_split.rs
    │   │   ├── vma/
    │   │   │   ├── mod.rs
    │   │   │   ├── descriptor.rs
    │   │   │   ├── tree.rs
    │   │   │   ├── operations.rs
    │   │   │   └── cow.rs
    │   │   └── fault/
    │   │       ├── mod.rs
    │   │       ├── handler.rs
    │   │       ├── cow.rs
    │   │       ├── demand_paging.rs
    │   │       └── swap_in.rs
    │   ├── heap/
    │   │   ├── mod.rs
    │   │   ├── allocator/
    │   │   │   ├── mod.rs
    │   │   │   ├── hybrid.rs
    │   │   │   ├── size_classes.rs
    │   │   │   └── global.rs
    │   │   ├── thread_local/
    │   │   │   ├── mod.rs
    │   │   │   ├── cache.rs
    │   │   │   ├── magazine.rs
    │   │   │   └── drain.rs
    │   │   └── large/
    │   │       ├── mod.rs
    │   │       └── vmalloc.rs
    │   ├── dma/
    │   │   ├── mod.rs
    │   │   ├── core/
    │   │   │   ├── mod.rs
    │   │   │   ├── types.rs
    │   │   │   ├── descriptor.rs
    │   │   │   ├── mapping.rs
    │   │   │   └── error.rs
    │   │   ├── iommu/
    │   │   │   ├── mod.rs
    │   │   │   ├── intel_vtd.rs
    │   │   │   ├── amd_iommu.rs
    │   │   │   ├── arm_smmu.rs
    │   │   │   ├── domain.rs
    │   │   │   └── page_table.rs
    │   │   ├── channels/
    │   │   │   ├── mod.rs
    │   │   │   ├── manager.rs
    │   │   │   ├── channel.rs
    │   │   │   ├── priority.rs
    │   │   │   └── affinity.rs
    │   │   ├── engines/
    │   │   │   ├── mod.rs
    │   │   │   ├── ioat.rs
    │   │   │   ├── idxd.rs
    │   │   │   ├── ahci_dma.rs
    │   │   │   ├── nvme_dma.rs
    │   │   │   └── virtio_dma.rs
    │   │   ├── ops/
    │   │   │   ├── mod.rs
    │   │   │   ├── memcpy.rs
    │   │   │   ├── memset.rs
    │   │   │   ├── scatter_gather.rs
    │   │   │   ├── cyclic.rs
    │   │   │   └── interleaved.rs
    │   │   ├── completion/
    │   │   │   ├── mod.rs
    │   │   │   ├── handler.rs
    │   │   │   ├── polling.rs
    │   │   │   └── wakeup.rs               # Via trait DmaWakeupHandler
    │   │   └── stats/
    │   │       ├── mod.rs
    │   │       └── counters.rs
    │   ├── swap/
    │   │   ├── mod.rs
    │   │   ├── backend.rs
    │   │   ├── policy.rs
    │   │   ├── compress.rs
    │   │   └── cluster.rs
    │   ├── cow/
    │   │   ├── mod.rs
    │   │   ├── tracker.rs
    │   │   └── breaker.rs
    │   ├── huge_pages/
    │   │   ├── mod.rs
    │   │   ├── thp.rs
    │   │   ├── hugetlbfs.rs
    │   │   └── split.rs
    │   ├── protection/
    │   │   ├── mod.rs
    │   │   ├── nx.rs
    │   │   ├── smep.rs
    │   │   ├── smap.rs
    │   │   └── pku.rs
    │   ├── integrity/
    │   │   ├── mod.rs
    │   │   ├── canary.rs
    │   │   ├── guard_pages.rs
    │   │   └── sanitizer.rs
    │   └── utils/
    │       ├── mod.rs
    │       ├── futex_table.rs
    │       ├── oom_killer.rs
    │       └── shrinker.rs
    │
    ├── scheduler/                          # 🔵 COUCHE 1 — → memory/ uniquement
    │   ├── mod.rs
    │   ├── core/
    │   │   ├── mod.rs
    │   │   ├── task.rs
    │   │   ├── runqueue.rs
    │   │   ├── pick_next.rs
    │   │   ├── switch.rs
    │   │   └── preempt.rs
    │   ├── asm/
    │   │   ├── switch_asm.s                # r15 garanti préservé — callee-saved
    │   │   └── fast_path.s
    │   ├── policies/
    │   │   ├── mod.rs
    │   │   ├── cfs.rs
    │   │   ├── realtime.rs
    │   │   ├── deadline.rs
    │   │   ├── idle.rs
    │   │   └── ai_guided.rs
    │   ├── smp/
    │   │   ├── mod.rs
    │   │   ├── load_balance.rs
    │   │   ├── migration.rs
    │   │   ├── affinity.rs
    │   │   └── topology.rs
    │   ├── sync/
    │   │   ├── mod.rs
    │   │   ├── wait_queue.rs
    │   │   ├── mutex.rs
    │   │   ├── rwlock.rs
    │   │   ├── spinlock.rs
    │   │   ├── condvar.rs
    │   │   └── barrier.rs
    │   ├── timer/
    │   │   ├── mod.rs
    │   │   ├── hrtimer.rs
    │   │   ├── tick.rs
    │   │   ├── clock.rs
    │   │   └── deadline_timer.rs
    │   ├── fpu/
    │   │   ├── mod.rs
    │   │   ├── lazy.rs
    │   │   ├── save_restore.rs
    │   │   └── state.rs
    │   ├── energy/
    │   │   ├── mod.rs
    │   │   ├── c_states.rs
    │   │   ├── frequency.rs
    │   │   └── power_profile.rs
    │   └── stats/
    │       ├── mod.rs
    │       ├── per_cpu.rs
    │       └── latency.rs
    │
    ├── security/                           # 🔐 SÉCURITÉ NOYAU (sans preuve formelle)
    │   ├── mod.rs
    │   │
    │   │   # ✅ SUPPRIMÉ : PROOF_SECURITY.md     (plus de preuve Coq)
    │   │   # ✅ SUPPRIMÉ : tcb/boundary.rs        (périmètre TCB formel)
    │   │   # ✅ SUPPRIMÉ : tcb/invariants.rs      (invariants annotés Coq)
    │   │   # ✅ SUPPRIMÉ : tcb/ dossier entier
    │   │
    │   ├── INVARIANTS.md                   # ← NOUVEAU — remplace PROOF_SECURITY.md
    │   │                                   # Invariants documentés + couverts par proptest
    │   │                                   # Voir section INVARIANTS ci-dessous
    │   │
    │   ├── capability/                     # 🔑 SOURCE UNIQUE DE VÉRITÉ — accès direct
    │   │   ├── mod.rs
    │   │   │
    │   │   │   # ✅ SUPPRIMÉ : PROOF_SCOPE.md     (délimitation périmètre Coq)
    │   │   │   # ✅ SUPPRIMÉ : model.rs            (modèle formel Coq — fusionné dans token.rs)
    │   │   │   # Structure et comportement conservés, annotation Coq retirée
    │   │   │
    │   │   ├── token.rs                    # CapToken (128 bits) + layout documenté
    │   │   │                               # Ancien model.rs fusionné ici
    │   │   ├── rights.rs                   # Rights : READ|WRITE|EXEC|GRANT|REVOKE|DELEGATE
    │   │   ├── table.rs                    # CapTable par processus (radix tree)
    │   │   ├── verify.rs                   # verify() — POINT D'ENTRÉE UNIQUE dans tout l'OS
    │   │   │                               # ← Extrait de revocation.rs pour clarté
    │   │   │                               # ipc/, fs/, process/ appellent TOUS cette fonction
    │   │   ├── revocation.rs               # revoke() O(1) par génération++
    │   │   ├── delegation.rs               # delegate() — sous-ensemble strict de droits
    │   │   └── namespace.rs                # Namespace capability (isolation)
    │   │
    │   ├── access_control/                 # ← NOUVEAU — remplace le bridge ipc/
    │   │   ├── mod.rs                      # Contrôle d'accès unifié pour tous les modules
    │   │   ├── checker.rs                  # check_access(token, object, rights) → Result
    │   │   │                               # Appelle security::capability::verify() en interne
    │   │   │                               # Ajoute : logging audit + contexte d'erreur riche
    │   │   └── object_types.rs             # ObjectKind : Channel | File | ShmRegion | Process
    │   │                                   # Chaque kind a ses droits spécifiques définis ici
    │   │
    │   ├── zero_trust/
    │   │   ├── mod.rs
    │   │   ├── policy.rs
    │   │   ├── context.rs
    │   │   ├── verify.rs
    │   │   └── labels.rs
    │   │
    │   ├── crypto/
    │   │   ├── mod.rs
    │   │   ├── xchacha20_poly1305.rs
    │   │   ├── blake3.rs
    │   │   ├── x25519.rs
    │   │   ├── ed25519.rs
    │   │   ├── rng.rs
    │   │   ├── kdf.rs
    │   │   └── aes_gcm.rs
    │   │
    │   ├── isolation/
    │   │   ├── mod.rs
    │   │   ├── domains.rs
    │   │   ├── namespaces.rs
    │   │   ├── sandbox.rs
    │   │   └── pledge.rs
    │   │
    │   ├── integrity_check/
    │   │   ├── mod.rs
    │   │   ├── code_signing.rs
    │   │   ├── runtime_check.rs
    │   │   └── secure_boot.rs
    │   │
    │   ├── exploit_mitigations/
    │   │   ├── mod.rs
    │   │   ├── kaslr.rs
    │   │   ├── stack_protector.rs
    │   │   ├── cfg.rs
    │   │   ├── cet.rs
    │   │   └── safe_stack.rs
    │   │
    │   └── audit/
    │       ├── mod.rs
    │       ├── logger.rs
    │       ├── rules.rs
    │       └── syscall_audit.rs
    │
    ├── process/                            # 🟤 COUCHE 1.5
    │   ├── mod.rs
    │   ├── core/
    │   │   ├── mod.rs
    │   │   ├── pid.rs
    │   │   ├── pcb.rs
    │   │   ├── tcb.rs
    │   │   └── registry.rs
    │   ├── lifecycle/
    │   │   ├── mod.rs
    │   │   ├── create.rs
    │   │   ├── fork.rs
    │   │   ├── exec.rs
    │   │   ├── exit.rs
    │   │   ├── wait.rs
    │   │   └── reap.rs
    │   ├── thread/
    │   │   ├── mod.rs
    │   │   ├── creation.rs
    │   │   ├── join.rs
    │   │   ├── detach.rs
    │   │   ├── local_storage.rs
    │   │   └── pthread_compat.rs
    │   ├── signal/
    │   │   ├── mod.rs
    │   │   ├── delivery.rs
    │   │   ├── handler.rs
    │   │   ├── mask.rs
    │   │   ├── queue.rs
    │   │   └── default.rs
    │   ├── state/
    │   │   ├── mod.rs
    │   │   ├── transitions.rs
    │   │   └── wakeup.rs
    │   ├── group/
    │   │   ├── mod.rs
    │   │   ├── session.rs
    │   │   ├── pgrp.rs
    │   │   └── job_control.rs
    │   ├── namespace/
    │   │   ├── mod.rs
    │   │   ├── pid_ns.rs
    │   │   ├── mount_ns.rs
    │   │   ├── net_ns.rs
    │   │   ├── uts_ns.rs
    │   │   └── user_ns.rs
    │   └── resource/
    │       ├── mod.rs
    │       ├── rlimit.rs
    │       ├── usage.rs
    │       └── cgroup.rs
    │
    ├── ipc/                                # 🟢 COUCHE 2a — → memory/ + scheduler/ + security/
    │   ├── mod.rs
    │   │
    │   │   # ✅ SUPPRIMÉ : ipc/capability_bridge/   (shim pour la preuve — plus nécessaire)
    │   │   # ipc/ appelle security::capability::verify() DIRECTEMENT
    │   │   # via security::access_control::checker::check_access()
    │   │
    │   ├── core/
    │   │   ├── mod.rs
    │   │   ├── types.rs
    │   │   ├── fastcall_asm.s
    │   │   ├── transfer.rs
    │   │   ├── sequence.rs
    │   │   └── constants.rs
    │   ├── channel/
    │   │   ├── mod.rs
    │   │   ├── sync.rs
    │   │   ├── async.rs
    │   │   ├── mpmc.rs
    │   │   ├── broadcast.rs
    │   │   ├── typed.rs
    │   │   └── streaming.rs
    │   ├── ring/
    │   │   ├── mod.rs
    │   │   ├── spsc.rs
    │   │   ├── mpmc.rs
    │   │   ├── fusion.rs
    │   │   ├── slot.rs
    │   │   ├── batch.rs
    │   │   └── zerocopy.rs
    │   ├── shared_memory/
    │   │   ├── mod.rs
    │   │   ├── pool.rs
    │   │   ├── mapping.rs
    │   │   ├── page.rs
    │   │   ├── descriptor.rs
    │   │   ├── allocator.rs
    │   │   └── numa_aware.rs
    │   ├── endpoint/
    │   │   ├── mod.rs
    │   │   ├── descriptor.rs
    │   │   ├── registry.rs
    │   │   ├── connection.rs
    │   │   └── lifecycle.rs
    │   ├── sync/
    │   │   ├── mod.rs
    │   │   ├── futex.rs
    │   │   ├── wait_queue.rs
    │   │   ├── event.rs
    │   │   ├── barrier.rs
    │   │   └── rendezvous.rs
    │   ├── message/
    │   │   ├── mod.rs
    │   │   ├── builder.rs
    │   │   ├── serializer.rs
    │   │   ├── router.rs
    │   │   └── priority.rs
    │   ├── rpc/
    │   │   ├── mod.rs
    │   │   ├── server.rs
    │   │   ├── client.rs
    │   │   ├── protocol.rs
    │   │   └── timeout.rs
    │   └── stats/
    │       ├── mod.rs
    │       └── counters.rs
    │
    ├── fs/                                 # 🟠 COUCHE 3
    │   ├── mod.rs
    │   ├── core/
    │   │   ├── mod.rs
    │   │   ├── vfs.rs
    │   │   ├── inode.rs
    │   │   ├── dentry.rs
    │   │   ├── descriptor.rs
    │   │   └── types.rs
    │   ├── io/
    │   │   ├── mod.rs
    │   │   ├── uring.rs
    │   │   ├── zero_copy.rs
    │   │   ├── aio.rs
    │   │   ├── mmap.rs
    │   │   ├── direct_io.rs
    │   │   └── completion.rs
    │   ├── cache/
    │   │   ├── mod.rs
    │   │   ├── page_cache.rs
    │   │   ├── dentry_cache.rs
    │   │   ├── inode_cache.rs
    │   │   ├── buffer.rs
    │   │   ├── prefetch.rs
    │   │   └── eviction.rs
    │   ├── integrity/
    │   │   ├── mod.rs
    │   │   ├── checksum.rs
    │   │   ├── journal.rs
    │   │   ├── recovery.rs
    │   │   ├── scrubbing.rs
    │   │   ├── healing.rs
    │   │   └── validator.rs
    │   ├── ext4plus/
    │   │   ├── mod.rs
    │   │   ├── superblock.rs
    │   │   ├── group_desc.rs
    │   │   ├── inode/
    │   │   │   ├── ops.rs
    │   │   │   ├── extent.rs
    │   │   │   ├── xattr.rs
    │   │   │   └── acl.rs
    │   │   ├── directory/
    │   │   │   ├── htree.rs
    │   │   │   ├── linear.rs
    │   │   │   └── ops.rs
    │   │   └── allocation/
    │   │       ├── balloc.rs
    │   │       ├── mballoc.rs
    │   │       └── prealloc.rs
    │   ├── pseudo/
    │   │   ├── mod.rs
    │   │   ├── procfs.rs
    │   │   ├── sysfs.rs
    │   │   ├── devfs.rs
    │   │   └── tmpfs.rs
    │   ├── ipc_fs/
    │   │   ├── mod.rs
    │   │   ├── pipefs.rs
    │   │   └── socketfs.rs
    │   ├── block/
    │   │   ├── mod.rs
    │   │   ├── device.rs
    │   │   ├── scheduler.rs
    │   │   ├── queue.rs
    │   │   └── bio.rs
    │   └── compatibility/
    │       ├── mod.rs
    │       ├── posix.rs
    │       └── linux_compat.rs
    │
    └── syscall/                            # 🔵 INTERFACE SYSCALL
        ├── mod.rs
        ├── table.rs
        ├── entry.s
        ├── dispatch.rs
        ├── validation.rs
        ├── fast_path.rs
        ├── compat/
        │   ├── linux.rs
        │   └── posix.rs
        └── numbers.rs
```

---

## CE QUI EST SUPPRIMÉ — LISTE EXHAUSTIVE

```
FICHIERS SUPPRIMÉS (preuve formelle) :
  kernel/src/security/PROOF_SECURITY.md
  kernel/src/security/tcb/                     ← dossier entier
  kernel/src/security/tcb/mod.rs
  kernel/src/security/tcb/boundary.rs
  kernel/src/security/tcb/invariants.rs        ← annotations Coq
  kernel/src/security/tcb/audit.rs             ← remplacé par security/audit/
  kernel/src/security/capability/PROOF_SCOPE.md
  kernel/src/security/capability/model.rs      ← fusionné dans token.rs
  kernel/src/ipc/capability_bridge/            ← dossier entier
  kernel/src/ipc/capability_bridge/mod.rs
  kernel/src/ipc/capability_bridge/check.rs

DOSSIERS SUPPRIMÉS (hors kernel) :
  proofs/kernel_security/                      ← preuves Coq
  tools/exo-proof/                             ← outillage Coq

AJOUTS EN REMPLACEMENT :
  kernel/src/security/INVARIANTS.md            ← documentation invariants
  kernel/src/security/access_control/          ← accès direct unifié
  kernel/src/security/capability/verify.rs     ← extrait pour clarté
  tests/invariants/capability_proptest.rs      ← tests de propriétés
```

---

## SECURITY/INVARIANTS.md — CONTENU

```markdown
# INVARIANTS.md — security/capability/
# Remplace la preuve formelle Coq/TLA+
# Assurance : documentation + proptest + CI

## INV-1 : Sûreté capability
  verify(token, required) = Ok ⟹
    token.object_id existe dans la table
    ∧ token.generation == table[token.object_id].generation
    ∧ token.rights.contains(required)
  Vérifié par : proptest (1000 cas aléatoires), voir tests/invariants/

## INV-2 : Révocation instantanée
  revoke(object_id) ; verify(token_with_same_id, _) = Err(Revoked)
  Propriété : génération incrémentée → tout token existant invalide
  Vérifié par : proptest + test unitaire exhaustif

## INV-3 : Confinement délégation
  delegate(t1) = t2 ⟹ t2.rights ⊆ t1.rights
  Implémentation : t2.rights = t1.rights & requested_rights (AND binaire)
  AND binaire est prouvé correct par construction — impossible de produire
  des droits hors du sous-ensemble par une opération AND
  Vérifié par : test exhaustif sur les 2^6 = 64 combinaisons de droits

## INV-4 : Unicité du point de vérification
  Aucun module ne peut contourner security::capability::verify()
  Vérifié par : CI grep — voir section CI ci-dessous

## INV-5 : Génération strictement croissante
  revoke() → génération++
  La génération n'est jamais décrémentée
  Implémentation : fetch_add(1, Release) — atomique, pas de wrap (u32)
  Vérifié par : test unitaire + proptest
```

---

## SECURITY/ACCESS_CONTROL/ — REMPLACE LE BRIDGE

Le `capability_bridge/` dans `ipc/` existait uniquement pour limiter le périmètre
de preuve Coq à un minimum. Sans preuve, il n'a plus de raison d'être.

À sa place, `security/access_control/` centralise le contrôle d'accès pour
**tous** les modules (`ipc/`, `fs/`, `process/`) avec un contexte d'erreur riche
et le logging audit intégré.

```rust
// kernel/src/security/access_control/checker.rs
//
// Point d'entrée unifié pour TOUTES les vérifications d'accès.
// Appelé directement par ipc/, fs/, process/.
// Remplace : ipc/capability_bridge/check.rs (supprimé)

use crate::security::capability::{verify, CapTable, CapToken, Rights, CapError};
use crate::security::audit;

/// Résultat enrichi — plus de contexte qu'un simple CapError
#[derive(Debug)]
pub enum AccessError {
    CapabilityDenied { reason: CapError, object: ObjectKind, module: &'static str },
    ObjectNotFound   { object: ObjectKind },
    InsufficientRights { had: Rights, needed: Rights },
}

/// Vérification d'accès — appelé par ipc/, fs/, process/
/// Log automatique dans security/audit/ si échec
pub fn check_access(
    table:    &CapTable,
    token:    CapToken,
    object:   ObjectKind,
    required: Rights,
    caller:   &'static str,   // "ipc::channel", "fs::vfs", "process::lifecycle"
) -> Result<(), AccessError> {
    match verify(table, token, required) {
        Ok(()) => {
            // Succès : log optionnel (verbose audit uniquement)
            audit::logger::log_access_ok(caller, object, token.object_id());
            Ok(())
        }
        Err(e) => {
            // Échec : toujours loggué
            audit::logger::log_access_denied(caller, object, token.object_id(), &e);
            Err(AccessError::CapabilityDenied {
                reason: e,
                object,
                module: caller,
            })
        }
    }
}
```

```rust
// kernel/src/security/access_control/object_types.rs
// Définit les kinds d'objets et leurs droits attendus

#[derive(Debug, Clone, Copy)]
pub enum ObjectKind {
    IpcChannel,      // ipc/channel/ — Rights: READ (recv) | WRITE (send)
    IpcEndpoint,     // ipc/endpoint/ — Rights: EXEC (connect) | WRITE (publish)
    ShmRegion,       // ipc/shared_memory/ — Rights: READ | WRITE
    File,            // fs/core/ — Rights: READ | WRITE | EXEC
    Directory,       // fs/core/ — Rights: READ (list) | WRITE (create/delete)
    Process,         // process/ — Rights: WRITE (signal) | EXEC (ptrace)
    CryptoKey,       // security/crypto/ — Rights: EXEC (use key)
}
```

---

## NOUVEAU FLUX D'ACCÈS — SANS BRIDGE

### IPC (avant et après)

```rust
// AVANT (avec bridge) :
// ipc/channel/sync.rs
use crate::ipc::capability_bridge::check::check_ipc_right;  // ← shim
check_ipc_right(&table, token, endpoint_id, Rights::WRITE)?;
//   ↓ bridge appelait
// security::capability::verify(table, token, Rights::WRITE)

// APRÈS (direct) :
// ipc/channel/sync.rs
use crate::security::access_control::checker::check_access;
use crate::security::access_control::object_types::ObjectKind;
check_access(&table, token, ObjectKind::IpcChannel, Rights::WRITE, "ipc::channel")?;
//   ↓ check_access appelle
// security::capability::verify(table, token, Rights::WRITE) + audit log
```

### FS (inchangé — appelait déjà directement)

```rust
// fs/core/vfs.rs — INCHANGÉ
use crate::security::access_control::checker::check_access;
check_access(&table, token, ObjectKind::File, Rights::READ, "fs::vfs")?;
```

### Process (inchangé — appelait déjà directement)

```rust
// process/lifecycle/exec.rs — INCHANGÉ
use crate::security::access_control::checker::check_access;
check_access(&table, token, ObjectKind::File, Rights::EXEC, "process::exec")?;
```

---

## RÈGLES MISES À JOUR — TOUS LES MODULES

### SECURITY/ — Nouvelles règles v6

```
┌────────────────────────────────────────────────────────────────┐
│ RÈGLES ABSOLUES — security/ (v6, sans preuve formelle)         │
├────────────────────────────────────────────────────────────────┤
│ SEC-01 │ capability/verify.rs = point d'entrée UNIQUE          │
│          │ Toute vérification dans tout l'OS passe par là      │
│          │ (plus de distinction périmètre TCB / hors périmètre) │
│ SEC-02 │ access_control/checker.rs = façade unifiée            │
│          │ ipc/, fs/, process/ appellent check_access()         │
│          │ check_access() appelle verify() + audit log          │
│ SEC-03 │ Révocation = O(1) génération++ (jamais parcours)       │
│ SEC-04 │ Délégation = AND binaire des droits (sous-ensemble     │
│          │ garanti par construction)                            │
│ SEC-05 │ XChaCha20 sur TOUS les canaux inter-domaines          │
│ SEC-06 │ KASLR actif                                           │
│ SEC-07 │ Retpoline sur tous les appels indirects hot path       │
│ SEC-08 │ SSBD per-thread, switché avec le contexte             │
│ SEC-09 │ Audit log : ring buffer non-bloquant, tamper-proof    │
│ SEC-10 │ Invariants couverts par proptest (1000+ cas)          │
│          │ (remplace la preuve formelle Coq)                    │
├────────────────────────────────────────────────────────────────┤
│ SUPPRIMÉES (obsolètes sans preuve) :                           │
│ ✗ SEC-10 ancien : "Périmètre preuve ≤ 500 lignes"             │
│ ✗ SEC-02 ancien : "ipc/capability_bridge/ shim"               │
├────────────────────────────────────────────────────────────────┤
│ INTERDITS                                                      │
├────────────────────────────────────────────────────────────────┤
│ ✗  Dupliquer verify() dans un autre module                     │
│ ✗  Appeler security::capability::verify() sans passer par      │
│    access_control::check_access() (sauf dans capability/ lui-  │
│    même)                                                       │
│ ✗  Délégation sans AND binaire des droits                      │
│ ✗  Canal inter-domaines sans XChaCha20                         │
│ ✗  Modifier capability/verify.rs sans MAJ INVARIANTS.md        │
└────────────────────────────────────────────────────────────────┘
```

### IPC/ — Règles mises à jour v6

```
┌────────────────────────────────────────────────────────────────┐
│ RÈGLES ABSOLUES — ipc/ (v6)                                    │
├────────────────────────────────────────────────────────────────┤
│ IPC-01 │ SPSC ring : head et tail sur cache lines SÉPARÉES     │
│ IPC-02 │ futex.rs = délégation à memory/utils/futex_table      │
│ IPC-03 │ Pages SHM = NO_COW + SHM_PINNED (les deux)           │
│ IPC-04 │ Vérification capability = check_access() de           │
│          │ security/access_control/ — APPEL DIRECT             │
│          │ ← remplace IPC-04 ancien ("capability_bridge/")     │
│ IPC-05 │ ipc/ N'APPELLE PAS fs/ directement                   │
│ IPC-06 │ Fusion Ring : anti-thundering herd                    │
│ IPC-07 │ Fast IPC = fichier .s ASM                            │
│ IPC-08 │ Spectre v1 : array_index_nospec() sur accès buffers   │
├────────────────────────────────────────────────────────────────┤
│ SUPPRIMÉES :                                                   │
│ ✗ IPC-04 ancien : "capability_bridge/ délègue à security/"    │
├────────────────────────────────────────────────────────────────┤
│ INTERDITS                                                      │
├────────────────────────────────────────────────────────────────┤
│ ✗  Table futex locale dans ipc/                                │
│ ✗  Logique capability propre dans ipc/ (→ security/access_ctrl)│
│ ✗  Pages SHM sans NO_COW + SHM_PINNED                         │
│ ✗  Dépendance directe sur fs/                                  │
│ ✗  SPSC sans CachePadded (false sharing)                       │
│ ✗  import ipc::capability_bridge (supprimé)                    │
└────────────────────────────────────────────────────────────────┘
```

### FS/ — Règles mises à jour v6

```
┌────────────────────────────────────────────────────────────────┐
│ RÈGLES ABSOLUES — fs/ (v6)                                     │
├────────────────────────────────────────────────────────────────┤
│ FS-01 │ Relâcher lock inode AVANT sleep (release-before-sleep) │
│ FS-02 │ io_uring : EINTR propre (IORING_OP_ASYNC_CANCEL)      │
│ FS-03 │ IPC via shim UNIQUEMENT (fs/ipc_fs/)                  │
│ FS-04 │ Capabilities : check_access() de security/access_ctrl/ │
│         │ (comportement inchangé, même destination finale)      │
│ FS-05 │ Thundering herd : completion callbacks sélectifs       │
│ FS-06 │ ElfLoader trait enregistré par fs/ pour process/exec   │
│ FS-07 │ Slab shrinker → memory/utils/shrinker.rs              │
│ FS-08 │ Blake3 checksums sur toutes les écritures ext4+        │
│ FS-09 │ WAL avant toute modification méta                      │
├────────────────────────────────────────────────────────────────┤
│ INTERDITS                                                      │
├────────────────────────────────────────────────────────────────┤
│ ✗  Être appelé par scheduler/ ou memory/                       │
│ ✗  Tenir lock inode pendant sleep                              │
│ ✗  ipc/ importe fs/ directement                               │
│ ✗  Écrire sans WAL                                            │
└────────────────────────────────────────────────────────────────┘
```

---

## SÉQUENCE DE BOOT v6 — MISE À JOUR

```
SÉQUENCE DE BOOT EXO-OS v6

1.  arch::boot::early_init()
2.  arch::boot::parse_memory_map()
3.  memory::physical::frame::emergency_pool::init()     # EN PREMIER ABSOLU
4.  memory::physical::allocator::bitmap::bootstrap()
5.  memory::physical::allocator::buddy::init()
6.  memory::heap::allocator::global::init()
7.  memory::utils::futex_table::init()
8.  arch::x86_64::gdt::init()
9.  arch::x86_64::idt::init()
10. arch::x86_64::tss::init_with_ist_stacks()
11. arch::x86_64::apic::local_apic::init()
12. arch::x86_64::acpi::parser::init()
13. scheduler::core::init()
14. scheduler::fpu::save_restore::detect_xsave_size()
15. scheduler::timer::tick::init(HZ=1000)
16. scheduler::timer::hrtimer::init()
17. security::capability::init()                        # AVANT process/ — inchangé
18. security::access_control::checker::init()           # ← NOUVEAU (v6)
    # Initialise le sous-système access_control
    # Enregistre les ObjectKind et leurs droits associés
19. security::crypto::rng::init()
20. security::audit::logger::init()                     # ← REMONTÉ (était implicite)
    # Audit log actif dès maintenant — toutes les vérif sont loggées
21. process::core::registry::init()
22. process::state::wakeup::register_with_dma()
23. memory::dma::iommu::init()
24. fs::core::vfs::init()
25. fs::ext4plus::mount_root()
26. ipc::core::init()
27. security::exploit_mitigations::kaslr::verify()
28. arch::x86_64::smp::start_aps()
29. memory::physical::frame::pool::init_percpu()
30. memory::utils::oom_killer::start_thread()
31. process::lifecycle::create::spawn_pid1()

SUPPRIMÉ du boot :
  ✗ Tout appel à ipc::capability_bridge (supprimé)
  ✗ Chargement de preuves Coq (jamais exécuté en runtime de toute façon)

AJOUTÉ au boot :
  ✅ Step 18 : security::access_control::checker::init()
  ✅ Step 20 : security::audit::logger::init() (explicitement positionné)
```

---

## RÈGLES TRANSVERSALES v6 — MISE À JOUR

```
┌─────────────────────────────────────────────────────────────────┐
│ RÈGLES TRANSVERSALES v6 — applicables à TOUS les modules        │
├─────────────────────────────────────────────────────────────────┤
│ TRANS-01 │ Ordre couches : memory(0)→scheduler(1)→             │
│            │ process(1.5)→ipc(2a)→fs(3)                        │
│ TRANS-02 │ Dépendances circulaires → trait abstrait au boot     │
│            │ (DmaWakeupHandler, ElfLoader)                      │
│ TRANS-03 │ EmergencyPool initialisé EN PREMIER (step 3)         │
│ TRANS-04 │ FutexTable = UNIQUE dans memory/utils/              │
│ TRANS-05 │ capability::verify() = UNIQUE dans security/         │
│            │ Tous les modules passent par access_control/        │
│            │ ← remplace "délèguent via bridge"                  │
│ TRANS-06 │ Lock ordering = ordre croissant des IDs             │
│ TRANS-07 │ Hot path = zéro allocation, zéro sleep, zéro lock   │
│ TRANS-08 │ IRQ handlers = zéro allocation                       │
│ TRANS-09 │ RAII partout                                         │
│ TRANS-10 │ Signal au retour userspace uniquement                │
│ TRANS-11 │ TLB shootdown synchrone AVANT free_pages()          │
│ TRANS-12 │ DMA frame → DMA_PINNED jusqu'à ACK completion       │
│ TRANS-13 │ Pages IPC/SHM → NO_COW + SHM_PINNED                │
│ TRANS-14 │ Context switch : r15 + MXCSR + x87 FCW              │
│ TRANS-15 │ CR3 switché DANS switch_asm                         │
│ TRANS-16 │ Retpoline sur tous les appels indirects hot path     │
│ TRANS-17 │ Spectre v1 : array_index_nospec()                   │
│ TRANS-18 │ XChaCha20 sur tous canaux inter-domaines            │
│ TRANS-19 │ IA kernel = lookup table .rodata ou EMA O(1)        │
│ TRANS-20 │ Panic pour état corrompu, Err() pour récupérable     │
│ TRANS-21 │ Pas de preuve formelle — invariants dans             │
│            │ INVARIANTS.md + proptest + CI grep                 │
│            │ ← remplace TRANS-05 ancien (périmètre Coq)        │
├─────────────────────────────────────────────────────────────────┤
│ SUPPRIMÉES (obsolètes) :                                        │
│ ✗  "Périmètre Coq ≤ 500 lignes"                               │
│ ✗  "ipc/ passe par capability_bridge/"                         │
│ ✗  "security/tcb/ = périmètre de confiance formel"            │
└─────────────────────────────────────────────────────────────────┘
```

---

## CI STRICT — REMPLACE LA GARANTIE COQ

Sans preuve formelle, le CI devient la ligne de défense principale
pour garantir les invariants architecturaux.

```bash
#!/bin/bash
# scripts/ci_security_checks.sh
# Lance à chaque commit — échec = merge bloqué

set -e

echo "=== CHECK 1 : Aucun module ne bypasse security::capability::verify() ==="
# Toute vérification doit passer par access_control::check_access()
# ou être dans security/capability/ lui-même
DIRECT_VERIFY=$(grep -rn "capability::verify(" kernel/src/ \
    | grep -v "security/capability/" \
    | grep -v "security/access_control/" \
    | grep -v "#\[cfg(test)\]" \
    | grep -v "//")
if [ -n "$DIRECT_VERIFY" ]; then
    echo "ERREUR : appel direct à capability::verify() hors de security/"
    echo "$DIRECT_VERIFY"
    exit 1
fi

echo "=== CHECK 2 : ipc/capability_bridge/ ne doit plus exister ==="
if [ -d "kernel/src/ipc/capability_bridge" ]; then
    echo "ERREUR : ipc/capability_bridge/ existe encore — à supprimer"
    exit 1
fi

echo "=== CHECK 3 : security/tcb/ ne doit plus exister ==="
if [ -d "kernel/src/security/tcb" ]; then
    echo "ERREUR : security/tcb/ existe encore — à supprimer"
    exit 1
fi

echo "=== CHECK 4 : Aucune annotation Coq résiduelle ==="
COQ_REFS=$(grep -rn "PROOF_SCOPE\|Coq\|TLA+\|PROP-[0-9]" kernel/src/ \
    | grep -v "INVARIANTS.md" \
    | grep -v "//.*historique")
if [ -n "$COQ_REFS" ]; then
    echo "ERREUR : références Coq résiduelles trouvées"
    echo "$COQ_REFS"
    exit 1
fi

echo "=== CHECK 5 : Tests proptest présents et passent ==="
cargo test --test capability_invariants -- --nocapture

echo "=== TOUS LES CHECKS OK ==="
```

---

## TESTS PROPTEST — REMPLACENT LES PREUVES COQ

```rust
// tests/invariants/capability_proptest.rs
// Remplace les preuves Coq des propriétés INV-1 à INV-5

use proptest::prelude::*;
use crate::security::capability::{CapTable, CapToken, Rights, ObjectId, Generation};

proptest! {
    /// INV-2 : Révocation instantanée
    /// Quelle que soit la séquence create/revoke, un token révoqué est rejeté
    #[test]
    fn prop_revoked_token_always_fails(
        object_id in 0u64..u64::MAX,
        rights in 0u16..0x3F,
    ) {
        let mut table = CapTable::new();
        let token = table.create(ObjectId(object_id), Rights(rights));
        table.revoke(ObjectId(object_id));
        prop_assert!(
            crate::security::capability::verify(&table, token, Rights(rights)).is_err()
        );
    }

    /// INV-3 : Délégation = sous-ensemble strict
    #[test]
    fn prop_delegation_cannot_escalate(
        original_rights in 0u16..0x3F,
        requested_rights in 0u16..0x3F,
    ) {
        let mut table = CapTable::new();
        let id = ObjectId(42);
        let t1 = table.create(id, Rights(original_rights));
        let t2 = table.delegate(t1, Rights(requested_rights));
        // t2.rights DOIT être un sous-ensemble de t1.rights
        prop_assert_eq!(
            t2.rights().0 & !t1.rights().0,
            0,  // aucun bit dans t2 ne doit être absent de t1
            "délégation a accordé plus de droits que l'original"
        );
    }

    /// INV-5 : Génération strictement croissante
    #[test]
    fn prop_generation_monotone(revoke_count in 1u32..100) {
        let mut table = CapTable::new();
        let id = ObjectId(1);
        table.create(id, Rights::READ);
        let mut last_gen = 0u32;
        for _ in 0..revoke_count {
            let gen = table.generation_of(id);
            prop_assert!(gen > last_gen || gen == 0, "génération non monotone");
            last_gen = gen;
            table.revoke(id);
            table.create(id, Rights::READ);
        }
    }
}
```

---

## TABLEAU DE CORRESPONDANCE — ANCIEN → NOUVEAU

| Ancien (v5, avec preuve) | Nouveau (v6, sans preuve) | Statut |
|---|---|---|
| `security/tcb/boundary.rs` | Supprimé | ✅ |
| `security/tcb/invariants.rs` | `security/INVARIANTS.md` | ✅ |
| `security/tcb/audit.rs` | `security/audit/logger.rs` (existait déjà) | ✅ |
| `security/PROOF_SECURITY.md` | `security/INVARIANTS.md` | ✅ |
| `security/capability/PROOF_SCOPE.md` | Supprimé | ✅ |
| `security/capability/model.rs` | Fusionné dans `token.rs` | ✅ |
| `security/capability/revocation.rs` (+ verify) | `revocation.rs` + `verify.rs` séparé | ✅ |
| `ipc/capability_bridge/mod.rs` | Supprimé | ✅ |
| `ipc/capability_bridge/check.rs` | `security/access_control/checker.rs` | ✅ |
| `proofs/kernel_security/` | `tests/invariants/capability_proptest.rs` | ✅ |
| `tools/exo-proof/` | `scripts/ci_security_checks.sh` | ✅ |
| SEC-02 "capability_bridge shim" | SEC-02 "access_control façade directe" | ✅ |
| IPC-04 "capability_bridge" | IPC-04 "check_access() direct" | ✅ |
| TRANS-05 "bridge" | TRANS-05 "access_control/" | ✅ |
| Step 18 boot (absent) | Step 18 `access_control::init()` | ✅ |
| Coq prop PROP-1 à PROP-4 | `proptest` INV-1 à INV-5 | ✅ |

---

*Exo-OS — Architecture Kernel v6*
*Suppression preuve formelle · Système capability conservé et simplifié*
*DOC1-10 restent valides aux ajustements près de ce document*
