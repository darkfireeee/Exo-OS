# EXO-OS — Architecture Complète v7

> **Kernel Exokernel · Ring 0 + Ring 1 + Drivers · Spécification Finale**
>
> *v7 — Mars 2026 — 5ème cycle revue · 5 corrections · Spécification finale validée*

---

## Résumé des cycles de revue

| Version | Corrections | Validateurs | Statut |
|---------|-------------|-------------|--------|
| v1 | Fondations | — | Instable |
| v2 | ~18 | 6 IAs | Cohérente |
| v3 | ~25 | 6 IAs | Architecturalement saine |
| v4 | 14 | 6 IAs | Quasi-finale |
| v5 | 13 | 6 IAs | Ring 1 intégré |
| v6 | 6 | 6 IAs | Bugs x86\_64 bas-niveau corrigés (cr3, FPU) |
| **v7** | **5** | **5 IAs** | **Spécification finale — prête pour implémentation** |

---

## Changelog v7

| ID | Correction | Source | Gravité |
|----|-----------|--------|---------|
| **V7-C-01** | `BootInfo` : passée par **adresse virtuelle** (kernel mappe la page dans la VMA de `init_server` avant son lancement) | Gemini | 🔴 `#PF` garanti |
| **V7-C-02** | `switch_asm.s` : **suppression de `MXCSR + FCW`** — avec Lazy FPU, le context switch ne touche jamais la FPU ; seul `CR0.TS=1` est positionné | Gemini | 🔴 Contradiction + corruption |
| **V7-C-03** | `switch.rs` : mise à jour de **`TSS.RSP0`** obligatoire à chaque context switch (sinon prochaine interruption Ring 3→0 empile sur la pile du thread précédent) | Gemini | 🔴 Corruption pile kernel |
| **V7-C-04** | `exec()` : clarification texte — *mask hérité du caller + pending signals flushed (comportement ExoOS explicite, pas POSIX strict)* | MiniMax/Copilot | ⚠️ Ambiguïté doc |
| **V7-C-05** | `MAX_CORES_RUNTIME` : `assert!` → `if > layout { log_error(); kernel_halt_diagnostic(); }` pour diagnostics boot | Copilot | ⚠️ Robustesse |

**Rejets** :
- **Copilot items P0** (verify(), nonces, SECURITY_READY) — Items d'implémentation déjà documentés dans §8 et §9 comme "à implémenter". Pas des erreurs de spec.
- **Copilot ExoFS TL-rules** — Hors scope de ce document d'architecture.
- **Copilot `.expect()` audit** — Tâche d'implémentation, pas de spec.
- **Z-AI, GROK4, MiniMax** — Valident la v6. Aucun bug supplémentaire.

---

## 1. Présentation et Philosophie

### 1.1 Architecture générale

Exo-OS est un système d'exploitation expérimental en **Rust x86\_64** adoptant une architecture **hybride exokernel/microkernel**. Ring 0 contient les primitives matérielles, IPC et ExoFS (pour la performance). Ring 1 contient les services système sous forme de processus `no_std` communiquant exclusivement par IPC capability-checked.

| Ring | Désignation | Exemples |
|------|-------------|----------|
| Ring 0 | Kernel | `memory/`, `scheduler/`, `ipc/`, `security/`, `fs/exofs/` |
| Ring 1 | Services privilégiés | 9 servers + 3 drivers `no_std` |
| Ring 3 | Applications | Userspace POSIX partiel via `exo-libc` |

### 1.2 Vocabulaire des identifiants

| Identifiant | Type | Rôle | Note |
|-------------|------|------|------|
| `ObjectId` | `[u8;32]` opaque | ID global unique ExoFS | `bytes[0..8]` = compteur `u64` LE ; `bytes[8..32]` = zéro. `is_valid()` vérifie le padding. |
| `BlobId` | `Blake3([u8;32])` | Hash contenu — déduplication | Calculé par `crypto_server/hash.rs` (Ring 1 — SRV-04) |
| `CapToken` | `struct {gen, oid, rights}` | Autorisation O(1) | `verify()` constant-time à implémenter. Révocation instantanée. |

### 1.3 Règles absolues Ring 1

| Règle | Description |
|-------|-------------|
| **SRV-01** | `init_server` reçoit `ChildDied` IPC du kernel → `sigchld_handler.rs` → `supervisor.rs` |
| **SRV-02** | Aucun crate Ring 1 (sauf `crypto_server`) n'importe `blake3` ou `chacha20poly1305`. CI grep obligatoire. |
| **SRV-04** | Toute opération crypto Ring 1 passe par `crypto_server` IPC. `ObjectId` calculé uniquement dans `hash.rs`. |
| **IPC-01** | `SpscRing` : `#[repr(C, align(64))]` sur `head` et `tail` — manuel, pas de crossbeam. |
| **IPC-02** | Tous les types `protocol.rs` sont `Sized` et taille fixe. `FixedString<N>`. Aucun `&str`, `Vec`, `String`, `Box`. |
| **IPC-03** | `IpcMessage.sender_pid:u32` renseigné par le kernel. Utilisé pour tous les replies directs. |
| **CAP-01** | `verify_cap_token()` en première instruction de `main.rs` — `panic!` si invalide. |
| **CAP-02** | Claim PCI : `Claim{device_id, driver_cap, nonce}` — nonce généré kernel, non forgeable. |
| **PHX-01** | Chaque server critique retourne `PrepareIsolationAck{server, checkpoint_id}` avant gel ExoPhoenix. |
| **PHX-02** | `#![no_std]` et `panic='abort'` dans chaque crate server/driver. |
| **PHX-03** | Chaque binaire ELF déployé doit avoir son `Blake3(ELF)` enregistré dans ExoFS (`build/register_binaries.sh`). |

---

## 2. Architecture en Couches

### 2.1 Hiérarchie des modules

| Couche | Module | Dépend de | Appelé par |
|--------|--------|-----------|------------|
| 0 | `memory/` | `arch/` (ASM pur) | scheduler, ipc, fs, process — comms ascendantes via trait/fn ptr |
| 1 | `scheduler/` | `memory/` uniquement | ipc, fs, process, arch |
| 1.5 | `process/` | memory + scheduler | ipc, fs, arch/syscall |
| TCB | `security/` | memory + scheduler | ipc, fs, process — `verify()` = unique point décision |
| 2a | `ipc/` | memory + sched. + security | fs — `check_access()` appelle `verify()` internement |
| 3 | `fs/` | memory + sched. + security | userspace via syscall 0-519 |
| R1 | `servers/` | syscall ExoOS + IPC | Ring 3 |
| R1 | `drivers/` | device\_server + IPC | servers Ring 1 |

