#!/usr/bin/env python3
"""
tools/split_coreutils.py
Découpe modulaire de userspace/apps/coreutils/src/lib.rs (2 212 lignes) en :
  src/
    - host.rs           : Module hôte (tests/bancs avec std)
    - bare/
        - types.rs      : Constantes ABI, structures LinuxStat/SysInfo/Utsname, Args, exit, panic
        - util.rs       : Helpers d'E/S, parsing, normalisation de chemins, formatage, dev_*
        - cmds/
            - fs.rs     : Commandes système de fichiers (ls, cat, echo, mkdir, touch, rm, rmdir, pwd, stat, cp, mv, tree, sync, dd, basename, dirname)
            - system.rs : Commandes processus/système (sleep, meminfo, syscall_stat, ipc_stat, ps, top, kill, uptime, whoami, uname, clear, true, false)
            - text.rs   : Commandes d'analyse textuelle (wc, grep)
            - mod.rs    : Façade réexportant les commandes
        - mod.rs        : Façade du module bare réexportant types, util et cmds
    - lib.rs            : Macro exo_command!, réexports et tests
"""

import os
import subprocess

BASE_DIR = os.path.join("userspace", "apps", "coreutils", "src")
BARE_DIR = os.path.join(BASE_DIR, "bare")
CMDS_DIR = os.path.join(BARE_DIR, "cmds")

