//! # servers/exosh
#![allow(dead_code, unused_imports)]
use core::cell::UnsafeCell;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use exo_syscall_abi as syscall;

use crate::types::*;
use crate::util::*;
use crate::builtins::*;
use crate::exec::*;

pub fn attach_stdio_to_tty() {
    let path = b"/dev/pts/0\0";
    let fd = unsafe {
        syscall::syscall4(
            syscall::SYS_OPENAT,
            AT_FDCWD,
            path.as_ptr() as u64,
            syscall::O_RDWR,
            0,
        )
    };
    if fd < 0 {
        return;
    }
    let raw_fd = fd as u64;
    let _ = unsafe { syscall::syscall2(syscall::SYS_DUP2, raw_fd, STDIN) };
    let _ = unsafe { syscall::syscall2(syscall::SYS_DUP2, raw_fd, STDOUT) };
    let _ = unsafe { syscall::syscall2(syscall::SYS_DUP2, raw_fd, STDERR) };
    if raw_fd > STDERR {
        let _ = unsafe { syscall::syscall1(syscall::SYS_CLOSE, raw_fd) };
    }
}

pub fn register_readiness_endpoint() {
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
    if pid <= 0 {
        return;
    }

    let endpoint = ((pid as u64) << 32) | 1;
    let name = b"exosh";
    let _ = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            endpoint,
        )
    };
}

pub fn read_line(line: &mut [u8; LINE_MAX], state: &mut ShellState) -> usize {
    prompt(state);
    loop {
        let rc = unsafe {
            syscall::syscall3(
                syscall::SYS_READ,
                STDIN,
                line.as_mut_ptr() as u64,
                (LINE_MAX - 1) as u64,
            )
        };
        if rc > 0 {
            let n = (rc as usize).min(LINE_MAX - 1);
            return if n > 0 && line[n - 1] == b'\n' {
                n - 1
            } else {
                n
            };
        } else if rc < 0 && rc != syscall::EAGAIN {
            log_stdin_read_error_once(rc);
        }
        sleep_ms(10);
    }
}

pub fn log_stdin_read_error_once(rc: i64) {
    if STDIN_READ_ERROR_LOGGED.swap(true, Ordering::AcqRel) {
        return;
    }
    let mut buf = [0u8; 48];
    let mut len = 0usize;
    push_bytes(&mut buf, &mut len, b"exosh: stdin read rc=");
    push_i64(&mut buf, &mut len, rc);
    push_bytes(&mut buf, &mut len, b"\n");
    unsafe {
        let _ = syscall::syscall3(syscall::SYS_EXO_LOG, buf.as_ptr() as u64, len as u64, 1);
    }
}

pub fn set_tty_echo(enabled: bool) {
    let mode: u64 = if enabled { 3 } else { 0 }; // bit 0 = echo, bit 1 = canonical
    unsafe {
        let _ = syscall::syscall3(syscall::SYS_IOCTL, STDIN, 0x5402, mode);
    }
}