### 2.2 Lock Ordering

> **Justification** : FS libère ses locks avant IPC car **IPC est bloquant**. Tenir un spinlock FS non-préemptif pendant une opération IPC bloquante = inversion de priorité + latence noyau non bornée.

| Niveau | Module | Règle |
|--------|--------|-------|
| 1 (acquérir en premier) | Memory | Jamais tenus lors d'appels aux couches supérieures |
| 2 | Scheduler | RunQueue locks (ordre `cpu_id` croissant). Jamais tenus pendant IPC. |
| 3 | Security | CapTable locks — jamais tenus lors d'appels IPC ou FS |
| 4 | IPC | Channel/endpoint/SHM — jamais tenus lors d'appels FS |
| 5 (acquérir en dernier) | FS | **DOIT relâcher AVANT tout appel IPC** |

### 2.3 Constantes MAX\_CPUS / MAX\_CORES

| Constante | Valeur | Fichier | Note |
|-----------|--------|---------|------|
| `SSR_MAX_CORES_LAYOUT` | 256 | `exo-phoenix-ssr/lib.rs` | Compile-time FIXE — dimensionne offsets SSR |
| `MAX_CORES_RUNTIME` | CPUID (≤256) | `arch/boot/early_init.rs` step 14 | Vérification runtime — voir §3.1.1 |
| `MAX_CPUS` topology | 256 | `scheduler/smp/topology.rs` | Cible hardware |
| `MAX_CPUS` preempt | **64 ⚠️ → corriger 256** | `scheduler/core/preempt.rs` | **Phase 0 obligatoire** |

---

## 3. Arborescences — Kernel Ring 0

### 3.1 arch/x86\_64/

```
arch/x86_64/
├── boot/
│   ├── early_init.rs   18 étapes boot (voir §3.1.1)
│   ├── multiboot2.rs   Multiboot2 header parsing
│   ├── uefi.rs         UEFI boot (implémenté, non câblé — Phase 3)
│   └── memory_map.rs   E820/UEFI → memory/
├── cpu/
│   ├── fpu.rs          XSAVE/XRSTOR — NOTE: MXCSR/FCW dans XSaveArea UNIQUEMENT
│   └── tsc.rs          TSC calibration + tsc_sync() cross-core
├── apic/
│   └── ipi.rs          0xF1=reschedule, 0xF2=TLB shootdown, 0xF3=ExoPhoenix freeze
├── tss.rs              TSS.RSP0 mis à jour à chaque context_switch (V7-C-03)
└── memory_iface.rs     register_tlb_ipi_sender() — APRÈS memory/ init (step 11)
```

#### 3.1.1 Séquence boot — 18 étapes

| Étape | Action | Règle |
|-------|--------|-------|
| 1–3 | Bootstrap ROM, detect boot protocol, parse E820/UEFI | |
| 4 | `EmergencyPool` (`.bss` statique — `FRAMES=256` + `WAITNODES=256`) | **EMERGENCY-01** |
| 5–7 | Bitmap bootstrap, paging+KPTI, GDT/IDT (réserve `0xF0-0xFF` + `0xF3` ExoPhoenix) | |
| 8–9 | BSP kernel stack + per-CPU GS, CPUID detection | |
| 10 | Local APIC + I/O APIC + IOMMU (VT-d si disponible) | |
| **11** | **Sous-système mémoire complet** (bitmap→buddy→SLUB→per-CPU→NUMA) | EMERGENCY-01 first |
| 12 | KERNEL\_AS + protections (NX/SMEP/SMAP/PKU) | |
| **13** | **Enregistrer IPI TLB sender** auprès de `memory/` ← APRÈS buddy (step 11) | MEM-01 |
| 14 | Parser ACPI (MADT, HPET, SRAT) + calibrer TSC | |
| 14\* | Vérification `MAX_CORES_RUNTIME` ≤ `SSR_MAX_CORES_LAYOUT` (V7-C-05) | SMP check |
| 14.5 | Allouer per-AP kernel stacks via buddy + init PerCpuData | SMP requis |
| **15** | Init scheduler + RunQueues → démarrer APs (INIT/SIPI) → **spin-wait** | CVE-EXO-001 |
| 16 | Init IPC + SHM pool | |
| 17 | Monter ExoFS + `boot_recovery_sequence` | |
| **18** | `security::init()` → `SECURITY_READY.store(true)` ← **APs franchissent spin-wait** | CVE-EXO-001 |

> **Step 14\* — vérification MAX\_CORES\_RUNTIME (V7-C-05)** :
>
> ```rust
> let runtime = cpuid_detected_cores() as usize;
> if runtime > SSR_MAX_CORES_LAYOUT {
>     log_error!("FATAL: {} CPUs détectés > SSR layout {}", runtime, SSR_MAX_CORES_LAYOUT);
>     kernel_halt_diagnostic(HaltCode::SSR_OVERFLOW);
> }
> MAX_CORES_RUNTIME.store(runtime as u32, Ordering::Release);
> ```
>
> Utiliser `if/halt` plutôt que `assert!` pour un message d'erreur diagnostique exploitable.

---

### 3.2 scheduler/ — TCB v7 (256B)

#### context\_switch() — règles x86\_64 critiques

> **V7-C-02 — switch\_asm.s ne touche PAS la FPU** : avec le modèle Lazy FPU, le context switch positionne uniquement `CR0.TS = 1` (déclenche `#NM` au prochain accès FPU). Ni `MXCSR`, ni `FCW`, ni `xsave`/`xrstor` dans `switch_asm.s`. L'état FPU complet (MXCSR, FCW, registres x87/SSE/AVX…) est géré exclusivement par `fpu/lazy.rs` via `XSaveArea`.

