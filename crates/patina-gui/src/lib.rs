//! Patina GUI built with Slint and Material 3.
//!
//! All GUI code lives in this library so the desktop binary and the Android
//! `cdylib` share one entry point.

slint::include_modules!();

fn main_window() -> Result<MainWindow, slint::PlatformError> {
    MainWindow::new()
}

/// Runs the GUI event loop until the main window closes.
///
/// # Errors
///
/// Returns an error if the Slint platform cannot create or run the window.
pub fn run() -> Result<(), slint::PlatformError> {
    main_window()?.run()
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: slint::android::AndroidApp) {
    slint::android::init(app).expect("failed to initialize the Slint Android backend");
    let window = main_window().expect("failed to create the main window");
    // Touch screens have no hover; Material state layers would otherwise stick.
    MaterialWindowAdapter::get(&window).set_disable_hover(true);
    window.run().expect("the Slint event loop failed");
}
