# EXO-OS — Architecture Complète v6

> **Kernel Exokernel · Ring 0 + Ring 1 + Drivers · Spécification Finale**
>
> *v6 — Mars 2026 — 4ème cycle revue · 6 corrections · format Markdown*

---

## Changelog v6

| ID | Correction | Source | Gravité |
|----|-----------|--------|---------|
| **V6-C-01** | TCB [48] : `cr3_phys` (8B) ajouté dans `pkrs+_pad` — `pkrs(4B)+_pad(4B)+cr3_phys(8B)=16B` | Gemini | 🔴 Bug réel |
| **V6-C-02** | FPU lazy : 1ère utilisation → `fninit + xsave64` (pas `xrstor64` sur zone neuve = `#GP`) | Gemini | 🔴 Crash garanti |
| **V6-C-03** | `exec()` signal mask : **hérité** du processus appelant (conformité POSIX — `ALLSIGS_UNBLOCKED` incorrect) | Gemini | 🔴 POSIX |
| **V6-C-04** | §6 numérotation : 12 étapes, `scheduler_server=11`, `exo_shield=12` — correction texte V5-C-09 | GROK4+KIMI+Z-AI | ⚠️ Typo |
| **V6-C-05** | `S-38` : Phase 3 → **Phase 4** (alignement roadmap) | Z-AI | ⚠️ Incohérence |
| **V6-C-06** | `XSaveArea` size : `CPUID leaf 0Dh sub-leaf 0` — taille exacte détectée au boot | KIMI | ⚠️ Précision |

**Rejets** :
- **MiniMax « TCB=264B »** — FAUX. 18 termes = 256B prouvé par offsets [0..255]. Voir §3.2.
- **Copilot TL-rules ExoFS** — Hors scope du document d'architecture.
- **Gemini G-2 « effacer pending signals = POSIX »** — ExoOS est « POSIX partiel ». Le `flush_all_except_sigkill` est conservé comme comportement défini ExoOS. Seul le signal mask est corrigé (V6-C-03).

---

## 1. Présentation et Philosophie

### 1.1 Architecture générale

Exo-OS est un système d'exploitation expérimental en **Rust x86\_64** adoptant une architecture **hybride exokernel/microkernel**. Ring 0 contient les primitives matérielles, IPC et ExoFS (pour la performance). Ring 1 contient les services système sous forme de processus `no_std` communiquant exclusivement par IPC capability-checked.

| Ring | Désignation | Exemples |
|------|-------------|---------|
| Ring 0 | Kernel | `memory/`, `scheduler/`, `ipc/`, `security/`, `fs/exofs/` |
| Ring 1 | Services privilégiés | 9 servers + 3 drivers `no_std` |
| Ring 3 | Applications | Userspace POSIX partiel via `exo-libc` |

### 1.2 Vocabulaire des identifiants

| Identifiant | Type | Rôle | Note |
|-------------|------|------|------|
| `ObjectId` | `[u8;32]` opaque | ID global unique ExoFS | `bytes[0..8]` = compteur `u64` LE. `bytes[8..32]` = zéro. `is_valid()` rejette tout autre. |
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
| R1 | `drivers/` | device_server + IPC | servers Ring 1 |

### 2.2 Lock Ordering

> **Justification C3-21** : FS libère ses locks avant IPC car **IPC est bloquant**. Tenir un spinlock FS (non-préemptif) pendant une opération IPC bloquante = inversion de priorité + latence noyau non bornée.

| Niveau | Module | Règle |
|--------|--------|-------|
| 1 (acquérir en premier) | Memory | Jamais tenus lors d'appels aux couches supérieures |
| 2 | Scheduler | RunQueue locks (ordre cpu\_id croissant). Jamais tenus pendant IPC. |
| 3 | Security | CapTable locks — jamais tenus lors d'appels IPC ou FS |
| 4 | IPC | Channel/endpoint/SHM — jamais tenus lors d'appels FS |
| 5 (acquérir en dernier) | FS | **DOIT relâcher AVANT tout appel IPC** (IPC est bloquant) |

### 2.3 Constantes MAX\_CPUS / MAX\_CORES

> **V5-C-03** : `MAX_CORES_RUNTIME` = valeur CPUID au boot = runtime. **Impossible** en `static_assert!`. → `assert!()` à l'étape 14 du boot.

