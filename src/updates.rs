//! Opt-in release check.
//!
//! Nothing leaves the machine until the player says yes. On the first launch
//! that reaches this code the player is asked once (default: no); the answer is
//! stored in the `[state]` section of `coke_config.ini` next to the executable.
//! With consent, a background
//! thread asks the GitHub releases API once per launch for the latest tag,
//! and a newer tag offers Download / Ignore this version / Never check again.
//!
//! The check is deliberately non-installing: Download only opens the releases
//! page in the default browser.

use crate::*;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

const API_HOST: &str = "api.github.com";
const API_PATH: &str = "/repos/aktionkokain/coke/releases/latest";
const RELEASES_URL: &str = "https://github.com/aktionkokain/coke/releases";
const CHECK_AFTER_MS: u32 = 4000;
const MAX_BODY: usize = 64 * 1024;

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
    std::thread::spawn(move || unsafe { run(owner as Hwnd, mode) });
}

/// Persisted opt-in state.
///
/// It lives in the `[state]` section of the main config file so there is only
/// one file next to the executable; `deploy.ps1` carries that section over when
/// it installs a new config. It is not a setting, so it stays out of the rest of
/// the file.
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
    fn path() -> std::path::PathBuf {
        exe_dir().join(CONFIG_FILE)
    }

    fn load() -> Self {
        let text = std::fs::read_to_string(Self::path()).unwrap_or_default();
        let state = parse_state(&text);

        // Migrate a state file left by an older build, then delete it.
        let legacy = exe_dir().join(LEGACY_STATE_FILE);
        if !legacy.exists() {
            return state;
        }
        let fallback = std::fs::read_to_string(&legacy)
            .map(|text| parse_state(&format!("[{STATE_SECTION}]\n{text}")))
            .unwrap_or_default();
        let mut merged = state;
        merged.consent = merged.consent.or(fallback.consent);
        merged.disabled |= fallback.disabled;
        merged.ignored = merged.ignored.or(fallback.ignored);
        merged.notified = merged.notified.or(fallback.notified);
        merged.save();
        let _ = std::fs::remove_file(&legacy);
        merged
    }

    fn save(&self) {
        let path = Self::path();
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        // Keep the file's CRLF/LF convention.
        let eol = if existing.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let stripped = strip_state_section(&existing, eol);
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
        if let Err(e) = std::fs::write(&path, out) {
            log(&format!("updates: cannot write {}: {e}", path.display()));
        }
    }
}

