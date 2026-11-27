//! Patina GUI built with Slint and Material 3.
//!
//! All GUI code lives in this library so the desktop binary and the Android
//! `cdylib` share one entry point. `patina-core` does the work; the controller
//! translates between its views and the Slint data contract.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use patina_core::store::Store;
use slint::ComponentHandle as _;

slint::include_modules!();

mod controller;
mod convert;
mod language;
mod notify;
mod storage;
#[cfg(test)]
mod tests;
mod wire;

use controller::Controller;

/// Seconds between background refreshes (idle prompts, break reminders, the clock).
const REFRESH_SECS: u64 = 30;

/// Creates the window, wires it to the data under `store`, and runs the event loop.
fn launch(
    store: Store,
    root: std::path::PathBuf,
    storage: StorageUi,
    prefs_dir: std::path::PathBuf,
) -> anyhow::Result<()> {
    let window = MainWindow::new()?;
    #[cfg(target_os = "android")]
    {
        // Touch screens have no hover; Material state layers would otherwise stick.
        MaterialWindowAdapter::get(&window).set_disable_hover(true);
    }
    let ctl = Rc::new(RefCell::new(Controller::new(
        store,
        root,
        window.as_weak(),
        storage,
        prefs_dir,
    )));
    wire::connect(&window, ctl.clone());
    ctl.borrow().apply_language();
    ctl.borrow_mut().refresh();
    let timer = slint::Timer::default();
    let tick = ctl.clone();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_secs(REFRESH_SECS),
        move || tick.borrow_mut().refresh(),
    );
    window.run()?;
    Ok(())
}

/// Runs the GUI event loop until the main window closes.
///
/// # Errors
///
/// Returns an error if the data location cannot be determined or the Slint platform
/// cannot create or run the window.
pub fn run() -> anyhow::Result<()> {
    let root = patina_core::store::default_root()?;
    let store = Store::new(root.clone(), patina_core::store::device_id()?);
    let prefs = patina_core::store::device_dir()?;
    launch(store, root, StorageUi::Desktop, prefs)
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: slint::android::AndroidApp) {
    let internal = app
        .internal_data_path()
        .expect("the app has no internal data directory");
    slint::android::init(app).expect("failed to initialize the Slint Android backend");
    // The device id stays in the private directory so a sync tool never copies it.
    let device = patina_core::store::device_id_in(&internal).expect("cannot read the device id");
    let (root, shared) = storage::choose_root(
        &internal,
        std::path::Path::new("/storage/emulated/0/Documents/Patina"),
        device,
    );
    let kind = if shared {
        StorageUi::Shared
    } else {
        StorageUi::Private
    };
    launch(Store::new(root.clone(), device), root, kind, internal).expect("the GUI failed");
}
