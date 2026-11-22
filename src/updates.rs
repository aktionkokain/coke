//! Opt-in release check.
//!
//! Nothing leaves the machine until the player says yes. On the first launch
//! that reaches this code the player is asked once (default: no); the answer is
//! stored in the `[state]` section of `coke_config.ini` next to the executable.
//! With consent, a background
//! thread asks the GitHub releases API once per launch for the latest tag,
//! and offers the release page for a newer build. When automatic profile
//! discovery found no match, it can also inspect the release's profile archive.

use crate::*;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::io::{self, Write};
use std::os::windows::{ffi::OsStrExt, fs::OpenOptionsExt};
use std::path::Path;

const API_HOST: &str = "api.github.com";
const API_PATH: &str = "/repos/aktionkokain/coke/releases/latest";
const RELEASE_HOST: &str = "github.com";
const RELEASE_PATH: &str = "/aktionkokain/coke/releases";
const CHECK_AFTER_MS: u32 = 4000;
const MAX_RELEASE_JSON: usize = 64 * 1024;
const MAX_PROFILE_ARCHIVE: usize = 2 * 1024 * 1024;

static STARTED: AtomicU32 = AtomicU32::new(0);
static FIRST_SEEN: AtomicU32 = AtomicU32::new(0);
static FIRST_WINDOW: AtomicUsize = AtomicUsize::new(0);

/// Called from the watcher every tick. Starts the one-shot worker once the main
/// window has existed for a few seconds, so the prompt does not race startup.
pub unsafe fn tick(hwnd: Hwnd) {
    let mode = config().check_updates;
    if mode == UPDATE_CHECK_OFF || STARTED.load(Ordering::Relaxed) != 0 {
        return;
    }
    let now = unsafe { GetTickCount() };
    let window = hwnd as usize;
    if FIRST_WINDOW.load(Ordering::Relaxed) != window {
        // The engine destroys its intro window and creates the real one; start
        // the delay over so the prompt is owned by the window the player sees.
        FIRST_WINDOW.store(window, Ordering::Relaxed);
        FIRST_SEEN.store(now, Ordering::Relaxed);
        return;
    }
    if now.wrapping_sub(FIRST_SEEN.load(Ordering::Relaxed)) < CHECK_AFTER_MS {
        return;
    }
    if STARTED.swap(1, Ordering::Relaxed) != 0 {
        return;
    }
    let owner = hwnd as usize;
    if let Err(error) = std::thread::Builder::new()
        .name("coke-updates".into())
        .spawn(move || unsafe { run(owner as Hwnd, mode) })
    {
        log(&format!("updates: cannot start worker: {error}"));
    }
}

/// Persisted opt-in state.
///
/// It lives in the `[state]` section of the main config file so there is only
/// one configuration file next to the executable.
#[derive(Default, Debug, PartialEq, Eq)]
struct State {
    consent: Option<bool>,
    disabled: bool,
    ignored: Option<String>,
    notified: Option<String>,
}

/// Older builds kept the state in its own file; it is migrated on first load.
const LEGACY_STATE_FILE: &str = "coke_state.ini";
const STATE_SECTION: &str = "state";

impl State {
    fn load() -> Self {
        Self::load_at(&config_path(), &exe_dir().join(LEGACY_STATE_FILE))
    }

    fn load_at(path: &Path, legacy: &Path) -> Self {
        let mut state = parse_state(&std::fs::read_to_string(path).unwrap_or_default());
        let Ok(text) = std::fs::read_to_string(legacy) else {
            return state;
        };
        let fallback = parse_state(&format!("[{STATE_SECTION}]\n{text}"));
        if let Ok(saved) = Self::edit_at(path, |state| {
            state.merge_legacy(&fallback);
            Ok(())
        }) {
            let _ = std::fs::remove_file(legacy);
            saved
        } else {
            state.merge_legacy(&fallback);
            state
        }
    }

    fn merge_legacy(&mut self, fallback: &Self) {
        self.consent = self.consent.or(fallback.consent);
        self.disabled |= fallback.disabled;
        self.ignored = self.ignored.take().or_else(|| fallback.ignored.clone());
        self.notified = self.notified.take().or_else(|| fallback.notified.clone());
    }

    fn edit(edit: impl FnOnce(&mut Self) -> io::Result<()>) -> io::Result<Self> {
        Self::edit_at(&config_path(), edit)
    }

