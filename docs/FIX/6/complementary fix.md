Je vais faire ça méthodiquement — recherches web d'abord sur les points ambigus, puis correction fichier par fichier.Confirmation : le syscall mmap retourne sur échec une valeur négative correspondant au code d'erreur kernel — donc `base < 0` est **correct** pour détecter les erreurs (les codes d'erreur sont dans la plage `-1` à `-4095`). BUG-M07 est annulé. Je procède aux corrections réelles.### État des correctifs appliqués ✅

Voici le bilan complet de ce qui a été produit :

---

#### `memory_server` — 7/8 bugs corrigés
| Fichier | Bugs corrigés |
|---|---|
| `ipc_bridge.rs` | **BUG-M01** : `SERVER_ENDPOINT_ID` 5→**3** |
| `mmap_service.rs` | **BUG-M02** : vérification `EACCES` dans `attach_shared_region` |
| `mmap_service.rs` | **BUG-M03** : `share_count` encodé dans `value1[48..63]`, plus de cast `u64→u32` |
| `mmap_service.rs` | **BUG-M04** : `saturating_add` → `wrapping_add` + skip de `0` |
| `mmap_service.rs` | **BUG-M06** : offset `QUOTA_SET` payload corrigé : `limit@4` au lieu de `@8` |
| `mmap_service.rs` | **BUG-M08** : double-lookup éliminé via `free_region_by_idx(idx)` |
| `allocator.rs` | **BUG-M05** : `snapshot()` en lecture seule via `&self`, plus d'upsert fantôme |
| *(BUG-M07)* | *Annulé — `base < 0` est la convention correcte Linux/ExoOS* |

---

#### `ipc_router` — 9/9 bugs corrigés
| Fichier | Bugs corrigés |
|---|---|
| `load_balancer.rs` | **BUG-I01** : `iter()` → `iter_mut()` dans `set_circuit_state` et `record_failure` |
| `router.rs` | **BUG-I02** : magic `302` → `syscall::SYS_IPC_SEND` (=300) dans tous les forwards |
| `exocordon.rs` | **BUG-I03** : DAG de 5 arêtes → **30 arêtes** (toutes les communications légitimes) |
| `router.rs` | **BUG-I04** : `ROUTE_COUNT.fetch_add` conditionné à `is_new` |
| `router.rs` | **BUG-I05** : `fetch_sub(1)` → `fetch_update(saturating_sub(1))` |
| `router.rs` | **BUG-I06** : récursion infinie `LoadBalanced` → délégation à `select_instance()` |
| `router.rs` | **BUG-I07** : verrou libéré avant les syscalls IPC en mode Broadcast |
| `main.rs` | **BUG-I08** : `Registry::register()` avec déduplication (update sur restart) |
| `main.rs` | **BUG-I09** : `name_len` validé, rejet explicite si vide ou > payload dispo |
| `exocordon.rs` | **BUG-I10** : `maybe_refill_quotas()` → `fetch_update()` atomique |

---

#### Kernel `exec.rs` — 1 bug (P1-05)
L'insertion de `tss::update_rsp0()` dans `do_execve()` a été préparée mais non écrite — il manquait le contexte d'accès au `cpu_id` et `kstack_ptr` depuis `exec.rs`. Cette correction nécessite d'importer `crate::arch::x86_64::tss` et d'appeler `tss::update_rsp0(cpu_id, thread.sched_tcb.kstack_ptr)` juste après la mise à jour de `thread.addresses`. À faire en dernier après validation des correctifs serveurs.