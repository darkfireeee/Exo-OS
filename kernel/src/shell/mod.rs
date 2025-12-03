//! # Exo-Shell v0.5.0 - Interactive Kernel Shell
//!
//! Shell interactif pour Exo-OS avec support VFS complet
//!
//! ## Commandes disponibles
//! - `help` - Affiche l'aide
//! - `exit` - Quitte le shell (halt)
//! - `clear` - Efface l'écran
//! - `echo <text>` - Affiche du texte
//! - `pwd` - Affiche le répertoire courant
//! - `cd <dir>` - Change de répertoire
//! - `version` - Affiche la version
//! - `ls [path]` - Liste les fichiers
//! - `cat <file>` - Affiche le contenu d'un fichier
//! - `mkdir <dir>` - Crée un répertoire
//! - `rm <file>` - Supprime un fichier
//! - `rmdir <dir>` - Supprime un répertoire
//! - `touch <file>` - Crée un fichier vide
//! - `write <file> <text>` - Écrit dans un fichier

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use crate::fs::vfs;

const VERSION: &str = "0.5.0";

/// Affiche une string avec logger
fn print(s: &str) {
    crate::logger::early_print(s);
}

/// Affiche une string avec retour à la ligne
fn println(s: &str) {
    print(s);
    print("\n");
}

/// Point d'entrée du shell
pub fn run() -> ! {
    // Afficher le splash screen
    print_splash();
    
    // Boucle principale
    loop {
        // Afficher le prompt
        print("exo-os:~$ ");
        
        // Lire la commande (pour l'instant, simuler avec les commandes de test)
        // TODO: Implémenter la lecture du clavier
        println("Shell ready - keyboard input not yet implemented");
        println("Testing VFS commands...\n");
        
        // Tests automatiques pour valider le shell
        execute_command("help");
        execute_command("version");
        execute_command("ls /");
        execute_command("mkdir /test");
        execute_command("ls /");
        execute_command("touch /test/hello.txt");
        execute_command("write /test/hello.txt Hello from Exo-OS!");
        execute_command("cat /test/hello.txt");
        execute_command("ls /test");
        
        println("\n\nShell tests complete. System halting...");
        
        // Halt après les tests
        loop {
            unsafe {
                core::arch::asm!("hlt", options(nomem, nostack));
            }
        }
    }
}

/// Affiche le splash screen du shell
fn print_splash() {
    println("\n╔═══════════════════════════════════════════════════════════════╗");
    println("║                                                               ║");
    println("║     ███████╗██╗  ██╗ ██████╗       ███████╗██╗  ██╗          ║");
    println("║     ██╔════╝╚██╗██╔╝██╔═══██╗      ██╔════╝██║  ██║          ║");
    println("║     █████╗   ╚███╔╝ ██║   ██║█████╗███████╗███████║          ║");
    println("║     ██╔══╝   ██╔██╗ ██║   ██║╚════╝╚════██║██╔══██║          ║");
    println("║     ███████╗██╔╝ ██╗╚██████╔╝      ███████║██║  ██║          ║");
    println("║     ╚══════╝╚═╝  ╚═╝ ╚═════╝       ╚══════╝╚═╝  ╚═╝          ║");
    println("║                                                               ║");
    print("║               🚀 Interactive Kernel Shell v");
    print(VERSION);
    println("              ║");
    println("║                                                               ║");
    println("╚═══════════════════════════════════════════════════════════════╝");
    println("\n✨ Type 'help' for available commands\n");
}

/// Parse et exécute une commande
fn execute_command(line: &str) {
    let parts: Vec<&str> = line.trim().split_whitespace().collect();
    
    if parts.is_empty() {
        return;
    }
    
    let cmd = parts[0];
    let args = &parts[1..];
    
    match cmd {
        "help" => cmd_help(),
        "exit" => cmd_exit(),
        "clear" => cmd_clear(),
        "echo" => cmd_echo(args),
        "pwd" => cmd_pwd(),
        "cd" => cmd_cd(args),
        "version" => cmd_version(),
        "ls" => cmd_ls(args),
        "cat" => cmd_cat(args),
        "mkdir" => cmd_mkdir(args),
        "rm" => cmd_rm(args),
        "rmdir" => cmd_rmdir(args),
        "touch" => cmd_touch(args),
        "write" => cmd_write(args),
        _ => {
            print("❌ Unknown command: '");
            print(cmd);
            println("'. Type 'help' for available commands.");
        }
    }
}

// ============================================================================
// Commandes Shell
// ============================================================================

fn cmd_help() {
    print("\n📚 Exo-Shell v");
    print(VERSION);
    println(" - Available Commands:\n");
    println("  🔹 help              - Show this help message");
    println("  🔹 exit              - Exit the shell (halt system)");
    println("  🔹 clear             - Clear the screen");
    println("  🔹 echo <text>       - Print text to screen");
    println("  🔹 pwd               - Print working directory");
    println("  🔹 cd <dir>          - Change directory");
    println("  🔹 version           - Show shell version");
    println("\n📂 File System Commands:\n");
    println("  🔹 ls [path]         - List directory contents");
    println("  🔹 cat <file>        - Display file contents");
    println("  🔹 mkdir <dir>       - Create directory");
    println("  🔹 rm <file>         - Remove file");
    println("  🔹 rmdir <dir>       - Remove directory");
    println("  🔹 touch <file>      - Create empty file");
    println("  🔹 write <file> <txt> - Write text to file");
    println("");
}

