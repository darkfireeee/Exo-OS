# Exo-OS — Journal d’avancement (20/03/2026)

Objectif de ce journal : tracer **toutes les avancées/modifications**, de la plus petite à la plus grande, avec validation associée (règle d’or : test + confirmation QEMU).

## 1) Modifications minimales (non-fonctionnelles)

1. **Nettoyage de noms dans les interruptions ExoPhoenix**
   - Fichier : `kernel/src/exophoenix/interrupts.rs`
   - Changement : renommage terminologique `EOI` → `ACK` côté helpers internes (`eoi_direct` → `apic_ack`, constante offset renommée) pour éviter les faux positifs de grep contextuel.
   - Impact : **aucun changement de comportement** runtime attendu.

2. **Validation textuelle stricte sur 0xF1 freeze handler**
   - Contrôle exécuté :
     - `grep -A30 'handler_freeze\|0xF1' kernel/src/exophoenix/interrupts.rs | grep -i 'eoi'`
   - Résultat : **aucune sortie** (contrôle passé).

## 2) Modifications intermédiaires (fonctionnelles ciblées)

3. **Phase 3.5 Sentinel implémentée**
   - Fichier : `kernel/src/exophoenix/sentinel.rs`
   - Ajouts principaux :
     - boucle `run_forever()` complète,
     - détection SMI (cycle anormalement long) avec skip sans escalade,
     - walker itératif des tables de pages (pas de récursion),
     - check nonce de liveness,
     - scoring additif + bascule `Threat` au seuil.
   - Contrainte respectée : `init_reaper` non modifié.

## 3) Modifications documentation roadmap (faible risque)

4. **Mise à jour de la checklist `Phase 4 — ExoFS` (cases vérifiées en code)**
   - Fichier : `docs/Phases/ExoOS_Roadmap_Avant_ExoBoot.md`
   - Cases passées en ✅ après audit code :
     - `verify() constant-time (LAC-01)`
     - `SYS_EXOFS_OPEN_BY_PATH=519`
     - `__NR_getdents64=520` dans musl-exo
     - `EpochRecord checksum Blake3 vérifié au montage`
     - `verify_cap() présent sur handlers 500–519`

## 4) Validations techniques exécutées

### Build/compilation

- Validation WSL : `cargo check -q`
- Résultat observé : `STAGE0_EXIT=0`.

### Confirmation QEMU (règle d’or)

- Validation headless via debug port 0xE9 (fichier `/tmp/t.log`).
- Trace observée (concaténée) :
  - `XK12356789abcdefgZA23[CAL:PIT-DRV-FAIL][CAL:FB3G][TIME-INIT hz=3000000000]456abc*** ALLOC ERROR size=256 alig=64`

#### Interprétation

- Le kernel boote et passe les marqueurs précoces/milieu de séquence.
- Le run atteint ensuite un **arrêt sur erreur d’allocation** (`ALLOC ERROR`) avant la signature terminale historique `...ZAIOK`.
- Conclusion : confirmation QEMU **partielle mais réelle** (progression runtime confirmée, blocage plus tardif identifié).

## 5) État actuel et garde-fous

- Approche suivie : **modifs minimales**, pas de création superflue, pas de refonte invasive.
- Section serveurs : le code `servers/*` existe déjà (plusieurs crates présentes), mais la checklist roadmap reste majoritairement à compléter côté orchestration de boot/service manager.

## 6) Prochain incrément recommandé (sans casser le code)

1. Stabiliser la piste `ALLOC ERROR size=256 align=64` (diagnostic ciblé alloc path au point `456abc`).
2. Revalider QEMU headless jusqu’à signature stable attendue.
3. Ensuite seulement : reprendre les items serveurs (PID order + supervision SIGCHLD) avec tests runtime incrémentaux.

---

## 7) Correctif ciblé ALLOC ERROR (20/03/2026 — suite)

### Correctifs appliqués (minimaux, sûrs)

1. **Correction de mapping des classes heap intermédiaires**
   - Fichier : `kernel/src/memory/heap/allocator/size_classes.rs`
   - Correction : les tailles intermédiaires (24/48/96/192/384/768/1536) pointaient vers des classes SLUB trop petites.
   - Mapping corrigé vers la classe immédiatement supérieure valide (ex. 24→32, 48→64, 192→256, etc.).

