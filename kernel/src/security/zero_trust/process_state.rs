// kernel/src/security/zero_trust/process_state.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ZERO-TRUST PROCESS STATE — état de sécurité persistant par-process (TIER 1.1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Le moteur `policy.rs` est RÉEL (MLS Bell-LaPadula/Biba, restrictions, règles par
// ressource) mais restait INERTE pour les syscalls : le dispatch reconstruisait un
// `SecurityContext::new_normal` neuf (restrictions=0, trust figé) à chaque appel,
// ignorant l'état réel du process → comportement « TRUST_ALL » de fait.
//
// Ce module détient cet état réel et construit le contexte consulté à chaque
// syscall (`dispatch.rs`) :
//   • restrictions sandbox/pledge persistées par PID (durcissables uniquement) ;
//   • niveau de confiance dérivé de l'état système (init/Ring 1/normal).
//
// STOCKAGE : tableau lock-free indexé par PID (`[AtomicU64; MAX_TRACKED_PIDS]`).
//   - Défaut (slot=0) = AUCUNE restriction → identique au comportement antérieur
//     (BOOT-SAFE : rien n'est restreint tant qu'un process n'a pas explicitement
//     opt-in via `restrict_process`).
//   - Monotone : `restrict_process` ne fait qu'AJOUTER des bits de restriction
//     (RÈGLE ZT-02 / PLEDGE-01 : les droits ne peuvent que diminuer).
//   - PID hors plage (>= MAX) : non traçable → `restrict_process` échoue
//     (fail-closed au site d'opt-in) ; la lecture renvoie 0 (cohérent — un tel PID
//     n'a jamais pu être restreint).
//   - Recyclage : le slot est remis à 0 à la libération du PID (cf. lifecycle/wait).
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};

use super::context::{restriction_flags, PrincipalId, SecurityContext, TrustLevel};
use super::verify::ring1_trusted_mask;
use crate::syscall::numbers;

/// Nombre de PID traçables sans verrou. Couvre largement les PID système précoces
/// et les premiers process utilisateur d'ExoOS. Un PID >= cette borne est traité
/// comme non-restreignable (cf. en-tête).
pub const MAX_TRACKED_PIDS: usize = 1024;

/// PID du process init — ne peut pas être restreint (RÈGLE PLEDGE-03).
const INIT_PID: u32 = 1;

/// État de restriction par PID. `0` = aucune restriction.
static PROCESS_RESTRICTIONS: [AtomicU64; MAX_TRACKED_PIDS] =
    [const { AtomicU64::new(0) }; MAX_TRACKED_PIDS];

/// Ajoute des restrictions au process `pid` (monotone — ne peut que DURCIR).
///
/// Refuse init (PID 1, RÈGLE PLEDGE-03), le PID 0 et les PID hors plage.
/// Retourne `true` si la restriction a été appliquée.
pub fn restrict_process(pid: u32, flags: u64) -> bool {
    if pid == 0 || pid == INIT_PID {
        return false;
    }
    let idx = pid as usize;
    if idx >= MAX_TRACKED_PIDS {
        return false;
    }
    PROCESS_RESTRICTIONS[idx].fetch_or(flags, Ordering::Release);
    true
}

/// Restrictions actives du process `pid` (0 = aucune).
#[inline]
pub fn process_restrictions(pid: u32) -> u64 {
    let idx = pid as usize;
    if idx >= MAX_TRACKED_PIDS {
        return 0;
    }
    PROCESS_RESTRICTIONS[idx].load(Ordering::Acquire)
}

/// Réinitialise l'état d'un PID. Appelé à la libération du PID (recyclage) pour
/// qu'un futur process réutilisant ce numéro reparte SANS restriction héritée.
pub fn clear_process_restrictions(pid: u32) {
    let idx = pid as usize;
    if idx < MAX_TRACKED_PIDS {
        PROCESS_RESTRICTIONS[idx].store(0, Ordering::Release);
    }
}

/// Héritage fork : l'enfant reçoit AU MOINS les restrictions du parent
/// (RÈGLE ZT-03 / SAND-03 : un fils ne peut pas être moins restreint que son père).
pub fn inherit_restrictions(parent_pid: u32, child_pid: u32) {
    let parent = process_restrictions(parent_pid);
    if parent != 0 {
        let _ = restrict_process(child_pid, parent);
    }
}