| Constante | Valeur | Fichier | Note |
|-----------|--------|---------|------|
| `SSR_MAX_CORES_LAYOUT` | 256 | `exo-phoenix-ssr/lib.rs` | Compile-time FIXE — dimensionne offsets SSR |
| `MAX_CORES_RUNTIME` | CPUID (≤256) | `arch/boot/early_init.rs` step 14 | `assert!(≤LAYOUT)` à step 14 |
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
│   ├── fpu.rs          XSAVE/XRSTOR + MXCSR + x87 FCW
│   └── tsc.rs          TSC calibration + tsc_sync() cross-core
├── apic/
│   └── ipi.rs          IPI 0xF1=reschedule, 0xF2=TLB shootdown
│                       IDT 0xF3=ExoPhoenix freeze request (V5-C-04)
│                       Kernel B → IPI Local APIC → exo_shield handler
└── memory_iface.rs     register_tlb_ipi_sender() — APRÈS memory/ init step 11
```

#### 3.1.1 Séquence boot — 18 étapes

| Étape | Action | Règle |
|-------|--------|-------|
| 1–3 | Bootstrap ROM, detect boot protocol, parse E820/UEFI | |
| 4 | EmergencyPool (`.bss` statique — `FRAMES=256` + `WAITNODES=256`) | **EMERGENCY-01** |
| 5–7 | Bitmap bootstrap, paging+KPTI, GDT/IDT (réserve `0xF0-0xFF` pour IPI + `0xF3` ExoPhoenix) | |
| 8–9 | BSP kernel stack + per-CPU GS, CPUID detection | |
| 10 | Local APIC + I/O APIC + IOMMU (VT-d si disponible) | |
| **11** | **Sous-système mémoire complet** (bitmap→buddy→SLUB→per-CPU→NUMA) | EMERGENCY-01 first |
| 12 | KERNEL\_AS + protections (NX/SMEP/SMAP/PKU) | |
| **13** | **Enregistrer IPI TLB sender** auprès de `memory/` ← APRÈS buddy (step 11) | MEM-01 |
| 14 | Parser ACPI (MADT, HPET, SRAT) + calibrer TSC | |
| 14\* | `assert!(MAX_CORES_RUNTIME ≤ SSR_MAX_CORES_LAYOUT)` ← runtime (V5-C-03) | SMP check |
| 14.5 | Allouer per-AP kernel stacks via buddy + init PerCpuData | SMP requis |
| **15** | Init scheduler + RunQueues → démarrer APs (INIT/SIPI) → **spin-wait** | CVE-EXO-001 |
| 16 | Init IPC + SHM pool | |
| 17 | Monter ExoFS + `boot_recovery_sequence` | |
| **18** | `security::init()` → `SECURITY_READY.store(true)` ← **APs franchissent spin-wait** | CVE-EXO-001 |

> **Note** : L'`EmergencyPool` est un tableau `.bss` statique inclus dans le binaire kernel — aucune allocation buddy nécessaire pour l'initialiser (EMERGENCY-01).

---

### 3.2 scheduler/ — TCB v6 (256B)

```
scheduler/
├── core/
│   ├── task.rs         TCB 256B v6 — cr3_phys ajouté (V6-C-01)
│   ├── switch.rs       context_switch() — sauvegarde rax..r14 + r15 + CR3
│   ├── preempt.rs      MAX_CPUS=64 ⚠️ (cible: 256 — Phase 0)
│   ├── runqueue.rs     PerCpuRunQueue intrusive via rq_next/rq_prev TCB [240/248]
│   │   // rq_next/rq_prev = null quand BLOCKED (V5-C-02)
│   │   // WaitNode = structure SÉPARÉE depuis EmergencyPool
│   ├── pick_next.rs    pick_next_task() O(1)
│   └── kthread.rs      create_kernel_thread(entry_fn, priority)
├── fpu/
│   ├── lazy.rs         #NM handler — séquence lazy FPU v6 (V6-C-02)
│   └── save_restore.rs xsave64() / xrstor64() FFI
├── policies/           cfs.rs, rt.rs, deadline.rs, idle.rs
├── smp/                topology.rs, affinity.rs, migration.rs, load_balance.rs
├── timer/ energy/ ai_guided/
└── asm/switch_asm.s    CR3 + 15 GPRs (rax..r14 + r15) + MXCSR + FCW
```

#### TCB Layout v6 — 256 octets exactement

> **V6-C-01** : `cr3_phys` (8B) ajouté dans l'espace `pkrs+_pad` ([48..63]). `switch_asm.s` lit/écrit `CR3` depuis ce champ. `pkrs(4B)+_pad(4B)+cr3_phys(8B)=16B` — TCB reste 256B.

| Champ | Offset | Taille | Rôle v6 |
|-------|--------|--------|---------|
| `cap_table_ptr` | [0] | 8 B | `*const CapTable` du processus (PARTAGÉ par threads) — CL1 |
| `kstack_ptr` | [8] | 8 B | RSP Ring 0 sauvegardé au switch kernel→kernel |
| `tid` | [16] | 8 B | Thread ID global unique |
| `sched_state` | [24] | 8 B | AtomicU8 : RUNNING/BLOCKED/ZOMBIE/DEAD |
| `fs_base` | [32] | 8 B | MSR 0xC0000100 — TLS userspace (pthread, Rust std) |
| `user_gs_base` | [40] | 8 B | MSR 0xC0000101 userspace sauvegardé AVANT SWAPGS |
| `pkrs` | [48] | 4 B | Protection Key Rights Supervisor (Intel PKS 32b) |
| `_pad` | [52] | 4 B | Alignement |
| **`cr3_phys`** | **[56]** | **8 B** | **Adresse physique PML4 — lu/écrit par `switch_asm.s` (V6-C-01)** |
| `rax..r14` (14 GPRs) | [64] | 112 B | `rax,rbx,rcx,rdx,rsi,rdi,rbp,r8..r14` — 14×8B |
| `r15` | [176] | 8 B | 15ème GPR — callee-saved ABI SysV |
| `[pad align]` | [184] | 8 B | Alignement — zone GPR = 128B total |
| `rip` | [192] | 8 B | Instruction pointer Ring 3 — point de reprise |
| `rsp_user` | [200] | 8 B | Stack pointer Ring 3 — distinct de `kstack_ptr` |
| `rflags` | [208] | 8 B | EFLAGS étendu |
| `cs/ss` | [216] | 8 B | Segment selectors (`cs << 32 | ss`) |
| `cr2` | [224] | 8 B | Page fault addr — diagnostic ExoPhoenix Kernel B. **JAMAIS restauré.** |
| `fpu_state_ptr` | [232] | 8 B | `*mut XSaveArea` — null si FPU jamais utilisé. Libéré via `release_thread_resources()`. |
| `rq_next` | [240] | 8 B | RunQueue intrusive — null si BLOCKED/hors RunQueue |
| `rq_prev` | [248] | 8 B | RunQueue intrusive — null si BLOCKED/hors RunQueue |

> **Vérification** : `rq_prev[248] + 8B = byte 255`. Champs [0..255] = **256B**. 4 cache lines de 64B. `#[repr(C, align(64))]`.
>
> **Calcul par cache lines** :
> - CL1 [0..63] : `8+8+8+8+8+8+4+4+8 = 64B` ✓
> - CL2+3 [64..191] : `112+8+8 = 128B` ✓
> - CL4 [192..255] : `8+8+8+8+8+8+8+8 = 64B` ✓
> - **Total = 256B** ✓

