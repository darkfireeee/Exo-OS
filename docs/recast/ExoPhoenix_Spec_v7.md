# ExoPhoenix — Spécification v7

**SSR Layout v7 (CORR-02 corrigé) · TCB Layout GI-01 · Versionnage explicite · Handoff / reseed**  
**Avril 2026 · ExoOS Project**

> **Statut** : source de vérité runtime attendue pour la lib `exo-phoenix-ssr`.
> `ExoPhoenix_Spec_v6.md` est **obsolète** et ne doit plus être utilisée pour les layouts.
> Cette version documente le layout v7, le contrat de handoff et les garde-fous de version.

> **Périmètre** : `kernel/src/exophoenix/*`, la lib partagée `libs/exo-phoenix-ssr`,
> ainsi que le contrat de réveil de `crypto_server` après restore.
> **Clarification** : le serveur Ring 1 `servers/exo_shield` est un composant distinct.

---

## 0. Ce que change v7 par rapport à v6

| Point | v6 (obsolète) | v7 (actuel) | Raison |
|---|---|---|---|
| `MAX_CORES` | 64 | **256** | CORR-02 : cibles SMP haute densité |
| `FREEZE_ACK` stride | 64 bytes | **4 bytes** | `AtomicU32` compact, sans false sharing critique |
| `SSR_PMC` offset | `0x1080` | **`0x4000`** | Recalcul du layout pour 256 cœurs |
| `SSR_LOG_AUDIT` offset | `0x8000` | **`0x8000`** | Zone 16 KiB, cohérente avec v7 |
| `SSR_METRICS` offset | `0xC000` | **`0xC000`** | Zone 16 KiB, cohérente avec v7 |
| `magic/version` SSR | absent | **présent** | Empêche le mélange v6/v7 au boot |
| Reseed post-restore | implicite / absent | **explicite** | `PhoenixWakeEntropy` vers `crypto_server` |
| TLA `MAX_CORES` | non paramétré | **paramétré** | TLC doit couvrir 256 cœurs |

**Le layout TCB GI-01 (256 bytes, 4 cache lines) reste inchangé et reste la base canonique.**

---

## 1. Layout SSR v7 — source de vérité

**Adresse physique** : `0x0100_0000`  
**Taille totale** : `0x10000` (64 KiB)  
**`SSR_MAX_CORES_LAYOUT`** : `256`  
**Déclaration** : région e820 réservée, inaccessible Ring 1 sans mapping explicite.

### 1.1 Format de version

Le premier mot de la SSR encode un **magic/version** de compatibilité.

```text
u64 magic_version = [magic:32][major:16][minor:16]
```

- `magic` identifie la SSR ExoPhoenix.
- `major` doit être `7`.
- `minor` doit être compatible avec la lib compilée.

Le boot doit **refuser** toute SSR dont le magic/version ne correspond pas à la lib chargée.

### 1.2 Cartographie mémoire

| Offset | Taille | Type | Contenu |
|---|---|---|---|
| `+0x0000` | 8 bytes | `u64` | **Magic / version** |
| `+0x0008` | 8 bytes | `AtomicU64` | **HANDOFF FLAG** — 0=NORMAL, 1=FREEZE_REQ, 2=FREEZE_ACK_ALL, 3=B_ACTIVE |
| `+0x0010` | 8 bytes | `AtomicU64` | **LIVENESS NONCE** — écrit par B, recopié par A, vérifié par B via PULL |
| `+0x0018` | 8 bytes | `AtomicU64` | **SEQLOCK** — cohérence de lecture |
| `+0x0020` | 32 bytes | `[u8; 32]` | Padding cache line 0 |
| `+0x0040` | 64 bytes | `struct align(64)` | **CANAL COMMANDE B→A** |
| `+0x0080` | `256 × 4 = 1024 bytes` | `AtomicU32[256]` | **FREEZE ACK PER-CORE** |
| `+0x0480` | `0x3B80 bytes` | réservé | **Réserve future / garde** |
| `+0x4000` | `256 × 64 = 16384 bytes` | `[u8; 64][256]` | **PMC SNAPSHOT PER-CORE** |
| `+0x8000` | `16384 bytes` | `[u8; 16384]` | **LOG AUDIT B** — append-only |
| `+0xC000` | `16384 bytes` | `[u8; 16384]` | **MÉTRIQUES PUSH A→B** |
| `+0x10000` | — | — | Fin SSR |

