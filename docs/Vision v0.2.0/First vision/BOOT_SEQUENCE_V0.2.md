# BOOT_SEQUENCE_V0.2 — Séquence de boot sécurité

**Statut :** référence v0.2.0 corrigée  
**But :** fixer l'ordre d'initialisation observable avant Ring0 -> Ring1, y compris les contraintes ExoPhoenix.

---

## Invariants d'ordre

1. `memory_init()` prépare les structures minimales avant tout allocateur dynamique.
2. `arch_init()` installe GDT/IDT, APIC/LAPIC et sources de temps avant les modules qui consomment IRQ/NMI.
3. SMEP, SMAP, NX/XD et KPTI doivent être actifs avant les chemins qui exécutent ou exposent du code Ring3.
4. ExoCage/CET et ExoNMI sont initialisés avant le handoff Ring1.
5. `security_init()` se termine par `SECURITY_READY = true`; aucun AP ne doit servir IPC/ExoFS avant ce flag.
6. La SSR ExoPhoenix est la fenêtre physique `[0x0100_0000..0x0110_0000)` et reste exclue des plages utilisables.
7. Les services Ring1 démarrent par vagues de dépendances, pas en chaîne séquentielle stricte.

---

## Déroulé

| Phase | Module | Condition de sortie |
|---|---|---|
| 0 | Bootloader / `exo-boot` | BootInfo valide, memory map transmise, SSR non allouable |
| 1 | `memory::early` | Physmap initiale 1 GiB, extension selon RAM détectée |
| 2 | `arch::x86_64` | GDT/IDT/syscall/APIC installés |
| 3 | Mitigations CPU | NX/XD, SMEP, SMAP, KPTI, canaries prêts |
| 4 | ExoPhoenix SSR | magic/version v7 validés, région 64 KiB cohérente |
| 5 | ExoCage / ExoNMI / ExoArgos | CET/NMI/PMC prêts avant exposition Ring1 |
| 6 | `security_init()` | capability, zero-trust, crypto, audit, ExoLedger, ExoKairos prêts |
| 7 | Scheduler + IPC | endpoints et policy Ring1 utilisables après `SECURITY_READY` |
| 8 | `init_server` | services Ring1 démarrés par vagues, readiness confirmée |
| 9 | `exosh` | shell texte disponible sans dépendre du réseau |

---

## Boot Ring1 par vagues

`init_server` démarre les services dont les dépendances sont satisfaites, puis attend la readiness de toute la vague en parallèle. Une vague lente ne doit pas bloquer le spawn d'un service indépendant dans la vague suivante si ses dépendances sont prêtes.

Ordre logique attendu :

1. `memory_server`, `scheduler_server`, `crypto_server`
2. `device_server`, `virtio_drivers`
3. `vfs_server`
4. `input_server`, `tty_server`, `network_server`
5. `exo_shield`
6. `exosh`

La cible de recovery ExoPhoenix reste `< 500 ms` pour les services critiques déjà chauds; le boot froid peut utiliser les timeouts de readiness plus larges documentés dans `service_table.rs`.

---

## Points de validation

- `kernel/src/arch/constants.rs` porte les constantes SSR, physmap, IPC et ExoKairos auditées.
- `kernel/src/exophoenix/ssr.rs` vérifie à la compilation la SSR 64 KiB et le budget de métadonnées recovery 4 KiB.
- `kernel/src/arch/x86_64/boot/memory_map.rs` exclut la SSR des sous-plages utilisables avant insertion dans les allocateurs.
- `servers/init_server/src/boot_sequence.rs` utilise le démarrage par vagues.
- `servers/init_server/src/service_table.rs` ne rend pas `exosh` dépendant du réseau pour être disponible.

---

## Hors périmètre v0.2.0

- Wayland, `wgpu`, `winit`, `iced`
- Installateur graphique
- Bureau multi-fenêtres

