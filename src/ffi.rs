//! Win32 / OpenGL types, constants and import declarations. No logic lives here.
//!
//! This is a deliberately complete hand-written binding surface: entries the
//! current code does not call are kept so the next hook does not have to add
//! them again. Dead-code analysis is therefore off for this module only.
#![allow(dead_code)]

use std::ffi::{c_char, c_void};

// ---- code ----
// ---------------------------------------------------------------- win32 types

pub type Hmodule = *mut c_void;
pub type Hdc = *mut c_void;
pub type Hglrc = *mut c_void;
pub type Hwnd = *mut c_void;
pub type Handle = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// `MSG` for the small modal prompt's message loop.
#[repr(C)]
pub struct Msg {
    pub hwnd: Hwnd,
    pub message: u32,
    pub wparam: usize,
    pub lparam: isize,
    pub time: u32,
    pub pt: Point,
    pub private: u32,
}

/// `WNDCLASSW`. `lpfn_wnd_proc` uses `Option` so a null function pointer has the
/// same layout as the C field.
#[repr(C)]
pub struct WndClassW {
    pub style: u32,
    pub lpfn_wnd_proc: Option<unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize>,
    pub cb_cls_extra: i32,
    pub cb_wnd_extra: i32,
    pub h_instance: Hmodule,
    pub h_icon: Handle,
    pub h_cursor: Handle,
    pub hbr_background: Handle,
    pub lpsz_menu_name: *const u16,
    pub lpsz_class_name: *const u16,
}

#[repr(C)]
pub struct MonitorInfo {
    pub cb_size: u32,
    pub rc_monitor: Rect,
    pub rc_work: Rect,
    pub flags: u32,
}

#[repr(C)]
pub struct MemBasicInfo {
    pub base_address: *mut c_void,
    pub allocation_base: *mut c_void,
    pub allocation_protect: u32,
    pub region_size: usize,
    pub state: u32,
    pub protect: u32,
    pub ty: u32,
}

#[repr(C)]
pub struct ThreadEntry32 {
    pub dw_size: u32,
    pub cnt_usage: u32,
    pub th32_thread_id: u32,
    pub th32_owner_process_id: u32,
    pub tp_base_pri: i32,
    pub tp_delta_pri: i32,
    pub dw_flags: u32,
}

/// `WIN32_FIND_DATAW` for `FindFirstFileW`/`FindNextFileW`.
#[repr(C)]
pub struct Win32FindDataW {
    pub dw_file_attributes: u32,
    pub ft_creation_time: [u32; 2],
    pub ft_last_access_time: [u32; 2],
    pub ft_last_write_time: [u32; 2],
    pub n_file_size_high: u32,
    pub n_file_size_low: u32,
    pub dw_reserved0: u32,
    pub dw_reserved1: u32,
    pub c_file_name: [u16; 260],
    pub c_alternate_file_name: [u16; 14],
}

pub const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;

pub const GWL_STYLE: i32 = -16;
pub const GWL_EXSTYLE: i32 = -20;
pub const GWLP_WNDPROC: i32 = -4;
pub const GW_OWNER: u32 = 4;

pub const WS_VISIBLE: u32 = 0x1000_0000;
pub const WS_CLIPSIBLINGS: u32 = 0x0400_0000;
pub const WS_CLIPCHILDREN: u32 = 0x0200_0000;
pub const WS_CAPTION: u32 = 0x00c0_0000;
pub const WS_SYSMENU: u32 = 0x0008_0000;
pub const WS_THICKFRAME: u32 = 0x0004_0000;
pub const WS_MINIMIZEBOX: u32 = 0x0002_0000;
pub const WS_MAXIMIZEBOX: u32 = 0x0001_0000;
pub const WS_POPUP: u32 = 0x8000_0000;
pub const WS_CHILD: u32 = 0x4000_0000;
pub const WS_EX_TOPMOST: u32 = 0x0000_0008;
pub const WS_EX_APPWINDOW: u32 = 0x0004_0000;
pub const WS_EX_DLGMODALFRAME: u32 = 0x0000_0001;
pub const WS_EX_CONTROLPARENT: u32 = 0x0001_0000;
pub const WS_TABSTOP: u32 = 0x0001_0000;
pub const BS_PUSHBUTTON: u32 = 0x0000_0000;
pub const BS_DEFPUSHBUTTON: u32 = 0x0000_0001;
pub const SS_LEFT: u32 = 0x0000_0000;
pub const GWLP_USERDATA: i32 = -21;
pub const DEFAULT_GUI_FONT: i32 = 17;
pub const IDC_SIZEALL: usize = 32646;
pub const IDOK: isize = 1;
pub const IDCANCEL: isize = 2;
pub const LOGPIXELSX: i32 = 88;

