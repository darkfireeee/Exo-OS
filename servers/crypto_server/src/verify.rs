//! verify.rs — Contexte et streaming de vérification Ed25519 en verify_strict

use ed25519_dalek::{Signature, VerifyingKey};

pub const VERIFY_CONTEXTS: usize = 4;
pub const VERIFY_MAX_MESSAGE: usize = 4096;

pub const VERIFY_OP_BEGIN: u8 = 0;
pub const VERIFY_OP_UPDATE: u8 = 1;
pub const VERIFY_OP_FINAL: u8 = 2;

#[derive(Clone, Copy)]
pub struct VerifyContext {
    pub in_use: bool,
    pub owner_principal: u64,
    pub public_key: [u8; 32],
    pub signature: [u8; 64],
    pub total_len: u16,
    pub received_len: u16,
    pub message: [u8; VERIFY_MAX_MESSAGE],
}

impl VerifyContext {
    pub const fn empty() -> Self {
        Self {
            in_use: false,
            owner_principal: 0,
            public_key: [0u8; 32],
            signature: [0u8; 64],
            total_len: 0,
            received_len: 0,
            message: [0u8; VERIFY_MAX_MESSAGE],
        }
    }
}

static VERIFY_TABLE: spin::Mutex<[VerifyContext; VERIFY_CONTEXTS]> =
    spin::Mutex::new([VerifyContext::empty(); VERIFY_CONTEXTS]);

pub fn reset_verify_context(ctx: &mut VerifyContext) {
    *ctx = VerifyContext::empty();
}

pub fn alloc_verify_context(
    owner_principal: u64,
    public_key: [u8; 32],
    signature: [u8; 64],
    total_len: usize,
) -> u32 {
    if total_len == 0 || total_len > VERIFY_MAX_MESSAGE {
        return 0;
    }

    let mut table = VERIFY_TABLE.lock();
    for (idx, ctx) in table.iter_mut().enumerate() {
        if ctx.in_use {
            continue;
        }
        *ctx = VerifyContext {
            in_use: true,
            owner_principal,
            public_key,
            signature,
            total_len: total_len as u16,
            received_len: 0,
            message: [0u8; VERIFY_MAX_MESSAGE],
        };
        return (idx + 1) as u32;
    }
    0
}

pub fn append_verify_context(ctx_handle: u32, owner_principal: u64, chunk: &[u8]) -> bool {
    if ctx_handle == 0 {
        return false;
    }

    let idx = (ctx_handle - 1) as usize;
    if idx >= VERIFY_CONTEXTS {
        return false;
    }

    let mut table = VERIFY_TABLE.lock();
    let ctx = &mut table[idx];
    if !ctx.in_use || ctx.owner_principal != owner_principal {
        return false;
    }

    let start = ctx.received_len as usize;
    let end = start.saturating_add(chunk.len());
    if end > ctx.total_len as usize || end > VERIFY_MAX_MESSAGE {
        return false;
    }

    ctx.message[start..end].copy_from_slice(chunk);
    ctx.received_len = end as u16;
    true
}

pub fn finalize_verify_context(ctx_handle: u32, owner_principal: u64) -> Option<bool> {
    if ctx_handle == 0 {
        return None;
    }

    let idx = (ctx_handle - 1) as usize;
    if idx >= VERIFY_CONTEXTS {
        return None;
    }

    let mut table = VERIFY_TABLE.lock();
    let ctx = &mut table[idx];
    if !ctx.in_use || ctx.owner_principal != owner_principal {
        return None;
    }

    if ctx.received_len != ctx.total_len {
        reset_verify_context(ctx);
        return Some(false);
    }

    let verify_result = VerifyingKey::from_bytes(&ctx.public_key)
        .ok()
        .map(|vk| {
            let sig = Signature::from_bytes(&ctx.signature);
            // FIX-DEEP-CRYPTO : verify_strict (PAS verify) — rejette les clés
            // faibles (low-order) et la malléabilité de signature.
            vk.verify_strict(&ctx.message[..ctx.total_len as usize], &sig)
                .is_ok()
        })
        .unwrap_or(false);

    reset_verify_context(ctx);
    Some(verify_result)
}
