# Benchmark & Rapport de Qualification ExoPhoenix (`P5`–`P8` / `F0`–`F10`)

Ce rapport consigne les mesures empiriques réelles obtenues lors de l'exécution de la suite de qualification indiscutable ExoPhoenix ([`scripts/qemu/run_phoenix_qualification.py`](../scripts/qemu/run_phoenix_qualification.py) / [`tests/exo-compliance/src/phoenix_qualification.rs`](../tests/exo-compliance/src/phoenix_qualification.rs)) sur un noyau compilé en **`--release`** (`opt-level = 3`, `lto = true`, `codegen-units = 1`) en topologie **SMP 4 cœurs (`-smp 4`)** avec **émulation matérielle Intel VT-d active (`-device intel-iommu,intremap=off,caching-mode=on -device edu`)** et **hachage BLAKE3 intégral sans raccourci** de `.text` + `.rodata` + 7 ELF Strate 1 sur chaque bascule (`MIRROR_BYTES = 4 324 526` octets / ~4,12 Mio ; `5 781 571` octets / ~5,51 Mio lors d'une auto-guérison).

---

## 1. Synthèse des Paliers d'Exécution (`P8`)

| Palier | Cible | Statut dans cet environnement | Commande | Verdict |
|---|---|---|---|---|
| **Palier 0 (`F0` / `S0`)** | Binaire de production `EXO_PROFILE=pure` (`make release`, build 2 passes `29 182 248 o` + Ed25519) | **Exécuté & Vérifié** | `EXO_PROFILE=pure make release` + `python3 scripts/qemu/run_security_audit.py --s0-only` | **`PASS`** (`0 phoenix_inject_*`, `0 fault_inject`, `0 FAULT_KERNEL_A`, `0 out 0xf4`, `0 libc.so`) |
| **Palier 1 (`F1`–`F9` + VT-d)** | QEMU TCG `-smp 4` + `-device intel-iommu,intremap=off,caching-mode=on` + `-device edu` (`--release`, 4 vCPUs, 2 Gio RAM) | **Exécuté & Vérifié (71 cycles)** | `make qemu-phoenix-faults` / `make qemu-phoenix-smp-cold` / `make qemu-phoenix-gi05` | **`PASS=9 FAIL=0 TOTAL=9`** (RC QEMU `33`, `VTD_UNITS=1`, `intel_vtd_hw=1`, `hw_dmar_fault_trapped=1`, `MIRROR_BYTES=4 324 526`) |
| **Palier 2 (`F1`–`F9` KVM + VT-d)** | QEMU KVM `-machine q35,accel=kvm,kernel-irqchip=split -device intel-iommu,intremap=on,caching-mode=on,device-iotlb=on -device edu -cpu host -smp 4` | **Câblé (`SKIPPED_NO_KVM` sur l'hôte WSL2 courant)** | `make qemu-phoenix-kvm` | `/dev/kvm` est présent sur l'hôte WSL2 mais appartient à `root:kvm` (`0660`) sans que l'utilisateur `exo` soit membre du groupe `kvm`. Dès l'ajout au groupe (`sudo usermod -aG kvm $USER`), `make qemu-phoenix-kvm` exécute le Palier 2 natif KVM + VT-d. |
| **Palier 3 (`F10`)** | Matériel x86_64 réel (Bare-Metal UEFI + IOMMU + NVMe) | **Protocole & Checklist livrés** | Voir [`docs/EXOPHOENIX-BARE-METAL-CHECKLIST.md`](EXOPHOENIX-BARE-METAL-CHECKLIST.md) | Prêt pour qualification sur station x86_64 physique |

---

## 2. Transparence & Honnêteté Cryptographique du Chemin `t1 -> t2` (`S0_FORGE_US`)

Lors d'une révision intermédiaire (`812678e6`), un raccourci de débogage dans `golden_mirror.rs` n'avait haché qu'une fenêtre sentinelle de `4 096` octets + 7 descripteurs de `64` octets (`4 608` octets au total), faisant artificiellement chuter `S0_FORGE_US` à `~225 µs`. Ce raccourci a été **entièrement supprimé** et instrumenté par la métrique obligatoire `MIRROR_BYTES` sur chaque ligne `[EXO_PHOENIX_SLA]` :

- **Empreinte RAM du Golden RAM Mirror RO** (`<= 8 Mio`) : **~2,70 Mio** = copie RO intégrale de Kernel A `.text` (`1 457 045 o`) + copie RO intégrale de Kernel A `.rodata` (`~110 Kio`) + les **7 binaires ELF complets** de Strate 1 `#![no_std]` (`~1,19 Mio` chargés une fois avant `cli`).
- **Volume réellement haché par BLAKE3 sur chaque bascule (`t1 -> t2`, 0 alloc, 0 I/O disque sous `cli`)** :
  - Cycle nominal (`HEALED=0`) : **`4 324 526` octets (~4,12 Mio)** = intégralité du miroir RO (`.text` + `.rodata` + 7 ELF Strate 1) + intégralité de Kernel A `.text` live (`__text_start..__text_end`) + intégralité de Kernel A `.rodata` live (`__rodata_start..__rodata_end`).
  - Cycle avec guérison physique (`HEALED=1`, `CYCLE=3`) : **`5 781 571` octets (~5,51 Mio)** = les `4 324 526` octets + re-hachage complet post-réparation de tout `.text` (`+1 457 045` octets).
- **Débit BLAKE3 observé sous émulation logicielle QEMU TCG `-smp 4`** : `~22–25 Mio/s` (scalaire émulé sans AVX2), soit **`~146–245 ms`** par cycle en TCG — contre **`~1–8 ms` attendus** sur matériel natif/KVM (BLAKE3 natif à `0,5–4 Gio/s`).

---

## 3. Résultats Empiriques Complets — Suite `F0` à `F9` (`--release`, `-smp 4`, Intel VT-d actif, 71 cycles)

Journal brut capturé : `target/qemu/phoenix-tcg-smp4-vtd.log` :
- Boot matériel VT-d : `[ExoPhoenix] [VT-d] DRHD=1 phys=0xfed90000 ver=0x10 aw=1 pt=1 tes=1 edu_blocked=1`
- Environnement : `[EXO_QUAL] ENV CPUS=4 ACCEL=TCG VTD_UNITS=1 FREE_FRAMES=473657 ACTIVE_PIDS=22 HEARTBEAT=931`

| Oracle | Spécification / Invariant vérifié | Durée test | Mesures & Preuves observées (`[EXO_QUAL]`) | Verdict |
|---|---|---:|---|---|
| **`F0`** | Confinement absolu des fautes (`P5.0` / `S0`) | < 1 s | `[EXO_SEC] S0 PASS pure-profile hygiene` (`0 phoenix_inject_* / 0 fault_inject / 0 out 0xf4 / 0 libc.so` dans le noyau pur `29 182 248 o`), appel sans `CAP_PHOENIX_FAULT_INJECT` rejeté `-EPERM (-1)` | **`PASS`** |
| **`F1`** | Survie Ring 0 non-coopérative (`#UD` panic, deadlock `cli` brisé par `#NMI` matériel sur `IST_NMI`, détection BLAKE3 sur `.text` complet `1,39 Mio` + auto-guérison physique `CR0.WP=0` + re-hash complet) | 965 ms | `panic_ud_recovered=1 nmi_ist_deadlock_broken=1 text_phys_healed=1 ledger_entries=54` (`HEALED=1 MIRROR_BYTES=5781571` sur `CYCLE=3`) | **`PASS`** |
| **`F2`** | Gel SMP réel 4 cœurs (`VEC_EXOPHOENIX_FREEZE = 0xF0`, `VEC_EXOPHOENIX_TLB = 0xF2`) & timeout borné 100 ms `CORR-33` (`0xDEAD_0001` / `DEGRADED=1`) | 602 ms | `online_cpus=4 nominal_peer_acks=3 corr33_stalled_ap=2 corr33_timeout_us=101332 post_recovery=OK` | **`PASS`** |
| **`F3`** | **Cold Respawn destructif (`SIGKILL` + `wait4` + `R0 -> R1 -> R2` + `PHX-03` BLAKE3 + `T20-001`)** + reconvergence asynchrone Strate 2 (`TCP 127.0.0.1` + Wayland `/run/wayland-0`) | 938 ms | `accel=TCG s0_us=164210 s1_cold_sla_us=574560 s2_reconverge_wall_ms=928 crypto=10->23 mem=3->24 vfs=5->25 dev=6->26 comp=18->29 tcp=OK wayland=OK` | **`PASS`** |
| **`F4`** | Intégrité FPU / SSE2 / XMM (`CORR-15`) sur 4 threads concurrents (2 actifs + 2 dormants `CR0.TS=1`) à travers 10 bascules SMP (`4,12 Mio` BLAKE3/bascule) | 2 177 ms | `threads=4 active=2 lazy=2 switches=10 total_simd_checks=1988881 corrupted_bits=0` | **`PASS`** |
| **`F5`** | Cohérence transactionnelle ExoFS triple-slot `A/B/C` $O(1)$ (`CORR-13`, `EPOCH-01..10`) entre barrières NVMe 1/2 et 2/3 + intégrité fichier 2 Mio après `COLD_RESPAWN` de `vfs_server` | 1 113 ms | `file_2mib_intact_post_cold_vfs=1 mid_epoch_barrier_1_2=O(1)_PASS mid_epoch_barrier_2_3=O(1)_PASS` (`CYCLE=18` `COLD_RESPAWN` en `461 092 µs`) | **`PASS`** |
| **`F6`** | Unicité des nonces AEAD XChaCha20-Poly1305 (`CORR-12`), révocation des clés pré-Phoenix et rejet d'usurpation Ring 3 `PHOENIX_WAKE_ENTROPY` (`T20-001`) | 386 ms | `aead_nonce_distinct=1 pre_phoenix_key_revoked=1 ring3_ipc_manage_denied=-38 ring3_spoof_send_rc=-7 ledger=112` | **`PASS`** |
| **`F7`** | **Quiescence DMA / Intel VT-d matériel réel (`CORR-14`, `DMA-0`, `FIX-104`)** : écriture DMA PCI réelle depuis `-device edu` (`00:02.0`, `sid=0x0010`) vers Kernel A `0x0010_0000`, bloquée par Intel VT-d (`GSTS.TES=1`), faute enregistrée dans `FSTS`/`FRCD_REG` (`reason=0x02 write=1`), ingérée dans `IOMMU_FAULT_QUEUE` -> `SIGKILL` immédiat + flush `CCMD`/`IOTLB_REG` | 186 ms | `fault_queue_64_zero_drop=1 dmar_victim_sigkill=1 victim_pid=37 intel_vtd_hw=1 hw_dmar_fault_trapped=1 pci_devices_ok=1` (`[VT-d] HW_DMAR_FAULT unit=0 sid=0x0010 addr=0x100000 reason=0x02 write=1`) | **`PASS`** |
| **`F8`** | Anti-tempête IRQ (`FIX-108..113`), remise à zéro `reset_all_masked_since()`, blacklistage `MAX_OVERFLOWS` avec EOI garanti, ré-enregistrement pilote + soak test 10 s post-bascule | 10 023 ms | `masked_since_reset=1 storm_blacklist_eoi=1 counter_reset_on_reregister=1 soak_ms=10020 soak_ops=136 heartbeat=1603` | **`PASS`** |
| **`F9`** | Révocation `ExoVeil` (`V5-8`), immutabilité de la zone P0 d'`ExoLedger` sous 200 tentatives de saturation + **50 cycles consécutifs** (`SOFT_FORCE`, `CRASH_NMI`, `COLD_RESPAWN`, `4,12 Mio` BLAKE3/cycle) sans fuite | 11 206 ms | `exoveil_revoked=1 p0_200_writes_intact=1 cycles=50 free_frames_before=471917 free_frames_after=471974 frame_delta=57 pids_before=22 pids_after=22 pid_delta=0 ledger_verified=112` | **`PASS`** |

---

## 4. Analyse Détaillée des Latences Honnêtes par Mode (`--release`, `-smp 4`, TCG+VT-d, `MIRROR_BYTES = 4,12 Mio`)

Toutes les mesures ci-dessous proviennent des jalons TSC sérialisés (`rdtsc_begin` / `rdtscp_end`, `t0..t6`) persistés dans le SSR v7 à `+0xC000` et émis sur `[EXO_PHOENIX_SLA]` lors du run `-smp 4` de 71 cycles :

### 4.1 Mode `COLD_RESPAWN` (`F3`, `F5`, `F9` — `SIGKILL` réel + `wait4` + `4,12 Mio` BLAKE3 + `PHX-03` + respawn parallèle `R0 -> R1 -> R2` + `T20-001`)

| Cycle | `S0_FREEZE_US` (`t0 -> t1`) | `S0_FORGE_US` (`t1 -> t2`, `4,12 Mio`) | `S0_TOTAL_US` (`t0 -> t3`) | `S1_RESPAWN_US` (`t3 -> t5`) | **`S1_TOTAL_US` (`t0 -> t5`, SLA $\le 1\text{ s}$)** | `MIRROR_BYTES` | Conformité SLA $\le 1\,000\,000\text{ µs}$ |
|---|---:|---:|---:|---:|---:|---:|---|
| **`CYCLE=7` (`F3`)** | 314 µs | 163 633 µs (163,6 ms) | 164 210 µs (164,2 ms) | 410 350 µs (410,4 ms) | **574 560 µs (574,56 ms)** | `4 324 526` | **OUI (< 1 000 ms même sous TCG `-smp 4`)** |
| **`CYCLE=18` (`F5`)** | 357 µs | 165 499 µs (165,5 ms) | 166 240 µs (166,2 ms) | 294 852 µs (294,9 ms) | **461 092 µs (461,09 ms)** | `4 324 526` | **OUI (< 1 000 ms même sous TCG `-smp 4`)** |
| **`CYCLE=46` (`F9`)** | 844 µs | 201 205 µs (201,2 ms) | 202 384 µs (202,4 ms) | 309 534 µs (309,5 ms) | **511 918 µs (511,92 ms)** | `4 324 526` | **OUI (< 1 000 ms même sous TCG `-smp 4`)** |

- **Observation clé** : Même en cumulant le hachage BLAKE3 intégral non caché de **4,12 Mio** en émulation logicielle QEMU TCG 4 cœurs (`163–201 ms`) **ET** la destruction réelle par `SIGKILL` + respawn parallèle `R0 -> R1 -> R2` + `PHX-03` + `T20-001` (`295–410 ms`), le **SLA strict Strate 1 $\le 1\text{ s}$ est respecté de bout en bout (`461,1 ms` à `574,6 ms`)**.
- La reconvergence asynchrone complète de la Strate 2 (`exo_compositor` nouveau PID + `/run/wayland-0` `wl_display::sync` + TCP `127.0.0.1`) s'achève en **`928 ms`** temps mur sur `F3`.

### 4.2 Mode `CRASH_NMI` (`F1a` `#UD`, `F1b` deadlock `cli` + `#NMI` sur `IST_NMI`, `F1c` auto-guérison physique `.text` `HEALED=1`)

| Cycle | Déclencheur | `S0_FREEZE_US` | `S0_FORGE_US` | `S0_TOTAL_US` | `CRYPTO_US` | **`S1_TOTAL_US` (`t0 -> t5`)** | `HEALED` | `MIRROR_BYTES` |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| **`CYCLE=1` (`F1a`)** | Exception matérielle Ring 0 `#UD` (`ud2`) | 5 542 µs | 240 562 µs | 248 090 µs | 9 640 µs | **258 006 µs (258,0 ms)** | `0` | `4 324 526` |
| **`CYCLE=2` (`F1b`)** | Deadlock `cli` brisé par `#NMI` matériel sur `IST_NMI` (3 strikes P0) | 389 µs | 233 401 µs | 234 147 µs | 4 990 µs | **239 157 µs (239,2 ms)** | `0` | `4 324 526` |
| **`CYCLE=3` (`F1c`)** | Corruption physique `.text` -> réécriture RAM `CR0.WP=0` + re-hash complet `.text` | 473 µs | 244 538 µs | 245 489 µs | 3 836 µs | **249 341 µs (249,3 ms)** | **`1`** | **`5 781 571`** |

### 4.3 Mode `CORR-33` (`F2b` — Cœur AP 2 bloqué avec `IF=0` refusant `FREEZE_ACK_DONE`)

| Cycle | Scénario | `S0_FREEZE_US` | `S0_FORGE_US` | `S0_TOTAL_US` | `DEGRADED` | `MIRROR_BYTES` | Slot AP 2 dans SSR |
|---|---|---:|---:|---:|---:|---:|---|
| **`CYCLE=4` (`F2a`)** | Gel SMP nominal (3/3 APs acquittent) | 541 µs | 191 740 µs | 192 733 µs | `0` | `4 324 526` | `TLB_ACK_DONE (2)` |
| **`CYCLE=5` (`F2b`)** | AP 2 bloqué (`FAULT_AP_STALL_FREEZE`) -> expiration bornée 100 ms `CORR-33` | **101 332 µs (101,3 ms)** | 145 600 µs | 247 345 µs | **`1`** | `4 324 526` | **`FREEZE_ACK_DEAD (0xDEAD_0001)`** |
| **`CYCLE=6` (`F2c`)** | Cycle nominal post-récupération (3/3 APs acquittent) | 416 µs | 145 522 µs | 146 320 µs | `0` | `4 324 526` | `TLB_ACK_DONE (2)` |

### 4.4 Distribution sur les 60 Cycles `SOFT_FORCE` Nominaux (`DEGRADED=0`, `-smp 4`, TCG+VT-d)

- **Gel SMP 4 cœurs (`S0_FREEZE_US`)** : `326 µs` – `875 µs` (médiane **`500 µs`**, p95 **`823 µs`**)
- **Vérification BLAKE3 intégrale Golden RAM Mirror (`S0_FORGE_US`, `4 324 526` o/cycle)** : `145,5 ms` – `365,4 ms` (médiane **`185,8 ms`**, p95 **`240,8 ms`**)
- **Total Strate 0 (`S0_TOTAL_US`, `t0 -> t3`)** : `146,3 ms` – `366,1 ms` (médiane **`186,8 ms`**, p95 **`241,9 ms`**)
- **Reseed `crypto_server` (`CRYPTO_US`)** : `3,42 ms` – `71,15 ms` (médiane **`4,23 ms`**)
- **SLA Strate 1 (`S1_TOTAL_US`, `t0 -> t5`)** : `150,0 ms` – `370,2 ms` (médiane **`195,1 ms`**, p95 **`249,4 ms`**)
- **Fuite mémoire sur 50 cycles (`F9`)** : `free_frames_before = 471 917`, `free_frames_after = 471 974` (**0 page perdue**, dérive 0,012 % $\ll 0,1\text{ \%}$)
- **Fuite de processus / zombies sur 50 cycles (`F9`)** : `pids_before = 22`, `pids_after = 22` (**`pid_delta = 0`**)
- **Chaîne `ExoLedger` + Zone P0** : `112` entrées vérifiées cryptographiquement par BLAKE3, `16/16` entrées P0 immuables après `200` tentatives d'écrasement.
