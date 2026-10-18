// Keep a console in debug builds so panics are visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use chrono::Utc;
use eframe::egui;
use stale_core::quarantine::{auto_purge, QuarantineOps};
use stale_core::selfclean;
use stale_core::settings::Settings;
use stale_gui::app::StaleApp;
use stale_gui::logging;
use stale_gui::strings::{self, t};

/// The window icon: the 256 px image of `stale.ico`, made by `assets/make-icon.ps1`.
const WINDOW_ICON_PNG: &[u8] = include_bytes!("../assets/icon-256.png");

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

/// `text` in a message box titled with the app's name; `error` picks the error icon. `on_top`
/// keeps it above other windows (the uninstall cleanup runs without a window of its own).
fn message_box(text: &str, error: bool, on_top: bool) {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND, MB_TOPMOST, MESSAGEBOX_STYLE,
    };
    let text: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let title: Vec<u16> = t().app_title.encode_utf16().chain(std::iter::once(0)).collect();
    let icon = if error { MB_ICONERROR } else { MB_ICONINFORMATION };
    let top = if on_top { MB_TOPMOST | MB_SETFOREGROUND } else { MESSAGEBOX_STYLE(0) };
    // SAFETY: both strings are NUL-terminated and outlive the call; no owner window.
    unsafe { MessageBoxW(None, PCWSTR(text.as_ptr()), PCWSTR(title.as_ptr()), MB_OK | icon | top) };
}

fn main() -> eframe::Result {
    // Nothing is read below the data folder before it is locked down (`secure_data_dir`), so
    // the message boxes up to there follow the Windows display language.
    strings::set_lang(strings::detect_lang());
    // A build whose stale-core has the test-only environment overrides (e.g. `--all-features`)
    // would trust whatever STALE_E2E_* says: never run it.
    if stale_core::E2E_OVERRIDES {
        message_box(t().e2e_build_refused, true, false);
        std::process::exit(1);
    }
    // Run by Stale's own uninstaller: no window and no single-instance check (the uninstaller
    // makes sure the program is closed).
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|a| a == "--uninstall-cleanup") {
        std::process::exit(uninstall_cleanup(&args[1..]));
    }
    if !single_instance() {
        message_box(t().already_running, false, false);
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
    // The data folder is ours now: the saved language, else the Windows display language.
    let settings = Settings::load_from(&Settings::path());
    strings::set_lang(strings::startup_lang(settings.language.as_deref()));
    log::info!("language: {}", strings::lang().code());
    spawn_auto_purge(settings.quarantine_days);
    let mut viewport = egui::ViewportBuilder::default().with_inner_size([1100.0, 700.0]).with_title(t().app_title);
    match eframe::icon_data::from_png_bytes(WINDOW_ICON_PNG) {
        Ok(icon) => viewport = viewport.with_icon(icon),
        Err(e) => log::warn!("could not decode the window icon: {e}"),
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(t().app_title, options, Box::new(|cc| Ok(Box::new(StaleApp::new(cc)))))
}

/// `--uninstall-cleanup`, run by Stale's own uninstaller in two phases:
/// - `--ask [--silent]` before the uninstaller's own confirmation: only decides (asking about a
///   quarantine unless `--silent`) and exits with [`selfclean::exit_code_for`]: 10 remove the
///   data folder, 11 purge then remove, 12 keep everything, 2 stop the uninstall.
/// - `--apply remove|purge [--silent]` once the uninstall is confirmed: does the work
///   ([`selfclean::apply`], which re-checks the quarantine) and exits 0, or 1 when anything could
///   not be purged or removed (the installer's log records the code).
///
/// Anything else changes nothing and exits [`selfclean::EXIT_KEEP`].
fn uninstall_cleanup(args: &[std::ffi::OsString]) -> i32 {
    let silent = args.iter().any(|a| a == "--silent");
    let arg_after = |flag: &str| args.iter().position(|a| a == flag).map(|i| args.get(i + 1).and_then(|v| v.to_str()));
    let apply_mode = match arg_after("--apply") {
        None => None,
        Some(v) => match v.and_then(selfclean::ApplyMode::parse) {
            Some(mode) => Some(mode),
            None => {
                eprintln!("--apply needs remove or purge");
                return selfclean::EXIT_KEEP;
            }
        },
    };
    if apply_mode.is_none() && !args.iter().any(|a| a == "--ask") {
        eprintln!("--uninstall-cleanup needs --ask or --apply remove|purge");
        return selfclean::EXIT_KEEP;
    }
    // The folder is locked down first so the settings and manifests below it can be trusted; one
    // that cannot be (e.g. it is a link) is left alone. Nothing is logged: the log lives in it.
    if let Err(e) = stale_core::paths::secure_data_dir() {
        eprintln!("could not lock down the data folder, leaving it: {e}");
        return selfclean::EXIT_KEEP;
    }
    let settings = Settings::load_from(&Settings::path());
    strings::set_lang(strings::startup_lang(settings.language.as_deref()));
    let ops = QuarantineOps::live();
    let Some(mode) = apply_mode else {
        let action = selfclean::decide(&ops, &mut |total| {
            if silent { None } else { stale_gui::cleanup_dialog::ask(total) }
        });
        return selfclean::exit_code_for(action);
    };
    let out = selfclean::apply(&ops, &stale_core::paths::data_dir(), &selfclean::live_payload_parents(), mode);
    for e in &out.errors {
        eprintln!("{e}");
    }
    if out.untrusted > 0 {
        eprintln!("{} quarantine folder(s) could not be trusted: kept, with the data folder", out.untrusted);
    }
    if out.purge_failed && !silent {
        message_box(t().uninstall_cleanup_failed, true, true);
    }
    out.exit_code()
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