pub const SWP_FRAMECHANGED: u32 = 0x0020;
pub const SWP_SHOWWINDOW: u32 = 0x0040;
pub const SWP_NOMOVE: u32 = 0x0002;
pub const SWP_NOSIZE: u32 = 0x0001;
pub const SWP_NOZORDER: u32 = 0x0004;
pub const SWP_NOACTIVATE: u32 = 0x0010;

pub const SW_MINIMIZE: i32 = 6;
pub const SW_SHOWMINIMIZED: i32 = 2;
pub const SW_SHOWMINNOACTIVE: i32 = 7;
pub const SW_FORCEMINIMIZE: i32 = 11;
pub const SW_MAXIMIZE: i32 = 3;
pub const SW_RESTORE: i32 = 9;

pub const WM_SIZE: u32 = 0x0005;
pub const WM_WINDOWPOSCHANGED: u32 = 0x0047;
pub const WM_ACTIVATEAPP: u32 = 0x001c;
pub const WM_ACTIVATE: u32 = 0x0006;
pub const WA_INACTIVE: u32 = 0;
pub const WM_KEYDOWN: u32 = 0x0100;
pub const WM_TIMER: u32 = 0x0113;
pub const DT_CALCRECT: u32 = 0x0400;
pub const DT_WORDBREAK: u32 = 0x0010;
pub const DT_NOPREFIX: u32 = 0x0800;
pub const VK_LBUTTON: i32 = 0x01;
pub const VK_RBUTTON: i32 = 0x02;
pub const WM_ENTERSIZEMOVE: u32 = 0x0231;
pub const WM_EXITSIZEMOVE: u32 = 0x0232;
pub const WM_CLOSE: u32 = 0x0010;
pub const WM_SYSCOMMAND: u32 = 0x0112;
pub const WM_NCHITTEST: u32 = 0x0084;
pub const WM_SETCURSOR: u32 = 0x0020;
pub const WM_COMMAND: u32 = 0x0111;
pub const WM_DESTROY: u32 = 0x0002;
pub const WM_SETFONT: u32 = 0x0030;
pub const WM_MOUSEMOVE: u32 = 0x0200;
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_LBUTTONDBLCLK: u32 = 0x0203;
pub const WM_RBUTTONDOWN: u32 = 0x0204;
pub const WM_RBUTTONUP: u32 = 0x0205;
pub const WM_RBUTTONDBLCLK: u32 = 0x0206;
pub const WM_MBUTTONDOWN: u32 = 0x0207;
pub const WM_MBUTTONUP: u32 = 0x0208;
pub const WM_MBUTTONDBLCLK: u32 = 0x0209;
pub const WM_MOUSEWHEEL: u32 = 0x020a;
pub const WM_XBUTTONDOWN: u32 = 0x020b;
pub const WM_XBUTTONUP: u32 = 0x020c;

pub const SC_MINIMIZE: usize = 0xF020;
pub const SC_MAXIMIZE: usize = 0xF030;
pub const SC_CLOSE: usize = 0xF060;
pub const SC_RESTORE: usize = 0xF120;

