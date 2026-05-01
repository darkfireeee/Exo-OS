# Validation sécurité 2026-05-01

## Périmètre

- `servers/exo_shield`
- `kernel/src/security/*`
- `kernel/src/exophoenix/*`
- `servers/crypto_server`

## Correctifs couverts

- Extension du modèle capability noyau sur `exo_shield` au-delà des mutations administratives.
- Les requêtes `SCAN_REQUEST`, `EVENT_REPORT` et `THREAT_QUERY` passent désormais par une classification explicite:
  - self-service sur le PID appelant: public + audité + borné,
  - action inter-processus ou requête détaillée: capability noyau obligatoire.
- Le contrat documentaire serveur reflète maintenant ce découpage exact.

## Validation WSL

Suite standard:

- `./run_tests.sh --verbose`
  - `PASS: 25`
  - `FAIL: 0`
  - `SKIP: 6`

Tests ciblés sécurité / Phoenix:

- `cargo test -p exo-shield --lib ipc_gate:: -- --nocapture`
- `cargo test -p exo-os-kernel --lib security::capability::tests -- --nocapture`
- `cargo test -p exo-os-kernel --lib ipc_policy::tests -- --nocapture`
- `cargo test -p exo-phoenix-ssr -- --nocapture`

Stress tests exécutés:

- `cargo test -p exo-os-kernel --lib test_raw_mailbox_send_recv_stress -- --nocapture`
- `cargo test -p exo-os-kernel --lib test_call_raw_roundtrip_stress -- --nocapture`
- `cargo test -p exo-os-kernel --lib test_01_iommu_queue_hft_smp_stress -- --nocapture`
- `cargo test -p exo-os-kernel --lib test_apply_tsc_offset_stress_roundtrip -- --nocapture`
- `cargo test -p exo-os-kernel --lib emergency_pool_double_init_stress_preserves_capacity -- --nocapture`

Résultat: tous les tests ci-dessus passent.

## Validation QEMU

Boot framebuffer réel:

- `scripts/qemu/capture_boot_framebuffer.sh`
- Capture produite: [exoos-qemu-latest.png](/C:/Users/xavie/Desktop/Exo-OS/docs/avancement/qemu_boot/exoos-qemu-latest.png)

Constats:

- le framebuffer atteint `EXO-OS KERNEL BOOT COMPLETE`,
- les étapes `ARCH`, `MEMORY`, `TIME`, `DRIVERS`, `SCHEDULER`, `PROCESS`, `SECURITY`, `IPC`, `FS` sont toutes visibles en `OK`,
- le port debug `0xE9` se termine par `OK`.

Boot SMP QEMU:

- `qemu-system-x86_64 -machine q35 -smp 4 ...`
- le log `0xE9` atteint également `OK` après fenêtre plus longue.

## Limites constatées

- La build QEMU disponible dans cet environnement n'expose pas la feature CPU `cet` sur `qemu64`; un smoke test `-cpu qemu64,+cet` ne peut donc pas être exécuté ici.
- Aucun harness QEMU dédié de cycle `freeze -> restore -> PhoenixWakeEntropy -> reprise` n'est présent dans le dépôt à cette date.
- En conséquence, le boot QEMU valide le démarrage réel et la stabilité SMP, mais pas encore un handoff ExoPhoenix complet injecté à chaud.

## Alignement TLA / documentation

Les preuves TLA+ n'ont pas été relancées, conformément à la consigne et aux sorties déjà présentes dans `docs/Exo-OS-TLA+/`.

Alignement structurel confirmé:

- `CAP-01`: la vérification d'autorité repose sur `kernel/src/security/capability/` et non sur un simple PID forgeable.
- ExoShield serveur: autorité IPC bornée, auditée, capability-gated sur les actions inter-processus.
- ExoPhoenix v7: le chemin post-restore envoie bien l'entropie vers `crypto_server` avant la reprise normale, via `kernel/src/exophoenix/handoff.rs`.

## Prochaine marche utile

Pour valider ExoPhoenix de bout en bout sur QEMU, il manque surtout un test d'intégration dédié qui déclenche réellement la séquence de handoff et vérifie:

- passage par `PrepareIsolation`,
- reconstruction,
- envoi de `PhoenixWakeEntropy`,
- nonces crypto renouvelés,
- reprise sans trafic IPC parasite avant reseed.
