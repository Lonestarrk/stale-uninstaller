//! Pure name-matching rules shared by all leftover scanners.

pub const GENERIC_NAMES: [&str; 14] = [
    "tools", "setup", "update", "updater", "common", "program", "app", "software", "microsoft",
    "windows", "temp", "data", "local", "roaming",
];

const LEGAL_SUFFIXES: [&str; 11] =
    ["inc", "ltd", "ab", "aps", "gmbh", "llc", "corporation", "corp", "co", "s.a.", "b.v."];

fn undotted(s: &str) -> String {
    s.chars().filter(|c| *c != '.').collect()
}

fn is_number(s: &str) -> bool {
    s.chars().any(|c| c.is_ascii_digit()) && s.chars().all(|c| c.is_ascii_digit() || c == '.')
}

fn is_version_token(t: &str) -> bool {
    // Tokens with no letters at all (versions, year ranges, lone dashes) carry no name.
    !t.chars().any(|c| c.is_alphabetic()) || is_number(t) || matches!(t.strip_prefix('v'), Some(rest) if is_number(rest))
}

/// Removes parenthesised groups that are architecture notes, e.g. `(x64)`.
fn strip_arch_groups(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(open) = rest.find('(') {
        out.push_str(&rest[..open]);
        match rest[open..].find(')') {
            Some(close) => {
                let inner = &rest[open + 1..open + close];
                let arch = ["x64", "x86", "64-bit", "32-bit"].iter().any(|a| inner.contains(a));
                if !arch {
                    out.push_str(&rest[open..=open + close]);
                }
                rest = &rest[open + close + 1..];
            }
            None => {
                out.push_str(&rest[open..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn normalize_name(s: &str) -> String {
    let stripped = strip_arch_groups(&s.to_lowercase());
    let mut tokens: Vec<&str> = stripped
        .split_whitespace()
        .filter(|t| !is_version_token(t) && !matches!(*t, "64-bit" | "32-bit" | "x64" | "x86"))
        .collect();
    while matches!(tokens.last(), Some(&"setup") | Some(&"installer")) {
        tokens.pop();
    }
    tokens.join(" ")
}

pub fn is_matchable(name: &str) -> bool {
    let n = normalize_name(name);
    n.chars().count() >= 4 && !GENERIC_NAMES.contains(&n.as_str())
}

fn compact(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

pub fn name_matches(candidate: &str, hint_name: &str) -> bool {
    if !is_matchable(candidate) || !is_matchable(hint_name) {
        return false;
    }
    let c = normalize_name(candidate);
    let h = normalize_name(hint_name);
    if c == h || compact(&c) == compact(&h) {
        return true;
    }
    let mut words = h.split(' ');
    let first = words.next().unwrap_or("");
    words.next().is_some() && is_matchable(first) && c == first
}

fn strip_legal_suffix(s: &str) -> String {
    let mut words: Vec<&str> = s.split(' ').collect();
    while words.len() > 1 {
        let last = words[words.len() - 1];
        let last = undotted(last);
        if LEGAL_SUFFIXES.iter().any(|suffix| undotted(suffix) == last) {
            words.pop();
        } else {
            break;
        }
    }
    words.join(" ")
}

pub fn publisher_matches(candidate: &str, publisher: &str) -> bool {
    // (suffix-stripped, spaces-removed) and (unstripped, spaces/dots-removed) forms,
    // so "DiscordInc" still matches "Discord Inc.".
    let forms = |s: &str| {
        let n = normalize_name(s);
        (compact(&strip_legal_suffix(&n)), compact(&n).replace('.', ""))
    };
    let (cs, cf) = forms(candidate);
    let (ps, pf) = forms(publisher);
    if cs.is_empty() || ps.is_empty() {
        return false;
    }
    cs == ps || cf == pf || cs == pf || cf == ps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test] fn normalize_strips_versions_and_arch() { assert_eq!(normalize_name("Blender 3.6 (x64)"), "blender"); assert_eq!(normalize_name("Foo Setup"), "foo"); assert_eq!(normalize_name("7-Zip 23.01 (x64 edition)"), "7-zip"); }
    #[test] fn generic_and_short_names_are_not_matchable() { assert!(!is_matchable("Tools")); assert!(!is_matchable("Git")); assert!(is_matchable("Unity Hub")); }
    #[test] fn name_matches_handles_spaces_and_first_word() { assert!(name_matches("UnityHub", "Unity Hub")); assert!(name_matches("Unity", "Unity Hub")); assert!(!name_matches("Unityverse", "Unity Hub")); assert!(!name_matches("Tools", "Foo Tools")); }
    #[test] fn publisher_matches_ignores_legal_suffix() { assert!(publisher_matches("Unity Technologies", "Unity Technologies ApS")); assert!(publisher_matches("DiscordInc", "Discord Inc.")); assert!(!publisher_matches("Microsoft", "Microsoft Corporation Edge Team")); }
    #[test] fn normalize_strips_v_tokens_bitness_and_installer() { assert_eq!(normalize_name("Tool v2 64-bit Installer"), "tool"); assert_eq!(normalize_name("Game 2024"), "game"); assert_eq!(normalize_name("  Foo   Bar  "), "foo bar"); }
    #[test] fn publisher_matches_strips_bv_and_sa() { assert!(publisher_matches("Foo B.V.", "Foo")); assert!(publisher_matches("Bar S.A.", "Bar")); }
    #[test] fn normalize_drops_ranges_and_lone_punctuation() { assert_eq!(normalize_name("Microsoft Visual C++ 2015-2022 Redistributable (x64) - 14.38.33130"), "microsoft visual c++ redistributable"); assert_eq!(normalize_name("Node.js"), "node.js"); assert_eq!(normalize_name("7-Zip"), "7-zip"); }
}