> **V7-C-03 — TSS.RSP0 mis à jour à chaque switch** : sur x86\_64, lorsqu'un thread Ring 3 reçoit une interruption matérielle ou effectue un syscall, le CPU charge la pile kernel depuis `TSS.RSP0` (champ du Task State Segment du cœur courant). Si `switch.rs` ne met pas à jour `TSS.RSP0` avec le `kstack_ptr` du **nouveau thread**, la prochaine entrée en Ring 0 empilera sur la pile du thread précédent → corruption immédiate.

```
scheduler/
├── core/
│   ├── task.rs         TCB 256B v7 (inchangé depuis v6)
│   ├── switch.rs       context_switch() — séquence v7 :
│   │   //  1. Si fpu_loaded(prev) → xsave64(prev.fpu_state_ptr)
│   │   //  2. prev.set_state(Runnable)
│   │   //  3. context_switch_asm(prev.kstack_ptr, next.kstack_ptr, next.cr3_phys)
│   │   //  4. next.set_state(Running)
│   │   //  5. set_cr0_ts()   ← CR0.TS=1 (Lazy FPU — V7-C-02)
│   │   //  6. tss_set_rsp0(current_cpu(), next.kstack_ptr)  ← V7-C-03 OBLIGATOIRE
│   ├── preempt.rs      MAX_CPUS=64 ⚠️ (cible 256 — Phase 0)
│   ├── runqueue.rs     intrusive via rq_next/rq_prev [240/248]
│   │   // rq_next/rq_prev = null quand BLOCKED
│   │   // WaitNode = structure séparée EmergencyPool
│   ├── pick_next.rs    O(1)
│   └── kthread.rs      create_kernel_thread(entry_fn, priority)
├── fpu/
│   ├── lazy.rs         #NM handler — séquence lazy FPU v6 (fninit sur 1ère utilisation)
│   └── save_restore.rs xsave64() / xrstor64() FFI
├── policies/           cfs.rs, rt.rs, deadline.rs, idle.rs
├── smp/                topology.rs, affinity.rs, migration.rs, load_balance.rs
├── timer/ energy/ ai_guided/
└── asm/switch_asm.s    CR3 + 15 GPRs (rax..r14 + r15)
                        PAS de MXCSR/FCW (V7-C-02 — Lazy FPU)
```

#### switch\_asm.s — pseudo-code v7

```asm
context_switch_asm:          // (prev_kstack_ptr*, next_kstack, next_cr3)
    push %rbx
    push %rbp
    push %r12
    push %r13
    push %r14
    push %r15                // 6 callee-saved + rip implicite = 7×8 = 56B sur pile
    mov  %rsp, (%rdi)        // sauvegarder RSP du prev dans prev.kstack_ptr
    cmp  %rdx, %cr3
    je   .skip_cr3
    mov  %rdx, %cr3          // changer espace d'adressage (KPTI)
.skip_cr3:
    mov  %rsi, %rsp          // charger RSP du next depuis next.kstack_ptr
    pop  %r15
    pop  %r14
    pop  %r13
    pop  %r12
    pop  %rbp
    pop  %rbx
    ret                      // retourne dans le contexte du next thread
    // NOTE: MXCSR/FCW ABSENTS — CR0.TS=1 déclenche #NM si FPU utilisée (Lazy FPU)
```

#### FPU Lazy — Séquence v6/v7 (inchangée)

```
#NM exception → sched_fpu_handle_nm()
  1. CLTS (clear CR0.TS)
  2. Si fpu_state_ptr == null:
       alloc_fpu_state()  // taille = CPUID leaf 0Dh sub-leaf 0
       fninit()           // init x87 FPU en état propre
       vzeroupper()       // zeroise registres AVX (si disponible)
       xsave64(ptr)       // créer baseline valide
  3. Sinon:
       xrstor64(ptr)      // restaurer état précédent
```

#### TCB Layout v7 — 256 octets (inchangé depuis v6)

| Champ | Offset | Taille | Rôle |
|-------|--------|--------|------|
| `cap_table_ptr` | [0] | 8 B | `*const CapTable` du processus (PARTAGÉ par threads) — CL1 |
| `kstack_ptr` | [8] | 8 B | RSP Ring 0 — source de vérité pour `TSS.RSP0` (V7-C-03) |
| `tid` | [16] | 8 B | Thread ID global unique |
| `sched_state` | [24] | 8 B | AtomicU8 : RUNNING/BLOCKED/ZOMBIE/DEAD |
| `fs_base` | [32] | 8 B | MSR `0xC0000100` — TLS userspace |
| `user_gs_base` | [40] | 8 B | MSR `0xC0000101` userspace sauvegardé AVANT `SWAPGS` |
| `pkrs` | [48] | 4 B | Intel PKS 32b |
| `_pad` | [52] | 4 B | Alignement |
| `cr3_phys` | [56] | 8 B | Adresse physique PML4 — lu par `switch_asm.s` |
| `rax..r14` (14 GPRs) | [64] | 112 B | `rax,rbx,rcx,rdx,rsi,rdi,rbp,r8..r14` — 14×8B |
| `r15` | [176] | 8 B | 15ème GPR — callee-saved ABI SysV |
| `[pad align]` | [184] | 8 B | Alignement — zone GPR = 128B total |
| `rip` | [192] | 8 B | Instruction pointer Ring 3 |
| `rsp_user` | [200] | 8 B | Stack pointer Ring 3 |
| `rflags` | [208] | 8 B | EFLAGS étendu |
| `cs/ss` | [216] | 8 B | Segment selectors |
| `cr2` | [224] | 8 B | Page fault addr — **diagnostic ExoPhoenix UNIQUEMENT, JAMAIS restauré** |
| `fpu_state_ptr` | [232] | 8 B | `*mut XSaveArea` — null si jamais utilisé. Libéré via `release_thread_resources()`. |
| `rq_next` | [240] | 8 B | RunQueue intrusive — null si BLOCKED |
| `rq_prev` | [248] | 8 B | RunQueue intrusive — null si BLOCKED |

> **Vérification** : CL1=[0..63]=64B ✓, CL2+3=[64..191]=128B ✓, CL4=[192..255]=64B ✓, **Total=256B** ✓

---

### 3.3 process/

#### exec() — séquence finale v7 (V7-C-04)