    fn edit_at(path: &Path, edit: impl FnOnce(&mut Self) -> io::Result<()>) -> io::Result<Self> {
        let result = (|| {
            let _lock = lock_state(path)?;
            let text = match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
                Err(e) => return Err(e),
            };
            let mut state = parse_state(&text);
            edit(&mut state)?;
            state.save(path, &text)?;
            Ok(state)
        })();
        if let Err(ref error) = result {
            log(&format!("updates: could not save state: {error}"));
        }
        result
    }

    fn save(&self, path: &Path, existing: &str) -> io::Result<()> {
        // Keep the file's CRLF/LF convention.
        let eol = if existing.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let stripped = strip_state_section(existing, eol);
        let base = stripped.trim_end();

        let mut out = String::new();
        if !base.is_empty() {
            out.push_str(base);
            out.push_str(eol);
            out.push_str(eol);
        }
        out.push_str("[state]");
        out.push_str(eol);
        out.push_str("# Written by Coke. Delete this section to see the update prompt again.");
        out.push_str(eol);
        if let Some(consent) = self.consent {
            out.push_str(&format!(
                "update_consent={}{eol}",
                if consent { "yes" } else { "no" }
            ));
        }
        if self.disabled {
            out.push_str(&format!("update_disabled=1{eol}"));
        }
        if let Some(v) = &self.ignored {
            out.push_str(&format!("ignored_update={v}{eol}"));
        }
        if let Some(v) = &self.notified {
            out.push_str(&format!("notified_update={v}{eol}"));
        }
        let tmp = path.with_file_name(format!("{}.tmp-{}", CONFIG_FILE, std::process::id()));
        let result = (|| -> std::io::Result<()> {
            let mut file = std::fs::File::create(&tmp)?;
            file.write_all(out.as_bytes())?;
            file.sync_all()?;
            let from: Vec<u16> = tmp.as_os_str().encode_wide().chain([0]).collect();
            let to: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
            if unsafe {
                MoveFileExW(
                    from.as_ptr(),
                    to.as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            } == 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }
}

fn lock_state(config: &Path) -> io::Result<std::fs::File> {
    let path = config.with_file_name(".coke-config.lock");
    let started = std::time::Instant::now();
    loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(&path)
        {
            Err(e) if e.raw_os_error() == Some(32) && started.elapsed().as_millis() < 1000 => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            result => return result,
        }
    }
}

/// Read the `[state]` section. Other sections are ignored.
fn parse_state(text: &str) -> State {
    let mut state = State::default();
    let mut in_state = false;
    for raw in text.lines() {
        let line = ini_content(raw);
        if line.is_empty() {
            continue;
        }
        if let Some(name) = ini_section(line) {
            in_state = name.eq_ignore_ascii_case(STATE_SECTION);
            continue;
        }
        if !in_state {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim().to_ascii_lowercase().as_str() {
            "update_consent" => {
                state.consent = Some(matches!(
                    value.to_ascii_lowercase().as_str(),
                    "1" | "yes" | "true" | "on"
                ));
            }
            "update_disabled" => {
                state.disabled = matches!(
                    value.to_ascii_lowercase().as_str(),
                    "1" | "yes" | "true" | "on"
                );
            }
            "ignored_update" if !value.is_empty() => {
                state.ignored = Some(value.to_string());
            }
            "notified_update" if !value.is_empty() => {
                state.notified = Some(value.to_string());
            }
            _ => {}
        }
    }
    state
}

/// Drop the `[state]` section and keep everything else byte-for-byte in spirit.
fn strip_state_section(text: &str, eol: &str) -> String {
    let mut out = String::new();
    let mut in_state = false;
    for raw in text.lines() {
        if let Some(name) = ini_section(ini_content(raw)) {
            in_state = name.eq_ignore_ascii_case(STATE_SECTION);
            if in_state {
                continue;
            }
        }
        if in_state {
            continue;
        }
        out.push_str(raw);
        out.push_str(eol);
    }
    out
}

unsafe fn run(owner: Hwnd, mode: u8) {
    unsafe {
        let mut state = State::load();
        if state.disabled {
            log("updates: not checking (disabled in the [state] section)");
            return;
        }
        let consent = match mode {
            UPDATE_CHECK_ON => true,
            _ => match state.consent {
                Some(answer) => answer,
                None => match ask_consent(owner) {
                    Some(answer) => {
                        let Ok(saved) = State::edit(|state| {
                            state.consent = state.consent.or(Some(answer));
                            Ok(())
                        }) else {
                            return;
                        };
                        state = saved;
                        !state.disabled && state.consent == Some(true)
                    }
                    None => {
                        // Dismissed (clicked the game, close box, Escape):
                        // nothing is recorded, so the question comes back on the
                        // next launch instead of silently opting in or out.
                        log("updates: consent prompt dismissed, asking again next launch");
                        return;
                    }
                },
            },
        };
        if !consent {
            log("updates: not checking (the player declined)");
            return;
        }

        // No interval throttle: the check runs once per launch (the STARTED
        // guard above), which is what "check when the game opens" means.
        let Some(release) = fetch(
            API_HOST,
            API_PATH,
            "application/vnd.github+json",
            MAX_RELEASE_JSON,
        )
        .and_then(|body| parse_release(&body)) else {
            log("updates: check failed or no release found");
            return;
        };
        let tag = release.tag_name;
        log(&format!(
            "updates: latest release {tag} (running {VERSION})"
        ));
        let newer = is_newer(&tag, VERSION);
        if newer
            && (state.ignored.as_deref() == Some(tag.as_str())
                || state.notified.as_deref() == Some(tag.as_str()))
        {
            log(&format!("updates: {tag} already ignored or shown"));
            return;
        }

        let archive_path = format!("{RELEASE_PATH}/download/{tag}/{PROFILES_ZIP}");
        let matching_profile = !is_newer(VERSION, &tag)
            && release
                .assets
                .iter()
                .any(|asset| asset.name == PROFILES_ZIP)
            && config().profile.is_none()
            && profile::profile().is_none()
            && check_release_profile(&archive_path)
            && profile::profile().is_none();
        if !newer {
            log("updates: up to date");
            if matching_profile
                && prompt(
                    owner,
                    "Coke profiles",
                    "A profile matches this game build",
                    &format!(
                        "Download {PROFILES_ZIP} from Coke {tag}, place it next to the game executable, and restart the game."
                    ),
                    &[("Download profiles", 1), ("Later", 0)],
                    0,
                    0,
                ) == 1
            {
                open_url(&format!("https://{RELEASE_HOST}{archive_path}"));
            }
            return;
        }

        let release_url = format!("https://{RELEASE_HOST}{RELEASE_PATH}/tag/{tag}");
        let profile_note = if matching_profile {
            "\n\nThis release also includes a profile that matches your game build. Install the release and its PROFILES.zip together."
        } else {
            ""
        };
        let body = format!(
            "You are running Coke {VERSION}; the latest release is {tag}. \
             Open release to download it in your browser.{profile_note}"
        );
        let choice = prompt(
            owner,
            "Coke update",
            "A newer Coke release is available",
            &body,
            &[
                ("Open release", 1),
                ("Ignore this version", 2),
                ("Never check again", 3),
            ],
            1,
            0,
        );
        if choice == 0 {
            // Dismissed: do not record anything, so this is asked again later.
            log("updates: update notice dismissed, not recording a choice");
            return;
        }
        match choice {
            1 => {
                log(&format!("updates: opening {release_url}"));
                open_url(&release_url);
            }
            2 => {
                log(&format!("updates: ignoring {tag}"));
            }
            _ => {
                log("updates: disabled by the player");
            }
        }
        // Never ask about the same tag twice, whatever was chosen.
        let _ = State::edit(|state| {
            if choice == 2 {
                state.ignored = Some(tag.clone());
            }
            if choice == 3 {
                state.disabled = true;
            }
            state.notified = Some(tag);
            Ok(())
        });
    }
}

unsafe fn check_release_profile(path: &str) -> bool {
    let Some(bytes) =
        (unsafe { fetch(RELEASE_HOST, path, "application/zip", MAX_PROFILE_ARCHIVE) })
    else {
        log("updates: could not fetch the release profiles");
        return false;
    };
    let exe = profile::discover_exe();
    let size = profile::run_image_size();
    let timestamp = profile::run_timestamp();
    let hash = profile::text_hash();
    match install::release_profile(&bytes, &exe, |p| {
        profile::matches_build(p, size, timestamp, hash)
    }) {
        Ok(true) => {
            log("updates: release has a matching profile");
            true
        }
        Ok(false) => {
            log("updates: release has no matching profile");
            false
        }
        Err(error) => {
            log(&format!(
                "updates: could not inspect release profiles: {error}"
            ));
            false
        }
    }
}

unsafe fn ask_consent(owner: Hwnd) -> Option<bool> {
    unsafe {
        let body = format!(
            "Coke can ask GitHub for the latest release every time the game \
             starts. It sends no \
             personal data. If no local profile matches, Coke can also inspect the \
             release's profile archive to look for a match. Installation is manual. The answer is \
             remembered in the [state] section of {CONFIG_FILE} next to the game, \
             and you can change it later with check_updates on or off.\n\n\
             Click the game, the close box or press Escape to decide later: \
             nothing is saved and the question returns on the next launch."
        );
        log("updates: asking for consent");
        match prompt(
            owner,
            "Coke update check",
            "Check for Coke updates automatically?",
            &body,
            &[("Yes, check for updates", 1), ("No, don't check", 0)],
            0,
            2,
        ) {
            1 => Some(true),
            0 => Some(false),
            _ => None,
        }
    }
}

unsafe fn fetch(host: &str, path: &str, accept: &str, limit: usize) -> Option<Vec<u8>> {
    unsafe {
        let session = WinHttpOpen(
            wide(&format!("Coke/{VERSION} update-check")).as_ptr(),
            WINHTTP_ACCESS_TYPE_DEFAULT_PROXY,
            core::ptr::null(),
            core::ptr::null(),
            0,
        );
        if session.is_null() {
            return None;
        }
        WinHttpSetTimeouts(session, 4000, 4000, 4000, 4000);
        let mut response = None;

        let connect = WinHttpConnect(session, wide(host).as_ptr(), 443, 0);
        if !connect.is_null() {
            let request = WinHttpOpenRequest(
                connect,
                wide("GET").as_ptr(),
                wide(path).as_ptr(),
                core::ptr::null(),
                core::ptr::null(),
                core::ptr::null(),
                WINHTTP_FLAG_SECURE,
            );
            if !request.is_null() {
                // GitHub rejects requests without a User-Agent.
                let headers = format!(
                    "User-Agent: Coke/{VERSION} update-check\r\n\
                     Accept: {accept}\r\n"
                );
                let sent = WinHttpSendRequest(
                    request,
                    wide(&headers).as_ptr(),
                    u32::MAX,
                    core::ptr::null_mut(),
                    0,
                    0,
                    0,
                );
                if sent != 0 && WinHttpReceiveResponse(request, core::ptr::null_mut()) != 0 {
                    let mut status: u32 = 0;
                    let mut len = core::mem::size_of::<u32>() as u32;
                    let ok = WinHttpQueryHeaders(
                        request,
                        WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                        core::ptr::null(),
                        &mut status as *mut u32 as *mut core::ffi::c_void,
                        &mut len,
                        core::ptr::null_mut(),
                    );
                    if ok == 0 {
                        log("updates: could not read the HTTP status");
                    } else if status != 200 {
                        log(&format!("updates: GitHub returned HTTP {status}"));
                    } else {
                        let mut body = Vec::new();
                        let mut complete = false;
                        loop {
                            let mut available: u32 = 0;
                            if WinHttpQueryDataAvailable(request, &mut available) == 0 {
                                break;
                            }
                            if available == 0 {
                                complete = true;
                                break;
                            }
                            let want = available.min(4096);
                            let mut buf = [0u8; 4096];
                            let mut read: u32 = 0;
                            if WinHttpReadData(
                                request,
                                buf.as_mut_ptr() as *mut core::ffi::c_void,
                                want,
                                &mut read,
                            ) == 0
                                || read == 0
                            {
                                break;
                            }
                            if body.len() + read as usize > limit {
                                break;
                            }
                            body.extend_from_slice(&buf[..read as usize]);
                        }
                        if complete {
                            response = Some(body);
                        }
                    }
                } else {
                    log("updates: HTTP request failed");
                }
                WinHttpCloseHandle(request);
            }
            WinHttpCloseHandle(connect);
        }
        WinHttpCloseHandle(session);
        response
    }
}

#[derive(serde::Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

#[derive(serde::Deserialize)]
struct ReleaseAsset {
    name: String,
}

fn parse_release(json: &[u8]) -> Option<Release> {
    let mut release: Release = serde_json::from_slice(json).ok()?;
    let tag = release.tag_name.trim();
    if !tag
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'+'))
    {
        return None;
    }
    parse_version(tag)?;
    release.tag_name = tag.to_owned();
    Some(release)
}

