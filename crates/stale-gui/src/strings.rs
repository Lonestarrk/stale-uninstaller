//! All user-visible text (Swedish).

pub const APP_TITLE: &str = "Stale Uninstaller";
pub const ALREADY_RUNNING: &str = "Stale Uninstaller körs redan.";
/// Shown when the build has the test-only e2e-overrides feature; the program then exits.
pub const E2E_BUILD_REFUSED: &str = "Den här versionen är byggd med testfunktionen e2e-overrides och kan inte köras. Bygg om utan den.";
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
pub const KEPT_MENU: &str = "Behållna…";
pub const KEPT_TITLE: &str = "Behållna";
pub const KEPT_EMPTY: &str = "Inget är behållet";
pub const KEPT_REMOVE: &str = "Ta bort";
pub const UNDO: &str = "Ångra";

/// A kept program name in the "Behållna" window.
pub fn kept_name(name: &str) -> String {
    format!("Program: {name}")
}

/// A kept folder in the "Behållna" window.
pub fn kept_path(path: &str) -> String {
    format!("Mapp: {path}")
}

/// A kept registry key in the "Behållna" window.
pub fn kept_key(key: &str) -> String {
    format!("Registernyckel: {key}")
}

/// The one-line notice after "Behåll", next to "Ångra".
pub fn kept_notice(name: &str) -> String {
    format!("{name} behålls och visas inte längre.")
}

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
/// Counted non-main exe (a subcomponent of the program).
pub const COUNTED_SUBCOMPONENT: &str = "räknas";
/// Between the date and the exe name in "Idag (via link.exe)".
pub const VIA: &str = "via";
pub const RUNS: &str = "körningar";
pub const EXCLUDED_NAME_PATTERN: &str = "uppdaterare/tjänst-namn";
pub const EXCLUDED_SERVICE: &str = "tjänst";
pub const EXCLUDED_SCHEDULED_TASK: &str = "schemalagd uppgift";
pub const EXCLUDED_NOT_MAIN: &str = "inte huvudprogram";
pub const SOURCE_PREFETCH: &str = "Prefetch";
pub const SOURCE_BAM: &str = "BAM";
pub const SOURCE_USERASSIST: &str = "UserAssist";
pub const SOURCE_FILETIME: &str = "filtid";

// Uninstall queue dialog
pub const UNINSTALL_THREAD_FAILED: &str = "Kunde inte starta avinstallationen";
pub const QUEUE_ABORTED: &str = "Avinstallationen avbröts oväntat – se loggfilen";
pub const QUEUE_TITLE: &str = "Avinstallera";
pub const OTHER_ACCOUNT_QUEUE: &str = "Program från andra konton avinstalleras med dina administratörsrättigheter; kontrollera avinstallerarens sökväg nedan.";
pub const UNKNOWN_ACCOUNT: &str = "okänt konto";

/// Start-up warning: the elevated token's account differs from the logged-in user.
pub fn other_account_warning(session: &str, token: &str) -> String {
    format!("Du kör som {token} men är inloggad som {session}: per-användarprogram och AppData avser {token}.")
}

/// A queue item belonging to another account; `account` is `None` when it could not be resolved.
pub fn other_account_item(name: &str, account: Option<&str>) -> String {
    format!("{name} (konto: {})", account.unwrap_or(UNKNOWN_ACCOUNT))
}
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

/// A queued program without a runnable uninstaller: "Foo: ingen avinstallerare hittades, tvingad borttagning".
pub fn no_uninstaller(name: &str) -> String {
    format!("{name}: ingen avinstallerare hittades, tvingad borttagning")
}
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
pub const NOTE_CONTAINS_USER_DATA: &str = "innehåller troligen dina egna filer";
pub const NOTE_SUFFIX_MATCH: &str = "namnet är programmets namn med ett tillägg";
pub const NOTE_PROGID_OF_PROGRAM: &str = "filtyp som bär programmets namn";
pub const NOTE_APPDATA_ONLY: &str = "bara mappar i AppData";
pub const NOTE_DATA_ONLY: &str = "bara datamappar (ProgramData/AppData)";
pub const NOTE_OTHER_PROFILE: &str = "ligger i en annan användares profil";
pub const NOTE_FIRST_WORD: &str = "namnet är första ordet i ett installerat programs namn";
pub const NOTE_WORD_OF_INSTALLED: &str = "namnet ingår i ett installerat programs namn";
pub const NOTE_SIBLING_OWNED: &str = "utgivarmappen innehåller ett installerat program";
pub const NOTE_PUBLISHER_OF_INSTALLED: &str = "namnet är utgivaren av ett installerat program";
pub const NOTE_IN_USE: &str = "används av en tjänst, uppgift, autostart eller genväg";
pub const NOTE_LAUNCHER_LIBRARY: &str = "tillhör ett spelbibliotek";
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

