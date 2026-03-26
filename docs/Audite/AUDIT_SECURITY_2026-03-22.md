# Audit approfondi du module `security` (Exo-OS)

Date: 2026-03-22
Périmètre: `kernel/src/security/**`
Objectif: base de refonte/correction sécurité noyau

---

## 1) Résumé exécutif

Le module `security` est la couche 2b.
Il regroupe capacités, contrôle d’accès, zero-trust, crypto, isolation, intégrité, mitigations, audit.
Il est critique pour robustesse et surface d’attaque.

Forces:
- structure complète et modulaire.
- orchestrateur `security_init` avec ordre explicite.
- drapeau `SECURITY_READY` pour synchronisation boot SMP.
- API de contrôle d’accès unifiée.

Fragilités:
- présence de stubs crypto (AES-GCM, XChaCha20-Poly1305 cible).
- parties namespace/sandbox encore à consolider.
- logique multi-sous-systèmes très couplée à l’ordre d’initialisation.
- gestion fine de `capability` à préserver sans régression ABI interne.

---

## 2) Positionnement et invariants

`security` doit être initialisé avant usage des couches consommatrices.
`security` doit publier son état de readiness pour APs.
`security` doit garder des interfaces stables vers IPC/FS/process.
`security` doit centraliser autorisations sur un chemin unique de vérification.
`security` doit tracer les denials importants.

Invariants clés:
- `integrity_init()` exécuté en premier.
- `SECURITY_READY` passe à true en fin d’init.
- capability subsystem initialisé une seule fois.
- access control initialisé avant usages transverses.

---

## 3) Cartographie exhaustive des fichiers

### 3.1 Racine
- `kernel/src/security/mod.rs`

### 3.2 `access_control`
- `kernel/src/security/access_control/mod.rs`
- `kernel/src/security/access_control/checker.rs`
- `kernel/src/security/access_control/object_types.rs`

### 3.3 `audit`
- `kernel/src/security/audit/mod.rs`
- `kernel/src/security/audit/logger.rs`
- `kernel/src/security/audit/rules.rs`
- `kernel/src/security/audit/syscall_audit.rs`

### 3.4 `capability`
- `kernel/src/security/capability/mod.rs`
- `kernel/src/security/capability/delegation.rs`
- `kernel/src/security/capability/namespace.rs`
- `kernel/src/security/capability/revocation.rs`
- `kernel/src/security/capability/rights.rs`
- `kernel/src/security/capability/table.rs`
- `kernel/src/security/capability/token.rs`
- `kernel/src/security/capability/verify.rs`

### 3.5 `crypto`
- `kernel/src/security/crypto/mod.rs`
- `kernel/src/security/crypto/aes_gcm.rs`
- `kernel/src/security/crypto/blake3.rs`
- `kernel/src/security/crypto/ed25519.rs`
- `kernel/src/security/crypto/kdf.rs`
- `kernel/src/security/crypto/rng.rs`
- `kernel/src/security/crypto/x25519.rs`
- `kernel/src/security/crypto/xchacha20_poly1305.rs`

### 3.6 `exploit_mitigations`
- `kernel/src/security/exploit_mitigations/mod.rs`
- `kernel/src/security/exploit_mitigations/cet.rs`
- `kernel/src/security/exploit_mitigations/cfg.rs`
- `kernel/src/security/exploit_mitigations/kaslr.rs`
- `kernel/src/security/exploit_mitigations/safe_stack.rs`
- `kernel/src/security/exploit_mitigations/stack_protector.rs`

### 3.7 `integrity_check`
- `kernel/src/security/integrity_check/mod.rs`
- `kernel/src/security/integrity_check/code_signing.rs`
- `kernel/src/security/integrity_check/runtime_check.rs`
- `kernel/src/security/integrity_check/secure_boot.rs`

