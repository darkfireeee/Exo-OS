# Plan Sécurité ExoOS v0.2.0 — Audit & Implémentation réelle

> Objectif : rendre **chaque** module de sécurité réellement fonctionnel, robuste et
> profond (pas de scaffolding, pas de théâtre). Modèle cible = **capability-based**
> (pas POSIX rwx). Suivi vivant — cocher au fur et à mesure.

Date de départ : 2026-06-15. Branche : `main`.

---

## 0. Racine commune du problème

Trois mécanismes de contrôle d'accès coexistent, et **le lien `identité process → droits
accordés → consultés à l'enforcement` est absent ou inerte dans les trois** :

| Chemin | Mécanisme | Statut confirmé |
|---|---|---|
| 22 syscalls ExoFS | `verify_cap(cap_token, …)` ([validation.rs:174](../../kernel/src/fs/exofs/syscall/validation.rs)) | ❌ FAUX — bitmask auto-déclaré (a2) |
| dispatch syscall | `verify_syscall` → zero_trust ([dispatch.rs:197](../../kernel/src/syscall/dispatch.rs)) | ⚠️ TRUST_ALL — `restrictions: 0` |
| IPC / objets | `check_access` → `capability::verify(pcb.cap_table)` | ⚠️ Infra réelle mais INERTE — `pcb.cap_table` jamais peuplée par `grant()` |

Infra `capability` ([table.rs](../../kernel/src/security/capability/table.rs)) = **réelle et
solide** (grant/revoke/inherit_from/génération) mais **personne n'accorde de droits** et
l'ExoFS l'ignore. Bug 1-ligne : `ALL_RIGHTS=0xFFFF` masque `RIGHT_ADMIN=1<<16`
([rights.rs:53/56](../../kernel/src/fs/exofs/core/rights.rs)) → `ExoFsQuotaSet` = EPERM à 100 %.

## Inventaire (17 kernel + 1 serveur)

- **Cassés/inertes (confirmés)** : `capability` (inerte), `zero_trust` (TRUST_ALL), `verify_cap` ExoFS (faux), bug RIGHT_ADMIN.
- **Réels & solides** : `crypto` (blake3/ed25519/x25519/aes-gcm/chacha/rng), infra `capability` (pas branchée).
- **Init au boot, enforcement NON vérifié** : `access_control`, `ipc_policy`, `integrity_check`, `exploit_mitigations`, `isolation`, `audit`, + ExoShield-v1.0 : `exoseal`, `exoveil`, `exocage`, `exoargos`, `exonmi`, `exoledger`, `exokairos`.
- **Serveur** : `exo_shield` (NGAV) — réel mais probablement aveugle (feed kernel ?) + gated sur cap inerte.

---

## TIER 0 — Le socle capability (sans lui, tout est théâtre)