pub const HTCLIENT: isize = 1;
pub const HTCAPTION: isize = 2;
pub const HTLEFT: isize = 10;
pub const HTRIGHT: isize = 11;
pub const HTTOP: isize = 12;
pub const HTTOPLEFT: isize = 13;
pub const HTTOPRIGHT: isize = 14;
pub const HTBOTTOM: isize = 15;
pub const HTBOTTOMLEFT: isize = 16;
pub const HTBOTTOMRIGHT: isize = 17;
pub const HTMAXBUTTON: isize = 9;
pub const HTCLOSE: isize = 20;

pub const HWND_TOPMOST: isize = -1;
pub const HWND_NOTOPMOST: isize = -2;

pub const SM_CXSCREEN: i32 = 0;
pub const SM_CYSCREEN: i32 = 1;
pub const SPI_GETWORKAREA: u32 = 0x0030;
pub const MONITOR_DEFAULTTONEAREST: u32 = 2;

pub const VK_F11: i32 = 0x7a;
pub const VK_F9: i32 = 0x78;
pub const VK_CONTROL: i32 = 0x11;
pub const VK_MENU: i32 = 0x12;
pub const VK_SHIFT: i32 = 0x10;
pub const VK_L: i32 = 0x4c;
pub const VK_ESCAPE: i32 = 0x1b;

pub const DLL_PROCESS_ATTACH: u32 = 1;
pub const LOAD_WITH_ALTERED_SEARCH_PATH: u32 = 0x0000_0008;
pub const PAGE_EXECUTE_READWRITE: u32 = 0x40;
pub const PAGE_READWRITE: u32 = 0x04;
pub const PAGE_WRITECOPY: u32 = 0x08;
pub const PAGE_EXECUTE_WRITECOPY: u32 = 0x80;
pub const MEM_COMMIT: u32 = 0x0000_1000;
/// Region is private to the process (heap, stacks) - not a module image and
/// not a mapped file.
pub const MEM_PRIVATE: u32 = 0x0002_0000;
pub const PAGE_NOACCESS: u32 = 0x01;
pub const PAGE_GUARD: u32 = 0x100;

pub const THREAD_PRIORITY_IDLE: i32 = -15;
pub const THREAD_PRIORITY_BELOW_NORMAL: i32 = -1;
pub const THREAD_QUERY_INFORMATION: u32 = 0x0040;
pub const THREAD_SET_INFORMATION: u32 = 0x0020;
pub const TH32CS_SNAPTHREAD: u32 = 0x0000_0004;

pub const PRIORITY_CLASS: [(i32, u32); 5] = [
    (0, 0x0000_0040), // idle
    (1, 0x0000_4000), // below normal
    (2, 0x0000_0020), // normal
    (3, 0x0000_8000), // above normal
    (4, 0x0000_0080), // high
];

// GL constants used for framebuffer tracking.
pub const GL_FRAMEBUFFER: u32 = 0x8d40;
pub const GL_READ_FRAMEBUFFER: u32 = 0x8ca8;
pub const GL_DRAW_FRAMEBUFFER: u32 = 0x8ca9;

// Minimal fixed-function state used by the mod overlay triangles.
pub const GL_TRIANGLES: u32 = 0x0004;
pub const GL_BLEND: u32 = 0x0be2;
pub const GL_DEPTH_TEST: u32 = 0x0b71;
pub const GL_ALPHA_TEST: u32 = 0x0bc0;
pub const GL_LIGHTING: u32 = 0x0b50;
pub const GL_TEXTURE_2D: u32 = 0x0de1;
pub const GL_SCISSOR_TEST: u32 = 0x0c11;
pub const GL_CULL_FACE: u32 = 0x0b44;
pub const GL_PROJECTION: u32 = 0x1701;
pub const GL_MODELVIEW: u32 = 0x1700;
pub const GL_SRC_ALPHA: u32 = 0x0302;
pub const GL_ONE_MINUS_SRC_ALPHA: u32 = 0x0303;
pub const GL_VERTEX_ARRAY: u32 = 0x8074;
pub const GL_COLOR_ARRAY: u32 = 0x8076;
pub const GL_FLOAT: u32 = 0x1406;
pub const GL_UNSIGNED_BYTE: u32 = 0x1401;