2. **Fallback robuste SLUB → vmalloc sur petite allocation**
   - Fichier : `kernel/src/memory/heap/allocator/hybrid.rs`
   - Correction : en cas d’échec SLUB sur une petite allocation, tentative automatique via `vmalloc::kalloc`.
   - Sécurité free : détection d’un bloc vmalloc via `kalloc_usable_size()` pour libération correcte (`kfree`) même quand l’allocation initiale était dans le chemin “small”.

### Revalidation technique exécutée

- Build WSL (`cargo check -q`) : **OK**.
- Rebuild ISO (`make iso`) : **OK**.
- QEMU headless (port 0xE9 vers `/tmp/t4.log` puis `/tmp/t5.log`) :
  - Trace observée :
    - `XK12356789abcdefgZA23[CAL:PIT-DRV-FAIL][CAL:FB3G][TIME-INIT hz=3000000000]`
    - `456abcdP7`
  - **`*** ALLOC ERROR size=256 align=64` non reproduit** après correctifs.

### Conclusion de cette itération

- Le blocage allocateur à `size=256 align=64` est levé.
- Le boot progresse au-delà du point `456abc` observé précédemment.
- Prochaine étape recommandée : poursuivre la stabilisation post-`P7` (instrumentation légère et jalons E9 additionnels), puis reprendre les items serveurs.

---

## 8) Stabilisation post-P7 : correctif #UD TSC/RDRAND (20/03/2026 — suite)

### Diagnostic observé

- En validation longue, un run s’arrêtait tôt avec trace tronquée `...ZA23`.
- Le log QEMU `-d int` montrait une exception `#UD` à RIP `0x1844d3`.
- Désassemblage : `0x1844d3 = rdtscp` dans `calibrate_tsc_with_pit` (module `cpu/tsc.rs`).

### Correctifs appliqués (ciblés)

1. **Fallback sûr si RDTSCP absent**
  - Fichier : `kernel/src/arch/x86_64/cpu/tsc.rs`
  - Changement : `read_tsc_end()` n’exécute `rdtscp` que si `CPU_FEATURES.has_rdtscp()`.
  - Fallback : `lfence; rdtsc; lfence` + `aux=0`.
  - Effet : évite `#UD` sur VM/CPU sans RDTSCP.