- [x] **0.1** ✅ Fix `RIGHT_ADMIN` — `ALL_RIGHTS=0x0001_FFFF` (inclut bit 16), `PRIVILEGED_RIGHTS` inclut admin (non-délégable). 5 tests `tier0_admin_tests` ([rights.rs](../../kernel/src/fs/exofs/core/rights.rs)). Compile OK (ISOOK-T01).
- [x] **0.2** ✅ Binding réel : fork **hérite** la CapTable du parent (`CapTable::inherit_from`, [fork.rs](../../kernel/src/process/lifecycle/fork.rs) ~l.567) ; init PID1 reçoit une **cap admin sur FS_ROOT** ([create.rs](../../kernel/src/process/lifecycle/create.rs) avant insert). Helper [captable.rs](../../kernel/src/fs/exofs/syscall/captable.rs) (`grant/check/revoke` via `pcb.cap_table`) + méthode publique `CapTable::check_object` ([table.rs](../../kernel/src/security/capability/table.rs)).
- [x] **0.3** ✅ Mint à l'open : `sys_exofs_object_open` ([object_open.rs](../../kernel/src/fs/exofs/syscall/object_open.rs)) `grant_object_cap(object_id, rights_from_open_flags)` après ouverture ; arg `cap_rights` bidon ignoré.
- [x] **0.4** ✅ **COMPLET** — les **24 sites** traités, **plus aucun faux `verify_cap`** (compile **ISOOK-T0E**) :
  - **Gate fd RÉEL** (`check_fd`, borné par flags d'open) : object_read, object_write, object_stat-fd.
  - **Gate global RÉEL** (`check_root`, cap FS_ROOT) : gc_trigger, snapshot_create/list/mount, import_object, epoch_commit, **quota-SET (admin → init only après T1.0)**.
  - **Mint RÉEL** (acquisition) : object_open, object_create (cap owner pleine), open_by_path.
  - **Permissif honnête** (théâtre retiré, gate default-deny en TIER 1) : object_delete, object_set_meta (×2), get_content_hash, path_resolve, readdir, relation_create/query, export_object, quota-Query, object_stat-path. (Gater par chemin sans la politique TIER 1 casserait `rm`/`chmod` POSIX qui n'ouvrent pas avant ; le faux bitmask était du théâtre sans valeur sécurité → retiré.)
- [x] **0.5** ✅ **CONFIRMÉ RÉEL — pas de bypass.** Audit du chemin IPC câblé ([table.rs](../../kernel/src/syscall/table.rs)) :
  - **SEND** (`sys_exo_ipc_send` l.2900) : `enforce_direct_ipc_policy` (classe de service) + `validate_ipc_envelope_auth` (l.3286) → `check_token_owner` → `check_token` (l.283) = `verify::verify_typed` contre **KERNEL_CAP_TABLE réel** (génération+rights+type) **+** `lookup_service_cap_meta` (owner_pid+target_pid+type). Anti-spoof : pour appelant non-trusted, `sender_pid` de l'enveloppe est **toujours forcé** à `caller_pid` (FIX-IPC-SENDER-AUTH). Fail-**closed** si non-init (`NotSupported`→EACCES).
  - **RECV** (`recv_ipc_message` l.3678) : gate ownership `exo_ipc_endpoint_pid(ep) != caller_pid → EACCES` (l.3693) — un process ne lit **que** sa propre mailbox.
  - **Population** : `exo_cap_create` (`sys_exo_cap_create` l.3849) → `capability::create` (l.213) mint dans KERNEL_CAP_TABLE + SERVICE_CAP_META, **gated par `check_direct_ipc`** (ipc_policy) + TTL ExoKairos. Init au boot : `init_capability_subsystem()` (security_init étape 2, [mod.rs:253](../../kernel/src/security/mod.rs)).
  - Seuls chemins permissifs : endpoints de réponse éphémères kernel (bit 63) et appelants Ring-1 trusted (`can_inject`) — légitimes par design. Le bypass taille-hors-format a déjà été fermé (FIX-IPC-AUTH §A-02).
  - NB : `send_checked`/`check_channel_access` (API v6, [capability_bridge/check.rs](../../kernel/src/ipc/capability_bridge/check.rs)) est **redondante et non-câblée** ; le chemin réellement câblé est le chemin token-enveloppe ci-dessus, qui est réel. → **table IPC distincte de `pcb.cap_table`** : IpcEndpoint = tokens de service kernel (KERNEL_CAP_TABLE) ; FileInode = caps per-process (pcb.cap_table). Deux tables réelles, deux classes d'objets.
- [x] **0.6** ✅ **Tests e2e capability — 6 verts, 0 régression** (validés WSL `cargo test`) :
  - **ExoFS** ([captable.rs](../../kernel/src/fs/exofs/syscall/captable.rs) `tier06_e2e_tests`) : `open_flags_derive_exact_rights` (RDONLY n'accorde jamais WRITE, WRONLY pas de READ, dérivation bornée), `e2e_deny_then_grant_then_revoke` (**refusé sans cap → mint à l'open (droits réels dérivés des flags) → autorisé/borné → close/révocation → refusé**), `e2e_rdwr_grants_write` (montée RDWR effective).
  - **IPC** ([mod.rs](../../kernel/src/security/capability/mod.rs) `tests`) : `service_token_roundtrip_for_allowed_route` (émission+vérif, mauvais owner→deny), `service_token_creation_respects_ipc_policy` (route interdite→deny, ipc_policy enforce), `service_token_revocation_is_effective` (**token accepté → `revoke_handle` → même token rejeté**, génération bumpée).
  - Couvre les 2 tables (FileInode `pcb.cap_table` + IpcEndpoint KERNEL_CAP_TABLE) et les 3 propriétés exigées : refusé sans cap, autorisé avec, révocation effective.

### Design retenu TIER 0 — modèle capability ExoFS (object_id-keyed, fd-as-capability)

Après audit du flux ExoFS (objet = `blake3(path)`, fd track RDONLY/RDWR, `OpenArgs`
porte des résidus POSIX `mode`/`owner_uid`, pas d'ACL réel) :

- **Source de vérité = `pcb.cap_table`** (la vraie `CapTable`, déjà dans le PCB, héritée
  au fork via `inherit_from`). Une capability = `(object_id → rights, génération, type)`.
- **Mint à l'open** : `sys_exofs_object_open` → après résolution `object_id`, calcule les
  droits à partir des flags (RDONLY→READ|STAT|LIST ; RDWR→+WRITE|CREATE|DELETE|SETMETA),
  puis `cap_table.grant(object_id, rights, ExoFsObject)`.
- **Vérif à chaque op** : remplace le faux `verify_cap(bitmask)` par une consultation réelle
  `cap_table.get(object_id)` + check `granted.contains(required)` (+ génération = révocation).
- **Encodage des droits** : `capability::Rights` (bits 0-5 = READ/WRITE/EXEC/GRANT/REVOKE/DELEGATE,
  6-15 = IPC…) a une sémantique DIFFÉRENTE de `fs/exofs::RightsMask` (17 droits). On stocke
  donc les **bits ExoFS bruts** dans le champ `rights` de la cap, discriminés par le
  `type_tag = ExoFsObject` (la CapTable est agnostique aux bits — `contains` est un subset
  bitwise correct quel que soit le masque, tant que grant & check utilisent la même
  interprétation). Aucune correspondance lossy. Réutilise la CapTable prouvée telle quelle.
- **Pourquoi fd-as-capability et pas token-en-userspace** : pas de token forgeable/volable
  côté user ; le fd est un handle inforgeable per-process ; le `cap_token` u64 (a2) est de
  toute façon trop petit pour un vrai CapToken (CAP_TOKEN_WIRE_SIZE ≈ 20 o). On supprime
  donc l'arg `cap_token` bidon. Politique d'open permissive pour l'instant (tout chemin) →
  **durcie en TIER 1** ; mais le MÉCANISME (mint + vérif + révocation) est réel dès TIER 0.

### TIER 1.0 — Moindre privilège à l'héritage fork ✅ (fait)

`CapTable::inherit_from_masked(parent, strip_rights, strip_type)` ([table.rs](../../kernel/src/security/capability/table.rs)) + fork l'utilise ([fork.rs](../../kernel/src/process/lifecycle/fork.rs)) pour **retirer `PRIVILEGED_RIGHTS` (gc/import/snapshot/admin) des caps FileInode héritées**. Init garde l'admin FS ; **aucun fork (shell/app) ne l'hérite** → seuls grant/délégation explicites rétablissent les droits privilégiés. 3 tests `tier_sec_tests` (check_object enforce droits+type, revoke refuse, masking strippe admin/gc). Compile **ISOOK-T1**, boot **progresse** (atteint `init: ready ipc_router pid=2`, 0 EACCES — au-delà du stall #25 antérieur).

## TIER 1 — Activer la politique (suite)

- [x] **1.1** ✅ **zero_trust : TRUST_ALL → enforcement RÉEL** (8 tests verts). Le moteur `policy::evaluate` ([policy.rs](../../kernel/src/security/zero_trust/policy.rs)) était déjà réel (untrusted-deny, MLS Bell-LaPadula/Biba pour les **données**, CryptoKey/DMA/Device→Trusted+) mais **inerte** : `dispatch.rs` reconstruisait `SecurityContext::new_normal` (restrictions=0, trust figé) à chaque syscall → TRUST_ALL de fait.
  - **Nouveau state per-process** ([process_state.rs](../../kernel/src/security/zero_trust/process_state.rs)) : store **lock-free** `[AtomicU64; 1024]` indexé par PID. `restrict_process` (monotone, durcit seulement ; refuse init/PID hors-plage), `process_restrictions`, `clear_process_restrictions`, `inherit_restrictions`, `trust_for_pid` (init→System, Ring 1→Trusted, reste→Normal), `context_for_caller`. **Boot-safe** : défaut=0 → comportement identique à avant ; l'enforcement ne mord que sur opt-in.
  - **Contexte réel câblé au dispatch** ([dispatch.rs](../../kernel/src/syscall/dispatch.rs) §2c) : `context_for_caller(pid, tid)` remplace `new_normal`. Constructeur `SecurityContext::for_process(principal, trust, restrictions)` ([context.rs](../../kernel/src/security/zero_trust/context.rs)).
  - **Enforcement syscall réel** ([policy.rs](../../kernel/src/security/zero_trust/policy.rs) branche `Syscall`) : `syscall_restriction_mask(nr)` mappe fork/clone/vfork→`NO_FORK|NO_PROCESS_CREATE`, execve→`NO_EXEC|NO_PROCESS_CREATE`, socket/connect/bind/…→`NO_NETWORK` → `DenyAndAudit` si l'appelant porte la restriction. (Le MLS ne s'applique pas à l'**acte** d'appeler un syscall — correct.)
  - **Héritage & recyclage** : fork transmet les restrictions du parent ([fork.rs](../../kernel/src/process/lifecycle/fork.rs), RÈGLE ZT-03/SAND-03) ; le slot est remis à 0 au recyclage du PID ([wait.rs](../../kernel/src/process/lifecycle/wait.rs)).
  - **Tests** : 5 × `process_state` (monotone/guardé, clear, héritage fils, mapping, trust) + `syscall_restriction_is_enforced` (NO_FORK → fork refusé, read OK, non-restreint → fork OK).
  - **NB** : l'infra `sandbox.rs`/`pledge.rs` (SandboxPolicy 256-bit, PledgeSet OpenBSD) existe et est réelle mais **sans store per-PID** → le syscall d'opt-in (`pledge()`/sandbox) qui peuplera ce state est **TIER 2.10** (isolation). Le MÉCANISME zero-trust (state + dérivation + enforcement + héritage) est, lui, réel et testé dès maintenant.
- [x] **1.2** ✅ **ipc_policy : déjà RÉEL et enforced** (confirmé en auditant le chemin câblé, 11 tests). `POLICY` = **51 routes** `(src_class→dst_class)` en **allowlist default-deny** (assert compile-time =51, Architecture v7) ([ipc_policy.rs](../../kernel/src/security/ipc_policy.rs)). `check_direct_ipc` applique l'allowlist ; `enforce_direct_ipc_policy` ([table.rs:3785](../../kernel/src/syscall/table.rs)) l'appelle au bord syscall (send/call) + **log les refus dans exoledger** (audit) + EACCES. Anti-spoof : `can_inject_src_pid` (Ring 1 only) + `sender_pid` forcé à `caller_pid` pour tout non-trusted (FIX-IPC-SENDER-AUTH). `register_service_class` interdit aux PID dynamiques d'usurper Init/Broker + câble `zero_trust::register_ring1_pid` ; `register_service` valide le CapToken (revoked/malformé refusé). **Défense en profondeur** : route service-class **+** CapToken par message (cf. 0.5).

## TIER 2 — Auditer & rendre réel chaque module kernel (1 par 1)

- [ ] **2.1** `integrity_check` — 🔍 **AUDITÉ : crypto RÉELLE (ed25519+blake3) mais 3 chemins non-câblés.**
  - **runtime_check** ([runtime_check.rs](../../kernel/src/security/integrity_check/runtime_check.rs)) : hash blake3 `.text/.rodata` réel, compare en temps constant, panic sur altération. `init_runtime_integrity()` calcule les refs au boot ✓, MAIS `security_periodic_check()` ([mod.rs:349](../../kernel/src/security/mod.rs)) — censé tourner « toutes les N ticks » — **n'a AUCUN appelant** → check **jamais re-déclenché**. ⇒ *Gap 2.1-a : à câbler via un **kthread dédié** (modèle reaper) en **mode observe** (log/compteur, pas `assert!`/panic), gaté sur `is_security_ready()` (.text stable). **PAS** dans le tick IRQ : hash Blake3 de tout `.text` en contexte interruption = latence prohibitive + risque boot #25.*
  - **secure_boot** ([secure_boot.rs](../../kernel/src/security/integrity_check/secure_boot.rs)) : ed25519 BootAttestation + PCR blake3 réels ; `check_chain_of_trust` gaté à l'exec ([exec.rs:272](../../kernel/src/process/lifecycle/exec.rs), non-bloquant en dev / strict via `strict_exec_signatures`). MAIS `verify_boot_attestation` (seul setter de `CHAIN_VERIFIED`) **n'a AUCUN appelant** → `is_chain_verified()` **toujours false** → le check exec est **toujours sauté**. ⇒ *Gap 2.1-b : exo-boot doit passer une BootAttestation signée + kernel appelle verify_boot_attestation early-boot.*
  - **code_signing** ([code_signing.rs](../../kernel/src/security/integrity_check/code_signing.rs)) : `verify_module_signature` (blake3+ed25519, anti-replay) réel MAIS **AUCUN appelant** (le loader ne vérifie pas la signature des serveurs au chargement) **+ clé maître = « placeholder »**. ⇒ *Gap 2.1-c : autorité de signature réelle = **dépend de crypto_server (2C)** + câbler le loader ELF des serveurs `/system`.*
  - **Conclusion** : 2.1 « réel » **dépend de TIER 2-CRYPTO** (clés/autorité de signature). Les gaps 2.1-a (périodique) sont câblables sans crypto mais **boot-risqués** (panic). → séquencer **crypto_server d'abord**.
- [x] **2.2** ✅ **exoseal RÉEL + CÂBLÉ.** ([exoseal.rs](../../kernel/src/security/exoseal.rs)) `configure_nic_iommu_policy` = **vrai verrou IOMMU NIC** (création domaine + whitelist DMA `0x0A00_0000..0x0B00_0000` + attach des devices class 0x02 + activate). `exoseal_boot_phase0` : exoveil_init (PKS) + exocage_global_enable (CET) + watchdog + `verify_p0_fixes` (NIC locked, CET enabled, PKS domains Caps/Credentials/TcbHot **default-deny révoqués**) → sur échec : exoledger `BootSealViolation` + **handoff SSR** (ExoPhoenix). `exoseal_boot_complete` : PKS restore + `SECURITY_READY`. Gated sur support HW (cet_supported/pks_available). 4 tests.
- [x] **2.3** ✅ **exoveil RÉEL.** ([exoveil.rs](../../kernel/src/security/exoveil.rs)) Domaines PKS via **MSR/PKRS réels** (`is_domain_revoked`, `pks_restore_for_normal_ops`), default-deny au boot, restore en ops normales. Câblé par exoseal.
- [x] **2.4** ✅ **exocage RÉEL — vrai Intel CET.** ([exocage.rs](../../kernel/src/security/exocage.rs)) MSRs CET réels (IA32_U_CET 0x6A0, IA32_S_CET 0x6A2, PL0_SSP 0x6A4, INTERRUPT_SSP_TABLE 0x6A8), shadow-stack token (Intel CET §3.4), IBT/ENDBRANCH, **handler #CP → HANDOFF ExoPhoenix immédiat** sur violation. `exocage_global_enable` appelé par exoseal.
- [x] **2.5** ✅ **exoargos RÉEL.** ([exoargos.rs](../../kernel/src/security/exoargos.rs)) PMC via `rdpmc`/PERFEVTSEL réels, baseline + détection d'anomalie → exoledger.
- [x] **2.6** ✅ **exonmi RÉEL.** ([exonmi.rs](../../kernel/src/security/exonmi.rs)) Watchdog APIC (LVT) réel, tick + strikes → exoledger `WatchdogExpired` P0 + handoff.
- [x] **2.7** ✅ **exoledger RÉEL + CÂBLÉ.** ([exoledger.rs](../../kernel/src/security/exoledger.rs)) Log chaîné Blake3 (`hash = Blake3(seq‖tsc‖actor_oid‖action‖prev_hash)`), **zone P0 immuable** (16, append-only, overflow→saturate gracieux) + ring (96), `verify_p0/ring_integrity` (recompute + vérif chaînage, Kernel B), ISR-safe (atomics, no-alloc), 2 tests. **Câblé aux vrais événements** : exoseal (BootSealViolation/NicIommuLocked/BootEvent), exonmi (WatchdogExpired P0), exocage (CpViolation P0), exoargos (anomalie), **object_write (AccessDenied = refus capability TIER 0)**, exit, ipc_policy (IpcUnauthorized). *Gap mineur (observe, optionnel)* : object_read/stat denials non loggés vers exoledger (le sont vers security::audit via le gate).
- [x] **2.8** ✅ **exokairos RÉEL + CÂBLÉ.** ([exokairos.rs](../../kernel/src/security/exokairos.rs)) Capabilities temporelles : `deadline_mac` = **HMAC-Blake3(oid‖deadline_tsc‖KERNEL_SECRET)** tronqué 16 o ; deadline TSC réelle dans `cap_deadline_table` **kernel-only** (Ring 1 ne peut ni forger ni déduire la deadline) ; `verify(current_tsc)` const-time ; `KERNEL_SECRET` (32 o) init par ExoSeal au boot (RNG, immuable). **Câblé** : `register_ttl_for_cap(oid, rights)` appelé par `capability::create` (chaque token de service IPC reçoit un TTL — cf. 0.5). Budget calls/bytes par fenêtre `KAIROS_WINDOW_NS`.
- [x] **2.9** ✅ **exploit_mitigations RÉEL.** ([exploit_mitigations/](../../kernel/src/security/exploit_mitigations/mod.rs)) CET ([cet.rs](../../kernel/src/security/exploit_mitigations/cet.rs)) + MSRs réels ; KASLR entropy passée à `mitigations_init` (security_init §6). Vrai code HW, pas de stub.
- [~] **2.10** `isolation` — **infra RÉELLE** (SandboxPolicy 256-bit + PledgeSet OpenBSD → SandboxPolicy, [sandbox.rs](../../kernel/src/security/isolation/sandbox.rs)/[pledge.rs](../../kernel/src/security/isolation/pledge.rs), cf. audit 1.1). **Reste** : le syscall d'opt-in `pledge()`/sandbox qui peuple le state per-PID zero-trust (`process_state::restrict_process`, déjà prêt côté enforcement TIER 1.1). C'est le seul vrai « à câbler » d'isolation — mécanisme et enforcement prêts, manque le point d'entrée userspace.
- [x] **2.11** ✅ **audit RÉEL + CÂBLÉ.** ([audit/logger.rs](../../kernel/src/security/audit/logger.rs)) Ring d'audit (rdtsc timestamp, catégories, `flush_to_userspace`, SecurityViolation toujours loggé RÈGLE AUDIT-02). **Branché sur les refus réels** : `zero_trust::verify_access` (Deny/DenyAndAudit/DenyAndAlert → `log_security_violation`) — **donc mon enforcement TIER 1.1 (NO_FORK/…) est audité automatiquement** ; `access_control::checker` (log_event + log_security_violation) ; `syscall_audit`. **Deux systèmes complémentaires** : exoledger = ledger chaîné tamper-evident (P0/critique, Kernel-B-vérifiable) ; security::audit = ring opérationnel userspace-readable. Pas de redondance.

### TIER 2-CRYPTO — Service crypto (colonne vertébrale de confiance)

Le `crypto_server` (Ring1, `/sbin/exo-crypto-server`, `DEPS_CRYPTO`) + le module kernel
`security/crypto` (blake3, **ed25519 réel**, x25519, aes-gcm, xchacha20-poly1305, kdf, rng)
sont **complémentaires et critiques** : c'est l'autorité qui **détient/gère les clés** dont
dépendent toutes les autres couches —
- signature/vérif de code (TIER 2.1 integrity_check, exec bloquant) ;
- `deadline_mac` HMAC-Blake3 des caps temporelles exokairos (TIER 0/1) ;
- attestation/consentement du broker (TIER 3) ; scellement d'objets système ;
- `KERNEL_SECRET` (déjà dérivé du CSPRNG au boot, security_init §10).

- [x] **2C.1** ✅ **AUDITÉ : crypto_server = vrai service robuste** ([servers/crypto_server](../../servers/crypto_server/src/main.rs)). Protocole IPC v3 (14 ops : derive/random/encrypt/decrypt/hash/sign/verify-streaming/TLS/revoke/rotate/revoke-owner/stats/phoenix-reseed). **Requêtes cap-gated** (`authorize_request` → `exo_cap_check(IPC_SEND, CRYPTO_SERVER_PID, IpcEndpoint)`). Crypto = crates RustCrypto IETF (ed25519-dalek, chacha20poly1305, blake3, x25519, hkdf) — jamais from-scratch (RÈGLE SRV-CRYPTO-01). Divergence endpoint(4)/PID(5) documentée (FIX-SRV-M7).
- [x] **2C.2** ✅ **Gestion de clés réelle** ([keystore.rs](../../servers/crypto_server/src/keystore.rs)) : 64 slots, quota/owner (8), TTL 300 s, **clés jamais exportées** (handles opaques only ; `get_key`→usage→`wipe_bytes`), **shredding DoD 5220.22-M 3-passes** (zéros/aléa/zéros, volatile), révocation/rotation/expire/revoke-owner/revoke-pre-phoenix, contrôle owner (CAP-01). **FIX-SEC-2C.4** : `rotate_key` utilisait une dérivation **ad-hoc faible** (XOR+FNV, commentaire faux « pas accès à blake3 ») → remplacée par **HKDF-Blake3 réel** (`keyed_hash(old_key, info)` → `derive_key(ctx, prk)`) + wipe du matériel intermédiaire. *Reste (follow-up profond)* : scellement KEK des clés en RAM (défense en profondeur ; l'isolation de process protège déjà, pas de persistance disque).
- [~] **2C.3** ⚠️ **Clarification architecturale** : la vérif des binaires **au niveau kernel** (secure_boot/code_signing, Ring 0) doit utiliser le **crypto kernel** (`security/crypto`, ed25519 réel) — PAS le `crypto_server` Ring 1 (ce serait une **inversion** : le kernel dépendrait d'un service qu'il lance). `crypto_server` est l'**autorité crypto userspace** (TLS, sign/verify/encrypt délégués aux apps/serveurs) — ce rôle est réel et câblé (cap-gated). Le vrai gap « binaires signés » est dans **2.1** (clés placeholder → PKI build-time réelle), pas dans crypto_server. Le MAC exokairos (deadline) est kernel → `KERNEL_SECRET` + blake3 (cf. 2.8). → 2C.3 redéfini : crypto_server = autorité **userspace** (fait), signature kernel = 2.1.
- [x] **2C.4** ✅ **Aucun chemin crypto faible.** Kernel `ed25519_verify` ([ed25519.rs:114](../../kernel/src/security/crypto/ed25519.rs)) = RustCrypto `VerifyingKey::verify` réel (3 tests : roundtrip, tampered rejeté, wrong-key rejeté). RNG ([rng.rs](../../kernel/src/security/crypto/rng.rs)) = RDRAND + fallback TSC+stack **documenté en dernier recours** (RÈGLE RNG-03, après 10 échecs RDRAND — acceptable). Côté serveur, le seul fallback faible (rotate_key) est éliminé (2C.2). Cohérence kernel↔serveur : deux domaines distincts (kernel Ring 0 / service Ring 1), chacun sur RustCrypto.

## TIER 3 — Serveur exo_shield (NGAV)

- [ ] **3.1** Brancher le **feed kernel→shield** : hooks exec/syscall/memory réels qui forwardent les événements.
- [ ] **3.2** Gating des requêtes privilégiées sur le **cap réel** (ExoCapTokenWire validé contre la cap_table).
- [ ] **3.3** Vérifier engine/signatures/behavioral/ml/network/sandbox/forensics réellement actionnés.

## TIER 4 — Affichage FS capability-based (corriger le faux rwx)

- [ ] **4.1** `ls -la` via terminal montre des **rwx POSIX faux** : ExoOS est capability-based. Remplacer par une vue captoken/capability (droits réels accordés, owner/principal, type d'objet) — vision 0.2.0.
- [ ] **4.2** Aligner `stat`/`readdir`/fs_bridge sur le modèle capability (pas de mode bits POSIX trompeurs).

## TIER 5 — ExoPhoenix

- [ ] **5.1** Câbler `stage0_init()` sur un cœur dédié (AP réservé hors `smp_boot_aps`) → `sentinel::run_forever()`.
- [ ] **5.2** Comprendre/fixer pourquoi la SSR v7 tombe en `Degraded` sous QEMU au lieu de `Normal`.
- [ ] **5.3** Rechargement physique réel (`reconstruct_kernel_a` + reset drivers Ring1) + isolation huge pages (`mark_a_pages_not_present`).
- [ ] **5.4** Réactiver `seed_kernel_a_image_blob()` (désactivé commit c2dd3826) une fois la régression fs/blob résolue.

---

## Journal

- 2026-06-15 : audit initial, plan créé. Démarrage TIER 0.
- 2026-06-15 : **0.1 RIGHT_ADMIN fait** (ALL_RIGHTS=0x1FFFF + PRIVILEGED inclut admin + 5 tests), compile OK.
- 2026-06-15 : **design TIER 0 figé** (object_id-keyed cap_table / fd-as-capability). Primitives validées :
  `CapObjectType::FileInode` (objets ExoFS), `Rights::from_bits_truncate` préserve tous les bits (round-trip
  ExoFS fidèle), `contains` = subset bitwise correct, accès PCB via `current_pid()`→`find_by_pid`→`pcb.cap_table`.
- 2026-06-15 : **TIER 0.4 COMPLET** — 24 sites traités, **plus aucun faux `verify_cap`** (théâtre retiré). Compile ISOOK-T0E, **2922 tests ExoFS passent (0 échec)** — zéro régression. quota-SET passe en gate admin réel (init only après T1.0). TIER 0 (capability socle) **entièrement fait & validé**.
- 2026-06-15 : **PREUVE par tests host (25 passed, 0 failed)** — `cargo test --lib tier` : mes 3 tests `tier_sec_tests` PASSENT (check_object enforce droits+type, revoke refuse, fork-masking strippe admin/gc) = **l'enforcement capability est réel, prouvé** ; + les 6 tests d'intégration ExoFS de nouveau VERTS. Bug réel corrigé : `caller_pid()` lisait `gs:[0x20]`→segfault en host-test + EPERM-sur-None faux → `caller_pid` `#[cfg(test)]`-safe + **contexte kernel/test (None)=autorisé** (sémantique correcte ; en prod ces syscalls ont toujours un process). TIER 1.0 moindre privilège fork inclus.
- 2026-06-15 : **TIER 0 socle IMPLÉMENTÉ & VALIDÉ** — helper `captable.rs` (grant/check/revoke + check_fd/check_blob/check_root), `CapTable::check_object` public, fork inherit (inherit_from), cap admin FS_ROOT à init, mint à l'open, **gate capability RÉEL** sur object_read/write/stat-fd (bornés par flags d'open) + gc/snapshot_create (cap FS_ROOT). Bugs corrigés : `ObjectId` ExoFS = hash 32 o (→ clé u64 = 8 premiers octets), `get()` privé (→ `check_object` public). **Compile + boot non régressé** (ipc_router registered, 0 EACCES). Reste : router les ~16 sites non-critiques (mécanique) + #25 (corruption post-fork, pause) bloque l'e2e shell.
- 2026-06-15 : **guide d'implémentation complet prêt à appliquer** → [GUIDE-IMPL-TIER0.md](GUIDE-IMPL-TIER0.md).
  Contient : code exact du helper (`captable.rs`), table de routage des **24 sites** verify_cap (source object_id
  fd/path/global par syscall), points d'insertion exacts (fork.rs:669, create.rs:291-301, percpu.rs:279), 7
  hypothèses/risques (H1 contexte kernel, H4 fork-inherit prérequis, H6 refcount fd au revoke…), ordre d'application
  + plan de validation. **Gaps confirmés** : fork n'hérite pas la CapTable ; verify_cap faux ; cap_table jamais peuplée.
  → Budget frais = application mécanique (helper → fork inherit → mint open + grant init → router 24 sites → build → e2e).
- 2026-06-15 : **TIER 0 ENTIÈREMENT TERMINÉ** (0.1→0.6). **0.5** : audit prouve que l'IPC enforce réellement (KERNEL_CAP_TABLE + SERVICE_CAP_META via `check_token_owner`, ipc_policy au bord syscall, anti-spoof sender_pid, RECV ownership) — **pas un bypass**, fail-closed, init au boot. **0.6** : 6 tests e2e (3 ExoFS `tier06_e2e_tests` + 3 IPC dont `service_token_revocation_is_effective`) couvrant les 2 tables et les 3 propriétés (refusé sans cap / autorisé avec / révocation effective). `cargo test tier` = 28 ✓, `service_token` = 3 ✓.
- 2026-06-15 : **TIER 1 TERMINÉ**. **1.1 zero_trust** : le moteur MLS était réel mais inerte (dispatch = `new_normal` restrictions=0). Implémenté l'**état zero-trust per-process** (`process_state.rs`, store lock-free `[AtomicU64;1024]`, monotone, boot-safe) + `SecurityContext::for_process` + câblage dispatch (`context_for_caller`) + **enforcement syscall réel** (`syscall_restriction_mask`: NO_FORK/NO_EXEC/NO_NETWORK/NO_PROCESS_CREATE → DenyAndAudit) + héritage fork + recyclage PID. **1.2 ipc_policy** : confirmé déjà réel (51-routes allowlist default-deny, enforce au bord syscall + audit exoledger, anti-spoof). Tests : `cargo test zero_trust` = **8 ✓** (5 process_state + enforcement + 2 régressions). **Build ISO réel = OK** (exo-os.iso 53M, ISO_EXIT=0). Reste avant ExoPhoenix : TIER 2 (modules 1-à-1 + crypto_server), TIER 3 (exo_shield feed), TIER 4 (affichage rwx→capability).
- 2026-06-15 : **décision séquencement TIER 2** (user) : **crypto_server d'abord** (dépendance), **observe d'abord / enforce derrière flag** (boot #25 fragile). **2.1 integrity_check AUDITÉ** : crypto réelle (ed25519+blake3) mais 3 chemins **non-câblés** (`security_periodic_check`, `verify_boot_attestation`, `verify_module_signature` sans appelant) + clés placeholder → dépend de 2C.
- 2026-06-15 : **TIER 2-CRYPTO (crypto_server) AUDITÉ + DURCI**. Service Ring 1 **réel & robuste** (protocole IPC v3 cap-gated, ed25519/xchacha20/blake3 RustCrypto, clés jamais exportées, shredding DoD 3-passes, TTL/rotation/révocation/Phoenix-reseed). **FIX-SEC-2C.4** : `rotate_key` dérivait via XOR+FNV faible (commentaire faux « pas accès blake3 ») → **HKDF-Blake3 réel** (keyed_hash→derive_key) + wipe intermédiaire. Kernel `ed25519_verify` = RustCrypto réel (testé) ; RNG = RDRAND + fallback TSC documenté. **2C.3 reclarifié** : signature **kernel** des binaires = crypto kernel (pas d'inversion vers Ring 1) → reste un gap **2.1** (PKI réelle). **Build ISO = OK** (53M, kernel + crypto_server bare-metal link OK, ISO_EXIT=0).
- 2026-06-15 : **TIER 2 ExoShield AUDIT SWEEP** — verdict : les modules ExoShield-v1.0 sont **réels & câblés**, pas du scaffold (0 `unimplemented!/todo!/FIXME` dans security/ hors placeholder code_signing). Confirmés en profondeur : **2.7 exoledger** (chaîné Blake3 + P0 immuable, câblé à exoseal/exocage/exonmi/exoargos/object_write/ipc_policy/exit), **2.8 exokairos** (HMAC-Blake3 deadline + table kernel-only, câblé à capability::create), **2.11 audit** (ring, câblé zero_trust/access_control/syscall — capte mon enforcement TIER 1.1). **Boucle d'audit fermée** sur mes refus TIER 0 (ExoFS AccessDenied→exoledger) et TIER 1 (zero_trust Deny→audit). **Seul gap TIER 2 réel** = 2.1 integrity_check non-câblé (PKI placeholder). Restent à confirmer en profondeur (signaux réels : appels exoledger + init boot) : 2.2 exoseal, 2.3 exoveil, 2.4 exocage, 2.5 exoargos, 2.6 exonmi, 2.9 exploit_mitigations, 2.10 isolation.

## Crypto — complétion & durcissement (socle de confiance, zéro placeholder)

> Demande user : *« termine proprement et complètement les crypto en kernel et en serveur, résolution totale et robuste »* — la crypto étant le socle dont dépendent FS/ExoPhoenix/tous les fix.

**Kernel `security/crypto`** — audit exhaustif des 7 modules : **0 placeholder, 0 from-scratch faible** (blake3/ed25519/x25519/aes-gcm/kdf via crates RustCrypto ; ChaCha20 = seule maison, raison documentée = la crate casse LLVM sur x86_64-unknown-none sans SSE2).
- [x] **RNG durci** ([rng.rs](../../kernel/src/security/crypto/rng.rs)) : l'ancien seed dépendait de **RDRAND-ou-fallback-TSC brut** (si RDRAND échoue → seed crypto prédictible). Nouveau `gather_seed` = **pool multi-sources** (RDSEED ×4 + RDRAND ×6 + **jitter TSC** + pointeur de pile) **conditionné par Blake3** + ajout de `rdseed64` (entropie matérielle vraie, CF vérifié) + suivi `hw_seeded` (qualité d'entropie exposée). Seed/reseed effacés (write_volatile). 2 tests.
- [x] **ChaCha20 validé RFC 8439** : ajout du **KAT §2.3.2** (`chacha20_block_matches_rfc8439_2_3_2`) — prouve la **conformité au standard** de la primitive manuelle (pas juste l'auto-cohérence). **31 tests crypto kernel verts.**

**Serveur `crypto_server`** — 2 faiblesses **critiques** trouvées + corrigées + dédup `secure_random` (CSPRNG kernel `getrandom`) :
- [x] **`tls.rs` — clé X25519 éphémère par LCG** : `generate_x25519_keypair` seedait la **clé privée (forward secrecy)** via un LCG sur `tsc ^ rdrand_u64()` (RDRAND **sans contrôle CF** → 0/garbage), 256 bits réduits à ≤64 bits inversibles → **confidentialité du canal nulle**. → **CSPRNG kernel** (`fill_random`→`secure_random`), **échec→abandon** du handshake (jamais de clé faible).
- [x] **`pki.rs` — clé privée Root CA = `[0u8;32]`** (tout-zéros) : reconstructible par quiconque → forge de **tout** certificat, usurpation de **tout** service. → clé racine générée par **CSPRNG**, **jamais exportée** (mémoire isolée, `ROOT_PRIVATE_KEY`), wipe pile, **fail-closed** si pas d'entropie ; `root_sign` pour les intermédiaires.
- [x] **`keystore.rs`** : `rotate_key` XOR+FNV → **HKDF-Blake3** (fait précédemment). `xchacha20.rs`/`main.rs`/handshake = RustCrypto réel, cap-gated. *Reste (follow-up honnête, valeur marginale)* : scellement KEK des clés en RAM — l'isolation de process protège déjà, pas de persistance disque ; vrai scellement = PKS/TPM (intégration matérielle future).
- **Validation** : `cargo check -p exo-crypto-server` OK + build ISO bare-metal en cours.