/// Niveau de confiance **authentifié** d'un PID, dérivé de l'état réel du système.
///
/// - init (PID 1) → System (process le plus privilégié) ;
/// - service Ring 1 de confiance (mask maintenu par `ipc_policy`) → Trusted ;
/// - tout le reste (apps, shell) → Normal.
pub fn trust_for_pid(pid: u32) -> TrustLevel {
    if pid == INIT_PID {
        return TrustLevel::System;
    }
    if pid < 64 && (ring1_trusted_mask() & (1u64 << pid)) != 0 {
        return TrustLevel::Trusted;
    }
    TrustLevel::Normal
}

/// Construit le `SecurityContext` RÉEL du process appelant : niveau de confiance
/// dérivé de l'état système + restrictions persistées. Remplace le `new_normal`
/// inerte précédemment câblé dans le dispatch (TIER 1.1).
pub fn context_for_caller(pid: u32, tid: u32) -> SecurityContext {
    let principal = PrincipalId {
        uid: 0,
        gid: 0,
        pid,
        tid,
        ns_id: 0,
    };
    SecurityContext::for_process(principal, trust_for_pid(pid), process_restrictions(pid))
}

/// Masque des restrictions qui INTERDISENT le syscall `nr`. Si l'appelant porte
/// une de ces restrictions, le syscall est refusé par `policy::check_restrictions`.
///
/// Mapping (numéros Linux-compat, cf. [`crate::syscall::numbers`]) :
///   - création de process (fork/clone/vfork) → `NO_FORK | NO_PROCESS_CREATE`
///   - exec (execve)                          → `NO_EXEC | NO_PROCESS_CREATE`
///   - réseau (socket/connect/bind/listen/    → `NO_NETWORK`
///     accept/sendto/recvfrom/…)
#[inline]
pub fn syscall_restriction_mask(nr: u64) -> u64 {
    use restriction_flags::*;
    match nr {
        numbers::SYS_FORK | numbers::SYS_CLONE | numbers::SYS_VFORK => NO_FORK | NO_PROCESS_CREATE,
        numbers::SYS_EXECVE => NO_EXEC | NO_PROCESS_CREATE,
        numbers::SYS_SOCKET
        | numbers::SYS_CONNECT
        | numbers::SYS_BIND
        | numbers::SYS_LISTEN
        | numbers::SYS_ACCEPT
        | numbers::SYS_SENDTO
        | numbers::SYS_RECVFROM
        | numbers::SYS_SENDMSG
        | numbers::SYS_RECVMSG
        | numbers::SYS_SOCKETPAIR => NO_NETWORK,
        _ => 0,
    }
}

