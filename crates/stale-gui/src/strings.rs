//! All user-visible text (Swedish).

pub const APP_TITLE: &str = "Stale Uninstaller";
pub const LOADING: &str = "Läser installerade program…";
pub const NO_PROGRAMS: &str = "Inga program hittades.";
pub const WARNINGS: &str = "Varningar";
pub const SCAN_THREAD_FAILED: &str = "Kunde inte starta genomsökningen";
pub const SCAN_ABORTED: &str = "Genomsökningen avbröts oväntat – se loggfilen";
pub const UNKNOWN_DRIVE: &str = "?";

// Toolbar
pub const UNINSTALL: &str = "Avinstallera";
pub const FORCE_REMOVE: &str = "Tvingad borttagning";
pub const FIND_LEFTOVERS: &str = "Sök rester";
pub const REFRESH: &str = "Uppdatera";
pub const SEARCH_HINT: &str = "Sök program…";

// Table columns
pub const COL_NAME: &str = "Namn";
pub const COL_PUBLISHER: &str = "Utgivare";
pub const COL_DRIVE: &str = "Disk";
pub const COL_SIZE: &str = "Storlek";
pub const COL_INSTALLED: &str = "Installerad";
pub const COL_LAST_USED: &str = "Senast använd";

// Filter panel
pub const GROUP_LAST_USED: &str = "Senast använd";
pub const GROUP_DRIVE: &str = "Disk";
pub const GROUP_SIZE: &str = "Storlek";
pub const BUCKET_ALL: &str = "Alla";
pub const BUCKET_OVER_SIX_MONTHS: &str = "Över 6 månader";
pub const BUCKET_OVER_ONE_YEAR: &str = "Över 1 år";
pub const BUCKET_NEVER_SEEN: &str = "Aldrig sedd";
pub const BUCKET_OVER_ONE_GB: &str = "Över 1 GB";

// Relative dates and sizes
pub const NEVER_SEEN: &str = "Aldrig sedd";
/// Usage not known yet (scan still running).
pub const PENDING: &str = "…";
pub const TODAY: &str = "Idag";
pub const YESTERDAY: &str = "Igår";
pub const DAYS_AGO: &str = "dagar sedan";
pub const MONTHS_AGO: &str = "mån sedan";
pub const YEARS_AGO: &str = "år sedan";
pub const MB: &str = "MB";
pub const GB: &str = "GB";
pub const UNKNOWN_VALUE: &str = "–";
/// Between items on one line: "C: 38 MB · D: 41 MB".
pub const LIST_SEPARATOR: &str = " · ";

// Settings menu
pub const SETTINGS_MENU: &str = "⚙";
pub const SHOW_HIDDEN: &str = "Visa dolda poster";
pub const AUTO_PURGE_DAYS: &str = "Töm karantän automatiskt efter N dagar";
/// Next to the auto-purge days when they are 0.
pub const AUTO_PURGE_OFF: &str = "(av)";

// Details panel
pub const NO_SELECTION: &str = "Inget program valt";
pub const DETAIL_LOCATION: &str = "Plats";
pub const DETAIL_UNINSTALLER: &str = "Avinstallerare";
pub const DETAIL_REGISTRY_KEY: &str = "Registernyckel";
/// Appended to the registry key of a program installed for another account.
pub const OTHER_USER_SUFFIX: &str = " (annan användare)";
pub const DETAIL_LAST_USED: &str = "Senast använd";
pub const UNCERTAIN_FILETIME: &str = " – osäker (filtid)";
pub const COL_FILE: &str = "Fil";
pub const COL_LAST_RUN: &str = "Senast körd";
pub const COL_SOURCE: &str = "Källa";
pub const COL_COUNTED: &str = "Räknas";
pub const COUNTED_YES: &str = "✓";
pub const RUNS: &str = "körningar";
pub const EXCLUDED_NAME_PATTERN: &str = "uppdaterare/tjänst-namn";
pub const EXCLUDED_SERVICE: &str = "tjänst";
pub const EXCLUDED_NOT_MAIN: &str = "inte huvudprogram";
pub const SOURCE_PREFETCH: &str = "Prefetch";
pub const SOURCE_BAM: &str = "BAM";
pub const SOURCE_USERASSIST: &str = "UserAssist";
pub const SOURCE_FILETIME: &str = "filtid";

