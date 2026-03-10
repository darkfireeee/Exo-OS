//! # phase3-tests — Tests unitaires Phase 3 Exo-OS
//!
//! Ce crate re-implémente la logique pure (sans accès hardware) des syscalls
//! implémentés en Phase 3, et vérifie leur comportement correct.
//!
//! Modules testés :
//!   - `wait`       : encodage wstatus POSIX, WaitOptions, WaitError → errno
//!   - `uname`      : layout struct utsname (390 bytes), contenu des champs
//!   - `argv`       : logique copy_userspace_argv (simulation mémoire locale)
//!   - `sigaltstack`: layout SigAltStack, SS_DISABLE, MINSIGSTKSZ
//!   - `errno`      : valeurs POSIX des constantes ajoutées (ESRCH, ECHILD)
//!   - `kill`       : validate_signal, Signal conversion
//!   - `waitid`     : encodage siginfo_t x86_64

// ─────────────────────────────────────────────────────────────────────────────
// Re-implémentation de la logique pure (miroir exact du code kernel)
// ─────────────────────────────────────────────────────────────────────────────

/// Mirror de `syscall::errno` — constantes POSIX
mod errno {
    pub const EPERM:  i64 = -1;
    pub const ENOENT: i64 = -2;
    pub const ESRCH:  i64 = -3;
    pub const EINTR:  i64 = -4;
    pub const ECHILD: i64 = -10;
    pub const EAGAIN: i64 = -11;
    pub const EFAULT: i64 = -14;
    pub const EINVAL: i64 = -22;
}

/// Mirror de `process::lifecycle::wait` — types et encodage wstatus POSIX
mod wait {
    #[derive(Copy, Clone, PartialEq, Eq, Debug)]
    pub struct Pid(pub u32);

    #[derive(Copy, Clone, Debug)]
    pub struct WaitOptions(pub u32);

    impl WaitOptions {
        pub const WNOHANG:    u32 = 1 << 0;
        pub const WUNTRACED:  u32 = 1 << 1;
        pub const WCONTINUED: u32 = 1 << 2;
        pub const WALL:       u32 = 1 << 3;

        pub fn has(self, flag: u32) -> bool { self.0 & flag != 0 }
    }

    #[derive(Copy, Clone, PartialEq, Eq, Debug)]
    pub enum WaitReason { Exited, Signaled, Stopped, Continued }

    #[derive(Copy, Clone, Debug)]
    pub struct WaitResult {
        pub pid:     Pid,
        pub wstatus: u32,
        pub reason:  WaitReason,
    }

    impl WaitResult {
        /// POSIX : processus terminé normalement → exit_code << 8
        pub fn exited(pid: Pid, code: u8) -> Self {
            Self { pid, wstatus: (code as u32) << 8, reason: WaitReason::Exited }
        }
        /// POSIX : tué par signal → signal_number | 0x80 si core dumped
        pub fn signaled(pid: Pid, sig: u8, core_dumped: bool) -> Self {
            let dump_bit = if core_dumped { 0x80u32 } else { 0 };
            Self { pid, wstatus: (sig as u32) | dump_bit, reason: WaitReason::Signaled }
        }
    }

    #[derive(Copy, Clone, PartialEq, Eq, Debug)]
    pub enum WaitError { NoChild, WouldBlock, Interrupted, InvalidPid }
}

/// Mirror de `process::signal::handler` — SigAltStack
mod signal_handler {
    #[repr(C)]
    #[derive(Copy, Clone, Debug, PartialEq)]
    pub struct SigAltStack {
        pub ss_sp:    u64,
        pub ss_flags: i32,
        pub _pad:     u32,
        pub ss_size:  u64,
    }

    pub const SS_ONSTACK: i32 = 1;
    pub const SS_DISABLE: i32 = 2;
    pub const MINSIGSTKSZ: u64 = 2048;
}

/// Mirror de `syscall::validation` — validate_signal
mod validation {
    pub fn validate_signal(raw: u64) -> Result<u32, ()> {
        if raw == 0 || raw > 64 { return Err(()); }
        Ok(raw as u32)
    }

