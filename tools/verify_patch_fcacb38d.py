#!/usr/bin/env python3
"""
verify_patch_fcacb38d.py — Vérification du patch ExoOS commit fcacb38d
=======================================================================
Vérifie que toutes les corrections ont été appliquées correctement.

Usage:
    python3 tools/verify_patch_fcacb38d.py [--repo-root /path/to/Exo-OS]

Retourne 0 si tous les checks passent, 1 sinon.
"""

import re
import sys
import os
import argparse
from pathlib import Path

# ─── Couleurs console ───────────────────────────────────────────────────────
RED   = "\033[1;31m"
GREEN = "\033[1;32m"
YELL  = "\033[1;33m"
BLUE  = "\033[1;34m"
RESET = "\033[0m"

PASS  = f"{GREEN}[PASS]{RESET}"
FAIL  = f"{RED}[FAIL]{RESET}"
WARN  = f"{YELL}[WARN]{RESET}"
INFO  = f"{BLUE}[INFO]{RESET}"


def read(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return ""


def check(label: str, condition: bool, detail: str = "") -> bool:
    if condition:
        print(f"  {PASS} {label}")
    else:
        msg = f"  {FAIL} {label}"
        if detail:
            msg += f"\n         → {detail}"
        print(msg)
    return condition


results = []

def run_check(label: str, condition: bool, detail: str = "") -> None:
    results.append(check(label, condition, detail))


# ─── Vérifications ──────────────────────────────────────────────────────────

def check_router(root: Path) -> None:
    print(f"\n{BLUE}=== FIX-ROUTER-01 : syscall 302 → SYS_IPC_SEND ==={RESET}")
    src = read(root / "servers/ipc_router/src/router.rs")

    run_check(
        "302 hardcodé absent de router.rs",
        "302," not in src and ", 302," not in src,
        "Il reste une occurrence de '302,' dans router.rs"
    )
    run_check(
        "SYS_IPC_SEND présent dans router.rs",
        "SYS_IPC_SEND" in src,
        "SYS_IPC_SEND n'est pas utilisé dans router.rs"
    )
    run_check(
        "Import exo_syscall_abi présent",
        "exo_syscall_abi as syscall_abi" in src,
        "L'alias 'use exo_syscall_abi as syscall_abi' est absent"
    )
    run_check(
        "syscall_abi::SYS_IPC_SEND utilisé dans forward_message",
        "syscall_abi::SYS_IPC_SEND" in src,
        "La constante qualifiée syscall_abi::SYS_IPC_SEND est absente"
    )


def check_security_gate(root: Path) -> None:
    print(f"\n{BLUE}=== FIX-IPC-04 : MAX_INLINE_PAYLOAD 48 → 192 ==={RESET}")
    src = read(root / "servers/ipc_router/src/security_gate.rs")

    run_check(
        "MAX_INLINE_PAYLOAD != 48",
        "usize = 48" not in src,
        "MAX_INLINE_PAYLOAD est toujours à 48"
    )
    run_check(
        "MAX_INLINE_PAYLOAD référence IPC_INLINE_PAYLOAD_SIZE",
        "IPC_INLINE_PAYLOAD_SIZE" in src,
        "MAX_INLINE_PAYLOAD ne pointe pas vers IPC_INLINE_PAYLOAD_SIZE"
    )
    # Valeur numérique attendue : vérification cohérence via syscall_abi
    abi_src = read(root / "servers/syscall_abi/src/lib.rs")
    if abi_src:
        match = re.search(r"IPC_INLINE_PAYLOAD_SIZE:\s*usize\s*=\s*(\d+)", abi_src)
        if not match:
            match = re.search(r"pub const IPC_INLINE_PAYLOAD_SIZE: usize = (\d+)", abi_src)
        if match:
            val = int(match.group(1))
            run_check(
                f"IPC_INLINE_PAYLOAD_SIZE = {val} dans syscall_abi (≥ 192 requis)",
                val >= 192,
                f"IPC_INLINE_PAYLOAD_SIZE vaut {val}, minimum attendu 192"
            )
        else:
            print(f"  {WARN} Impossible de lire IPC_INLINE_PAYLOAD_SIZE depuis syscall_abi")


def check_exocordon(root: Path) -> None:
    print(f"\n{BLUE}=== FIX-EXOCORDON-01 : DAG étendu + nouveaux ServiceId ==={RESET}")
    src = read(root / "servers/ipc_router/src/exocordon.rs")

    # ServiceIds ajoutés
    for sid, name in [(11, "Input"), (12, "Tty"), (13, "Fb"), (14, "Exosh"), (15, "Ps2")]:
        run_check(
            f"ServiceId::{name} = {sid} présent",
            f"= {sid}," in src or f"= {sid}," in src or f"{name}" in src,
            f"ServiceId::{name} absent de l'enum"
        )

    # Mappings dans service_id_of()
    for pid, name in [(11, "Input"), (12, "Tty"), (13, "Fb"), (14, "Exosh"), (15, "Ps2")]:
        run_check(
            f"service_id_of({pid}) → ServiceId::{name}",
            f"{pid} => Some(ServiceId::{name})" in src,
            f"Mapping PID {pid} → {name} absent de service_id_of()"
        )

    # Arêtes critiques du pipeline d'affichage
    for src_sid, dst_sid in [("Input", "Tty"), ("Tty", "Fb"), ("Exosh", "Tty"),
                               ("ExoShield", "Tty"), ("IpcBroker", "ExoShield")]:
        run_check(
            f"Arête {src_sid} → {dst_sid} présente",
            f"ServiceId::{src_sid}, ServiceId::{dst_sid}" in src,
            f"Arête {src_sid}→{dst_sid} absente du DAG"
        )

    # L'ancien DAG avait 5 arêtes — le nouveau doit en avoir plus
    edge_count = src.count("AuthEdge::new(")
    run_check(
        f"DAG contient ≥ 20 arêtes (actuel: {edge_count})",
        edge_count >= 20,
        f"DAG contient seulement {edge_count} arêtes, minimum 20 attendu"
    )

    # Invariant : ExoShield = 10
    run_check(
        "Invariant ExoShield = 10 préservé",
        "ExoShield as u8 == 10" in src,
        "L'assert const ExoShield==10 a disparu"
    )


def check_shield_pid(root: Path) -> None:
    print(f"\n{BLUE}=== FIX-SHIELD-PID : EXO_SHIELD_PID 12 → 10 ==={RESET}")
    src = read(root / "servers/exo_shield/src/ipc_gate/policy.rs")

    run_check(
        "EXO_SHIELD_PID != 12",
        "EXO_SHIELD_PID: u32 = 12" not in src,
        "EXO_SHIELD_PID est encore à 12 (= TTY_SERVER_ENDPOINT)"
    )
    run_check(
        "EXO_SHIELD_PID = 10",
        "EXO_SHIELD_PID: u32 = 10" in src,
        "EXO_SHIELD_PID n'est pas à 10"
    )

    # Vérification cohérence avec main.rs exo_shield
    shield_main = read(root / "servers/exo_shield/src/main.rs")
    if shield_main:
        match = re.search(r"EXO_SHIELD_ENDPOINT[^=]*=\s*(\d+)", shield_main)
        if match:
            endpoint_val = int(match.group(1))
            run_check(
                f"EXO_SHIELD_PID (10) cohérent avec EXO_SHIELD_ENDPOINT ({endpoint_val}) dans main.rs",
                endpoint_val == 10,
                f"EXO_SHIELD_ENDPOINT vaut {endpoint_val} dans main.rs, attendu 10"
            )


def check_lib_rs(root: Path) -> None:
    print(f"\n{BLUE}=== FIX-LIB-CFG + FIX-STAGE0 ==={RESET}")
    src = read(root / "kernel/src/lib.rs")

    # #[cfg] guard
    run_check(
        "#[cfg(target_arch)] présent avant pub use arch::x86_64",
        re.search(r'#\[cfg\(target_arch\s*=\s*"x86_64"\)\]\s*pub use arch::x86_64', src) is not None,
        "Le guard #[cfg(target_arch = \"x86_64\")] est absent avant pub use arch::x86_64"
    )

    # stage0 call
    run_check(
        "stage0_init_all_steps() appelé dans kernel_init()",
        "stage0_init_all_steps()" in src,
        "stage0_init_all_steps() n'est pas appelé dans lib.rs"
    )
    run_check(
        "Appel stage0 est entre Phase 5 et Phase 6",
        _stage0_between_sec_and_ipc(src),
        "stage0_init_all_steps() n'est pas positionné entre SECURITY et IPC"
    )


def _stage0_between_sec_and_ipc(src: str) -> bool:
    """Vérifie que stage0_init_all_steps() apparaît après stage_ok(SECURITY) et avant ipc_init."""
    pos_security = src.find('stage_ok("SECURITY")')
    pos_stage0   = src.find("stage0_init_all_steps()")
    pos_ipc      = src.find("ipc_init(")
    if pos_security < 0 or pos_stage0 < 0 or pos_ipc < 0:
        return False
    return pos_security < pos_stage0 < pos_ipc


def check_kairos(root: Path) -> None:
    print(f"\n{BLUE}=== FIX-KAIROS-01 : overflow saturating_mul(100) ==={RESET}")
    src = read(root / "kernel/src/security/exokairos.rs")

    run_check(
        "saturating_mul(100) absent de throttle_or_kill",
        "used.saturating_mul(100)" not in src,
        "used.saturating_mul(100) est encore présent"
    )
    run_check(
        "checked_mul(100) présent dans throttle_or_kill",
        "checked_mul(100)" in src,
        "checked_mul(100) absent — l'overflow n'est pas protégé"
    )
    run_check(
        "KillThresholdExceeded retourné sur overflow",
        "KillThresholdExceeded" in src and "None =>" in src,
        "Le bras None de checked_mul ne retourne pas KillThresholdExceeded"
    )


def check_ipc_constants(root: Path) -> None:
    print(f"\n{BLUE}=== FIX-IPC-PROCS : IPC_MAX_PROCESSES aligné sur MAX_PROCESSES ==={RESET}")
    src = read(root / "kernel/src/ipc/core/constants.rs")

    run_check(
        "IPC_MAX_PROCESSES n'est plus 65_536 hardcodé",
        "IPC_MAX_PROCESSES: usize = 65_536" not in src,
        "IPC_MAX_PROCESSES est encore hardcodé à 65 536"
    )
    run_check(
        "IPC_MAX_PROCESSES re-export depuis arch::constants::MAX_PROCESSES",
        "MAX_PROCESSES as IPC_MAX_PROCESSES" in src,
        "IPC_MAX_PROCESSES ne re-exporte pas arch::constants::MAX_PROCESSES"
    )

    # Cohérence : MAX_PROCESSES dans arch/constants.rs
    arch_src = read(root / "kernel/src/arch/constants.rs")
    if arch_src:
        match = re.search(r"MAX_PROCESSES:\s*usize\s*=\s*([\d_]+)", arch_src)
        if not match:
            match = re.search(r"pub const MAX_PROCESSES: usize = ([\d_]+)", arch_src)
        if match:
            val_str = match.group(1).replace("_", "")
            val = int(val_str)
            run_check(
                f"MAX_PROCESSES ({val}) dans arch/constants.rs est raisonnable (≤ 65536)",
                val <= 65536,
                f"MAX_PROCESSES vaut {val}, dépasse la limite IPC d'origine"
            )


def check_ibpb(root: Path) -> None:
    print(f"\n{BLUE}=== FIX-IBPB : émission IBPB au context-switch ==={RESET}")
    src = read(root / "kernel/src/scheduler/core/switch.rs")

    run_check(
        "MSR_IA32_PRED_CMD importé dans switch.rs",
        "MSR_IA32_PRED_CMD" in src,
        "MSR_IA32_PRED_CMD absent des imports de switch.rs"
    )
    run_check(
        "PRED_CMD_IBPB importé dans switch.rs",
        "PRED_CMD_IBPB" in src,
        "PRED_CMD_IBPB absent des imports de switch.rs"
    )
    run_check(
        "Appel write_msr(MSR_IA32_PRED_CMD, PRED_CMD_IBPB) présent",
        "write_msr(MSR_IA32_PRED_CMD, PRED_CMD_IBPB)" in src,
        "L'émission IBPB (write_msr PRED_CMD) est absente"
    )
    run_check(
        "Guard has_ibpb() présent avant émission IBPB",
        "has_ibpb()" in src,
        "Le check has_ibpb() est absent — IBPB émis sans vérifier le support CPU"
    )
    run_check(
        "Guard pid != pid (cross-process) présent",
        "prev.pid != next.pid" in src,
        "La condition cross-processus prev.pid != next.pid est absente"
    )


def check_no_regressions(root: Path) -> None:
    """Vérifications de non-régression : invariants critiques qui ne doivent pas changer."""
    print(f"\n{BLUE}=== NON-RÉGRESSION : invariants critiques ==={RESET}")

    # service_table.rs — exo_shield dans les deps d'exosh
    st_src = read(root / "servers/init_server/src/service_table.rs")
    run_check(
        "DEPS_EXOSH contient exo_shield (ordre boot Strata)",
        '"exo_shield"' in st_src and "DEPS_EXOSH" in st_src,
        "exo_shield absent de DEPS_EXOSH — régression de l'ordre boot"
    )

    # map_page_unflushed — assert! dur
    mem_src = read(root / "kernel/src/memory/virtual/address_space/user.rs")
    run_check(
        "map_page_unflushed utilise assert! (pas debug_assert!)",
        "assert!(" in mem_src and "map_page_unflushed: adresse hors" in mem_src,
        "map_page_unflushed a perdu son assert! dur (PATCH-MEM-01)"
    )

    # SSR bitmask — const_assert CORE_MASK_WORDS
    ssr_src = read(root / "kernel/src/exophoenix/ssr.rs")
    run_check(
        "SSR const_assert CORE_MASK_WORDS * 64 == MAX_CORES_LAYOUT",
        "CORE_MASK_WORDS * 64 == MAX_CORES_LAYOUT" in ssr_src,
        "L'assertion SSR bitmask 256-core a disparu"
    )

    # ipc_router/lib.rs — exo_syscall_abi disponible
    lib_toml = read(root / "servers/ipc_router/Cargo.toml")
    run_check(
        "exo-syscall-abi dans les dépendances de ipc_router",
        "exo-syscall-abi" in lib_toml,
        "exo-syscall-abi absent de Cargo.toml — les imports syscall_abi ne compileront pas"
    )


def check_syscall_numbers(root: Path) -> None:
    """Détecte les numéros de syscall hardcodés dans les serveurs Ring1."""
    print(f"\n{BLUE}=== AUDIT : numéros de syscall hardcodés dans les serveurs ==={RESET}")

    # Numéros connus qui ne doivent pas apparaître en dur
    forbidden = {
        300: "SYS_EXO_IPC_SEND",
        301: "SYS_EXO_IPC_RECV",
        302: "SYS_EXO_IPC_RECV_NB",
        303: "SYS_IPC_REGISTER",
        304: "SYS_IPC_LOOKUP",
    }

    servers_dir = root / "servers"
    violations = []

    for rs_file in servers_dir.rglob("*.rs"):
        content = rs_file.read_text(encoding="utf-8", errors="ignore")
        for num, name in forbidden.items():
            # Cherche le numéro comme argument de syscall (précédé par ( ou ,)
            pattern = rf'syscall\w*\s*\(\s*{num}\b'
            if re.search(pattern, content):
                violations.append((str(rs_file.relative_to(root)), num, name))

    if violations:
        for filepath, num, name in violations:
            print(f"  {WARN} {filepath}: syscall hardcodé {num} (devrait être {name})")
        run_check(
            f"Aucun numéro de syscall IPC hardcodé dans les serveurs ({len(violations)} trouvés)",
            False,
            f"{len(violations)} violation(s) ci-dessus"
        )
    else:
        run_check(
            "Aucun numéro de syscall IPC hardcodé dans les serveurs",
            True
        )


def check_exocordon_lib_rs(root: Path) -> None:
    """Vérifie que lib.rs exporte bien exo_syscall_abi pour security_gate."""
    print(f"\n{BLUE}=== DÉPENDANCE : exo_syscall_abi accessible depuis security_gate ==={RESET}")
    # security_gate est dans la lib crate, qui partage les deps du crate
    lib_toml = read(root / "servers/ipc_router/Cargo.toml")
    run_check(
        "exo-syscall-abi dans [dependencies] de ipc_router",
        "exo-syscall-abi" in lib_toml,
        "exo-syscall-abi absent — security_gate.rs ne peut pas accéder à IPC_INLINE_PAYLOAD_SIZE"
    )
    sg_src = read(root / "servers/ipc_router/src/security_gate.rs")
    run_check(
        "security_gate.rs importe/utilise exo_syscall_abi",
        "exo_syscall_abi" in sg_src,
        "security_gate.rs n'utilise pas exo_syscall_abi"
    )


# ─── Main ───────────────────────────────────────────────────────────────────

def main() -> int:
    parser = argparse.ArgumentParser(description="Vérification du patch ExoOS fcacb38d")
    parser.add_argument("--repo-root", default=".", help="Racine du dépôt Exo-OS")
    args = parser.parse_args()

    root = Path(args.repo_root).resolve()
    print(f"{INFO} Dépôt : {root}")
    print(f"{INFO} Commit ciblé : fcacb38d (patch ExoOS v0.2.0 Strata)")

    check_router(root)
    check_security_gate(root)
    check_exocordon(root)
    check_shield_pid(root)
    check_lib_rs(root)
    check_kairos(root)
    check_ipc_constants(root)
    check_ibpb(root)
    check_no_regressions(root)
    check_syscall_numbers(root)
    check_exocordon_lib_rs(root)

    passed = sum(1 for r in results if r)
    failed = sum(1 for r in results if not r)
    total  = len(results)

    print(f"\n{'─'*60}")
    print(f"  Résultats : {GREEN}{passed}/{total} PASS{RESET}  |  {RED}{failed}/{total} FAIL{RESET}")
    print(f"{'─'*60}")

    if failed == 0:
        print(f"\n{GREEN}✓ Tous les checks passent — patch prêt à committer.{RESET}")
        return 0
    else:
        print(f"\n{RED}✗ {failed} check(s) échoué(s) — voir détails ci-dessus.{RESET}")
        return 1


if __name__ == "__main__":
    sys.exit(main())