// Uninstall queue dialog
pub const UNINSTALL_THREAD_FAILED: &str = "Kunde inte starta avinstallationen";
pub const QUEUE_ABORTED: &str = "Avinstallationen avbröts oväntat – se loggfilen";
pub const QUEUE_TITLE: &str = "Avinstallera";
pub const OPT_RESTORE_POINT: &str = "Skapa återställningspunkt för hela omgången";
pub const OPT_RUN_VENDOR: &str = "Kör programmens egna avinstallerare";
pub const CANCEL: &str = "Avbryt";
pub const START: &str = "Starta";
pub const STEP_RESTORE_POINT: &str = "Återställningspunkt";
pub const STEP_VENDOR: &str = "Egen avinstallerare";
pub const STEP_LEFTOVERS: &str = "Rester";
pub const STEP_DONE: &str = "Klart";
pub const STEP_SEPARATOR: &str = "·";
pub const RESTORE_POINT_LABEL: &str = "Återställningspunkt";
pub const RP_CREATED: &str = "skapad";
pub const RP_SKIPPED: &str = "hoppades över (en punkt skapades nyligen)";
pub const RP_DISABLED: &str = "systemåterställning är avstängd";
pub const RP_FAILED: &str = "misslyckades";
pub const STATE_PENDING: &str = "Väntar";
pub const STATE_RUNNING: &str = "Pågår";
pub const STATE_UNINSTALLED: &str = "Avinstallerat";
/// Removed without the program's own uninstaller.
pub const STATE_FORCED: &str = "Tvingad";
pub const STATE_NOT_UNINSTALLED: &str = "Inte avinstallerat";
/// Precedes an exit code: "Inte avinstallerat (kod 1602)".
pub const EXIT_CODE: &str = "kod";
pub const STATE_SKIPPED: &str = "Hoppades över";
pub const STOP_WAITING: &str = "Sluta vänta";
pub const STOP_WAITING_TIP: &str = "Fortsätter nu, utan att vänta på avinstallerarens kvarvarande processer";
pub const SHOW_LEFTOVERS: &str = "Visa rester";
pub const RETRY: &str = "Försök igen";
pub const CLOSE: &str = "Stäng";

// Leftovers view
pub const LEFTOVERS_TITLE: &str = "Rester";
pub const BACK_TO_PROGRAMS: &str = "Till programlistan";
pub const NO_LEFTOVERS: &str = "Inga rester hittades.";
pub const SECTION_FILES: &str = "Mappar och filer";
pub const SECTION_REGISTRY: &str = "Registernycklar";
pub const SECTION_OTHER: &str = "Övrigt";
pub const KIND_DIR: &str = "Mapp";
pub const KIND_FILE: &str = "Fil";
pub const KIND_REGKEY: &str = "Nyckel";
pub const KIND_REGVALUE: &str = "Värde";
pub const KIND_SHORTCUT: &str = "Genväg";
pub const KIND_SERVICE: &str = "Tjänst";
pub const KIND_TASK: &str = "Schemalagd uppgift";
pub const WARNING_ICON: &str = "⚠";
pub const NOTE_USER_FOLDER: &str = "troligen dina egna filer";
pub const NOTE_SHARED_PUBLISHER: &str = "delas med andra program från samma utgivare";
pub const NOTE_OTHER_PROGRAM: &str = "används av ett annat installerat program";
pub const MARK_SAFE: &str = "Markera säkra";
pub const REMOVE_CHECKED: &str = "Ta bort markerade";
pub const SERVICE_REBOOT: &str = "Tjänster tas bort helt först efter omstart";
pub const QUARANTINE_THREAD_FAILED: &str = "Kunde inte starta borttagningen";
pub const QUARANTINE_ABORTED: &str = "Borttagningen avbröts oväntat – se loggfilen";
/// `{X}` is the drive letter.
pub const INSUFFICIENT_SPACE: &str = "Inte tillräckligt med ledigt utrymme på {X}:";
pub const CONTINUE: &str = "Fortsätt";

/// "E: saknar NTFS – 30 MB kopieras till C:" in the removal confirmation.
pub fn fallback_notice(from: char, size: &str) -> String {
    format!("{from}: saknar NTFS – {size} kopieras till C:")
}

/// [`INSUFFICIENT_SPACE`] for `drive`.
pub fn insufficient_space(drive: char) -> String {
    INSUFFICIENT_SPACE.replace("{X}", &drive.to_string())
}

/// "(3 träffar, 58 MB)" after a section title.
pub fn section_counts(hits: usize, size: &str) -> String {
    format!("({hits} träffar, {size})")
}

/// The leftovers view's footer: "3 av 5 markerade · 58 MB frigörs".
pub fn checked_summary(checked: usize, total: usize, size: &str) -> String {
    format!("{checked} av {total} markerade · {size} frigörs")
}

/// "4 objekt flyttade till karantän".
pub fn moved_to_quarantine(count: usize) -> String {
    format!("{count} objekt flyttade till karantän")
}

// Standalone leftover search
pub const SEARCH_TITLE: &str = FIND_LEFTOVERS;
pub const SEARCH_NAME: &str = "Programnamn";
pub const SEARCH_DIR: &str = "Mapp (valfri)";
pub const SEARCH: &str = "Sök";
pub const SEARCHING: &str = "Söker rester…";
pub const LEFTOVER_SCAN_THREAD_FAILED: &str = "Kunde inte starta sökningen efter rester";
pub const LEFTOVER_SCAN_ABORTED: &str = "Sökningen efter rester avbröts oväntat – se loggfilen";

