//! Warnings kept as values and worded when drawn, so a language switch translates them too.

use stale_core::scan::ScanWarning;

use crate::strings::{self, Strings};

/// A fixed warning text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Msg {
    ScanAborted,
    QueueAborted,
    UninstallThreadFailed,
    QuarantineAborted,
    QuarantineThreadFailed,
    QuarantineAdminAborted,
    QuarantineAdminThreadFailed,
    QuarantineEntryMissing,
    LeftoverScanAborted,
    LeftoverScanThreadFailed,
    GhostScanAborted,
    GhostScanThreadFailed,
    GhostsNeedPrograms,
    KeepSaveFailed,
    SpaceScanAborted,
    SpaceScanThreadFailed,
    BrowseAborted,
    RemovalListAborted,
}

impl Msg {
    /// The text in the language of `s`.
    pub fn text(self, s: &'static Strings) -> &'static str {
        match self {
            Msg::ScanAborted => s.scan_aborted,
            Msg::QueueAborted => s.queue_aborted,
            Msg::UninstallThreadFailed => s.uninstall_thread_failed,
            Msg::QuarantineAborted => s.quarantine_aborted,
            Msg::QuarantineThreadFailed => s.quarantine_thread_failed,
            Msg::QuarantineAdminAborted => s.quarantine_admin_aborted,
            Msg::QuarantineAdminThreadFailed => s.quarantine_admin_thread_failed,
            Msg::QuarantineEntryMissing => s.quarantine_entry_missing,
            Msg::LeftoverScanAborted => s.leftover_scan_aborted,
            Msg::LeftoverScanThreadFailed => s.leftover_scan_thread_failed,
            Msg::GhostScanAborted => s.ghost_scan_aborted,
            Msg::GhostScanThreadFailed => s.ghost_scan_thread_failed,
            Msg::GhostsNeedPrograms => s.ghosts_need_programs,
            Msg::KeepSaveFailed => s.keep_save_failed,
            Msg::SpaceScanAborted => s.space_scan_aborted,
            Msg::SpaceScanThreadFailed => s.space_scan_thread_failed,
            Msg::BrowseAborted => s.browse_aborted,
            Msg::RemovalListAborted => s.removal_list_aborted,
        }
    }
}

/// A warning shown in the warnings list or the uninstall dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Warning {
    Scan(ScanWarning),
    Fixed(Msg),
    /// `"<text>: <detail>"`; the detail is technical (an OS error, an id).
    Detail(Msg, String),
    /// The elevated token's account differs from the logged-in user.
    OtherAccount { session: String, token: String },
    /// A queued program without a runnable uninstaller.
    NoUninstaller(String),
    /// Purging `program`'s quarantine failed; `error` is the core's purge error.
    PurgeFailed { program: String, error: String },
}

impl Warning {
    /// The text in the current language.
    pub fn text(&self) -> String {
        let s = strings::t();
        match self {
            Warning::Scan(w) => strings::scan_warning(w),
            Warning::Fixed(m) => m.text(s).to_string(),
            Warning::Detail(m, detail) => format!("{}: {detail}", m.text(s)),
            Warning::OtherAccount { session, token } => strings::other_account_warning(session, token),
            Warning::NoUninstaller(name) => strings::no_uninstaller(name),
            Warning::PurgeFailed { program, error } => {
                format!("{} ({program}): {}", s.purge_failed, strings::purge_error(error))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strings::{EN, SV};

    #[test]
    fn fixed_messages_follow_the_language_they_are_drawn_in() {
        assert_eq!(Msg::ScanAborted.text(&SV), SV.scan_aborted);
        assert_eq!(Msg::ScanAborted.text(&EN), "The scan stopped unexpectedly – see the log file");
        assert_eq!(Warning::Detail(Msg::KeepSaveFailed, "denied".into()).text(), format!("{}: denied", SV.keep_save_failed));
    }

    #[test]
    fn warnings_render_through_their_helpers() {
        let w = Warning::Scan(ScanWarning::PrefetchUnreadable { skipped: 2, total: 9 });
        assert_eq!(w.text(), "Prefetch: 2 av 9 filer kunde inte läsas");
        assert_eq!(Warning::NoUninstaller("App".into()).text(), "App: ingen avinstallerare hittades, tvingad borttagning");
        let purge = Warning::PurgeFailed { program: "Foo".into(), error: "drive-missing: E:".into() };
        assert_eq!(purge.text(), format!("{} (Foo): Disken E: är inte ansluten – posten behålls", SV.purge_failed));
    }
}