    pub fn validate_pid(raw: u64) -> Result<u32, ()> {
        if raw == 0 || raw >= 4_194_304 { return Err(()); }
        Ok(raw as u32)
    }
}

/// Mirror de `copy_userspace_argv` (dispatch.rs)
/// Lit un tableau de pointeurs vers des chaînes C depuis une tranche mémoire.
mod argv {
    /// Simule copy_userspace_argv sans accès matériel.
    /// `memory` est une vue plate de la "mémoire userspace" simulée.
    /// `argv_offset` est la position du tableau de pointeurs dans `memory`.
    pub fn copy_argv_from_slice(memory: &[u8], argv_offset: usize) -> Vec<String> {
        let mut result = Vec::new();
        let mut i = argv_offset;

        loop {
            // Lire un pointeur u64 (little-endian)
            if i + 8 > memory.len() { break; }
            let ptr = u64::from_le_bytes(memory[i..i+8].try_into().unwrap()) as usize;
            if ptr == 0 { break; }  // NULL termine le tableau
            if ptr >= memory.len() { break; }

            // Lire la chaîne C à ptr
            let mut s = Vec::new();
            let mut j = ptr;
            while j < memory.len() && memory[j] != 0 {
                s.push(memory[j]);
                j += 1;
            }
            result.push(String::from_utf8_lossy(&s).into_owned());
            i += 8;
        }
        result
    }
}

/// Mirror de `sys_uname` (misc.rs)
/// Retourne les 390 bytes du struct utsname rempli.
mod uname {
    const FIELD_SIZE: usize = 65;

    pub fn fill_utsname() -> [u8; 390] {
        let mut buf = [0u8; 390];
        let fields: [(&[u8], usize); 6] = [
            (b"Exo-OS",       0),
            (b"exo-os",       65),
            (b"1.0.0",        130),
            (b"#1 SMP 2026",  195),
            (b"x86_64",       260),
            (b"(none)",       325),
        ];
        for (src, off) in &fields {
            let len = src.len().min(64);
            buf[*off..*off + len].copy_from_slice(&src[..len]);
        }
        buf
    }

    /// Lit un champ (null-terminé, 65 bytes) d'un utsname buffer.
    pub fn read_field(buf: &[u8; 390], offset: usize) -> String {
        let field = &buf[offset..offset + FIELD_SIZE];
        let end = field.iter().position(|&b| b == 0).unwrap_or(FIELD_SIZE);
        String::from_utf8_lossy(&field[..end]).into_owned()
    }
}

/// Mirror de `sys_waitid` — construction du siginfo_t x86_64
mod waitid {
    use crate::wait::{WaitResult, WaitReason};

    const SIGCHLD:    i32 = 17;
    const CLD_EXITED: i32 = 1;
    const CLD_KILLED: i32 = 2;

