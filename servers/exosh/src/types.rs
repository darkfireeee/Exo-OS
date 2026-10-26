//! # servers/exosh
#![allow(dead_code, unused_imports)]
use core::cell::UnsafeCell;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use exo_syscall_abi as syscall;

use crate::util::*;
use crate::terminal::*;
use crate::builtins::*;
use crate::exec::*;

pub const STDIN: u64 = 0;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LinuxStat {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_nlink: u64,
    pub st_mode: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub __pad0: u32,
    pub st_rdev: u64,
    pub st_size: i64,
    pub st_blksize: i64,
    pub st_blocks: i64,
    pub st_atim: Timespec,
    pub st_mtim: Timespec,
    pub st_ctim: Timespec,
    pub __unused: [i64; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LinuxSysInfo {
    pub uptime: i64,
    pub loads: [u64; 3],
    pub totalram: u64,
    pub freeram: u64,
    pub sharedram: u64,
    pub bufferram: u64,
    pub totalswap: u64,
    pub freeswap: u64,
    pub procs: u16,
    pub pad: u16,
    pub _pad2: u32,
    pub totalhigh: u64,
    pub freehigh: u64,
    pub mem_unit: u32,
    pub _pad3: [u8; 8],
}

#[derive(Clone, Copy, Default)]
pub struct LsOptions {
    pub long: bool,
    pub all: bool,
    pub human: bool,
}

#[derive(Clone, Copy, Default)]
pub struct RmOptions {
    pub force: bool,
    pub recursive: bool,
}

#[derive(Clone, Copy)]
pub struct ShellState {
    pub cwd: [u8; PATH_MAX],
    pub cwd_len: usize,
    pub old_cwd: [u8; PATH_MAX],
    pub old_cwd_len: usize,
    pub last_status: i32,
    pub history: [[u8; LINE_MAX]; HISTORY_MAX],
    pub history_len: [usize; HISTORY_MAX],
    pub history_count: usize,
    pub history_next: usize,
}

impl ShellState {
    pub const fn new() -> Self {
        let mut cwd = [0u8; PATH_MAX];
        cwd[0] = b'/';
        Self {
            cwd,
            cwd_len: 1,
            old_cwd: [0u8; PATH_MAX],
            old_cwd_len: 0,
            last_status: 0,
            history: [[0u8; LINE_MAX]; HISTORY_MAX],
            history_len: [0usize; HISTORY_MAX],
            history_count: 0,
            history_next: 0,
        }
    }

    pub fn cwd(&self) -> &[u8] {
        &self.cwd[..self.cwd_len]
    }

    pub fn record_old_cwd(&mut self) {
        self.old_cwd[..self.cwd_len].copy_from_slice(&self.cwd[..self.cwd_len]);
        self.old_cwd[self.cwd_len] = 0;
        self.old_cwd_len = self.cwd_len;
    }

    pub fn set_cwd(&mut self, path: &[u8]) {
        let len = path.len().min(PATH_MAX - 1);
        self.cwd[..len].copy_from_slice(&path[..len]);
        self.cwd[len] = 0;
        self.cwd_len = len;
    }

    pub fn remember(&mut self, command: &[u8]) {
        let command = trim(command);
        if command.is_empty() {
            return;
        }
        if let Some(last) = self.history_at_offset(0) {
            if bytes_eq(last, command) {
                return;
            }
        }

        let len = command.len().min(LINE_MAX - 1);
        let idx = self.history_next;
        self.history[idx][..len].copy_from_slice(&command[..len]);
        self.history_len[idx] = len;
        self.history_next = (self.history_next + 1) % HISTORY_MAX;
        if self.history_count < HISTORY_MAX {
            self.history_count += 1;
        }
    }

    pub fn history_at_offset(&self, offset: usize) -> Option<&[u8]> {
        if offset >= self.history_count {
            return None;
        }
        let idx = (self.history_next + HISTORY_MAX - 1 - offset) % HISTORY_MAX;
        Some(&self.history[idx][..self.history_len[idx]])
    }
}

#[derive(Clone, Copy)]
pub enum ShellKey {
    Enter,
    Interrupt,
    EndOfFile,
    ClearScreen,
    Backspace,
    Delete,
    Tab,
    Char(u8),
    HistoryPrev,
    HistoryNext,
    CursorLeft,
    CursorRight,
    CursorHome,
    CursorEnd,
}

