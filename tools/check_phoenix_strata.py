#!/usr/bin/env python3
"""
check_phoenix_strata.py — Static anti-drift verifier for ExoPhoenix 3-Strata O(1) SLA (<= 1s).

Enforces Specification §6.1:
  1. Closed Strate 1 whitelist: ONLY the 6 canonical critical servers
     (`ipc_broker`/`ipc_router`, `memory_server`, `init_server`, `vfs_server`,
     `crypto_server`, `device_server`) + permanent fallback display (`fb_server`)
     may declare `PhoenixRecovery::CriticalSync` or own an `isolation.rs` file.
  2. Default recovery policy in `libs/exo-phoenix-ssr/src/lib.rs` must be
     `PhoenixRecovery::TransientAsync` (Strate 2).
  3. Static timeout bounds:
     - `PREPARE_ISOLATION_GLOBAL_BUDGET_MS <= 250` in `servers/exo_shield/src/subscription.rs`
     - `WAVE_R0_BUDGET_MS + WAVE_R1_BUDGET_MS + WAVE_R2_BUDGET_MS <= 750`
       in `servers/exo_shield/src/restore_sequence.rs`
  4. Subscription & restore tables only list members of the closed Strate 1 whitelist.
"""

import argparse
import pathlib
import re
import sys

STRATE1_WHITELIST = frozenset({
    "ipc_broker",
    "ipc_router",
    "memory_server",
    "init_server",
    "vfs_server",
    "crypto_server",
    "device_server",
    "fb_server",
})


def extract_u64_const(src: str, name: str):
    m = re.search(rf"pub\s+const\s+{re.escape(name)}\s*:\s*u(?:32|64)\s*=\s*([0-9_]+)\s*;", src)
    if not m:
        return None
    return int(m.group(1).replace("_", ""))


def extract_str_array(src: str, const_name: str):
    m = re.search(
        rf"pub\s+const\s+{re.escape(const_name)}\s*:\s*&\[&str\]\s*=\s*&\[([^\]]*)\]",
        src,
        re.DOTALL,
    )
    if not m:
        return None
    body = "\n".join(line for line in m.group(1).splitlines() if not line.strip().startswith("//"))
    return re.findall(r'"([^"]+)"', body)


