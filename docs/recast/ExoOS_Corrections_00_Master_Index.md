# ExoOS — Index Master des Corrections v2.0
**Round 1 : Z-AI, KIMI, Grok4, Gemini, ChatGPT5, Copilote, MiniMax + Claude**  
**Round 2 : Z-AI, Copilote, ChatGPT5, KIMI, MiniMax + double passe Claude**  
**Total consolidé : CORR-01 à CORR-52 + SRV-05 · Mars 2026**

---

## Décisions d'arbitrage Round 2

### Faux positifs rejetés

| Proposition | Source | Raison du rejet |
|-------------|--------|-----------------|
| EN-01 : CORR-15 condition CR0.TS inversée | MiniMax | **INCORRECT** — `!cr0.contains(TASK_SWITCHED)` teste TS=0 → FPU dans registres CPU → XSAVE nécessaire. Confirmé par Intel Manual §CR0.TS bit 3. MiniMax se contredit en §5.1. |
| EN-03 : SeqCst IommuFaultQueue | MiniMax | Pattern seqlock Release/Acquire est correct pour MPSC. SeqCst = barrières inutiles sur x86_64 TSO. Z-AI §1.3 confirme : heapless no-alloc ✅ |
| IC-02 : MAX_PENDING_ACKS manquant | MiniMax | Déjà défini = `4096` dans Driver Framework v10 ligne `const MAX_PENDING_ACKS: u32 = 4096;` — MiniMax propose 16 ce qui est faux. |
| CORR-33 CapGuard RAII | Z-AI | Inutilement complexe. Règle documentaire "no-cache" suffit. Reformulé en CORR-33. |
| KIMI CORR-34 spin_loop ISR | KIMI | **INTERDIT** en ISR (CORR-04). Spin pour DoS mitigation dans ISR serait catastrophique. |
| KIMI CORR-35 deadlock wait_link | KIMI | Faux positif. `PCI_TOPOLOGY.parent_bridge()` retourne `Option<PciBdf>` par **valeur copie** — read lock libéré avant la boucle yield. Pas de deadlock. |
| KIMI CORR-36 CapToken future replay | KIMI | Hors scope Phase 8. Requiert que l'attaquant contrôle le RNG kernel ou l'atomique generation. |
| KIMI CORR-39 SYS_EXOFS 520-529 | KIMI | **Conflit de plage** — 520-529 = SYS_PHOENIX_*. Voir CORR-35. |

---

## Tableau complet des 52 corrections + SRV-05

### 🔴 Critiques (9)

| ID | Titre | Fichier |
|----|-------|---------|
| CORR-01 | TCB Layout unifié Architecture v7 | 01 |
| CORR-02 | SSR Layout MAX_CORES=256, offsets unifiés | 01 |
| CORR-03 | SSR Header — MAGIC en premier | 01 |
| CORR-04 | Vec\<IpcEndpoint\> en ISR → tableau fixe | 03 |
| CORR-05 | CapabilityType enum `#[repr(C)]` illégal | 06 |
| CORR-06 | EpollEventAbi packed → UB Rust E0793 | 04 |
| CORR-07 | ObjectId::is_valid() exception ZERO_BLOB | 01 |
| CORR-32 | Double Claim PCI + TOCTOU sys_pci_claim | **07** |
| CORR-41 | IOMMU ABA race ISR → pid_at_fault snapshot | **08** |

### 🟠 Majeures (23)

| ID | Titre | Fichier |
|----|-------|---------|
| CORR-08 | masked_since CAS → Release | 03 |
| CORR-09 | BootInfo toujours virtuel | 02 |
| CORR-10 | IPI broadcasts → exclure Core 0 | 02 |
| CORR-11 | FS/GS base rdmsr/wrmsr | 02 |
| CORR-12 | Crypto nonce rollback Phoenix | 05 |
| CORR-13 | VFS sync_fs avant ACK Phoenix | 05 |
| CORR-14 | DMA bus master disable Phoenix | 05 |
| CORR-15 | FPU XSAVE forcé avant gel | 05 |
| CORR-16 | domain_of_pid() manquant | 03 |
| CORR-17 | sender_pid → reply_nonce | 06 |
| CORR-18 | switch_asm.s "15 GPRs" trompeur | 02 |
| CORR-19 | spin_count reset par tentative | 03 |
| CORR-31 | IpcMessage ABI payload 48B guide | **07** |
| CORR-33 | verify_cap_token règle no-cache | **07** |
| CORR-34 | FPU multi-thread cleanup do_exit | **08** |
| CORR-36 | Panic Handler Ring 1 + SRV-01 | **08** |
| CORR-37 | Phoenix freeze timeout Kernel A | **08** |
| CORR-40 | Phoenix restore → ordre redémarrage | **08** |
| CORR-42 | current_time_ms saturating_sub | **08** |
| CORR-44 | copy_file_range quota bypass reflink | **07** |
| CORR-46 | fd_table post-restore stale ObjectIds | **08** |
| CORR-50 | MAX_HANDLERS_PER_IRQ rejet explicite | **09** |
| CORR-51 | BootInfo mappé read-only | **07** |

### ⚠️ Lacunes (16)

| ID | Titre | Fichier |
|----|-------|---------|
| CORR-20 | SYS_EXOFS_* 500-518 mapping | 04 |
| CORR-21 | SRV-03 documenté supprimé | 06 |
| CORR-22 | BlobId = concept uniquement | 04 |
| CORR-23 | IommuDomainRegistry spec | 03 |
| CORR-24 | SeqLock Phase 9 roadmap | 02 |
| CORR-25 | device_server pci/ gdi/ manquants | 06 |
| CORR-26 | CI virtio_block harmonisation | 06 |
| CORR-35 | Phoenix syscalls 520-529 complet | **07** |
| CORR-38 | IRQ Table size 256 documentée | **09** |
| CORR-39 | DMA Map Table bornes mémoire | **09** |
| CORR-43 | SRV-05 ipc_broker persistence | **07** |
| CORR-45 | IpcEndpoint Copy invariant | **07** |
| CORR-47 | IoVec align(8) + validation | **09** |
| CORR-48 | O_DIRECT bounce buffer resp. | **09** |
| CORR-49 | CR0.TS après Phoenix restore | **08** |
| CORR-52 | verify_cap_token spec constant-time | **07** |

### 🔵 Mineures (4 + SRV-05)

| ID | Titre | Fichier |
|----|-------|---------|
| CORR-27 | MAX_CPUS preempt.rs 256 | 02 |
| CORR-28 | Arborescence V3 archiver | 06 |
| CORR-29 | user_gs_base nommage | 01 |
| CORR-30 | FixedString len: u32 | 01 |
| SRV-05 | ipc_broker persistence rule | **07** |

---

## Fichiers (9 fichiers de correction)

```
01_Kernel_Types.md           — CORR-01,02,03,07,29,30
02_Architecture.md           — CORR-09,10,11,18,24,27
03_Driver_Framework.md       — CORR-04,08,16,19,23
04_ExoFS.md                  — CORR-06,20,22
05_ExoPhoenix.md             — CORR-12,13,14,15
06_Servers_Arborescence.md   — CORR-05,17,21,25,26,28
07_IPC_Cap_Security.md       — CORR-31,32,33,35,43,44,45,51,52 + SRV-05
08_Phoenix_Runtime.md        — CORR-34,36,37,40,41,42,46,49
09_IRQ_DMA_Misc.md           — CORR-38,39,47,48,50
```

---
*ExoOS Corrections Index v2.0 — Mars 2026*
