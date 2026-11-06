//! # syscall/table/ipc/security_ops.rs — Handlers for Capabilities, Pledge, and ExoShield Drain

use crate::syscall::table::common::*;
use crate::syscall::numbers::*;
use crate::syscall::validation::{copy_from_user, copy_to_user, UserBuf};

/// `exo_cap_create(type, rights, target_pid, token_out_ptr)` -> handle or errno.
pub fn sys_exo_cap_create(
    cap_type: u64,
    rights: u64,
    target: u64,
    token_out_ptr: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_CAP_CREATE);
    if token_out_ptr == 0 {
        return EFAULT;
    }

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EACCES;
    }

    let cap_type = match checked_u32_sysarg(cap_type) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let rights = match checked_u32_sysarg(rights) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let target = match checked_u32_sysarg(target) {
        Ok(v) => v,
        Err(e) => return e,
    };

    match crate::security::capability::create(cap_type, rights, target, caller_pid) {
        Ok(token) => {
            let token_bytes = token.to_bytes();
            if copy_to_user(
                token_out_ptr as *mut u8,
                token_bytes.as_ptr(),
                crate::security::capability::CAP_TOKEN_WIRE_SIZE,
            )
            .is_err()
            {
                return EFAULT;
            }
            token.object_id().as_u64() as i64
        }
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `exo_cap_revoke(handle)`.
pub fn sys_exo_cap_revoke(handle: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXO_CAP_REVOKE);
    let handle = match checked_u32_sysarg(handle) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();

    match crate::security::capability::revoke_handle_by_caller(caller_pid, handle) {
        Ok(_) => 0,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `exo_cap_check(token_ptr, rights, target_pid, expected_type)` -> 0 or errno.
pub fn sys_exo_cap_check(
    token_ptr: u64,
    required_rights: u64,
    target_pid: u64,
    expected_type: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_CAP_CHECK);
    if token_ptr == 0 {
        return EFAULT;
    }

    let mut token_bytes = [0u8; crate::security::capability::CAP_TOKEN_WIRE_SIZE];
    if copy_from_user(
        token_bytes.as_mut_ptr(),
        token_ptr as *const u8,
        token_bytes.len(),
    )
    .is_err()
    {
        return EFAULT;
    }

    let token = match crate::security::capability::CapToken::from_bytes(&token_bytes) {
        Some(token) => token,
        None => return EINVAL,
    };

    let required_rights = match checked_u32_sysarg(required_rights) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let target_pid = match checked_u32_sysarg(target_pid) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 || caller_pid != target_pid {
        return EACCES;
    }
    let expected_type = match checked_u32_sysarg(expected_type) {
        Ok(v) => v,
        Err(e) => return e,
    };

    match crate::security::capability::check_token(
        token,
        required_rights,
        target_pid,
        expected_type,
    ) {
        Ok(_) => 0,
        Err(e) => e.to_kernel_errno() as i64,
    }
}

/// `exo_pledge(promises, flags, ...)` — **irreversibly** restricts the current
/// process (OpenBSD pledge()-style sandbox, TIER 2.10). `promises` = bitmask of
/// `pledge_flags`; unpromised capabilities become zero-trust restrictions
/// enforced at the syscall boundary (see `syscall_restriction_mask` / TIER 1.1).
/// **Monotonic**: only hardens (RULE PLEDGE-01) — re-pledging adds
/// restrictions, never removes any. Init (PID 1) cannot pledge (PLEDGE-03).
///
/// Returns: `0` if applied (or nothing to restrict), `EPERM` if init / PID out of range.
pub fn sys_exo_pledge(promises: u64, _flags: u64, _a3: u64, _a4: u64, _a5: u64, _a6: u64) -> i64 {
    stat_inc(SYS_EXO_PLEDGE);
    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EPERM;
    }
    let restrictions = crate::security::zero_trust::pledge_promises_to_restrictions(promises);
    if restrictions == 0 {
        // All enforced capabilities are promised -> nothing to restrict (success).
        return 0;
    }
    // restrict_process is monotonic and rejects init (PID 1) / PID out of range.
    if crate::security::zero_trust::restrict_process(caller_pid, restrictions) {
        0
    } else {
        EPERM
    }
}

/// `exo_shield_drain(buf_ptr, count, ...)` — drains the kernel->exo_shield security
/// event feed (TIER 3.1). **Reserved for the exo_shield server** (gated on its
/// service class) to prevent event leaks to an arbitrary process.
/// Copies at most `min(count, 64)` `ShieldEvent` into `buf_ptr`;
/// returns the drained count (>= 0) or a negative errno.
pub fn sys_exo_shield_drain(
    buf_ptr: u64,
    count: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    _a6: u64,
) -> i64 {
    stat_inc(SYS_EXO_SHIELD_DRAIN);
    use crate::security::shield_feed::{ShieldEvent, SHIELD_EVENT_SIZE};

    let caller_pid = crate::syscall::fast_path::syscall_current_pid();
    if caller_pid == 0 {
        return EPERM;
    }
    // Only exo_shield drains the feed.
    if crate::security::ipc_policy::service_class_of(crate::process::core::pid::Pid(caller_pid))
        != crate::security::ipc_policy::ServiceClass::ExoShield
    {
        return EACCES;
    }

    let count = count as usize;
    if count == 0 {
        return 0;
    }
    // At most 64 events / call (~2.5 KiB) — bounds copy + lock.
    let n = count.min(64);
    let bytes = n * SHIELD_EVENT_SIZE;
    if UserBuf::validate(buf_ptr, bytes, 64 * SHIELD_EVENT_SIZE).is_err() {
        return EFAULT;
    }

    let mut events = [ShieldEvent::zero(); 64];
    let drained = crate::security::shield_feed::drain(&mut events[..n]);
    if drained == 0 {
        return 0;
    }
    let copy_bytes = drained * SHIELD_EVENT_SIZE;
    if copy_to_user(buf_ptr as *mut u8, events.as_ptr() as *const u8, copy_bytes).is_err() {
        return EFAULT;
    }
    drained as i64
}

#[cfg(test)]
mod capability_syscall_arg_tests {
    use super::*;

    #[test]
    fn checked_u32_sysarg_accepts_full_u32_range() {
        assert_eq!(checked_u32_sysarg(0), Ok(0));
        assert_eq!(checked_u32_sysarg(u32::MAX as u64), Ok(u32::MAX));
    }

    #[test]
    fn checked_u32_sysarg_rejects_high_bits_in_syscall_abi() {
        assert_eq!(checked_u32_sysarg(u32::MAX as u64 + 1), Err(EINVAL));
    }

    #[test]
    fn checked_i32_sysarg_accepts_sign_extended_negative_values() {
        assert_eq!(checked_i32_sysarg(u64::MAX), Ok(-1));
        assert_eq!(checked_i32_sysarg(i32::MIN as i64 as u64), Ok(i32::MIN));
        assert_eq!(checked_i32_sysarg(i32::MAX as u64), Ok(i32::MAX));
    }

    #[test]
    fn checked_i32_sysarg_rejects_non_canonical_positive_high_bits() {
        assert_eq!(checked_i32_sysarg(i32::MAX as u64 + 1), Err(EINVAL));
        assert_eq!(checked_i32_sysarg(0x8000_0000_0000_0000), Err(EINVAL));
    }

    #[test]
    fn checked_usize_sysarg_accepts_native_width() {
        assert_eq!(checked_usize_sysarg(0), Ok(0));
        assert_eq!(checked_usize_sysarg(usize::MAX as u64), Ok(usize::MAX));
    }
}
