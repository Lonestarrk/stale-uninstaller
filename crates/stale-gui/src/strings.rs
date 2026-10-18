//! All user-visible text, in Swedish and English.
//!
//! Fixed texts are fields of [`Strings`]; [`t()`] returns the set for the current language.
//! Texts with values are functions that branch on [`lang()`]. Every new text needs both
//! languages: a field missing from `SV` or `EN`, or a missing match arm, does not compile.

use std::sync::atomic::{AtomicU8, Ordering};

use stale_core::provenance::{InstallSource, Signature};
use stale_core::quarantine::codes;
use stale_core::scan::ScanWarning;

/// The user interface language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Sv,
    En,
}

impl Lang {
    /// `"sv"` or `"en"`, as saved in `settings.json`.
    pub fn code(self) -> &'static str {
        match self {
            Lang::Sv => "sv",
            Lang::En => "en",
        }
    }

    /// The language of a saved code; `None` for anything else.
    pub fn from_code(code: &str) -> Option<Lang> {
        match code {
            "sv" => Some(Lang::Sv),
            "en" => Some(Lang::En),
            _ => None,
        }
    }
}

/// The process-wide language: 0 = Swedish (the default until `set_lang`), 1 = English.
static LANG: AtomicU8 = AtomicU8::new(0);

/// Switches every text drawn from now on.
pub fn set_lang(lang: Lang) {
    LANG.store(if lang == Lang::En { 1 } else { 0 }, Ordering::Relaxed);
}

pub fn lang() -> Lang {
    if LANG.load(Ordering::Relaxed) == 1 { Lang::En } else { Lang::Sv }
}

/// The fixed texts in the current language.
pub fn t() -> &'static Strings {
    texts(lang())
}

/// The fixed texts in `lang`.
pub fn texts(lang: Lang) -> &'static Strings {
    match lang {
        Lang::Sv => &SV,
        Lang::En => &EN,
    }
}

/// Swedish for a Windows LANGID whose primary language is Swedish (sv-SE 0x041D, sv-FI 0x081D),
/// English for everything else.
pub fn lang_for_langid(langid: u16) -> Lang {
    const LANG_SWEDISH: u16 = 0x1D;
    if langid & 0x3FF == LANG_SWEDISH { Lang::Sv } else { Lang::En }
}

/// The Windows display language of the user, as a [`Lang`].
pub fn detect_lang() -> Lang {
    // SAFETY: no arguments; only reads the user's UI language.
    lang_for_langid(unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() })
}

/// The saved language (`settings.json`'s `"language"`), else the Windows display language.
pub fn startup_lang(saved: Option<&str>) -> Lang {
    saved.and_then(Lang::from_code).unwrap_or_else(detect_lang)
}