pub fn set_tty_foreground(pid: u32) {
    unsafe {
        let _ = syscall::syscall3(syscall::SYS_IOCTL, STDIN, 0x5410, pid as u64);
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Sigaction {
    pub sa_handler: u64,
    pub sa_flags: u64,
    pub sa_restorer: u64,
    pub sa_mask: u64,
}

pub fn ignore_sigint() {
    let _ = exo_rt::sigaction_0syscall(
        2,
        Some(exo_rt::SigActionSlot {
            handler_vaddr: 1, // SIG_IGN
            flags: 0x1000_0000,
            _pad: 0,
            mask: 0,
            restorer: 0,
        }),
        None,
    );
    let act = Sigaction {
        sa_handler: 1, // SIG_IGN
        sa_flags: 0x1000_0000, // SA_RESTART
        sa_restorer: 0,
        sa_mask: 0,
    };
    unsafe {
        let _ = syscall::syscall4(
            syscall::SYS_RT_SIGACTION,
            2, // SIGINT
            &act as *const Sigaction as u64,
            0,
            8,
        );
    }
}

pub fn restore_sigint() {
    let _ = exo_rt::sigaction_0syscall(
        2,
        Some(exo_rt::SigActionSlot {
            handler_vaddr: 0, // SIG_DFL
            flags: 0,
            _pad: 0,
            mask: 0,
            restorer: 0,
        }),
        None,
    );
    let act = Sigaction {
        sa_handler: 0, // SIG_DFL
        sa_flags: 0,
        sa_restorer: 0,
        sa_mask: 0,
    };
    unsafe {
        let _ = syscall::syscall4(
            syscall::SYS_RT_SIGACTION,
            2, // SIGINT
            &act as *const Sigaction as u64,
            0,
            8,
        );
    }
}

pub fn read_line_interactive(line: &mut [u8; LINE_MAX], state: &mut ShellState) -> usize {
    let mut len = 0usize;
    let mut cursor = 0usize;
    let mut visible_len = 0usize;
    let mut history_offset: Option<usize> = None;

    set_tty_echo(false);
    render_input_line(line, 0, 0, state, &mut visible_len, true);

    loop {
        let Some(key) = read_shell_key_poll() else {
            sleep_ms(5);
            continue;
        };
        match key {
            ShellKey::Enter => {
                write_all(b"\x08 \x08\n");
                set_tty_echo(true);
                return len;
            }
            ShellKey::Interrupt => {
                write_all(b"^C\n");
                len = 0;
                cursor = 0;
                history_offset = None;
                visible_len = 0;
                line[0] = 0;
                render_input_line(line, 0, 0, state, &mut visible_len, true);
            }
            ShellKey::EndOfFile => {
                if len == 0 {
                    write_all(b"\n");
                    return 0;
                }
            }
            ShellKey::HistoryPrev => {
                apply_history_prev(line, &mut len, &mut cursor, &mut history_offset, state);
                render_input_line(line, len, cursor, state, &mut visible_len, true);
            }
            ShellKey::HistoryNext => {
                apply_history_next(line, &mut len, &mut cursor, &mut history_offset, state);
                render_input_line(line, len, cursor, state, &mut visible_len, true);
            }
            ShellKey::CursorLeft => {
                if cursor > 0 {
                    cursor -= 1;
                    render_input_line(line, len, cursor, state, &mut visible_len, true);
                }
            }
            ShellKey::CursorRight => {
                if cursor < len {
                    cursor += 1;
                    render_input_line(line, len, cursor, state, &mut visible_len, true);
                }
            }
            ShellKey::CursorHome => {
                cursor = 0;
                render_input_line(line, len, cursor, state, &mut visible_len, true);
            }
            ShellKey::CursorEnd => {
                cursor = len;
                render_input_line(line, len, cursor, state, &mut visible_len, true);
            }
            ShellKey::Backspace => {
                if cursor > 0 {
                    delete_before_cursor(line, &mut len, &mut cursor);
                    render_input_line(line, len, cursor, state, &mut visible_len, true);
                }
            }
            ShellKey::Delete => {
                delete_at_cursor(line, &mut len, cursor);
                render_input_line(line, len, cursor, state, &mut visible_len, true);
            }
            ShellKey::ClearScreen => {
                write_all(b"\x0c");
                visible_len = 0;
                render_input_line(line, len, cursor, state, &mut visible_len, true);
            }
            ShellKey::Char(b) => {
                let append_at_end = cursor == len;
                if insert_input_byte(line, &mut len, &mut cursor, b) {
                    if append_at_end {
                        visible_len = len + 1;
                        render_input_append(b);
                    } else {
                        render_input_line(line, len, cursor, state, &mut visible_len, true);
                    }
                }
            }
            _ => {}
        }
    }
}

pub fn apply_history_prev(
    line: &mut [u8; LINE_MAX],
    len: &mut usize,
    cursor: &mut usize,
    history_offset: &mut Option<usize>,
    state: &ShellState,
) {
    if state.history_count == 0 {
        return;
    }
    let next = match *history_offset {
        Some(offset) if offset + 1 < state.history_count => offset + 1,
        Some(offset) => offset,
        None => 0,
    };
    *history_offset = Some(next);
    if let Some(history) = state.history_at_offset(next) {
        replace_input_line(line, len, cursor, history);
    }
}

pub fn apply_history_next(
    line: &mut [u8; LINE_MAX],
    len: &mut usize,
    cursor: &mut usize,
    history_offset: &mut Option<usize>,
    state: &ShellState,
) {
    match *history_offset {
        Some(offset) if offset > 0 => {
            let next = offset - 1;
            *history_offset = Some(next);
            if let Some(history) = state.history_at_offset(next) {
                replace_input_line(line, len, cursor, history);
            }
        }
        Some(_) => {
            *history_offset = None;
            replace_input_line(line, len, cursor, b"");
        }
        None => {}
    }
}

pub fn replace_input_line(
    line: &mut [u8; LINE_MAX],
    len: &mut usize,
    cursor: &mut usize,
    new_line: &[u8],
) {
    let new_len = new_line.len().min(LINE_MAX - 1);
    line[..new_len].copy_from_slice(&new_line[..new_len]);
    *len = new_len;
    *cursor = new_len;
}

pub fn insert_input_byte(
    line: &mut [u8; LINE_MAX],
    len: &mut usize,
    cursor: &mut usize,
    byte: u8,
) -> bool {
    if *len + 1 >= LINE_MAX {
        return false;
    }
    let mut i = *len;
    while i > *cursor {
        line[i] = line[i - 1];
        i -= 1;
    }
    line[*cursor] = byte;
    *len += 1;
    *cursor += 1;
    true
}

pub fn delete_before_cursor(line: &mut [u8; LINE_MAX], len: &mut usize, cursor: &mut usize) {
    if *cursor == 0 || *len == 0 {
        return;
    }
    let start = *cursor - 1;
    let mut i = start;
    while i + 1 < *len {
        line[i] = line[i + 1];
        i += 1;
    }
    *len -= 1;
    *cursor -= 1;
}

pub fn delete_at_cursor(line: &mut [u8; LINE_MAX], len: &mut usize, cursor: usize) {
    if cursor >= *len {
        return;
    }
    let mut i = cursor;
    while i + 1 < *len {
        line[i] = line[i + 1];
        i += 1;
    }
    *len -= 1;
}

pub fn render_input_line(
    line: &[u8; LINE_MAX],
    len: usize,
    cursor: usize,
    state: &ShellState,
    visible_len: &mut usize,
    show_cursor: bool,
) {
    let mut out = [0u8; PATH_MAX + LINE_MAX + 96];
    let mut out_len = 0usize;

    push_bytes(&mut out, &mut out_len, b"\r");
    push_bytes(&mut out, &mut out_len, b"exosh:");
    push_bytes(&mut out, &mut out_len, state.cwd());
    push_bytes(&mut out, &mut out_len, b"$ ");

    let cursor = cursor.min(len);
    push_bytes(&mut out, &mut out_len, &line[..cursor]);
    if show_cursor {
        push_bytes(&mut out, &mut out_len, ANSI_REVERSE);
        if cursor < len {
            push_bytes(&mut out, &mut out_len, &line[cursor..cursor + 1]);
        } else {
            push_bytes(&mut out, &mut out_len, b" ");
        }
        push_bytes(&mut out, &mut out_len, ANSI_RESET);
        if cursor < len {
            push_bytes(&mut out, &mut out_len, &line[cursor + 1..len]);
        }
    } else {
        push_bytes(&mut out, &mut out_len, &line[cursor..len]);
    }

    let next_visible_len = if show_cursor && cursor == len {
        len.saturating_add(1)
    } else {
        len
    };
    if *visible_len > next_visible_len {
        let diff = *visible_len - next_visible_len;
        let mut i = 0usize;
        while i < diff {
            push_bytes(&mut out, &mut out_len, b" ");
            i += 1;
        }
    }
    *visible_len = next_visible_len;

    if out_len > 0 {
        write_bytes(&out[..out_len]);
    }
}

pub fn render_input_append(byte: u8) {
    let mut out = [0u8; 16];
    let mut len = 0usize;
    push_bytes(&mut out, &mut len, b"\x08");
    push_bytes(&mut out, &mut len, &[byte]);
    push_bytes(&mut out, &mut len, ANSI_REVERSE);
    push_bytes(&mut out, &mut len, b" ");
    push_bytes(&mut out, &mut len, ANSI_RESET);
    write_bytes(&out[..len]);
}

pub fn read_byte_poll() -> Option<u8> {
    let mut byte = [0u8; 1];
    let rc = unsafe {
        syscall::syscall3(
            syscall::SYS_READ,
            STDIN,
            byte.as_mut_ptr() as u64,
            byte.len() as u64,
        )
    };
    if rc == 1 {
        Some(byte[0])
    } else {
        None
    }
}

pub fn input_reply_endpoint() -> Option<u64> {
    let cached = INPUT_REPLY_ENDPOINT.load(Ordering::Acquire);
    if cached != 0 {
        let _ = ensure_input_stream_attached(cached);
        return Some(cached);
    }

    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
    if pid <= 0 {
        return None;
    }
    let endpoint = ((pid as u64) << 32) | EXOSH_INPUT_REPLY_CHANNEL;
    let name = b"exosh_input_reply";
    let rc = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            endpoint,
        )
    };
    if rc < 0 && rc != syscall::EEXIST {
        return None;
    }
    INPUT_REPLY_ENDPOINT.store(endpoint, Ordering::Release);
    let _ = ensure_input_stream_attached(endpoint);
    Some(endpoint)
}