### 3.8 `isolation`
- `kernel/src/security/isolation/mod.rs`
- `kernel/src/security/isolation/domains.rs`
- `kernel/src/security/isolation/namespaces.rs`
- `kernel/src/security/isolation/pledge.rs`
- `kernel/src/security/isolation/sandbox.rs`

### 3.9 `zero_trust`
- `kernel/src/security/zero_trust/mod.rs`
- `kernel/src/security/zero_trust/context.rs`
- `kernel/src/security/zero_trust/labels.rs`
- `kernel/src/security/zero_trust/policy.rs`
- `kernel/src/security/zero_trust/verify.rs`

---

## 4) APIs publiques structurantes

Exports majeurs de `security/mod.rs`:
- readiness: `SECURITY_READY`, `is_security_ready`
- capability: `CapToken`, `Rights`, `verify`, `revoke`, `delegate`, `init_capability_subsystem`
- access control: `check_access`, `ObjectKind`, `AccessError`
- zero trust: `SecurityLabel`, `SecurityContext`, `verify_access`
- crypto: `crypto_init`, `rng_fill`, `rng_u64`, `blake3_hash`, `blake3_mac`
- isolation: `SecurityDomain`, `DomainContext`, `SandboxPolicy`, `NamespaceSet`, `PledgeSet`
- integrity: `integrity_init`, `check_kernel_integrity`, `verify_module_signature`, `check_chain_of_trust`
- mitigations: `mitigations_init`, `kaslr_offset`, `cfg_validate_indirect_call`, `safe_stack_*`
- audit: `audit_init`, `log_event`, `audit_syscall_entry`, `audit_syscall_exit`, `audit_capability_deny`

Init orchestrée:
- `security_init(kaslr_entropy, phys_base)`

---

## 5) État fonctionnel et risques

Points robustes:
- séquence init explicitée.
- sous-systèmes bien découpés.
- tables capability centralisées.
- audit subsystem présent.

Risques:
- stubs crypto cibles no_std limitant certaines fonctionnalités.
- complexité des interactions access_control + zero_trust + capability.
- risques de fenêtre SMP si `SECURITY_READY` mal consommé côté AP.

---

## 6) Concurrence et primitives de synchro

Patterns observés:
- `AtomicBool` (readiness/security state)
- `spin::Mutex` dans plusieurs tables globales (namespaces, audit rules/logger, secure boot state)
- `SpinLock` côté capability table kernel
- atomiques de stats/compteurs dans plusieurs sous-modules

Points d’attention:
- contention sur tables globales sous charge élevée.
- importance des orderings Acquire/Release sur readiness.

---

## 7) TODO/stub/placeholder relevés

- `crypto/xchacha20_poly1305.rs` : STUB cible indisponible.
- `crypto/aes_gcm.rs` : STUB cible indisponible.
- `capability/mod.rs` : wrappers POSIX capget/capset en NotSupported.
- placeholders documentés dans `integrity_check/code_signing.rs`.

Impact refonte:
- fonctionnalités crypto à prioriser selon roadmap.
- clarifier stratégie de fallback et de désactivation.

---

## 8) Usage `&str` et interfaces texte

`&str` apparaît dans:
- logs/audit textuels.
- helpers affichage rights/catégories.
- encodage tags syscall audit.

Règle refonte:
- limiter formatage dynamique dans chemins sensibles.
- garder diagnostics exploitables sans surcoût excessif.

---

## 9) Crates/imports majeurs

Imports fréquents:
- `core::sync::atomic::*`
- `spin::Mutex`
- modules internes `crate::security::*`
- dépendances vers scheduler sync pour certains locks capability

Contrainte no_std:
- éviter dépendances crypto lourdes non supportées cible.

---

## 10) Journal de contrôle détaillé (SEC-CHK)

