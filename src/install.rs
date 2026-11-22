//! Drag-and-drop profile install.
//!
//! A release ships every title's profile folder in one `PROFILES.zip`. The
//! player drops that zip next to the executable; on startup the proxy creates
//! `profiles/` and extracts each profile folder into it. The zip is left in
//! place, so the same file can be used again after a clean install.

use crate::*;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// Install `PROFILES.zip` from the game folder, if it is present.
///
/// Runs before profile discovery. A missing zip is not an error: profiles may
/// already be installed, or the player may be using safe mode.
pub fn install_profiles() {
    let root = exe_dir();
    let zip = root.join(PROFILES_ZIP);
    if !zip.is_file() {
        return;
    }
    let dest = root.join(PROFILE_DIR);
    match extract(&zip, &dest) {
        Ok(n) => log(&format!(
            "profiles: installed {n} file(s) from {} into {}",
            zip.display(),
            dest.display()
        )),
        Err(e) => log(&format!("profiles: cannot install {}: {e}", zip.display())),
    }
}

const MAX_PROFILE_SIZE: u64 = 256 * 1024;
const MAX_ENTRIES: usize = 4096;
const MAX_DEPTH: usize = 8;

pub(crate) fn ordinary_directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|info| {
        info.is_dir() && info.file_attributes() & 0x400 == 0 // FILE_ATTRIBUTE_REPARSE_POINT
    })
}

fn ensure_directory(path: &Path) -> Result<(), String> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("create {}: {e}", path.display())),
    }
    ordinary_directory(path)
        .then_some(())
        .ok_or_else(|| format!("unsafe profile directory {}", path.display()))
}

fn extract(zip_path: &Path, dest: &Path) -> Result<usize, String> {
    ensure_directory(dest)?;
    let file = File::open(zip_path).map_err(|e| format!("open: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("read zip: {e}"))?;
    if archive.len() > MAX_ENTRIES {
        return Err("too many archive entries".into());
    }
    let mut written = 0usize;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| format!("entry {i}: {e}"))?;
        let name = entry
            .name()
            .map_err(|e| format!("entry {i} name: {e}"))?
            .into_owned();
        // Directory entries are created implicitly when their files are written.
        if entry.is_dir() || name.ends_with(['/', '\\']) {
            continue;
        }
        // A zip can carry symlinks; only write regular files.
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            log(&format!(
                "profiles: skipping symlink {name} in {}",
                zip_path.display()
            ));
            continue;
        }
        let Some(rel) = profile_relative_path(&name) else {
            continue;
        };
        let out_path = dest.join(&rel);
        let mut parent = dest.to_path_buf();
        for component in rel.parent().into_iter().flat_map(Path::components) {
            parent.push(component);
            ensure_directory(&parent)?;
        }
        if fs::symlink_metadata(&out_path).is_ok() {
            continue;
        }
        let text = read_profile(&mut entry, &name)?;
        written += usize::from(
            publish_profile(&out_path, text.as_bytes())
                .map_err(|e| format!("write {name}: {e}"))?,
        );
    }
    Ok(written)
}

fn read_profile<R: Read>(
    entry: &mut zip::read::ZipFile<'_, R>,
    name: &str,
) -> Result<String, String> {
    let size = entry.size();
    if size > MAX_PROFILE_SIZE {
        return Err(format!("oversized profile {name}"));
    }
    let mut text = String::new();
    entry
        .take(MAX_PROFILE_SIZE + 1)
        .read_to_string(&mut text)
        .map_err(|e| format!("read {name}: {e}"))?;
    if text.len() as u64 != size {
        return Err(format!("invalid profile payload {name}"));
    }
    Ok(text)
}

