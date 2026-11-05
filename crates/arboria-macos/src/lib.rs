//! The macOS-only pieces the core links on macOS (ADR 0004): Touch ID through
//! LocalAuthentication, and notifications through UserNotifications. On other systems the
//! crate is empty but for functions that refuse, so the core compiles and tests everywhere.
//!
//! Notifications are posted as the app bundle that runs them (Arboria.app's executable
//! with `--notify`): macOS shows a banner only for an app the human allowed in System
//! Settings › Notifications. (`osascript`'s `display notification` is filed under Script
//! Editor, which on recent macOS often has no permission and cannot ask for it: the
//! notification is recorded but never shown.)
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

/// The notification permission of the app bundle this process runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyStatus {
    /// Never asked: [`request_notifications`] shows the system prompt.
    NotDetermined,
    Denied,
    Authorized,
}

#[cfg(target_os = "macos")]
fn center(
) -> Result<objc2::rc::Retained<objc2_user_notifications::UNUserNotificationCenter>, String> {
    use objc2_foundation::NSBundle;
    // `currentNotificationCenter` raises outside an app bundle: check first
    if NSBundle::mainBundle().bundleIdentifier().is_none() {
        return Err("not inside an app bundle (notifications come from Arboria.app)".into());
    }
    Ok(objc2_user_notifications::UNUserNotificationCenter::currentNotificationCenter())
}

/// Whether this app may show notifications (asks the system, shows nothing).
#[cfg(target_os = "macos")]
pub fn notification_status() -> Result<NotifyStatus, String> {
    use std::ptr::NonNull;
    use std::sync::mpsc;

    use block2::RcBlock;
    use objc2_user_notifications::{UNAuthorizationStatus, UNNotificationSettings};

    let center = center()?;
    let (tx, rx) = mpsc::channel();
    let reply = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
        // SAFETY: the system passes a valid settings object for the block's duration.
        let st = unsafe { settings.as_ref() }.authorizationStatus();
        let _ = tx.send(st);
    });
    center.getNotificationSettingsWithCompletionHandler(&reply);
    let st = rx
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| "no answer from the notification system".to_string())?;
    Ok(if st == UNAuthorizationStatus::NotDetermined {
        NotifyStatus::NotDetermined
    } else if st == UNAuthorizationStatus::Denied {
        NotifyStatus::Denied
    } else {
        NotifyStatus::Authorized
    })
}

/// Ask for permission (the system prompt, the first time only); Ok(true): allowed.
#[cfg(target_os = "macos")]
pub fn request_notifications() -> Result<bool, String> {
    use std::sync::mpsc;

    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::NSError;
    use objc2_user_notifications::UNAuthorizationOptions;

    let center = center()?;
    let (tx, rx) = mpsc::channel();
    let reply = RcBlock::new(move |ok: Bool, err: *mut NSError| {
        // SAFETY: the system passes a valid NSError (or nil) for the block's duration.
        let why = unsafe { err.as_ref() }.map(|e| e.localizedDescription().to_string());
        let _ = tx.send((ok.as_bool(), why));
    });
    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
        &reply,
    );
    match rx.recv_timeout(TIMEOUT) {
        Ok((ok, None)) => Ok(ok),
        Ok((_, Some(why))) => Err(why),
        Err(_) => Err("nobody answered the notification prompt".into()),
    }
}

/// Show a notification as this app (asks for permission first if it never did).
#[cfg(target_os = "macos")]
pub fn notify(title: &str, body: &str) -> Result<(), String> {
    use std::sync::mpsc;

    use block2::RcBlock;
    use objc2_foundation::{NSError, NSString};
    use objc2_user_notifications::{UNMutableNotificationContent, UNNotificationRequest};

    match notification_status()? {
        NotifyStatus::Denied => {
            return Err(
                "notifications are off for Arboria (System Settings › Notifications)".into(),
            )
        }
        NotifyStatus::NotDetermined => {
            if !request_notifications()? {
                return Err("notifications were not allowed".into());
            }
        }
        NotifyStatus::Authorized => {}
    }
    let center = center()?;
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    let id = format!(
        "arboria-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(&id),
        &content,
        None,
    );
    let (tx, rx) = mpsc::channel();
    let done = RcBlock::new(move |err: *mut NSError| {
        // SAFETY: the system passes a valid NSError (or nil) for the block's duration.
        let why = unsafe { err.as_ref() }.map(|e| e.localizedDescription().to_string());
        let _ = tx.send(why);
    });
    center.addNotificationRequest_withCompletionHandler(&request, Some(&done));
    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(None) => Ok(()),
        Ok(Some(why)) => Err(why),
        Err(_) => Err("no answer from the notification system".into()),
    }
}

#[cfg(not(target_os = "macos"))]
pub fn notification_status() -> Result<NotifyStatus, String> {
    Err("needs macOS".into())
}

#[cfg(not(target_os = "macos"))]
pub fn request_notifications() -> Result<bool, String> {
    Err("needs macOS".into())
}

#[cfg(not(target_os = "macos"))]
pub fn notify(_title: &str, _body: &str) -> Result<(), String> {
    Err("needs macOS".into())
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

    /// A test binary is not an app bundle: refused, not a crash (`currentNotificationCenter`
    /// would raise).
    #[cfg(target_os = "macos")]
    #[test]
    fn notifications_need_the_app_bundle() {
        let e = super::notify("Arboria", "test").unwrap_err();
        assert!(e.contains("app bundle"), "{e}");
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