pub fn ensure_input_stream_attached(reply_endpoint: u64) -> bool {
    let cached = INPUT_ATTACHED_ENDPOINT.load(Ordering::Acquire);
    if cached == reply_endpoint {
        return true;
    }
    let req = syscall::InputRequest {
        sender_pid: 0,
        msg_type: syscall::INPUT_MSG_ATTACH,
        reply_endpoint,
        event: syscall::InputEventWire::default(),
    };
    let rc = unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            syscall::INPUT_SERVER_ENDPOINT,
            &req as *const syscall::InputRequest as u64,
            core::mem::size_of::<syscall::InputRequest>() as u64,
            0,
            0,
            0,
        )
    };
    if rc < 0 {
        return false;
    }
    INPUT_ATTACHED_ENDPOINT.store(reply_endpoint, Ordering::Release);
    true
}

pub fn recv_input_reply(reply_endpoint: u64, timeout_ms: u64) -> Option<syscall::InputReply> {
    let mut reply = syscall::InputReply::default();
    let recv_rc = unsafe {
        syscall::syscall4(
            syscall::SYS_IPC_RECV,
            reply_endpoint,
            &mut reply as *mut syscall::InputReply as u64,
            core::mem::size_of::<syscall::InputReply>() as u64,
            syscall::IPC_FLAG_TIMEOUT | timeout_ms,
        )
    };
    if recv_rc < 0 {
        return None;
    }
    Some(reply)
}