### 1.3 Constantes de référence

```rust
pub const SSR_BASE_PHYS: u64 = 0x0100_0000;
pub const SSR_SIZE: usize = 0x1_0000;
pub const SSR_LAYOUT_MAJOR: u16 = 7;
pub const SSR_LAYOUT_MINOR: u16 = 0;
pub const SSR_MAX_CORES_LAYOUT: usize = 256;

pub const SSR_MAGIC_OFFSET: usize = 0x0000;
pub const SSR_HANDOFF_FLAG_OFFSET: usize = 0x0008;
pub const SSR_LIVENESS_NONCE_OFFSET: usize = 0x0010;
pub const SSR_SEQLOCK_OFFSET: usize = 0x0018;
pub const SSR_CMD_B2A_OFFSET: usize = 0x0040;
pub const SSR_FREEZE_ACK_OFFSET: usize = 0x0080;
pub const SSR_PMC_OFFSET: usize = 0x4000;
pub const SSR_LOG_AUDIT_OFFSET: usize = 0x8000;
pub const SSR_METRICS_OFFSET: usize = 0xC000;

pub const fn freeze_ack_offset(apic_id: u32) -> usize {
    SSR_FREEZE_ACK_OFFSET + apic_id as usize * 4
}

pub const fn pmc_snapshot_offset(apic_id: u32) -> usize {
    SSR_PMC_OFFSET + apic_id as usize * 64
}
```

### 1.4 Assertions statiques attendues

```rust
const _: () = assert!(SSR_SIZE == 0x1_0000);
const _: () = assert!(SSR_FREEZE_ACK_OFFSET + SSR_MAX_CORES_LAYOUT * 4 <= SSR_PMC_OFFSET);
const _: () = assert!(SSR_PMC_OFFSET + SSR_MAX_CORES_LAYOUT * 64 <= SSR_LOG_AUDIT_OFFSET);
const _: () = assert!(SSR_LOG_AUDIT_OFFSET + 0x4000 <= SSR_METRICS_OFFSET);
const _: () = assert!(SSR_METRICS_OFFSET + 0x4000 <= SSR_SIZE);
```

---

## 2. TCB Layout GI-01 — inchangé

Le layout TCB GI-01 issu de l’architecture finale reste la référence canonique.

```rust
#[repr(C, align(64))]
pub struct ThreadControlBlock { /* 256 bytes */ }
```

| Champ | Offset | Taille | Rôle |
|---|---|---|---|
| `tid` | 0 | 8 | Thread ID |
| `kstack_ptr` | **8** | 8 | RSP Ring 0 — hardcodé dans `switch_asm.s` |
| `priority` | 16 | 1 | Priorité scheduler |
| `policy` | 17 | 1 | Politique scheduler |
| `_pad0` | 18 | 6 | Alignement |
| `sched_state` | 24 | 8 | État atomique |
| `vruntime` | 32 | 8 | vruntime CFS |
| `deadline_abs` | 40 | 8 | Deadline EDF |
| `cpu_affinity` | 48 | 8 | Bitmask CPU |
| `cr3_phys` | **56** | 8 | PML4 physique — hardcodé |
| `cpu_id` | 64 | 8 | CPU courant |
| `fs_base` | 72 | 8 | `MSR_FS_BASE` |
| `gs_base` | 80 | 8 | `MSR_KERNEL_GS_BASE` / base GS user |
| `pkrs` | 88 | 4 | PKS |
| `pid` | 92 | 4 | ProcessId |
| `signal_mask` | 96 | 8 | Masque signaux |
| `dl_runtime` | 104 | 8 | Budget EDF |
| `dl_period` | 112 | 8 | Période EDF |
| `_pad2` | 120 | 8 | Alignement |
| `run_time_acc` | 128 | 8 | Temps CPU cumulé |
| `switch_count` | 136 | 8 | Nombre de context switches |
| `_cold_reserve` | 144 | 88 | Réservé ExoShield |
| `fpu_state_ptr` | **232** | 8 | état FPU/XSaveArea |
| `rq_next` | **240** | 8 | RunQueue intrusive next |
| `rq_prev` | **248** | 8 | RunQueue intrusive prev |

