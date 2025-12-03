//! Exo-Shell - Shell interactif pour Exo-OS
//! Version 0.5.0 - Shell complet avec support VFS

#![no_std]
#![no_main]

mod builtin;
mod parser;
mod executor;

use core::panic::PanicInfo;

// Syscalls Linux x86_64
const SYS_READ: usize = 0;
const SYS_WRITE: usize = 1;
const SYS_OPEN: usize = 2;
const SYS_CLOSE: usize = 3;
const SYS_EXECVE: usize = 59;
const SYS_EXIT: usize = 60;
const SYS_WAIT4: usize = 61;
const SYS_FORK: usize = 57;
const SYS_GETPID: usize = 39;
const SYS_CHDIR: usize = 80;
const SYS_GETCWD: usize = 79;

// File descriptors standards
const STDIN: usize = 0;
const STDOUT: usize = 1;
const STDERR: usize = 2;

// Couleurs ANSI
const RESET: &str = "\x1b[0m";
const GREEN: &str = "\x1b[32m";
const BLUE: &str = "\x1b[34m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";

/// Syscall wrapper
#[inline]
unsafe fn syscall1(n: usize, a1: usize) -> isize {
    let ret: isize;
    core::arch::asm!(
        "syscall",
        in("rax") n,
        in("rdi") a1,
        lateout("rax") ret,
        options(nostack)
    );
    ret
}

#[inline]
unsafe fn syscall3(n: usize, a1: usize, a2: usize, a3: usize) -> isize {
    let ret: isize;
    core::arch::asm!(
        "syscall",
        in("rax") n,
        in("rdi") a1,
        in("rsi") a2,
        in("rdx") a3,
        lateout("rax") ret,
        options(nostack)
    );
    ret
}

/// Write to stdout
fn print(s: &str) {
    unsafe {
        syscall3(SYS_WRITE, STDOUT, s.as_ptr() as usize, s.len());
    }
}

/// Write to stderr
fn eprint(s: &str) {
    unsafe {
        syscall3(SYS_WRITE, STDERR, s.as_ptr() as usize, s.len());
    }
}

/// Print avec newline
fn println(s: &str) {
    print(s);
    print("\n");
}

/// Read from stdin (bloquant)
fn read_line(buf: &mut [u8]) -> usize {
    let mut pos = 0;
    
    loop {
        if pos >= buf.len() - 1 {
            break;
        }
        
        let mut c = [0u8; 1];
        let n = unsafe { syscall3(SYS_READ, STDIN, c.as_mut_ptr() as usize, 1) };
        
        if n <= 0 {
            break;
        }
        
        match c[0] {
            b'\n' | b'\r' => {
                print("\n");
                break;
            }
            0x7F | 0x08 => { // Backspace
                if pos > 0 {
                    pos -= 1;
                    print("\x08 \x08"); // Efface le caractère
                }
            }
            0x03 => { // Ctrl+C
                print("^C\n");
                pos = 0;
                break;
            }
            0x04 => { // Ctrl+D (EOF)
                if pos == 0 {
                    return 0; // Signal EOF
                }
            }
            32..=126 => { // Caractères affichables
                buf[pos] = c[0];
                pos += 1;
                unsafe {
                    syscall3(SYS_WRITE, STDOUT, c.as_ptr() as usize, 1);
                }
            }
            _ => {} // Ignorer autres caractères
        }
    }
    
    pos
}

/// Affiche le prompt
fn show_prompt() {
    print(GREEN);
    print("exo");
    print(RESET);
    print(":");
    print(BLUE);
    print("/"); // TODO: Afficher CWD avec getcwd()
    print(RESET);
    print("$ ");
}

/// Affiche le banner de bienvenue
fn show_banner() {
    println("\n╔══════════════════════════════════════╗");
    println("║      Exo-Shell v0.5.0                ║");
    println("║      Shell interactif pour Exo-OS    ║");
    println("╚══════════════════════════════════════╝\n");
    println("Tapez 'help' pour la liste des commandes\n");
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    show_banner();
    
    let mut line_buf = [0u8; 256];
    
    loop {
        show_prompt();
        
        let len = read_line(&mut line_buf);
        
        if len == 0 {
            // EOF (Ctrl+D) - quitter
            println("\nExit");
            unsafe { syscall1(SYS_EXIT, 0); }
        }
        
        // Convertir en &str
        let line = match core::str::from_utf8(&line_buf[..len]) {
            Ok(s) => s.trim(),
            Err(_) => {
                eprint("Erreur: ligne non-UTF8\n");
                continue;
            }
        };
        
        if line.is_empty() {
            continue;
        }
        
        // Parser et exécuter
        match parser::parse_command(line) {
            Ok(cmd) => {
                if let Err(e) = executor::execute(&cmd) {
                    eprint(RED);
                    eprint("Erreur: ");
                    eprint(e);
                    eprint("\n");
                    eprint(RESET);
                }
            }
            Err(e) => {
                eprint(RED);
                eprint("Parse error: ");
                eprint(e);
                eprint("\n");
                eprint(RESET);
            }
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    eprint("\n[PANIC] Shell crashed!\n");
    unsafe { syscall1(SYS_EXIT, 1); }
    loop {}
}