> **V7-C-04 — Clarification** : deux comportements distincts dans `exec()` :
> 1. **Signal mask** : **hérité** du processus appelant (conformité POSIX IEEE 1003.1)
> 2. **Pending signals** : **flushés** sauf `SIGKILL/SIGSTOP` — comportement ExoOS explicitement défini (pas POSIX strict, documenté)

```
process/exec.rs  — do_exec() séquence v7 :
  1. verify_cap(EXEC) + is_valid(object_id) + ObjectKind != Secret
  2. mask_all_signals_manual()
     // Masque tous les signaux sans RAII restaurateur
  2.5 signal_queue.flush_all_except_sigkill()
     // [ExoOS-spécifique] Flush pending signals — comportement défini, pas POSIX strict
  3. reset_signal_handlers_to_sdf()
     // Réinitialise handlers → SIG_DFL (AVANT reset du mask)
  4. load_elf(object_id)
  5. reset_tcb_context()
     // fs_base, user_gs_base, cr3_phys (nouveau PML4)
     tcb.signal_mask = CALLER_SIGNAL_MASK   // hérité (POSIX) ← V6-C-03
  6. tss_set_rsp0(cpu, tcb.kstack_ptr)      // ← V7-C-03 : mettre à jour TSS.RSP0
  7. return_to_new_userspace()
```

```
process/exit.rs  — do_exit() + thread_exit() :
  release_thread_resources(tcb):
    if fpu_state_ptr != null:
      dealloc(ptr, Layout::from_size_align(xsave_size, 64))
      // xsave_size = CPUID leaf 0Dh sub-leaf 0 (détecté au boot)
    tcb.rq_next = null; tcb.rq_prev = null
  cap_table.revoke_all()
  signal_queue.flush_all()
  set_state(Zombie) → wakeup_joiners()
```

---

### 3.4 security/ et fs/exofs/

```
security/
├── mod.rs              SECURITY_READY: AtomicBool (false → true step 18)
├── capability/verify.rs  → À implémenter constant-time (crate subtle no_std)
├── capability/table.rs   CapTable partagée par threads via cap_table_ptr TCB
├── access_control/check.rs  check_access() = wrapper → verify() internement
└── crypto/rng.rs         fill_random() — RDRAND + ChaCha20 CSPRNG

fs/exofs/
├── syscall/            500-518 ExoFS + 519=réservé (sys_ni_syscall)
├── storage/virtio_adapter.rs  PCI MMIO QEMU 0x1000_0000 #[cfg(qemu_only)]
├── epoch/epoch_record.rs      TODO:517 métadonnées rollback
├── epoch/gc/                  GC → create_kernel_thread() Phase 4
├── path/path_index.rs         SipHash-2-4 keyed (mount_secret_key depuis rng)
├── crypto/                    RustCrypto no_std Ring 0 (Cargo.toml ✅ LAC-03)
├── audit/ring_buffer.rs       AUDIT-RING-SEC lock-free
└── vfs_compat/
```

> ⚠️ **IOMMU mode dégradé** : Sans IOMMU, un driver Ring 1 compromis peut accéder à toute la mémoire physique (violation Zero Trust). En production, `device_server` DOIT attribuer des IOMMU domains restrictifs (IOVAs uniquement).

---

## 4. ExoPhoenix — SSR Layout

> **SSR à `0x0100_0000–0x0100_FFFF` (64 KiB). Début exact Zone DMA32. Déclarée e820.**

| Offset | Taille | Contenu |
|--------|--------|---------|
| +0x0000 | 64 B | **HEADER** : `MAGIC(8B)` + `HANDOFF_FLAG(8B)` + `LIVENESS_NONCE(8B)` + `SEQLOCK(8B)` + `pad(32B)` |
| +0x0040 | 64 B | CANAL COMMANDE B→A (align 64B) |
| +0x0080 | 16 384 B | **FREEZE ACK PER-CORE** — 256 × 64B |
| +0x4080 | 16 384 B | **PMC SNAPSHOT PER-CORE** — 256 × 64B |
| +0x8080 | 16 256 B | **EXTENSIONS RESERVED** : `+0x8080–0x9FFF` compteurs, `+0xA000–0xBFFF` trace buffer |
| +0xC000 | 8 192 B | **LOG AUDIT B** — append-only, RO pour Kernel A. Protection PKU/PKS recommandée. |
| +0xE000 | 8 192 B | **MÉTRIQUES PUSH A→B** — rate-limited, overflow silencieux |
| **TOTAL** | **65 536 B** | **= 64 KiB ✓** — `SSR_LAYOUT_MAGIC` validé au boot A et B |

> **Vérification** : `64+64+16384+16384+16256+8192+8192 = 65536B` ✓
>
> **IDT 0xF3** : Kernel B → IPI Local APIC → vecteur `0xF3` → handler `exo_shield`.

---

## 5. Arborescences Ring 1 — Libs

### 5.1 Workspace global

```toml
# ExoOS/Cargo.toml
[workspace]
members = [
  "kernel", "exo-boot",
  "servers/ipc_broker", "servers/init_server", "servers/vfs_server",
  "servers/memory_server", "servers/crypto_server", "servers/device_server",
  "servers/scheduler_server", "servers/network_server", "servers/exo_shield",
  "drivers/virtio-block", "drivers/virtio-net", "drivers/virtio-console",
  "libs/exo-types", "libs/exo-ipc", "libs/exo-syscall", "libs/exo-phoenix-ssr",
]

[workspace.dependencies]
exo-types       = { path = "libs/exo-types" }
exo-ipc         = { path = "libs/exo-ipc" }
exo-syscall     = { path = "libs/exo-syscall" }
exo-phoenix-ssr = { path = "libs/exo-phoenix-ssr" }
```

### 5.2 libs/exo-types/

```
libs/exo-types/         no_std — AUCUNE dépendance blake3/chacha20 (SRV-02)
└── src/
    ├── lib.rs
    ├── cap.rs          CapToken, CapabilityType, Rights
    │                   + verify_cap_token(token, expected_type) → panic! si invalide
    ├── error.rs        ExoError — codes d'erreur unifiés
    ├── object_id.rs    ObjectId([u8;32]) — bytes[0..8]=compteur u64 LE, bytes[8..32]=zéro
    │                   fn is_valid(&self) -> bool { self.0[8..32].iter().all(|&b| b == 0) }
    ├── ipc_msg.rs      IpcMessage { sender_pid:u32, msg_type:u32, payload:[u8;56] } = 64B
    └── fixed_string.rs FixedString<N>, ServiceName=FixedString<64>, PathBuf=FixedString<512>
```

