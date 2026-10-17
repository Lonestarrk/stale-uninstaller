use std::path::PathBuf;

use chrono::{DateTime, Utc};

use super::filetime::filetime_to_utc;
use super::{UsageHit, UsageProvider, UsageSource};

pub mod decompress;

use decompress::decompress_mam;

/// Prefetch stores at most this many characters of the executable name.
const EXE_NAME_MAX_CHARS: usize = 29;

#[derive(Debug, PartialEq)]
pub struct PrefetchFile {
    pub version: u32,
    pub exe_name: String,
    pub last_runs: Vec<DateTime<Utc>>,
    pub run_count: u32,
    pub referenced_files: Vec<String>,
}

fn slice(b: &[u8], off: usize, len: usize) -> Result<&[u8], String> {
    off.checked_add(len)
        .and_then(|end| b.get(off..end))
        .ok_or_else(|| format!("read of {len} bytes at offset {off} is out of range ({} bytes)", b.len()))
}

fn u32_at(b: &[u8], off: usize) -> Result<u32, String> {
    Ok(u32::from_le_bytes(slice(b, off, 4)?.try_into().unwrap()))
}

fn u64_at(b: &[u8], off: usize) -> Result<u64, String> {
    Ok(u64::from_le_bytes(slice(b, off, 8)?.try_into().unwrap()))
}

fn utf16(b: &[u8]) -> Vec<u16> {
    b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect()
}

/// Parses a decompressed Prefetch file (versions 17, 23, 26 and 30).
pub fn parse_prefetch(bytes: &[u8]) -> Result<PrefetchFile, String> {
    let version = u32_at(bytes, 0)?;
    if slice(bytes, 4, 4)? != b"SCCA" {
        return Err("missing SCCA signature".into());
    }
    let (run_slots, run_count_off) = match version {
        17 | 23 => (1, 152),
        26 => (8, 208),
        // v31 (Windows 11 24H2+) shares the v30 layout.
        30 | 31 => (8, if u32_at(bytes, 200)? != 0 { 200 } else { 208 }),
        v => return Err(format!("unsupported Prefetch version {v}")),
    };

    let name = utf16(slice(bytes, 16, (EXE_NAME_MAX_CHARS + 1) * 2)?);
    let name_len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    let exe_name = String::from_utf16_lossy(&name[..name_len]);

    let strings_off = u32_at(bytes, 100)? as usize;
    let strings_len = u32_at(bytes, 104)? as usize;
    let referenced_files = utf16(slice(bytes, strings_off, strings_len)?)
        .split(|&c| c == 0)
        .filter(|s| !s.is_empty())
        .map(String::from_utf16_lossy)
        .collect();

    let mut last_runs = Vec::new();
    for i in 0..run_slots {
        if let Some(t) = filetime_to_utc(u64_at(bytes, 128 + 8 * i)?) {
            last_runs.push(t);
        }
    }
    let run_count = u32_at(bytes, run_count_off)?;

    Ok(PrefetchFile { version, exe_name, last_runs, run_count, referenced_files })
}