/// Read the `[state]` section. Other sections are ignored.
fn parse_state(text: &str) -> State {
    let mut state = State::default();
    let mut in_state = false;
    for raw in text.lines() {
        let line = raw.split(['#', ';']).next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[') {
            let name = name.strip_suffix(']').unwrap_or(name).trim();
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
        let trimmed = raw.trim();
        if let Some(name) = trimmed.strip_prefix('[') {
            let name = name.strip_suffix(']').unwrap_or(name).trim();
            in_state = name.eq_ignore_ascii_case(STATE_SECTION);
            if in_state {
                continue;
            }
        }
        if in_state {
            continue;
        }
        out.push_str(raw.trim_end());
        out.push_str(eol);
    }
    out
}

unsafe fn run(owner: Hwnd, mode: u8) {
    unsafe {
        let mut state = State::load();
        let consent = match mode {
            UPDATE_CHECK_ON => true,
            _ => match state.consent {
                Some(answer) => answer,
                None => match ask_consent(owner) {
                    Some(answer) => {
                        state.consent = Some(answer);
                        state.save();
                        answer
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
        if mode == UPDATE_CHECK_ASK && state.disabled {
            log("updates: not checking (disabled in the [state] section)");
            return;
        }

        // No interval throttle: the check runs once per launch (the STARTED
        // guard above), which is what "check when the game opens" means.
        let Some(tag) = fetch_latest_tag() else {
            log("updates: check failed or no release found");
            return;
        };
        log(&format!(
            "updates: latest release {tag} (running {VERSION})"
        ));
        if !is_newer(&tag, VERSION) {
            log("updates: up to date");
            return;
        }
        if state.ignored.as_deref() == Some(tag.as_str())
            || state.notified.as_deref() == Some(tag.as_str())
        {
            log(&format!("updates: {tag} already ignored or shown"));
            return;
        }

        let body = format!(
            "You are running Coke {VERSION}; the latest release is {tag}. \
             Download opens {RELEASES_URL} in your browser. Coke never installs \
             an update for you."
        );
        let choice = prompt(
            owner,
            "Coke update",
            "A newer Coke release is available",
            &body,
            &[
                ("Download", 1),
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
                log(&format!("updates: opening {RELEASES_URL}"));
                open_url(RELEASES_URL);
            }
            2 => {
                log(&format!("updates: ignoring {tag}"));
                state.ignored = Some(tag.clone());
            }
            _ => {
                log("updates: disabled by the player");
                state.disabled = true;
            }
        }
        // Never ask about the same tag twice, whatever was chosen.
        state.notified = Some(tag);
        state.save();
    }
}

unsafe fn ask_consent(owner: Hwnd) -> Option<bool> {
    unsafe {
        let body = format!(
            "Coke can ask GitHub for the latest release every time the game \
             starts. It sends no \
             personal data: just an ordinary HTTPS request for the public release \
             list. Nothing is downloaded or installed automatically. The answer is \
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

/// Fetch the `tag_name` of the latest published GitHub release.
unsafe fn fetch_latest_tag() -> Option<String> {
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
        let mut tag = None;

        let connect = WinHttpConnect(session, wide(API_HOST).as_ptr(), 443, 0);
        if !connect.is_null() {
            let request = WinHttpOpenRequest(
                connect,
                wide("GET").as_ptr(),
                wide(API_PATH).as_ptr(),
                core::ptr::null(),
                core::ptr::null(),
                core::ptr::null(),
                WINHTTP_FLAG_SECURE,
            );
            if !request.is_null() {
                // GitHub rejects requests without a User-Agent.
                let headers = format!(
                    "User-Agent: Coke/{VERSION} update-check\r\n\
                     Accept: application/vnd.github+json\r\n"
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
                        loop {
                            let mut available: u32 = 0;
                            if WinHttpQueryDataAvailable(request, &mut available) == 0
                                || available == 0
                            {
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
                            body.extend_from_slice(&buf[..read as usize]);
                            if body.len() >= MAX_BODY {
                                break;
                            }
                        }
                        tag = parse_tag(&String::from_utf8_lossy(&body));
                    }
                } else {
                    log("updates: HTTP request failed");
                }
                WinHttpCloseHandle(request);
            }
            WinHttpCloseHandle(connect);
        }
        WinHttpCloseHandle(session);
        tag
    }
}

/// Pull `"tag_name": "v0.2.0"` out of the release JSON without a JSON crate.
fn parse_tag(json: &str) -> Option<String> {
    let after_key = &json[json.find("\"tag_name\"")? + 10..];
    let after_colon = &after_key[after_key.find(':')? + 1..];
    let after_quote = &after_colon[after_colon.find('"')? + 1..];
    let end = after_quote.find('"')?;
    let tag = after_quote[..end].trim();
    (!tag.is_empty()).then(|| tag.to_string())
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
    fn reads_the_tag_from_release_json() {
        let json = r#"{"url":"x","tag_name":"v0.2.0","name":"0.2.0"}"#;
        assert_eq!(parse_tag(json).as_deref(), Some("v0.2.0"));
        assert_eq!(
            parse_tag(r#"{"tag_name" : " 0.3 "}"#).as_deref(),
            Some("0.3")
        );
        assert_eq!(parse_tag(r#"{"name":"no tag"}"#), None);
        assert_eq!(parse_tag(r#"{"tag_name":""}"#), None);
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
