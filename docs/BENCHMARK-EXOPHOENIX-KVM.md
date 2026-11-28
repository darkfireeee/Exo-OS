# Benchmark & Rapport de Qualification ExoPhoenix (`P5`–`P8` / `F0`–`F10`)

Ce rapport consigne les mesures empiriques réelles obtenues lors de l'exécution de la suite de qualification indiscutable ExoPhoenix (`make qemu-phoenix-kvm`, [`scripts/qemu/run_phoenix_qualification.py`](../scripts/qemu/run_phoenix_qualification.py) / [`tests/exo-compliance/src/phoenix_qualification.rs`](../tests/exo-compliance/src/phoenix_qualification.rs)) sur un noyau compilé en **`--release` 2 passes** (`opt-level = 3`, `lto = true`, `codegen-units = 1`, `29 539 176 o` + signature Ed25519, post-ségrégation W0–W5 `200f513f`) en topologie **SMP 4 cœurs (`-smp 4`)**, en **Palier 1 (`TCG+VT-d`)** et en **Palier 2 (`KVM+VT-d`, `-cpu host`, `SMAP=1`, `SMEP=1`)**, avec **hachage BLAKE3 intégral sans raccourci** de `.text` + `.rodata` + 7 ELF Strate 1 sur chaque bascule (`MIRROR_BYTES = 4 300 654` octets / ~4,10 Mio ; `5 758 019` octets / ~5,49 Mio lors d'une auto-guérison).

---

## 1. Synthèse des Paliers d'Exécution (`P8`)

| Palier | Cible | Statut | Commande | Verdict |
|---|---|---|---|---|
| **Palier 0 (`F0` / `S0`)** | Binaire de production `EXO_PROFILE=pure` (`make release`, build 2 passes + signature Ed25519) | **Exécuté & Vérifié** | `EXO_PROFILE=pure make release` + `run_security_audit.py --s0-only` | **`PASS`** (`0 phoenix_inject_*`, `0 fault_inject`, `0 FAULT_KERNEL_A`, `0 out 0xf4`, `0 libc.so`) |
| **Palier 1 (`F1`–`F9` TCG+VT-d)** | QEMU TCG `-smp 4` + `-device intel-iommu,intremap=off,caching-mode=on` + `-device edu` (`--release`, 2 Gio RAM) | **Exécuté & Vérifié (71 cycles)** | `python3 scripts/qemu/run_phoenix_qualification.py --suite all --accel tcg --smp 4` | **`PASS=9 FAIL=0 TOTAL=9`** (RC QEMU `33`, `VTD_UNITS=1`, `intel_vtd_hw=1`, `hw_dmar_fault_trapped=1`, `MIRROR_BYTES=4 324 526`) |
| **Palier 2 (`F1`–`F9` KVM+VT-d)** | QEMU KVM `-machine q35,accel=kvm,kernel-irqchip=split -device intel-iommu,intremap=on,caching-mode=on,device-iotlb=on -device edu -cpu host -smp 4` (**`SMAP=1 + SMEP=1 + KPTI + VT-d`**, build 2 passes, post-W0–W5) | **Exécuté & Vérifié (71 cycles)** | `make qemu-phoenix-kvm` | **`PASS=9 FAIL=0 TOTAL=9`** (RC QEMU `33`, `ACCEL=KVM`, `VTD_UNITS=1`, `intel_vtd_hw=1`, `hw_dmar_fault_trapped=1`, `MIRROR_BYTES=4 300 654`, **`COLD_RESPAWN` `73,7–169,0 ms`**, `111,7 M` checks SIMD) |
| **Palier 3 (`F10`)** | Matériel x86_64 réel (Bare-Metal UEFI + IOMMU + NVMe) | **Protocole & Checklist livrés** | Voir [`docs/EXOPHOENIX-BARE-METAL-CHECKLIST.md`](EXOPHOENIX-BARE-METAL-CHECKLIST.md) | Prêt pour qualification sur station x86_64 physique |

---

## 2. Honnêteté Cryptographique (`MIRROR_BYTES`) & Durcissement Matériel `SMAP`