#### FPU Lazy — Séquence v6 (V6-C-02)

> **Bug corrigé** : `xrstor64` sur une `XSaveArea` fraîchement allouée (zeroed) = `#GP` garanti. Le CPU exige un en-tête XSTATE\_BV valide à l'offset 512. Solution : `fninit` pour la première utilisation, `xrstor64` uniquement sur état préalablement sauvegardé.

```
#NM exception → sched_fpu_handle_nm()
  1. CLTS (clear CR0.TS → autoriser instructions FPU)
  2. Si fpu_state_ptr == null:
       alloc_fpu_state() via global_alloc
       // XSaveArea taille = CPUID leaf 0Dh sub-leaf 0 (V6-C-06)
       Stocker ptr dans tcb.fpu_state_ptr
       fninit()              ← V6-C-02 : init x87 FPU en état propre
       vzeroupper()          ← zeroise registres AVX (si disponible)
       xsave64(fpu_state_ptr) ← sauvegarder état clean comme baseline
  3. Sinon (fpu_state_ptr non-null = état sauvegardé):
       xrstor64(fpu_state_ptr) ← restaurer état précédent
```

> **XSaveArea size (V6-C-06)** : La taille de la zone XSAVE dépend des extensions CPU activées (SSE, AVX, AVX-512, AMX…). Elle est reportée par `CPUID leaf 0Dh, sub-leaf 0, ECX`. `alloc_fpu_state()` doit utiliser cette valeur détectée au boot, pas une taille fixe.

---

### 3.3 process/ — exec() v6 (V6-C-03)

> **V6-C-03** : Le signal mask doit être **hérité** du processus appelant (conformité POSIX IEEE 1003.1). `ALLSIGS_UNBLOCKED` en v5 était incorrect. Les pending signals sont préservés conformément au comportement ExoOS-spécifique.

```
process/
├── exec.rs   do_exec() — séquence finale v6 :
│   1. verify_cap(EXEC) + is_valid(object_id) + ObjectKind != Secret
│   2. mask_all_signals_manual()
│      // Masque manuellement TOUS les signaux (sans RAII restaurateur)
│   2.5 signal_queue.flush_all_except_sigkill()
│       // Flush les signaux pendants (comportement ExoOS défini)
│       // Note: POSIX préserve les pending — ExoOS choisit de les flush (partiel)
│   3. reset_signal_handlers_to_sdf()
│       // Réinitialise handlers → SIG_DFL (AVANT reset du mask)
│   4. load_elf(object_id)
│   5. reset_tcb_context()
│       // fs_base, user_gs_base, cr3_phys
│       tcb.signal_mask = CALLER_SIGNAL_MASK  ← V6-C-03 : HÉRITÉ, pas ALLSIGS_UNBLOCKED
│   6. return_to_new_userspace()
│
├── exit.rs   do_exit() + thread_exit() → release_thread_resources(tcb)
│   // release_thread_resources(tcb: &mut TCB) {
│   //     if let Some(ptr) = tcb.fpu_state_ptr.take() {
│   //         dealloc(ptr, Layout::from_size_align(xsave_size, 64).unwrap())
│   //     }  ← taille = CPUID 0Dh (V6-C-06)
│   //     tcb.rq_next = null; tcb.rq_prev = null;
│   // }
│
├── fork.rs   shadow-copy RCU + TLB flush parent
├── signal/   delivery.rs, handlers.rs, sigreturn.rs
└── wait.rs   wait4(), waitpid()
```

---

### 3.4 security/ et fs/exofs/

```
security/
├── mod.rs              SECURITY_READY: AtomicBool (false → true step 18)
├── capability/
│   ├── verify.rs       verify() — À implémenter constant-time (crate subtle no_std)
│   ├── table.rs        CapTable partagée par threads (cap_table_ptr dans TCB)
│   └── rights.rs       Rights bitflags 14 droits
├── access_control/
│   └── check.rs        check_access() = wrapper → appelle verify() internement
└── crypto/
    └── rng.rs          fill_random() — RDRAND + ChaCha20 CSPRNG

fs/exofs/
├── syscall/            500-518 ExoFS + 519=réservé (sys_ni_syscall)
├── storage/
│   └── virtio_adapter.rs  PCI MMIO QEMU 0x1000_0000 #[cfg(qemu_only)]
├── epoch/
│   ├── epoch_record.rs    TODO:517 métadonnées rollback
│   └── gc/                GC → create_kernel_thread() Phase 4
├── path/path_index.rs     SipHash-2-4 keyed (mount_secret_key depuis rng)
├── crypto/                RustCrypto no_std Ring 0 (Cargo.toml ✅)
├── audit/ring_buffer.rs   AUDIT-RING-SEC lock-free
└── vfs_compat/
```