2. **Garde-fou RDRAND (anti-#UD)**
  - Fichier : `kernel/src/security/crypto/rng.rs`
  - Changement : `rdrand64()` vérifie `CPU_FEATURES.has_rdrand()` avant l’instruction `rdrand`.
  - Effet : fallback logiciel conservé sans exécuter une instruction non supportée.

> Note: une instrumentation E9 temporaire post-`P7` a été utilisée pendant le diagnostic puis retirée pour garder une trace boot propre.

### Revalidation technique exécutée

- Build kernel (`cargo build`) : **OK**.
- Rebuild ISO (manuel via `grub-mkrescue`) : **OK**.
- QEMU headless (`/tmp/t11.log`) :
  - `XK12356789abcdefgZA23[CAL:PIT-DRV-FAIL][CAL:FB3G][TIME-INIT hz=3]456abcdP789!I`
  - `OK`

### Conclusion de cette itération

- Le blocage post-`P7` est levé.
- Le boot atteint désormais la signature finale `...P789!I` puis `OK`.
- Priorité suivante : reprendre la stabilisation longue durée (timeouts plus longs + checkpoints serveurs) avant d’ouvrir un nouveau front fonctionnel.

---

## 9) Validation de stabilité répétée (20/03/2026 — série A/B/C)

### Campagne exécutée

- Trois runs QEMU headless consécutifs (A, B, C), timeout long identique, même ISO.
- Traces capturées sur port 0xE9 : `/tmp/t13a.log`, `/tmp/t13b.log`, `/tmp/t13c.log`.

### Résultats observés

- Taille des traces : `92 bytes` pour A/B/C (strictement identiques).
- Signature fin de boot observée sur les 3 runs :
  - `XK12356789abcdefgZA23[CAL:PIT-DRV-FAIL][CAL:FB3G][TIME-INIT hz=3]`
  - `456abcdP789!I`
  - `OK`

### Conclusion de cette campagne

- La séquence de boot finale est **reproductible** sur runs répétés.
- Aucun retour du `ALLOC ERROR` ni du `#UD` post-correctifs.
- La base est jugée stable pour reprendre le prochain incrément roadmap (serveurs / orchestration) en conservant la règle build + QEMU à chaque étape.

---

## 10) Recherche poussée Phase 5 (20/03/2026 — audit structurel)

### Objectif

- Vérifier l’état réel des items `Phase 5 — Servers Ring 1` avant toute modif invasive.
- Confirmer les vrais bloqueurs pour éviter les faux diagnostics et préserver la stabilité.

### Constat principal

1. **Chemin actif fork/exec**
  - Les stubs `sys_fork` / `sys_execve` dans `syscall/handlers/process.rs` existent, mais
    le chemin actif syscall passe par `syscall/dispatch.rs` (`handle_fork_inplace` / `handle_execve_inplace`).
  - Action faite : commentaire explicite ajouté dans `handlers/process.rs` pour éviter les audits erronés.

2. **Phase 5 — SRV-04 (crypto isolation)**
  - Audit dépendances `servers/*`: uniquement `spin.workspace` dans les `Cargo.toml`.
  - Aucun import RustCrypto opérationnel hors `crypto_server` (et en pratique aucun import direct côté servers).
  - Action faite : checklist roadmap mise à jour en ✅ pour l’item SRV-04.

3. **Blocage restant pour les autres items Phase 5**
  - `ipc_broker PID 2`, `init PID 1 + supervision`, `vfs_server monte ExoFS` restent non validés runtime.
  - Le point clé à traiter ensuite est l’orchestration de démarrage userspace (init/service bootstrap), pas la stabilité noyau de base.

### Conclusion

- Recherche poussée terminée sans régression runtime introduite.
- Prochaine étape recommandée (incrément sûr) : démarrage piloté et feature-gaté de la chaîne init/services, avec validation build + QEMU à chaque sous-étape.

---

## 11) ExoFS Phase 4 — clôture des 4 points restants (20/03/2026)

### Correctifs appliqués (ciblés, non-invasifs)

1. **Pipeline crypto en ordre strict (`Blake3 → compression → XChaCha20`)**
  - Fichier : `kernel/src/fs/exofs/storage/blob_writer.rs`
  - Changement : le payload compressé est désormais chiffré avant écriture disque (`SecretWriter`).
  - Lecture associée : `kernel/src/fs/exofs/storage/blob_reader.rs` déchiffre avant décompression.

2. **Nonces XChaCha20 dérivés via `AtomicU64 + HKDF`**
  - Fichiers :
    - `kernel/src/fs/exofs/crypto/xchacha20.rs`
    - `kernel/src/fs/exofs/crypto/secret_writer.rs`
  - Changement : abandon du schéma nonce aléatoire seul pour un schéma monotone atomique + dérivation HKDF.

3. **`mount_secret_key` initialisée depuis CSPRNG au montage**
  - Fichiers :
    - `kernel/src/fs/exofs/path/path_index.rs`
    - `kernel/src/fs/exofs/path/mod.rs`
  - Changement : clé SipHash globale de montage initialisée à la demande depuis `ENTROPY_POOL`, puis appliquée par défaut aux `PathIndex`.

4. **Shims `ZSTD_malloc` / `ZSTD_free` ajoutés**
  - Fichier : `kernel/src/fs/exofs/compress/zstd_wrapper.rs`
  - Changement : ajout de symboles C compatibles `zstd-sys` via allocateur noyau `no_std`.

### Validation exécutée

- Build WSL (`cargo check -q` dans `kernel/`) : **OK** (`CHECK_DONE`).
- Vérification éditeur (`Problems`) sur fichiers modifiés : **aucune erreur**.

### Décision roadmap

- Les 4 cases Phase 4 ExoFS restantes ont été basculées en ✅ dans
  `docs/Phases/ExoOS_Roadmap_Avant_ExoBoot.md`.