### 5.3 libs/exo-ipc/

```
libs/exo-ipc/
└── src/
    ├── send.rs         ipc_send(pid:u32, msg:IpcMessage)
    ├── receive.rs      ipc_receive() → IpcMessage — blocking
    └── ring.rs         SpscRing<T,N> — #[repr(C, align(64))] sur head/tail (IPC-01)
```

### 5.4 libs/exo-syscall/

```
libs/exo-syscall/
└── src/
    ├── exofs.rs        Wrappers syscalls 500-518 ExoFS + 519=réservé
    ├── process.rs      fork/exec/exit/wait
    ├── memory.rs       mmap/munmap/mprotect
    └── phoenix.rs      phoenix_query(520), phoenix_notify(521, AllReady)
```

### 5.5 libs/exo-phoenix-ssr/

```rust
// libs/exo-phoenix-ssr/src/lib.rs — SOURCE UNIQUE PARTAGÉE A↔B
pub const SSR_LAYOUT_MAGIC:       u64   = 0x5353525F4558_4F53; // "SSR_EXOS"
pub const SSR_BASE_PHYS:          u64   = 0x0100_0000;
pub const SSR_SIZE:               usize = 0x10000;             // 64 KiB
pub const SSR_MAX_CORES_LAYOUT:   usize = 256;                 // compile-time FIXE
pub static MAX_CORES_RUNTIME: AtomicU32 = AtomicU32::new(0);  // CPUID boot
pub const SSR_FREEZE_ACK_OFFSET:  usize = 0x0080;
pub const SSR_PMC_OFFSET:         usize = 0x4080;
pub const SSR_EXTENSIONS_START:   usize = 0x8080;
pub const SSR_EXTENSIONS_SIZE:    usize = 0x3F80; // 16 256 B
pub const SSR_LOG_AUDIT_OFFSET:   usize = 0xC000;
pub const SSR_METRICS_OFFSET:     usize = 0xE000;

pub const fn freeze_ack_offset(apic_id: usize) -> usize {
    SSR_FREEZE_ACK_OFFSET + apic_id * 64
}
pub const fn pmc_snapshot_offset(apic_id: usize) -> usize {
    SSR_PMC_OFFSET + apic_id * 64
}
```

---

## 6. Arborescences Ring 1 — Servers

### 6.1 Ordre de démarrage canonique — 12 étapes

| Étape | Server | PID | Condition | Priorité |
|-------|--------|-----|-----------|----------|
| 1 | `ipc_broker` | 2 | Rien — PID 2 assigné kernel au boot | **P1 CRITIQUE** |
| 2 | `memory_server` | dyn | ipc\_broker disponible | **P1 CRITIQUE** |
| 3 | `init_server` | 1 | ipc\_broker + memory\_server + **BootInfo virt** | **P1 CRITIQUE** |
| 4 | `vfs_server` | 3 | init\_server + ExoFS kernel monté | P2 HAUTE |
| 5 | `crypto_server` | 4 | vfs\_server disponible — **SEUL RustCrypto Ring 1** | P2 HAUTE |
| 6 | `device_server` | dyn | ipc\_broker + memory\_server — **AVANT tout driver** | P2 HAUTE |
| 7 | `virtio-block` | dyn | device\_server disponible | **P1 CRITIQUE** |
| 8 | `virtio-net` | dyn | device\_server disponible | P2 HAUTE |
| 9 | `virtio-console` | dyn | device\_server disponible | P2 HAUTE |
| 10 | `network_server` | dyn | virtio-net disponible | P3 |
| **11** | `scheduler_server` | dyn | init\_server disponible | P3 |
| **12** | `exo_shield` | dyn | Phase 3 ExoPhoenix stable **UNIQUEMENT** | Après Phase 3 |

### 6.2 servers/ipc\_broker/ — PID 2

```
src/
├── main.rs         verify_cap_token() → enregistrement → boucle IPC
├── registry.rs     ServiceName → (pid:u32, CapToken)
├── directory.rs    Lookup service → reply via sender_pid
├── protocol.rs     Register{name, cap}, Lookup, Deregister
└── persistence.rs  Dump registry → ExoFS — survie au crash
```

### 6.3 servers/init\_server/ — PID 1

> **V7-C-01** : `init_server` reçoit `boot_info_virt` (adresse **virtuelle**). Le kernel Ring 0 mappe la page `BootInfo` dans la VMA de `init_server` avant son lancement via `exec()`. Déréférencer une adresse physique depuis un processus avec paging actif = `#PF` immédiat.

```
src/
├── main.rs           fn _start(boot_info_virt: usize) -> !
│                     // Adresse VIRTUELLE — mappée par le kernel dans la VMA (V7-C-01)
│                     let bi = unsafe { &*(boot_info_virt as *const BootInfo) };
│                     assert!(bi.validate());
│                     verify_cap_token(&bi.ipc_broker_cap, CapabilityType::IpcBroker);
├── supervisor.rs     Restart policy — écoute ChildDied
├── service_table.rs  Services ordonnés avec restart policy
├── sigchld_handler.rs Reçoit ChildDied IPC → supervisor.rs (SRV-01)
├── isolation.rs      PrepareIsolation → flush → ExoFS → PrepareIsolationAck
└── protocol.rs       Start, Stop, Status, Restart, ChildDied, PrepareIsolation, Ack
```

### 6.4 servers/vfs\_server/ — PID 3

```
src/
├── main.rs
├── mount.rs        Mount namespace — PathBuf/ExoFS objectId
├── path_resolver.rs PathBuf → ObjectId (SYS_EXOFS_PATH_RESOLVE)
├── pseudo_fs.rs    /proc /sys /dev
├── fd_table.rs     Table FDs par processus
├── isolation.rs    PrepareIsolation → flush → ExoFS → ack
└── protocol.rs     Open{path:PathBuf}, Close, Read, Write, Stat, Readdir, Isolation
```

### 6.5 servers/memory\_server/