/// Every fixed user-visible text, one field per text. `SV` and `EN` must both set every field,
/// so a missing translation does not compile.
pub struct Strings {
    pub app_title: &'static str,
    pub already_running: &'static str,
    /// Shown when the build has the test-only e2e-overrides feature; the program then exits.
    pub e2e_build_refused: &'static str,
    pub loading: &'static str,
    pub no_programs: &'static str,
    pub warnings: &'static str,
    pub scan_thread_failed: &'static str,
    pub scan_aborted: &'static str,
    pub unknown_drive: &'static str,
    // Toolbar
    pub uninstall: &'static str,
    pub force_remove: &'static str,
    pub find_leftovers: &'static str,
    pub refresh: &'static str,
    pub search_hint: &'static str,
    // Table columns
    pub col_name: &'static str,
    pub col_publisher: &'static str,
    pub col_drive: &'static str,
    pub col_size: &'static str,
    pub col_installed: &'static str,
    pub col_last_used: &'static str,
    // Filter panel
    pub group_last_used: &'static str,
    pub group_drive: &'static str,
    pub group_size: &'static str,
    pub bucket_all: &'static str,
    pub bucket_over_six_months: &'static str,
    pub bucket_over_one_year: &'static str,
    pub bucket_never_seen: &'static str,
    pub bucket_over_one_gb: &'static str,
    // Relative dates and sizes
    pub never_seen: &'static str,
    /// Usage not known yet (scan still running).
    pub pending: &'static str,
    pub today: &'static str,
    pub yesterday: &'static str,
    pub days_ago: &'static str,
    pub months_ago: &'static str,
    pub years_ago: &'static str,
    pub mb: &'static str,
    pub gb: &'static str,
    pub unknown_value: &'static str,
    /// Between items on one line: "C: 38 MB · D: 41 MB".
    pub list_separator: &'static str,
    // Settings menu
    pub settings_menu: &'static str,
    pub show_hidden: &'static str,
    pub auto_purge_days: &'static str,
    /// Next to the auto-purge days when they are 0.
    pub auto_purge_off: &'static str,
    pub kept_menu: &'static str,
    /// The language submenu; the same in both languages.
    pub language_menu: &'static str,
    /// Same in both languages.
    pub lang_sv: &'static str,
    /// Same in both languages.
    pub lang_en: &'static str,
    pub kept_title: &'static str,
    pub kept_empty: &'static str,
    pub kept_remove: &'static str,
    pub undo: &'static str,
    // Details panel
    pub no_selection: &'static str,
    pub detail_location: &'static str,
    pub detail_uninstaller: &'static str,
    pub detail_registry_key: &'static str,
    /// Appended to the registry key of a program installed for another account.
    pub other_user_suffix: &'static str,
    pub detail_last_used: &'static str,
    pub uncertain_filetime: &'static str,
    // Details panel: provenance
    pub detail_signed_by: &'static str,
    pub detail_description: &'static str,
    pub detail_company: &'static str,
    pub detail_installed: &'static str,
    pub detail_install_source: &'static str,
    pub signature_unsigned: &'static str,
    /// A valid signature whose signer name could not be read.
    pub signature_unknown_signer: &'static str,
    /// `{reason}` is replaced by the translated reason.
    pub signature_invalid: &'static str,
    pub sig_bad_digest: &'static str,
    pub sig_expired: &'static str,
    pub sig_untrusted_root: &'static str,
    pub sig_revoked: &'static str,
    pub sig_distrusted: &'static str,
    pub sig_bad_certificate: &'static str,
    /// `{code}` is replaced by the hex status.
    pub sig_error_code: &'static str,
    pub source_winget: &'static str,
    pub source_store: &'static str,
    pub source_steam: &'static str,
    pub source_epic: &'static str,
    pub source_gog: &'static str,
    pub source_ea: &'static str,
    pub source_ubisoft: &'static str,
    pub source_driver: &'static str,
    pub source_oem: &'static str,
    pub source_msi: &'static str,
    pub source_unknown: &'static str,
    pub col_file: &'static str,
    pub col_last_run: &'static str,
    pub col_source: &'static str,
    pub col_counted: &'static str,
    pub counted_yes: &'static str,
    /// Counted non-main exe (a subcomponent of the program).
    pub counted_subcomponent: &'static str,
    /// Between the date and the exe name in "Idag (via link.exe)".
    pub via: &'static str,
    /// The date is an automatic start (scheduled task, Run value, Startup folder): "Idag (autostart)".
    pub autostart: &'static str,
    pub excluded_name_pattern: &'static str,
    pub excluded_service: &'static str,
    pub excluded_scheduled_task: &'static str,
    pub excluded_not_main: &'static str,
    pub source_prefetch: &'static str,
    pub source_bam: &'static str,
    pub source_userassist: &'static str,
    pub source_filetime: &'static str,
    // Uninstall queue dialog
    pub uninstall_thread_failed: &'static str,
    pub queue_aborted: &'static str,
    pub queue_title: &'static str,
    pub other_account_queue: &'static str,
    pub unknown_account: &'static str,
    pub opt_restore_point: &'static str,
    pub opt_run_vendor: &'static str,
    pub cancel: &'static str,
    pub start: &'static str,
    /// Badge on a runtime/SDK/developer tool that other programs probably rely on.
    pub dependency_badge: &'static str,
    pub dependency_warning: &'static str,
    /// Per-item acknowledgement checkbox for such a program.
    pub dependency_understand: &'static str,
    pub dependency_runtime: &'static str,
    pub dependency_sdk: &'static str,
    pub dependency_toolchain: &'static str,
    pub step_restore_point: &'static str,
    pub step_vendor: &'static str,
    pub step_leftovers: &'static str,
    pub step_done: &'static str,
    pub step_separator: &'static str,
    pub restore_point_label: &'static str,
    pub rp_created: &'static str,
    pub rp_skipped: &'static str,
    pub rp_disabled: &'static str,
    pub rp_failed: &'static str,
    pub state_pending: &'static str,
    pub state_running: &'static str,
    pub state_uninstalled: &'static str,
    /// Removed without the program's own uninstaller.
    pub state_forced: &'static str,
    pub state_not_uninstalled: &'static str,
    /// Precedes an exit code: "Inte avinstallerat (kod 1602)".
    pub exit_code: &'static str,
    pub state_skipped: &'static str,
    pub stop_waiting: &'static str,
    pub stop_waiting_tip: &'static str,
    pub show_leftovers: &'static str,
    pub retry: &'static str,
    pub close: &'static str,
    // Leftovers view
    pub leftovers_title: &'static str,
    pub back_to_programs: &'static str,
    pub no_leftovers: &'static str,
    pub section_files: &'static str,
    pub section_registry: &'static str,
    pub section_other: &'static str,
    pub kind_dir: &'static str,
    pub kind_file: &'static str,
    pub kind_regkey: &'static str,
    pub kind_regvalue: &'static str,
    pub kind_shortcut: &'static str,
    pub kind_service: &'static str,
    pub kind_task: &'static str,
    pub warning_icon: &'static str,
    pub note_user_folder: &'static str,
    pub note_shared_publisher: &'static str,
    pub note_other_program: &'static str,
    pub note_contains_user_data: &'static str,
    pub note_suffix_match: &'static str,
    pub note_progid_of_program: &'static str,
    pub note_appdata_only: &'static str,
    pub note_data_only: &'static str,
    pub note_other_profile: &'static str,
    pub note_first_word: &'static str,
    pub note_word_of_installed: &'static str,
    pub note_sibling_owned: &'static str,
    pub note_publisher_of_installed: &'static str,
    pub note_in_use: &'static str,
    pub note_launcher_library: &'static str,
    pub mark_safe: &'static str,
    pub remove_checked: &'static str,
    pub service_reboot: &'static str,
    pub quarantine_thread_failed: &'static str,
    pub quarantine_aborted: &'static str,
    /// `{X}` is the drive letter.
    pub insufficient_space: &'static str,
    pub continue_button: &'static str,
    // Standalone leftover search
    pub search_title: &'static str,
    pub search_name: &'static str,
    pub search_dir: &'static str,
    pub search: &'static str,
    pub searching: &'static str,
    pub leftover_scan_thread_failed: &'static str,
    pub leftover_scan_aborted: &'static str,
    // Ghost programs view
    pub ghosts: &'static str,
    pub ghosts_title: &'static str,
    pub ghosts_scanning: &'static str,
    pub ghosts_empty: &'static str,
    pub ghosts_not_scanned: &'static str,
    pub ghosts_need_programs: &'static str,
    pub ghost_scan_thread_failed: &'static str,
    pub ghost_scan_aborted: &'static str,
    pub assess_probably: &'static str,
    pub assess_leftovers: &'static str,
    pub assess_uncertain: &'static str,
    pub keep: &'static str,
    pub keep_save_failed: &'static str,
    pub keep_tip: &'static str,
    pub show_ghost_leftovers: &'static str,
    /// The leftovers view's back button when it was opened from the ghosts view.
    pub back: &'static str,
    pub ref_run: &'static str,
    pub ref_app_paths: &'static str,
    pub ref_classes_server: &'static str,
    pub ref_shell_command: &'static str,
    pub ref_service: &'static str,
    pub ref_task: &'static str,
    pub ref_shortcut: &'static str,
    // Quarantine view
    pub quarantine: &'static str,
    pub quarantine_title: &'static str,
    pub quarantine_empty: &'static str,
    pub purge_all: &'static str,
    pub purge: &'static str,
    pub col_program: &'static str,
    pub col_date: &'static str,
    pub col_status: &'static str,
    pub status_ok: &'static str,
    pub restore: &'static str,
    pub purge_one: &'static str,
    pub restore_partial: &'static str,
    pub quarantine_entry_missing: &'static str,
    pub purge_failed: &'static str,
    pub quarantine_admin_thread_failed: &'static str,
    pub quarantine_admin_aborted: &'static str,
    /// The confirmation button of "Ta bort permanent".
    pub purge_one_confirm_button: &'static str,
    pub unknown_content_left: &'static str,
    /// `{X}` is the drive letter.
    pub drive_missing: &'static str,
    /// Core error code `occupied`.
    pub code_occupied: &'static str,
    /// Core error code `acl-not-reinherited`.
    pub code_acl_not_reinherited: &'static str,
    /// Core error code `reparse-in-destination`.
    pub code_reparse_in_destination: &'static str,
    /// Core error code `reparse-in-quarantine-path`.
    pub code_reparse_in_quarantine_path: &'static str,
    /// Core error code `partial-move`.
    pub code_partial_move: &'static str,
    /// Core error code `manifest-save-failed`.
    pub code_manifest_save_failed: &'static str,
    /// Core error code `not-storable`.
    pub code_not_storable: &'static str,
    /// Core error code `stored-outside-quarantine`.
    pub code_stored_outside: &'static str,
    /// Core error code `backup-outside-quarantine`.
    pub code_backup_outside: &'static str,
    /// Core error code `backup-is-link`.
    pub code_backup_is_link: &'static str,
    /// Core error code `quarantine-not-trusted`.
    pub code_not_trusted: &'static str,
    /// Core error code `invalid-id`.
    pub code_invalid_id: &'static str,
    /// Core error code `is-reparse`.
    pub code_is_reparse: &'static str,
    /// Core error code `destination-exists`.
    pub code_destination_exists: &'static str,
    /// Core error code `locked`.
    pub code_locked: &'static str,
    /// Core error code `link-in-tree`.
    pub code_link_in_tree: &'static str,
}

