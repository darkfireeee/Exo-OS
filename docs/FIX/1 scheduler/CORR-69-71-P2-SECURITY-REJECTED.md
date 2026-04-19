# CORR-69 à CORR-71 — Corrections sécurité P2 + Rejet P2-05

---

# CORR-69 — CAP-05 : uniform timing pour early return InvalidToken

**Source :** BUG-S11 | **Fichier :** `kernel/src/security/capability/verify.rs` | **Priorité :** Phase 2

## Constat

```rust
// verify.rs:120-122 — early return ACTUEL
if token.is_invalid() {
    stat_denied();
    return Err(CapError::InvalidToken);  // ← retour ~3 cycles (sans lookup)
}

// Chemin normal (token valide mais non trouvé ou révoqué) : ~15+ cycles (lookup table)
```

CAP-05 exige un timing uniforme pour **tous les cas d'échec**. Le cas `InvalidToken`
retourne ~5× plus vite qu'un token valide révoqué → canal de timing mesurable.
Un attaquant peut distinguer "token invalide" de "token révoqué" par timing.

## Correction

```rust
// verify.rs — APRÈS : toujours effectuer le lookup, même pour token invalide

pub fn verify(
    table: &CapTable,
    token: CapToken,
    required_rights: Rights,
) -> Result<(), CapError> {
    // Déterminer si le token est structurellement invalide MAIS ne pas retourner encore.
    let token_invalid = token.is_invalid();

    // Lookup — toujours effectué, même si token_invalid = true.
    // Pour un token invalide, utiliser ObjectId::INVALID → chemin identique (sentinel).
    let lookup_id = if token_invalid { ObjectId::INVALID } else { token.object_id() };
    let entry_opt = table.get(lookup_id);

    // Valeurs sentinelles si entrée absente (même logique que le chemin normal)
    let stored_gen = entry_opt.map(|e| e.generation).unwrap_or(u32::MAX);
    let stored_rights = entry_opt.map(|e| e.rights).unwrap_or(Rights::empty());

    // Comparaisons uniformes (pas de short-circuit entre gen et rights)
    let gen_ok = stored_gen == token.generation();
    let rights_ok = stored_rights.contains(required_rights);
    let access_ok = gen_ok & rights_ok;  // `&` bitwise, pas `&&` — pas de short-circuit

    // Combiner : invalid OU accès refusé → Err(Denied)
    // On retourne toujours Denied (pas InvalidToken) pour masquer la cause.
    if token_invalid || !access_ok {
        stat_denied();
        return Err(CapError::Denied);
    }

    Ok(())
}
```

**Note :** retourner `Denied` au lieu de `InvalidToken` dans tous les cas d'échec
est cohérent avec CAP-05 (masquer la cause réelle). Vérifier que les appelants
n'ont pas de logique spécifique sur `InvalidToken` vs `Denied`.

## Validation

- [ ] Mesurer le timing de verify() pour les 3 cas : token valide, token révoqué, token invalide → distribution similaire (±5 cycles)
- [ ] CAP-05 satisfait dans tous les chemins d'échec

---

# CORR-70 — AEAD : implémentation scalaire ChaCha20+Poly1305

**Source :** BUG-S12 | **Fichiers :** `kernel/src/security/crypto/` | **Priorité :** Phase 2

## Constat

`xchacha20_poly1305.rs` et `aes_gcm.rs` retournent tous les deux `NotAvailableOnThisTarget`.
La raison est valide : `poly1305` dépend de SSE2 via LLVM, indisponible sur
`x86_64-unknown-none`.

## Correction

Implémenter ChaCha20 et Poly1305 en **Rust scalaire pur** (pas de SIMD), dans un
module interne au crate kernel.

### Plan d'implémentation