```
src/
├── main.rs
├── allocator.rs    Interface buddy kernel — Alloc, Free
├── mmap.rs         MapShared(ObjectId), MapAnon, Unmap, Protect
│                   ⚠ retourne offset virtuel — JAMAIS adresse physique
├── region_table.rs Registre régions allouées par processus
├── isolation.rs    PrepareIsolation → flush → ExoFS → ack
└── protocol.rs     Alloc, Free, MapShared, Protect, PrepareIsolation, Ack
```

### 6.6 servers/crypto\_server/ — PID 4

```
src/
├── main.rs         NE JAMAIS exposer clés brutes
├── rng.rs          CSPRNG — RDRAND + ChaCha20
├── key_store.rs    Stockage clés en mémoire chiffrée
├── hash.rs         Blake3 → ObjectId — SEULE source d'ObjectId Ring 1 (SRV-04)
├── entropy.rs      RDRAND(64B) → entropy_pool → HKDF → master_key
├── isolation.rs    sealed blob = ChaCha20Poly1305(clé dérivée, clé active)
│                   → EXOFS_WRITE → ObjectId → PrepareIsolationAck
└── protocol.rs     GenKey, Encrypt, Decrypt, Hash{data}→ObjectId, Isolation
```

### 6.7 servers/device\_server/

```
src/
├── pci_registry.rs   Registre PCI : bus/device/func → driver assigné
├── lifecycle.rs      Start/Stop/Reset driver Ring 1. FLR PCI sur reset.
├── probe.rs          Découverte device↔driver
├── irq_router.rs     Routage IRQs hardware → drivers via IPC
├── claim_validator.rs Valide : CapToken + PciId autorisé + libre (CAP-02)
└── protocol.rs       Probe, Claim{device_id, driver_cap, nonce}, Release, IrqNotify
```

### 6.8 servers/scheduler\_server/ — SANS isolation.rs

> État transient — relancé depuis zéro après cycle ExoPhoenix.

```
src/
├── main.rs
├── policy.rs       CFS adapté Ring 1/3
├── thread_table.rs Table threads actifs
└── protocol.rs     SetPriority, Yield, GetStat
```

### 6.9 servers/network\_server/ — SANS isolation.rs

```
src/
├── main.rs
├── socket.rs       Table sockets TCP/UDP
├── routing.rs      Table de routage, ARP
├── driver_iface.rs Interface vers virtio-net
└── protocol.rs     Socket, Connect, Bind, Send, Recv, Close
# smoltcp default-features=false features=['socket-tcp','socket-udp']
```

### 6.10 servers/exo\_shield/ — APRÈS Phase 3 ExoPhoenix

```
src/
├── main.rs              Enregistre handler IDT 0xF3 (après SECURITY_READY)
├── irq_handler.rs       Handler IDT 0xF3 → broadcast PrepareIsolation
├── isolation_notify.rs  Collecte PrepareIsolationAck → phoenix_notify(521)
├── subscription.rs      Registry servers abonnés
└── protocol.rs          PhoenixEvent, PrepareIsolation, PrepareIsolationAck
```

---

## 7. Arborescences Ring 1 — Drivers

> ⚠️ **Ordre critique** : `device_server` (étape 6) **AVANT** tout driver (étapes 7-9).

### 7.1 drivers/virtio-block/ — PRIORITÉ 1

```
src/
├── main.rs         Claim sécurisé (driver_cap+nonce) → device_server + init + boucle
├── virtio.rs       Feature negotiation, device reset, config space
├── queue.rs        VirtQueue split ring
├── block.rs        VIRTIO_BLK_T_IN/OUT
└── exofs_backend.rs Backend ExoFS
```

### 7.2 drivers/virtio-net/ + drivers/virtio-console/

```
virtio-net/src/   main.rs, virtio.rs, queue.rs (RX+TX), net.rs → network_server
virtio-console/src/ main.rs, virtio.rs, console.rs → /dev/console dans vfs_server
```

### 7.3 CI Enforcement + PHX-03

```bash
# SRV-02 : blake3/chacha20 uniquement dans crypto_server
grep -rn 'blake3\|chacha20poly1305' servers/ drivers/ libs/ \
  | grep -v 'servers/crypto_server' && echo 'VIOLATION SRV-02' && exit 1

# no_std IPC : pas de types dynamiques
grep -rn 'Vec<\|: String\|Box<\|use alloc' libs/exo-types/ \
  servers/*/src/protocol.rs && echo 'VIOLATION IPC no_std' && exit 1

# panic=abort dans chaque Cargo.toml
for f in servers/*/Cargo.toml drivers/*/Cargo.toml; do
  grep -q 'panic = "abort"' "$f" || { echo "MISSING: $f"; exit 1; }
done

# PHX-03 : build/register_binaries.sh
for binary in ipc_broker init_server vfs_server memory_server crypto_server \
              device_server scheduler_server network_server \
              virtio_block virtio_net virtio_console; do
  HASH=$(b3sum --no-names target/release/$binary)
  exofs-deploy register --binary $binary --hash $HASH
done
```

---

## 8. BootInfo v7

> **V7-C-01** : Le kernel Ring 0 mappe la page contenant `BootInfo` dans la VMA de `init_server` avant de le lancer. `init_server` reçoit l'adresse **virtuelle** `boot_info_virt` — pas l'adresse physique brute.

```rust
#[repr(C)]
pub struct BootInfo {
    pub magic:              u64,      // 0x424F4F545F494E46
    pub version:            u32,      // BOOT_INFO_VERSION = 1
    pub _pad:               [u8; 4],  // alignement CapToken (u64)
    pub ipc_broker_cap:     CapToken,
    pub ssr_phys_addr:      u64,
    pub nr_cpus:            u32,
    pub _pad2:              [u8; 4],
    pub memory_bitmap_phys: u64,
    pub memory_bitmap_size: u64,
    pub kernel_heap_start:  u64,
    pub kernel_heap_end:    u64,
    pub reserved:           [u64; 16], // must be 0
}

// Dans process/exec.rs — chargement init_server :
// 1. Allouer page BootInfo en mémoire physique
// 2. Remplir les champs BootInfo
// 3. Mapper la page dans la VMA de init_server (adresse virtuelle définie)
// 4. Passer l'adresse virtuelle comme premier argument de _start()
```

---

## 9. Défauts — État v7

### 9.1 Corrections confirmées