> ⚠️ **IOMMU mode dégradé** : Déploiement de drivers Ring 1 SANS IOMMU = **mode dégradé de sécurité**. Un driver Ring 1 compromis (ou device malveillant via PCIe) peut accéder à toute la mémoire physique, contournant le modèle Zero Trust. En production, `device_server` DOIT attribuer des IOMMU domains restrictifs (IOVAs uniquement, jamais adresses physiques directes).

---

## 4. ExoPhoenix — SSR Layout v6

> **SSR à 0x0100\_0000–0x0100\_FFFF (64 KiB). Début exact Zone DMA32. Déclarée e820.**

| Offset | Taille | Contenu |
|--------|--------|---------|
| +0x0000 | 64 B | **HEADER** : `MAGIC(8B)` + `HANDOFF_FLAG(8B)` + `LIVENESS_NONCE(8B)` + `SEQLOCK(8B)` + `pad(32B)` |
| +0x0040 | 64 B | CANAL COMMANDE B→A (align 64B) |
| +0x0080 | 16 384 B | **FREEZE ACK PER-CORE** — `SSR_MAX_CORES_LAYOUT=256` × 64B |
| +0x4080 | 16 384 B | **PMC SNAPSHOT PER-CORE** — 256 × 64B (EVTSEL0..3 + CTR0..3) |
| +0x8080 | 16 256 B | **EXTENSIONS RESERVED** (~16 KiB) : `+0x8080–0x9FFF` compteurs, `+0xA000–0xBFFF` trace buffer |
| +0xC000 | 8 192 B | **LOG AUDIT B** — append-only, RO pour Kernel A. Protection PKU/PKS recommandée. |
| +0xE000 | 8 192 B | **MÉTRIQUES PUSH A→B** — rate-limited, overflow silencieux |
| **TOTAL** | **65 536 B** | **= 64 KiB ✓**. `SSR_MAX_CORES_LAYOUT=256`. `SSR_LAYOUT_MAGIC` validé au boot A et B. |

> **Vérification** : `64+64+16384+16384+16256+8192+8192 = 65536B = 64KiB` ✓
>
> **IDT 0xF3** (V5-C-04) : Kernel B → IPI Local APIC → vecteur 0xF3 → handler `exo_shield`. Réservé dans IDT au step 7 du boot.

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
    ├── lib.rs          #![no_std] — pub use cap, error, object_id, ipc_msg, fixed_string
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
    └── phoenix.rs      phoenix_query(520) → PhoenixEvent
                        phoenix_notify(521, AllReady) → signal gel Kernel B
```

### 5.5 libs/exo-phoenix-ssr/

```rust
// libs/exo-phoenix-ssr/src/lib.rs — SOURCE UNIQUE PARTAGÉE A↔B
pub const SSR_LAYOUT_MAGIC:       u64   = 0x5353525F4558_4F53; // "SSR_EXOS"
pub const SSR_BASE_PHYS:          u64   = 0x0100_0000;
pub const SSR_SIZE:               usize = 0x10000;             // 64 KiB
pub const SSR_MAX_CORES_LAYOUT:   usize = 256;                 // FIXE compile-time
pub static MAX_CORES_RUNTIME: AtomicU32 = AtomicU32::new(0);  // CPUID boot
pub const SSR_FREEZE_ACK_OFFSET:  usize = 0x0080;
pub const SSR_PMC_OFFSET:         usize = 0x4080;
pub const SSR_EXTENSIONS_START:   usize = 0x8080;
pub const SSR_EXTENSIONS_SIZE:    usize = 0x3F80; // 16 256 B
pub const SSR_LOG_AUDIT_OFFSET:   usize = 0xC000; // 8 KiB
pub const SSR_METRICS_OFFSET:     usize = 0xE000; // 8 KiB

pub const fn freeze_ack_offset(apic_id: usize) -> usize { SSR_FREEZE_ACK_OFFSET + apic_id * 64 }
pub const fn pmc_snapshot_offset(apic_id: usize) -> usize { SSR_PMC_OFFSET + apic_id * 64 }