fn cmd_exit() {
    println("\n👋 Goodbye from Exo-OS!\n");
    
    // Halt le système
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}

fn cmd_clear() {
    // TODO: Implémenter clear screen
    println("⚠️  clear not yet implemented");
}

fn cmd_echo(args: &[&str]) {
    if args.is_empty() {
        println("");
    } else {
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                print(" ");
            }
            print(arg);
        }
        println("");
    }
}

fn cmd_pwd() {
    // TODO: Implémenter le current working directory
    // Pour l'instant, hardcodé à "/"
    println("/");
}

fn cmd_cd(args: &[&str]) {
    if args.is_empty() {
        println("❌ Usage: cd <directory>");
        return;
    }
    
    // TODO: Implémenter le changement de répertoire
    println("⚠️  cd not yet implemented (working directory: /)");
}

fn cmd_version() {
    print("Exo-Shell v");
    println(VERSION);
    println("Part of Exo-OS v0.5.0 (Quantum Leap)");
    println("Build: 2024-12-03");
}

fn cmd_ls(args: &[&str]) {
    let path = if args.is_empty() { "/" } else { args[0] };
    
    print("\n📁 Listing: ");
    print(path);
    println("\n");
    
    // Utiliser VFS pour lister le répertoire
    match vfs::readdir(path) {
        Ok(entries) => {
            if entries.is_empty() {
                println("  (empty directory)");
            } else {
                for entry in entries {
                    print("  📄 ");
                    println(&entry);
                }
            }
            println("");
        }
        Err(_e) => {
            println("❌ Error reading directory\n");
        }
    }
}

fn cmd_cat(args: &[&str]) {
    if args.is_empty() {
        println("❌ Usage: cat <file>");
        return;
    }
    
    let path = args[0];
    
    // Ouvrir le fichier avec les constantes VFS
    match vfs::open(path, vfs::O_RDONLY) {
        Ok(fd) => {
            // Lire tout le contenu
            let mut buffer = [0u8; 4096];
            
            match vfs::read(fd, &mut buffer) {
                Ok(bytes_read) => {
                    if bytes_read == 0 {
                        println("(empty file)");
                    } else {
                        // Afficher le contenu (supposé être du texte UTF-8)
                        if let Ok(text) = core::str::from_utf8(&buffer[..bytes_read]) {
                            println(text);
                        } else {
                            println("❌ File contains non-UTF8 data");
                        }
                    }
                }
                Err(_e) => {
                    println("❌ Error reading file");
                }
            }
            
            // Fermer le fichier
            let _ = vfs::close(fd);
        }
        Err(_e) => {
            println("❌ Error opening file");
        }
    }
}

fn cmd_mkdir(args: &[&str]) {
    if args.is_empty() {
        println("❌ Usage: mkdir <directory>");
        return;
    }
    
    let path = args[0];
    
    match vfs::create_dir(path) {
        Ok(_) => {
            print("✅ Directory '");
            print(path);
            println("' created");
        }
        Err(_e) => {
            println("❌ Error creating directory");
        }
    }
}

fn cmd_rm(args: &[&str]) {
    if args.is_empty() {
        println("❌ Usage: rm <file>");
        return;
    }
    
    let path = args[0];
    
    match vfs::unlink(path) {
        Ok(_) => {
            print("✅ File '");
            print(path);
            println("' removed");
        }
        Err(_e) => {
            println("❌ Error removing file");
        }
    }
}

fn cmd_rmdir(args: &[&str]) {
    if args.is_empty() {
        println("❌ Usage: rmdir <directory>");
        return;
    }
    
    let path = args[0];
    
    match vfs::rmdir(path) {
        Ok(_) => {
            print("✅ Directory '");
            print(path);
            println("' removed");
        }
        Err(_e) => {
            println("❌ Error removing directory");
        }
    }
}

fn cmd_touch(args: &[&str]) {
    if args.is_empty() {
        println("❌ Usage: touch <file>");
        return;
    }
    
    let path = args[0];
    
    // Créer un fichier vide (open avec CREATE + WRITE)
    match vfs::open(path, vfs::O_CREAT | vfs::O_WRONLY) {
        Ok(fd) => {
            // Fermer immédiatement
            let _ = vfs::close(fd);
            print("✅ File '");
            print(path);
            println("' created");
        }
        Err(_e) => {
            println("❌ Error creating file");
        }
    }
}

fn cmd_write(args: &[&str]) {
    if args.len() < 2 {
        println("❌ Usage: write <file> <text>");
        return;
    }
    
    let path = args[0];
    let text_parts: Vec<&str> = args[1..].iter().copied().collect();
    let text = text_parts.join(" ");
    
    // Ouvrir en écriture (créer si n'existe pas)
    match vfs::open(path, vfs::O_CREAT | vfs::O_WRONLY | vfs::O_TRUNC) {
        Ok(fd) => {
            // Écrire le texte
            match vfs::write(fd, text.as_bytes()) {
                Ok(_bytes_written) => {
                    let _ = vfs::close(fd);
                    print("✅ Wrote to '");
                    print(path);
                    println("'");
                }
                Err(_e) => {
                    let _ = vfs::close(fd);
                    println("❌ Error writing to file");
                }
            }
        }
        Err(_e) => {
            println("❌ Error opening file for writing");
        }
    }
}