// Quarantine view
pub const QUARANTINE: &str = "Karantän";

/// The toolbar's quarantine button: "Karantän", or "Karantän (3)" when it holds anything.
pub fn quarantine_button(count: usize) -> String {
    if count == 0 { QUARANTINE.to_string() } else { format!("{QUARANTINE} ({count})") }
}

pub const QUARANTINE_TITLE: &str = "Karantän";
pub const QUARANTINE_EMPTY: &str = "Karantänen är tom";
pub const PURGE_ALL: &str = "Töm karantän";
pub const PURGE: &str = "Töm";
pub const COL_PROGRAM: &str = "Program";
pub const COL_DATE: &str = "Datum";
pub const COL_STATUS: &str = "Status";
pub const STATUS_OK: &str = "ok";
pub const RESTORE: &str = "Ångra";
pub const PURGE_ONE: &str = "Ta bort permanent";
pub const RESTORE_PARTIAL: &str = "Delvis återställt – följande kunde inte återställas:";
pub const QUARANTINE_ENTRY_MISSING: &str = "Karantänposten finns inte längre";
pub const PURGE_FAILED: &str = "Kunde inte ta bort karantänposten permanent";
pub const QUARANTINE_ADMIN_THREAD_FAILED: &str = "Kunde inte starta karantänåtgärden";
pub const QUARANTINE_ADMIN_ABORTED: &str = "Karantänåtgärden avbröts oväntat – se loggfilen";

/// The confirmation button of "Ta bort permanent".
pub const PURGE_ONE_CONFIRM_BUTTON: &str = "Ta bort";

/// The "Ta bort permanent" confirmation.
pub fn purge_one_confirm(name: &str, size: &str) -> String {
    format!("Detta tar bort {name} permanent ({size}) och kan inte ångras.")
}

/// The "Töm karantän" confirmation.
pub fn purge_all_confirm(size: &str) -> String {
    format!("Detta frigör {size} och kan inte ångras.")
}

/// A quarantine's status column: "2 fel".
pub fn error_count(n: usize) -> String {
    format!("{n} fel")
}

pub const UNKNOWN_CONTENT_LEFT: &str = "Okänt innehåll kvar i karantänmappen – lämnas orört";
/// `{X}` is the drive letter.
pub const DRIVE_MISSING: &str = "Disken {X}: är inte ansluten – posten behålls";

/// A quarantine's status column: "1 ofullständiga" (steps started but never recorded as done).
pub fn incomplete_count(n: usize) -> String {
    format!("{n} ofullständiga")
}

/// A core error code (a quarantine `Failure::error`) in Swedish; other text as it is.
pub fn core_error(error: &str) -> String {
    match error {
        stale_core::quarantine::UNKNOWN_CONTENT_LEFT => UNKNOWN_CONTENT_LEFT.to_string(),
        other => other.to_string(),
    }
}

/// A purge error: each `"; "`-separated `"<code>: <what>"` part with a known code becomes
/// `"<what>: <text>"` (`"drive-missing: X:"` becomes [`DRIVE_MISSING`]); other parts stay as
/// they are.
pub fn purge_error(e: &str) -> String {
    e.split("; ")
        .map(|part| match part.split_once(": ") {
            Some((code, what)) if code == stale_core::quarantine::UNKNOWN_CONTENT_LEFT => {
                format!("{what}: {UNKNOWN_CONTENT_LEFT}")
            }
            Some((code, drive)) if code == stale_core::quarantine::DRIVE_MISSING => {
                DRIVE_MISSING.replace("{X}", drive.trim_end_matches(':'))
            }
            _ => part.to_string(),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn purge_error_translates_known_codes() {
        assert_eq!(
            purge_error(r"unknown-content-left: C:\Stale Uninstaller\quarantine\x; access denied"),
            format!(r"C:\Stale Uninstaller\quarantine\x: {UNKNOWN_CONTENT_LEFT}; access denied")
        );
        assert_eq!(purge_error("drive-missing: E:"), "Disken E: är inte ansluten – posten behålls");
        assert_eq!(core_error("unknown-content-left"), UNKNOWN_CONTENT_LEFT);
        assert_eq!(core_error("something else"), "something else");
        assert_eq!(incomplete_count(2), "2 ofullständiga");
    }

    #[test]
    fn removal_notices() {
        assert_eq!(fallback_notice('E', "30 MB"), "E: saknar NTFS – 30 MB kopieras till C:");
        assert_eq!(insufficient_space('C'), "Inte tillräckligt med ledigt utrymme på C:");
    }

    #[test]
    fn purge_one_confirm_names_program_and_size() {
        assert_eq!(purge_one_confirm("Foo", "38 MB"), "Detta tar bort Foo permanent (38 MB) och kan inte ångras.");
    }
}
