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
pub const NOT_YET: &str = "Kommer i nästa steg";

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
