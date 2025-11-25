//! CPU Context for x86_64
//! 
//! Windowed context switch - only 4 registers!

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Context {
    pub rsp: u64,
    pub rip: u64,
    pub cr3: u64,
    pub rflags: u64,
}

impl Context {
    pub const fn new() -> Self {
        Context {
            rsp: 0,
            rip: 0,
            cr3: 0,
            rflags: 0x200, // IF set
        }
    }

    pub fn with_entry(entry: u64, stack: u64, page_table: u64) -> Self {
        Context {
            rsp: stack,
            rip: entry,
            cr3: page_table,
            rflags: 0x200,
        }
    }
}

/// Save current context
#[inline(never)]
pub unsafe fn save_context(ctx: &mut Context) {
    // TODO: MSVC/LLVM has issues with labels in inline asm (causes .Ltmp0 error)
    // Simplified version without forward label
    core::arch::asm!(
        "mov [{ctx} + 0], rsp",
        // Store return address from stack
        "mov rax, [rsp]",
        "mov [{ctx} + 8], rax",
        "mov rax, cr3",
        "mov [{ctx} + 16], rax",
        "pushfq",
        "pop QWORD PTR [{ctx} + 24]",
        ctx = in(reg) ctx,
        out("rax") _,
    );
}

/// Restore context
#[inline(never)]
pub unsafe fn restore_context(ctx: &Context) -> ! {
    core::arch::asm!(
        "mov rsp, [{ctx} + 0]",
        "mov rax, [{ctx} + 16]",
        "mov cr3, rax",
        "push QWORD PTR [{ctx} + 24]",
        "popfq",
        "jmp QWORD PTR [{ctx} + 8]",
        ctx = in(reg) ctx,
        options(noreturn)
    );
}