| Défaut | Statut v7 | Cycle |
|--------|-----------|-------|
| LAC-03 Cargo.toml crypto | ✅ CORRIGÉ | v4 |
| MEM-FUTEX 4096 buckets | ✅ CORRIGÉ | v3 |
| CVE-EXO-004 EmergencyPool | ✅ CORRIGÉ | v3 |
| SSR Extensions 16 KiB | ✅ CORRIGÉ | v4 |
| exec() RAII guard | ✅ CORRIGÉ | v4 |
| exec() signal mask hérité | ✅ CORRIGÉ | v6 |
| AP stacks allocation | ✅ CORRIGÉ | v4 |
| fpu\_state\_ptr leak | ✅ CORRIGÉ | v5 |
| TCB cr3\_phys manquant | ✅ CORRIGÉ | v6 |
| FPU #GP sur zone neuve | ✅ CORRIGÉ | v6 |
| §6.1 numérotation | ✅ CORRIGÉ | v6 |
| TSS.RSP0 non mis à jour | ✅ CORRIGÉ | **v7** |
| MXCSR/FCW dans switch\_asm | ✅ CORRIGÉ | **v7** |
| BootInfo adresse physique | ✅ CORRIGÉ | **v7** |
| MAX\_CPUS preempt=64 | ⚠️ Phase 0 | — |

### 9.2 Vulnérabilités P0 restantes (à implémenter)

| ID | Action |
|----|--------|
| **LAC-01** | `verify()` constant-time — crate `subtle` no\_std, `ct_eq()` |
| **LAC-04** | `NONCE_COUNTER` atomique + `HKDF(counter \|\| object_id \|\| rdrand)` |
| **LAC-06** | `key_storage.rs` — Argon2id OWASP (m=65536, t=3, p=4), sel 128 bits |
| **CVE-EXO-001** | `spin-wait` ASM sur APs avant `SECURITY_READY` |

### 9.3 Problèmes P1 (à implémenter)

- **MEM-DMA-IRQ** : DMA ISR libère lock PUIS wakeup — Phase 2
- **LOCK-05** : `reserve_for_commit(n)` AVANT `EPOCH_COMMIT_LOCK` — Phase 2
- **PREEMPT-BLOCK** : `debug_assert!(preempt_count==0)` avant `block_current()` — Phase 2
- **SCHED-INTRU** : implémenter RunQueue via `rq_next/rq_prev` TCB [240/248] — Phase 2
- **IOMMU-01** : IOMMU domains pour drivers Ring 1 — Phase 2
- **TSC-EDF** : CPUID Invariant TSC + `tsc_sync()` — Phase 2

---

## 10. Checklist de Conformité v7 (45 checks)

| # | Vérification | Module | Statut |
|---|-------------|--------|--------|
| S-01 | `verify_cap()` avant tout accès ExoFS | `fs/exofs/syscall/*` | 🔴 Obligatoire |
| S-02 | `verify()` constant-time (`subtle`, `ct_eq()`) | `security/capability/verify.rs` | 🔴 À implémenter |
| S-03 | `check_access()` = wrapper de `verify()` | `security/access_control/` | 🔴 Vérifier |
| S-04 | `SECURITY_READY` + APs spin-wait ASM step 15 | `security/mod.rs` + `arch/smp/` | 🔴 À implémenter |
| S-05 | Blake3 AVANT compression | `fs/exofs/crypto/blake3.rs` | 🔴 Obligatoire |
| S-06 | Nonce = `NONCE_COUNTER` + `HKDF(counter \|\| object_id)` | `fs/exofs/crypto/xchacha20.rs` | 🔴 À implémenter |
| S-07 | Pipeline données→Blake3→LZ4→XChaCha20→disque | `secret_writer.rs` | 🔴 Obligatoire |
| S-08 | `Cargo.toml` kernel : blake3, chacha20poly1305, hkdf, argon2, siphasher | `kernel/Cargo.toml` | ✅ CORRIGÉ |
| S-09 | `ObjectKind::Secret` : BlobId jamais retourné | `get_content_hash.rs` | 🔴 Obligatoire |
| S-10 | `exec()` sur Secret = `Err(NotExecutable)` | `process/exec.rs` | 🔴 Obligatoire |
| S-11 | `exec()` : mask hérité caller + pending flush ExoOS | `process/exec.rs` | ✅ CORRIGÉ v7 |
| S-12 | PathIndex = SipHash-2-4 keyed depuis `rng::fill_random()` | `path_index.rs` | 🔴 Obligatoire |
| S-13 | Quota vérifié AVANT allocation | `quota_enforcement.rs` | 🔴 Obligatoire |
| S-14 | AUDIT-RING-SEC : toutes opérations loggées | `audit/ring_buffer.rs` | 🔴 Obligatoire |
| S-15 | GET\_CONTENT\_HASH toujours auditée | `get_content_hash.rs` | 🔴 Obligatoire |
| S-16 | Argon2id m=65536 t=3 p=4 + sel 128 bits | `fs/exofs/crypto/key_storage.rs` | 🔴 À implémenter |
| S-17 | Cap table fork shadow-copy RCU + rollback | `process/fork.rs` | 🔴 Obligatoire |
| S-18 | `do_exit()` + `thread_exit()` : `release_thread_resources()` | `process/` | ✅ SPÉCIFIÉ v5 |
| S-19 | RunQueue intrusive via `rq_next/rq_prev` TCB [240/248] | `scheduler/core/runqueue.rs` | ✅ SPÉCIFIÉ v4 |
| S-20 | CI grep Ring 1 : aucun blake3/chacha20 hors `crypto_server` | `CI/Makefile` | 🔴 CI |
| S-21 | `debug_assert!(preempt_count==0)` avant `block_current()` | `sync/wait_queue.rs` | 🔴 À ajouter |
| S-22 | DMA ISR : libérer lock AVANT wakeup | `memory/dma/` | 🔴 À corriger |
| S-23 | `SSR_LAYOUT_MAGIC` vérifié par Kernel A et B au boot | `exo-phoenix-ssr` | 🔴 À implémenter |
| S-24 | Chaque server Ring 1 : `verify_cap_token()` en `main.rs` | `servers/*/main.rs` | 🔴 Obligatoire |
| S-25 | Aucun `&str`/`Vec`/`String` dans `protocol.rs` Ring 1 | `servers/*/protocol.rs` | 🔴 CI |
| S-26 | CI `MAX_CPUS==256` dans `preempt.rs` | `CI/build.rs` | 🟡 Phase 0 |
| S-27 | `EMERGENCY_POOL_SIZE_FRAMES=256` + `WAITNODES=256` (`.bss`) | `memory/physical/frame/pool.rs` | ✅ CORRIGÉ |
| S-28 | Boot : step 11 (memory) AVANT step 13 (register IPI TLB) | `arch/boot/early_init.rs` | ✅ CORRIGÉ v4 |
| S-29 | Drivers Ring 1 : allocations DMA via IOMMU (IOVAs) | `drivers/*/virtio.rs` | 🟡 P1 |
| S-30 | EDF : CPUID Invariant TSC + `tsc_sync()` | `scheduler/policies/deadline.rs` | 🟡 P1 |
| S-31 | `fpu_state_ptr` libéré dans `thread_exit()` ET `do_exit()` | `process/` | ✅ SPÉCIFIÉ v5 |
| S-32 | BootInfo : `_pad:[u8;4]` + `repr(C)` + `validate()` | `boot/boot_info.rs` | ✅ CORRIGÉ v4 |
| S-33 | `ObjectId::is_valid()` vérifié avant `verify()` | `exo-types/object_id.rs` | ✅ SPÉCIFIÉ v4 |
| S-34 | AP stacks allouées step 14.5 avant INIT/SIPI | `arch/boot/early_init.rs` | ✅ SPÉCIFIÉ v4 |
| S-35 | Vérification `MAX_CORES_RUNTIME` avec `kernel_halt_diagnostic` | `arch/boot/early_init.rs` | ✅ CORRIGÉ v7 |
| S-36 | `rq_next/rq_prev = null` quand thread BLOCKED | `scheduler/core/runqueue.rs` | ✅ SPÉCIFIÉ v5 |
| S-37 | PHX-03 : `build/register_binaries.sh` avant premier boot | `build/` | 🔴 CI |
| S-38 | `exo_shield` : handler IDT 0xF3 enregistré après `SECURITY_READY` | `servers/exo_shield/` | 🟡 Phase 4 |
| S-39 | `scheduler_server` + `network_server` : pas d'`isolation.rs` | `servers/` | ✅ CORRIGÉ v5 |
| S-40 | `cr3_phys` dans TCB [56] — lu par `switch_asm.s` | `scheduler/core/task.rs` | ✅ SPÉCIFIÉ v6 |
| S-41 | FPU 1ère utilisation : `fninit + xsave64` (pas `xrstor64`) | `scheduler/fpu/lazy.rs` | ✅ SPÉCIFIÉ v6 |
| S-42 | `XSaveArea` size = CPUID leaf 0Dh sub-leaf 0 | `scheduler/fpu/` | ✅ SPÉCIFIÉ v6 |
| S-43 | `exec()` : signal mask hérité du processus appelant | `process/exec.rs` | ✅ CORRIGÉ v6 |
| **S-44** | `switch_asm.s` : PAS de MXCSR/FCW — Lazy FPU, seul CR0.TS=1 | `scheduler/asm/switch_asm.s` | ✅ CORRIGÉ **v7** |
| **S-45** | `context_switch()` : `tss_set_rsp0(cpu, next.kstack_ptr)` obligatoire | `scheduler/core/switch.rs` | ✅ CORRIGÉ **v7** |

