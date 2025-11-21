// libs/exo_std/src/process/exit.rs

/// Termine le processus en cours avec le code de sortie spécifié
pub fn exit(code: i32) -> ! {
    sys_exit(code);
    loop {} // Empêcher le retour
}

/// Termine le processus en cas d'erreur fatale
pub fn abort() -> ! {
    sys_abort();
    loop {} // Empêcher le retour
}

// Appels système
fn sys_exit(code: i32) -> ! {
    #[cfg(feature = "test_mode")]
    {
        // En mode test, panic pour arrêter l'exécution
        panic!("Process exited with code {}", code);
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_exit(code: i32) -> !;
            }
            sys_exit(code);
        }
    }
}

fn sys_abort() -> ! {
    #[cfg(feature = "test_mode")]
    {
        panic!("Process aborted");
    }
    
    #[cfg(not(feature = "test_mode"))]
    {
        unsafe {
            extern "C" {
                fn sys_abort() -> !;
            }
            sys_abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::panic::{self, AssertUnwindSafe};
    
    #[test]
    fn test_exit() {
        // Ne peut pas tester directement exit() car il ne retourne pas
        // On teste à la place que la fonction compile
        let _ = exit;
    }
    
    #[test]
    #[should_panic(expected = "Process exited with code 42")]
    fn test_exit_panics_in_test_mode() {
        exit(42);
    }
}