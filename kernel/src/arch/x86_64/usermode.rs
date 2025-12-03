//! User Mode Transition
//! 
//! Handles the transition from kernel mode (Ring 0) to user mode (Ring 3).
//! This is critical for running userspace processes.

use crate::arch::x86_64::gdt::{USER_CODE_SELECTOR, USER_DATA_SELECTOR};
use crate::memory::VirtualAddress;

/// CPU state for transitioning to user mode
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct UserContext {
    // General purpose registers (saved in order of pushes)
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    
    // Interrupt frame (pushed by CPU on interrupt)
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

impl UserContext {
    /// Create a new user context for process entry
    pub fn new(entry_point: VirtualAddress, stack_pointer: VirtualAddress) -> Self {
        Self {
            // Clear all general purpose registers
            rax: 0,
            rbx: 0,
            rcx: 0,
            rdx: 0,
            rsi: 0,
            rdi: 0,
            rbp: 0,
            r8: 0,
            r9: 0,
            r10: 0,
            r11: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            
            // Entry point
            rip: entry_point.as_u64(),
            
            // User code segment with RPL=3
            cs: (USER_CODE_SELECTOR | 3) as u64,
            
            // Enable interrupts (IF=1), clear direction flag
            rflags: 0x202,
            
            // User stack
            rsp: stack_pointer.as_u64(),
            
            // User data segment with RPL=3
            ss: (USER_DATA_SELECTOR | 3) as u64,
        }
    }
    
    /// Set arguments for _start (argc, argv, envp in registers)
    pub fn set_args(&mut self, argc: u64, argv: u64, envp: u64) {
        self.rdi = argc;  // First argument
        self.rsi = argv;  // Second argument
        self.rdx = envp;  // Third argument
    }
    
    /// Set auxiliary vector pointer
    pub fn set_auxv(&mut self, auxv: u64) {
        self.rcx = auxv;  // Fourth argument
    }
}

/// Jump to user mode
/// 
/// This function never returns - it performs an IRETQ to user mode.
/// 
/// # Safety
/// 
/// The caller must ensure:
/// - `context` points to a valid UserContext on the kernel stack
/// - User page tables are properly set up
/// - TSS RSP0 is set to a valid kernel stack
#[inline(never)]
pub unsafe fn jump_to_usermode(context: &UserContext) -> ! {
    // The context is laid out to match what IRETQ expects
    // We need to:
    // 1. Set up the stack with the interrupt frame
    // 2. Restore general purpose registers
    // 3. Execute IRETQ
    
    core::arch::asm!(
        // Load the context address into RSP
        // The context struct has GP regs followed by interrupt frame
        
        // Restore general purpose registers
        "mov r15, [rdi + 0]",
        "mov r14, [rdi + 8]",
        "mov r13, [rdi + 16]",
        "mov r12, [rdi + 24]",
        "mov r11, [rdi + 32]",
        "mov r10, [rdi + 40]",
        "mov r9,  [rdi + 48]",
        "mov r8,  [rdi + 56]",
        "mov rbp, [rdi + 64]",
        // rdi will be loaded last
        "mov rsi, [rdi + 80]",
        "mov rdx, [rdi + 88]",
        "mov rcx, [rdi + 96]",
        "mov rbx, [rdi + 104]",
        "mov rax, [rdi + 112]",
        
        // Set up stack for IRETQ
        // Push: SS, RSP, RFLAGS, CS, RIP
        "push qword ptr [rdi + 152]",  // SS
        "push qword ptr [rdi + 144]",  // RSP
        "push qword ptr [rdi + 136]",  // RFLAGS
        "push qword ptr [rdi + 128]",  // CS
        "push qword ptr [rdi + 120]",  // RIP
        
        // Load RDI last (it was our context pointer)
        "mov rdi, [rdi + 72]",
        
        // IRETQ pops: RIP, CS, RFLAGS, RSP, SS
        // This transitions us to Ring 3
        "iretq",
        
        in("rdi") context as *const UserContext,
        options(noreturn)
    );
}

/// Return to user mode from syscall (SYSRET path)
/// 
/// Faster than IRETQ but with restrictions:
/// - RCX must contain RIP
/// - R11 must contain RFLAGS
/// 
/// # Safety
/// 
/// Same requirements as jump_to_usermode, plus:
/// - Must be called from syscall handler (SYSCALL was used to enter)
#[inline(never)]
pub unsafe fn sysret_to_usermode(
    rip: u64,
    rsp: u64,
    rflags: u64,
    rax: u64,  // Return value
) -> ! {
    core::arch::asm!(
        // Set up for SYSRET
        // RCX = return RIP
        // R11 = return RFLAGS
        "mov rcx, {rip}",
        "mov r11, {rflags}",
        "mov rsp, {rsp}",
        "mov rax, {rax}",
        
        // Clear other registers for security
        "xor rbx, rbx",
        "xor rdx, rdx",
        "xor rsi, rsi",
        "xor rdi, rdi",
        "xor rbp, rbp",
        "xor r8, r8",
        "xor r9, r9",
        "xor r10, r10",
        "xor r12, r12",
        "xor r13, r13",
        "xor r14, r14",
        "xor r15, r15",
        
        // SYSRET to Ring 3
        // This sets:
        // - RIP = RCX
        // - RFLAGS = R11
        // - CS = IA32_STAR[48:63] + 16 (user code)
        // - SS = IA32_STAR[48:63] + 8 (user data)
        "sysretq",
        
        rip = in(reg) rip,
        rsp = in(reg) rsp,
        rflags = in(reg) rflags,
        rax = in(reg) rax,
        options(noreturn)
    );
}

/// Set up TSS for kernel stack on privilege transitions
pub fn setup_tss_rsp0(kernel_stack: VirtualAddress) {
    use crate::arch::x86_64::tss;
    
    unsafe {
        // Set RSP0 - the stack used when transitioning Ring 3 -> Ring 0
        tss::set_rsp0(kernel_stack.as_u64());
    }
}

/// Validate that user mode is properly configured
pub fn validate_usermode_config() -> Result<(), &'static str> {
    // Check GDT selectors
    if USER_CODE_SELECTOR == 0 || USER_DATA_SELECTOR == 0 {
        return Err("GDT user segments not configured");
    }
    
    // Check SYSCALL/SYSRET MSRs
    // (Assumes init was called)
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test_case]
    fn test_user_context_creation() {
        let ctx = UserContext::new(
            VirtualAddress::new(0x400000),
            VirtualAddress::new(0x7FFFFFFFE000),
        );
        
        assert_eq!(ctx.rip, 0x400000);
        assert_eq!(ctx.rsp, 0x7FFFFFFFE000);
        assert_eq!(ctx.cs, (USER_CODE_SELECTOR | 3) as u64);
        assert_eq!(ctx.ss, (USER_DATA_SELECTOR | 3) as u64);
        assert_eq!(ctx.rflags & 0x200, 0x200); // IF set
    }
}
