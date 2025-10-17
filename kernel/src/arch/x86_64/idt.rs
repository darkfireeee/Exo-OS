// src/arch/x86_64/idt.rs
// Interrupt Descriptor Table

use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use crate::println;
use lazy_static::lazy_static;

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        
        // Configuration des handlers d'exceptions
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        idt.divide_error.set_handler_fn(divide_error_handler);
        idt.debug.set_handler_fn(debug_handler);
        idt.non_maskable_interrupt.set_handler_fn(nmi_handler);
        idt.overflow.set_handler_fn(overflow_handler);
        idt.bound_range_exceeded.set_handler_fn(bound_range_exceeded_handler);
        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
        idt.device_not_available.set_handler_fn(device_not_available_handler);
        
        // Double fault
        unsafe {
            idt.double_fault.set_handler_fn(double_fault_handler)
                .set_stack_index(crate::arch::x86_64::gdt::DOUBLE_FAULT_IST_INDEX);
        }
        
        // Page fault
        idt.page_fault.set_handler_fn(page_fault_handler);
        
        // Général protection fault
        idt.general_protection_fault.set_handler_fn(general_protection_fault_handler);
        
        // Interruptions externes (IRQ)
        idt[0x20].set_handler_fn(timer_interrupt_handler);
        idt[0x21].set_handler_fn(keyboard_interrupt_handler);
        
        idt
    };
}

pub fn init() {
    IDT.load();
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: &mut InterruptStackFrame) {
    println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn divide_error_handler(stack_frame: &mut InterruptStackFrame) {
    println!("EXCEPTION: DIVIDE ERROR\n{:#?}", stack_frame);
    loop {}
}

extern "x86-interrupt" fn debug_handler(stack_frame: &mut InterruptStackFrame) {
    println!("EXCEPTION: DEBUG\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn nmi_handler(stack_frame: &mut InterruptStackFrame) {
    println!("EXCEPTION: NON-MASKABLE INTERRUPT\n{:#?}", stack_frame);
    loop {}
}

extern "x86-interrupt" fn overflow_handler(stack_frame: &mut InterruptStackFrame) {
    println!("EXCEPTION: OVERFLOW\n{:#?}", stack_frame);
    loop {}
}

extern "x86-interrupt" fn bound_range_exceeded_handler(stack_frame: &mut InterruptStackFrame) {
    println!("EXCEPTION: BOUND RANGE EXCEEDED\n{:#?}", stack_frame);
    loop {}
}

extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: &mut InterruptStackFrame) {
    println!("EXCEPTION: INVALID OPCODE\n{:#?}", stack_frame);
    loop {}
}

extern "x86-interrupt" fn device_not_available_handler(stack_frame: &mut InterruptStackFrame) {
    println!("EXCEPTION: DEVICE NOT AVAILABLE\n{:#?}", stack_frame);
    loop {}
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: &mut InterruptStackFrame,
    _error_code: u64,
) -> ! {
    println!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
    loop {}
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: &mut InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    println!("EXCEPTION: PAGE FAULT");
    println!("Address accessed: {:?}", x86_64::registers::control::Cr2::read());
    println!("Error code: {:?}", error_code);
    println!("{:#?}", stack_frame);
    loop {}
}

extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: &mut InterruptStackFrame,
    error_code: u64,
) {
    println!("EXCEPTION: GENERAL PROTECTION FAULT");
    println!("Error code: {:?}", error_code);
    println!("{:#?}", stack_frame);
    loop {}
}

extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: &mut InterruptStackFrame) {
    // Gérer l'interruption du timer
    println!("Timer interrupt");
    
    // Envoyer EOI (End Of Interrupt) au PIC
    unsafe {
        x86_64::instructions::port::outb(0x20, 0x20);
    }
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: &mut InterruptStackFrame) {
    // Gérer l'interruption du clavier
    println!("Keyboard interrupt");
    
    // Envoyer EOI (End Of Interrupt) au PIC
    unsafe {
        x86_64::instructions::port::outb(0x20, 0x20);
    }
}