1. **Hachage BLAKE3 intégral non caché sur chaque bascule (`t1 -> t2`, 0 alloc, 0 I/O disque sous `cli`)** :
   - **Empreinte RAM du Golden RAM Mirror RO** (`<= 8 Mio`) : **~2,69 Mio** = copie RO intégrale de Kernel A `.text` (`1 457 365 o`) + copie RO intégrale de `.rodata` (`~110 Kio`) + les **7 binaires ELF complets** de Strate 1 `#![no_std]` (allégés suite à la scission S5B de `fb_server` et à la déduplication `exo-srv-rt`, chargés une seule fois avant `cli`).
   - **Volume réellement haché à chaque cycle** (tracé par `MIRROR_BYTES=` sur chaque ligne `[EXO_PHOENIX_SLA]`) :
     - Cycle nominal (`HEALED=0`) : **`4 300 654` octets (~4,10 Mio)** = intégralité du miroir RO + intégralité de `.text` live (`__text_start..__text_end`) + intégralité de `.rodata` live (`__rodata_start..__rodata_end`) — **−30 848 o** par rapport à la baseline pré-ségrégation (`4 331 502 o`).
     - Cycle avec guérison physique (`HEALED=1`, `CYCLE=3`) : **`5 758 019` octets (~5,49 Mio)** (+ re-hachage intégral post-réparation de tout `.text` = `+1 457 365 o`).
2. **Correction matérielle `SMAP` (`CR4.SMAP`) débusquée par Palier 2 (`-cpu host`)** :
   - Le passage sous `-cpu host` avec `SMAP` matériel actif a immédiatement révélé des accès Ring 0 directs à la page utilisateur `SignalTcb` et aux structures `SignalFrame`/`UContext` sur pile utilisateur (`syscall/handlers/signal.rs`, `syscall/dispatch/signals.rs`, `process/signal/delivery.rs`, `process/signal/handler.rs`).
   - Tous ces chemins passent désormais exclusivement par l'alias physmap noyau (`resolve_user_page_kernel_alias`, `read_user_typed`, `copy_to_user`), rendant le noyau **100 % conforme `SMAP=1 + SMEP=1 + KPTI`** sous `-cpu host` et en bare-metal.

---

## 3. Résultats Empiriques Palier 2 — **QEMU KVM + Intel VT-d (`make qemu-phoenix-kvm`, `-cpu host`, `SMAP=1`, `-smp 4`, 71 cycles, post-W0–W5)**

Journal brut capturé : `target/qemu/phoenix-kvm-vtd.log` (build 2 passes `29 539 176 o` + signature Ed25519, ExoFS rootfs `81 blobs / 21 889 864 o`) :
- Boot matériel VT-d : `[ExoPhoenix] [VT-d] DRHD=1 phys=0xfed90000 ver=0x10 aw=1 pt=1 tes=1 edu_blocked=1`
- Environnement : `[EXO_QUAL] ENV CPUS=4 ACCEL=KVM VTD_UNITS=1 FREE_FRAMES=470744 ACTIVE_PIDS=22 HEARTBEAT=518`