pub static SV: Strings = Strings {
    app_title: "Stale Uninstaller",
    already_running: "Stale Uninstaller körs redan.",
    e2e_build_refused: "Den här versionen är byggd med testfunktionen e2e-overrides och kan inte köras. Bygg om utan den.",
    loading: "Läser installerade program…",
    no_programs: "Inga program hittades.",
    warnings: "Varningar",
    scan_thread_failed: "Kunde inte starta genomsökningen",
    scan_aborted: "Genomsökningen avbröts oväntat – se loggfilen",
    unknown_drive: "?",
    // Toolbar
    uninstall: "Avinstallera",
    force_remove: "Tvingad borttagning",
    find_leftovers: "Sök rester",
    refresh: "Uppdatera",
    search_hint: "Sök program…",
    // Table columns
    col_name: "Namn",
    col_publisher: "Utgivare",
    col_drive: "Disk",
    col_size: "Storlek",
    col_installed: "Installerad",
    col_last_used: "Senast använd",
    // Filter panel
    group_last_used: "Senast använd",
    group_drive: "Disk",
    group_size: "Storlek",
    bucket_all: "Alla",
    bucket_over_six_months: "Över 6 månader",
    bucket_over_one_year: "Över 1 år",
    bucket_never_seen: "Aldrig sedd",
    bucket_over_one_gb: "Över 1 GB",
    // Relative dates and sizes
    never_seen: "Aldrig sedd",
    pending: "…",
    today: "Idag",
    yesterday: "Igår",
    days_ago: "dagar sedan",
    months_ago: "mån sedan",
    years_ago: "år sedan",
    mb: "MB",
    gb: "GB",
    unknown_value: "–",
    list_separator: " · ",
    // Settings menu
    settings_menu: "⚙",
    show_hidden: "Visa dolda poster",
    auto_purge_days: "Töm karantän automatiskt efter N dagar",
    auto_purge_off: "(av)",
    kept_menu: "Behållna…",
    language_menu: "Språk / Language",
    lang_sv: "Svenska",
    lang_en: "English",
    kept_title: "Behållna",
    kept_empty: "Inget är behållet",
    kept_remove: "Ta bort",
    undo: "Ångra",
    // Details panel
    no_selection: "Inget program valt",
    detail_location: "Plats",
    detail_uninstaller: "Avinstallerare",
    detail_registry_key: "Registernyckel",
    other_user_suffix: " (annan användare)",
    detail_last_used: "Senast använd",
    uncertain_filetime: " – osäker (filtid)",
    detail_signed_by: "Signerad av",
    detail_description: "Beskrivning",
    detail_company: "Tillverkare",
    detail_installed: "Installerad",
    detail_install_source: "Källa",
    signature_unsigned: "osignerad",
    signature_unknown_signer: "okänd utgivare",
    signature_invalid: "ogiltig signatur: {reason}",
    sig_bad_digest: "filen har ändrats efter signeringen",
    sig_expired: "certifikatet har gått ut",
    sig_untrusted_root: "certifikatet är inte betrott",
    sig_revoked: "certifikatet är återkallat",
    sig_distrusted: "utgivaren är spärrad",
    sig_bad_certificate: "felaktigt certifikat",
    sig_error_code: "fel {code}",
    source_winget: "Winget",
    source_store: "Microsoft Store",
    source_steam: "Steam",
    source_epic: "Epic Games",
    source_gog: "GOG",
    source_ea: "EA app",
    source_ubisoft: "Ubisoft Connect",
    source_driver: "Drivrutin",
    source_oem: "Förinstallerad (tillverkare)",
    source_msi: "Windows Installer (MSI)",
    source_unknown: "Okänd",
    col_file: "Fil",
    col_last_run: "Senast körd",
    col_source: "Källa",
    col_counted: "Räknas",
    counted_yes: "✓",
    counted_subcomponent: "räknas",
    via: "via",
    autostart: "autostart",
    excluded_name_pattern: "uppdaterare/tjänst-namn",
    excluded_service: "tjänst",
    excluded_scheduled_task: "schemalagd uppgift",
    excluded_not_main: "inte huvudprogram",
    source_prefetch: "Prefetch",
    source_bam: "BAM",
    source_userassist: "UserAssist",
    source_filetime: "filtid",
    // Uninstall queue dialog
    uninstall_thread_failed: "Kunde inte starta avinstallationen",
    queue_aborted: "Avinstallationen avbröts oväntat – se loggfilen",
    queue_title: "Avinstallera",
    other_account_queue: "Program från andra konton avinstalleras med dina administratörsrättigheter; kontrollera avinstallerarens sökväg nedan.",
    unknown_account: "okänt konto",
    opt_restore_point: "Skapa återställningspunkt för hela omgången",
    opt_run_vendor: "Kör programmens egna avinstallerare",
    cancel: "Avbryt",
    start: "Starta",
    dependency_badge: "används av andra program",
    dependency_warning: "Detta program används troligen av andra program. Avinstallera bara om du vet att inget behöver det.",
    dependency_understand: "Jag förstår",
    dependency_runtime: "körmiljö",
    dependency_sdk: "utvecklingspaket",
    dependency_toolchain: "utvecklingsverktyg",
    step_restore_point: "Återställningspunkt",
    step_vendor: "Egen avinstallerare",
    step_leftovers: "Rester",
    step_done: "Klart",
    step_separator: "·",
    restore_point_label: "Återställningspunkt",
    rp_created: "skapad",
    rp_skipped: "hoppades över (en punkt skapades nyligen)",
    rp_disabled: "systemåterställning är avstängd",
    rp_failed: "misslyckades",
    state_pending: "Väntar",
    state_running: "Pågår",
    state_uninstalled: "Avinstallerat",
    state_forced: "Tvingad",
    state_not_uninstalled: "Inte avinstallerat",
    exit_code: "kod",
    state_skipped: "Hoppades över",
    stop_waiting: "Sluta vänta",
    stop_waiting_tip: "Fortsätter nu, utan att vänta på avinstallerarens kvarvarande processer",
    show_leftovers: "Visa rester",
    retry: "Försök igen",
    close: "Stäng",
    // Leftovers view
    leftovers_title: "Rester",
    back_to_programs: "Till programlistan",
    no_leftovers: "Inga rester hittades.",
    section_files: "Mappar och filer",
    section_registry: "Registernycklar",
    section_other: "Övrigt",
    kind_dir: "Mapp",
    kind_file: "Fil",
    kind_regkey: "Nyckel",
    kind_regvalue: "Värde",
    kind_shortcut: "Genväg",
    kind_service: "Tjänst",
    kind_task: "Schemalagd uppgift",
    warning_icon: "⚠",
    note_user_folder: "troligen dina egna filer",
    note_shared_publisher: "delas med andra program från samma utgivare",
    note_other_program: "används av ett annat installerat program",
    note_contains_user_data: "innehåller troligen dina egna filer",
    note_suffix_match: "namnet är programmets namn med ett tillägg",
    note_progid_of_program: "filtyp som bär programmets namn",
    note_appdata_only: "bara mappar i AppData",
    note_data_only: "bara datamappar (ProgramData/AppData)",
    note_other_profile: "ligger i en annan användares profil",
    note_first_word: "namnet är första ordet i ett installerat programs namn",
    note_word_of_installed: "namnet ingår i ett installerat programs namn",
    note_sibling_owned: "utgivarmappen innehåller ett installerat program",
    note_publisher_of_installed: "namnet är utgivaren av ett installerat program",
    note_in_use: "används av en tjänst, uppgift, autostart eller genväg",
    note_launcher_library: "tillhör ett spelbibliotek",
    mark_safe: "Markera säkra",
    remove_checked: "Ta bort markerade",
    service_reboot: "Tjänster tas bort helt först efter omstart",
    quarantine_thread_failed: "Kunde inte starta borttagningen",
    quarantine_aborted: "Borttagningen avbröts oväntat – se loggfilen",
    insufficient_space: "Inte tillräckligt med ledigt utrymme på {X}:",
    continue_button: "Fortsätt",
    // Standalone leftover search
    search_title: "Sök rester",
    search_name: "Programnamn",
    search_dir: "Mapp (valfri)",
    search: "Sök",
    searching: "Söker rester…",
    leftover_scan_thread_failed: "Kunde inte starta sökningen efter rester",
    leftover_scan_aborted: "Sökningen efter rester avbröts oväntat – se loggfilen",
    // Ghost programs view
    ghosts: "Spökprogram",
    ghosts_title: "Spökprogram",
    ghosts_scanning: "Letar efter rester av avinstallerade program…",
    ghosts_empty: "Inga spökprogram hittades",
    ghosts_not_scanned: "Ingen sökning gjord – tryck Uppdatera",
    ghosts_need_programs: "Programlistan är tom – kör Uppdatera innan spökprogram söks",
    ghost_scan_thread_failed: "Kunde inte starta sökningen efter spökprogram",
    ghost_scan_aborted: "Sökningen efter spökprogram avbröts oväntat – se loggfilen",
    assess_probably: "Troligen avinstallerat",
    assess_leftovers: "Rester utan installation",
    assess_uncertain: "Osäker",
    keep: "Behåll",
    keep_save_failed: "Kunde inte spara listan över behållna program",
    keep_tip: "Visa aldrig det här programmet, dess mappar eller nycklar igen",
    show_ghost_leftovers: "Visa rester för markerade",
    back: "Tillbaka",
    ref_run: "Autostart",
    ref_app_paths: "App Paths",
    ref_classes_server: "COM-komponent",
    ref_shell_command: "Filtyp",
    ref_service: "Tjänst",
    ref_task: "Schemalagd uppgift",
    ref_shortcut: "Genväg",
    // Quarantine view
    quarantine: "Karantän",
    quarantine_title: "Karantän",
    quarantine_empty: "Karantänen är tom",
    purge_all: "Töm karantän",
    purge: "Töm",
    col_program: "Program",
    col_date: "Datum",
    col_status: "Status",
    status_ok: "ok",
    restore: "Ångra",
    purge_one: "Ta bort permanent",
    restore_partial: "Delvis återställt – följande kunde inte återställas:",
    quarantine_entry_missing: "Karantänposten finns inte längre",
    purge_failed: "Kunde inte ta bort karantänposten permanent",
    quarantine_admin_thread_failed: "Kunde inte starta karantänåtgärden",
    quarantine_admin_aborted: "Karantänåtgärden avbröts oväntat – se loggfilen",
    purge_one_confirm_button: "Ta bort",
    unknown_content_left: "Okänt innehåll kvar i karantänmappen – lämnas orört",
    drive_missing: "Disken {X}: är inte ansluten – posten behålls",
    code_occupied: "originalet finns igen",
    code_acl_not_reinherited: "behörigheterna kunde inte återställas",
    code_reparse_in_destination: "länk i målsökvägen",
    code_reparse_in_quarantine_path: "länk i karantänsökvägen",
    code_partial_move: "bara delvis flyttad",
    code_manifest_save_failed: "karantänposten kunde inte sparas",
    code_not_storable: "sökvägen kan inte lagras i karantän",
    code_stored_outside: "lagrad utanför karantänmappen",
    code_backup_outside: "säkerhetskopian ligger utanför karantänmappen",
    code_backup_is_link: "säkerhetskopian är en länk",
    code_not_trusted: "karantänmappen ägs inte av Administratörer eller SYSTEM",
    code_invalid_id: "ogiltigt karantän-id",
    code_is_reparse: "en länk eller kopplingspunkt",
    code_destination_exists: "målet finns redan",
    code_locked: "används av ett annat program",
    code_link_in_tree: "länk i mappträdet",
};

