//! OS notifications (desktop only). Text is English: Slint's `@tr` does not reach here.

/// Shows a desktop notification; failures are logged, never raised.
#[cfg(not(target_os = "android"))]
pub fn desktop(summary: &str, body: &str) {
    if let Err(e) = notify_rust::Notification::new()
        .summary(summary)
        .body(body)
        .show()
    {
        eprintln!("notification failed: {e}");
    }
}

/// Android has no desktop notification service here.
#[cfg(target_os = "android")]
pub fn desktop(_summary: &str, _body: &str) {}
