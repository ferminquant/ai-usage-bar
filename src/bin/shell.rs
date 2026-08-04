#[path = "../shell_logic.rs"]
#[allow(dead_code)]
mod shell_logic;

#[cfg(not(windows))]
fn main() {
    eprintln!("ai-usage-bar-shell is only available on Windows");
}

#[cfg(windows)]
mod windows_shell {
    use ai_usage_bar::{
        build_tray_view, CodexAdapter, ProviderRegistry, RefreshPolicy, RefreshService,
        UsageSnapshot,
    };
    use std::ffi::c_void;
    use std::ptr::null_mut;
    use std::sync::Arc;
    use std::thread;

    use super::shell_logic::{
        normalize_used_percent, render_detail_text, usage_band, UsageBand,
    };

    use windows::core::*;
    use windows::Win32::Foundation::*;
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::UI::Controls::*;
    use windows::Win32::UI::HiDpi::*;
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    const WIDGET_W: i32 = 158;
    const WIDGET_H: i32 = 44;
    const CARD_W: i32 = WIDGET_W;
    const CARD_H: i32 = 40;
    const SCREEN_MARGIN: i32 = 12;
    const TASKBAR_GAP: i32 = 4;
    const REFRESH_INTERVAL_MS: u32 = 60_000;
    const REFRESH_TIMER_ID: usize = 1;
    const POSITION_TIMER_ID: usize = 2;
    const TOOLTIP_POLL_TIMER_ID: usize = 3;
    const POSITION_TIMER_INTERVAL_MS: u32 = 1_000;
    const TOOLTIP_POLL_INTERVAL_MS: u32 = 100;

    const MENU_REFRESH: usize = 1001;
    const MENU_COPY_DETAILS: usize = 1002;
    const MENU_QUIT: usize = 1003;
    const STATUS_TIMER_ID: usize = 4;
    const STATUS_INTERVAL_MS: u32 = 2_500;
    const CF_UNICODETEXT_FORMAT: u32 = 13;

    const WM_APP_REFRESH_DONE: u32 = 0x8000 + 1;

    const COLOR_BACKGROUND: COLORREF = COLORREF(0x002a2a2a);
    const COLOR_OUTER: COLORREF = COLORREF(0x00141414);
    const COLOR_BORDER: COLORREF = COLORREF(0x00555555);
    const COLOR_CARD_BORDER: COLORREF = COLORREF(0x006a6a6a);
    const COLOR_NEUTRAL: COLORREF = COLORREF(0x009e9e9e);
    const COLOR_GREEN: COLORREF = COLORREF(0x0050af4c);
    const COLOR_YELLOW: COLORREF = COLORREF(0x0007c1ff);
    const COLOR_RED: COLORREF = COLORREF(0x003643f4);
    const COLOR_TEXT: COLORREF = COLORREF(0x00f5f5f5);
    const COLOR_MUTED: COLORREF = COLORREF(0x00c0c0c0);

    struct AppState {
        refresh_service: Arc<RefreshService>,
        snapshots: Vec<UsageSnapshot>,
        used_percent: Option<f64>,
        tooltip: String,
        tooltip_hwnd: Option<HWND>,
        tooltip_text: Vec<u16>,
        tooltip_visible: bool,
        refresh_in_flight: bool,
        status: Option<String>,
    }

    struct RefreshPayload {
        result: std::result::Result<Vec<UsageSnapshot>, String>,
    }

    impl AppState {
        fn loading(refresh_service: Arc<RefreshService>) -> Self {
            Self {
                refresh_service,
                snapshots: Vec::new(),
                used_percent: None,
                tooltip: "AI Usage Bar — loading…".to_string(),
                tooltip_hwnd: None,
                tooltip_text: Vec::new(),
                tooltip_visible: false,
                refresh_in_flight: false,
                status: None,
            }
        }
    }