/// Traduit un bitmask de **promesses pledge** (OpenBSD-like, `pledge_flags`) en
/// **restrictions zero-trust** : toute capacité NON promise devient une restriction
/// enforced au bord syscall (cf. `syscall_restriction_mask`). TIER 2.10 — pont
/// entre l'API `pledge()` userspace et l'enforcement TIER 1.1.
///
/// Mapping (promesse absente → restriction) :
///   - `PROC` (création de process) absent → `NO_FORK | NO_PROCESS_CREATE`
///   - `EXEC` absent                       → `NO_EXEC`
///   - `INET` (réseau) absent              → `NO_NETWORK`
///
/// Seules les capacités réellement enforced sont mappées (fork/exec/réseau) ;
/// les autres pledges (rpath/wpath/…) seront ajoutées quand l'enforcement FS/IO
/// par-syscall existera. Conservatif : ne restreint jamais plus que les flags connus.
pub fn pledge_promises_to_restrictions(promises: u64) -> u64 {
    use crate::security::isolation::pledge::pledge_flags;
    use restriction_flags::*;
    let mut r = 0u64;
    if promises & pledge_flags::PROC == 0 {
        r |= NO_FORK | NO_PROCESS_CREATE;
    }
    if promises & pledge_flags::EXEC == 0 {
        r |= NO_EXEC;
    }
    if promises & pledge_flags::INET == 0 {
        r |= NO_NETWORK;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    // PID de test choisis hauts pour éviter toute collision avec d'éventuels PID
    // système (et restant < MAX_TRACKED_PIDS).
    const P_A: u32 = 700;
    const P_B: u32 = 701;
    const P_CHILD: u32 = 702;

    fn reset(pids: &[u32]) {
        for &p in pids {
            clear_process_restrictions(p);
        }
    }

    /// Défaut = aucune restriction (boot-safe). `restrict_process` durcit ; init
    /// et les PID hors plage sont refusés.
    #[test]
    fn restrict_is_monotonic_and_guarded() {
        reset(&[P_A]);
        assert_eq!(process_restrictions(P_A), 0, "défaut = non restreint");

        assert!(restrict_process(P_A, restriction_flags::NO_FORK));
        assert!(process_restrictions(P_A) & restriction_flags::NO_FORK != 0);

        // Ajout monotone : la première restriction reste présente.
        assert!(restrict_process(P_A, restriction_flags::NO_NETWORK));
        let r = process_restrictions(P_A);
        assert!(r & restriction_flags::NO_FORK != 0 && r & restriction_flags::NO_NETWORK != 0);

        // init ne peut pas être restreint (RÈGLE PLEDGE-03).
        assert!(!restrict_process(INIT_PID, restriction_flags::NO_FORK));
        assert_eq!(process_restrictions(INIT_PID), 0);

        // PID hors plage → non traçable.
        assert!(!restrict_process(MAX_TRACKED_PIDS as u32, restriction_flags::NO_FORK));
        assert_eq!(process_restrictions(MAX_TRACKED_PIDS as u32 + 5), 0);

        reset(&[P_A]);
    }

    /// Le recyclage (clear) repart d'un état vierge.
    #[test]
    fn clear_resets_slot() {
        reset(&[P_B]);
        assert!(restrict_process(P_B, restriction_flags::SANDBOX_FULL));
        assert!(process_restrictions(P_B) != 0);
        clear_process_restrictions(P_B);
        assert_eq!(process_restrictions(P_B), 0);
    }

    /// L'enfant fork hérite (au moins) des restrictions du parent.
    #[test]
    fn child_inherits_parent_restrictions() {
        reset(&[P_A, P_CHILD]);
        assert!(restrict_process(P_A, restriction_flags::NO_EXEC));
        inherit_restrictions(P_A, P_CHILD);
        assert!(
            process_restrictions(P_CHILD) & restriction_flags::NO_EXEC != 0,
            "le fils doit hériter NO_EXEC"
        );
        reset(&[P_A, P_CHILD]);
    }

    /// Le mapping syscall→restriction couvre create-process / exec / réseau.
    #[test]
    fn syscall_mask_maps_dangerous_syscalls() {
        use restriction_flags::*;
        assert!(syscall_restriction_mask(numbers::SYS_FORK) & NO_FORK != 0);
        assert!(syscall_restriction_mask(numbers::SYS_CLONE) & NO_PROCESS_CREATE != 0);
        assert!(syscall_restriction_mask(numbers::SYS_EXECVE) & NO_EXEC != 0);
        assert!(syscall_restriction_mask(numbers::SYS_SOCKET) & NO_NETWORK != 0);
        // Un syscall bénin (read=0) n'est jamais filtré par ce mécanisme.
        assert_eq!(syscall_restriction_mask(numbers::SYS_READ), 0);
    }

    /// `trust_for_pid` : init = System, inconnu = Normal.
    #[test]
    fn trust_derivation_init_and_default() {
        assert_eq!(trust_for_pid(INIT_PID), TrustLevel::System);
        assert_eq!(trust_for_pid(P_A), TrustLevel::Normal);
    }

    /// TIER 2.10 — `pledge()` : une promesse absente devient une restriction
    /// enforced ; toutes les promesses → rien à restreindre.
    #[test]
    fn pledge_maps_absent_promises_to_restrictions() {
        use crate::security::isolation::pledge::pledge_flags;
        use restriction_flags::*;

        // STDIO seul (pas PROC/EXEC/INET) → fork+exec+réseau restreints.
        let r = pledge_promises_to_restrictions(pledge_flags::STDIO);
        assert!(r & NO_FORK != 0 && r & NO_PROCESS_CREATE != 0);
        assert!(r & NO_EXEC != 0);
        assert!(r & NO_NETWORK != 0);

        // PROC|EXEC|INET promis → aucune restriction (sur les flags enforced).
        let none = pledge_promises_to_restrictions(
            pledge_flags::PROC | pledge_flags::EXEC | pledge_flags::INET,
        );
        assert_eq!(none, 0);

        // INET seul → réseau autorisé, fork/exec restreints.
        let net_only = pledge_promises_to_restrictions(pledge_flags::INET);
        assert_eq!(net_only & NO_NETWORK, 0);
        assert!(net_only & NO_FORK != 0 && net_only & NO_EXEC != 0);
    }
}
