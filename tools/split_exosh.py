#!/usr/bin/env python3
"""
tools/split_exosh.py
Découpe modulaire de servers/exosh/src/main.rs (4 201 lignes) en :
  - types.rs     : constantes, structs, ShellState, ShellKey, DdIoBuffer
  - util.rs      : parsing texte, chemins, syscall wrappers, formatage et sorties
  - terminal.rs  : édition de ligne, historique, endpoints input/tty, lecture touches
  - builtins/    : fs.rs, proc.rs, net.rs, bench.rs, mod.rs
  - exec.rs      : parsing ligne de commande, run_commands, pipelines et spawn externe
  - main.rs      : imports racine, point d'entrée _start() et panic handler
"""

import os
import re
from collections import defaultdict

SRC_DIR = os.path.join("servers", "exosh", "src")
MAIN_PATH = os.path.join(SRC_DIR, "main.rs")

import subprocess

def main():
    out = subprocess.check_output(
        ['git', 'show', 'HEAD:servers/exosh/src/main.rs'],
        text=True,
        encoding='utf-8'
    )
    lines = out.splitlines()

    items = []
    in_item = False
    brace_depth = 0
    comment_start = None

    for i, line in enumerate(lines):
        s = line.strip()
        if not in_item:
            if s.startswith('//') or s.startswith('/*') or s.startswith('#['):
                if comment_start is None:
                    comment_start = i
                continue
            if any(s.startswith(k + ' ') or s.startswith('pub ' + k + ' ') or s.startswith('pub(crate) ' + k + ' ') or s.startswith('unsafe ' + k + ' ') or s.startswith('pub unsafe ' + k + ' ') or s.startswith('pub extern ') for k in ['fn', 'struct', 'enum', 'impl', 'static', 'const', 'type', 'mod']):
                in_item = True
                item_start = comment_start if comment_start is not None else i
                comment_start = None
                brace_depth = 0
            else:
                comment_start = None
                continue

        if in_item:
            brace_depth += line.count('{') - line.count('}')
            if brace_depth == 0 and (line.endswith(';') or line.strip() == '}' or line.endswith('}')):
                items.append((item_start, i, lines[item_start:i+1]))
                in_item = False
                comment_start = None

    print(f"Total items detected: {len(items)}")

    def get_sym(chunk):
        for l in chunk:
            s = l.strip()
            for k in ['fn', 'struct', 'enum', 'impl', 'static', 'const', 'type', 'mod']:
                pattern = rf'^(?:pub\s+)?(?:unsafe\s+)?(?:extern\s+"[^"]+"\s+)?{k}\s+([A-Za-z0-9_]+)'
                m = re.match(pattern, s)
                if m: return m.group(1)
                if s.startswith('impl ') or s.startswith('impl<'):
                    m = re.match(r'^impl(?:\s*<[^>]+>)?\s+([A-Za-z0-9_]+)', s)
                    if m: return m.group(1)
        return 'UNKNOWN'

    # Catégorisation
    TYPES_SYMS = {
        'STDIN', 'LinuxStat', 'LinuxSysInfo', 'LsOptions', 'RmOptions', 'ShellState', 'ShellKey'
    }
    MAIN_SYMS = {'_start', 'panic'}
    EXEC_SYMS = {
        'run_pipeline', 'run_commands', 'run_command', 'run_external_or_report', 'run_external',
        'external_command_path', 'cstr_prefix', 'cmd_contains_slash', 'build_prefixed_command_path'
    }
    TERMINAL_SYMS = {
        'attach_stdio_to_tty', 'register_readiness_endpoint', 'read_line', 'log_stdin_read_error_once',
        'read_line_interactive', 'apply_history_prev', 'apply_history_next', 'replace_input_line',
        'insert_input_byte', 'delete_before_cursor', 'delete_at_cursor', 'render_input_line',
        'render_input_append', 'read_byte_poll', 'input_reply_endpoint', 'ensure_input_stream_attached',
        'recv_input_reply', 'input_poll_event', 'map_input_event_to_shell_key', 'read_shell_key_poll',
        'read_ansi_escape_key', 'tty_reply_endpoint', 'tty_read_line', 'read_byte_wait_ms'
    }
    FS_SYMS = {
        'cmd_cd', 'cmd_pwd', 'cmd_ls', 'cmd_mkdir', 'cmd_touch', 'touch_one_arg', 'cmd_cat', 'cmd_echo',
        'cmd_rm', 'cmd_cp', 'cmd_mv', 'cmd_rmdir', 'cmd_tree', 'tree_walk', 'cmd_stat', 'cmd_dd',
        'print_dd_summary', 'parse_ls_args', 'parse_ls_dirents', 'print_long_entry', 'write_colored_name',
        'parse_tree_dirents', 'write_tree_indent', 'parse_rm_options', 'rm_one_arg', 'is_star_pattern',
        'rm_star', 'star_parent_arg', 'rm_path', 'rm_directory_children', 'rm_dirent_batch', 'copy_file_path',
        'cmd_sync'
    }
    PROC_SYMS = {
        'cmd_help', 'cmd_top', 'process_list_snapshot', 'process_entry_name', 'write_process_state',
        'cmd_time', 'cmd_sleep', 'cmd_kill', 'cmd_history', 'cmd_meminfo', 'perf_read', 'cmd_syscall_stat',
        'cmd_ipc_stat', 'reboot_with_cmd', 'cmd_shutdown', 'cmd_reboot'
    }
    NET_SYMS = {
        'cmd_ping', 'icmp_echo_request', 'icmp_echo_reply_matches', 'internet_checksum', 'cmd_tcping'
    }
    BENCH_SYMS = {
        'cmd_bench', 'parse_bench_n', 'bench_ipc', 'bench_sched', 'CRYPTO_SERVER_ENDPOINT', '_',
        'bench_crypto', 'bench_fs'
    }

    # Le reste va dans util.rs
    targets = defaultdict(list)
    for start, end, chunk in items:
        sym = get_sym(chunk)
        if sym in MAIN_SYMS:
            targets['main'].append((sym, chunk))
        elif sym in TYPES_SYMS:
            targets['types'].append((sym, chunk))
        elif sym in EXEC_SYMS:
            targets['exec'].append((sym, chunk))
        elif sym in TERMINAL_SYMS:
            targets['terminal'].append((sym, chunk))
        elif sym in FS_SYMS:
            targets['builtins_fs'].append((sym, chunk))
        elif sym in PROC_SYMS:
            targets['builtins_proc'].append((sym, chunk))
        elif sym in NET_SYMS:
            targets['builtins_net'].append((sym, chunk))
        elif sym in BENCH_SYMS:
            targets['builtins_bench'].append((sym, chunk))
        else:
            targets['util'].append((sym, chunk))

    for t, sym_list in targets.items():
        print(f"Category {t:15}: {len(sym_list):3d} items")

    def make_pub_items(chunk):
        out = []
        in_struct = False
        for line in chunk:
            stripped = line.strip()
            indent = line[:len(line) - len(line.lstrip())]

            if re.match(r'^(?:pub\s+)?struct\s+[A-Za-z0-9_]+', stripped) and '{' in stripped:
                in_struct = True
                if stripped.startswith('struct '):
                    line = indent + 'pub ' + stripped
                out.append(line)
                continue
            elif in_struct:
                if stripped == '}':
                    in_struct = False
                    out.append(line)
                    continue
                if not stripped.startswith('//') and not stripped.startswith('#[') and not stripped.startswith('pub '):
                    if ':' in stripped:
                        line = indent + 'pub ' + stripped
                out.append(line)
                continue

            if stripped.startswith('fn '):
                line = indent + 'pub fn ' + stripped[3:]
            elif stripped.startswith('unsafe fn '):
                line = indent + 'pub unsafe fn ' + stripped[len('unsafe fn '):]
            elif stripped.startswith('struct ') and '(' in stripped:
                line = indent + re.sub(r'struct\s+([A-Za-z0-9_]+)\((.*)\);', r'pub struct \1(pub \2);', stripped)
            elif stripped.startswith('struct '):
                line = indent + 'pub ' + stripped
            elif stripped.startswith('enum '):
                line = indent + 'pub ' + stripped
            elif stripped.startswith('static ') and not stripped.startswith('static INPUT_'):
                line = indent + 'pub ' + stripped
            elif stripped.startswith('static INPUT_') or stripped.startswith('static TTY_') or stripped.startswith('static STDIN_'):
                line = indent + 'pub ' + stripped
            elif stripped.startswith('const ') and not stripped.startswith('const _'):
                line = indent + 'pub ' + stripped

            out.append(line)
        return '\n'.join(out)

    def get_header_for(mod_name):
        mods = ['types', 'util', 'terminal', 'builtins', 'exec']
        lines = [
            "//! # servers/exosh",
            "#![allow(dead_code, unused_imports)]",
            "use core::cell::UnsafeCell;",
            "use core::panic::PanicInfo;",
            "use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};",
            "use exo_syscall_abi as syscall;",
            ""
        ]
        if mod_name == 'builtins':
            lines.append("use super::*;")
            lines.append("use crate::types::*;")
            lines.append("use crate::util::*;")
            lines.append("use crate::terminal::*;")
            lines.append("use crate::exec::*;")
        else:
            for m in mods:
                if m != mod_name:
                    lines.append(f"use crate::{m}::*;")
        lines.append("")
        return "\n".join(lines)

    # Ecriture types.rs
    types_content = get_header_for('types') + "\n"
    for sym, chunk in targets['types']:
        types_content += make_pub_items(chunk) + "\n\n"
    with open(os.path.join(SRC_DIR, "types.rs"), "w", encoding="utf-8") as f:
        f.write(types_content)
    print("Wrote types.rs")

    # Ecriture util.rs
    util_content = get_header_for('util') + "\n"
    for sym, chunk in targets['util']:
        util_content += make_pub_items(chunk) + "\n\n"
    with open(os.path.join(SRC_DIR, "util.rs"), "w", encoding="utf-8") as f:
        f.write(util_content)
    print("Wrote util.rs")

    # Ecriture terminal.rs
    terminal_content = get_header_for('terminal') + "\n"
    for sym, chunk in targets['terminal']:
        terminal_content += make_pub_items(chunk) + "\n\n"
    with open(os.path.join(SRC_DIR, "terminal.rs"), "w", encoding="utf-8") as f:
        f.write(terminal_content)
    print("Wrote terminal.rs")

    # Ecriture builtins/
    builtins_dir = os.path.join(SRC_DIR, "builtins")
    os.makedirs(builtins_dir, exist_ok=True)

    # builtins/fs.rs
    bfs_content = get_header_for('builtins') + "\n"
    for sym, chunk in targets['builtins_fs']:
        bfs_content += make_pub_items(chunk) + "\n\n"
    with open(os.path.join(builtins_dir, "fs.rs"), "w", encoding="utf-8") as f:
        f.write(bfs_content)

    # builtins/proc.rs
    bproc_content = get_header_for('builtins') + "\n"
    for sym, chunk in targets['builtins_proc']:
        bproc_content += make_pub_items(chunk) + "\n\n"
    with open(os.path.join(builtins_dir, "proc.rs"), "w", encoding="utf-8") as f:
        f.write(bproc_content)

    # builtins/net.rs
    bnet_content = get_header_for('builtins') + "\n"
    for sym, chunk in targets['builtins_net']:
        bnet_content += make_pub_items(chunk) + "\n\n"
    with open(os.path.join(builtins_dir, "net.rs"), "w", encoding="utf-8") as f:
        f.write(bnet_content)

    # builtins/bench.rs
    bbench_content = get_header_for('builtins') + "\n"
    for sym, chunk in targets['builtins_bench']:
        bbench_content += make_pub_items(chunk) + "\n\n"
    with open(os.path.join(builtins_dir, "bench.rs"), "w", encoding="utf-8") as f:
        f.write(bbench_content)

    # builtins/mod.rs
    bmod_content = """pub mod fs;
pub mod proc;
pub mod net;
pub mod bench;

pub use fs::*;
pub use proc::*;
pub use net::*;
pub use bench::*;
"""
    with open(os.path.join(builtins_dir, "mod.rs"), "w", encoding="utf-8") as f:
        f.write(bmod_content)
    print("Wrote builtins/ modules")

    # Ecriture exec.rs
    exec_content = get_header_for('exec') + "\n"
    for sym, chunk in targets['exec']:
        exec_content += make_pub_items(chunk) + "\n\n"
    with open(os.path.join(SRC_DIR, "exec.rs"), "w", encoding="utf-8") as f:
        f.write(exec_content)
    print("Wrote exec.rs")

    # Ecriture main.rs
    main_content = """#![no_std]
#![no_main]
#![allow(dead_code, unused_imports)]

use core::panic::PanicInfo;

pub mod types;
pub mod util;
pub mod terminal;
pub mod builtins;
pub mod exec;

use types::*;
use util::*;
use terminal::*;
use builtins::*;
use exec::*;

"""
    for sym, chunk in targets['main']:
        main_content += '\n'.join(chunk) + "\n\n"
    with open(MAIN_PATH, "w", encoding="utf-8") as f:
        f.write(main_content)
    print("Wrote new modular main.rs")

if __name__ == "__main__":
    main()
