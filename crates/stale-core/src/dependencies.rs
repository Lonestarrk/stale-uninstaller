//! Recognises runtimes, SDKs and developer tools that other programs usually rely on, so the
//! GUI can ask for an extra confirmation before one is uninstalled.

use crate::Program;

/// Reason keys, in precedence order: a name matching several takes the first.
const PATTERNS: &[(&str, &[&str])] = &[
    (
        "sdk",
        &["Microsoft .NET SDK*", "Windows Software Development Kit*", "Windows SDK*", "*JDK*", "Java*Development Kit*"],
    ),
    (
        "runtime",
        &[
            "Microsoft Visual C++ * Redistributable*",
            "Microsoft .NET*Runtime*",
            "Microsoft Windows Desktop Runtime*",
            "Microsoft ASP.NET Core*",
            "Microsoft Edge WebView2 Runtime",
            "Java*",
            "*JRE*",
            "DirectX*",
            "Microsoft XNA*",
            "PhysX*",
            "Vulkan Run Time*",
            "OpenAL",
        ],
    ),
    (
        "toolchain",
        &["Python", "Python *", "Node.js", "Git", "Git version *", "Visual Studio Build Tools*", "Microsoft Visual Studio Installer"],
    ),
];

/// A reason key (`"runtime"`, `"sdk"` or `"toolchain"`) when `p` is something other programs
/// commonly depend on.
pub fn is_shared_dependency(p: &Program) -> Option<&'static str> {
    let name = p.name.trim().to_lowercase();
    PATTERNS
        .iter()
        .find(|(_, pats)| pats.iter().any(|pat| glob(&pat.to_lowercase(), &name)))
        .map(|(key, _)| *key)
}

/// `*` matches any run of characters; everything else matches itself. Both sides are lowercase.
fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some(s) = star {
            mark += 1;
            ti = mark;
            pi = s + 1;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Hive, ProgramId, RegKeyPath, Scope};

    fn prog(name: &str) -> Program {
        Program {
            account: None,
            id: ProgramId(name.into()),
            key: RegKeyPath { hive: Hive::LocalMachine, path: name.into() },
            name: name.into(),
            publisher: None,
            version: None,
            install_location: None,
            install_date: None,
            uninstall_string: None,
            quiet_uninstall_string: None,
            display_icon: None,
            is_msi: false,
            estimated_size_bytes: None,
            drive: Some('C'),
            hidden: false,
            scope: Scope::Machine,
        }
    }

    fn reason(name: &str) -> Option<&'static str> {
        is_shared_dependency(&prog(name))
    }

    #[test]
    fn vc_redist_is_runtime() {
        assert_eq!(reason("Microsoft Visual C++ 2015-2022 Redistributable (x64) - 14.38.33135"), Some("runtime"));
        assert_eq!(reason("microsoft visual c++ 2013 redistributable (x86)"), Some("runtime"));
        assert_eq!(reason("Microsoft .NET Runtime - 8.0.1 (x64)"), Some("runtime"));
        assert_eq!(reason("Microsoft Windows Desktop Runtime - 8.0.1 (x64)"), Some("runtime"));
        assert_eq!(reason("Microsoft Edge WebView2 Runtime"), Some("runtime"));
        assert_eq!(reason("Java 8 Update 391"), Some("runtime"));
        assert_eq!(reason("OpenAL"), Some("runtime"));
    }

    #[test]
    fn dotnet_sdk_is_sdk() {
        assert_eq!(reason("Microsoft .NET SDK 8.0.100 (x64)"), Some("sdk"));
        assert_eq!(reason("Windows Software Development Kit - Windows 10.0.22621"), Some("sdk"));
        // Both Java* and *JDK* match: sdk wins.
        assert_eq!(reason("Java(TM) SE Development Kit 17"), Some("sdk"));
        assert_eq!(reason("Eclipse Temurin JDK with Hotspot 17.0.9"), Some("sdk"));
    }

    #[test]
    fn git_and_python_are_toolchains() {
        assert_eq!(reason("Git"), Some("toolchain"));
        assert_eq!(reason("Git version 2.43.0"), Some("toolchain"));
        assert_eq!(reason("Git Extensions 4.2.1"), None);
        assert_eq!(reason("Node.js"), Some("toolchain"));
        assert_eq!(reason("Python 3.12.1 (64-bit)"), Some("toolchain"));
        assert_eq!(reason("Visual Studio Build Tools 2022"), Some("toolchain"));
        assert_eq!(reason("Microsoft Visual Studio Installer"), Some("toolchain"));
    }

    #[test]
    fn ordinary_program_is_not_dependency() {
        for n in ["Discord", "7-Zip 23.01 (x64)", "GitHub Desktop", "Notepad++ (64-bit x64)", "Obsidian", ""] {
            assert_eq!(reason(n), None, "{n}");
        }
    }
}