// Ghost programs view
pub const GHOSTS: &str = "Spökprogram";
pub const GHOSTS_TITLE: &str = GHOSTS;
pub const GHOSTS_SCANNING: &str = "Letar efter rester av avinstallerade program…";
pub const GHOSTS_EMPTY: &str = "Inga spökprogram hittades";
pub const GHOSTS_NOT_SCANNED: &str = "Ingen sökning gjord – tryck Uppdatera";
pub const GHOSTS_NEED_PROGRAMS: &str = "Programlistan är tom – kör Uppdatera innan spökprogram söks";
pub const GHOST_SCAN_THREAD_FAILED: &str = "Kunde inte starta sökningen efter spökprogram";
pub const GHOST_SCAN_ABORTED: &str = "Sökningen efter spökprogram avbröts oväntat – se loggfilen";
pub const ASSESS_PROBABLY: &str = "Troligen avinstallerat";
pub const ASSESS_LEFTOVERS: &str = "Rester utan installation";
pub const ASSESS_UNCERTAIN: &str = "Osäker";
pub const KEEP: &str = "Behåll";
pub const KEEP_SAVE_FAILED: &str = "Kunde inte spara listan över behållna program";
pub const KEEP_TIP: &str = "Visa aldrig det här programmet, dess mappar eller nycklar igen";
pub const SHOW_GHOST_LEFTOVERS: &str = "Visa rester för markerade";
/// The leftovers view's back button when it was opened from the ghosts view.
pub const BACK: &str = "Tillbaka";
pub const REF_RUN: &str = "Autostart";
pub const REF_APP_PATHS: &str = "App Paths";
pub const REF_CLASSES_SERVER: &str = "COM-komponent";
pub const REF_SHELL_COMMAND: &str = "Filtyp";
pub const REF_SERVICE: &str = "Tjänst";
pub const REF_TASK: &str = "Schemalagd uppgift";
pub const REF_SHORTCUT: &str = "Genväg";

/// The section of ghosts that were probably uninstalled (checked by default).
pub fn probably_section(count: usize) -> String {
    format!("Troligen avinstallerade ({count})")
}

/// The open section of leftovers without an installation (unchecked by default).
pub fn leftovers_section(count: usize) -> String {
    format!("Rester utan installation (bocka i själv) ({count})")
}

/// The collapsed section of uncertain ghosts: "Osäkra (3)".
pub fn uncertain_section(count: usize) -> String {
    format!("Osäkra ({count})")
}

/// The header of a ghost's evidence list: "Spår (4)".
pub fn evidence_header(count: usize) -> String {
    format!("Spår ({count})")
}

/// The ghosts view's footer: "2 av 5 markerade · 58 MB".
pub fn ghost_summary(checked: usize, total: usize, size: &str) -> String {
    format!("{checked} av {total} markerade · {size}")
}

/// An orphan folder: "Mapp: C:\Program Files\Foo (38 MB)".
pub fn evidence_folder(path: &str, size: &str) -> String {
    format!("Mapp: {path} ({size})")
}

/// A key whose path value points at a missing file: "Nyckel: HKLM\… (pekar på saknad D:\Foo)".
pub fn evidence_key_missing_path(key: &str, path: &str) -> String {
    format!("Nyckel: {key} (pekar på saknad {path})")
}

/// A vendor key with nothing installed behind it: "Nyckel: HKCU\Software\Foo".
pub fn evidence_key(key: &str) -> String {
    format!("Nyckel: {key}")
}

/// A reference to a missing file: "Autostart: HKLM\…\Run → C:\Foooo.exe".
pub fn evidence_ref(kind: &str, location: &str, target: &str) -> String {
    format!("{kind}: {location} → {target}")
}

/// An execution trace: "Körd 2024-01-20 (Prefetch): C:\Foooo.exe".
pub fn evidence_usage(date: &str, source: &str, exe: &str) -> String {
    format!("Körd {date} ({source}): {exe}")
}

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
    fn other_account_warning_text_is_swedish() {
        assert_eq!(
            other_account_warning(r"PC\jimmy", r"PC\admin"),
            r"Du kör som PC\admin men är inloggad som PC\jimmy: per-användarprogram och AppData avser PC\admin."
        );
        assert_eq!(other_account_item("Foo", Some(r"PC\anna")), r"Foo (konto: PC\anna)");
        assert_eq!(other_account_item("Foo", None), "Foo (konto: okänt konto)");
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