// Vérification runtime au boot step 14 (V5-C-03) :
// assert!(MAX_CORES_RUNTIME.load(Relaxed) as usize <= SSR_MAX_CORES_LAYOUT);
```

---

## 6. Arborescences Ring 1 — Servers

### 6.1 Ordre de démarrage canonique (12 étapes) — V6-C-04

> **Correction V6-C-04** : 12 étapes. `scheduler_server` = étape **11**, `exo_shield` = étape **12**. (Le changelog V5-C-09 mentionnait 12/13 par erreur.)

| Étape | Server | PID | Condition | Priorité |
|-------|--------|-----|-----------|----------|
| 1 | `ipc_broker` | 2 | Rien — premier absolu (PID 2 assigné kernel) | **P1 CRITIQUE** |
| 2 | `memory_server` | dyn | ipc\_broker disponible | **P1 CRITIQUE** |
| 3 | `init_server` | 1 | ipc\_broker + memory\_server + **BootInfo struct** | **P1 CRITIQUE** |
| 4 | `vfs_server` | 3 | init\_server + ExoFS kernel monté | P2 HAUTE |
| 5 | `crypto_server` | 4 | vfs\_server disponible — **SEUL avec RustCrypto Ring 1** | P2 HAUTE |
| 6 | `device_server` | dyn | ipc\_broker + memory\_server — **AVANT tout driver** | P2 HAUTE |
| 7 | `virtio-block` | dyn | device\_server disponible | **P1 CRITIQUE** |
| 8 | `virtio-net` | dyn | device\_server disponible | P2 HAUTE |
| 9 | `virtio-console` | dyn | device\_server disponible | P2 HAUTE |
| 10 | `network_server` | dyn | virtio-net disponible | P3 |
| **11** | `scheduler_server` | dyn | init\_server disponible | P3 |
| **12** | `exo_shield` | dyn | Phase 3 ExoPhoenix stable **UNIQUEMENT** | Après Phase 3 |

### 6.2 servers/ipc_broker/ — PID 2

```
src/
├── main.rs         verify_cap_token() → enregistrement → boucle IPC
├── registry.rs     ServiceName → (pid:u32, CapToken)
├── directory.rs    Lookup service → reply via sender_pid
├── protocol.rs     Register{name, cap}, Lookup, Deregister
└── persistence.rs  Dump registry → ExoFS (ObjectId) — survie au crash
```

### 6.3 servers/init_server/ — PID 1

```
src/
├── main.rs           fn _start(boot_info_phys: usize) -> !
│                     // BootInfo struct, pas argv[] textuel
│                     let bi = unsafe { &*(boot_info_phys as *const BootInfo) };
│                     assert!(bi.validate()); verify_cap_token(&bi.ipc_broker_cap, ...);
├── supervisor.rs     Restart policy — écoute ChildDied
├── service_table.rs  Services ordonnés avec restart policy
├── sigchld_handler.rs Reçoit ChildDied IPC → supervisor.rs (SRV-01)
├── isolation.rs      PrepareIsolation → flush → ExoFS → PrepareIsolationAck
└── protocol.rs       Start, Stop, Status, Restart, ChildDied, PrepareIsolation, Ack
```

### 6.4 servers/vfs_server/ — PID 3

```
src/
├── main.rs         verify_cap_token() + boucle IPC
├── mount.rs        Mount namespace — PathBuf/ExoFS objectId
├── path_resolver.rs PathBuf → ObjectId (wrappe SYS_EXOFS_PATH_RESOLVE)
├── pseudo_fs.rs    /proc /sys /dev
├── fd_table.rs     Table FDs par processus
├── isolation.rs    PrepareIsolation → flush mount_table + fd_table → ExoFS → ack
└── protocol.rs     Open{path:PathBuf}, Close, Read, Write, Stat, Readdir, Isolation
```

### 6.5 servers/memory_server/

```
src/
├── main.rs         verify_cap_token(CapabilityType::MemoryServer) + boucle IPC
├── allocator.rs    Interface buddy kernel — Alloc, Free
├── mmap.rs         MapShared(ObjectId), MapAnon, Unmap, Protect
│                   ⚠ MapShared retourne offset virtuel — JAMAIS adresse physique
├── region_table.rs Registre régions allouées par processus
├── isolation.rs    PrepareIsolation → flush region_table → ExoFS → ack
└── protocol.rs     Alloc, Free, MapShared, Protect, PrepareIsolation, Ack
```

### 6.6 servers/crypto_server/ — PID 4 (SEUL avec RustCrypto Ring 1)

```
src/
├── main.rs         verify_cap_token() + boucle IPC — NE JAMAIS exposer clés brutes
├── rng.rs          CSPRNG — RDRAND + ChaCha20
├── key_store.rs    Stockage clés en mémoire chiffrée
├── session.rs      Sessions chiffrées inter-processus
├── hash.rs         Blake3 → ObjectId — SEULE source d'ObjectId Ring 1 (SRV-04)
├── entropy.rs      RDRAND(64B) → entropy_pool → HKDF → master_key
├── isolation.rs    PrepareIsolation → sealed blob :
│                   ChaCha20Poly1305(clé dérivée master_key, clé active) → blob
│                   EXOFS_WRITE → ObjectId → PrepareIsolationAck
└── protocol.rs     GenKey, Encrypt, Decrypt, Hash{data}→ObjectId, GenRandom, Isolation
```

### 6.7 servers/device_server/

```
src/
├── main.rs           verify_cap_token() + boucle IPC
├── pci_registry.rs   Registre PCI : bus/device/func → driver assigné
├── lifecycle.rs      Start/Stop/Reset driver Ring 1. FLR PCI sur reset.
├── probe.rs          Découverte et association device↔driver
├── irq_router.rs     Routage IRQs hardware vers drivers Ring 1 via IPC
├── claim_validator.rs Valide : CapToken valide + PciId autorisé + libre (CAP-02)
└── protocol.rs       Probe, Claim{device_id, driver_cap, nonce}, Release, IrqNotify
```

### 6.8 servers/scheduler_server/ — SANS isolation.rs

> **Sans `isolation.rs`** — état transient. Relancé depuis zéro après cycle ExoPhoenix.

```
src/
├── main.rs         verify_cap_token() + boucle IPC
├── policy.rs       CFS adapté Ring 1/3 — priorités, nice values
├── thread_table.rs Table threads actifs avec priorités
└── protocol.rs     SetPriority, Yield, GetStat
```

### 6.9 servers/network_server/ — SANS isolation.rs

```
src/
├── main.rs         verify_cap_token() + boucle IPC
├── socket.rs       Table sockets TCP/UDP par processus
├── routing.rs      Table de routage, ARP
├── driver_iface.rs Interface vers virtio-net via device_server
└── protocol.rs     Socket, Connect, Bind, Send, Recv, Close
# smoltcp default-features=false features=['socket-tcp','socket-udp']
```

### 6.10 servers/exo_shield/ — APRÈS Phase 3 ExoPhoenix

```
src/
├── main.rs              Enregistre handler IDT 0xF3 (après SECURITY_READY)
├── irq_handler.rs       Handler IDT 0xF3 : reçoit Freeze Request de Kernel B
│                        → broadcast PrepareIsolation aux servers abonnés
├── event_relay.rs       Relais événements ExoPhoenix vers Ring 1
├── isolation_notify.rs  Collecte PrepareIsolationAck → phoenix_notify(521, AllReady)
├── subscription.rs      Registry servers abonnés aux événements ExoPhoenix
└── protocol.rs          PhoenixEvent, PrepareIsolation, PrepareIsolationAck
```

---

## 7. Arborescences Ring 1 — Drivers

> ⚠️ **Ordre critique** : `device_server` (étape 6) **AVANT** tout driver (étapes 7-9).

### 7.1 drivers/virtio-block/ — PRIORITÉ 1

```
src/
├── main.rs         Claim sécurisé (driver_cap+nonce) → device_server + init Virtio + boucle
├── virtio.rs       Feature negotiation, device reset, config space
├── queue.rs        VirtQueue split ring : descripteur + available + used ring
├── block.rs        VIRTIO_BLK_T_IN/OUT — lecture/écriture secteurs 512B
└── exofs_backend.rs Enregistrement comme backend ExoFS
```

### 7.2 drivers/virtio-net/ — PRIORITÉ 2

```
src/
├── main.rs     Claim sécurisé + init Virtio + boucle receive/transmit
├── virtio.rs   Feature negotiation, MAC, MTU
├── queue.rs    RX queue + TX queue (deux VirtQueues séparées)
└── net.rs      Interface vers network_server via IPC
```

### 7.3 drivers/virtio-console/ — PRIORITÉ 3

```
src/
├── main.rs     Claim sécurisé + init Virtio console + boucle read/write
├── virtio.rs   Protocole Virtio console — port 0 (stdin/stdout)
└── console.rs  Interface vers /dev/console dans vfs_server
```

### 7.4 CI Enforcement + PHX-03

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

# PHX-03 : enregistrer ELF dans ExoFS (build/register_binaries.sh)
for binary in ipc_broker init_server vfs_server memory_server crypto_server \
              device_server scheduler_server network_server \
              virtio_block virtio_net virtio_console; do
  HASH=$(b3sum --no-names target/release/$binary)
  exofs-deploy register --binary $binary --hash $HASH
done
```

