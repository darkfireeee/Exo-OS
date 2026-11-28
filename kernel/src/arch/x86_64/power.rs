//! # arch/x86_64/power.rs — Real hardware & hypervisor ACPI S5 power-off and CPU reset.

use super::{acpi, disable_interrupts, halt_cpu, inb, io_delay, outb, outw};

/// Real hardware/hypervisor ACPI S5 power-off (FADT PM1a_CNT + QEMU Q35/i440fx + VirtualBox).
#[inline(never)]
pub fn arch_poweroff() -> ! {
    disable_interrupts();
    let fadt_pm1a = acpi::pm_timer::acpi_pm1a_ctl_port();
    // SAFETY: Ring 0 ACPI PM1a_CNT S5 power-off I/O writes on validated chipset ports.
    unsafe {
        if fadt_pm1a != 0 {
            outw(fadt_pm1a, 0x2000); // SLP_EN | SLP_TYP=0 (QEMU Q35/PIIX4 S5)
            io_delay();
            outw(fadt_pm1a, 0x3400); // SLP_EN | SLP_TYP=5 (VirtualBox / ACPI S5)
            io_delay();
        }
        outw(0x0604, 0x2000); // QEMU Q35 ICH9 PM1a_CNT S5
        io_delay();
        outw(0xB004, 0x2000); // QEMU i440fx / Bochs S5
        io_delay();
        outw(0x4004, 0x3400); // VirtualBox ACPI S5
        io_delay();
    }
    halt_cpu()
}

/// Real hardware/hypervisor CPU reset (PCI 0xCF9 Q35 ICH9 -> i8042 0x64/0xFE -> triple-fault).
#[inline(never)]
pub fn arch_reboot() -> ! {
    disable_interrupts();
    // SAFETY: Ring 0 standard x86_64 chipset reset sequence (0xCF9 -> i8042 -> null-IDT triple fault).
    unsafe {
        // 1. PCI Reset Control Register (0xCF9 — Intel ICH9 / Q35 / modern x86_64 chipsets)
        outb(0x0CF9, 0x02);
        io_delay();
        outb(0x0CF9, 0x06); // SYS_RST + RST_CPU (hard reset)
        io_delay();
        outb(0x0CF9, 0x0E); // FULL_RST + SYS_RST + RST_CPU
        io_delay();

        // 2. Legacy PS/2 i8042 controller pulse reset line
        for _ in 0..4096 {
            let st = inb(0x0064);
            if (st & 0x01) != 0 {
                let _ = inb(0x0060);
            }
            if (st & 0x02) == 0 {
                break;
            }
            io_delay();
        }
        outb(0x0064, 0xFE);
        io_delay();

        // 3. Architectural fallback: load empty IDT and trigger triple fault -> platform reset
        let null_idtr: [u8; 10] = [0; 10];
        core::arch::asm!(
            "lidt [{idtr}]",
            "int3",
            idtr = in(reg) null_idtr.as_ptr(),
            options(nostack)
        );
    }
    halt_cpu()
}
