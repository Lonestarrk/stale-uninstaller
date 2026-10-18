// Keep a console in debug builds so panics are visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use chrono::Utc;
use eframe::egui;
use stale_core::quarantine::{auto_purge, QuarantineOps};
use stale_core::settings::Settings;
use stale_gui::app::StaleApp;
use stale_gui::logging;
use stale_gui::strings::{self, t};

/// Name of the machine-wide mutex that marks a running Stale Uninstaller.
const INSTANCE_MUTEX: &str = r"Global\StaleUninstaller";

/// Takes the single-instance mutex; `false` when another Stale Uninstaller (any session) holds it.
/// The handle stays open until the process ends.
fn single_instance() -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;
    let name: Vec<u16> = INSTANCE_MUTEX.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `name` is NUL-terminated and outlives the call; the handle is deliberately leaked
    // so the mutex lives as long as the process.
    unsafe {
        match CreateMutexW(None, false, PCWSTR(name.as_ptr())) {
            Ok(_) => GetLastError() != ERROR_ALREADY_EXISTS,
            // Cannot tell (e.g. access denied to another session's mutex): assume another instance.
            Err(e) => {
                eprintln!("CreateMutexW: {e}");
                false
            }
        }
    }
}

/// `text` in a message box titled with the app's name; `error` picks the error icon.
fn message_box(text: &str, error: bool) {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK};
    let text: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let title: Vec<u16> = t().app_title.encode_utf16().chain(std::iter::once(0)).collect();
    let icon = if error { MB_ICONERROR } else { MB_ICONINFORMATION };
    // SAFETY: both strings are NUL-terminated and outlive the call; no owner window.
    unsafe { MessageBoxW(None, PCWSTR(text.as_ptr()), PCWSTR(title.as_ptr()), MB_OK | icon) };
}

fn main() -> eframe::Result {
    // Before any text is shown: the saved language, else the Windows display language.
    let settings = Settings::load_from(&Settings::path());
    strings::set_lang(strings::startup_lang(settings.language.as_deref()));
    // A build whose stale-core has the test-only environment overrides (e.g. `--all-features`)
    // would trust whatever STALE_E2E_* says: never run it.
    if stale_core::E2E_OVERRIDES {
        message_box(t().e2e_build_refused, true);
        std::process::exit(1);
    }
    if !single_instance() {
        message_box(t().already_running, false);
        return Ok(());
    }
    // Before anything (the log included) is written below the data folder.
    let secured = stale_core::paths::secure_data_dir();
    if let Err(e) = &secured {
        eprintln!("could not lock down the data folder: {e}");
    }
    logging::init();
    install_panic_hook();
    log::info!("Stale Uninstaller {} starting", stale_core::version());
    match secured {
        Ok(warnings) => warnings.iter().for_each(|w| log::warn!("data folder: {w}")),
        Err(e) => log::warn!("could not lock down the data folder: {e}"),
    }
    log::info!("language: {}", strings::lang().code());
    spawn_auto_purge(settings.quarantine_days);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1100.0, 700.0]).with_title(t().app_title),
        ..Default::default()
    };
    eframe::run_native(t().app_title, options, Box::new(|cc| Ok(Box::new(StaleApp::new(cc)))))
}

/// Purges quarantines older than `days` on the "stale-auto-purge" thread, so the window never waits.
fn spawn_auto_purge(days: u32) {
    let spawned = std::thread::Builder::new().name("stale-auto-purge".into()).spawn(move || {
        let purged = auto_purge(&QuarantineOps::live(), days, Utc::now());
        if !purged.is_empty() {
            let ids: Vec<&str> = purged.iter().map(|id| id.0.as_str()).collect();
            log::info!("auto-purge (older than {days} days): {}", ids.join(", "));
        }
    });
    if let Err(e) = spawned {
        log::error!("could not spawn auto-purge thread: {e}");
    }
}

/// Logs every panic (message and location) before the default hook runs: in release builds
/// there is no console, so the log file is the only trace of a crashed scan thread.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic payload>".to_string());
        let location = info.location().map_or_else(|| "<unknown>".to_string(), |l| l.to_string());
        let thread = std::thread::current().name().unwrap_or("<unnamed>").to_string();
        log::error!("panic in thread '{thread}' at {location}: {msg}");
        default(info);
    }));
}