pub fn input_poll_event() -> Option<syscall::InputEventWire> {
    let reply_endpoint = input_reply_endpoint()?;
    if !ensure_input_stream_attached(reply_endpoint) {
        return None;
    }
    let mut reply = syscall::InputReply::default();
    let rc = unsafe {
        syscall::syscall4(
            syscall::SYS_EXO_IPC_RECV_NB,
            reply_endpoint,
            &mut reply as *mut syscall::InputReply as u64,
            core::mem::size_of::<syscall::InputReply>() as u64,
            0,
        )
    };
    if rc >= core::mem::size_of::<syscall::InputReply>() as i64 && reply.status == 0 {
        Some(reply.event)
    } else {
        None
    }
}

pub fn check_interrupt_poll() -> bool {
    if let Some(event) = input_poll_event() {
        if event.device == syscall::INPUT_DEVICE_KEYBOARD
            && event.state == syscall::INPUT_KEY_PRESSED
            && (event.ascii == 0x03 || event.ascii == b'q')
        {
            return true;
        }
    }
    false
}

pub fn map_input_event_to_shell_key(event: syscall::InputEventWire) -> Option<ShellKey> {
    if event.device != syscall::INPUT_DEVICE_KEYBOARD || event.state != syscall::INPUT_KEY_PRESSED {
        return None;
    }
    match event.code {
        HID_KEY_UP => return Some(ShellKey::HistoryPrev),
        HID_KEY_DOWN => return Some(ShellKey::HistoryNext),
        HID_KEY_LEFT => return Some(ShellKey::CursorLeft),
        HID_KEY_RIGHT => return Some(ShellKey::CursorRight),
        HID_KEY_HOME => return Some(ShellKey::CursorHome),
        HID_KEY_END => return Some(ShellKey::CursorEnd),
        HID_KEY_DELETE => return Some(ShellKey::Delete),
        _ => {}
    }

    match event.ascii {
        b'\n' | b'\r' => Some(ShellKey::Enter),
        0x03 => Some(ShellKey::Interrupt),
        0x04 => Some(ShellKey::EndOfFile),
        0x08 | 0x7F => Some(ShellKey::Backspace),
        0x0C => Some(ShellKey::ClearScreen),
        b'\t' => Some(ShellKey::Tab),
        b if b.is_ascii_graphic() || b == b' ' => Some(ShellKey::Char(b)),
        _ => None,
    }
}

