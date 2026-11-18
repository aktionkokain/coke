//! Standalone prompt used by the opt-in update check.
//!
//! The game has no comctl32 v6 manifest, so `TaskDialogIndirect` is not
//! available. This builds a plain top-level Win32 window with a heading,
//! wrapped body text and any number of push buttons, sized from the measured
//! text so nothing is ever clipped.
//!
//! It is deliberately **not** owned by the game: it gets its own taskbar button
//! and Alt+Tab entry, and clicking the game leaves it open instead of throwing
//! it away. Only the close box, Escape or a button finishes it; the caller
//! supplies a distinct "decide later" result for the first two.

use crate::*;
use std::sync::OnceLock;

const CLASS_NAME: &str = "CokePrompt";

struct PromptState {
    result: i32,
    default_result: i32,
    cancel_result: i32,
    controls: Vec<(isize, i32)>,
    /// Set when a result has been recorded, so a late message cannot overwrite
    /// a real button answer.
    finished: bool,
}

static CLASS_REGISTERED: OnceLock<()> = OnceLock::new();

fn class_registered() -> &'static OnceLock<()> {
    &CLASS_REGISTERED
}

unsafe extern "system" fn prompt_wndproc(
    hwnd: Hwnd,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    unsafe {
        let state_ptr = GetWindowLongW(hwnd, GWLP_USERDATA) as isize as *mut PromptState;
        match msg {
            WM_COMMAND => {
                if !state_ptr.is_null() {
                    let state = &mut *state_ptr;
                    let id = (wparam & 0xffff) as isize;
                    let result = if state.finished {
                        None
                    } else if id == IDOK {
                        Some(state.default_result)
                    } else if id == IDCANCEL {
                        Some(state.cancel_result)
                    } else {
                        state
                            .controls
                            .iter()
                            .find(|(control, _)| *control == id)
                            .map(|(_, value)| *value)
                    };
                    if let Some(result) = result {
                        state.result = result;
                        state.finished = true;
                        DestroyWindow(hwnd);
                    }
                }
                0
            }
            WM_KEYDOWN if wparam as i32 == VK_ESCAPE => {
                if !state_ptr.is_null() && !(*state_ptr).finished {
                    (*state_ptr).result = (*state_ptr).cancel_result;
                    (*state_ptr).finished = true;
                    DestroyWindow(hwnd);
                }
                0
            }
            WM_CLOSE => {
                if !state_ptr.is_null() && !(*state_ptr).finished {
                    (*state_ptr).result = (*state_ptr).cancel_result;
                    (*state_ptr).finished = true;
                }
                DestroyWindow(hwnd);
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// Scale factor in hundredths (100 = 96 dpi).
unsafe fn dpi_percent() -> i32 {
    unsafe {
        let dc = GetDC(core::ptr::null_mut());
        if dc.is_null() {
            return 100;
        }
        let dpi = GetDeviceCaps(dc, LOGPIXELSX);
        ReleaseDC(core::ptr::null_mut(), dc);
        if dpi <= 0 {
            100
        } else {
            (dpi * 100 / 96).clamp(75, 400)
        }
    }
}

fn scaled(value: i32, percent: i32) -> i32 {
    (value * percent / 100).max(1)
}

/// Height of `text` wrapped to `width`, measured with the prompt's font.
unsafe fn measure_text(text: &str, width: i32, font: Handle) -> i32 {
    unsafe {
        let dc = CreateCompatibleDC(core::ptr::null_mut());
        if dc.is_null() {
            return 0;
        }
        let old = SelectObject(dc, font);
        let text = wide(text);
        let mut rect = Rect {
            left: 0,
            top: 0,
            right: width.max(1),
            bottom: 0,
        };
        DrawTextW(
            dc,
            text.as_ptr(),
            -1,
            &mut rect,
            DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX,
        );
        if !old.is_null() {
            SelectObject(dc, old);
        }
        DeleteDC(dc);
        (rect.bottom - rect.top).max(0)
    }
}

/// Show a modal prompt owned by `owner` and block this thread until a button is
/// pressed. Returns `default_result` if the window could not be created.
///
/// `buttons` are `(label, result)` pairs. `default_result` is used for Enter and
/// is focused on open; `cancel_result` is used for Escape and the close box.
pub unsafe fn prompt(
    owner: Hwnd,
    title: &str,
    heading: &str,
    body: &str,
    buttons: &[(&str, i32)],
    default_result: i32,
    cancel_result: i32,
) -> i32 {
    unsafe {
        if buttons.is_empty() {
            return default_result;
        }
        let instance = GetModuleHandleW(core::ptr::null());
        class_registered().get_or_init(|| {
            let cursor = LoadCursorW(core::ptr::null_mut(), core::ptr::null());
            let class_name = wide(CLASS_NAME);
            let class = WndClassW {
                style: 0,
                lpfn_wnd_proc: Some(prompt_wndproc),
                cb_cls_extra: 0,
                cb_wnd_extra: 0,
                h_instance: instance,
                h_icon: core::ptr::null_mut(),
                h_cursor: cursor,
                hbr_background: (5 + 1) as Handle, // COLOR_WINDOW + 1
                lpsz_menu_name: core::ptr::null(),
                lpsz_class_name: class_name.as_ptr(),
            };
            // RegisterClassW copies the name; the vector only has to live
            // until the call returns.
            RegisterClassW(&class);
        });

        let pct = dpi_percent();
        let margin = scaled(16, pct);
        let gap = scaled(8, pct);
        let button_h = scaled(26, pct);
        let button_pad = scaled(28, pct);
        let char_w = scaled(7, pct);

        let widths: Vec<i32> = buttons
            .iter()
            .map(|(label, _)| {
                (label.chars().count() as i32 * char_w + button_pad).max(scaled(90, pct))
            })
            .collect();
        let buttons_w: i32 = widths.iter().sum::<i32>() + gap * (buttons.len() as i32 - 1).max(0);

        // Measure the real text with the real font, then size the window from
        // it. If the text is tall, widen the dialog (up to the work area) so
        // lines get shorter instead of the button row leaving the screen.
        let font = GetStockObject(DEFAULT_GUI_FONT);
        let wa = work_area();
        let base_w = (buttons_w + margin * 2).max(scaled(430, pct));
        let max_client_w = (wa.right - wa.left - scaled(64, pct)).max(base_w);
        let max_client_h = (wa.bottom - wa.top - scaled(64, pct)).max(scaled(240, pct));

        let mut client_w = base_w;
        let (heading_h, body_h, client_h) = loop {
            let label_w = client_w - margin * 2;
            let heading_h = measure_text(heading, label_w, font).max(scaled(16, pct));
            let body_h = measure_text(body, label_w, font).max(scaled(16, pct)) + scaled(2, pct);
            let client_h = margin + heading_h + gap + body_h + gap + button_h + margin;
            if client_h <= max_client_h || client_w >= max_client_w {
                break (heading_h, body_h, client_h);
            }
            client_w = (client_w + scaled(80, pct)).min(max_client_w);
        };
        let label_w = client_w - margin * 2;

        // Lay the prompt out in *client* coordinates, then convert the whole
        // thing to an outer rect. CreateWindowExW takes the outer size, so
        // without AdjustWindowRect the caption ate ~31 px of the client area
        // and clipped the button row off the bottom.
        let style = WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_VISIBLE;
        let ex_style = WS_EX_CONTROLPARENT | WS_EX_APPWINDOW | WS_EX_TOPMOST;
        let mut outer = Rect {
            left: 0,
            top: 0,
            right: client_w,
            bottom: client_h,
        };
        AdjustWindowRect(&mut outer, style, 0);
        let outer_w = (outer.right - outer.left).max(client_w);
        let outer_h = (outer.bottom - outer.top).max(client_h);

        // Centre over the game window when we can see it, else the work area.
        let center = || {
            (
                wa.left + (wa.right - wa.left - outer_w) / 2,
                wa.top + (wa.bottom - wa.top - outer_h) / 2,
            )
        };
        let (x, y) = if !owner.is_null() {
            let mut r = Rect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            if GetWindowRect(owner, &mut r) != 0 {
                (
                    r.left + (r.right - r.left - outer_w) / 2,
                    r.top + (r.bottom - r.top - outer_h) / 2,
                )
            } else {
                center()
            }
        } else {
            center()
        };
        let x = x.clamp(wa.left, (wa.right - outer_w).max(wa.left));
        let y = y.clamp(wa.top, (wa.bottom - outer_h).max(wa.top));

        let state = Box::into_raw(Box::new(PromptState {
            result: default_result,
            default_result,
            cancel_result,
            controls: Vec::new(),
            finished: false,
        }));

        let hwnd = CreateWindowExW(
            ex_style,
            wide(CLASS_NAME).as_ptr(),
            wide(title).as_ptr(),
            style,
            x,
            y,
            outer_w,
            outer_h,
            core::ptr::null_mut(), // not owned: own taskbar button and Alt+Tab entry
            core::ptr::null_mut(),
            instance,
            core::ptr::null_mut(),
        );
        if hwnd.is_null() {
            log("prompt: CreateWindowExW failed");
            drop(Box::from_raw(state));
            return default_result;
        }
        SetWindowLongW(hwnd, GWLP_USERDATA, state as isize as i32);

        let body_y = margin + heading_h + gap;
        let heading_ctl = CreateWindowExW(
            0,
            wide("STATIC").as_ptr(),
            wide(heading).as_ptr(),
            WS_CHILD | WS_VISIBLE | SS_LEFT,
            margin,
            margin,
            label_w,
            heading_h,
            hwnd,
            core::ptr::null_mut(),
            instance,
            core::ptr::null_mut(),
        );
        let body_ctl = CreateWindowExW(
            0,
            wide("STATIC").as_ptr(),
            wide(body).as_ptr(),
            WS_CHILD | WS_VISIBLE | SS_LEFT,
            margin,
            body_y,
            label_w,
            body_h,
            hwnd,
            core::ptr::null_mut(),
            instance,
            core::ptr::null_mut(),
        );
        for ctl in [heading_ctl, body_ctl] {
            if !ctl.is_null() {
                SendMessageW(ctl, WM_SETFONT, font as usize, 1);
            }
        }

        let button_y = client_h - margin - button_h;
        let mut button_x = margin;
        let mut default_ctl = core::ptr::null_mut();
        for (i, (label, result)) in buttons.iter().enumerate() {
            let control_id = 1000 + i as isize;
            let is_default = *result == default_result;
            let bs = if is_default {
                BS_DEFPUSHBUTTON
            } else {
                BS_PUSHBUTTON
            };
            let ctl = CreateWindowExW(
                0,
                wide("BUTTON").as_ptr(),
                wide(label).as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | bs,
                button_x,
                button_y,
                widths[i],
                button_h,
                hwnd,
                control_id as Handle,
                instance,
                core::ptr::null_mut(),
            );
            if !ctl.is_null() {
                SendMessageW(ctl, WM_SETFONT, font as usize, 1);
                (*state).controls.push((control_id, *result));
                if is_default {
                    default_ctl = ctl;
                }
            }
            button_x += widths[i] + gap;
        }

        // The window is independent and topmost, so clicking the game leaves it
        // open (and on the taskbar/Alt+Tab) instead of discarding it.
        SetForegroundWindow(hwnd);
        if !default_ctl.is_null() {
            SetFocus(default_ctl);
        }

        let mut msg = Msg {
            hwnd: core::ptr::null_mut(),
            message: 0,
            wparam: 0,
            lparam: 0,
            time: 0,
            pt: Point { x: 0, y: 0 },
            private: 0,
        };
        while GetMessageW(&mut msg, core::ptr::null_mut(), 0, 0) > 0 {
            if IsDialogMessageW(hwnd, &msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        if !owner.is_null() && IsWindow(owner) != 0 {
            SetForegroundWindow(owner);
        }

        // One owner for the allocation: `state` is the only Box, so there is no
        // second free here (the double free used to corrupt the game's heap).
        let result = (*state).result;
        drop(Box::from_raw(state));
        result
    }
}

/// Open a URL in the user's default browser.
pub unsafe fn open_url(url: &str) {
    unsafe {
        let op = wide("open");
        let target = wide(url);
        ShellExecuteW(
            core::ptr::null_mut(),
            op.as_ptr(),
            target.as_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            1, // SW_SHOWNORMAL
        );
    }
}