---

## 8. BootInfo v6

```rust
#[repr(C)]  // ABI déterministe — layout fixe
pub struct BootInfo {
    pub magic:              u64,      // 0x424F4F545F494E46 ("BOOT_INF")
    pub version:            u32,      // BOOT_INFO_VERSION = 1
    pub _pad:               [u8; 4],  // Alignement CapToken (u64)
    pub ipc_broker_cap:     CapToken, // passé par adresse physique, pas argv[]
    pub ssr_phys_addr:      u64,      // adresse physique SSR ExoPhoenix
    pub nr_cpus:            u32,      // MAX_CORES_RUNTIME
    pub _pad2:              [u8; 4],  // Alignement
    pub memory_bitmap_phys: u64,
    pub memory_bitmap_size: u64,
    pub kernel_heap_start:  u64,      // VirtAddr KERNEL_HEAP_START
    pub kernel_heap_end:    u64,      // VirtAddr KERNEL_HEAP_END
    pub reserved:           [u64; 16], // must be 0
}

impl BootInfo {
    pub fn validate(&self) -> bool {
        self.magic == BOOT_INFO_MAGIC
            && self.version == BOOT_INFO_VERSION
            && self.reserved.iter().all(|&x| x == 0)
    }
}
```

---

## 9. Défauts — État v6

### 9.1 Corrections confirmées

| Défaut | Statut v6 | Note |
|--------|-----------|------|
| LAC-03 Cargo.toml crypto | ✅ CORRIGÉ | CI grep Ring 1 |
| MEM-FUTEX 4096 buckets | ✅ CORRIGÉ | `constants.rs` + SipHash |
| CVE-EXO-004 EmergencyPool | ✅ CORRIGÉ | `.bss` FRAMES=256 + WAITNODES=256 |
| SSR Extensions 16 KiB | ✅ CORRIGÉ v4 | `0x3F80` exact |
| exec() RAII guard | ✅ CORRIGÉ v4 | Masquage manuel |
| exec() signal mask | ✅ CORRIGÉ v6 (V6-C-03) | Hérité du processus appelant |
| AP stacks | ✅ CORRIGÉ v4 | Étape 14.5 |
| fpu_state_ptr leak | ✅ CORRIGÉ v5 | `release_thread_resources()` |
| TCB cr3_phys manquant | ✅ CORRIGÉ v6 (V6-C-01) | `[56]` dans pkrs+\_pad |
| FPU #GP sur zone neuve | ✅ CORRIGÉ v6 (V6-C-02) | `fninit` → `xsave64` au lieu de `xrstor64` |
| §6.1 numérotation | ✅ CORRIGÉ v6 (V6-C-04) | 12 étapes, scheduler=11, shield=12 |
| MAX\_CPUS preempt=64 | ⚠️ Phase 0 | Corriger →256 avant >64 cœurs |