pub static EN: Strings = Strings {
    app_title: "Stale Uninstaller",
    already_running: "Stale Uninstaller is already running.",
    e2e_build_refused: "This build was made with the test feature e2e-overrides and cannot run. Rebuild it without the feature.",
    loading: "Reading installed programs…",
    no_programs: "No programs found.",
    warnings: "Warnings",
    scan_thread_failed: "Could not start the scan",
    scan_aborted: "The scan stopped unexpectedly – see the log file",
    unknown_drive: "?",
    // Toolbar
    uninstall: "Uninstall",
    force_remove: "Force remove",
    find_leftovers: "Find leftovers",
    refresh: "Refresh",
    search_hint: "Search programs…",
    // Table columns
    col_name: "Name",
    col_publisher: "Publisher",
    col_drive: "Drive",
    col_size: "Size",
    col_installed: "Installed",
    col_last_used: "Last used",
    // Filter panel
    group_last_used: "Last used",
    group_drive: "Drive",
    group_size: "Size",
    bucket_all: "All",
    bucket_over_six_months: "Over 6 months",
    bucket_over_one_year: "Over 1 year",
    bucket_never_seen: "Never seen",
    bucket_over_one_gb: "Over 1 GB",
    // Relative dates and sizes
    never_seen: "Never seen",
    pending: "…",
    today: "Today",
    yesterday: "Yesterday",
    days_ago: "days ago",
    months_ago: "mo ago",
    years_ago: "yr ago",
    mb: "MB",
    gb: "GB",
    unknown_value: "–",
    list_separator: " · ",
    // Settings menu
    settings_menu: "⚙",
    show_hidden: "Show hidden entries",
    auto_purge_days: "Empty the quarantine automatically after N days",
    auto_purge_off: "(off)",
    kept_menu: "Keep list…",
    language_menu: "Språk / Language",
    lang_sv: "Svenska",
    lang_en: "English",
    kept_title: "Keep list",
    kept_empty: "The keep list is empty",
    kept_remove: "Remove",
    undo: "Undo",
    // Details panel
    no_selection: "No program selected",
    detail_location: "Location",
    detail_uninstaller: "Uninstaller",
    detail_registry_key: "Registry key",
    other_user_suffix: " (other user)",
    detail_last_used: "Last used",
    uncertain_filetime: " – uncertain (file time)",
    detail_signed_by: "Signed by",
    detail_description: "Description",
    detail_company: "Company",
    detail_installed: "Installed",
    detail_install_source: "Source",
    signature_unsigned: "unsigned",
    signature_unknown_signer: "unknown publisher",
    signature_invalid: "invalid signature: {reason}",
    sig_bad_digest: "the file changed after signing",
    sig_expired: "the certificate has expired",
    sig_untrusted_root: "the certificate is not trusted",
    sig_revoked: "the certificate is revoked",
    sig_distrusted: "the publisher is blocked",
    sig_bad_certificate: "bad certificate",
    sig_error_code: "error {code}",
    source_winget: "Winget",
    source_store: "Microsoft Store",
    source_steam: "Steam",
    source_epic: "Epic Games",
    source_gog: "GOG",
    source_ea: "EA app",
    source_ubisoft: "Ubisoft Connect",
    source_driver: "Driver",
    source_oem: "Preinstalled (manufacturer)",
    source_msi: "Windows Installer (MSI)",
    source_unknown: "Unknown",
    col_file: "File",
    col_last_run: "Last run",
    col_source: "Source",
    col_counted: "Counted",
    counted_yes: "✓",
    counted_subcomponent: "counted",
    via: "via",
    autostart: "autostart",
    excluded_name_pattern: "updater or service name",
    excluded_service: "service",
    excluded_scheduled_task: "scheduled task",
    excluded_not_main: "not the main program",
    source_prefetch: "Prefetch",
    source_bam: "BAM",
    source_userassist: "UserAssist",
    source_filetime: "file time",
    // Uninstall queue dialog
    uninstall_thread_failed: "Could not start the uninstall",
    queue_aborted: "The uninstall stopped unexpectedly – see the log file",
    queue_title: "Uninstall",
    other_account_queue: "Programs from other accounts are uninstalled with your administrator rights; check the uninstaller's path below.",
    unknown_account: "unknown account",
    opt_restore_point: "Create a restore point for the whole batch",
    opt_run_vendor: "Run the programs' own uninstallers",
    cancel: "Cancel",
    start: "Start",
    dependency_badge: "used by other programs",
    dependency_warning: "Other programs probably rely on this. Only uninstall it if you know nothing needs it.",
    dependency_understand: "I understand",
    dependency_runtime: "runtime",
    dependency_sdk: "SDK",
    dependency_toolchain: "developer tool",
    step_restore_point: "Restore point",
    step_vendor: "Program's uninstaller",
    step_leftovers: "Leftovers",
    step_done: "Done",
    step_separator: "·",
    restore_point_label: "Restore point",
    rp_created: "created",
    rp_skipped: "skipped (one was created recently)",
    rp_disabled: "System Restore is turned off",
    rp_failed: "failed",
    state_pending: "Waiting",
    state_running: "Running",
    state_uninstalled: "Uninstalled",
    state_forced: "Force-removed",
    state_not_uninstalled: "Not uninstalled",
    exit_code: "code",
    state_skipped: "Skipped",
    stop_waiting: "Stop waiting",
    stop_waiting_tip: "Continue now, without waiting for the uninstaller's remaining processes",
    show_leftovers: "Show leftovers",
    retry: "Try again",
    close: "Close",
    // Leftovers view
    leftovers_title: "Leftovers",
    back_to_programs: "Back to programs",
    no_leftovers: "No leftovers found.",
    section_files: "Folders and files",
    section_registry: "Registry keys",
    section_other: "Other",
    kind_dir: "Folder",
    kind_file: "File",
    kind_regkey: "Key",
    kind_regvalue: "Value",
    kind_shortcut: "Shortcut",
    kind_service: "Service",
    kind_task: "Scheduled task",
    warning_icon: "⚠",
    note_user_folder: "probably your own files",
    note_shared_publisher: "shared with other programs from the same publisher",
    note_other_program: "used by another installed program",
    note_contains_user_data: "probably contains your own files",
    note_suffix_match: "the name is the program's name with a suffix",
    note_progid_of_program: "a file type named after the program",
    note_appdata_only: "only folders in AppData",
    note_data_only: "only data folders (ProgramData/AppData)",
    note_other_profile: "in another user's profile",
    note_first_word: "the name is the first word of an installed program's name",
    note_word_of_installed: "the name is part of an installed program's name",
    note_sibling_owned: "the publisher folder holds an installed program",
    note_publisher_of_installed: "the name is the publisher of an installed program",
    note_in_use: "used by a service, task, autostart or shortcut",
    note_launcher_library: "belongs to a game library",
    mark_safe: "Check safe items",
    remove_checked: "Remove checked",
    service_reboot: "Services are fully removed only after a restart",
    quarantine_thread_failed: "Could not start the removal",
    quarantine_aborted: "The removal stopped unexpectedly – see the log file",
    insufficient_space: "Not enough free space on {X}:",
    continue_button: "Continue",
    // Standalone leftover search
    search_title: "Find leftovers",
    search_name: "Program name",
    search_dir: "Folder (optional)",
    search: "Search",
    searching: "Searching for leftovers…",
    leftover_scan_thread_failed: "Could not start the leftover search",
    leftover_scan_aborted: "The leftover search stopped unexpectedly – see the log file",
    // Ghost programs view
    ghosts: "Ghost programs",
    ghosts_title: "Ghost programs",
    ghosts_scanning: "Looking for leftovers of uninstalled programs…",
    ghosts_empty: "No ghost programs found",
    ghosts_not_scanned: "No search yet – press Refresh",
    ghosts_need_programs: "The program list is empty – run Refresh before searching for ghost programs",
    ghost_scan_thread_failed: "Could not start the ghost program search",
    ghost_scan_aborted: "The ghost program search stopped unexpectedly – see the log file",
    assess_probably: "Probably uninstalled",
    assess_leftovers: "Leftovers only",
    assess_uncertain: "Uncertain",
    keep: "Keep",
    keep_save_failed: "Could not save the list of kept programs",
    keep_tip: "Never show this program, its folders or keys again",
    show_ghost_leftovers: "Show leftovers of checked programs",
    back: "Back",
    ref_run: "Autostart",
    ref_app_paths: "App Paths",
    ref_classes_server: "COM component",
    ref_shell_command: "File type",
    ref_service: "Service",
    ref_task: "Scheduled task",
    ref_shortcut: "Shortcut",
    // Quarantine view
    quarantine: "Quarantine",
    quarantine_title: "Quarantine",
    quarantine_empty: "The quarantine is empty",
    purge_all: "Empty quarantine",
    purge: "Empty quarantine",
    col_program: "Program",
    col_date: "Date",
    col_status: "Status",
    status_ok: "ok",
    restore: "Restore",
    purge_one: "Delete permanently",
    restore_partial: "Partly restored – the following could not be restored:",
    quarantine_entry_missing: "The quarantine entry no longer exists",
    purge_failed: "Could not delete the quarantine entry permanently",
    quarantine_admin_thread_failed: "Could not start the quarantine action",
    quarantine_admin_aborted: "The quarantine action stopped unexpectedly – see the log file",
    purge_one_confirm_button: "Delete",
    unknown_content_left: "Unknown content left in the quarantine folder – left untouched",
    drive_missing: "Drive {X}: is not connected – the entry is kept",
    code_occupied: "the original exists again",
    code_acl_not_reinherited: "permissions could not be inherited from the parent folder",
    code_reparse_in_destination: "a link in the destination path",
    code_reparse_in_quarantine_path: "a link in the quarantine path",
    code_partial_move: "only partly moved",
    code_manifest_save_failed: "the quarantine entry could not be saved",
    code_not_storable: "the path cannot be stored in quarantine",
    code_stored_outside: "stored outside the quarantine folder",
    code_backup_outside: "the backup is outside the quarantine folder",
    code_backup_is_link: "the backup is a link",
    code_not_trusted: "the quarantine folder is not owned by Administrators or SYSTEM",
    code_invalid_id: "invalid quarantine id",
    code_is_reparse: "a link or reparse point",
    code_destination_exists: "the destination already exists",
    code_locked: "in use by another program",
    code_link_in_tree: "a link in the folder tree",
};