/// `\VOLUME{guid}\PROGRAM FILES\X.EXE` -> `\PROGRAM FILES\X.EXE`; `None` if `p` has no volume prefix.
pub fn strip_volume_prefix(p: &str) -> Option<String> {
    let rest = p.strip_prefix(r"\VOLUME{")?;
    let rest = &rest[rest.find('}')? + 1..];
    rest.starts_with(r"\").then(|| rest.to_string())
}

/// True if `path` is the executable itself. Prefetch truncates long names to 29 characters,
/// so a truncated `exe_name` only has to be a prefix of the file name.
fn is_exe_path(path: &str, exe_name: &str) -> bool {
    let file = path.rsplit(r"\").next().unwrap_or(path).to_uppercase();
    let exe = exe_name.to_uppercase();
    !exe.is_empty()
        && path.contains(r"\")
        && (file == exe || (exe.chars().count() == EXE_NAME_MAX_CHARS && file.starts_with(&exe)))
}

pub struct PrefetchProvider {
    pub dir: PathBuf,
}

impl Default for PrefetchProvider {
    fn default() -> Self {
        Self { dir: PathBuf::from(r"C:\Windows\Prefetch") }
    }
}

impl PrefetchProvider {
    fn read_file(path: &std::path::Path) -> Result<PrefetchFile, String> {
        let data = std::fs::read(path).map_err(|e| e.to_string())?;
        parse_prefetch(&decompress_mam(&data)?)
    }

    /// Hits, skipped `.pf` files and total `.pf` files.
    fn hits_in_dir(&self) -> Result<(Vec<UsageHit>, usize, usize), String> {
        let entries = std::fs::read_dir(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        let mut hits = Vec::new();
        let (mut skipped, mut total) = (0, 0);
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pf")) {
                continue;
            }
            total += 1;
            let Ok(pf) = Self::read_file(&path) else {
                skipped += 1;
                continue;
            };
            let Some(&last_run) = pf.last_runs.iter().max() else { continue };
            for f in pf.referenced_files.iter().filter(|f| is_exe_path(f, &pf.exe_name)) {
                hits.push(UsageHit {
                    exe_path: PathBuf::from(strip_volume_prefix(f).unwrap_or_else(|| f.clone())),
                    last_run,
                    source: UsageSource::Prefetch,
                    run_count: Some(pf.run_count),
                });
            }
        }
        Ok((hits, skipped, total))
    }

    /// Hits plus the number of `.pf` files that could not be read or parsed.
    /// An unreadable directory yields no hits and zero skipped files.
    pub fn collect_with_stats(&self) -> (Vec<UsageHit>, usize) {
        self.hits_in_dir().map(|(hits, skipped, _)| (hits, skipped)).unwrap_or_default()
    }
}

impl UsageProvider for PrefetchProvider {
    fn name(&self) -> &'static str {
        "Prefetch"
    }

    fn collect(&self) -> Result<Vec<UsageHit>, String> {
        self.hits_in_dir().map(|(hits, _, _)| hits)
    }

    fn collect_with_warnings(&self) -> (Result<Vec<UsageHit>, String>, Vec<String>) {
        match self.hits_in_dir() {
            Ok((hits, skipped, total)) => {
                let warnings = if skipped > 0 {
                    vec![format!("Prefetch: {skipped} av {total} filer kunde inte läsas")]
                } else {
                    vec![]
                };
                (Ok(hits), warnings)
            }
            Err(e) => (Err(e), vec![]),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use chrono::Datelike;

    use super::decompress::decompress_mam;
    use super::*;
    use crate::usage::UsageSource;

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(r"tests\fixtures\prefetch\RSAPPUI.EXE-F7E40085.pf")
    }

    #[test]
    fn parse_rejects_bad_signature() {
        assert!(parse_prefetch(&[0u8; 300]).is_err());
    }

    #[test]
    fn parse_rejects_truncated_file() {
        let f = std::fs::read(fixture()).unwrap();
        let raw = decompress_mam(&f).unwrap();
        assert!(parse_prefetch(&raw[..150]).is_err());
    }

    #[test]
    fn parses_real_fixture() {
        let raw = decompress_mam(&std::fs::read(fixture()).unwrap()).unwrap();
        let pf = parse_prefetch(&raw).unwrap();
        println!(
            "version={} exe={} runs={:?} run_count={} refs={}",
            pf.version,
            pf.exe_name,
            pf.last_runs,
            pf.run_count,
            pf.referenced_files.len()
        );
        assert!(pf.exe_name.ends_with(".EXE"));
        assert!(pf.version == 26 || pf.version == 30 || pf.version == 31);
        assert!(!pf.last_runs.is_empty() && pf.last_runs[0].year() >= 2015);
        assert!(pf.referenced_files.iter().any(|f| f.ends_with(&pf.exe_name)));
        assert!(pf.run_count > 0);
        let exe_suffix = format!(r"\{}", pf.exe_name);
        assert!(pf.referenced_files.iter().any(|f| f.ends_with(&exe_suffix)));
    }

    #[test]
    fn parse_never_panics_on_any_truncation() {
        let raw = decompress_mam(&std::fs::read(fixture()).unwrap()).unwrap();
        for len in 0..raw.len() {
            let _ = parse_prefetch(&raw[..len]);
        }
    }

    #[test]
    fn provider_skips_corrupt_file_but_keeps_good_ones() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::copy(fixture(), dir.path().join("RSAPPUI.EXE-F7E40085.pf")).unwrap();
        std::fs::write(dir.path().join("GARBAGE.EXE-00000000.pf"), [0xABu8; 40]).unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"ignored").unwrap();
        let p = PrefetchProvider { dir: dir.path().to_path_buf() };
        let (hits, skipped) = p.collect_with_stats();
        assert_eq!(skipped, 1);
        assert!(!hits.is_empty());
        for h in &hits {
            assert_eq!(h.source, UsageSource::Prefetch);
            assert!(h.run_count.is_some());
            let s = h.exe_path.to_string_lossy();
            assert!(s.starts_with(r"\") && !s.starts_with(r"\VOLUME{"), "{s}");
            assert!(s.ends_with(r"\RSAPPUI.EXE"), "{s}");
        }
        assert_eq!(p.collect().unwrap(), hits);
        let (with_warnings, warnings) = p.collect_with_warnings();
        assert_eq!(with_warnings.unwrap(), hits);
        assert_eq!(warnings, vec!["Prefetch: 1 av 2 filer kunde inte läsas".to_string()]);
    }

    #[test]
    fn provider_without_bad_files_has_no_warnings() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::copy(fixture(), dir.path().join("RSAPPUI.EXE-F7E40085.pf")).unwrap();
        let (hits, warnings) = PrefetchProvider { dir: dir.path().to_path_buf() }.collect_with_warnings();
        assert!(!hits.unwrap().is_empty());
        assert!(warnings.is_empty());
    }

    #[test]
    fn provider_errs_when_dir_missing() {
        let dir = tempfile::tempdir().unwrap();
        let p = PrefetchProvider { dir: dir.path().join("nope") };
        assert!(p.collect().is_err());
    }

    #[test]
    fn strip_volume_prefix_works() {
        assert_eq!(
            strip_volume_prefix(r"\VOLUME{01d9-ab}\PROGRAM FILES\X.EXE").as_deref(),
            Some(r"\PROGRAM FILES\X.EXE")
        );
        assert_eq!(strip_volume_prefix(r"\PROGRAM FILES\X.EXE"), None);
        assert_eq!(strip_volume_prefix(r"\VOLUME{01d9-ab"), None);
    }
}