### 9.2 Vulnérabilités P0

| ID | Statut | Action |
|----|--------|--------|
| **LAC-01** | 🔴 À implémenter | `verify()` constant-time — crate `subtle` no\_std |
| **LAC-03** | ✅ CORRIGÉ | `kernel/Cargo.toml` présent |
| **LAC-04** | 🔴 À implémenter | `NONCE_COUNTER` atomique + HKDF nonces XChaCha20 |
| **LAC-06** | 🔴 À implémenter | `key_storage.rs` — Argon2id OWASP (m=65536, t=3, p=4) |
| **CVE-EXO-001** | 🔴 À implémenter | `spin-wait` ASM sur APs avant `SECURITY_READY` |

### 9.3 Problèmes P1 restants

- **MEM-DMA-IRQ** : DMA ISR libère lock PUIS wakeup (SoftIRQ ou bit pending) — Phase 2
- **LOCK-05** : Writeback : `reserve_for_commit(n)` AVANT `EPOCH_COMMIT_LOCK` — Phase 2
- **PREEMPT-BLOCK** : `debug_assert!(preempt_count==0)` avant `block_current()` — Phase 2
- **SCHED-INTRU** : RunQueue intrusive spécifiée (TCB [240/248]) — à implémenter Phase 2
- **IOMMU-01** : IOMMU domains pour drivers Ring 1 (mode dégradé sans) — Phase 2
- **TSC-EDF** : CPUID Invariant TSC + `tsc_sync()` cross-core — Phase 2

---

## 10. Checklist de Conformité v6 (39 checks)