/// A kept program name in the "Behållna" window.
pub fn kept_name(name: &str) -> String {
    match lang() {
        Lang::Sv => format!("Program: {name}"),
        Lang::En => format!("Program: {name}"),
    }
}

/// A kept folder in the "Behållna" window.
pub fn kept_path(path: &str) -> String {
    match lang() {
        Lang::Sv => format!("Mapp: {path}"),
        Lang::En => format!("Folder: {path}"),
    }
}

/// A kept registry key in the "Behållna" window.
pub fn kept_key(key: &str) -> String {
    match lang() {
        Lang::Sv => format!("Registernyckel: {key}"),
        Lang::En => format!("Registry key: {key}"),
    }
}

/// The one-line notice after "Behåll", next to "Ångra".
pub fn kept_notice(name: &str) -> String {
    match lang() {
        Lang::Sv => format!("{name} behålls och visas inte längre."),
        Lang::En => format!("{name} is kept and no longer shown."),
    }
}

/// Start-up warning: the elevated token's account differs from the logged-in user.
pub fn other_account_warning(session: &str, token: &str) -> String {
    match lang() {
        Lang::Sv => format!("Du kör som {token} men är inloggad som {session}: per-användarprogram och AppData avser {token}."),
        Lang::En => format!("You are running as {token} but signed in as {session}: per-user programs and AppData are those of {token}."),
    }
}