- SEC-CHK-001 vérifier `security_init` appelé exactement une fois.
- SEC-CHK-002 vérifier `integrity_init` exécuté en premier.
- SEC-CHK-003 vérifier `capability` init après integrity.
- SEC-CHK-004 vérifier `crypto_init` après capability.
- SEC-CHK-005 vérifier `audit_init` exécuté avant readiness final.
- SEC-CHK-006 vérifier `access_control::init` exécuté.
- SEC-CHK-007 vérifier `SECURITY_READY.store(Release)` final.
- SEC-CHK-008 vérifier lecture readiness en Acquire côté consommateurs.
- SEC-CHK-009 vérifier APs SMP attendent readiness.
- SEC-CHK-010 vérifier absence fenêtre BOOT-SEC.
- SEC-CHK-011 vérifier `is_security_ready` utilisé correctement.
- SEC-CHK-012 vérifier aucun check access avant init.
- SEC-CHK-013 vérifier capability double-init panic attendu.
- SEC-CHK-014 vérifier `KERNEL_CAP_TABLE` init non-nulle.
- SEC-CHK-015 vérifier object id counter monotone.
- SEC-CHK-016 vérifier object id overflow planifié.
- SEC-CHK-017 vérifier Rights bitmask validation stricte.
- SEC-CHK-018 vérifier CapObjectType conversion stricte.
- SEC-CHK-019 vérifier `grant` erreurs mappées proprement.
- SEC-CHK-020 vérifier revoke O(1) réel.
- SEC-CHK-021 vérifier revocation génération atomique.
- SEC-CHK-022 vérifier delegation subset invariants.
- SEC-CHK-023 vérifier delegation chain limites.
- SEC-CHK-024 vérifier namespace isolation tokens.
- SEC-CHK-025 vérifier verify() chemin unique pour checks.
- SEC-CHK-026 vérifier verify_typed cohérence.
- SEC-CHK-027 vérifier verify_read/write/ipc wrappers.
- SEC-CHK-028 vérifier cross_namespace_verify sécurité.
- SEC-CHK-029 vérifier cap table lecture lock-free sécurité.
- SEC-CHK-030 vérifier cap table mutation lockée.
- SEC-CHK-031 vérifier AccessControl appel capability verify.
- SEC-CHK-032 vérifier ObjectKind mapping droits correct.
- SEC-CHK-033 vérifier AccessError taxonomie utile.
- SEC-CHK-034 vérifier deny path audit log.
- SEC-CHK-035 vérifier allow path minimal overhead.
- SEC-CHK-036 vérifier zero_trust labels cohérents.
- SEC-CHK-037 vérifier policy Bell-LaPadula.
- SEC-CHK-038 vérifier policy Biba.
- SEC-CHK-039 vérifier contexte trust levels validés.
- SEC-CHK-040 vérifier verify_access deterministic.
- SEC-CHK-041 vérifier denial logging zero_trust.
- SEC-CHK-042 vérifier exceptions policy explicitement gérées.
- SEC-CHK-043 vérifier crypto module exports stables.
- SEC-CHK-044 vérifier blake3 hash correctness tests.
- SEC-CHK-045 vérifier blake3 mac checks.
- SEC-CHK-046 vérifier kdf derive deterministic.
- SEC-CHK-047 vérifier kdf labels non ambiguës.
- SEC-CHK-048 vérifier x25519 keypair generation.
- SEC-CHK-049 vérifier x25519 DH output sanity.
- SEC-CHK-050 vérifier ed25519 sign/verify roundtrip.
- SEC-CHK-051 vérifier rng init appelé avant usage.
- SEC-CHK-052 vérifier rng ready flag.
- SEC-CHK-053 vérifier rng fill error handling.
- SEC-CHK-054 vérifier rng stats non divulgue secrets.
- SEC-CHK-055 vérifier constant_time_eq utilisée.
- SEC-CHK-056 vérifier zeroization clés si wrappers.
- SEC-CHK-057 vérifier nonce reuse impossible.
- SEC-CHK-058 vérifier stub AES_GCM explicitement signalé.
- SEC-CHK-059 vérifier stub XChaCha20 explicitement signalé.
- SEC-CHK-060 vérifier fallback crypto côté appelants.
- SEC-CHK-061 vérifier integrity runtime hash init.
- SEC-CHK-062 vérifier hash baseline `.text/.rodata`.
- SEC-CHK-063 vérifier periodic checks non bloquants.
- SEC-CHK-064 vérifier assert integrity panic policy.
- SEC-CHK-065 vérifier code_signing registry locks.
- SEC-CHK-066 vérifier verify_module_signature erreurs.
- SEC-CHK-067 vérifier secure_boot chain flags.
- SEC-CHK-068 vérifier pcr bank mutex safety.
- SEC-CHK-069 vérifier boot_nonce handling.
- SEC-CHK-070 vérifier chain_of_trust semantics.
- SEC-CHK-071 vérifier exploit mitigations init order.
- SEC-CHK-072 vérifier KASLR offset set once.
- SEC-CHK-073 vérifier is_kernel_address coherence.
- SEC-CHK-074 vérifier safe pointer checks robustes.
- SEC-CHK-075 vérifier stack canary table lock.
- SEC-CHK-076 vérifier canary check path fast.
- SEC-CHK-077 vérifier cfg table mutation lockée.
- SEC-CHK-078 vérifier cfg_validate_indirect_call coverage.
- SEC-CHK-079 vérifier cfg lock prevents post-init mutation.
- SEC-CHK-080 vérifier cet support gating.
- SEC-CHK-081 vérifier cet status reporting.
- SEC-CHK-082 vérifier safe_stack new/remove thread.
- SEC-CHK-083 vérifier safe_stack check frequent path.
- SEC-CHK-084 vérifier isolation domains lifecycle.
- SEC-CHK-085 vérifier namespaces registry integrity.
- SEC-CHK-086 vérifier namespace id counter atomic.
- SEC-CHK-087 vérifier sandbox policy default restrictive.
- SEC-CHK-088 vérifier deny_enosys semantics.
- SEC-CHK-089 vérifier syscall max bounds sandbox.
- SEC-CHK-090 vérifier pledge restrictions mapping.
- SEC-CHK-091 vérifier pledge to sandbox coherence.
- SEC-CHK-092 vérifier audit logger ring lock.
- SEC-CHK-093 vérifier audit ring overwrite policy.
- SEC-CHK-094 vérifier flush_to_userspace semantics.
- SEC-CHK-095 vérifier audit rules global lock.
- SEC-CHK-096 vérifier evaluate rules deterministic.
- SEC-CHK-097 vérifier syscall_audit entry/exit pairing.
- SEC-CHK-098 vérifier rule action deny_enosys path.
- SEC-CHK-099 vérifier audit category taxonomy.
- SEC-CHK-100 vérifier outcome taxonomy.
- SEC-CHK-101 vérifier event serialization stable.
- SEC-CHK-102 vérifier privacy filtering logs.
- SEC-CHK-103 vérifier spam control audit logs.
- SEC-CHK-104 vérifier stat counters atomiques.
- SEC-CHK-105 vérifier relaxed ordering only on metrics.
- SEC-CHK-106 vérifier acquire/release on state flags.
- SEC-CHK-107 vérifier seqcst only if mandatory.
- SEC-CHK-108 vérifier no deadlock locks inter-modules.
- SEC-CHK-109 vérifier security locks ordering documented.
- SEC-CHK-110 vérifier interaction scheduler locks.
- SEC-CHK-111 vérifier interaction memory locks.
- SEC-CHK-112 vérifier interaction ipc checks.
- SEC-CHK-113 vérifier interaction fs checks.
- SEC-CHK-114 vérifier interaction process checks.
- SEC-CHK-115 vérifier exports API minimaux.
- SEC-CHK-116 vérifier imports std absents no_std.
- SEC-CHK-117 vérifier cfg(not(test)) handlers corrects.
- SEC-CHK-118 vérifier compile target bare-metal.
- SEC-CHK-119 vérifier warnings zero sur modules clés.
- SEC-CHK-120 vérifier unsafe blocs annotés SAFETY.
- SEC-CHK-121 vérifier ptr casts justifiés.
- SEC-CHK-122 vérifier integer casts sécurisés.
- SEC-CHK-123 vérifier overflow checked/saturating.
- SEC-CHK-124 vérifier error enums stables.
- SEC-CHK-125 vérifier kernel errno mapping.
- SEC-CHK-126 vérifier capset/capget notsupported explicite.
- SEC-CHK-127 vérifier returned errno -38 cohérent.
- SEC-CHK-128 vérifier docs security alignées code.
- SEC-CHK-129 vérifier docs capability alignées code.
- SEC-CHK-130 vérifier docs access_control alignées.
- SEC-CHK-131 vérifier docs zero_trust alignées.
- SEC-CHK-132 vérifier docs crypto alignées.
- SEC-CHK-133 vérifier docs integrity alignées.
- SEC-CHK-134 vérifier docs isolation alignées.
- SEC-CHK-135 vérifier docs mitigations alignées.
- SEC-CHK-136 vérifier docs audit alignées.
- SEC-CHK-137 vérifier tests capability grant/revoke.
- SEC-CHK-138 vérifier tests delegation subset.
- SEC-CHK-139 vérifier tests verify token invalid.
- SEC-CHK-140 vérifier tests access control deny.
- SEC-CHK-141 vérifier tests zero_trust policy.
- SEC-CHK-142 vérifier tests rng readiness.
- SEC-CHK-143 vérifier tests blake3 vectors.
- SEC-CHK-144 vérifier tests kdf vectors.
- SEC-CHK-145 vérifier tests x25519 vectors.
- SEC-CHK-146 vérifier tests ed25519 vectors.
- SEC-CHK-147 vérifier tests integrity baseline.
- SEC-CHK-148 vérifier tests secure_boot chain.
- SEC-CHK-149 vérifier tests cfg allow/deny.
- SEC-CHK-150 vérifier tests stack canary breach.
- SEC-CHK-151 vérifier tests sandbox syscall filter.
- SEC-CHK-152 vérifier tests namespace create/destroy.
- SEC-CHK-153 vérifier tests pledge restrictions.
- SEC-CHK-154 vérifier tests audit ring overflow.
- SEC-CHK-155 vérifier tests syscall_audit hooks.
- SEC-CHK-156 vérifier tests readiness smp wait.
- SEC-CHK-157 vérifier tests double init panic.
- SEC-CHK-158 vérifier tests lock contention tables.
- SEC-CHK-159 vérifier stress tests denials.
- SEC-CHK-160 vérifier stress tests crypto throughput.
- SEC-CHK-161 vérifier stress tests audit throughput.
- SEC-CHK-162 vérifier perf overhead access check.
- SEC-CHK-163 vérifier perf overhead audit enabled.
- SEC-CHK-164 vérifier perf overhead mitigations.
- SEC-CHK-165 vérifier side-channel surfaces inventory.
- SEC-CHK-166 vérifier timing attacks check paths.
- SEC-CHK-167 vérifier cache attacks mitigations.
- SEC-CHK-168 vérifier speculation hardening suffisant.
- SEC-CHK-169 vérifier secret material lifetime.
- SEC-CHK-170 vérifier memory scrubbing strategy.
- SEC-CHK-171 vérifier key rotation plan.
- SEC-CHK-172 vérifier key derivation domain separation.
- SEC-CHK-173 vérifier nonce generation monotonicity.
- SEC-CHK-174 vérifier random source entropy baseline.
- SEC-CHK-175 vérifier fallback RNG behavior.
- SEC-CHK-176 vérifier secure failure defaults.
- SEC-CHK-177 vérifier deny-by-default policy.
- SEC-CHK-178 vérifier allowlist explicitness.
- SEC-CHK-179 vérifier object kind coverage complète.
- SEC-CHK-180 vérifier permission escalation impossible.
- SEC-CHK-181 vérifier revocation propagation complete.
- SEC-CHK-182 vérifier stale token rejection.
- SEC-CHK-183 vérifier namespace crossing controls.
- SEC-CHK-184 vérifier privilege boundaries.
- SEC-CHK-185 vérifier kernel/user boundary checks.
- SEC-CHK-186 vérifier panic strategy sur corruption.
- SEC-CHK-187 vérifier triage log quality.
- SEC-CHK-188 vérifier event correlation IDs.
- SEC-CHK-189 vérifier incident response hooks.
- SEC-CHK-190 vérifier policy hot reload roadmap.
- SEC-CHK-191 vérifier config surfaces minimales.
- SEC-CHK-192 vérifier compile-time feature toggles.
- SEC-CHK-193 vérifier release hardening defaults.
- SEC-CHK-194 vérifier debug modes non dangereux.
- SEC-CHK-195 vérifier ownership des sous-modules.
- SEC-CHK-196 vérifier code review obligatoire sécurité.
- SEC-CHK-197 vérifier backlog CVE-like entries.
- SEC-CHK-198 vérifier remediation SLA.
- SEC-CHK-199 vérifier alerting rules audit.
- SEC-CHK-200 vérifier monitoring counters export.
- SEC-CHK-201 vérifier secure boot logs.
- SEC-CHK-202 vérifier chain failure handling.
- SEC-CHK-203 vérifier module signature trust roots.
- SEC-CHK-204 vérifier trust root rotation plan.
- SEC-CHK-205 vérifier dependency on external entropy.
- SEC-CHK-206 vérifier no hidden backdoor paths.
- SEC-CHK-207 vérifier no debug bypass in release.
- SEC-CHK-208 vérifier no capability bypass in IPC/FS.
- SEC-CHK-209 vérifier direct verify calls hors policy.
- SEC-CHK-210 vérifier migration plan towards full crypto.
- SEC-CHK-211 vérifier migration plan namespace maturity.
- SEC-CHK-212 vérifier migration plan sandbox strict mode.
- SEC-CHK-213 vérifier migration plan policy compiler.
- SEC-CHK-214 vérifier roadmap documentation alignée.
- SEC-CHK-215 vérifier CI gates sécurité.
- SEC-CHK-216 vérifier fuzzing sur parser/policies.
- SEC-CHK-217 vérifier property tests rights algebra.
- SEC-CHK-218 vérifier formal checks envisageables.
- SEC-CHK-219 vérifier long-run stability tests.
- SEC-CHK-220 vérifier memory leaks absence.
- SEC-CHK-221 vérifier lockdep-like checks potentiels.
- SEC-CHK-222 vérifier deadlock scenarios revus.
- SEC-CHK-223 vérifier fail-open scenarios absents.
- SEC-CHK-224 vérifier fail-closed scenarios corrects.
- SEC-CHK-225 vérifier secure defaults documented.
- SEC-CHK-226 vérifier API deprecations planifiées.
- SEC-CHK-227 vérifier backward compat constraints.
- SEC-CHK-228 vérifier binary size impact security.
- SEC-CHK-229 vérifier boot time impact security init.
- SEC-CHK-230 vérifier runtime impact access checks.
- SEC-CHK-231 vérifier runtime impact audit heavy.
- SEC-CHK-232 vérifier metrics budgets fixés.
- SEC-CHK-233 vérifier perf/security tradeoffs explicités.
- SEC-CHK-234 vérifier emergency disable procedures.
- SEC-CHK-235 vérifier rollback strategy sécurité.
- SEC-CHK-236 vérifier release checklist sécurité.
- SEC-CHK-237 vérifier post-release monitoring.
- SEC-CHK-238 vérifier incident playbook.
- SEC-CHK-239 vérifier threat model à jour.
- SEC-CHK-240 vérifier trust assumptions à jour.
- SEC-CHK-241 vérifier architecture diagrams à jour.
- SEC-CHK-242 vérifier codeowners sécurité.
- SEC-CHK-243 vérifier KPI sécurité suivis.
- SEC-CHK-244 vérifier objective pass/fail criteria.
- SEC-CHK-245 vérifier readiness for refonte lot 1.
- SEC-CHK-246 vérifier readiness for refonte lot 2.
- SEC-CHK-247 vérifier readiness for refonte lot 3.
- SEC-CHK-248 vérifier priorisation des stubs crypto.
- SEC-CHK-249 vérifier priorisation readiness SMP.
- SEC-CHK-250 vérifier priorisation access control hardening.
- SEC-CHK-251 vérifier priorisation audit reliability.
- SEC-CHK-252 vérifier priorisation integrity checks.
- SEC-CHK-253 vérifier priorisation isolation maturity.
- SEC-CHK-254 vérifier synthèse risques finale.
- SEC-CHK-255 vérifier plan d’action exécutable.
- SEC-CHK-256 vérifier responsables nommés par lot.
- SEC-CHK-257 vérifier échéances réalistes.
- SEC-CHK-258 vérifier dépendances inter-lots.
- SEC-CHK-259 vérifier critères de validation finale.
- SEC-CHK-260 vérifier clôture audit sécurité.