**Assertions attendues** : `size_of::<ThreadControlBlock>() == 256`, et les offsets hardcodés ci-dessus.

---

## 3. Contraintes d’implémentation ExoPhoenix

### 3.1 Déduplication des cœurs

Tout chemin qui visite les slots APIC doit couvrir **0..255**.

- `seen_slots` doit être un bitmap 256 bits (`[u64; 4]` ou équivalent).
- La garde `slot >= 64` est obsolète et n’est pas compatible avec v7.
- Les boucles de freeze, d’isolation et de forge doivent traiter tous les cœurs `0..SSR_MAX_CORES_LAYOUT-1`.

### 3.2 Reseed post-restore

Après un restore ExoPhoenix, Kernel B doit envoyer **avant tout autre IPC** un message `PhoenixWakeEntropy` au `crypto_server`.

Rôle du message :
- réensemencer l’état de nonce / RNG utilisé pour les flux AEAD,
- invalider ou renégocier les sessions sensibles,
- éviter toute réutilisation de nonce après snapshot/restore.

### 3.3 Nomenclature

Dans cette spécification, **ExoPhoenix** désigne le mécanisme Kernel A/B et la SSR partagée.
`servers/exo_shield` reste un service Ring 1 distinct.

---

## 4. Protocole de handoff v7

La séquence reste : `NORMAL → FREEZE_REQ → FREEZE_ACK_ALL → B_ACTIVE → NORMAL`.

### Règles obligatoires

1. `SSR_HANDOFF_FLAG` est manipulé en `Release/Acquire`.
2. Les ACKs de freeze sont écrits dans `freeze_ack_offset(apic_id)`.
3. Les snapshots PMC utilisent `pmc_snapshot_offset(apic_id)`.
4. La collecte des ACKs doit couvrir tous les slots actifs.
5. Après le retour à `NORMAL`, `PhoenixWakeEntropy` doit être envoyé à `crypto_server` avant tout autre IPC.

### Schéma

```text
NORMAL
  │
  ├─ B détecte une anomalie ou déclenche forge
  │
  ▼
FREEZE_REQ (SSR[0x0008] ← 1, Release)
  │
  ├─ Tous les cœurs A : handler 0xF1 → freeze_ack[slot] ← FREEZE_ACK_DONE (Release)
  │                     (slot ∈ [0..255])
  │
  ▼
FREEZE_ACK_ALL
  │
  ├─ B snapshot kstack / FPU / PMC
  ├─ ExoForge : image propre + Merkle + reset Ring 1
  │
  ▼
B_ACTIVE (SSR[0x0008] ← 3, Release)
  │
  ├─ Restore : B réintègre A
  │
  ▼
NORMAL (SSR[0x0008] ← 0, Release)
  │
  ├─ B envoie PhoenixWakeEntropy → crypto_server
  └─ A reprend l’exécution normale
```

---

## 5. TLA+ — modèle et paramétrage

Le modèle `docs/Exo-OS-TLA+/ExoPhoenixHandoff.tla` doit être paramétré de manière explicite.

### 5.1 Paramètres attendus