#[link(name = "user32")]
unsafe extern "system" {
    pub fn SetWindowLongW(hwnd: Hwnd, index: i32, value: i32) -> i32;
    pub fn GetWindowLongW(hwnd: Hwnd, index: i32) -> i32;
    pub fn SetWindowPos(
        hwnd: Hwnd,
        after: Hwnd,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        flags: u32,
    ) -> i32;
    pub fn AdjustWindowRect(rect: *mut Rect, style: u32, menu: i32) -> i32;
    pub fn GetClientRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    pub fn GetWindowRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    pub fn ClientToScreen(hwnd: Hwnd, pt: *mut Point) -> i32;
    pub fn GetAsyncKeyState(key: i32) -> i16;
    pub fn FindWindowA(class: *const c_char, title: *const c_char) -> Hwnd;
    pub fn GetClassNameA(hwnd: Hwnd, buf: *mut c_char, len: i32) -> i32;
    pub fn GetSystemMetrics(index: i32) -> i32;
    pub fn SystemParametersInfoW(action: u32, param: u32, data: *mut c_void, win_ini: u32) -> i32;
    pub fn IsIconic(hwnd: Hwnd) -> i32;
    pub fn IsWindow(hwnd: Hwnd) -> i32;
    pub fn IsWindowVisible(hwnd: Hwnd) -> i32;
    pub fn MonitorFromWindow(hwnd: Hwnd, flags: u32) -> Handle;
    pub fn GetMonitorInfoW(monitor: Handle, info: *mut MonitorInfo) -> i32;
    pub fn SetWindowLongPtrW(hwnd: Hwnd, index: i32, value: isize) -> isize;
    pub fn GetWindowLongPtrW(hwnd: Hwnd, index: i32) -> isize;
    pub fn CallWindowProcW(
        prev: isize,
        hwnd: Hwnd,
        msg: u32,
        wparam: usize,
        lparam: isize,
    ) -> isize;
    pub fn DefWindowProcW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    pub fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
    pub fn PostMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> i32;
    pub fn ScreenToClient(hwnd: Hwnd, pt: *mut Point) -> i32;
    pub fn EnumWindows(proc: extern "system" fn(Hwnd, isize) -> i32, param: isize) -> i32;
    pub fn GetWindowThreadProcessId(hwnd: Hwnd, pid: *mut u32) -> u32;
    pub fn GetWindow(hwnd: Hwnd, cmd: u32) -> Hwnd;
    pub fn IsZoomed(hwnd: Hwnd) -> i32;
    pub fn GetForegroundWindow() -> Hwnd;
    pub fn GetWindowLongA(hwnd: Hwnd, index: i32) -> i32;
    pub fn SetWindowsHookExW(
        id: i32,
        proc: unsafe extern "system" fn(i32, usize, isize) -> isize,
        module: Hmodule,
        thread: u32,
    ) -> Handle;
    pub fn CallNextHookEx(hook: Handle, code: i32, wparam: usize, lparam: isize) -> isize;
    pub fn InvalidateRect(hwnd: Hwnd, rect: *const Rect, erase: i32) -> i32;
    pub fn RegisterClassW(class: *const WndClassW) -> u16;
    pub fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: Hwnd,
        menu: Handle,
        instance: Hmodule,
        param: *mut c_void,
    ) -> Hwnd;
    pub fn DestroyWindow(hwnd: Hwnd) -> i32;
    pub fn GetMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
    pub fn TranslateMessage(msg: *const Msg) -> i32;
    pub fn DispatchMessageW(msg: *const Msg) -> isize;
    pub fn IsDialogMessageW(hwnd: Hwnd, msg: *const Msg) -> i32;
    pub fn PostQuitMessage(code: i32);
    pub fn EnableWindow(hwnd: Hwnd, enable: i32) -> i32;
    pub fn GetActiveWindow() -> Hwnd;
    pub fn SetActiveWindow(hwnd: Hwnd) -> Hwnd;
    pub fn GetCursorPos(pt: *mut Point) -> i32;
    pub fn SetTimer(hwnd: Hwnd, id: usize, elapse: u32, func: *const ()) -> usize;
    pub fn KillTimer(hwnd: Hwnd, id: usize) -> i32;
    pub fn SetForegroundWindow(hwnd: Hwnd) -> i32;
    pub fn SetFocus(hwnd: Hwnd) -> Hwnd;
    pub fn SendMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    pub fn SetCursor(cursor: Handle) -> Handle;
    pub fn LoadCursorW(instance: Hmodule, name: *const u16) -> Handle;
    pub fn DrawTextW(dc: Hdc, text: *const u16, count: i32, rect: *mut Rect, format: u32) -> i32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    pub fn GetStockObject(object: i32) -> Handle;
    pub fn GetDC(hwnd: Hwnd) -> Hdc;
    pub fn ReleaseDC(hwnd: Hwnd, dc: Hdc) -> i32;
    pub fn GetDeviceCaps(dc: Hdc, index: i32) -> i32;
    pub fn CreateCompatibleDC(dc: Hdc) -> Hdc;
    pub fn DeleteDC(dc: Hdc) -> i32;
    pub fn SelectObject(dc: Hdc, object: Handle) -> Handle;
}