/// A queue item belonging to another account; `account` is `None` when it could not be resolved.
pub fn other_account_item(name: &str, account: Option<&str>) -> String {
    let account = account.unwrap_or(t().unknown_account);
    match lang() {
        Lang::Sv => format!("{name} (konto: {account})"),
        Lang::En => format!("{name} (account: {account})"),
    }
}

/// A queued program without a runnable uninstaller: "Foo: ingen avinstallerare hittades, tvingad borttagning".
pub fn no_uninstaller(name: &str) -> String {
    match lang() {
        Lang::Sv => format!("{name}: ingen avinstallerare hittades, tvingad borttagning"),
        Lang::En => format!("{name}: no uninstaller found, forced removal"),
    }
}

/// "E: saknar NTFS – 30 MB kopieras till C:" in the removal confirmation.
pub fn fallback_notice(from: char, size: &str) -> String {
    match lang() {
        Lang::Sv => format!("{from}: saknar NTFS – {size} kopieras till C:"),
        Lang::En => format!("{from}: is not NTFS – {size} will be copied to C:"),
    }
}

/// `insufficient_space` for `drive`.
pub fn insufficient_space(drive: char) -> String {
    t().insufficient_space.replace("{X}", &drive.to_string())
}

/// `singular` for 1, else `plural` (English counts).
fn plural(n: usize, singular: &'static str, plural: &'static str) -> &'static str {
    if n == 1 { singular } else { plural }
}

/// "(3 träffar, 58 MB)" after a section title.
pub fn section_counts(hits: usize, size: &str) -> String {
    section_counts_in(lang(), hits, size)
}

/// [`section_counts`] in `lang`.
pub fn section_counts_in(lang: Lang, hits: usize, size: &str) -> String {
    match lang {
        Lang::Sv => format!("({hits} träffar, {size})"),
        Lang::En => format!("({hits} {}, {size})", plural(hits, "hit", "hits")),
    }
}

/// A run count in the details table: "5 körningar" / "5 runs".
pub fn run_count(n: u32) -> String {
    run_count_in(lang(), n)
}

/// [`run_count`] in `lang`.
pub fn run_count_in(lang: Lang, n: u32) -> String {
    match lang {
        Lang::Sv => format!("{n} körningar"),
        Lang::En => format!("{n} {}", plural(n as usize, "run", "runs")),
    }
}

/// The leftovers view's footer: "3 av 5 markerade · 58 MB frigörs".
pub fn checked_summary(checked: usize, total: usize, size: &str) -> String {
    match lang() {
        Lang::Sv => format!("{checked} av {total} markerade · {size} frigörs"),
        Lang::En => format!("{checked} of {total} checked · {size} to free"),
    }
}

/// "4 objekt flyttade till karantän".
pub fn moved_to_quarantine(count: usize) -> String {
    moved_to_quarantine_in(lang(), count)
}

/// [`moved_to_quarantine`] in `lang`.
pub fn moved_to_quarantine_in(lang: Lang, count: usize) -> String {
    match lang {
        Lang::Sv => format!("{count} objekt flyttade till karantän"),
        Lang::En => format!("{count} {} moved to quarantine", plural(count, "item", "items")),
    }
}

/// The section of ghosts that were probably uninstalled (checked by default).
pub fn probably_section(count: usize) -> String {
    match lang() {
        Lang::Sv => format!("Troligen avinstallerade ({count})"),
        Lang::En => format!("Probably uninstalled ({count})"),
    }
}

/// The open section of leftovers without an installation (unchecked by default).
pub fn leftovers_section(count: usize) -> String {
    match lang() {
        Lang::Sv => format!("Rester utan installation (bocka i själv) ({count})"),
        Lang::En => format!("Leftovers only (check them yourself) ({count})"),
    }
}

/// The collapsed section of uncertain ghosts: "Osäkra (3)".
pub fn uncertain_section(count: usize) -> String {
    match lang() {
        Lang::Sv => format!("Osäkra ({count})"),
        Lang::En => format!("Uncertain ({count})"),
    }
}

/// The header of a ghost's evidence list: "Spår (4)".
pub fn evidence_header(count: usize) -> String {
    match lang() {
        Lang::Sv => format!("Spår ({count})"),
        Lang::En => format!("Traces ({count})"),
    }
}

/// The ghosts view's footer: "2 av 5 markerade · 58 MB".
pub fn ghost_summary(checked: usize, total: usize, size: &str) -> String {
    match lang() {
        Lang::Sv => format!("{checked} av {total} markerade · {size}"),
        Lang::En => format!("{checked} of {total} checked · {size}"),
    }
}

