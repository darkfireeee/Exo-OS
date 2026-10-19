# 11 — PHASE 10 : Shell `exosh`, Protocole d'Affichage Natif & `fb_server`

## 1. État Réel dans le Dépôt
* **`fb_server` & `display_abi.rs`** : `servers/syscall_abi/src/display_abi.rs` définit déjà `PixelFormat`, `DamageRect` (16 octets), `SurfaceGrant` (32 octets) et les opcodes `DISPLAY_OP_GET_INFO (0x5001)` à `DISPLAY_OP_FLIP (0x5005)`[cite: 18]. Côté noyau, `SYS_FRAMEBUFFER_INFO = 521` (`kernel/src/syscall/table/drivers.rs`) réserve l'accès au framebuffer physique GOP exclusivement au premier appelant (`fb_server`) via `FB_INFO_AUTHORIZED_PID` et revendique la région MMIO via `claim_trusted_mmio_for_pid`[cite: 7].
* **`exosh`** : Le shell interactif démarre et dialogue avec `tty_server` et `fb_server`, mais **viole la règle absolue de `SPEC-EXO-DISPLAY-PROTOCOL-STRATA.md`** : dans `servers/exosh/src/builtins/fs.rs` (`print_long_entry`) et `servers/exosh/src/util.rs` (`write_mode`), `ls -l` affiche encore les permissions POSIX `drwxr-xr-x`, le nombre de liens et `uid/gid`[cite: 6, 9, 16].

---

## 2. Procédure d'Implémentation Sans Ambiguïté

### A. Mise en Conformité de `exosh` avec `SPEC-EXO-DISPLAY-PROTOCOL-STRATA.md` (`9.4`, `10.4`, `10.7`)
1. Dans `servers/exosh/src/builtins/fs.rs` et `servers/exosh/src/util.rs`, supprimer l'affichage `drwxr-xr-x` / `uid:gid` de `print_long_entry()` et `write_mode()`[cite: 6, 16].
2. Pour chaque entrée listée par `ls` / `exo ls`, appeler `exo_syscall_abi::exofs_path_resolve_raw(path_ptr, path_len, 0, &mut resolved, EXOFS_READ_RIGHTS)` (`SYS_EXOFS_PATH_RESOLVE = 500`) qui remplit `ExofsPathResolveResult` (`blob_id: [u8; 32]`, `object_id: [u8; 32]`, `object_kind: u8`, `size_bytes: u64`, `epoch_id: u64`, `flags: u32`)[cite: 7, 16, 18].
3. Formater la sortie de `exo ls -l` (et `ls -l` dans `exosh`) selon la syntaxe stricte de `SPEC-EXO-DISPLAY-PROTOCOL-STRATA.md` §3 & §8 :
   ```text
   <type>  <rights>  <flags:<10>  @<token:04x>  ep:<epoch:04>  <size:<10>  <hash:8>  <name>