def main():
    raw_text = subprocess.check_output(
        ['git', 'show', 'HEAD:userspace/apps/coreutils/src/lib.rs'],
        text=True,
        encoding='utf-8'
    )
    lines = raw_text.splitlines(keepends=True)

    os.makedirs(CMDS_DIR, exist_ok=True)

    def slice_lines(start, end):
        return "".join(lines[start - 1 : end])

    def make_pub(code):
        out = []
        for line in code.splitlines():
            stripped = line.strip()
            indent = line[: len(line) - len(line.lstrip())]
            if stripped.startswith("fn "):
                if stripped.startswith("fn default(") or stripped.startswith("fn panic("):
                    line = indent + stripped
                else:
                    line = indent + "pub fn " + stripped[3:]
            elif stripped.startswith("unsafe fn "):
                line = indent + "pub unsafe fn " + stripped[len("unsafe fn ") :]
            elif stripped.startswith("const ") and not stripped.startswith("pub const "):
                line = indent + "pub const " + stripped[len("const ") :]
            elif stripped.startswith("struct ") and not stripped.startswith("pub struct "):
                line = indent + "pub struct " + stripped[len("struct ") :]
            out.append(line)
        return "\n".join(out) + "\n"

    # 1. HOST.RS : L5..L93
    host_raw = slice_lines(5, 93)
    with open(os.path.join(BASE_DIR, "host.rs"), "w", encoding="utf-8") as f:
        f.write("//! Implémentations compatibles environnement hôte (avec std)\n\n" + host_raw)
    print("Wrote host.rs")

    # 2. BARE/TYPES.RS : L103..L289
    types_raw = slice_lines(103, 289)
    types_raw = types_raw.replace(
        'write_all(STDERR, b"coreutils: panic\\n");',
        '''unsafe {
            let msg = b"coreutils: panic\\n";
            let _ = syscall::syscall3(syscall::SYS_WRITE, STDERR, msg.as_ptr() as u64, msg.len() as u64);
        }'''
    )
    # Rendre les champs de LinuxTimespec, LinuxStat, LinuxSysInfo, LinuxUtsname pub
    def make_struct_fields_pub(code):
        out = []
        in_struct = False
        for line in code.splitlines():
            stripped = line.strip()
            indent = line[: len(line) - len(line.lstrip())]
            if stripped.startswith("struct ") or stripped.startswith("pub struct "):
                if "{" in stripped:
                    in_struct = True
            elif in_struct:
                if stripped.startswith("}"):
                    in_struct = False
                elif ":" in stripped and not stripped.startswith("//") and not stripped.startswith("pub "):
                    line = indent + "pub " + stripped
            out.append(line)
        return "\n".join(out) + "\n"

    types_code = """#![allow(unused_imports, dead_code)]
use core::panic::PanicInfo;
use exo_syscall_abi as syscall;

""" + make_struct_fields_pub(make_pub(types_raw))
    with open(os.path.join(BARE_DIR, "types.rs"), "w", encoding="utf-8") as f:
        f.write(types_code)
    print("Wrote bare/types.rs")

    # 3. BARE/UTIL.RS : L290..L946
    util_raw = slice_lines(290, 946)
    util_code = """#![allow(unused_imports, dead_code)]
use super::types::*;
use exo_syscall_abi as syscall;

""" + make_pub(util_raw)
    with open(os.path.join(BARE_DIR, "util.rs"), "w", encoding="utf-8") as f:
        f.write(util_code)
    print("Wrote bare/util.rs")

    # 4. BARE/CMDS/FS.RS :
    # L948..L1502 : cmd_ls, cmd_cat, cmd_echo, cmd_mkdir, cmd_touch, cmd_rm, cmd_rmdir, cmd_pwd, cmd_stat, cmd_cp, cmd_mv, cmd_tree, cmd_sync
    # L1664..L1838: cmd_dd
    # L1895..L1913: cmd_basename, cmd_dirname
    fs_raw = slice_lines(948, 1502) + "\n" + slice_lines(1664, 1838) + "\n" + slice_lines(1895, 1913)
    fs_code = """#![allow(unused_imports, dead_code)]
use crate::bare::types::*;
use crate::bare::util::*;
use exo_syscall_abi as syscall;

""" + fs_raw
    with open(os.path.join(CMDS_DIR, "fs.rs"), "w", encoding="utf-8") as f:
        f.write(fs_code)
    print("Wrote bare/cmds/fs.rs")

    # 5. BARE/CMDS/SYSTEM.RS :
    # L1503..L1663: cmd_sleep, cmd_meminfo, cmd_syscall_stat, cmd_ipc_stat, cmd_ps, cmd_top, cmd_kill
    # L1839..L1894: cmd_uptime, cmd_whoami, cmd_uname, cmd_clear, cmd_true, cmd_false
    system_raw = slice_lines(1503, 1663) + "\n" + slice_lines(1839, 1894)
    system_code = """#![allow(unused_imports, dead_code)]
use crate::bare::types::*;
use crate::bare::util::*;
use exo_syscall_abi as syscall;

""" + system_raw
    with open(os.path.join(CMDS_DIR, "system.rs"), "w", encoding="utf-8") as f:
        f.write(system_code)
    print("Wrote bare/cmds/system.rs")

    # 6. BARE/CMDS/TEXT.RS :
    # L1915..L2156: count_wc_fd, cmd_wc, cmd_grep
    text_raw = slice_lines(1915, 2156)
    # L'accolade fermante du module bare était à la fin de text_raw (L2156)
    # Nettoyons cette accolade fermante orpheline
    lines_text = text_raw.splitlines()
    if lines_text and lines_text[-1].strip() == "}":
        lines_text.pop()
    text_clean = "\n".join(lines_text) + "\n"
    text_code = """#![allow(unused_imports, dead_code)]
use crate::bare::types::*;
use crate::bare::util::*;
use exo_syscall_abi as syscall;

""" + text_clean
    with open(os.path.join(CMDS_DIR, "text.rs"), "w", encoding="utf-8") as f:
        f.write(text_code)
    print("Wrote bare/cmds/text.rs")

    # 7. BARE/CMDS/MOD.RS
    cmds_mod = """pub mod fs;
pub mod system;
pub mod text;

pub use fs::*;
pub use system::*;
pub use text::*;
"""
    with open(os.path.join(CMDS_DIR, "mod.rs"), "w", encoding="utf-8") as f:
        f.write(cmds_mod)
    print("Wrote bare/cmds/mod.rs")

    # 8. BARE/MOD.RS
    bare_mod = """pub mod types;
pub mod util;
pub mod cmds;

pub use types::*;
pub use util::*;
pub use cmds::*;
"""
    with open(os.path.join(BARE_DIR, "mod.rs"), "w", encoding="utf-8") as f:
        f.write(bare_mod)
    print("Wrote bare/mod.rs")

    # 9. LIB.RS
    tests_raw = slice_lines(2180, len(lines))
    macro_raw = slice_lines(2158, 2178)

    lib_code = f"""#![cfg_attr(target_os = "none", no_std)]

#[cfg(not(target_os = "none"))]
pub mod host;

#[cfg(not(target_os = "none"))]
pub use host::{{cat, echo, ls, mkdir, rm, rmdir, touch}};

#[cfg(target_os = "none")]
pub mod bare;

{macro_raw}

{tests_raw}
"""
    with open(os.path.join(BASE_DIR, "lib.rs"), "w", encoding="utf-8") as f:
        f.write(lib_code)
    print("Wrote lib.rs")

if __name__ == "__main__":
    main()