```tla
CONSTANT MAX_CORES
CORES_A == 0..MAX_CORES - 1
```

### 5.2 Exigences de modélisation

- `MAX_CORES` ne doit pas être codé en dur à `64` ou `10`.
- `FreezeAckBitmap` doit être représenté comme un ensemble ou une fonction sur `CORES_A`, pas comme un `u64`.
- `NonceSeed` doit muter sur chaque restore.
- Pour TLC, instancier `MAX_CORES = 256`.

### 5.3 Propriétés minimales

- `HandoffFlag` ne doit pas permettre `B_ACTIVE` sans les ACKs requis.
- `FreezeAckBitmap` doit couvrir tous les cœurs actifs.
- `NonceSeed` doit changer au restore.

---

## 6. Versioning et validation de compatibilité

### 6.1 Version de crate

Le layout v7 est un **breaking change**. La crate partagée doit porter une version incrémentée, par exemple :

- `exo-phoenix-ssr = 0.2.0`

### 6.2 Validation au boot

Kernel A et Kernel B doivent :

1. lire le mot `magic/version` à `SSR_MAGIC_OFFSET`,
2. vérifier que le `magic` correspond à ExoPhoenix SSR,
3. vérifier que `major == 7`,
4. refuser la SSR si la version est incompatible.

Si la version ne correspond pas, le boot doit **s’arrêter proprement** plutôt que de continuer avec une mémoire partagée incohérente.

---

## 7. Points de vigilance d’implémentation

Ces points ne sont plus des ambiguïtés de spec ; ce sont des obligations de code.

| Point | Exigence |
|---|---|
| Support 256 cœurs | Toutes les boucles de slot doivent couvrir `0..255` |
| Dédoublonnage | Utiliser un bitmap 256 bits, pas un `u64` |
| Reset after restore | `PhoenixWakeEntropy` vers `crypto_server` avant tout autre IPC |
| Layout v6 | Ne plus l’utiliser comme référence |
| TLA+ | Paramètre `MAX_CORES = 256` |
| Compatibilité SSR | Vérifier le `magic/version` au boot |

---

## 8. Checklist de validation

| Statut | Point | Fichiers |
|---|---|---|
| ✅ FIGÉ | SSR v7 : `MAX_CORES=256`, `FREEZE_ACK` stride 4, PMC à `0x4000` | `libs/exo-phoenix-ssr/src/lib.rs` |
| ✅ FIGÉ | TCB GI-01 : 256 bytes, offsets hardcodés | `kernel/src/scheduler/tcb.rs` |
| ✅ FIGÉ | Handoff Release/Acquire | `kernel/src/exophoenix/handoff.rs` |
| ✅ À FAIRE | Bitmap 256 bits pour les slots | `kernel/src/exophoenix/{handoff,forge,isolate}.rs` |
| ✅ À FAIRE | `PhoenixWakeEntropy` / reseed post-restore | `kernel/src/exophoenix/*`, `servers/crypto_server/src/main.rs` |
| ✅ À FAIRE | Versioning crate + validation SSR | `libs/exo-phoenix-ssr`, boot code |
| ✅ À FAIRE | TLC paramétré `MAX_CORES=256` | `docs/Exo-OS-TLA+/ExoPhoenixHandoff.tla` |

---

## 9. Valeur de cette spécification

Cette version est conçue pour remplacer proprement la v6 parce qu’elle :

- acte le passage à **256 cœurs**,
- fixe un layout **cohérent et borné** dans 64 KiB,
- rend la compatibilité **explicite** via `magic/version`,
- formalise le **reseed post-restore**,
- et donne un cadre TLA+ paramétrique au lieu d’une borne en dur.

*ExoPhoenix v7 — Avril 2026*  
*Remplace `ExoPhoenix_Spec_v6.md` (obsolète).*  
*Source canonique compilée : `libs/exo-phoenix-ssr/src/lib.rs` (versionnée).* 
