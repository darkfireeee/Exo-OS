//! Exécuteur de commandes

use crate::parser::Command;
use crate::builtin;
use crate::{print, println, eprint};

// Syscalls
const SYS_OPEN: usize = 2;
const SYS_CLOSE: usize = 3;
const SYS_READ: usize = 0;
const SYS_WRITE: usize = 1;
const SYS_MKDIR: usize = 83;
const SYS_UNLINK: usize = 87;
const SYS_RMDIR: usize = 84;
const SYS_GETDENTS64: usize = 217;
const SYS_STAT: usize = 4;
const SYS_EXIT: usize = 60;

// Flags pour open
const O_RDONLY: usize = 0;
const O_WRONLY: usize = 1;
const O_CREAT: usize = 0o100;
const O_TRUNC: usize = 0o1000;

/// Exécute une commande
pub fn execute(cmd: &Command) -> Result<(), &'static str> {
    // Built-ins
    if builtin::is_builtin(cmd.name) {
        return builtin::execute_builtin(cmd.name, cmd.get_args());
    }
    
    // Commandes externes (fichiers dans /bin)
    match cmd.name {
        "ls" => cmd_ls(cmd.get_args()),
        "cat" => cmd_cat(cmd.get_args()),
        "mkdir" => cmd_mkdir(cmd.get_args()),
        "rm" => cmd_rm(cmd.get_args()),
        "rmdir" => cmd_rmdir(cmd.get_args()),
        "touch" => cmd_touch(cmd.get_args()),
        "write" => cmd_write(cmd.get_args()),
        _ => {
            eprint("Commande introuvable: ");
            eprint(cmd.name);
            eprint("\n");
            Err("Commande introuvable")
        }
    }
}

/// Liste les fichiers d'un répertoire
fn cmd_ls(args: &[&str]) -> Result<(), &'static str> {
    let path = if args.is_empty() { "/" } else { args[0] };
    
    // Ouvrir le répertoire
    let fd = unsafe {
        crate::syscall3(SYS_OPEN, path.as_ptr() as usize, O_RDONLY, 0)
    };
    
    if fd < 0 {
        return Err("Impossible d'ouvrir le répertoire");
    }
    
    // Buffer pour getdents64
    let mut buf = [0u8; 2048];
    
    loop {
        let nread = unsafe {
            crate::syscall3(SYS_GETDENTS64, fd as usize, buf.as_mut_ptr() as usize, buf.len())
        };
        
        if nread <= 0 {
            break;
        }
        
        // Parser les entrées (format getdents64)
        let mut offset = 0;
        while offset < nread as usize {
            // struct linux_dirent64 {
            //     u64 d_ino;
            //     i64 d_off;
            //     u16 d_reclen;
            //     u8 d_type;
            //     char d_name[];
            // }
            
            let reclen = u16::from_le_bytes([buf[offset + 16], buf[offset + 17]]) as usize;
            
            if reclen == 0 {
                break;
            }
            
            // Extraire le nom (après d_type à offset +18)
            let name_start = offset + 19;
            let name_end = (offset + reclen).min(buf.len());
            
            // Trouver le \0 terminal
            let mut name_len = 0;
            for i in name_start..name_end {
                if buf[i] == 0 {
                    break;
                }
                name_len += 1;
            }
            
            if name_len > 0 {
                if let Ok(name) = core::str::from_utf8(&buf[name_start..name_start + name_len]) {
                    println(name);
                }
            }
            
            offset += reclen;
        }
    }
    
    unsafe {
        crate::syscall1(SYS_CLOSE, fd as usize);
    }
    
    Ok(())
}

/// Affiche le contenu d'un fichier
fn cmd_cat(args: &[&str]) -> Result<(), &'static str> {
    if args.is_empty() {
        return Err("Usage: cat <file>");
    }
    
    let path = args[0];
    
    let fd = unsafe {
        crate::syscall3(SYS_OPEN, path.as_ptr() as usize, O_RDONLY, 0)
    };
    
    if fd < 0 {
        return Err("Impossible d'ouvrir le fichier");
    }
    
    let mut buf = [0u8; 512];
    
    loop {
        let n = unsafe {
            crate::syscall3(SYS_READ, fd as usize, buf.as_mut_ptr() as usize, buf.len())
        };
        
        if n <= 0 {
            break;
        }
        
        unsafe {
            crate::syscall3(SYS_WRITE, 1, buf.as_ptr() as usize, n as usize);
        }
    }
    
    unsafe {
        crate::syscall1(SYS_CLOSE, fd as usize);
    }
    
    Ok(())
}

/// Crée un répertoire
fn cmd_mkdir(args: &[&str]) -> Result<(), &'static str> {
    if args.is_empty() {
        return Err("Usage: mkdir <directory>");
    }
    
    let path = args[0];
    let ret = unsafe {
        crate::syscall3(SYS_MKDIR, path.as_ptr() as usize, 0o755, 0)
    };
    
    if ret < 0 {
        Err("Impossible de créer le répertoire")
    } else {
        Ok(())
    }
}

/// Supprime un fichier
fn cmd_rm(args: &[&str]) -> Result<(), &'static str> {
    if args.is_empty() {
        return Err("Usage: rm <file>");
    }
    
    let path = args[0];
    let ret = unsafe {
        crate::syscall3(SYS_UNLINK, path.as_ptr() as usize, path.len(), 0)
    };
    
    if ret < 0 {
        Err("Impossible de supprimer le fichier")
    } else {
        Ok(())
    }
}

/// Supprime un répertoire vide
fn cmd_rmdir(args: &[&str]) -> Result<(), &'static str> {
    if args.is_empty() {
        return Err("Usage: rmdir <directory>");
    }
    
    let path = args[0];
    let ret = unsafe {
        crate::syscall3(SYS_RMDIR, path.as_ptr() as usize, path.len(), 0)
    };
    
    if ret < 0 {
        Err("Impossible de supprimer le répertoire")
    } else {
        Ok(())
    }
}

/// Crée un fichier vide
fn cmd_touch(args: &[&str]) -> Result<(), &'static str> {
    if args.is_empty() {
        return Err("Usage: touch <file>");
    }
    
    let path = args[0];
    let fd = unsafe {
        crate::syscall3(SYS_OPEN, path.as_ptr() as usize, O_WRONLY | O_CREAT, 0o644)
    };
    
    if fd < 0 {
        Err("Impossible de créer le fichier")
    } else {
        unsafe {
            crate::syscall1(SYS_CLOSE, fd as usize);
        }
        Ok(())
    }
}

/// Écrit dans un fichier
fn cmd_write(args: &[&str]) -> Result<(), &'static str> {
    if args.len() < 2 {
        return Err("Usage: write <file> <text>");
    }
    
    let path = args[0];
    let text = args[1];
    
    let fd = unsafe {
        crate::syscall3(SYS_OPEN, path.as_ptr() as usize, O_WRONLY | O_CREAT | O_TRUNC, 0o644)
    };
    
    if fd < 0 {
        return Err("Impossible d'ouvrir le fichier");
    }
    
    let ret = unsafe {
        crate::syscall3(SYS_WRITE, fd as usize, text.as_ptr() as usize, text.len())
    };
    
    unsafe {
        crate::syscall1(SYS_CLOSE, fd as usize);
    }
    
    if ret < 0 {
        Err("Erreur d'écriture")
    } else {
        Ok(())
    }
}