/// An orphan folder: "Mapp: C:\Program Files\Foo (38 MB)".
pub fn evidence_folder(path: &str, size: &str) -> String {
    match lang() {
        Lang::Sv => format!("Mapp: {path} ({size})"),
        Lang::En => format!("Folder: {path} ({size})"),
    }
}

/// A key whose path value points at a missing file: "Nyckel: HKLM\… (pekar på saknad D:\Foo)".
pub fn evidence_key_missing_path(key: &str, path: &str) -> String {
    match lang() {
        Lang::Sv => format!("Nyckel: {key} (pekar på saknad {path})"),
        Lang::En => format!("Key: {key} (points to missing {path})"),
    }
}

/// A vendor key with nothing installed behind it: "Nyckel: HKCU\Software\Foo".
pub fn evidence_key(key: &str) -> String {
    match lang() {
        Lang::Sv => format!("Nyckel: {key}"),
        Lang::En => format!("Key: {key}"),
    }
}

/// The hover text for a dependency reason key from `stale_core::dependencies`.
pub fn dependency_reason_in(lang: Lang, key: &str) -> &'static str {
    let s = texts(lang);
    match key {
        "sdk" => s.dependency_sdk,
        "toolchain" => s.dependency_toolchain,
        _ => s.dependency_runtime,
    }
}

/// The text of an invalid-signature reason code from `stale_core::provenance`.
pub fn signature_reason_in(lang: Lang, code: &str) -> String {
    let s = texts(lang);
    match code {
        "bad-digest" => s.sig_bad_digest.to_string(),
        "expired" => s.sig_expired.to_string(),
        "untrusted-root" => s.sig_untrusted_root.to_string(),
        "revoked" => s.sig_revoked.to_string(),
        "distrusted" => s.sig_distrusted.to_string(),
        "bad-certificate" => s.sig_bad_certificate.to_string(),
        other => s.sig_error_code.replace("{code}", other.strip_prefix("error:").unwrap_or(other)),
    }
}

/// "Signerad av" value: `"{signer} ✓"`, `"ogiltig signatur: {reason}"`, `"osignerad"`, or "–"
/// when the exe is missing or unreadable.
pub fn signature_text_in(lang: Lang, signature: &Signature) -> String {
    let s = texts(lang);
    match signature {
        Signature::Valid { signer } if signer.trim().is_empty() => format!("{} ✓", s.signature_unknown_signer),
        Signature::Valid { signer } => format!("{signer} ✓"),
        Signature::Invalid { reason, .. } => s.signature_invalid.replace("{reason}", &signature_reason_in(lang, reason)),
        Signature::Unsigned => s.signature_unsigned.to_string(),
        Signature::Unknown => s.unknown_value.to_string(),
    }
}

/// The label of an install source.
pub fn install_source_in(lang: Lang, source: InstallSource) -> &'static str {
    let s = texts(lang);
    match source {
        InstallSource::Winget => s.source_winget,
        InstallSource::MicrosoftStore => s.source_store,
        InstallSource::Steam => s.source_steam,
        InstallSource::Epic => s.source_epic,
        InstallSource::Gog => s.source_gog,
        InstallSource::EaApp => s.source_ea,
        InstallSource::Ubisoft => s.source_ubisoft,
        InstallSource::Driver => s.source_driver,
        InstallSource::Oem => s.source_oem,
        InstallSource::Msi => s.source_msi,
        InstallSource::Unknown => s.source_unknown,
    }
}

/// A reference to a missing file: "Autostart: HKLM\…\Run → C:\Foooo.exe".
pub fn evidence_ref(kind: &str, location: &str, target: &str) -> String {
    format!("{kind}: {location} → {target}")
}

/// An execution trace: "Körd 2024-01-20 (Prefetch): C:\Foooo.exe".
pub fn evidence_usage(date: &str, source: &str, exe: &str) -> String {
    match lang() {
        Lang::Sv => format!("Körd {date} ({source}): {exe}"),
        Lang::En => format!("Last run {date} ({source}): {exe}"),
    }
}

/// The toolbar's quarantine button: "Karantän", or "Karantän (3)" when it holds anything.
pub fn quarantine_button(count: usize) -> String {
    let quarantine = t().quarantine;
    if count == 0 { quarantine.to_string() } else { format!("{quarantine} ({count})") }
}

/// The "Ta bort permanent" confirmation.
pub fn purge_one_confirm(name: &str, size: &str) -> String {
    match lang() {
        Lang::Sv => format!("Detta tar bort {name} permanent ({size}) och kan inte ångras."),
        Lang::En => format!("This deletes {name} permanently ({size}) and cannot be undone."),
    }
}

/// The "Töm karantän" confirmation.
pub fn purge_all_confirm(size: &str) -> String {
    match lang() {
        Lang::Sv => format!("Detta frigör {size} och kan inte ångras."),
        Lang::En => format!("This permanently deletes everything in quarantine ({size}) and cannot be undone."),
    }
}

/// A quarantine's status column: "2 fel".
pub fn error_count(n: usize) -> String {
    error_count_in(lang(), n)
}

/// [`error_count`] in `lang`.
pub fn error_count_in(lang: Lang, n: usize) -> String {
    match lang {
        Lang::Sv => format!("{n} fel"),
        Lang::En => format!("{n} {}", plural(n, "error", "errors")),
    }
}

/// A quarantine's status column: "1 ofullständiga" (steps started but never recorded as done).
pub fn incomplete_count(n: usize) -> String {
    match lang() {
        Lang::Sv => format!("{n} ofullständiga"),
        Lang::En => format!("{n} incomplete"),
    }
}

/// A warning from the scan, in `lang`.
pub fn scan_warning_in(lang: Lang, w: &ScanWarning) -> String {
    match (lang, w) {
        (Lang::Sv, ScanWarning::PrefetchUnreadable { skipped, total }) => {
            format!("Prefetch: {skipped} av {total} filer kunde inte läsas")
        }
        (Lang::En, ScanWarning::PrefetchUnreadable { skipped, total }) => {
            format!("Prefetch: {skipped} of {total} {} could not be read", plural(*total, "file", "files"))
        }
        (Lang::Sv, ScanWarning::SourceFailed { source, error }) => format!("{source} kunde inte läsas: {error}"),
        (Lang::En, ScanWarning::SourceFailed { source, error }) => format!("{source} could not be read: {error}"),
        (_, ScanWarning::ThreadFailed { error }) => format!("{}: {error}", texts(lang).scan_thread_failed),
    }
}

/// A warning from the scan, in the current language.
pub fn scan_warning(w: &ScanWarning) -> String {
    scan_warning_in(lang(), w)
}

