/* exo_syscall_numbers.h - Exo-OS Syscall Number Definitions
 *
 * Defines syscall numbers for Exo-OS and provides mapping from
 * Linux syscall numbers to Exo-OS equivalents.
 */

#ifndef _EXO_SYSCALL_NUMBERS_H
#define _EXO_SYSCALL_NUMBERS_H

/* ============================================================================
 * Exo-OS Native Syscall Numbers
 * ============================================================================ */

/* Process Management */
#define SYS_exo_exit        1
#define SYS_exo_spawn       2
#define SYS_exo_getpid      3
#define SYS_exo_gettid      4

/* I/O Operations */
#define SYS_exo_open        10
#define SYS_exo_close       11
#define SYS_exo_read        12
#define SYS_exo_write       13
#define SYS_exo_lseek       14

/* Memory Management */
#define SYS_exo_mmap        20
#define SYS_exo_munmap      21
#define SYS_exo_mprotect    22
#define SYS_exo_brk         23

/* IPC */
#define SYS_exo_send_msg    30
#define SYS_exo_recv_msg    31

/* Time */
#define SYS_exo_clock_gettime  40
#define SYS_exo_nanosleep      41

/* Process Control (Legacy Path) */
#define SYS_exo_fork        50
#define SYS_exo_execve      51
#define SYS_exo_wait4       52

/* ============================================================================
 * Linux to Exo-OS Syscall Mapping
 * (These definitions allow unmodified musl code to work)
 * ============================================================================ */

/* Direct mappings */
#define SYS_read        SYS_exo_read
#define SYS_write       SYS_exo_write
#define SYS_open        SYS_exo_open
#define SYS_close       SYS_exo_close
#define SYS_lseek       SYS_exo_lseek

#define SYS_mmap        SYS_exo_mmap
#define SYS_munmap      SYS_exo_munmap
#define SYS_mprotect    SYS_exo_mprotect
#define SYS_brk         SYS_exo_brk

#define SYS_getpid      SYS_exo_getpid
#define SYS_gettid      SYS_exo_gettid
#define SYS_exit        SYS_exo_exit

#define SYS_fork        SYS_exo_fork
#define SYS_execve      SYS_exo_execve
#define SYS_wait4       SYS_exo_wait4

#define SYS_clock_gettime  SYS_exo_clock_gettime
#define SYS_nanosleep      SYS_exo_nanosleep

/* ============================================================================
 * Linux Syscalls Without Direct Exo-OS Equivalent (return -ENOSYS)
 * ============================================================================ */

/* These will need emulation or may not be supported */
#define SYS_clone       (-1)  /* Use fork + flags emulation */
#define SYS_vfork       (-1)  /* Use fork emulation */
#define SYS_ptrace      (-1)  /* Not supported - use debugger API */

#endif /* _EXO_SYSCALL_NUMBERS_H */
