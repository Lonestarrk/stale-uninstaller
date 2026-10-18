//! Services, scheduled tasks and shortcuts that point into a program's install folder.

use super::{Confidence, Leftover, LeftoverKind, ProgramHint};
use crate::fsys::FsReader;
use crate::model::Hive;
use crate::pathutil::{exe_under, expand_env, norm};
use crate::registry::{key, RegValue, RegistryReader};
use crate::services::{normalize_image_path, SERVICES_ROOT};
use crate::shortcuts::ShortcutTarget;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The install folder to match against ([`ProgramHint::effective_install_dir`]); `None` when
/// unknown or too broad to claim anything.
fn install_dir(hint: &ProgramHint) -> Option<PathBuf> {
    hint.effective_install_dir()
}

fn high(kind: LeftoverKind) -> Leftover {
    Leftover { kind, confidence: Confidence::High, note: None }
}

/// Services and drivers whose `ImagePath` lies inside the install folder.
pub fn scan_services(reg: &dyn RegistryReader, hint: &ProgramHint) -> Vec<Leftover> {
    let Some(install) = install_dir(hint) else { return Vec::new() };
    let Ok(names) = reg.subkeys(&key(Hive::LocalMachine, SERVICES_ROOT)) else { return Vec::new() };
    names
        .into_iter()
        .filter_map(|name| {
            let values = reg.values(&key(Hive::LocalMachine, &format!(r"{SERVICES_ROOT}\{name}"))).ok()?;
            let raw = values.into_iter().find_map(|(n, v)| match v {
                RegValue::Str(s) | RegValue::ExpandStr(s) if n.eq_ignore_ascii_case("ImagePath") => Some(s),
                _ => None,
            })?;
            let image = normalize_image_path(&raw).filter(|p| exe_under(p, &install))?;
            Some(high(LeftoverKind::Service { name, image }))
        })
        .collect()
}

