//! Commandes built-in du shell

use crate::{print, println, eprint};

/// Liste des commandes built-in
pub fn is_builtin(cmd: &str) -> bool {
    matches!(cmd, "help" | "exit" | "clear" | "echo" | "pwd" | "cd" | "version")
}

/// Exécute une commande built-in
pub fn execute_builtin(name: &str, args: &[&str]) -> Result<(), &'static str> {
    match name {
        "help" => cmd_help(),
        "exit" => cmd_exit(args),
        "clear" => cmd_clear(),
        "echo" => cmd_echo(args),
        "pwd" => cmd_pwd(),
        "cd" => cmd_cd(args),
        "version" => cmd_version(),
        _ => Err("Commande built-in inconnue"),
    }
}

fn cmd_help() -> Result<(), &'static str> {
    println("\nCommandes built-in disponibles:");
    println("  help              Affiche cette aide");
    println("  exit [code]       Quitte le shell");
    println("  clear             Efface l'écran");
    println("  echo [args...]    Affiche les arguments");
    println("  pwd               Affiche le répertoire courant");
    println("  cd <dir>          Change de répertoire");
    println("  version           Affiche la version");
    println("  ls [dir]          Liste les fichiers");
    println("  cat <file>        Affiche le contenu d'un fichier");
    println("  mkdir <dir>       Crée un répertoire");
    println("  rm <file>         Supprime un fichier");
    println("  touch <file>      Crée un fichier vide");
    println("");
    Ok(())
}

fn cmd_exit(args: &[&str]) -> Result<(), &'static str> {
    let code = if args.is_empty() {
        0
    } else {
        // Parser le code de sortie
        parse_int(args[0]).unwrap_or(0)
    };
    
    println("Exit");
    unsafe {
        crate::syscall1(crate::SYS_EXIT, code as usize);
    }
    Ok(())
}

fn cmd_clear() -> Result<(), &'static str> {
    // ANSI escape: clear screen and move cursor to home
    print("\x1b[2J\x1b[H");
    Ok(())
}

fn cmd_echo(args: &[&str]) -> Result<(), &'static str> {
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            print(" ");
        }
        print(arg);
    }
    println("");
    Ok(())
}

fn cmd_pwd() -> Result<(), &'static str> {
    let mut buf = [0u8; 256];
    let ret = unsafe {
        crate::syscall3(crate::SYS_GETCWD, buf.as_mut_ptr() as usize, buf.len(), 0)
    };
    
    if ret < 0 {
        return Err("Erreur getcwd");
    }
    
    // Trouver la longueur (terminé par \0)
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    
    if let Ok(path) = core::str::from_utf8(&buf[..len]) {
        println(path);
        Ok(())
    } else {
        Err("Chemin non-UTF8")
    }
}

fn cmd_cd(args: &[&str]) -> Result<(), &'static str> {
    if args.is_empty() {
        return Err("Usage: cd <directory>");
    }
    
    let path = args[0];
    let ret = unsafe {
        crate::syscall3(crate::SYS_CHDIR, path.as_ptr() as usize, path.len(), 0)
    };
    
    if ret < 0 {
        Err("Répertoire introuvable")
    } else {
        Ok(())
    }
}

fn cmd_version() -> Result<(), &'static str> {
    println("Exo-Shell version 0.5.0");
    println("Exo-OS Microkernel Operating System");
    println("Copyright (c) 2025");
    Ok(())
}

/// Parse un entier simple (pas de gestion d'erreur robuste)
fn parse_int(s: &str) -> Option<i32> {
    let mut result = 0i32;
    let mut negative = false;
    
    let bytes = s.as_bytes();
    let mut i = 0;
    
    if bytes.is_empty() {
        return None;
    }
    
    if bytes[0] == b'-' {
        negative = true;
        i = 1;
    }
    
    while i < bytes.len() {
        let digit = bytes[i].wrapping_sub(b'0');
        if digit > 9 {
            return None;
        }
        result = result * 10 + digit as i32;
        i += 1;
    }
    
    Some(if negative { -result } else { result })
}