---

## 11) Conclusion

Le module `security` est ambitieux et déjà structuré.
La refonte doit sécuriser l’ordre init et les chemins d’autorisation.
Les stubs crypto doivent être traités explicitement dans une stratégie cible.
`SECURITY_READY` et la consommation côté SMP restent prioritaires.
Ce document sert de base de correction incrémentale et vérifiable.

## 12) Addendum de validation Security (complément 500+)

- SEC-ADD-001 valider matrice menaces par sous-module.
- SEC-ADD-002 valider matrice actifs critiques protégés.
- SEC-ADD-003 valider matrice surfaces d’attaque externes.
- SEC-ADD-004 valider matrice surfaces d’attaque internes.
- SEC-ADD-005 valider matrice hypothèses de confiance.
- SEC-ADD-006 valider matrice hypothèses de compromission.
- SEC-ADD-007 valider matrice impacts CIA par composant.
- SEC-ADD-008 valider matrice priorité remédiations.
- SEC-ADD-009 valider matrice dépendances sécurité inter-couches.
- SEC-ADD-010 valider matrice couverture contrôles existants.
- SEC-ADD-011 valider revue cryptographie par expert.
- SEC-ADD-012 valider revue capability model par expert.
- SEC-ADD-013 valider revue access control par expert.
- SEC-ADD-014 valider revue zero-trust policy par expert.
- SEC-ADD-015 valider revue isolation/sandbox par expert.
- SEC-ADD-016 valider revue integrity chain par expert.
- SEC-ADD-017 valider revue mitigations exploit par expert.
- SEC-ADD-018 valider revue audit forensics par expert.
- SEC-ADD-019 valider budget overhead sécurité global.
- SEC-ADD-020 valider budget overhead access checks.
- SEC-ADD-021 valider budget overhead capability verify.
- SEC-ADD-022 valider budget overhead zero-trust verify.
- SEC-ADD-023 valider budget overhead audit emission.
- SEC-ADD-024 valider budget overhead integrity periodic.
- SEC-ADD-025 valider budget overhead mitigations runtime.
- SEC-ADD-026 valider budget overhead sandbox filtering.
- SEC-ADD-027 valider budget overhead namespace checks.
- SEC-ADD-028 valider budget overhead secure boot checks.
- SEC-ADD-029 valider stratégie en cas d’échec integrity.
- SEC-ADD-030 valider stratégie en cas d’échec capability.
- SEC-ADD-031 valider stratégie en cas d’échec access control.
- SEC-ADD-032 valider stratégie en cas d’échec zero-trust.
- SEC-ADD-033 valider stratégie en cas d’échec crypto.
- SEC-ADD-034 valider stratégie en cas d’échec rng.
- SEC-ADD-035 valider stratégie en cas d’échec audit.
- SEC-ADD-036 valider stratégie en cas d’échec mitigations.
- SEC-ADD-037 valider stratégie en cas d’échec sandbox.
- SEC-ADD-038 valider stratégie en cas d’échec secure boot.
- SEC-ADD-039 valider tests fault injection cap table.
- SEC-ADD-040 valider tests fault injection policy engine.
- SEC-ADD-041 valider tests fault injection rng.
- SEC-ADD-042 valider tests fault injection audit logger.
- SEC-ADD-043 valider tests fault injection integrity checks.
- SEC-ADD-044 valider tests fault injection mitigations cfg.
- SEC-ADD-045 valider tests fault injection stack protector.
- SEC-ADD-046 valider tests fault injection safe_stack.
- SEC-ADD-047 valider tests fault injection sandbox.
- SEC-ADD-048 valider tests fault injection namespace.
- SEC-ADD-049 valider plan migration sortie stubs AES-GCM.
- SEC-ADD-050 valider plan migration sortie stubs XChaCha20.
- SEC-ADD-051 valider plan migration génération clés durcies.
- SEC-ADD-052 valider plan migration rotation clés durcies.
- SEC-ADD-053 valider plan migration stockage clés durcies.
- SEC-ADD-054 valider plan migration labels zero-trust.
- SEC-ADD-055 valider plan migration policy compiler.
- SEC-ADD-056 valider plan migration sandbox strict mode.
- SEC-ADD-057 valider plan migration namespace hardening.
- SEC-ADD-058 valider plan migration secure boot avancé.
- SEC-ADD-059 valider conformité logs aux exigences privacy.
- SEC-ADD-060 valider conformité logs aux exigences forensic.
- SEC-ADD-061 valider conformité logs aux exigences retention.
- SEC-ADD-062 valider conformité logs aux exigences intégrité.
- SEC-ADD-063 valider conformité logs aux exigences rotation.
- SEC-ADD-064 valider conformité logs aux exigences accès.
- SEC-ADD-065 valider conformité logs aux exigences export.
- SEC-ADD-066 valider conformité logs aux exigences purge.
- SEC-ADD-067 valider gouvernance des clés maîtresses.
- SEC-ADD-068 valider gouvernance des clés session.
- SEC-ADD-069 valider gouvernance des clés module.
- SEC-ADD-070 valider gouvernance des roots of trust.
- SEC-ADD-071 valider gouvernance des exceptions policy.
- SEC-ADD-072 valider gouvernance des bypass debug.
- SEC-ADD-073 valider gouvernance des features expérimentales.
- SEC-ADD-074 valider gouvernance des revocations massives.
- SEC-ADD-075 valider gouvernance des incidents sécurité.
- SEC-ADD-076 valider gouvernance des postmortems sécurité.
- SEC-ADD-077 valider gouvernance des release gates sécurité.
- SEC-ADD-078 valider gouvernance des KPI sécurité.
- SEC-ADD-079 valider gouvernance des ownerships sécurité.
- SEC-ADD-080 valider gouvernance des SLA de correction.
- SEC-ADD-081 valider preuve de conformité du lot S1.
- SEC-ADD-082 valider preuve de conformité du lot S2.
- SEC-ADD-083 valider preuve de conformité du lot S3.
- SEC-ADD-084 valider preuve de conformité du lot S4.
- SEC-ADD-085 valider preuve de conformité du lot S5.
- SEC-ADD-086 valider preuves de non-régression sécurité.
- SEC-ADD-087 valider critères de sortie sécurité refonte.
- SEC-ADD-088 valider responsables des lots sécurité.
- SEC-ADD-089 valider échéancier sécurité réaliste.
- SEC-ADD-090 valider clôture d’audit sécurité avec preuves.