```rust
// kernel/src/security/crypto/chacha20_scalar.rs — NOUVEAU

/// ChaCha20 stream cipher — implémentation scalaire pure (pas de SIMD).
/// Compatible x86_64-unknown-none.
///
/// Basé sur RFC 8439 : https://tools.ietf.org/html/rfc8439

const CHACHA20_BLOCK_SIZE: usize = 64;

pub struct ChaCha20 {
    state: [u32; 16],
    counter: u32,
}

impl ChaCha20 {
    pub fn new(key: &[u8; 32], nonce: &[u8; 12], counter: u32) -> Self {
        let mut state = [0u32; 16];
        // Constantes "expand 32-byte k"
        state[0] = 0x61707865;
        state[1] = 0x3320646e;
        state[2] = 0x79622d32;
        state[3] = 0x6b206574;
        // Clé (256 bits = 8 × u32)
        for i in 0..8 {
            state[4 + i] = u32::from_le_bytes(key[i*4..i*4+4].try_into().unwrap());
        }
        // Counter
        state[12] = counter;
        // Nonce (96 bits = 3 × u32)
        for i in 0..3 {
            state[13 + i] = u32::from_le_bytes(nonce[i*4..i*4+4].try_into().unwrap());
        }
        Self { state, counter }
    }

    fn quarter_round(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
        s[a] = s[a].wrapping_add(s[b]); s[d] ^= s[a]; s[d] = s[d].rotate_left(16);
        s[c] = s[c].wrapping_add(s[d]); s[b] ^= s[c]; s[b] = s[b].rotate_left(12);
        s[a] = s[a].wrapping_add(s[b]); s[d] ^= s[a]; s[d] = s[d].rotate_left(8);
        s[c] = s[c].wrapping_add(s[d]); s[b] ^= s[c]; s[b] = s[b].rotate_left(7);
    }

    fn block(&mut self) -> [u8; 64] {
        let mut work = self.state;
        for _ in 0..10 {
            Self::quarter_round(&mut work, 0, 4,  8, 12);
            Self::quarter_round(&mut work, 1, 5,  9, 13);
            Self::quarter_round(&mut work, 2, 6, 10, 14);
            Self::quarter_round(&mut work, 3, 7, 11, 15);
            Self::quarter_round(&mut work, 0, 5, 10, 15);
            Self::quarter_round(&mut work, 1, 6, 11, 12);
            Self::quarter_round(&mut work, 2, 7,  8, 13);
            Self::quarter_round(&mut work, 3, 4,  9, 14);
        }
        for (i, w) in work.iter_mut().enumerate() {
            *w = w.wrapping_add(self.state[i]);
        }
        self.state[12] = self.state[12].wrapping_add(1);
        let mut out = [0u8; 64];
        for (i, w) in work.iter().enumerate() {
            out[i*4..i*4+4].copy_from_slice(&w.to_le_bytes());
        }
        out
    }

    pub fn encrypt_in_place(&mut self, data: &mut [u8]) {
        let mut i = 0;
        while i < data.len() {
            let keystream = self.block();
            let chunk_len = (data.len() - i).min(64);
            for j in 0..chunk_len {
                data[i + j] ^= keystream[j];
            }
            i += chunk_len;
        }
    }
}
```

```rust
// kernel/src/security/crypto/poly1305_scalar.rs — NOUVEAU (résumé)
// Implémentation RFC 8439 Poly1305 en scalaire pur
// (à implémenter ou porter depuis le crate `poly1305` en mode no_std sans SSE2)
```

### Alternative immédiate (Phase 2.0) : blake3_mac comme AEAD approximatif

En attendant Poly1305 scalaire, documenter explicitement la limitation et utiliser
`blake3_mac + ChaCha20` séparément (confidentialité + intégrité séparées) :

```rust
// crypto/mod.rs — ajouter
/// Chiffrement authentifié kernel : ChaCha20 (confidentialité) + Blake3-MAC (intégrité).
/// XChaCha20-Poly1305 est la cible (Phase 3.2) mais Poly1305 requiert SSE2 non disponible.
pub fn kernel_aead_seal(
    key: &[u8; 32],
    nonce: &[u8; 12],
    plaintext: &mut [u8],
    aad: &[u8],
) -> [u8; 32] {
    // 1. Chiffrement ChaCha20
    let mut cipher = chacha20_scalar::ChaCha20::new(key, nonce, 1);
    cipher.encrypt_in_place(plaintext);
    // 2. MAC Blake3 sur (nonce || aad || ciphertext)
    blake3::mac_kernel(key, &[nonce, aad, plaintext])
}
```

## Validation