#[link(name = "shell32")]
unsafe extern "system" {
    pub fn ShellExecuteW(
        hwnd: Hwnd,
        op: *const u16,
        file: *const u16,
        params: *const u16,
        dir: *const u16,
        show: i32,
    ) -> Handle;
}

// ---- WinHTTP (release check). System TLS, no extra crate. ----
pub const WINHTTP_ACCESS_TYPE_DEFAULT_PROXY: u32 = 0;
pub const WINHTTP_FLAG_SECURE: u32 = 0x0080_0000;
pub const WINHTTP_QUERY_STATUS_CODE: u32 = 19;
pub const WINHTTP_QUERY_FLAG_NUMBER: u32 = 0x2000_0000;

#[link(name = "winhttp")]
unsafe extern "system" {
    pub fn WinHttpOpen(
        agent: *const u16,
        access_type: u32,
        proxy: *const u16,
        bypass: *const u16,
        flags: u32,
    ) -> Handle;
    pub fn WinHttpConnect(session: Handle, server: *const u16, port: u16, reserved: u32) -> Handle;
    pub fn WinHttpOpenRequest(
        connect: Handle,
        verb: *const u16,
        path: *const u16,
        version: *const u16,
        referrer: *const u16,
        accept_types: *const *const u16,
        flags: u32,
    ) -> Handle;
    pub fn WinHttpSetTimeouts(
        session: Handle,
        resolve: i32,
        connect: i32,
        send: i32,
        receive: i32,
    ) -> i32;
    pub fn WinHttpSendRequest(
        request: Handle,
        headers: *const u16,
        headers_len: u32,
        optional: *mut c_void,
        optional_len: u32,
        total_len: u32,
        context: usize,
    ) -> i32;
    pub fn WinHttpReceiveResponse(request: Handle, reserved: *mut c_void) -> i32;
    pub fn WinHttpQueryHeaders(
        request: Handle,
        info_level: u32,
        name: *const u16,
        buffer: *mut c_void,
        buffer_len: *mut u32,
        index: *mut u32,
    ) -> i32;
    pub fn WinHttpQueryDataAvailable(request: Handle, available: *mut u32) -> i32;
    pub fn WinHttpReadData(
        request: Handle,
        buffer: *mut c_void,
        to_read: u32,
        read: *mut u32,
    ) -> i32;
    pub fn WinHttpCloseHandle(handle: Handle) -> i32;
}