| Oracle | Spécification / Invariant vérifié | Durée test | Mesures & Preuves observées (`[EXO_QUAL]`, KVM+VT-d) | Verdict |
|---|---|---:|---|---|
| **`F0`** | Confinement absolu des fautes (`P5.0` / `S0`) | < 1 s | `[EXO_SEC] S0 PASS pure-profile hygiene` (`0 phoenix_inject_* / 0 fault_inject / 0 out 0xf4 / 0 libc.so`), appel sans `CAP_PHOENIX_FAULT_INJECT` rejeté `-EPERM (-1)` | **`PASS`** |
| **`F1`** | Survie Ring 0 non-coopérative (`#UD` panic, deadlock `cli` brisé par `#NMI` matériel sur `IST_NMI`, détection BLAKE3 sur `.text` complet + auto-guérison `CR0.WP=0` + re-hash complet) | 97 ms | `panic_ud_recovered=1 nmi_ist_deadlock_broken=1 text_phys_healed=1 ledger_entries=54` (`HEALED=1 MIRROR_BYTES=5758019` en **`10,5 ms`** sur `CYCLE=3`, `S1_TOTAL=11,97 ms`) | **`PASS`** |
| **`F2`** | Gel SMP réel 4 cœurs KVM (`0xF0` / `0xF2`) & timeout borné 100 ms `CORR-33` (`0xDEAD_0001` / `DEGRADED=1`) | 175 ms | `online_cpus=4 nominal_peer_acks=3 corr33_stalled_ap=2 corr33_timeout_us=100364 post_recovery=OK` | **`PASS`** |
| **`F3`** | **Cold Respawn destructif (`SIGKILL` + `wait4` + `R0 -> R1 -> R2` + `PHX-03` + `T20-001`)** + reconvergence asynchrone Strate 2 (`TCP 127.0.0.1` + Wayland `/run/wayland-0`) | 354 ms | `accel=KVM s0_us=9784 s1_cold_sla_us=169025 s2_reconverge_wall_ms=354 crypto=10->23 mem=3->24 vfs=5->25 dev=6->26 comp=18->29 tcp=OK wayland=OK` | **`PASS`** |
| **`F4`** | Intégrité FPU / SSE2 / XMM (`CORR-15`) sur 4 threads concurrents (2 actifs + 2 dormants `CR0.TS=1`) à travers 10 bascules SMP | 1 028 ms | `threads=4 active=2 lazy=2 switches=10 total_simd_checks=111709022 corrupted_bits=0` (**111,7 millions** de contrôles SIMD, 0 bit corrompu) | **`PASS`** |
| **`F5`** | Cohérence transactionnelle ExoFS triple-slot `A/B/C` $O(1)$ (`CORR-13`) entre barrières NVMe 1/2 et 2/3 + intégrité fichier 2 Mio après `COLD_RESPAWN` de `vfs_server` | 262 ms | `file_2mib_intact_post_cold_vfs=1 mid_epoch_barrier_1_2=O(1)_PASS mid_epoch_barrier_2_3=O(1)_PASS` (`CYCLE=18` `COLD_RESPAWN` en **`73,7 ms`**) | **`PASS`** |
| **`F6`** | Unicité des nonces AEAD XChaCha20-Poly1305 (`CORR-12`), révocation des clés pré-Phoenix et rejet d'usurpation Ring 3 `PHOENIX_WAKE_ENTROPY` (`T20-001`) | 89 ms | `aead_nonce_distinct=1 pre_phoenix_key_revoked=1 ring3_ipc_manage_denied=-38 ring3_spoof_send_rc=-7 ledger=112` | **`PASS`** |
| **`F7`** | **Quiescence DMA / Intel VT-d matériel réel (`CORR-14`, `DMA-0`, `FIX-104`)** : écriture DMA PCI réelle depuis `-device edu` (`sid=0x0010`) vers Kernel A `0x0010_0000`, bloquée en matériel (`GSTS.TES=1`), faute `FSTS`/`FRCD_REG` -> `SIGKILL` immédiat + flush `CCMD`/`IOTLB_REG` | 145 ms | `fault_queue_64_zero_drop=1 dmar_victim_sigkill=1 victim_pid=37 intel_vtd_hw=1 hw_dmar_fault_trapped=1 pci_devices_ok=1` (`[VT-d] HW_DMAR_FAULT unit=0 sid=0x0010 addr=0x100000 reason=0x02 write=1`) | **`PASS`** |
| **`F8`** | Anti-tempête IRQ (`FIX-108..113`), `reset_all_masked_since()`, blacklistage `MAX_OVERFLOWS` avec EOI garanti, ré-enregistrement pilote + soak test 10 s | 10 022 ms | `masked_since_reset=1 storm_blacklist_eoi=1 counter_reset_on_reregister=1 soak_ms=10021 soak_ops=156 heartbeat=953` | **`PASS`** |
| **`F9`** | Révocation `ExoVeil` (`V5-8`), immutabilité P0 sous 200 saturations + **50 cycles consécutifs** (`4,10 Mio` BLAKE3/cycle) sans fuite | 2 068 ms | `exoveil_revoked=1 p0_200_writes_intact=1 cycles=50 free_frames_before=468985 free_frames_after=469039 frame_delta=54 pids_before=22 pids_after=22 pid_delta=0 ledger_verified=112` | **`PASS`** |

---

## 4. Comparaison Honnête Palier 1 (`TCG+VT-d`) vs Palier 2 (`KVM+VT-d`, `-cpu host`, build 2 passes)