fn parse_version(value: &str) -> Option<Vec<u64>> {
    let value = value.trim().trim_start_matches(['v', 'V']);
    let core = value.split(['-', '+']).next().unwrap_or("");
    if core.is_empty() {
        return None;
    }
    core.split('.').map(|part| part.parse().ok()).collect()
}

/// True when `latest` is a higher dotted version than `current`. Pre-release
/// suffixes are ignored; a tag that is equal or unparsable is not newer.
fn is_newer(latest: &str, current: &str) -> bool {
    let (Some(latest), Some(current)) = (parse_version(latest), parse_version(current)) else {
        return false;
    };
    let len = latest.len().max(current.len());
    for i in 0..len {
        let a = latest.get(i).copied().unwrap_or(0);
        let b = current.get(i).copied().unwrap_or(0);
        if a != b {
            return a > b;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_preserves_legacy_on_write_failure_and_merges_on_success() {
        let root =
            std::env::temp_dir().join(format!("coke-update-migration-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join(CONFIG_FILE);
        let legacy = root.join(LEGACY_STATE_FILE);
        std::fs::write(&legacy, "update_disabled=1\n").unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(State::load_at(&path, &legacy).disabled);
        assert!(legacy.is_file());
        std::fs::remove_dir(&path).unwrap();
        std::fs::write(&path, "[display]\nwidth=1234\n").unwrap();
        assert!(State::load_at(&path, &legacy).disabled);
        assert!(!legacy.exists());
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("width=1234")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn concurrent_updates_keep_both_user_choices() {
        let root =
            std::env::temp_dir().join(format!("coke-update-concurrent-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join(CONFIG_FILE);
        std::fs::write(&path, "width=1234\n").unwrap();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                State::edit_at(&path, |state| {
                    state.consent = Some(true);
                    Ok(())
                })
                .unwrap()
            });
            scope.spawn(|| {
                State::edit_at(&path, |state| {
                    state.disabled = true;
                    Ok(())
                })
                .unwrap()
            });
        });
        let text = std::fs::read_to_string(&path).unwrap();
        let state = parse_state(&text);
        assert_eq!(state.consent, Some(true));
        assert!(state.disabled);
        assert!(text.contains("width=1234"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reads_release_tag_and_assets_and_rejects_unsafe_paths() {
        let json = br#"{"tag_name":"v0.2.0","assets":[{"name":"PROFILES.zip"}]}"#;
        let release = parse_release(json).unwrap();
        assert_eq!(release.tag_name, "v0.2.0");
        assert_eq!(release.assets[0].name, PROFILES_ZIP);
        assert_eq!(
            parse_release(br#"{"tag_name":" 0.3 "}"#).unwrap().tag_name,
            "0.3"
        );
        assert!(parse_release(br#"{"name":"no tag"}"#).is_none());
        assert!(parse_release(br#"{"tag_name":"v0.2.0-../../other"}"#).is_none());
    }

    #[test]
    fn state_lives_in_the_state_section() {
        let text = "\
[display]
width=1920
[state]
update_consent=yes
ignored_update=v9.9.9
[update]
check_updates=ask
";
        let state = parse_state(text);
        assert_eq!(state.consent, Some(true));
        assert_eq!(state.ignored.as_deref(), Some("v9.9.9"));
        assert_eq!(state.notified, None);

        // Keys outside [state] are not state, even if they share a name.
        let outside = "update_consent=yes\n[update]\ncheck_updates=ask\n";
        assert_eq!(parse_state(outside).consent, None);
    }

    #[test]
    fn stripping_the_state_section_keeps_the_settings() {
        let text =
            "[display]\nwidth=1920\n\n[state]\nupdate_consent=no\n\n[update]\ncheck_updates=ask\n";
        let stripped = strip_state_section(text, "\n");
        assert!(!stripped.contains("update_consent"));
        assert!(!stripped.contains("[state]"));
        assert!(stripped.contains("width=1920"));
        assert!(stripped.contains("check_updates=ask"));
    }

    #[test]
    fn compares_dotted_versions() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0-beta", "0.1.0"), "pre-release is not newer");
        assert!(!is_newer("banana", "0.1.0"));
        assert!(!is_newer("0.2.0", "not a version"));
    }
}