    fn to_wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn app_state(hwnd: HWND) -> Option<&'static mut AppState> {
        let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut AppState;
        if ptr.is_null() {
            None
        } else {
            // The pointer is owned by the window and lives until WM_NCDESTROY.
            Some(unsafe { &mut *ptr })
        }
    }

    fn app_state_ref(hwnd: HWND) -> Option<&'static AppState> {
        let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const AppState;
        if ptr.is_null() {
            None
        } else {
            // The pointer is owned by the window and lives until WM_NCDESTROY.
            Some(unsafe { &*ptr })
        }
    }

    fn percent_and_color(used_percent: Option<f64>) -> (Option<f64>, COLORREF) {
        let percent = normalize_used_percent(used_percent);
        let color = match usage_band(used_percent) {
            UsageBand::Neutral => COLOR_NEUTRAL,
            UsageBand::Green => COLOR_GREEN,
            UsageBand::Yellow => COLOR_YELLOW,
            UsageBand::Red => COLOR_RED,
        };
        (percent, color)
    }

    fn fill_rect(hdc: HDC, rect: RECT, color: COLORREF) {
        unsafe {
            let brush = CreateSolidBrush(color);
            let _ = FillRect(hdc, &rect, brush);
            let _ = DeleteObject(brush.into());
        }
    }

    fn fill_round_rect(hdc: HDC, rect: RECT, radius: i32, color: COLORREF) {
        unsafe {
            let brush = CreateSolidBrush(color);
            let pen = CreatePen(PS_SOLID, 1, color);
            let old_brush = SelectObject(hdc, brush.into());
            let old_pen = SelectObject(hdc, pen.into());
            let _ = RoundRect(
                hdc,
                rect.left,
                rect.top,
                rect.right,
                rect.bottom,
                radius,
                radius,
            );
            SelectObject(hdc, old_brush);
            SelectObject(hdc, old_pen);
            let _ = DeleteObject(brush.into());
            let _ = DeleteObject(pen.into());
        }
    }

    fn stroke_round_rect(hdc: HDC, rect: RECT, radius: i32, color: COLORREF) {
        unsafe {
            let pen = CreatePen(PS_SOLID, 1, color);
            let old_pen = SelectObject(hdc, pen.into());
            let old_brush = SelectObject(hdc, GetStockObject(NULL_BRUSH));
            let _ = RoundRect(
                hdc,
                rect.left,
                rect.top,
                rect.right,
                rect.bottom,
                radius,
                radius,
            );
            SelectObject(hdc, old_brush);
            SelectObject(hdc, old_pen);
            let _ = DeleteObject(pen.into());
        }
    }

    fn draw_text(
        hdc: HDC,
        rect: RECT,
        text: &str,
        height: i32,
        weight: FONT_WEIGHT,
        color: COLORREF,
        format: DRAW_TEXT_FORMAT,
    ) {
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        if wide.is_empty() {
            return;
        }

        unsafe {
            let font = CreateFontW(
                -height,
                0,
                0,
                0,
                weight.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                DEFAULT_QUALITY,
                DEFAULT_PITCH.0 as u32 | FF_DONTCARE.0 as u32,
                w!("Segoe UI"),
            );
            let old_font = SelectObject(hdc, font.into());
            let old_color = SetTextColor(hdc, color);
            let old_background = SetBkMode(hdc, TRANSPARENT);
            let mut draw_rect = rect;
            let _ = DrawTextW(hdc, &mut wide, &mut draw_rect, format);
            let _ = SetBkMode(hdc, BACKGROUND_MODE(old_background as u32));
            let _ = SetTextColor(hdc, old_color);
            SelectObject(hdc, old_font);
            let _ = DeleteObject(font.into());
        }
    }

    fn paint_widget(
        hwnd: HWND,
        used_percent: Option<f64>,
        refreshing: bool,
        status: Option<&str>,
    ) {
        unsafe {
            let mut paint = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut paint);
            let client = RECT {
                left: 0,
                top: 0,
                right: WIDGET_W,
                bottom: WIDGET_H,
            };
            fill_rect(hdc, client, COLOR_OUTER);

            let card = RECT {
                left: 0,
                top: 0,
                right: CARD_W,
                bottom: CARD_H,
            };
            fill_round_rect(hdc, card, 18, COLOR_BACKGROUND);
            stroke_round_rect(hdc, card, 18, COLOR_CARD_BORDER);

            let (used, color) = percent_and_color(used_percent);
            let remaining = used.map(|value| (100.0 - value).clamp(0.0, 100.0));
            let remaining_label = remaining
                .map(|value| format!("{value:.0}%"))
                .unwrap_or_else(|| "—".to_string());
            let status_label = status.unwrap_or(if refreshing {
                "Updating…"
            } else {
                "Codex left"
            });
            draw_text(
                hdc,
                RECT {
                    left: 14,
                    top: 2,
                    right: 82,
                    bottom: 29,
                },
                &remaining_label,
                21,
                FW_BOLD,
                COLOR_TEXT,
                DT_SINGLELINE | DT_VCENTER | DT_LEFT,
            );
            draw_text(
                hdc,
                RECT {
                    left: 88,
                    top: 4,
                    right: CARD_W - 8,
                    bottom: 27,
                },
                status_label,
                11,
                FW_NORMAL,
                COLOR_MUTED,
                DT_SINGLELINE | DT_VCENTER | DT_LEFT,
            );

            let track = RECT {
                left: 17,
                top: 33,
                right: CARD_W - 16,
                bottom: 36,
            };
            fill_round_rect(hdc, track, 3, COLOR_BORDER);
            if let Some(remaining) = remaining {
                let fill_width = (((track.right - track.left) as f64 * remaining) / 100.0)
                    .round()
                    .clamp(0.0, (track.right - track.left) as f64)
                    as i32;
                if fill_width > 0 {
                    fill_round_rect(
                        hdc,
                        RECT {
                            left: track.left,
                            top: track.top,
                            right: track.left + fill_width,
                            bottom: track.bottom,
                        },
                        3,
                        color,
                    );
                }
            }

            let _ = EndPaint(hwnd, &paint);
        }
    }

    fn monitor_work_area() -> Option<(RECT, RECT)> {
        unsafe {
            let monitor = MonitorFromWindow(HWND(null_mut()), MONITOR_DEFAULTTOPRIMARY);
            if monitor.0.is_null() {
                return None;
            }
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if !GetMonitorInfoW(monitor, &mut info).as_bool() {
                return None;
            }
            Some((info.rcMonitor, info.rcWork))
        }
    }

    fn taskbar_rects() -> Option<(RECT, RECT)> {
        unsafe {
            let taskbar = FindWindowW(w!("Shell_TrayWnd"), None).ok()?;
            let tray = FindWindowExW(Some(taskbar), None, w!("TrayNotifyWnd"), None).ok()?;
            let mut taskbar_rect = RECT::default();
            let mut tray_rect = RECT::default();
            GetWindowRect(taskbar, &mut taskbar_rect).ok()?;
            GetWindowRect(tray, &mut tray_rect).ok()?;
            Some((taskbar_rect, tray_rect))
        }
    }

    fn clamp_to_monitor(x: i32, y: i32, monitor: RECT) -> (i32, i32) {
        let max_x = (monitor.right - WIDGET_W).max(monitor.left);
        let max_y = (monitor.bottom - WIDGET_H).max(monitor.top);
        (x.clamp(monitor.left, max_x), y.clamp(monitor.top, max_y))
    }

    fn compute_widget_pos() -> (i32, i32) {
        if let Some((taskbar, tray)) = taskbar_rects() {
            let taskbar_width = taskbar.right - taskbar.left;
            let taskbar_height = taskbar.bottom - taskbar.top;
            let (x, y) = if taskbar_width >= taskbar_height {
                (
                    tray.left - WIDGET_W - TASKBAR_GAP,
                    taskbar.top + ((taskbar_height - WIDGET_H) / 2).max(0),
                )
            } else {
                let x = taskbar.left + ((taskbar_width - WIDGET_W) / 2).max(0);
                let y = if tray.top > taskbar.top {
                    tray.top - WIDGET_H - TASKBAR_GAP
                } else {
                    tray.bottom + TASKBAR_GAP
                };
                (x, y)
            };

            if let Some((monitor, _)) = monitor_work_area() {
                return clamp_to_monitor(x, y, monitor);
            }
            return (x, y);
        }

        let Some((monitor, work)) = monitor_work_area() else {
            return (0, 0);
        };

        // rcWork excludes the taskbar. Infer which edge it occupies instead of
        // trusting ABM_GETTASKBARPOS, whose coordinates can be virtualized by DPI.
        let (x, y) = if work.bottom < monitor.bottom && work.top == monitor.top {
            (
                work.right - WIDGET_W - SCREEN_MARGIN,
                work.bottom - WIDGET_H - TASKBAR_GAP,
            )
        } else if work.top > monitor.top && work.bottom == monitor.bottom {
            (
                work.right - WIDGET_W - SCREEN_MARGIN,
                work.top + TASKBAR_GAP,
            )
        } else if work.right < monitor.right && work.left == monitor.left {
            (
                work.right + TASKBAR_GAP,
                work.bottom - WIDGET_H - SCREEN_MARGIN,
            )
        } else if work.left > monitor.left && work.right == monitor.right {
            (
                work.left - WIDGET_W - TASKBAR_GAP,
                work.bottom - WIDGET_H - SCREEN_MARGIN,
            )
        } else {
            (
                work.right - WIDGET_W - SCREEN_MARGIN,
                work.bottom - WIDGET_H - SCREEN_MARGIN,
            )
        };

        clamp_to_monitor(x, y, monitor)
    }

    fn relocate_widget(hwnd: HWND) {
        let (x, y) = compute_widget_pos();
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                WIDGET_W,
                WIDGET_H,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
    }

    fn ensure_widget_topmost(hwnd: HWND) {
        unsafe {
            let mut widget_rect = RECT::default();
            let Ok(()) = GetWindowRect(hwnd, &mut widget_rect) else {
                return;
            };
            let center = POINT {
                x: (widget_rect.left + widget_rect.right) / 2,
                y: (widget_rect.top + widget_rect.bottom) / 2,
            };
            let Ok(taskbar) = FindWindowW(w!("Shell_TrayWnd"), None) else {
                return;
            };
            if WindowFromPoint(center) == taskbar {
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            }
        }
    }

    fn make_tool_info(hwnd: HWND, text: &mut [u16]) -> TTTOOLINFOW {
        TTTOOLINFOW {
            // comctl32 currently accepts the V2 TOOLINFO layout (through
            // lParam) but rejects the newer lpReserved-inclusive size.
            cbSize: (std::mem::size_of::<TTTOOLINFOW>() - std::mem::size_of::<*mut c_void>())
                as u32,
            uFlags: TTF_TRACK | TTF_ABSOLUTE,
            hwnd,
            uId: 1,
            rect: RECT {
                left: 0,
                top: 0,
                right: WIDGET_W,
                bottom: WIDGET_H,
            },
            lpszText: PWSTR(text.as_mut_ptr()),
            ..Default::default()
        }
    }

    fn set_tooltip_visible(hwnd: HWND, state: &mut AppState, visible: bool) {
        let Some(tooltip_hwnd) = state.tooltip_hwnd else {
            return;
        };

        unsafe {
            let state_changed = state.tooltip_visible != visible;
            if visible {
                // Keep the tooltip above the pill instead of placing it on the
                // cursor. This leaves both halves of the widget clickable while
                // the native tooltip remains visible.
                let mut widget_rect = RECT::default();
                let (tooltip_x, tooltip_y) = if GetWindowRect(hwnd, &mut widget_rect).is_ok() {
                    (widget_rect.left, widget_rect.top.saturating_sub(72))
                } else {
                    let mut point = POINT::default();
                    let _ = GetCursorPos(&mut point);
                    (point.x.saturating_sub(40), point.y.saturating_sub(90))
                };
                let packed_point =
                    ((tooltip_y as u32 & 0xffff) << 16) | (tooltip_x as u32 & 0xffff);
                let _ = SendMessageW(
                    tooltip_hwnd,
                    TTM_TRACKPOSITION,
                    None,
                    Some(LPARAM(packed_point as isize)),
                );
            }
            if !state_changed {
                return;
            }

            state.tooltip_visible = visible;
            if visible {
                let _ = SetTimer(
                    Some(hwnd),
                    TOOLTIP_POLL_TIMER_ID,
                    TOOLTIP_POLL_INTERVAL_MS,
                    None,
                );
            } else {
                let _ = KillTimer(Some(hwnd), TOOLTIP_POLL_TIMER_ID);
            }
            let mut tooltip_text = state.tooltip_text.clone();
            let mut tool = make_tool_info(hwnd, &mut tooltip_text);
            let _ = SendMessageW(
                tooltip_hwnd,
                TTM_TRACKACTIVATE,
                Some(WPARAM(usize::from(visible))),
                Some(LPARAM(&mut tool as *mut TTTOOLINFOW as isize)),
            );
            if !visible {
                let _ = SendMessageW(tooltip_hwnd, TTM_POP, None, None);
            }
        }
    }

    fn cursor_over_widget_or_tooltip(hwnd: HWND, state: &AppState) -> bool {
        unsafe {
            let mut point = POINT::default();
            if GetCursorPos(&mut point).is_err() {
                return false;
            }
            let window = WindowFromPoint(point);
            window == hwnd || state.tooltip_hwnd == Some(window)
        }
    }

    fn update_tooltip(hwnd: HWND, state: &mut AppState) {
        state.tooltip_text = to_wide(&state.tooltip);
        let Some(tooltip_hwnd) = state.tooltip_hwnd else {
            return;
        };

        let mut tool = make_tool_info(hwnd, &mut state.tooltip_text);
        unsafe {
            let _ = SendMessageW(
                tooltip_hwnd,
                TTM_UPDATETIPTEXTW,
                None,
                Some(LPARAM(&mut tool as *mut TTTOOLINFOW as isize)),
            );
            let _ = SendMessageW(tooltip_hwnd, TTM_UPDATE, None, None);
        }
    }

    fn create_tooltip(hwnd: HWND, hinst: HINSTANCE, state: &mut AppState) {
        unsafe {
            InitCommonControls();
            let Ok(tooltip_hwnd) = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                TOOLTIPS_CLASSW,
                w!(""),
                WINDOW_STYLE(TTS_ALWAYSTIP | TTS_NOPREFIX),
                0,
                0,
                0,
                0,
                Some(hwnd),
                None,
                Some(hinst),
                None,
            ) else {
                eprintln!("failed to create tooltip window");
                return;
            };

            state.tooltip_hwnd = Some(tooltip_hwnd);
            state.tooltip_text = to_wide(&state.tooltip);
            let mut tool = make_tool_info(hwnd, &mut state.tooltip_text);
            let added = SendMessageW(
                tooltip_hwnd,
                TTM_ADDTOOLW,
                None,
                Some(LPARAM(&mut tool as *mut TTTOOLINFOW as isize)),
            );
            if added.0 == 0 {
                eprintln!("failed to add tooltip tool");
                let _ = DestroyWindow(tooltip_hwnd);
                state.tooltip_hwnd = None;
                return;
            }

            // Keep longer provider details readable instead of forcing one line.
            let _ = SendMessageW(tooltip_hwnd, TTM_SETMAXTIPWIDTH, None, Some(LPARAM(360)));
        }
    }

    fn copy_text_to_clipboard(hwnd: HWND, text: &str) -> std::result::Result<(), String> {
        let wide = to_wide(text);
        unsafe {
            OpenClipboard(Some(hwnd)).map_err(|error| format!("open clipboard: {error}"))?;
            let result = (|| {
                EmptyClipboard().map_err(|error| format!("empty clipboard: {error}"))?;
                let bytes = wide
                    .len()
                    .checked_mul(std::mem::size_of::<u16>())
                    .ok_or_else(|| "clipboard text is too large".to_string())?;
                let memory = GlobalAlloc(GMEM_MOVEABLE, bytes)
                    .map_err(|error| format!("allocate clipboard memory: {error}"))?;
                let locked = GlobalLock(memory);
                if locked.is_null() {
                    let _ = GlobalFree(Some(memory));
                    return Err("lock clipboard memory".to_string());
                }
                std::ptr::copy_nonoverlapping(wide.as_ptr(), locked.cast::<u16>(), wide.len());
                let _ = GlobalUnlock(memory);

                match SetClipboardData(CF_UNICODETEXT_FORMAT, Some(HANDLE(memory.0))) {
                    Ok(_) => Ok(()),
                    Err(error) => {
                        let _ = GlobalFree(Some(memory));
                        Err(format!("set clipboard data: {error}"))
                    }
                }
            })();
            let close_result = CloseClipboard();
            result.and(close_result.map_err(|error| format!("close clipboard: {error}")))
        }
    }

    fn set_status(hwnd: HWND, state: &mut AppState, message: &str) {
        state.status = Some(message.to_string());
        unsafe {
            let _ = SetTimer(Some(hwnd), STATUS_TIMER_ID, STATUS_INTERVAL_MS, None);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }

    fn copy_details_to_clipboard(hwnd: HWND) {
        let details = app_state_ref(hwnd)
            .map(|state| render_detail_text(&state.snapshots))
            .unwrap_or_else(|| "No provider data".to_string());
        let result = copy_text_to_clipboard(hwnd, &details);
        if let Some(state) = app_state(hwnd) {
            match result {
                Ok(()) => set_status(hwnd, state, "Copied!"),
                Err(error) => {
                    eprintln!("clipboard copy failed: {error}");
                    set_status(hwnd, state, "Copy failed");
                }
            }
        }
    }

    fn begin_refresh(hwnd: HWND) {
        let Some(state) = app_state(hwnd) else {
            return;
        };
        if state.refresh_in_flight {
            return;
        }
        let refresh_service = state.refresh_service.clone();
        state.refresh_in_flight = true;
        state.status = None;
        unsafe {
            let _ = KillTimer(Some(hwnd), STATUS_TIMER_ID);
        }
        let current_view = build_tray_view(&state.snapshots);
        state.tooltip = if state.snapshots.is_empty() {
            "AI Usage Bar — refreshing…".to_string()
        } else {
            format!("{}\nRefreshing…", current_view.tooltip)
        };
        update_tooltip(hwnd, state);
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }

        let hwnd_raw = hwnd.0 as usize;
        thread::spawn(move || {
            let result = Ok(refresh_service.refresh_all());
            let payload = Box::new(RefreshPayload { result });
            let payload_ptr = Box::into_raw(payload);
            let target = HWND(hwnd_raw as *mut c_void);
            unsafe {
                if PostMessageW(
                    Some(target),
                    WM_APP_REFRESH_DONE,
                    WPARAM(payload_ptr as usize),
                    LPARAM(0),
                )
                .is_err()
                {
                    drop(Box::from_raw(payload_ptr));
                }
            }
        });
    }

    fn apply_refresh(hwnd: HWND, payload: RefreshPayload) {
        let Some(state) = app_state(hwnd) else {
            return;
        };
        state.refresh_in_flight = false;

        match payload.result {
            Ok(snapshots) => {
                let view = build_tray_view(&snapshots);
                state.snapshots = snapshots;
                state.used_percent = view.used_percent;
                state.tooltip = view.tooltip;
            }
            Err(error) => {
                let view = build_tray_view(&state.snapshots);
                state.tooltip = if state.snapshots.is_empty() {
                    format!("AI Usage Bar — refresh failed: {error}")
                } else {
                    format!("{}\nRefresh failed: {error}", view.tooltip)
                };
                eprintln!("refresh error: {error}");
            }
        }

        update_tooltip(hwnd, state);
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }

    fn show_context_menu(hwnd: HWND) {
        unsafe {
            let Ok(menu) = CreatePopupMenu() else {
                return;
            };
            let _ = AppendMenuW(menu, MF_STRING, MENU_REFRESH, w!("Refresh"));
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_COPY_DETAILS,
                w!("Copy details to clipboard"),
            );
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, w!(""));
            let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Quit"));

            let mut point = POINT::default();
            let _ = GetCursorPos(&mut point);
            let command = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                Some(0),
                hwnd,
                None,
            )
            .0 as usize;
            let _ = DestroyMenu(menu);

            match command {
                MENU_REFRESH => begin_refresh(hwnd),
                MENU_COPY_DETAILS => copy_details_to_clipboard(hwnd),
                MENU_QUIT => {
                    let _ = DestroyWindow(hwnd);
                }
                _ => {}
            }
        }
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_PAINT => {
                let (used_percent, refreshing, status) = app_state_ref(hwnd)
                    .map(|state| {
                        (
                            state.used_percent,
                            state.refresh_in_flight,
                            state.status.as_deref(),
                        )
                    })
                    .unwrap_or((None, false, None));
                paint_widget(hwnd, used_percent, refreshing, status);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_NCHITTEST => LRESULT(HTCLIENT as isize),
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_SETCURSOR => {
                if let Ok(cursor) = LoadCursorW(None, IDC_ARROW) {
                    SetCursor(Some(cursor));
                    return LRESULT(1);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_MOUSEMOVE => {
                if let Some(state) = app_state(hwnd) {
                    set_tooltip_visible(hwnd, state, true);
                }
                let mut tracking = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    ..Default::default()
                };
                let _ = TrackMouseEvent(&mut tracking);
                LRESULT(0)
            }
            WM_MOUSELEAVE => LRESULT(0),
            WM_LBUTTONUP => {
                // The whole pill is the manual refresh target. Details remain
                // available explicitly from the context menu, so a left click
                // always has visible, predictable behavior.
                begin_refresh(hwnd);
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                show_context_menu(hwnd);
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == REFRESH_TIMER_ID => {
                begin_refresh(hwnd);
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == POSITION_TIMER_ID => {
                ensure_widget_topmost(hwnd);
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == TOOLTIP_POLL_TIMER_ID => {
                let over_tool = app_state_ref(hwnd)
                    .map(|state| cursor_over_widget_or_tooltip(hwnd, state))
                    .unwrap_or(false);
                if !over_tool {
                    if let Some(state) = app_state(hwnd) {
                        set_tooltip_visible(hwnd, state, false);
                    }
                }
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == STATUS_TIMER_ID => {
                if let Some(state) = app_state(hwnd) {
                    state.status = None;
                    let _ = KillTimer(Some(hwnd), STATUS_TIMER_ID);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_DISPLAYCHANGE | WM_SETTINGCHANGE | WM_DPICHANGED => {
                relocate_widget(hwnd);
                let _ = InvalidateRect(Some(hwnd), None, true);
                LRESULT(0)
            }
            WM_APP_REFRESH_DONE => {
                if wparam.0 == 0 {
                    if let Some(state) = app_state(hwnd) {
                        state.refresh_in_flight = false;
                    }
                } else {
                    let payload = Box::from_raw(wparam.0 as *mut RefreshPayload);
                    apply_refresh(hwnd, *payload);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                let _ = KillTimer(Some(hwnd), REFRESH_TIMER_ID);
                let _ = KillTimer(Some(hwnd), POSITION_TIMER_ID);
                let _ = KillTimer(Some(hwnd), TOOLTIP_POLL_TIMER_ID);
                let _ = KillTimer(Some(hwnd), STATUS_TIMER_ID);
                if let Some(state) = app_state_ref(hwnd) {
                    if let Some(tooltip_hwnd) = state.tooltip_hwnd {
                        let _ = DestroyWindow(tooltip_hwnd);
                    }
                }
                PostQuitMessage(0);
                LRESULT(0)
            }
            WM_NCDESTROY => {
                let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState;
                if !state_ptr.is_null() {
                    drop(Box::from_raw(state_ptr));
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    fn set_dpi_awareness() {
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
    }

    pub fn run() {
        unsafe {
            set_dpi_awareness();

            let hinst = match GetModuleHandleW(None) {
                Ok(module) => HINSTANCE(module.0),
                Err(error) => {
                    eprintln!("failed to get module handle: {error}");
                    return;
                }
            };
            let class_name = w!("AIUsageBarWidget");
            let window_class = WNDCLASSW {
                lpfnWndProc: Some(wnd_proc),
                hInstance: hinst,
                lpszClassName: class_name,
                style: CS_HREDRAW | CS_VREDRAW,
                ..Default::default()
            };
            if RegisterClassW(&window_class) == 0 {
                eprintln!("failed to register AI Usage Bar window class");
                return;
            }

            let (x, y) = compute_widget_pos();
            let hwnd = match CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class_name,
                w!("AI Usage Bar"),
                WS_POPUP,
                x,
                y,
                WIDGET_W,
                WIDGET_H,
                None,
                None,
                Some(hinst),
                None,
            ) {
                Ok(window) => window,
                Err(error) => {
                    eprintln!("failed to create AI Usage Bar window: {error}");
                    return;
                }
            };

            let registry = ProviderRegistry::new();
            if let Err(error) = registry.register(CodexAdapter) {
                eprintln!("failed to register Codex provider: {error}");
                return;
            }
            let refresh_service = Arc::new(RefreshService::new(
                registry,
                RefreshPolicy::default(),
            ));
            let state_ptr = Box::into_raw(Box::new(AppState::loading(refresh_service)));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);
            create_tooltip(hwnd, hinst, &mut *state_ptr);

            if SetTimer(Some(hwnd), REFRESH_TIMER_ID, REFRESH_INTERVAL_MS, None) == 0 {
                eprintln!("failed to start usage refresh timer");
            }
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            // ShowWindow can place the taskbar above a newly created topmost
            // popup, so reassert the z-order after the first show.
            relocate_widget(hwnd);
            let _ = SetTimer(
                Some(hwnd),
                POSITION_TIMER_ID,
                POSITION_TIMER_INTERVAL_MS,
                None,
            );
            let _ = UpdateWindow(hwnd);
            begin_refresh(hwnd);

            let mut message = MSG::default();
            loop {
                let result = GetMessageW(&mut message, None, 0, 0);
                if result.0 == -1 || result.0 == 0 {
                    break;
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn percent_and_color_maps_normalized_usage_to_widget_colors() {
            assert_eq!(percent_and_color(None), (None, COLOR_NEUTRAL));
            assert_eq!(percent_and_color(Some(69.9)), (Some(69.9), COLOR_GREEN));
            assert_eq!(percent_and_color(Some(70.0)), (Some(70.0), COLOR_YELLOW));
            assert_eq!(percent_and_color(Some(90.0)), (Some(90.0), COLOR_RED));
            assert_eq!(percent_and_color(Some(120.0)), (Some(100.0), COLOR_RED));
            assert_eq!(percent_and_color(Some(f64::NAN)), (None, COLOR_NEUTRAL));
        }

        #[test]
        fn wide_strings_are_nul_terminated_for_win32_apis() {
            assert_eq!(to_wide("AI"), vec![b'A' as u16, b'I' as u16, 0]);
        }

        #[test]
        fn clipboard_payload_uses_the_same_detail_renderer_as_the_menu_action() {
            assert_eq!(render_detail_text(&[]), "No provider data");
        }
    }
}

#[cfg(windows)]
fn main() {
    windows_shell::run();
}