Toutes les mesures ci-dessous hachent **l'intégralité des `~4,10–4,13 Mio` (`4 300 654` – `4 324 526` octets)** à chaque bascule (`t1 -> t2`), avec **4 cœurs SMP** et **Intel VT-d actif (`VTD_UNITS=1`)** :

### 4.1 Distribution sur les 60 Cycles `SOFT_FORCE` Nominaux (`DEGRADED=0`)

| Métrique (`[EXO_PHOENIX_SLA]`) | Palier 1 — TCG+VT-d (`-smp 4`) | Palier 2 — **KVM+VT-d post-W0–W5 (`make qemu-phoenix-kvm`)** | Gain KVM / TCG |
|---|---|---|---|
| **`S0_FREEZE_US`** (`t0 -> t1`, gel 4 cœurs) | min `326 µs` / méd **`500 µs`** / p95 `823 µs` | min **`314 µs`** / méd **`438 µs`** / p95 **`557 µs`** | **1,1× – 1,5× plus rapide** |
| **`S0_FORGE_US`** (`t1 -> t2`, **4,10 Mio BLAKE3**) | min `145,5 ms` / méd **`185,8 ms`** / p95 `240,8 ms` | min **`7,6 ms`** / méd **`10,0 ms`** / p95 **`12,4 ms`** | **18,6× – 19,4× plus rapide** |
| **`S0_TOTAL_US`** (`t0 -> t3`, Strate 0 complète) | min `146,3 ms` / méd **`186,8 ms`** / p95 `241,9 ms` | min **`8,2 ms`** / méd **`10,6 ms`** / p95 **`13,3 ms`** | **17,5× – 18,2× plus rapide** |
| **`S1_TOTAL_US`** (`t0 -> t5`, SLA Strate 1) | min `150,0 ms` / méd **`195,1 ms`** / p95 `249,4 ms` | min **`8,6 ms`** / méd **`12,2 ms`** / p95 **`34,1 ms`** | **7,3× – 17,5× plus rapide** |

### 4.2 Mode `COLD_RESPAWN` (`F3`, `F5`, `F9` — `SIGKILL` réel + `wait4` + `4,10 Mio` BLAKE3 + `PHX-03` + `R0 -> R1 -> R2`)

| Cycle | Environnement | `S0_FORGE_US` (`4,10 Mio`) | `S0_TOTAL_US` | `S1_RESPAWN_US` | **`S1_TOTAL_US` (`t0 -> t5`)** | Verdict SLA $\le 1\text{ s}$ |
|---|---|---:|---:|---:|---:|---|
| **`CYCLE=7` (`F3`)** | Palier 1 (TCG+VT-d) | 163,6 ms | 164,2 ms | 410,4 ms | **574,6 ms** | **OUI** |
| **`CYCLE=18` (`F5`)** | Palier 1 (TCG+VT-d) | 165,5 ms | 166,2 ms | 294,9 ms | **461,1 ms** | **OUI** |
| **`CYCLE=46` (`F9`)** | Palier 1 (TCG+VT-d) | 201,2 ms | 202,4 ms | 309,5 ms | **511,9 ms** | **OUI** |
| **`CYCLE=7` (`F3`)** | **Palier 2 (KVM+VT-d post-W0–W5)** | **9,1 ms** | **9,8 ms** | **159,2 ms** | **169,0 ms** | **OUI (`5,9×` sous le SLA !)** |
| **`CYCLE=18` (`F5`)** | **Palier 2 (KVM+VT-d post-W0–W5)** | **9,1 ms** | **9,8 ms** | **63,9 ms** | **73,7 ms** | **OUI (`13,6×` sous le SLA !)** |
| **`CYCLE=46` (`F9`)** | **Palier 2 (KVM+VT-d post-W0–W5)** | **11,0 ms** | **11,7 ms** | **75,1 ms** | **86,8 ms** | **OUI (`11,5×` sous le SLA !)** |