def run_checks(root: pathlib.Path) -> int:
    errors = []

    # 1. Verify default PhoenixSafe::RECOVERY = PhoenixRecovery::TransientAsync
    ssr_lib = root / "libs/exo-phoenix-ssr/src/lib.rs"
    if not ssr_lib.exists():
        errors.append("Missing libs/exo-phoenix-ssr/src/lib.rs")
    else:
        ssr_src = ssr_lib.read_text(encoding="utf-8", errors="replace")
        if "const RECOVERY: PhoenixRecovery = PhoenixRecovery::TransientAsync;" not in ssr_src:
            errors.append(
                "libs/exo-phoenix-ssr/src/lib.rs must default PhoenixSafe::RECOVERY to PhoenixRecovery::TransientAsync"
            )
        if "CriticalSync" not in ssr_src or "TransientAsync" not in ssr_src:
            errors.append("libs/exo-phoenix-ssr/src/lib.rs must define CriticalSync and TransientAsync")

    # 2. Scan servers/ and drivers/ for unauthorized isolation.rs or CriticalSync
    for top in ("servers", "drivers"):
        base = root / top
        if not base.exists():
            continue
        for crate_dir in sorted(p for p in base.iterdir() if p.is_dir()):
            crate_name = crate_dir.name
            for rs_file in crate_dir.rglob("*.rs"):
                rel = rs_file.relative_to(root).as_posix()
                # Check isolation.rs files outside closed whitelist
                if rs_file.name == "isolation.rs" and crate_name not in STRATE1_WHITELIST:
                    errors.append(
                        f"Unauthorized isolation.rs outside closed Strate 1 whitelist: {rel} (crate '{crate_name}' is Strate 2)"
                    )
                # Check PhoenixRecovery::CriticalSync usage outside whitelist (and exo_shield policy tables)
                txt = rs_file.read_text(encoding="utf-8", errors="replace")
                if "PhoenixRecovery::CriticalSync" in txt and crate_name not in STRATE1_WHITELIST and crate_name != "exo_shield":
                    errors.append(
                        f"Unauthorized PhoenixRecovery::CriticalSync outside Strate 1 whitelist: {rel}"
                    )

    # 3. Verify servers/exo_shield/src/subscription.rs whitelist + budget <= 250 ms
    sub_path = root / "servers/exo_shield/src/subscription.rs"
    if not sub_path.exists():
        errors.append("Missing servers/exo_shield/src/subscription.rs")
    else:
        sub_src = sub_path.read_text(encoding="utf-8", errors="replace")
        budget = extract_u64_const(sub_src, "PREPARE_ISOLATION_GLOBAL_BUDGET_MS")
        if budget is None:
            errors.append("Missing PREPARE_ISOLATION_GLOBAL_BUDGET_MS in subscription.rs")
        elif budget > 250:
            errors.append(f"PREPARE_ISOLATION_GLOBAL_BUDGET_MS={budget} exceeds 250 ms hard limit")
        sub_list = extract_str_array(sub_src, "CRITICAL_SERVERS")
        if not sub_list:
            errors.append("Missing CRITICAL_SERVERS in subscription.rs")
        else:
            extra = [s for s in sub_list if s not in STRATE1_WHITELIST]
            if extra:
                errors.append(f"Unauthorized Strate 1 members in subscription.rs CRITICAL_SERVERS: {extra}")

    # 4. Verify servers/exo_shield/src/restore_sequence.rs waves R0+R1+R2 <= 750 ms + whitelist
    res_path = root / "servers/exo_shield/src/restore_sequence.rs"
    if not res_path.exists():
        errors.append("Missing servers/exo_shield/src/restore_sequence.rs")
    else:
        res_src = res_path.read_text(encoding="utf-8", errors="replace")
        r0 = extract_u64_const(res_src, "WAVE_R0_BUDGET_MS")
        r1 = extract_u64_const(res_src, "WAVE_R1_BUDGET_MS")
        r2 = extract_u64_const(res_src, "WAVE_R2_BUDGET_MS")
        if r0 is None or r1 is None or r2 is None:
            errors.append("Missing WAVE_R0/R1/R2_BUDGET_MS constants in restore_sequence.rs")
        elif (r0 + r1 + r2) > 750:
            errors.append(f"Sum of restore waves R0+R1+R2 = {r0 + r1 + r2} ms exceeds 750 ms limit")

        for wave_const in ("WAVE_R0_SERVERS", "WAVE_R1_SERVERS", "WAVE_R2_SERVERS"):
            wave_list = extract_str_array(res_src, wave_const)
            if not wave_list:
                errors.append(f"Missing {wave_const} in restore_sequence.rs")
            else:
                extra = [s for s in wave_list if s not in STRATE1_WHITELIST]
                if extra:
                    errors.append(f"Unauthorized Strate 1 members in {wave_const}: {extra}")

    print("Exo-OS ExoPhoenix 3-Strata O(1) audit")
    if errors:
        for e in errors:
            print(f"  [FAIL] {e}")
        print(f"errors: {len(errors)}")
        return 1

    print("  [PASS] Default PhoenixSafe::RECOVERY = TransientAsync (Strate 2)")
    print("  [PASS] Closed Strate 1 whitelist verified across servers/ & drivers/")
    print("  [PASS] PREPARE_ISOLATION_GLOBAL_BUDGET_MS <= 250 ms & R0+R1+R2 <= 750 ms")
    print("errors: 0")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", default=".", help="Repository root path")
    args = ap.parse_args()
    return run_checks(pathlib.Path(args.repo).resolve())


if __name__ == "__main__":
    sys.exit(main())