---

## 11. Feuille de Route — Finale

### Phase 0 — Cohérence immédiate
- `preempt.rs` : `MAX_CPUS` 64 → 256 (S-26)
- Vérification `MAX_CORES_RUNTIME` avec halt diagnostic step 14 (S-35)
- CI grep SRV-02 + no\_std + panic=abort (S-20/25)
- `build/register_binaries.sh` — PHX-03 (S-37)

### Phase 1 — Sécurité critique
- `verify()` constant-time via crate `subtle` no\_std — `ct_eq()` (LAC-01 / S-02)
- Nonces XChaCha20 : `NONCE_COUNTER` + HKDF (LAC-04)
- `key_storage.rs` Argon2id OWASP (LAC-06)
- `SECURITY_READY` spin-wait ASM sur APs (CVE-EXO-001 / S-04)
- Implémenter `context_switch()` v7 : `tss_set_rsp0()` + `CR0.TS=1` (S-44/S-45)
- Implémenter `switch_asm.s` v7 : CR3 + 15 GPRs, sans MXCSR/FCW (S-44)
- Implémenter `exec()` v7 : signal mask hérité + `tss_set_rsp0()` (S-43/S-45)
- Mapper `BootInfo` en VMA de `init_server` avant lancement (V7-C-01)

### Phase 2 — Robustesse kernel
- RunQueue intrusive : `rq_next/rq_prev` TCB [240/248] (SCHED-INTRU)
- DMA ISR wakeup différé hors lock (MEM-DMA-IRQ)
- `reserve_for_commit()` avant `EPOCH_COMMIT_LOCK` (LOCK-05)
- `debug_assert!(preempt_count==0)` avant `block_current()`
- IOMMU domains pour drivers Ring 1 (S-29)
- TSC sync + CPUID Invariant TSC check (S-30)

### Phase 3 — Ring 1 complet
- Implémenter servers Ring 1 étapes 1→11 (ordre canonique)
- Implémenter drivers virtio-block → virtio-net → virtio-console
- Parser ACPI SRAT pour NUMA multi-nœuds
- Câbler UEFI dans `early_init.rs`

### Phase 4 — ExoPhoenix & Qualité
- Implémenter `exo_shield` avec handler IDT 0xF3 IRQ-driven (étape 12)
- GC kthread autonome via `create_kernel_thread()` + timeout
- PCI Config Space scan dans `virtio_adapter.rs` (production)
- AUDIT-RING-SEC : sticky entries + compteur logs perdus
- PKU/PKS pour LOG AUDIT B dans SSR
- `proptest` + `INVARIANTS.md` pour délégation caps

---

*Exo-OS — Architecture Complète — **v7 Finale** — Mars 2026*

*5 cycles de revue · 6 IAs · 14+25+13+6+5 corrections cumulées · 45 checks CI · Prête pour implémentation*