fn unescape_xml(s: &str) -> String {
    // `&amp;` last so `&amp;lt;` becomes `&lt;`, not `<`.
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// Every `<Command>` text in a task definition, trimmed, env-expanded and unquoted.
pub(crate) fn task_commands(xml: &str) -> Vec<PathBuf> {
    const OPEN: &str = "<Command>";
    const CLOSE: &str = "</Command>";
    let mut out = Vec::new();
    let mut rest = xml.trim_start_matches('\u{feff}');
    while let Some(start) = rest.find(OPEN) {
        let after = &rest[start + OPEN.len()..];
        let Some(end) = after.find(CLOSE) else { break };
        let text = expand_env(&unescape_xml(after[..end].trim()));
        let text = text.trim().trim_matches('"').trim();
        if !text.is_empty() {
            out.push(PathBuf::from(text));
        }
        rest = &after[end + CLOSE.len()..];
    }
    out
}

pub(crate) fn task_files(fs: &dyn FsReader, dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs.list_dir(dir) {
        if entry.is_dir {
            task_files(fs, &entry.path, out);
        } else {
            out.push(entry.path);
        }
    }
}

/// `norm()`ed command of every scheduled-task action (usage matching must not count these).
pub fn task_exes(fs: &dyn FsReader, tasks_root: &Path) -> HashSet<String> {
    let mut files = Vec::new();
    task_files(fs, tasks_root, &mut files);
    files
        .iter()
        .filter_map(|f| fs.read_text(f))
        .flat_map(|xml| task_commands(&xml))
        .map(|c| norm(&c))
        .collect()
}

/// Scheduled tasks (one leftover per task file) with a command inside the install folder.
pub fn scan_tasks(fs: &dyn FsReader, tasks_root: &Path, hint: &ProgramHint) -> Vec<Leftover> {
    let Some(install) = install_dir(hint) else { return Vec::new() };
    let mut files = Vec::new();
    task_files(fs, tasks_root, &mut files);
    files
        .into_iter()
        .filter_map(|file| {
            let command = task_commands(&fs.read_text(&file)?).into_iter().find(|c| exe_under(c, &install))?;
            let rel = file.strip_prefix(tasks_root).ok()?;
            let name = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join(r"\");
            Some(high(LeftoverKind::ScheduledTask { name, xml: file, command }))
        })
        .collect()
}

/// Shortcuts (Start Menu and desktops) whose target lies inside the install folder.
pub fn scan_shortcuts(targets: &[ShortcutTarget], desktop_dirs_targets: &[ShortcutTarget], hint: &ProgramHint) -> Vec<Leftover> {
    let Some(install) = install_dir(hint) else { return Vec::new() };
    targets
        .iter()
        .chain(desktop_dirs_targets)
        .filter(|t| exe_under(&t.path, &install))
        .map(|t| high(LeftoverKind::Shortcut { lnk: t.lnk.clone(), target: t.path.clone() }))
        .collect()
}

/// `%PUBLIC%\Desktop` and each profile's `Desktop`, where they exist.
pub fn desktop_dirs(fs: &dyn FsReader) -> Vec<PathBuf> {
    let public = fs.env("PUBLIC").map(|p| PathBuf::from(p).join("Desktop"));
    public
        .into_iter()
        .chain(fs.profile_dirs().into_iter().map(|p| p.join("Desktop")))
        .filter(|d| fs.is_dir(d))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsys::FakeFs;
    use crate::registry::FakeRegistry;

    fn hint(dir: Option<&str>) -> ProgramHint {
        ProgramHint::manual("Foo", dir.map(PathBuf::from))
    }

    #[test]
    fn service_with_image_under_install_is_high() {
        let mut reg = FakeRegistry::new();
        let svc = |n: &str| key(Hive::LocalMachine, &format!(r"{SERVICES_ROOT}\{n}"));
        reg.set(svc("FooSvc"), "ImagePath", RegValue::ExpandStr(r"\??\C:\Apps\Foo\svc.exe".into()));
        reg.set(svc("Other"), "ImagePath", RegValue::Str(r"C:\Apps\Bar\svc.exe".into()));
        reg.add_key(svc("NoImage"));
        let out = scan_services(&reg, &hint(Some(r"C:\Apps\Foo")));
        assert_eq!(
            out,
            vec![Leftover {
                kind: LeftoverKind::Service { name: "FooSvc".into(), image: PathBuf::from(r"C:\Apps\Foo\svc.exe") },
                confidence: Confidence::High,
                note: None
            }]
        );
        assert!(scan_services(&reg, &hint(None)).is_empty());
        assert!(scan_services(&reg, &hint(Some(r"C:\Program Files"))).is_empty());
    }

    #[test]
    fn task_exes_collects_every_command_normalized() {
        let root = r"C:\Windows\System32\Tasks";
        let fs = FakeFs::new()
            .text(&format!(r"{root}\A\One"), r#"<Command>"C:\Apps\Foo\a.exe"</Command><Command>C:\X\b.EXE</Command>"#)
            .text(&format!(r"{root}\Two"), r"<Command>C:\Apps\Foo\c.exe</Command>");
        let got = task_exes(&fs, Path::new(root));
        assert_eq!(got.len(), 3);
        assert!(got.contains(&norm(Path::new(r"C:\x\B.exe"))));
    }

    #[test]
    fn task_xml_command_under_install_is_high() {
        let root = r"C:\Windows\System32\Tasks";
        let xml = r#"<Task><Actions><Exec><Command>C:\Other\x.exe</Command></Exec><Exec><Command>"C:\Apps\Foo\updater.exe"</Command></Exec><Exec><Command>C:\Apps\Foo\b.exe</Command></Exec></Actions></Task>"#;
        let fs = FakeFs::new()
            .text(&format!(r"{root}\Foo\Updater"), xml)
            .text(&format!(r"{root}\Unrelated"), r"<Command>C:\Elsewhere\y.exe</Command>");
        let out = scan_tasks(&fs, Path::new(root), &hint(Some(r"C:\Apps\Foo")));
        assert_eq!(
            out,
            vec![Leftover {
                kind: LeftoverKind::ScheduledTask {
                    name: r"Foo\Updater".into(),
                    xml: PathBuf::from(format!(r"{root}\Foo\Updater")),
                    command: PathBuf::from(r"C:\Apps\Foo\updater.exe"),
                },
                confidence: Confidence::High,
                note: None
            }]
        );
        assert!(scan_tasks(&fs, Path::new(root), &hint(None)).is_empty());
    }

    #[test]
    fn task_command_with_xml_entities() {
        let root = r"C:\Windows\System32\Tasks";
        let xml = "\u{feff}<Command>&quot;C:\\Apps\\Foo &amp; Co\\x.exe&quot;</Command>";
        let fs = FakeFs::new().text(&format!(r"{root}\T"), xml);
        let out = scan_tasks(&fs, Path::new(root), &hint(Some(r"C:\Apps\Foo & Co")));
        assert!(matches!(&out[0].kind, LeftoverKind::ScheduledTask { command, .. } if command == Path::new(r"C:\Apps\Foo & Co\x.exe")), "{out:?}");
    }

    #[test]
    fn task_with_env_var_command_matches() {
        let pf = std::env::var("ProgramFiles").unwrap();
        let root = r"C:\Windows\System32\Tasks";
        let fs = FakeFs::new().text(&format!(r"{root}\FooTask"), r"<Command>%ProgramFiles%\Foo\x.exe</Command>");
        let out = scan_tasks(&fs, Path::new(root), &hint(Some(&format!(r"{pf}\Foo"))));
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(matches!(&out[0].kind, LeftoverKind::ScheduledTask { name, .. } if name == "FooTask"));
    }

    #[test]
    fn shortcut_into_install_is_high_and_others_ignored() {
        let t = |lnk: &str, path: &str| ShortcutTarget { lnk: lnk.into(), ..ShortcutTarget::new(path) };
        let start = [t(r"C:\SM\Foo.lnk", r"C:\Apps\Foo\foo.exe"), t(r"C:\SM\Bar.lnk", r"C:\Apps\Bar\bar.exe")];
        let desk = [t(r"C:\Users\A\Desktop\Foo.lnk", r"C:\Apps\Foo\sub\foo2.exe")];
        let out = scan_shortcuts(&start, &desk, &hint(Some(r"C:\Apps\Foo")));
        let want = |lnk: &str, target: &str| Leftover {
            kind: LeftoverKind::Shortcut { lnk: lnk.into(), target: target.into() },
            confidence: Confidence::High,
            note: None,
        };
        assert_eq!(out, vec![want(r"C:\SM\Foo.lnk", r"C:\Apps\Foo\foo.exe"), want(r"C:\Users\A\Desktop\Foo.lnk", r"C:\Apps\Foo\sub\foo2.exe")]);
        assert!(scan_shortcuts(&start, &desk, &hint(None)).is_empty());
    }

    #[test]
    fn hint_without_install_location_uses_the_uninstaller_folder() {
        let dir = r"C:\Users\A\AppData\Local\Programs\Obsidian";
        let mut h = ProgramHint::manual("Obsidian", None);
        h.uninstall_string = Some(format!(r#""{dir}\Uninstall Obsidian.exe" /currentuser"#));
        let t = |lnk: &str, path: &str| ShortcutTarget { lnk: lnk.into(), ..ShortcutTarget::new(path) };
        let start = [t(r"C:\SM\Obsidian.lnk", &format!(r"{dir}\Obsidian.exe"))];
        let desk = [t(r"C:\Users\A\Desktop\Obsidian.lnk", &format!(r"{dir}\Obsidian.exe"))];
        assert_eq!(scan_shortcuts(&start, &desk, &h).len(), 2);

        let mut reg = FakeRegistry::new();
        let svc = key(Hive::LocalMachine, &format!(r"{SERVICES_ROOT}\ObsSvc"));
        reg.set(svc, "ImagePath", RegValue::Str(format!(r"{dir}\svc.exe")));
        assert_eq!(scan_services(&reg, &h).len(), 1);

        let root = r"C:\Windows\System32\Tasks";
        let fs = FakeFs::new().text(&format!(r"{root}\ObsUpdate"), &format!(r"<Command>{dir}\Obsidian.exe</Command>"));
        assert_eq!(scan_tasks(&fs, Path::new(root), &h).len(), 1);
    }

    #[test]
    fn shortcut_target_carries_lnk_path() {
        let t = ShortcutTarget { lnk: PathBuf::from(r"C:\SM\Foo.lnk"), ..ShortcutTarget::new(r"C:\Apps\Foo\foo.exe") };
        let out = scan_shortcuts(&[t], &[], &hint(Some(r"C:\Apps\Foo")));
        assert!(matches!(&out[0].kind, LeftoverKind::Shortcut { lnk, .. } if lnk == Path::new(r"C:\SM\Foo.lnk")));
    }

    #[test]
    fn desktop_dirs_lists_existing_public_and_profile_desktops() {
        let fs = FakeFs::new()
            .env("PUBLIC", r"C:\Users\Public")
            .dir(r"C:\Users\Public\Desktop", 0)
            .dir(r"C:\Users\A\Desktop", 0)
            .dir(r"C:\Users\B", 0)
            .profile(r"C:\Users\A")
            .profile(r"C:\Users\B");
        assert_eq!(desktop_dirs(&fs), vec![PathBuf::from(r"C:\Users\Public\Desktop"), PathBuf::from(r"C:\Users\A\Desktop")]);
    }
}