/// The text of a core error code (see `stale_core::quarantine::codes`), `None` for other text.
fn code_text(code: &str) -> Option<&'static str> {
    let s = t();
    Some(match code {
        codes::UNKNOWN_CONTENT_LEFT => s.unknown_content_left,
        codes::OCCUPIED => s.code_occupied,
        codes::ACL_NOT_REINHERITED => s.code_acl_not_reinherited,
        codes::REPARSE_IN_DESTINATION => s.code_reparse_in_destination,
        codes::REPARSE_IN_QUARANTINE_PATH => s.code_reparse_in_quarantine_path,
        codes::PARTIAL_MOVE => s.code_partial_move,
        codes::MANIFEST_SAVE_FAILED => s.code_manifest_save_failed,
        codes::NOT_STORABLE => s.code_not_storable,
        codes::STORED_OUTSIDE => s.code_stored_outside,
        codes::BACKUP_OUTSIDE => s.code_backup_outside,
        codes::BACKUP_IS_LINK => s.code_backup_is_link,
        codes::NOT_TRUSTED => s.code_not_trusted,
        codes::INVALID_ID => s.code_invalid_id,
        codes::IS_REPARSE => s.code_is_reparse,
        codes::DESTINATION_EXISTS => s.code_destination_exists,
        codes::LOCKED => s.code_locked,
        codes::LINK_IN_TREE => s.code_link_in_tree,
        _ => return None,
    })
}

/// `drive_missing` for `"E:"`.
fn drive_missing(drive: &str) -> String {
    t().drive_missing.replace("{X}", drive.trim().trim_end_matches(':'))
}

/// A core error (a quarantine `Failure::error`): `"<code>"` or `"<code>: <detail>"` with a known
/// code becomes its text (followed by the detail); other text is shown as it is.
pub fn core_error(error: &str) -> String {
    let (code, detail) = match error.split_once(": ") {
        Some((code, detail)) => (code, Some(detail)),
        None => (error, None),
    };
    if code == codes::DRIVE_MISSING {
        if let Some(drive) = detail {
            return drive_missing(drive);
        }
    }
    match (code_text(code), detail) {
        (Some(text), Some(detail)) => format!("{text}: {detail}"),
        (Some(text), None) => text.to_string(),
        (None, _) => error.to_string(),
    }
}

/// A purge error: each `"; "`-separated `"<code>: <what>"` part with a known code becomes
/// `"<what>: <text>"` (`"drive-missing: X:"` becomes `drive_missing`); other parts stay as
/// they are.
pub fn purge_error(e: &str) -> String {
    e.split("; ")
        .map(|part| match part.split_once(": ") {
            Some((code, drive)) if code == codes::DRIVE_MISSING => drive_missing(drive),
            Some((code, what)) => match code_text(code) {
                Some(text) => format!("{what}: {text}"),
                None => part.to_string(),
            },
            None => part.to_string(),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    // The language is process-wide and tests run in parallel: no test here switches it. The
    // default is Swedish; English is checked through the `_in`/`texts` variants.

    #[test]
    fn purge_error_translates_known_codes() {
        assert_eq!(
            purge_error(r"unknown-content-left: C:\Stale Uninstaller\quarantine\x; access denied"),
            format!(r"C:\Stale Uninstaller\quarantine\x: {}; access denied", SV.unknown_content_left)
        );
        assert_eq!(purge_error("drive-missing: E:"), "Disken E: är inte ansluten – posten behålls");
        assert_eq!(core_error("unknown-content-left"), SV.unknown_content_left);
        assert_eq!(core_error("something else"), "something else");
        assert_eq!(incomplete_count(2), "2 ofullständiga");
    }

    #[test]
    fn core_error_translates_codes_and_keeps_details() {
        assert_eq!(core_error("occupied"), "originalet finns igen");
        assert_eq!(core_error(r"locked: C:\Apps\Foo\a.dll"), r"används av ett annat program: C:\Apps\Foo\a.dll");
        assert_eq!(core_error("drive-missing: E:"), "Disken E: är inte ansluten – posten behålls");
        assert_eq!(core_error(r"remove C:\x: access denied"), r"remove C:\x: access denied");
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

    #[test]
    fn detect_lang_maps_swedish_and_falls_back_to_english() {
        assert_eq!(lang_for_langid(0x041D), Lang::Sv);
        assert_eq!(lang_for_langid(0x081D), Lang::Sv);
        assert_eq!(lang_for_langid(0x0409), Lang::En);
        assert_eq!(lang_for_langid(0x0407), Lang::En);
    }

    #[test]
    fn startup_lang_prefers_the_saved_language() {
        assert_eq!(startup_lang(Some("sv")), Lang::Sv);
        assert_eq!(startup_lang(Some("en")), Lang::En);
        assert_eq!(Lang::from_code(Lang::En.code()), Some(Lang::En));
        assert_eq!(Lang::from_code(Lang::Sv.code()), Some(Lang::Sv));
        assert_eq!(Lang::from_code("de"), None);
    }

    #[test]
    fn prefetch_warning_renders_in_both_languages() {
        let w = ScanWarning::PrefetchUnreadable { skipped: 2, total: 9 };
        assert_eq!(scan_warning_in(Lang::Sv, &w), "Prefetch: 2 av 9 filer kunde inte läsas");
        assert_eq!(scan_warning_in(Lang::En, &w), "Prefetch: 2 of 9 files could not be read");
        let failed = ScanWarning::SourceFailed { source: "BAM".into(), error: "boom".into() };
        assert_eq!(scan_warning_in(Lang::Sv, &failed), "BAM kunde inte läsas: boom");
        assert_eq!(scan_warning_in(Lang::En, &failed), "BAM could not be read: boom");
        let thread = ScanWarning::ThreadFailed { error: "x".into() };
        assert_eq!(scan_warning_in(Lang::En, &thread), "Could not start the scan: x");
    }

    #[test]
    fn english_counts_use_the_singular_for_one() {
        assert_eq!(error_count_in(Lang::En, 1), "1 error");
        assert_eq!(error_count_in(Lang::En, 2), "2 errors");
        assert_eq!(moved_to_quarantine_in(Lang::En, 1), "1 item moved to quarantine");
        assert_eq!(moved_to_quarantine_in(Lang::En, 4), "4 items moved to quarantine");
        assert_eq!(section_counts_in(Lang::En, 1, "3 MB"), "(1 hit, 3 MB)");
        assert_eq!(section_counts_in(Lang::En, 3, "3 MB"), "(3 hits, 3 MB)");
        assert_eq!(run_count_in(Lang::En, 1), "1 run");
        assert_eq!(run_count_in(Lang::En, 5), "5 runs");
        assert_eq!(run_count_in(Lang::Sv, 5), "5 körningar");
        let one = ScanWarning::PrefetchUnreadable { skipped: 1, total: 1 };
        assert_eq!(scan_warning_in(Lang::En, &one), "Prefetch: 1 of 1 file could not be read");
        assert_eq!(error_count_in(Lang::Sv, 1), "1 fel");
        assert_eq!(moved_to_quarantine_in(Lang::Sv, 1), "1 objekt flyttade till karantän");
    }

    #[test]
    fn language_menu_labels_are_the_same_in_both_languages() {
        for s in [&SV, &EN] {
            assert_eq!(s.language_menu, "Språk / Language");
            assert_eq!(s.lang_sv, "Svenska");
            assert_eq!(s.lang_en, "English");
        }
        assert_eq!(texts(Lang::En).uninstall, "Uninstall");
        assert_eq!(texts(Lang::Sv).uninstall, "Avinstallera");
    }
}