pub fn map_byte_to_shell_key(byte: u8) -> Option<ShellKey> {
    match byte {
        b'\n' | b'\r' => Some(ShellKey::Enter),
        0x03 => Some(ShellKey::Interrupt),
        0x04 => Some(ShellKey::EndOfFile),
        0x08 | 0x7F => Some(ShellKey::Backspace),
        0x0C => Some(ShellKey::ClearScreen),
        b'\t' => Some(ShellKey::Tab),
        b if b.is_ascii_graphic() || b == b' ' => Some(ShellKey::Char(b)),
        _ => None,
    }
}

pub fn read_shell_key_poll() -> Option<ShellKey> {
    let byte = read_byte_poll()?;
    if byte == 0x1B {
        if let Some(key) = read_ansi_escape_key() {
            return Some(key);
        }
    }
    map_byte_to_shell_key(byte)
}

pub fn read_ansi_escape_key() -> Option<ShellKey> {
    let second = read_byte_wait_ms(10)?;
    if second != b'[' {
        return None;
    }
    let third = read_byte_wait_ms(10)?;
    match third {
        b'A' => Some(ShellKey::HistoryPrev),
        b'B' => Some(ShellKey::HistoryNext),
        b'C' => Some(ShellKey::CursorRight),
        b'D' => Some(ShellKey::CursorLeft),
        b'H' | b'1' => Some(ShellKey::CursorHome),
        b'F' | b'4' => Some(ShellKey::CursorEnd),
        b'3' => {
            if read_byte_wait_ms(25) == Some(b'~') {
                Some(ShellKey::Delete)
            } else {
                None
            }
        }
        _ => None,
    }
}

pub fn tty_reply_endpoint() -> Option<u64> {
    let cached = TTY_REPLY_ENDPOINT.load(Ordering::Acquire);
    if cached != 0 {
        return Some(cached);
    }

    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
    if pid <= 0 {
        return None;
    }
    let endpoint = ((pid as u64) << 32) | EXOSH_TTY_REPLY_CHANNEL;
    let name = b"exosh_tty_reply";
    let rc = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            endpoint,
        )
    };
    if rc < 0 && rc != syscall::EEXIST {
        return None;
    }
    TTY_REPLY_ENDPOINT.store(endpoint, Ordering::Release);
    Some(endpoint)
}

pub fn tty_read_line(line: &mut [u8; LINE_MAX], state: &ShellState) -> Option<usize> {
    let reply_endpoint = tty_reply_endpoint()?;
    let mut prompted = false;

    loop {
        let mut req = syscall::TtyRequest::zeroed();
        req.msg_type = syscall::TTY_MSG_READ_LINE;
        req.reply_endpoint = reply_endpoint;

        let send_rc = unsafe {
            syscall::syscall6(
                syscall::SYS_IPC_SEND,
                syscall::TTY_SERVER_ENDPOINT,
                &req as *const syscall::TtyRequest as u64,
                core::mem::size_of::<syscall::TtyRequest>() as u64,
                0,
                0,
                0,
            )
        };
        if send_rc < 0 {
            return None;
        }
        if !prompted {
            prompt(state);
            prompted = true;
        }

        let mut reply = syscall::TtyReply::zeroed();
        let recv_rc = unsafe {
            syscall::syscall4(
                syscall::SYS_IPC_RECV,
                reply_endpoint,
                &mut reply as *mut syscall::TtyReply as u64,
                core::mem::size_of::<syscall::TtyReply>() as u64,
                syscall::IPC_FLAG_TIMEOUT | 50,
            )
        };
        if recv_rc == syscall::ETIMEDOUT {
            continue;
        }
        if recv_rc < 0 {
            return None;
        }
        if reply.signal != 0 {
            return Some(0);
        }
        if reply.status == syscall::EAGAIN || reply.len == 0 {
            sleep_ms(10);
            continue;
        }
        if reply.status != 0 {
            return None;
        }
        let n = (reply.len as usize).min(LINE_MAX - 1).min(reply.data.len());
        line[..n].copy_from_slice(&reply.data[..n]);
        return Some(if n > 0 && line[n - 1] == b'\n' {
            n - 1
        } else {
            n
        });
    }
}

pub fn read_byte_wait_ms(ms: u64) -> Option<u8> {
    let mut waited = 0u64;
    while waited < ms {
        if let Some(byte) = read_byte_poll() {
            return Some(byte);
        }
        sleep_ms(1);
        waited += 1;
    }
    None
}