#[link(name = "winmm")]
unsafe extern "system" {
    pub fn timeBeginPeriod(period: u32) -> u32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    pub fn LoadLibraryW(name: *const u16) -> Hmodule;
    pub fn LoadLibraryExW(name: *const u16, file: Hmodule, flags: u32) -> Hmodule;
    pub fn FreeLibrary(module: Hmodule) -> i32;
    pub fn GetProcAddress(module: Hmodule, name: *const c_char) -> *mut c_void;
    pub fn GetLastError() -> u32;
    pub fn FindFirstFileW(pattern: *const u16, data: *mut Win32FindDataW) -> Handle;
    pub fn FindNextFileW(handle: Handle, data: *mut Win32FindDataW) -> i32;
    pub fn FindClose(handle: Handle) -> i32;
    pub fn GetModuleHandleW(name: *const u16) -> Hmodule;
    pub fn GetModuleFileNameW(module: Hmodule, buf: *mut u16, len: u32) -> u32;
    pub fn GetSystemDirectoryW(buf: *mut u16, len: u32) -> u32;
    pub fn GetCurrentProcessId() -> u32;
    pub fn GetCurrentThreadId() -> u32;
    pub fn GetCurrentProcess() -> Handle;
    pub fn GetTickCount() -> u32;
    pub fn CopyFileW(from: *const u16, to: *const u16, fail_if_exists: i32) -> i32;
    pub fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, len: u32) -> u32;
    pub fn CreateDirectoryW(path: *const u16, sa: *mut c_void) -> i32;
    pub fn DisableThreadLibraryCalls(module: Hmodule) -> i32;
    pub fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
    pub fn VirtualProtect(addr: *mut c_void, size: usize, new: u32, old: *mut u32) -> i32;
    pub fn VirtualQuery(addr: *const c_void, info: *mut MemBasicInfo, len: usize) -> usize;
    /// Fault-safe memory read: returns 0 instead of raising an exception when
    /// any page in the range is inaccessible. Used by the heap scan so a
    /// region freed mid-walk cannot kill the process.
    pub fn ReadProcessMemory(
        process: Handle,
        base: *const c_void,
        buffer: *mut c_void,
        size: usize,
        read: *mut usize,
    ) -> i32;
    pub fn QueryPerformanceCounter(value: *mut i64) -> i32;
    pub fn QueryPerformanceFrequency(value: *mut i64) -> i32;
    pub fn Sleep(ms: u32);
    pub fn CreateThread(
        attr: *mut c_void,
        stack: usize,
        start: extern "system" fn(*mut c_void) -> u32,
        param: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    pub fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
    pub fn Thread32First(snap: Handle, entry: *mut ThreadEntry32) -> i32;
    pub fn Thread32Next(snap: Handle, entry: *mut ThreadEntry32) -> i32;
    pub fn OpenThread(access: u32, inherit: i32, tid: u32) -> Handle;
    pub fn GetThreadPriority(thread: Handle) -> i32;
    pub fn SetThreadPriority(thread: Handle, priority: i32) -> i32;
    pub fn SetPriorityClass(process: Handle, class: u32) -> i32;
    pub fn SetProcessAffinityMask(process: Handle, mask: usize) -> i32;
    pub fn GetProcessAffinityMask(
        process: Handle,
        process_mask: *mut usize,
        system_mask: *mut usize,
    ) -> i32;
    pub fn GetCurrentThread() -> Handle;
    pub fn SetThreadAffinityMask(thread: Handle, mask: usize) -> usize;
    pub fn CloseHandle(handle: Handle) -> i32;
    pub fn TerminateProcess(process: Handle, exit_code: u32) -> !;
    pub fn ExitProcess(exit_code: u32) -> !;
    pub fn AddVectoredExceptionHandler(
        first: u32,
        handler: unsafe extern "system" fn(*mut ExceptionPointers) -> i32,
    ) -> *mut c_void;
}

pub const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
pub const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;

#[repr(C)]
pub struct ExceptionRecord {
    pub code: u32,
    pub flags: u32,
    pub record: *mut ExceptionRecord,
    pub address: *mut c_void,
    pub number_parameters: u32,
    pub information: [usize; 15],
}

#[repr(C)]
pub struct ExceptionPointers {
    pub record: *mut ExceptionRecord,
    pub context: *mut c_void,
}
