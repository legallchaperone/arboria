//! The macOS-only pieces the core links on macOS (ADR 0004): Touch ID through
//! LocalAuthentication. On other systems the crate is empty but for [`authenticate`]
//! refusing, so the core compiles and tests everywhere.
//!
//! Invariants:
//! - This crate only asks the system whether the device owner is present; it records
//!   nothing and approves nothing. `arboria_core::human::confirm_on_device` is the only
//!   caller, and the only place a [`authenticate`] success becomes a `Confirmed`.
//! - The policy is `LAPolicyDeviceOwnerAuthentication`: Touch ID, or the account password
//!   when Touch ID is unavailable or locked out (never a silent success).
//! - Blocking: the reply comes on a system queue and is handed back through a channel;
//!   call it off the UI thread. A prompt left unanswered is cancelled after [`TIMEOUT`].

#![cfg_attr(not(target_os = "macos"), forbid(unsafe_code))]

use std::time::Duration;

/// How long the system prompt may stay open.
pub const TIMEOUT: Duration = Duration::from_secs(180);

/// Ask the device owner to confirm with Touch ID (or their password); `reason` is shown
/// in the system prompt. Err: why it did not pass (cancelled, failed, unavailable).
#[cfg(target_os = "macos")]
pub fn authenticate(reason: &str) -> Result<(), String> {
    use std::sync::mpsc;

    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSError, NSString};
    use objc2_local_authentication::{LAContext, LAPolicy};

    let policy = LAPolicy::DeviceOwnerAuthentication;
    // SAFETY: `+[LAContext new]` has no preconditions.
    let ctx = unsafe { LAContext::new() };
    // SAFETY: a valid context and policy; the error out-parameter is handled by objc2.
    if let Err(e) = unsafe { ctx.canEvaluatePolicy_error(policy) } {
        return Err(e.localizedDescription().to_string());
    }
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let reply = RcBlock::new(move |ok: Bool, err: *mut NSError| {
        let result = if ok.as_bool() {
            Ok(())
        } else {
            // SAFETY: the system passes a valid NSError (or nil) for the block's duration.
            let why = unsafe { err.as_ref() }
                .map(|e| e.localizedDescription().to_string())
                .unwrap_or_else(|| "not confirmed".to_string());
            Err(why)
        };
        let _ = tx.send(result);
    });
    // SAFETY: the block only sends on a channel (Send), as the API requires of `reply`.
    unsafe {
        ctx.evaluatePolicy_localizedReason_reply(policy, &NSString::from_str(reason), &reply);
    }
    match rx.recv_timeout(TIMEOUT) {
        Ok(r) => r,
        Err(_) => {
            // SAFETY: invalidating a live context ends the pending prompt.
            unsafe { ctx.invalidate() };
            Err("timed out waiting for Touch ID".to_string())
        }
    }
}

/// Whether this Mac can confirm the device owner (Touch ID or the account password):
/// asks the system only, shows nothing.
#[cfg(target_os = "macos")]
pub fn available() -> Result<(), String> {
    use objc2_local_authentication::{LAContext, LAPolicy};
    // SAFETY: `+[LAContext new]` has no preconditions.
    let ctx = unsafe { LAContext::new() };
    // SAFETY: a valid context and policy; the error out-parameter is handled by objc2.
    unsafe { ctx.canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthentication) }
        .map_err(|e| e.localizedDescription().to_string())
}

/// Not macOS: never.
#[cfg(not(target_os = "macos"))]
pub fn available() -> Result<(), String> {
    Err("Touch ID needs macOS".to_string())
}

/// Not macOS: always refused.
#[cfg(not(target_os = "macos"))]
pub fn authenticate(_reason: &str) -> Result<(), String> {
    Err("Touch ID needs macOS".to_string())
}

#[cfg(test)]
mod tests {
    /// On a Mac (CI's macos runners too): the LocalAuthentication query runs and answers
    /// without prompting (a VM usually has no Touch ID: then it says why).
    #[cfg(target_os = "macos")]
    #[test]
    fn asks_the_system_without_prompting() {
        match super::available() {
            Ok(()) => println!("device owner authentication available"),
            Err(why) => {
                println!("not available: {why}");
                assert!(!why.is_empty());
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn refuses_off_macos() {
        assert_eq!(
            super::authenticate("批准 run r001").unwrap_err(),
            "Touch ID needs macOS"
        );
    }
}
