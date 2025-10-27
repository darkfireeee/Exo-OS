//! Point d'entrée binaire du kernel Exo-OS
//! Entry point pour Multiboot2

#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]


use core::panic::PanicInfo;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
	// Affiche le panic sur le port série si possible
	// (On ne peut pas garantir que drivers::serial est initialisé ici)
	loop {
		unsafe { core::arch::asm!("hlt") };
	}
}