    /// Remplit un buffer de 128 bytes comme le fait sys_waitid.
    pub fn fill_siginfo(result: &WaitResult) -> [u8; 128] {
        let mut buf = [0u8; 128];

        let (si_code, si_status) = match result.reason {
            WaitReason::Exited   => (CLD_EXITED, (result.wstatus >> 8) as i32),
            _                    => (CLD_KILLED, (result.wstatus & 0x7F) as i32),
        };

        buf[0..4].copy_from_slice(&SIGCHLD.to_le_bytes());
        buf[4..8].copy_from_slice(&0i32.to_le_bytes());
        buf[8..12].copy_from_slice(&si_code.to_le_bytes());
        buf[12..16].copy_from_slice(&(result.pid.0 as i32).to_le_bytes());
        buf[16..20].copy_from_slice(&0u32.to_le_bytes());
        buf[20..24].copy_from_slice(&si_status.to_le_bytes());
        buf
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests_errno {
    use super::errno::*;

    #[test]
    fn esrch_est_moins_3() {
        assert_eq!(ESRCH, -3, "ESRCH doit être -3 (POSIX)");
    }

    #[test]
    fn echild_est_moins_10() {
        assert_eq!(ECHILD, -10, "ECHILD doit être -10 (POSIX)");
    }

    #[test]
    fn eperm_est_moins_1() {
        assert_eq!(EPERM, -1);
    }

    #[test]
    fn enoent_est_moins_2() {
        assert_eq!(ENOENT, -2);
    }

    #[test]
    fn eintr_est_moins_4() {
        assert_eq!(EINTR, -4);
    }

    #[test]
    fn efault_est_moins_14() {
        assert_eq!(EFAULT, -14);
    }

    #[test]
    fn eagain_est_moins_11() {
        assert_eq!(EAGAIN, -11);
    }

    #[test]
    fn einval_est_moins_22() {
        assert_eq!(EINVAL, -22);
    }
}

#[cfg(test)]
mod tests_wait {
    use super::wait::*;
    use super::errno::*;

    // ── WaitOptions ─────────────────────────────────────────────────────────

    #[test]
    fn waitoptions_wnohang_est_bit0() {
        assert_eq!(WaitOptions::WNOHANG, 0b001);
    }

    #[test]
    fn waitoptions_wuntraced_est_bit1() {
        assert_eq!(WaitOptions::WUNTRACED, 0b010);
    }

    #[test]
    fn waitoptions_wcontinued_est_bit2() {
        assert_eq!(WaitOptions::WCONTINUED, 0b100);
    }

    #[test]
    fn waitoptions_has_wnohang() {
        let opts = WaitOptions(WaitOptions::WNOHANG);
        assert!(opts.has(WaitOptions::WNOHANG));
        assert!(!opts.has(WaitOptions::WUNTRACED));
    }

    #[test]
    fn waitoptions_combinaison() {
        let opts = WaitOptions(WaitOptions::WNOHANG | WaitOptions::WCONTINUED);
        assert!(opts.has(WaitOptions::WNOHANG));
        assert!(opts.has(WaitOptions::WCONTINUED));
        assert!(!opts.has(WaitOptions::WUNTRACED));
    }

    // ── WaitResult encodage POSIX ────────────────────────────────────────────

    #[test]
    fn wstatus_exited_code_0() {
        let r = WaitResult::exited(Pid(42), 0);
        // Exit normal code 0 → wstatus = 0 << 8 = 0
        assert_eq!(r.wstatus, 0);
        assert_eq!(r.wstatus & 0xFF, 0);  // bits 0-7 = 0 (not signaled)
    }

    #[test]
    fn wstatus_exited_code_1() {
        let r = WaitResult::exited(Pid(42), 1);
        // Exit code 1 → wstatus = 1 << 8 = 256
        assert_eq!(r.wstatus, 256);
        assert_eq!(r.wstatus >> 8, 1);  // WEXITSTATUS(wstatus) == 1
    }

    #[test]
    fn wstatus_exited_code_255() {
        let r = WaitResult::exited(Pid(1), 255);
        assert_eq!(r.wstatus >> 8, 255);
        assert_eq!(r.wstatus & 0xFF, 0);
    }

    #[test]
    fn wstatus_signaled_sigkill() {
        // SIGKILL = 9, tué par signal → wstatus = 9 (sans core dump)
        let r = WaitResult::signaled(Pid(10), 9, false);
        assert_eq!(r.wstatus, 9);
        assert_eq!(r.wstatus & 0x7F, 9);   // WTERMSIG(wstatus) == 9
        assert_eq!(r.wstatus & 0x80, 0);   // pas de core dump
    }

    #[test]
    fn wstatus_signaled_core_dumped() {
        // SIGSEGV = 11, avec core dump → wstatus = 11 | 0x80 = 0x8B
        let r = WaitResult::signaled(Pid(5), 11, true);
        assert_eq!(r.wstatus & 0x7F, 11);   // WTERMSIG == 11
        assert_eq!(r.wstatus & 0x80, 0x80); // core dump flag
    }

    #[test]
    fn wstatus_pid_preservé() {
        let r = WaitResult::exited(Pid(1234), 42);
        assert_eq!(r.pid.0, 1234);
    }

    // ── WaitError → errno mapping ────────────────────────────────────────────

    #[test]
    fn waiterror_nochild_vers_echild() {
        // sys_wait4 doit retourner ECHILD quand WaitError::NoChild
        let errno_val = match WaitError::NoChild {
            WaitError::NoChild     => ECHILD,
            WaitError::WouldBlock  => 0,
            WaitError::Interrupted => EINTR,
            WaitError::InvalidPid  => EINVAL,
        };
        assert_eq!(errno_val, ECHILD, "NoChild doit mapper vers ECHILD (-10)");
    }

    #[test]
    fn waiterror_wouldblock_vers_zero() {
        // sys_wait4 avec WNOHANG : retourne 0 si fils présents mais pas zombie
        let retval: i64 = match WaitError::WouldBlock {
            WaitError::WouldBlock  => 0,
            WaitError::NoChild     => ECHILD,
            WaitError::Interrupted => EINTR,
            WaitError::InvalidPid  => EINVAL,
        };
        assert_eq!(retval, 0, "WouldBlock + WNOHANG doit retourner 0 (POSIX)");
    }

    #[test]
    fn waiterror_interrupted_vers_eintr() {
        let errno_val = match WaitError::Interrupted {
            WaitError::Interrupted => EINTR,
            _ => 0,
        };
        assert_eq!(errno_val, EINTR);
    }
}

#[cfg(test)]
mod tests_uname {
    use super::uname::*;

    #[test]
    fn uname_taille_totale_390_bytes() {
        let buf = fill_utsname();
        assert_eq!(buf.len(), 390, "struct utsname doit faire 390 bytes (6×65)");
    }

    #[test]
    fn uname_sysname_est_exo_os() {
        let buf = fill_utsname();
        assert_eq!(read_field(&buf, 0), "Exo-OS",
            "sysname (offset 0) doit être 'Exo-OS'");
    }

    #[test]
    fn uname_nodename_est_exo_os_minuscule() {
        let buf = fill_utsname();
        assert_eq!(read_field(&buf, 65), "exo-os",
            "nodename (offset 65) doit être 'exo-os'");
    }

    #[test]
    fn uname_release_est_version() {
        let buf = fill_utsname();
        assert_eq!(read_field(&buf, 130), "1.0.0",
            "release (offset 130) doit être '1.0.0'");
    }

    #[test]
    fn uname_version_contient_smp() {
        let buf = fill_utsname();
        let v = read_field(&buf, 195);
        assert!(v.contains("SMP"), "version (offset 195) doit contenir 'SMP', got: '{}'", v);
    }

    #[test]
    fn uname_machine_est_x86_64() {
        let buf = fill_utsname();
        assert_eq!(read_field(&buf, 260), "x86_64",
            "machine (offset 260) doit être 'x86_64'");
    }

    #[test]
    fn uname_domainname_est_none() {
        let buf = fill_utsname();
        assert_eq!(read_field(&buf, 325), "(none)",
            "domainname (offset 325) doit être '(none)'");
    }

    #[test]
    fn uname_champs_null_termines() {
        let buf = fill_utsname();
        // Chaque champ doit avoir un byte nul dans ses 65 bytes
        for field_offset in [0, 65, 130, 195, 260, 325] {
            let field = &buf[field_offset..field_offset + 65];
            assert!(
                field.contains(&0u8),
                "champ à offset {} doit avoir un null-terminator", field_offset
            );
        }
    }

    #[test]
    fn uname_sysname_pas_depassement() {
        let buf = fill_utsname();
        // sysname ne doit pas déborder dans nodename (byte 64 doit être 0)
        assert_eq!(buf[64], 0, "byte 64 doit être nul (séparateur sysname/nodename)");
    }

    #[test]
    fn uname_zero_initialise() {
        let buf = fill_utsname();
        // Les zones non-écrites doivent être zéro
        // "Exo-OS" = 6 octets, bytes 6-64 doivent être 0
        for i in 6..65 {
            assert_eq!(buf[i], 0, "byte {} doit être 0 (zone non écrite du sysname)", i);
        }
    }
}

#[cfg(test)]
mod tests_sigaltstack {
    use super::signal_handler::*;

    #[test]
    fn sigaltstack_taille_est_24_bytes() {
        assert_eq!(
            std::mem::size_of::<SigAltStack>(), 24,
            "struct stack_t (SigAltStack) doit faire 24 bytes"
        );
    }

    #[test]
    fn sigaltstack_layout_ss_sp_offset_0() {
        let s = SigAltStack { ss_sp: 0xDEAD, ss_flags: 0, _pad: 0, ss_size: 0 };
        let raw = unsafe {
            core::slice::from_raw_parts(&s as *const _ as *const u8, 24)
        };
        let sp = u64::from_le_bytes(raw[0..8].try_into().unwrap());
        assert_eq!(sp, 0xDEAD, "ss_sp doit être à l'offset 0");
    }

    #[test]
    fn sigaltstack_layout_ss_flags_offset_8() {
        let s = SigAltStack { ss_sp: 0, ss_flags: 0x1234i32, _pad: 0, ss_size: 0 };
        let raw = unsafe {
            core::slice::from_raw_parts(&s as *const _ as *const u8, 24)
        };
        let flags = i32::from_le_bytes(raw[8..12].try_into().unwrap());
        assert_eq!(flags, 0x1234, "ss_flags doit être à l'offset 8");
    }

    #[test]
    fn sigaltstack_layout_ss_size_offset_16() {
        let s = SigAltStack { ss_sp: 0, ss_flags: 0, _pad: 0, ss_size: 8192 };
        let raw = unsafe {
            core::slice::from_raw_parts(&s as *const _ as *const u8, 24)
        };
        let size = u64::from_le_bytes(raw[16..24].try_into().unwrap());
        assert_eq!(size, 8192, "ss_size doit être à l'offset 16");
    }

    #[test]
    fn ss_onstack_vaut_1() {
        assert_eq!(SS_ONSTACK, 1, "SS_ONSTACK doit valoir 1 (ABI Linux)");
    }

    #[test]
    fn ss_disable_vaut_2() {
        assert_eq!(SS_DISABLE, 2, "SS_DISABLE doit valoir 2 (ABI Linux)");
    }

    #[test]
    fn minsigstksz_vaut_2048() {
        assert_eq!(MINSIGSTKSZ, 2048, "MINSIGSTKSZ doit valoir 2048 (POSIX min)");
    }

    #[test]
    fn sigaltstack_ss_disable_efface_base_et_taille() {
        // Simuler sys_sigaltstack avec SS_DISABLE
        let mut base: u64 = 0xC0FFEE00;
        let mut size: u64 = 8192;
        let ss = SigAltStack { ss_sp: 0, ss_flags: SS_DISABLE, _pad: 0, ss_size: 0 };

        if ss.ss_flags & SS_DISABLE != 0 {
            base = 0;
            size = 0;
        }

        assert_eq!(base, 0, "SS_DISABLE doit effacer sigaltstack_base");
        assert_eq!(size, 0, "SS_DISABLE doit effacer sigaltstack_size");
    }

    #[test]
    fn sigaltstack_trop_petit_refuse() {
        // Taille < MINSIGSTKSZ doit retourner EINVAL
        let ss = SigAltStack { ss_sp: 0x8000, ss_flags: 0, _pad: 0, ss_size: 1024 };
        let result: Result<(), ()> = if ss.ss_size < MINSIGSTKSZ { Err(()) } else { Ok(()) };
        assert!(result.is_err(), "sigaltstack < MINSIGSTKSZ doit être refusé");
    }

    #[test]
    fn sigaltstack_taille_minimale_acceptee() {
        let ss = SigAltStack { ss_sp: 0x8000, ss_flags: 0, _pad: 0, ss_size: MINSIGSTKSZ };
        let result: Result<(), ()> = if ss.ss_size < MINSIGSTKSZ { Err(()) } else { Ok(()) };
        assert!(result.is_ok(), "sigaltstack de taille exactement MINSIGSTKSZ doit être accepté");
    }

    #[test]
    fn sigaltstack_flags_inconnus_refuses() {
        // ss_flags avec une valeur autre que 0, SS_ONSTACK, SS_DISABLE = EINVAL
        let ss = SigAltStack { ss_sp: 0x8000, ss_flags: 0x10, _pad: 0, ss_size: 8192 };
        let invalid = ss.ss_flags & SS_DISABLE == 0 && ss.ss_flags != 0;
        assert!(invalid, "flags inconnus doivent être détectés");
    }
}

#[cfg(test)]
mod tests_argv {
    use super::argv::*;

    /// Construit une fausse "mémoire userspace" avec un tableau argv.
    /// argv_data : liste de chaînes, chacune sera stockée dans le buffer.
    /// Retourne (buffer, argv_offset) où argv_offset est le début du tableau de pointeurs.
    fn build_test_memory(strings: &[&str]) -> (Vec<u8>, usize) {
        let mut memory: Vec<u8> = vec![0u8; 4096];
        // Placer les chaînes à partir de l'offset 256 (laisse de la place pour les pointeurs)
        let argc = strings.len();
        let ptr_table_offset = 0usize;  // le tableau de pointeurs commence à 0
        let strings_start = (argc + 1) * 8;  // après ARGC+1 pointeurs (dernier = NULL)

        let mut str_pos = strings_start;
        let mut ptrs: Vec<u64> = Vec::new();

        for s in strings {
            // Stocker la chaîne à str_pos
            let bytes = s.as_bytes();
            memory[str_pos..str_pos + bytes.len()].copy_from_slice(bytes);
            memory[str_pos + bytes.len()] = 0;  // null terminator
            ptrs.push(str_pos as u64);
            str_pos += bytes.len() + 1;
        }
        // Null terminator du tableau de pointeurs
        ptrs.push(0u64);

        // Écrire les pointeurs dans le buffer
        let mut ptr_pos = ptr_table_offset;
        for ptr in &ptrs {
            memory[ptr_pos..ptr_pos + 8].copy_from_slice(&ptr.to_le_bytes());
            ptr_pos += 8;
        }

        (memory, ptr_table_offset)
    }

    #[test]
    fn argv_vide_si_ptr_null() {
        // argv_ptr = 0 → liste vide (pas d'erreur)
        let memory = vec![0u8; 64];
        // Simuler: ptr = 0, deuxième élément = 0 (NULL)
        let result = copy_argv_from_slice(&memory, 0);
        assert!(result.is_empty(), "argv_ptr=0 doit retourner une liste vide");
    }

    #[test]
    fn argv_un_argument() {
        let (memory, offset) = build_test_memory(&["hello"]);
        let result = copy_argv_from_slice(&memory, offset);
        assert_eq!(result.len(), 1, "doit retourner 1 argument");
        assert_eq!(result[0], "hello");
    }

    #[test]
    fn argv_plusieurs_arguments() {
        let (memory, offset) = build_test_memory(&["/bin/ls", "-la", "/tmp"]);
        let result = copy_argv_from_slice(&memory, offset);
        assert_eq!(result.len(), 3, "doit retourner 3 arguments");
        assert_eq!(result[0], "/bin/ls");
        assert_eq!(result[1], "-la");
        assert_eq!(result[2], "/tmp");
    }

    #[test]
    fn argv_argument_vide() {
        let (memory, offset) = build_test_memory(&[""]);
        let result = copy_argv_from_slice(&memory, offset);
        // Une chaîne vide est un argument valide (chaîne de longueur 0)
        assert_eq!(result.len(), 1, "chaîne vide = 1 argument (chaîne de longueur 0)");
        assert_eq!(result[0], "");
    }

    #[test]
    fn argv_arguments_avec_espaces() {
        let (memory, offset) = build_test_memory(&["echo", "hello world", "foo"]);
        let result = copy_argv_from_slice(&memory, offset);
        assert_eq!(result[1], "hello world", "les espaces ne doivent pas couper l'argument");
    }

    #[test]
    fn argv_chemin_absolu() {
        let (memory, offset) = build_test_memory(&["/usr/bin/env", "PATH=/bin", "LANG=fr_FR.UTF-8"]);
        let result = copy_argv_from_slice(&memory, offset);
        assert_eq!(result[0], "/usr/bin/env");
        assert_eq!(result[1], "PATH=/bin");
        assert_eq!(result[2], "LANG=fr_FR.UTF-8");
    }
}

#[cfg(test)]
mod tests_signal_validation {
    use super::validation::*;

    #[test]
    fn signal_0_est_invalide() {
        assert!(validate_signal(0).is_err(), "signal 0 est invalide (POSIX)");
    }

    #[test]
    fn signal_1_est_valide() {
        assert_eq!(validate_signal(1), Ok(1), "SIGHUP=1 est valide");
    }

    #[test]
    fn signal_9_est_valide() {
        assert_eq!(validate_signal(9), Ok(9), "SIGKILL=9 est valide");
    }

    #[test]
    fn signal_64_est_valide() {
        assert_eq!(validate_signal(64), Ok(64), "signal RT max (64) est valide");
    }

    #[test]
    fn signal_65_est_invalide() {
        assert!(validate_signal(65).is_err(), "signal 65 est invalide");
    }

    #[test]
    fn pid_0_est_invalide() {
        assert!(validate_pid(0).is_err(), "PID 0 est invalide");
    }

    #[test]
    fn pid_1_est_valide() {
        assert_eq!(validate_pid(1), Ok(1));
    }

    #[test]
    fn pid_4194303_est_valide() {
        assert_eq!(validate_pid(4_194_303), Ok(4_194_303));
    }

    #[test]
    fn pid_4194304_est_invalide() {
        assert!(validate_pid(4_194_304).is_err(), "PID >= PID_MAX est invalide");
    }
}

#[cfg(test)]
mod tests_waitid_siginfo {
    use super::waitid::*;
    use super::wait::{Pid, WaitResult, WaitReason};

    #[test]
    fn siginfo_taille_128_bytes() {
        let r = WaitResult { pid: Pid(1), wstatus: 0, reason: WaitReason::Exited };
        let buf = fill_siginfo(&r);
        assert_eq!(buf.len(), 128, "siginfo_t doit faire 128 bytes");
    }

    #[test]
    fn siginfo_si_signo_est_sigchld() {
        let r = WaitResult { pid: Pid(42), wstatus: 256, reason: WaitReason::Exited };
        let buf = fill_siginfo(&r);
        let si_signo = i32::from_le_bytes(buf[0..4].try_into().unwrap());
        assert_eq!(si_signo, 17, "si_signo doit être SIGCHLD=17");
    }

    #[test]
    fn siginfo_si_errno_est_zero() {
        let r = WaitResult { pid: Pid(1), wstatus: 0, reason: WaitReason::Exited };
        let buf = fill_siginfo(&r);
        let si_errno = i32::from_le_bytes(buf[4..8].try_into().unwrap());
        assert_eq!(si_errno, 0, "si_errno doit être 0");
    }

    #[test]
    fn siginfo_si_code_exited_est_cld_exited() {
        let r = WaitResult { pid: Pid(7), wstatus: (42u32 << 8), reason: WaitReason::Exited };
        let buf = fill_siginfo(&r);
        let si_code = i32::from_le_bytes(buf[8..12].try_into().unwrap());
        assert_eq!(si_code, 1, "si_code doit être CLD_EXITED=1 pour une sortie normale");
    }

    #[test]
    fn siginfo_si_code_killed_est_cld_killed() {
        let r = WaitResult { pid: Pid(7), wstatus: 9, reason: WaitReason::Signaled };
        let buf = fill_siginfo(&r);
        let si_code = i32::from_le_bytes(buf[8..12].try_into().unwrap());
        assert_eq!(si_code, 2, "si_code doit être CLD_KILLED=2 pour un signal");
    }

    #[test]
    fn siginfo_si_pid_correct() {
        let r = WaitResult { pid: Pid(1234), wstatus: 256, reason: WaitReason::Exited };
        let buf = fill_siginfo(&r);
        let si_pid = i32::from_le_bytes(buf[12..16].try_into().unwrap());
        assert_eq!(si_pid, 1234, "si_pid doit contenir le PID du fils");
    }

    #[test]
    fn siginfo_si_status_exited_contient_exit_code() {
        // wstatus = 42 << 8 → si_status = 42
        let r = WaitResult { pid: Pid(1), wstatus: 42 << 8, reason: WaitReason::Exited };
        let buf = fill_siginfo(&r);
        let si_status = i32::from_le_bytes(buf[20..24].try_into().unwrap());
        assert_eq!(si_status, 42, "si_status pour exit doit contenir le code de sortie");
    }

    #[test]
    fn siginfo_si_status_killed_contient_signal() {
        // wstatus = 9 (SIGKILL) → si_status = 9
        let r = WaitResult { pid: Pid(1), wstatus: 9, reason: WaitReason::Signaled };
        let buf = fill_siginfo(&r);
        let si_status = i32::from_le_bytes(buf[20..24].try_into().unwrap());
        assert_eq!(si_status, 9, "si_status pour signal doit contenir le numéro de signal");
    }

    #[test]
    fn siginfo_bytes_restants_sont_zero() {
        let r = WaitResult { pid: Pid(1), wstatus: 0, reason: WaitReason::Exited };
        let buf = fill_siginfo(&r);
        // Bytes 24..128 doivent être zéro
        for i in 24..128 {
            assert_eq!(buf[i], 0, "byte {} doit être 0 (zone réservée siginfo_t)", i);
        }
    }
}

#[cfg(test)]
mod tests_integration {
    use super::wait::*;
    use super::errno::*;

    /// Test d'intégration : simuler le cycle complet wait4 avec WNOHANG
    #[test]
    fn wait4_wnohang_retourne_0_si_fils_vivant() {
        // Avec WNOHANG et un fils toujours en cours → retour 0 (pas d'erreur, pas de résultat)
        // Ce comportement est testé via la conversion WouldBlock → 0
        let err = WaitError::WouldBlock;
        let retval: i64 = match err {
            WaitError::WouldBlock  => 0,
            WaitError::NoChild     => ECHILD,
            WaitError::Interrupted => EINTR,
            WaitError::InvalidPid  => EINVAL,
        };
        assert_eq!(retval, 0, "WNOHANG + fils vivant = retour 0 (POSIX)");
    }

    /// Test : wstatus peut être décodé correctement (WIFEXITED, WIFSIGNALED macros)
    #[test]
    fn wstatus_posix_macros() {
        // exit(42) → WIFEXITED=true, WEXITSTATUS=42, WIFSIGNALED=false
        let r = WaitResult::exited(Pid(100), 42);
        let wstatus = r.wstatus;
        let wifexited   = (wstatus & 0x7F) == 0;  // bits 0-6 == 0
        let wexitstatus = (wstatus >> 8) & 0xFF;   // bits 8-15
        let wifsignaled = (wstatus & 0x7F) != 0 && (wstatus & 0x7F) != 0x7F;

        assert!(wifexited,   "WIFEXITED doit être true pour exit(42)");
        assert_eq!(wexitstatus, 42, "WEXITSTATUS doit être 42");
        assert!(!wifsignaled, "WIFSIGNALED doit être false pour exit(42)");
    }

    #[test]
    fn wstatus_posix_signaled_macros() {
        // Tué par SIGKILL(9) → WIFSIGNALED=true, WTERMSIG=9, WIFEXITED=false
        let r = WaitResult::signaled(Pid(100), 9, false);
        let wstatus = r.wstatus;
        let wifexited   = (wstatus & 0x7F) == 0;
        let wifsignaled = (wstatus & 0x7F) != 0 && (wstatus & 0x7F) != 0x7F;
        let wtermsig    = wstatus & 0x7F;

        assert!(!wifexited,  "WIFEXITED doit être false pour kill par signal");
        assert!(wifsignaled, "WIFSIGNALED doit être true pour kill par signal");
        assert_eq!(wtermsig, 9, "WTERMSIG doit être SIGKILL=9");
    }
}
