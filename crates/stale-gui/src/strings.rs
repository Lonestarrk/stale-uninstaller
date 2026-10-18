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

// Settings menu
pub const SETTINGS_MENU: &str = "⚙";
pub const SHOW_HIDDEN: &str = "Visa dolda poster";

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
pub const STATE_NOT_UNINSTALLED: &str = "Inte avinstallerat";
/// Precedes an exit code: "Inte avinstallerat (kod 1602)".
pub const EXIT_CODE: &str = "kod";
pub const STATE_SKIPPED: &str = "Hoppades över";
pub const STOP_WAITING: &str = "Sluta vänta";
pub const STOP_WAITING_TIP: &str = "Fortsätter när avinstalleraren avslutats, utan att vänta på dess underprocesser";
pub const SHOW_LEFTOVERS: &str = "Visa rester";
pub const RETRY: &str = "Försök igen";
pub const CLOSE: &str = "Stäng";

// Views
pub const VIEW_PENDING: &str = "Vyn kommer i nästa steg";

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
pub const MARK_SAFE: &str = "Markera säkra";
pub const REMOVE_CHECKED: &str = "Ta bort markerade";
pub const SERVICE_REBOOT: &str = "Tjänster tas bort helt först efter omstart";
pub const QUARANTINE_THREAD_FAILED: &str = "Kunde inte starta borttagningen";
pub const QUARANTINE_ABORTED: &str = "Borttagningen avbröts oväntat – se loggfilen";

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