| # | Vérification | Module | Statut |
|---|-------------|--------|--------|
| S-01 | `verify_cap()` avant tout accès ExoFS | `fs/exofs/syscall/*` | 🔴 Obligatoire |
| S-02 | `verify()` constant-time (`subtle`, `ct_eq()`) | `security/capability/verify.rs` | 🔴 À implémenter |
| S-03 | `check_access()` = wrapper de `verify()` | `security/access_control/` | 🔴 Vérifier |
| S-04 | `SECURITY_READY` + APs spin-wait ASM step 15 | `security/mod.rs` + `arch/smp/` | 🔴 À implémenter |
| S-05 | Blake3 AVANT compression | `fs/exofs/crypto/blake3.rs` | 🔴 Obligatoire |
| S-06 | Nonce = `NONCE_COUNTER` + HKDF(object\_id) | `fs/exofs/crypto/xchacha20.rs` | 🔴 À corriger |
| S-07 | Pipeline données→Blake3→LZ4→XChaCha20→disque | `secret_writer.rs` | 🔴 Obligatoire |
| S-08 | `Cargo.toml` kernel : blake3, chacha20poly1305, hkdf, argon2, siphasher | `kernel/Cargo.toml` | ✅ CORRIGÉ |
| S-09 | `ObjectKind::Secret` : BlobId jamais retourné | `get_content_hash.rs` | 🔴 Obligatoire |
| S-10 | `exec()` sur Secret = `Err(NotExecutable)` | `process/exec.rs` | 🔴 Obligatoire |
| S-11 | `exec()` : masquage manuel + flush signaux + mask hérité | `process/exec.rs` | ✅ CORRIGÉ v6 |
| S-12 | PathIndex = SipHash-2-4 keyed depuis `rng::fill_random()` | `path_index.rs` | 🔴 Obligatoire |
| S-13 | Quota vérifié AVANT allocation | `quota_enforcement.rs` | 🔴 Obligatoire |
| S-14 | AUDIT-RING-SEC : toutes opérations loggées | `audit/ring_buffer.rs` | 🔴 Obligatoire |
| S-15 | GET\_CONTENT\_HASH toujours auditée | `get_content_hash.rs` | 🔴 Obligatoire |
| S-16 | key\_storage : Argon2id m=65536 t=3 p=4 + sel 128 bits | `fs/exofs/crypto/` | 🔴 À implémenter |
| S-17 | Cap table fork shadow-copy RCU + rollback | `process/fork.rs` | 🔴 Obligatoire |
| S-18 | `do_exit()` + `thread_exit()` : `release_thread_resources()` | `process/exit.rs` + `thread.rs` | ✅ SPÉCIFIÉ v5 |
| S-19 | RunQueue intrusive via `rq_next/rq_prev` TCB [240/248] | `scheduler/core/runqueue.rs` | ✅ SPÉCIFIÉ v4 |
| S-20 | CI grep Ring 1 : aucun blake3/chacha20 hors crypto\_server | `CI / Makefile` | 🔴 CI |
| S-21 | `debug_assert!` preempt\_count==0 avant `block_current()` | `sync/wait_queue.rs` | 🔴 À ajouter |
| S-22 | DMA ISR : libérer lock AVANT wakeup | `memory/dma/` | 🔴 À corriger |
| S-23 | `SSR_LAYOUT_MAGIC` vérifié par Kernel A et B au boot | `exo-phoenix-ssr` | 🔴 À implémenter |
| S-24 | Chaque server Ring 1 : `verify_cap_token()` en `main.rs` | `servers/*/main.rs` | 🔴 Obligatoire |
| S-25 | Aucun `&str`/`Vec`/`String` dans `protocol.rs` Ring 1 | `servers/*/protocol.rs` | 🔴 CI |
| S-26 | CI `const_assert MAX_CPUS==256` dans `preempt.rs` | `CI / build.rs` | 🟡 Phase 0 |
| S-27 | `EMERGENCY_POOL_SIZE_FRAMES=256` + `WAITNODES=256` (`.bss`) | `memory/physical/frame/pool.rs` | ✅ CORRIGÉ |
| S-28 | Boot : step 11 (memory) AVANT step 13 (register IPI TLB) | `arch/boot/early_init.rs` | ✅ CORRIGÉ v4 |
| S-29 | Drivers Ring 1 : allocations DMA via IOMMU (IOVAs) | `drivers/*/virtio.rs` | 🟡 P1 |
| S-30 | EDF : CPUID Invariant TSC + `tsc_sync()` | `scheduler/policies/deadline.rs` | 🟡 P1 |
| S-31 | `fpu_state_ptr` libéré dans `thread_exit()` ET `do_exit()` | `process/exit.rs` + `thread.rs` | ✅ SPÉCIFIÉ v5 |
| S-32 | BootInfo : `_pad:[u8;4]` + `repr(C)` + `validate()` | `boot/boot_info.rs` | ✅ CORRIGÉ v4 |
| S-33 | `ObjectId::is_valid()` vérifié avant `verify()` | `exo-types/object_id.rs` | ✅ SPÉCIFIÉ v4 |
| S-34 | AP stacks allouées step 14.5 avant INIT/SIPI | `arch/boot/early_init.rs` | ✅ SPÉCIFIÉ v4 |
| S-35 | `assert!(MAX_CORES_RUNTIME ≤ SSR_MAX_CORES_LAYOUT)` step 14 | `arch/boot/early_init.rs` | ✅ CORRIGÉ v5 |
| S-36 | `rq_next/rq_prev = null` quand thread BLOCKED | `scheduler/core/runqueue.rs` | ✅ SPÉCIFIÉ v5 |
| S-37 | PHX-03 : `build/register_binaries.sh` avant premier boot | `build/` | 🔴 CI |
| **S-38** | `exo_shield` : handler IDT 0xF3 enregistré après `SECURITY_READY` | `servers/exo_shield/irq_handler.rs` | 🟡 **Phase 4** |
| S-39 | `scheduler_server` + `network_server` : pas d'`isolation.rs` | `servers/` | ✅ CORRIGÉ v5 |
| **S-40** | `cr3_phys` dans TCB [56] — lu/écrit par `switch_asm.s` | `scheduler/core/task.rs` + `asm/` | ✅ SPÉCIFIÉ v6 |
| **S-41** | FPU 1ère utilisation : `fninit + xsave64` (pas `xrstor64` sur zone neuve) | `scheduler/fpu/lazy.rs` | ✅ SPÉCIFIÉ v6 |
| **S-42** | `XSaveArea` size = CPUID leaf 0Dh sub-leaf 0 (détecté au boot) | `scheduler/fpu/` | ✅ SPÉCIFIÉ v6 |
| **S-43** | `exec()` : signal mask hérité du processus appelant | `process/exec.rs` | ✅ CORRIGÉ v6 |

---

## 11. Feuille de Route — v6 Finale

### Phase 0 — Cohérence immédiate
- `preempt.rs` : `MAX_CPUS` 64 → 256 (S-26)
- `assert!(MAX_CORES_RUNTIME ≤ SSR_MAX_CORES_LAYOUT)` step 14 (S-35)
- CI grep SRV-02 + no\_std + panic=abort (S-20/25)
- `build/register_binaries.sh` — PHX-03 (S-37)

### Phase 1 — Sécurité critique
- `verify()` constant-time via crate `subtle` no\_std (LAC-01)
- Nonces XChaCha20 : `NONCE_COUNTER` + HKDF (LAC-04)
- `key_storage.rs` Argon2id OWASP (LAC-06)
- `SECURITY_READY` spin-wait ASM sur APs (CVE-EXO-001)
- Implémenter `exec()` v6 + `do_exit()` + `thread_exit()` + `release_thread_resources()`
- Implémenter `switch_asm.s` avec sauvegarde `cr3_phys` depuis TCB [56]
- Implémenter FPU lazy v6 (`fninit` sur première utilisation)

### Phase 2 — Robustesse kernel
- RunQueue intrusive : utiliser `rq_next/rq_prev` TCB [240/248]
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
- PCI Config Space scan dans `virtio_adapter.rs`
- AUDIT-RING-SEC : sticky entries + compteur logs perdus
- PKU/PKS pour LOG AUDIT B dans SSR
- `proptest` + `INVARIANTS.md` pour délégation caps

---

*Exo-OS — Architecture Complète — **v6** — Mars 2026*
*6 corrections · 4 cycles revue · Ring 0+1+Drivers complets · 43 checks CI*