/// Inspect a release archive in memory using the same path and size rules as extraction.
pub(crate) fn release_profile(
    bytes: &[u8],
    exe: &str,
    matches_build: impl Fn(&profile::Profile) -> bool,
) -> Result<bool, String> {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    if archive.len() > MAX_ENTRIES {
        return Err("too many archive entries".into());
    }
    let mut matched = None;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().map_err(|e| e.to_string())?.into_owned();
        if entry.is_dir() || name.ends_with(['/', '\\']) || entry.is_symlink() {
            continue;
        }
        let Some(path) = profile_relative_path(&name) else {
            continue;
        };
        let Some(folder) = path.iter().next().and_then(|part| part.to_str()) else {
            continue;
        };
        if !folder.eq_ignore_ascii_case(exe) {
            continue;
        }
        let text = read_profile(&mut entry, &name)?;
        let Ok(candidate) = profile::parse(&text, &name) else {
            continue;
        };
        if !candidate.exe.eq_ignore_ascii_case(folder)
            || !matches_build(&candidate)
            || profile::validate_addresses(&candidate).is_err()
        {
            continue;
        }
        if let Some(previous) = &matched
            && !profile::same_facts(previous, &candidate)
        {
            return Err("ambiguous matching profiles in the release archive".into());
        }
        matched = Some(candidate);
    }
    Ok(matched.is_some())
}

