//! Windowed context switch interface (stub initial)

#![allow(dead_code)]

use crate::arch::x86_64::context::Context;

pub fn init() {
    // Placeholder
}

pub fn windowed_context_switch(_old: &Context, _new: &Context) {
    // TODO: Implement optimized windowed context switch
    unsafe {
        // crate::arch::context::switch(_old, _new);
    }
}

pub fn windowed_context_switch_to(_new: &Context) {
    // TODO: Implement optimized windowed context switch to
    unsafe {
        // crate::arch::context::switch_to(_new);
    }
}