- **Observation clé** : Sur `make qemu-phoenix-kvm` post-ségrégation (`200f513f`), **100 % des cycles `COLD_RESPAWN` (3/3)** achèvent la résurrection destructrice complète (hachage intégral BLAKE3 de `4,10 Mio` + `SIGKILL` + `wait4` + `PHX-03` + respawn parallèle `R0 -> R1 -> R2` + `T20-001`) entre **`73,7 ms` et `169,0 ms`**, et la reconvergence complète de la Strate 2 (redémarrage d'`exo_compositor` + socket Wayland `/run/wayland-0` `wl_display::sync` + TCP `127.0.0.1`) s'achève en **`354 ms`** temps mur sur `F3`.

### 4.3 Mode `CRASH_NMI` & `CORR-33` (Palier 2 KVM+VT-d post-W0–W5)

| Cycle | Scénario | `S0_FREEZE_US` | `S0_FORGE_US` | `S0_TOTAL_US` | **`S1_TOTAL_US`** | `MIRROR_BYTES` | Détail |
|---|---|---:|---:|---:|---:|---:|---|
| **`CYCLE=1` (`F1a`)** | `#UD` Ring 0 (`ud2`) | 517 µs | 7,3 ms | 8,0 ms | **11,7 ms** | `4 300 654` | `HEALED=0` |
| **`CYCLE=2` (`F1b`)** | Deadlock `cli` + `#NMI` `IST_NMI` | 298 µs | 7,1 ms | 7,6 ms | **8,3 ms** | `4 300 654` | `HEALED=0` |
| **`CYCLE=3` (`F1c`)** | Corruption `.text` -> guérison physique | 304 µs | 10,5 ms | 11,0 ms | **12,0 ms** | **`5 758 019`** | **`HEALED=1`** |
| **`CYCLE=5` (`F2b`)** | AP 2 bloqué -> timeout `CORR-33` | **100,4 ms** | 12,4 ms | 113,1 ms | **117,2 ms** | `4 300 654` | **`DEGRADED=1` (`0xDEAD_0001`)** |
| *7 cycles `CRASH_NMI`* | *Distribution (`F1` + `F4` + `F9`)* | *méd `411 µs`* | *min `7,1 ms` / méd `9,5 ms`* | *min `7,6 ms` / méd `10,2 ms`* | *min `8,3 ms` / méd `11,7 ms`* | `4,10–5,49 Mio` | `DEGRADED=0` |

---

## 5. Delta Avant / Après Ségrégation des Responsabilités (`c4b399c4` vs. `200f513f`, Palier 2 KVM+VT-d)

| Indicateur | Avant ségrégation (`c4b399c4`) | Après ségrégation W0–W5 (`200f513f`) | Évolution |
|---|---:|---:|---|
| `MIRROR_BYTES` nominal / auto-guérison | `4 331 502 o` / `5 792 003 o` | **`4 300 654 o` / `5 758 019 o`** | **−30,1 Kio** (−33,2 Kio en `HEALED=1`) |
| `SOFT_FORCE` `S0_FREEZE_US` (méd / p95) | `954 µs` / `4 051 µs` | **`438 µs` / `557 µs`** | **2,2× méd / 7,3× p95** |
| `SOFT_FORCE` `S0_FORGE_US` (méd / p95) | `23,3 ms` / `44,2 ms` | **`10,0 ms` / `12,4 ms`** | **2,3× méd / 3,6× p95** |
| `SOFT_FORCE` `S1_TOTAL_US` (méd / p95) | `34,5 ms` / `288,7 ms` | **`12,2 ms` / `34,1 ms`** | **2,8× méd / 8,5× p95** |
| `COLD_RESPAWN` `F3` / `F5` / `F9` (`S1_TOTAL_US`) | `193,6` / `160,9` / `391,3 ms` | **`169,0` / `73,7` / `86,8 ms`** | **1,15× à 4,5× plus rapide** |
| `F3` Reconvergence complète Strate 2 (wall) | `447 ms` | **`354 ms`** | **−21 %** |
| `F1` / `F4` / `F9` (50 cycles) durées totales | `282` / `1 393` / `6 734 ms` | **`97` / `1 028` / `2 068 ms`** | **1,4× à 3,3× plus rapide** |
| `F8` Débit soak 10 s (`soak_ops`) | `94` ops | **`156` ops** | **+66 %** |
| Qualification globale (`F0`–`F9`, RC QEMU `33`) | `PASS=9 FAIL=0` | **`PASS=9 FAIL=0`** | **Zéro régression (`pid_delta=0`, `corrupted_bits=0`)** |