fn publish_profile(path: &Path, bytes: &[u8]) -> std::io::Result<bool> {
    static SEQUENCE: AtomicU32 = AtomicU32::new(0);
    let temp = path.with_file_name(format!(
        ".coke-profile-{}-{:x}-{}.tmp",
        std::process::id(),
        unsafe { GetTickCount() },
        SEQUENCE.fetch_add(1, Ordering::Relaxed),
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        let from: Vec<u16> = std::os::windows::ffi::OsStrExt::encode_wide(temp.as_os_str())
            .chain([0])
            .collect();
        let to: Vec<u16> = std::os::windows::ffi::OsStrExt::encode_wide(path.as_os_str())
            .chain([0])
            .collect();
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH) } != 0 {
            Ok(true)
        } else {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Ok(false)
            } else {
                Err(error)
            }
        }
    })();
    if !matches!(result, Ok(true)) {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Turn a zip entry into a path below `profiles/`.
///
/// Entries must name a profile folder and a file in it (`<exe name>/<build>.ini`
/// or deeper). An optional leading `profiles/` is stripped, so both the release
/// zip and the repository layout work. Absolute paths, `..`, drive letters and
/// Windows-invalid names are rejected instead of being repaired.
fn profile_relative_path(name: &str) -> Option<PathBuf> {
    if name.starts_with(['/', '\\']) {
        return None;
    }
    let mut parts = Vec::new();
    for part in name.split(['/', '\\']) {
        if part.is_empty() || part == "." {
            continue;
        }
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ["COM", "LPT"].iter().any(|prefix| {
                stem.strip_prefix(prefix).is_some_and(|suffix| {
                    matches!(
                        suffix,
                        "1" | "2"
                            | "3"
                            | "4"
                            | "5"
                            | "6"
                            | "7"
                            | "8"
                            | "9"
                            | "\u{b9}"
                            | "\u{b2}"
                            | "\u{b3}"
                    )
                })
            });
        if device
            || part == ".."
            || part.contains([':', '*', '?', '"', '<', '>', '|'])
            || part.chars().any(char::is_control)
            || part.ends_with('.')
            || part.ends_with(' ')
        {
            return None;
        }
        parts.push(part);
    }
    if parts
        .first()
        .is_some_and(|part| part.eq_ignore_ascii_case(PROFILE_DIR))
    {
        parts.remove(0);
    }
    // A profile is always one folder (the executable name) plus at least one file.
    if !(2..=MAX_DEPTH).contains(&parts.len())
        || !parts[0].to_ascii_lowercase().ends_with(".exe")
        || !parts
            .last()
            .is_some_and(|name| name.to_ascii_lowercase().ends_with(".ini"))
    {
        return None;
    }
    let mut path = PathBuf::new();
    for part in parts {
        path.push(part);
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_profile_requires_build_identity_and_rejects_conflicting_matches() {
        let exe = "CM Black Sea.exe";
        let ini = include_str!("../profiles/CM Black Sea.exe/steam-32.ini");
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("CM Black Sea.exe/steam-32.ini", options)
            .unwrap();
        zip.write_all(ini.as_bytes()).unwrap();
        let bytes = zip.finish().unwrap().into_inner();

        let matches = |p: &profile::Profile| {
            profile::matches_build(p, 0x00a5f000, 0x63d82c90, Some(0xf7b80d9a))
        };
        assert!(release_profile(&bytes, exe, matches).unwrap());
        assert!(!release_profile(&bytes, "CM Cold War.exe", matches).unwrap());
        assert!(
            !release_profile(&bytes, exe, |p| {
                profile::matches_build(p, 0x00a5f000, 0x63d82c90, Some(0))
            })
            .unwrap()
        );

        let mut zip = zip::ZipWriter::new_append(std::io::Cursor::new(bytes)).unwrap();
        zip.start_file("CM Black Sea.exe/conflict.ini", options)
            .unwrap();
        zip.write_all(
            ini.replace("build = steam-32", "build = conflict")
                .as_bytes(),
        )
        .unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        assert!(release_profile(&bytes, exe, matches).is_err());
    }
    use std::io::Write;

    #[test]
    fn invalid_zip_payload_is_never_published() {
        let root = std::env::temp_dir().join(format!("coke-bad-zip-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let zip_path = root.join(PROFILES_ZIP);
        let mut zip = zip::ZipWriter::new(File::create(&zip_path).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("CM Test.exe/profile.ini", options).unwrap();
        let original = b"[profile]\n";
        zip.write_all(original).unwrap();
        zip.finish().unwrap();
        let mut corrupt = fs::read(&zip_path).unwrap();
        let offset = corrupt
            .windows(original.len())
            .position(|window| window == original)
            .unwrap();
        corrupt[offset] ^= 1;
        fs::write(&zip_path, corrupt).unwrap();

        let dest = root.join(PROFILE_DIR);
        assert!(extract(&zip_path, &dest).is_err());
        assert!(!dest.join("CM Test.exe/profile.ini").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn entry_paths_stay_below_profiles() {
        assert_eq!(
            profile_relative_path("CM Cold War.exe/gog-32.ini"),
            Some(PathBuf::from("CM Cold War.exe/gog-32.ini"))
        );
        assert_eq!(
            profile_relative_path("profiles\\CM Cold War.exe\\gog-32.ini"),
            Some(PathBuf::from("CM Cold War.exe/gog-32.ini"))
        );
        assert_eq!(
            profile_relative_path("./CM Cold War.exe/nested/gog-32.ini"),
            Some(PathBuf::from("CM Cold War.exe/nested/gog-32.ini"))
        );
    }

    #[test]
    fn unsafe_or_flat_entries_are_ignored() {
        for name in [
            "../outside.ini",
            "CM Cold War.exe/../../outside.ini",
            "C:/Windows/system32/evil.ini",
            "gog-32.ini",
            "README.md",
            "CM Cold War.exe/bad:name.ini",
            "CM Cold War.exe/trailing.",
        ] {
            assert_eq!(profile_relative_path(name), None, "{name}");
        }
    }

    #[test]
    fn extracts_profile_folders_from_a_zip() {
        let root = std::env::temp_dir().join(format!("coke-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let zip_path = root.join(PROFILES_ZIP);
        {
            let mut zip = zip::ZipWriter::new(File::create(&zip_path).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            for (name, body) in [
                ("CM Cold War.exe/gog-32.ini", "[profile]\n"),
                ("profiles/CM Black Sea.exe/steam-32.ini", "[profile]\n"),
                ("profiles/CM Cold War.exe/gog-32.ini", "[duplicate]\n"),
                ("README.md", "not a profile\n"),
            ] {
                zip.start_file(name, opts).unwrap();
                zip.write_all(body.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
        }

        let dest = root.join(PROFILE_DIR);
        assert_eq!(extract(&zip_path, &dest).unwrap(), 2);
        let installed = dest.join("CM Cold War.exe/gog-32.ini");
        assert_eq!(fs::read_to_string(&installed).unwrap(), "[profile]\n");
        fs::write(&installed, "edited").unwrap();
        assert_eq!(extract(&zip_path, &dest).unwrap(), 0);
        assert_eq!(fs::read_to_string(installed).unwrap(), "edited");
        assert!(dest.join("CM Black Sea.exe/steam-32.ini").is_file());
        assert!(!dest.join("README.md").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
