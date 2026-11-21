//! Drag-and-drop profile install.
//!
//! A release ships every title's profile folder in one `PROFILES.zip`. The
//! player drops that zip next to the executable; on startup the proxy creates
//! `profiles/` and extracts each profile folder into it. The zip is left in
//! place, so the same file can be used again after a clean install.

use crate::*;
use std::fs::File;
use std::path::{Path, PathBuf};

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

fn extract(zip_path: &Path, dest: &Path) -> Result<usize, String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("create {}: {e}", dest.display()))?;
    let file = File::open(zip_path).map_err(|e| format!("open: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("read zip: {e}"))?;
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
        // The ZIP is intentionally left beside the game so it can be reused
        // after a clean install. Never overwrite a profile that the user may
        // have edited locally while that ZIP remains present.
        if out_path.exists() {
            continue;
        }
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        let mut out =
            File::create(&out_path).map_err(|e| format!("create {}: {e}", out_path.display()))?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|e| format!("write {}: {e}", out_path.display()))?;
        written += 1;
    }
    Ok(written)
}

/// Turn a zip entry into a path below `profiles/`.
///
/// Entries must name a profile folder and a file in it (`<exe name>/<build>.ini`
/// or deeper). An optional leading `profiles/` is stripped, so both the release
/// zip and the repository layout work. Absolute paths, `..`, drive letters and
/// Windows-invalid names are rejected instead of being repaired.
fn profile_relative_path(name: &str) -> Option<PathBuf> {
    let mut parts = Vec::new();
    for part in name.split(['/', '\\']) {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".."
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
    if parts.len() < 2 {
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
    use std::io::Write;

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
                ("README.md", "not a profile\n"),
            ] {
                zip.start_file(name, opts).unwrap();
                zip.write_all(body.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
        }

        let dest = root.join(PROFILE_DIR);
        assert_eq!(extract(&zip_path, &dest).unwrap(), 2);
        assert!(dest.join("CM Cold War.exe/gog-32.ini").is_file());
        assert!(dest.join("CM Black Sea.exe/steam-32.ini").is_file());
        assert!(!dest.join("README.md").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