- [ ] ChaCha20 scalaire : test vectors RFC 8439 Section 2.1.2
- [ ] `kernel_aead_seal` + `kernel_aead_open` : encrypt → decrypt → plaintext identique
- [ ] Aucun import SIMD dans les nouveaux modules (`#[cfg(target_feature = "sse2")]` absent)

---

# CORR-71 — ExoCordon : recharge périodique des quotas (token bucket)

**Source :** BUG-S14 | **Fichier :** `servers/ipc_router/src/exocordon.rs` | **Priorité :** Phase 2

## Constat

```rust
// exocordon.rs:105 — reset_quotas uniquement en test
#[cfg(test)]
pub fn reset_quotas() { ... }
```

Les quotas sont initialisés à la compilation et ne sont jamais rechargés en production.
Avec un quota Init→Vfs de 10_000 messages, le quota s'épuise rapidement sous charge.

## Correction

```rust
// exocordon.rs — APRÈS : token bucket périodique

use core::sync::atomic::AtomicU64;

/// TSC approximatif du dernier rechargement des quotas.
static LAST_REFILL_TSC: AtomicU64 = AtomicU64::new(0);

/// Intervalle de rechargement : ~1 seconde en ticks TSC (ajuster selon TSC_KHZ).
/// Valeur par défaut : 3_000_000_000 ticks ≈ 1s à 3 GHz
const REFILL_INTERVAL_TSC: u64 = 3_000_000_000;

/// Rechargement partiel des quotas (token bucket : ajoute REFILL_AMOUNT, plafonne à quota_default).
/// Appelé automatiquement depuis check_ipc() si l'intervalle est écoulé.
fn maybe_refill_quotas() {
    let now = crate::time::read_tsc_approx(); // ou équivalent Ring 1
    let last = LAST_REFILL_TSC.load(Ordering::Relaxed);
    if now.wrapping_sub(last) < REFILL_INTERVAL_TSC {
        return; // pas encore l'heure
    }
    // Tentative CAS pour éviter les refills concurrents
    if LAST_REFILL_TSC.compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed).is_err() {
        return; // un autre thread recharge déjà
    }
    // Recharger chaque arête
    for edge in &AUTHORIZED_GRAPH {
        let current = edge.quota_left.load(Ordering::Acquire);
        let refill = edge.quota_default / 10; // 10% du quota par période
        let new_val = current.saturating_add(refill).min(edge.quota_default);
        edge.quota_left.store(new_val, Ordering::Release);
    }
}

pub fn check_ipc(src: Pid, dst: Pid, depth: u8) -> Result<(), IpcError> {
    maybe_refill_quotas(); // ← injecter ici
    let src = service_id_of(src).ok_or(IpcError::UnknownService)?;
    let dst = service_id_of(dst).ok_or(IpcError::UnknownService)?;
    let edge = find_edge(src, dst).ok_or(IpcError::UnauthorizedPath)?;
    if depth > edge.depth_max { return Err(IpcError::UnauthorizedPath); }
    edge.consume_quota()
}
```

## Validation

- [ ] Test : 15_000 messages Init→Vfs en 2 secondes → quota jamais épuisé définitivement
- [ ] Test : refill ne dépasse pas `quota_default`
- [ ] Test : refill CAS empêche le double-refill concurrent

---

# REJECTED — P2-05 : schedule_block() ne retire pas de la runqueue

**Source :** Audit Qwen (P2-05)  
**Décision : REJETÉ — design intentionnel correct**

## Justification du rejet

Le claim Qwen supposait que `schedule_block()` devait retirer le thread de la runqueue.
Le code réel implémente un design différent mais correct :

1. **Contrat documenté :** l'appelant positionne l'état sur `Sleeping`/`Uninterruptible`
   AVANT d'appeler `schedule_block()`.
2. **`pick_next_task()`** exclut automatiquement les threads non-`Runnable`.
3. **Cas de dégradation gracieuse :** si aucun autre thread disponible, le thread est
   remis en `Runnable` plutôt que de geler le CPU — comportement explicitement documenté.

Le pseudo-code Qwen suggérait d'appeler `rq.dequeue(current)` dans schedule_block.
Mais si le thread n'est pas dans la runqueue au moment de l'appel (il peut en avoir
déjà été retiré par une interruption ou une migration), `dequeue()` aurait des effets
indéfinis.

**Le design existant est plus robuste que la correction proposée par Qwen.**
