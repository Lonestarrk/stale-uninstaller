//! The question `--uninstall-cleanup --ask` puts when a quarantine exists: a task dialog with three
//! labelled buttons, or a Yes/No/Cancel message box when task dialogs are not available (no Common
//! Controls 6 activated, see the exe manifest).

use stale_core::pathutil::wide;
use stale_core::selfclean::Choice;

use crate::strings::{self, t, Lang};

/// Button ids of the task dialog.
const BUTTON_PURGE: i32 = 100;
const BUTTON_KEEP: i32 = 101;
const BUTTON_CANCEL: i32 = 102;
/// What a task dialog returns when it is closed (Esc, the close box).
const IDCANCEL: i32 = 2;

/// The task dialog's buttons in `lang`, in order; the second (keep) is the default.
fn buttons_in(lang: Lang) -> [(i32, &'static str); 3] {
    let s = strings::texts(lang);
    [
        (BUTTON_PURGE, s.uninstall_cleanup_purge),
        (BUTTON_KEEP, s.uninstall_cleanup_keep),
        (BUTTON_CANCEL, s.uninstall_cleanup_cancel),
    ]
}

/// The answer behind the button `id`; closing the dialog cancels the uninstall, as the message
/// box's Cancel does. `None` for anything else.
fn choice_for(id: i32) -> Option<Choice> {
    match id {
        BUTTON_PURGE => Some(Choice::Yes),
        BUTTON_KEEP => Some(Choice::No),
        BUTTON_CANCEL | IDCANCEL => Some(Choice::Cancel),
        _ => None,
    }
}

/// Asks what to do with a quarantine of `total` bytes; `None` when no question could be shown.
pub fn ask(total: u64) -> Option<Choice> {
    match task_dialog(total) {
        Some(answer) => answer,
        None => message_box(&strings::uninstall_cleanup_prompt(total)),
    }
}

/// Brings the dialog to the front and keeps it above other windows: the uninstaller that runs
/// this has its own window, and the question must not end up behind it.
unsafe extern "system" fn on_top(
    hwnd: windows::Win32::Foundation::HWND,
    msg: windows::Win32::UI::Controls::TASKDIALOG_NOTIFICATIONS,
    _wparam: windows::Win32::Foundation::WPARAM,
    _lparam: windows::Win32::Foundation::LPARAM,
    _data: isize,
) -> windows::core::HRESULT {
    use windows::Win32::UI::Controls::TDN_CREATED;
    use windows::Win32::UI::WindowsAndMessaging::{SetForegroundWindow, SetWindowPos, HWND_TOPMOST, SWP_NOMOVE, SWP_NOSIZE};
    if msg == TDN_CREATED {
        // SAFETY: `hwnd` is the dialog's window, alive during the notification.
        unsafe {
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
            let _ = SetForegroundWindow(hwnd);
        }
    }
    windows::Win32::Foundation::S_OK
}

/// The task dialog: `None` when it could not be shown (TaskDialogIndirect is missing, as with the
/// Common Controls 5 that a process without the manifest gets, or it failed), else the answer.
fn task_dialog(total: u64) -> Option<Option<Choice>> {
    use windows::core::{s, w, BOOL, HRESULT};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};
    use windows::Win32::UI::Controls::{
        TASKDIALOGCONFIG, TASKDIALOGCONFIG_0, TASKDIALOG_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION, TDF_USE_COMMAND_LINKS,
        TD_WARNING_ICON,
    };
    type TaskDialogIndirectFn = unsafe extern "system" fn(*const TASKDIALOGCONFIG, *mut i32, *mut i32, *mut BOOL) -> HRESULT;

    // System32 only; the manifest's activation context still redirects to Common Controls 6.
    // SAFETY: a constant, NUL-terminated name; the module stays loaded for the process.
    let module = unsafe { LoadLibraryExW(w!("comctl32.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) }.ok()?;
    // SAFETY: `module` is loaded; the name is a constant NUL-terminated string.
    let proc = unsafe { GetProcAddress(module, s!("TaskDialogIndirect")) }?;
    // SAFETY: TaskDialogIndirect has exactly this signature.
    let task_dialog_indirect: TaskDialogIndirectFn = unsafe { std::mem::transmute(proc) };

    let lang = strings::lang();
    let title = wide(t().app_title);
    let heading = wide(strings::uninstall_cleanup_heading(total));
    let question = wide(t().uninstall_cleanup_question);
    let labels: Vec<(i32, Vec<u16>)> = buttons_in(lang).iter().map(|(id, text)| (*id, wide(text))).collect();
    let buttons: Vec<TASKDIALOG_BUTTON> = labels
        .iter()
        .map(|(id, text)| TASKDIALOG_BUTTON { nButtonID: *id, pszButtonText: windows::core::PCWSTR(text.as_ptr()) })
        .collect();
    let config = TASKDIALOGCONFIG {
        cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
        dwFlags: TDF_USE_COMMAND_LINKS | TDF_ALLOW_DIALOG_CANCELLATION,
        pszWindowTitle: windows::core::PCWSTR(title.as_ptr()),
        Anonymous1: TASKDIALOGCONFIG_0 { pszMainIcon: TD_WARNING_ICON },
        pszMainInstruction: windows::core::PCWSTR(heading.as_ptr()),
        pszContent: windows::core::PCWSTR(question.as_ptr()),
        cButtons: buttons.len() as u32,
        pButtons: buttons.as_ptr(),
        nDefaultButton: BUTTON_KEEP,
        pfCallback: Some(on_top),
        ..Default::default()
    };
    let mut pressed = 0i32;
    // SAFETY: `config` and every string and button it points to outlive the call; the optional
    // out-pointers are null.
    let hr = unsafe { task_dialog_indirect(&config, &mut pressed, std::ptr::null_mut(), std::ptr::null_mut()) };
    if hr.is_err() {
        eprintln!("TaskDialogIndirect: {}", hr.message());
        return None;
    }
    Some(choice_for(pressed))
}

/// The fallback: the question in a Yes/No/Cancel message box (No is the default button); `None`
/// when it could not be shown.
fn message_box(text: &str) -> Option<Choice> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDCANCEL, IDNO, IDYES, MB_DEFBUTTON2, MB_ICONQUESTION, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNOCANCEL,
    };
    let text = wide(text);
    let title = wide(t().app_title);
    let style = MB_YESNOCANCEL | MB_ICONQUESTION | MB_DEFBUTTON2 | MB_SETFOREGROUND | MB_TOPMOST;
    // SAFETY: both strings are NUL-terminated and outlive the call; no owner window.
    let answer = unsafe { MessageBoxW(None, PCWSTR(text.as_ptr()), PCWSTR(title.as_ptr()), style) };
    match answer {
        IDYES => Some(Choice::Yes),
        IDNO => Some(Choice::No),
        IDCANCEL => Some(Choice::Cancel),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_map_to_choices_in_both_languages() {
        for lang in [Lang::Sv, Lang::En] {
            let choices: Vec<Option<Choice>> = buttons_in(lang).iter().map(|(id, _)| choice_for(*id)).collect();
            assert_eq!(choices, vec![Some(Choice::Yes), Some(Choice::No), Some(Choice::Cancel)]);
        }
        assert_eq!(buttons_in(Lang::En)[0].1, "Empty the quarantine and remove everything");
        assert_eq!(buttons_in(Lang::Sv)[1].1, "Behåll karantänen och datan");
        assert_eq!(choice_for(IDCANCEL), Some(Choice::Cancel), "closing the dialog cancels, as before");
        assert_eq!(choice_for(0), None);
    }
}
