# Benchmark & Rapport de Qualification ExoPhoenix (`P5`–`P8` / `F0`–`F10`)

Ce rapport consigne les mesures empiriques réelles obtenues lors de l'exécution de la suite de qualification indiscutable ExoPhoenix ([`scripts/qemu/run_phoenix_qualification.py`](../scripts/qemu/run_phoenix_qualification.py) / [`tests/exo-compliance/src/phoenix_qualification.rs`](../tests/exo-compliance/src/phoenix_qualification.rs)) sur un noyau compilé en **`--release`** (`opt-level = 3`, `lto = true`, `codegen-units = 1`) en topologie **SMP 4 cœurs (`-smp 4`)**.

---

## 1. Synthèse des Paliers d'Exécution (`P8`)

| Palier | Cible | Statut dans cet environnement | Commande | Verdict |
|---|---|---|---|---|
| **Palier 0 (`F0` / `S0`)** | Binaire de production `EXO_PROFILE=pure` (`make release`) | **Exécuté & Vérifié** | `python3 scripts/qemu/run_security_audit.py --s0-only` | **`PASS`** (`0 phoenix_inject_*`, `0 fault_inject`, `0 FAULT_KERNEL_A`, `0 out 0xf4`, `0 libc.so`) |
| **Palier 1 (`F1`–`F9`)** | QEMU TCG `-smp 4` (`--release`, 4 vCPUs, 2 Gio RAM) | **Exécuté & Vérifié (71 cycles)** | `make qemu-phoenix-faults` / `make qemu-phoenix-smp-cold` / `make qemu-phoenix-gi05` | **`PASS=9 FAIL=0 TOTAL=9`** (code de sortie QEMU `33`) |
| **Palier 2 (`F1`–`F9` KVM)** | QEMU KVM `-accel kvm -cpu host,+invtsc -smp 4 -device intel-iommu,intremap=on` | **Câblé (`SKIPPED_NO_KVM` sur l'hôte WSL2 courant)** | `make qemu-phoenix-kvm` | `/dev/kvm` est présent sur l'hôte WSL2 mais appartient à `root:kvm` (`0660`) sans que l'utilisateur `exo` soit membre du groupe `kvm`. Dès l'ajout au groupe (`sudo usermod -aG kvm $USER`), `make qemu-phoenix-kvm` exécute le Palier 2 complet avec VT-d matériel. |
| **Palier 3 (`F10`)** | Matériel x86_64 réel (Bare-Metal UEFI + IOMMU + NVMe) | **Protocole & Checklist livrés** | Voir [`docs/EXOPHOENIX-BARE-METAL-CHECKLIST.md`](EXOPHOENIX-BARE-METAL-CHECKLIST.md) | Prêt pour qualification sur station x86_64 physique |

---

## 2. Résultats Empiriques Complets — Suite `F0` à `F9` (`--release`, `-smp 4`, 71 cycles)

Journal brut capturé : `target/qemu/phoenix-tcg-smp4.log` (`CPUS=4 ACCEL=TCG FREE_FRAMES=473493 ACTIVE_PIDS=22 HEARTBEAT=918`).

| Oracle | Spécification / Invariant vérifié | Durée test | Mesures & Preuves observées (`[EXO_QUAL]`) | Verdict |
|---|---|---:|---|---|
| **`F0`** | Confinement absolu des fautes (`P5.0` / `S0`) | < 1 s | `0 phoenix_inject_* / 0 fault_inject / 0 out 0xf4 / 0 libc.so` dans le binaire `EXO_PROFILE=pure`, appel non autorisé sans `CAP_PHOENIX_FAULT_INJECT` rejeté avec `-EPERM (-1)` | **`PASS`** |
| **`F1`** | Survie Ring 0 non-coopérative (`#UD` panic, deadlock `cli` brisé par `#NMI` matériel sur `IST_NMI`, auto-guérison physique `.text` via `CR0.WP=0` depuis le Golden RAM Mirror RO) | 31 ms | `panic_ud_recovered=1 nmi_ist_deadlock_broken=1 text_phys_healed=1 ledger_entries=54` (`HEALED=1` sur `CYCLE=3`) | **`PASS`** |
| **`F2`** | Gel SMP réel 4 cœurs (`VEC_EXOPHOENIX_FREEZE = 0xF0`, `VEC_EXOPHOENIX_TLB = 0xF2`) & timeout borné 100 ms `CORR-33` (`0xDEAD_0001` / `DEGRADED=1`) | 113 ms | `online_cpus=4 nominal_peer_acks=3 corr33_stalled_ap=2 corr33_timeout_us=100950 post_recovery=OK` | **`PASS`** |
| **`F3`** | **Cold Respawn destructif (`SIGKILL` + `wait4` + `R0 -> R1 -> R2` + `PHX-03` BLAKE3 + `T20-001`)** + reconvergence asynchrone Strate 2 (`TCP 127.0.0.1` + Wayland `/run/wayland-0`) | 1 070 ms | `accel=TCG s0_us=1142 s1_cold_sla_us=642069 s2_reconverge_wall_ms=1055 crypto=10->23 mem=3->24 vfs=5->25 dev=6->26 comp=18->29 tcp=OK wayland=OK` | **`PASS`** |
| **`F4`** | Intégrité FPU / SSE2 / XMM (`CORR-15`) sur 4 threads concurrents (2 actifs + 2 dormants `CR0.TS=1`) à travers 10 bascules SMP | 147 ms | `threads=4 active=2 lazy=2 switches=10 total_simd_checks=613657 corrupted_bits=0` | **`PASS`** |
| **`F5`** | Cohérence transactionnelle ExoFS triple-slot `A/B/C` $O(1)$ (`CORR-13`, `EPOCH-01..10`) entre barrières NVMe 1/2 et 2/3 + intégrité fichier 2 Mio après `COLD_RESPAWN` de `vfs_server` | 992 ms | `file_2mib_intact_post_cold_vfs=1 mid_epoch_barrier_1_2=O(1)_PASS mid_epoch_barrier_2_3=O(1)_PASS` (`CYCLE=18` `COLD_RESPAWN` en `334 829 µs`) | **`PASS`** |
| **`F6`** | Unicité des nonces AEAD XChaCha20-Poly1305 (`CORR-12`), révocation des clés pré-Phoenix et rejet d'usurpation Ring 3 `PHOENIX_WAKE_ENTROPY` (`T20-001`) | 48 ms | `aead_nonce_distinct=1 pre_phoenix_key_revoked=1 ring3_ipc_manage_denied=-38 ring3_spoof_send_rc=-7 ledger=112` | **`PASS`** |
| **`F7`** | Quiescence DMA / IOMMU (`CORR-14`, `DMA-0`, `FIX-104`) : file lock-free 64 événements (`0` perte) + faute DMAR simulée -> `SIGKILL` immédiat du processus fautif + intégrité `.text` Kernel A | 80 ms | `fault_queue_64_zero_drop=1 dmar_victim_sigkill=1 victim_pid=37 intel_vtd_hw=0 pci_devices_ok=1` | **`PASS`** |
| **`F8`** | Anti-tempête IRQ (`FIX-108..113`), remise à zéro `reset_all_masked_since()`, blacklistage `MAX_OVERFLOWS` avec EOI garanti, ré-enregistrement pilote + soak test 10 s post-bascule | 10 024 ms | `masked_since_reset=1 storm_blacklist_eoi=1 counter_reset_on_reregister=1 soak_ms=10020 soak_ops=124 heartbeat=1542` | **`PASS`** |
| **`F9`** | Révocation `ExoVeil` (`V5-8`), immutabilité de la zone P0 d'`ExoLedger` sous 200 tentatives de saturation + **50 cycles consécutifs** (`SOFT_FORCE`, `CRASH_NMI`, `COLD_RESPAWN`) sans fuite de frames ni de PIDs | 1 255 ms | `exoveil_revoked=1 p0_200_writes_intact=1 cycles=50 free_frames_before=472703 free_frames_after=472752 frame_delta=49 pids_before=22 pids_after=22 pid_delta=0 ledger_verified=112` | **`PASS`** |

---

## 3. Analyse Détaillée des Latences par Mode (`--release`, `-smp 4`)

Toutes les mesures ci-dessous proviennent des jalons TSC sérialisés (`rdtsc_begin` / `rdtscp_end`, `t0..t6`) persistés dans le SSR v7 à `+0xC000` et émis sur `[EXO_PHOENIX_SLA]` lors du run `-smp 4` de 71 cycles :

### 3.1 Mode `COLD_RESPAWN` (`F3`, `F5`, `F9` — `SIGKILL` réel + `wait4` + `PHX-03` BLAKE3 + respawn parallèle `R0 -> R1 -> R2` + `T20-001`)

| Cycle | `S0_FREEZE_US` (`t0 -> t1`) | `S0_FORGE_US` (`t1 -> t2`) | `S0_TOTAL_US` (`t0 -> t3`) | `S1_RESPAWN_US` (`t3 -> t5`) | **`S1_TOTAL_US` (`t0 -> t5`, SLA $\le 1\text{ s}$)** | `S1_ONLINE` | Conformité SLA $\le 1\,000\,000\text{ µs}$ |
|---|---:|---:|---:|---:|---:|---:|---|
| **`CYCLE=7` (`F3`)** | 403 µs | 323 µs | 1 142 µs (1,14 ms) | 640 927 µs (640,93 ms) | **642 069 µs (642,07 ms)** | `7/7` | **OUI (< 1 000 ms même sous TCG `-smp 4`)** |
| **`CYCLE=18` (`F5`)** | 336 µs | 529 µs | 1 345 µs (1,35 ms) | 333 484 µs (333,48 ms) | **334 829 µs (334,83 ms)** | `7/7` | **OUI (< 1 000 ms même sous TCG `-smp 4`)** |
| **`CYCLE=46` (`F9`)** | 559 µs | 239 µs | 1 114 µs (1,11 ms) | 442 684 µs (442,68 ms) | **443 798 µs (443,80 ms)** | `7/7` | **OUI (< 1 000 ms même sous TCG `-smp 4`)** |

- **Observation clé** : Même sous émulation logicielle QEMU TCG 4 cœurs (`-smp 4`), la destruction complète par `SIGKILL` des 4 serveurs de Strate 1 (`crypto_server`, `memory_server`, `vfs_server`, `device_server`) et des 3 serveurs de Strate 2 (`network_server`, `scheduler_server`, `exo_compositor`), suivie de la vérification cryptographique BLAKE3 (`PHX-03`), du respawn parallèle en vagues `R0 -> R1 -> R2` avec de nouveaux PIDs et du reseed CSPRNG `T20-001`, respecte le **SLA strict $\le 1\text{ s}$ (`334,8 ms` à `642,1 ms`)**.
- La reconvergence asynchrone complète de la Strate 2 (`t5 -> t6`, incluant le redémarrage d'`exo_compositor` avec nouveau PID, le rétablissement du socket Wayland `/run/wayland-0` vérifié par `wl_display::sync` et un aller-retour TCP `127.0.0.1`) s'achève en **`1 055 ms`** temps mur hors chemin critique du SLA Strate 1.

### 3.2 Mode `CRASH_NMI` (`F1a` `#UD`, `F1b` deadlock `cli` + `#NMI` sur `IST_NMI`, `F1c` auto-guérison physique `.text` `HEALED=1`)

| Cycle | Déclencheur | `S0_FREEZE_US` | `S0_FORGE_US` | `S0_TOTAL_US` | `CRYPTO_US` | **`S1_TOTAL_US` (`t0 -> t5`)** | `HEALED` |
|---|---|---:|---:|---:|---:|---:|---:|
| **`CYCLE=1` (`F1a`)** | Exception matérielle Ring 0 `#UD` (`ud2`) | 2 373 µs | 3 645 µs | 6 857 µs | 3 802 µs | **10 911 µs (10,91 ms)** | `0` |
| **`CYCLE=2` (`F1b`)** | Deadlock `cli` brisé par `#NMI` matériel sur `IST_NMI` (3 strikes P0) | 278 µs | 166 µs | 639 µs | 543 µs | **1 196 µs (1,20 ms)** | `0` |
| **`CYCLE=3` (`F1c`)** | Corruption physique `.text` -> réécriture RAM `CR0.WP=0` depuis Golden Mirror | 502 µs | 901 µs | 1 703 µs | 1 614 µs | **3 330 µs (3,33 ms)** | **`1`** |

### 3.3 Mode `CORR-33` (`F2b` — Cœur AP 2 bloqué avec `IF=0` refusant `FREEZE_ACK_DONE`)

| Cycle | Scénario | `S0_FREEZE_US` | `S0_FORGE_US` | `S0_TOTAL_US` | `DEGRADED` | Slot AP 2 dans SSR |
|---|---|---:|---:|---:|---:|---|
| **`CYCLE=4` (`F2a`)** | Gel SMP nominal (3/3 APs acquittent) | 344 µs | 173 µs | 762 µs | `0` | `TLB_ACK_DONE (2)` |
| **`CYCLE=5` (`F2b`)** | AP 2 bloqué (`FAULT_AP_STALL_FREEZE`) -> expiration bornée 100 ms `CORR-33` | **100 950 µs (100,95 ms)** | 221 µs | 101 466 µs | **`1`** | **`FREEZE_ACK_DEAD (0xDEAD_0001)`** |
| **`CYCLE=6` (`F2c`)** | Cycle nominal post-récupération (3/3 APs acquittent) | 444 µs | 350 µs | 1 091 µs | `0` | `TLB_ACK_DONE (2)` |

### 3.4 Distribution sur 50 Cycles d'Endurance (`F9`, `CYCLE=21..71`, `-smp 4`)

- **Gel SMP 4 cœurs (`S0_FREEZE_US`)** : `220 µs` – `951 µs` (médiane **~345 µs**)
- **Vérification BLAKE3 Golden RAM Mirror (`S0_FORGE_US`)** : `156 µs` – `643 µs` (médiane **~225 µs**)
- **Total Strate 0 (`S0_TOTAL_US`, `t0 -> t3`)** : `750 µs` – `1 724 µs` (médiane **~920 µs**)
- **SLA Strate 1 (`S1_TOTAL_US`, `t0 -> t5`, cycles chauds)** : `1 091 µs` – `24 967 µs` (médiane **~1,55 ms**)
- **Fuite mémoire sur 50 cycles** : `free_frames_before = 472 703`, `free_frames_after = 472 752` (**0 page perdue**, dérive 0,01 % $\ll 0,1\text{ \%}$)
- **Fuite de processus / zombies sur 50 cycles** : `pids_before = 22`, `pids_after = 22` (**`pid_delta = 0`**)
- **Chaîne `ExoLedger` + Zone P0** : `112` entrées vérifiées cryptographiquement par BLAKE3, `16/16` entrées P0 immuables après `200` tentatives d'écrasement.
