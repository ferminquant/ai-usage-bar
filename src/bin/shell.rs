#[cfg(not(windows))]
fn main() {
    eprintln!("ai-usage-bar-shell is only available on Windows");
}

#[cfg(windows)]
mod windows_shell {
    use ai_usage_bar::{build_tray_view, fetch_codex_snapshots, ProviderCard, UsageSnapshot};
    use std::ffi::c_void;
    use std::ptr::null_mut;
    use std::thread;

    use windows::core::*;
    use windows::Win32::Foundation::*;
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Controls::*;
    use windows::Win32::UI::HiDpi::*;
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    const WIDGET_W: i32 = 210;
    const WIDGET_H: i32 = 44;
    const CARD_W: i32 = 158;
    const CARD_H: i32 = 40;
    const REFRESH_CENTER_X: i32 = 174;
    const PROVIDER_CENTER_X: i32 = 201;
    const SCREEN_MARGIN: i32 = 12;
    const TASKBAR_GAP: i32 = 4;
    const REFRESH_INTERVAL_MS: u32 = 60_000;
    const REFRESH_TIMER_ID: usize = 1;

    const MENU_REFRESH: usize = 1001;
    const MENU_DETAILS: usize = 1002;
    const MENU_QUIT: usize = 1003;

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
    const COLOR_ACCENT: COLORREF = COLORREF(0x00a8e6c0);

    struct AppState {
        snapshots: Vec<UsageSnapshot>,
        used_percent: Option<f64>,
        tooltip: String,
        tooltip_hwnd: Option<HWND>,
        tooltip_text: Vec<u16>,
        refresh_in_flight: bool,
    }

    struct RefreshPayload {
        result: std::result::Result<Vec<UsageSnapshot>, String>,
    }

    impl AppState {
        fn loading() -> Self {
            Self {
                snapshots: Vec::new(),
                used_percent: None,
                tooltip: "AI Usage Bar — loading…".to_string(),
                tooltip_hwnd: None,
                tooltip_text: Vec::new(),
                refresh_in_flight: false,
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
        match used_percent.filter(|value| value.is_finite()) {
            None => (None, COLOR_NEUTRAL),
            Some(value) => {
                let percent = value.clamp(0.0, 100.0);
                let color = if percent >= 90.0 {
                    COLOR_RED
                } else if percent >= 70.0 {
                    COLOR_YELLOW
                } else {
                    COLOR_GREEN
                };
                (Some(percent), color)
            }
        }
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

    fn draw_circle(hdc: HDC, center_x: i32, center_y: i32, radius: i32, color: COLORREF) {
        unsafe {
            let pen = CreatePen(PS_SOLID, 1, color);
            let old_pen = SelectObject(hdc, pen.into());
            let old_brush = SelectObject(hdc, GetStockObject(NULL_BRUSH));
            let _ = Ellipse(
                hdc,
                center_x - radius,
                center_y - radius,
                center_x + radius,
                center_y + radius,
            );
            SelectObject(hdc, old_brush);
            SelectObject(hdc, old_pen);
            let _ = DeleteObject(pen.into());
        }
    }

    fn paint_widget(hwnd: HWND, used_percent: Option<f64>) {
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
                "Codex left",
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

            draw_circle(hdc, REFRESH_CENTER_X, 19, 7, COLOR_BORDER);
            draw_text(
                hdc,
                RECT {
                    left: REFRESH_CENTER_X - 8,
                    top: 10,
                    right: REFRESH_CENTER_X + 8,
                    bottom: 28,
                },
                "↻",
                13,
                FW_NORMAL,
                COLOR_ACCENT,
                DT_SINGLELINE | DT_VCENTER | DT_CENTER,
            );
            draw_circle(hdc, PROVIDER_CENTER_X, 19, 8, COLOR_BORDER);
            draw_text(
                hdc,
                RECT {
                    left: PROVIDER_CENTER_X - 8,
                    top: 10,
                    right: PROVIDER_CENTER_X + 8,
                    bottom: 28,
                },
                "✦",
                12,
                FW_NORMAL,
                COLOR_TEXT,
                DT_SINGLELINE | DT_VCENTER | DT_CENTER,
            );
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

    fn make_tool_info(hwnd: HWND, text: &mut [u16]) -> TTTOOLINFOW {
        TTTOOLINFOW {
            cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
            uFlags: TTF_IDISHWND,
            hwnd,
            uId: hwnd.0 as usize,
            lpszText: PWSTR(text.as_mut_ptr()),
            ..Default::default()
        }
    }

    fn relay_tooltip_event(
        hwnd: HWND,
        state: &AppState,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) {
        let Some(tooltip_hwnd) = state.tooltip_hwnd else {
            return;
        };

        unsafe {
            let mut point = POINT::default();
            let _ = GetCursorPos(&mut point);
            let mut event = MSG {
                hwnd,
                message,
                wParam: wparam,
                lParam: lparam,
                pt: point,
                ..Default::default()
            };
            let _ = SendMessageW(
                tooltip_hwnd,
                TTM_RELAYEVENT,
                None,
                Some(LPARAM(&mut event as *mut MSG as isize)),
            );
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

    fn render_detail_text(snapshots: &[UsageSnapshot]) -> String {
        let cards = ProviderCard::from_snapshots(snapshots);
        if cards.is_empty() {
            return "No provider data".to_string();
        }

        let mut lines = Vec::new();
        for card in cards {
            lines.push(format!("=== {} ({}) ===", card.provider, card.account_id));
            for metric in &card.metrics {
                let unit_display = if metric.unit == "percent" {
                    "%"
                } else {
                    metric.unit.as_str()
                };
                let used = metric.used.as_deref().unwrap_or("?");
                let resets = metric.resets_at.as_deref().unwrap_or("?");
                lines.push(format!(
                    "  [{}] {:?} {} — {}{}, resets {}",
                    metric.label,
                    metric.metric_kind,
                    metric.window_kind,
                    used,
                    unit_display,
                    resets
                ));
                lines.push(format!("    observed: {}", metric.observed_at));
                lines.push(format!(
                    "    source: {}, confidence: {}",
                    metric.source, metric.confidence
                ));
                if metric.unlimited {
                    lines.push("    unlimited: true".to_string());
                }
                if let Some(error) = &metric.error {
                    lines.push(format!("    error: {error}"));
                }
            }
            lines.push(String::new());
        }
        lines.join("\n")
    }

    fn print_details(hwnd: HWND) {
        if let Some(state) = app_state_ref(hwnd) {
            println!("{}", render_detail_text(&state.snapshots));
        }
    }

    fn begin_refresh(hwnd: HWND) {
        let Some(state) = app_state(hwnd) else {
            return;
        };
        if state.refresh_in_flight {
            return;
        }
        state.refresh_in_flight = true;

        let hwnd_raw = hwnd.0 as usize;
        thread::spawn(move || {
            let result = fetch_codex_snapshots().map_err(|error| error.to_string());
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
                MENU_DETAILS,
                w!("Print details to console"),
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
                MENU_DETAILS => print_details(hwnd),
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
                let used_percent = app_state_ref(hwnd).and_then(|state| state.used_percent);
                paint_widget(hwnd, used_percent);
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
                if let Some(state) = app_state_ref(hwnd) {
                    relay_tooltip_event(hwnd, state, msg, wparam, lparam);
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
            WM_MOUSELEAVE => {
                if let Some(state) = app_state_ref(hwnd) {
                    relay_tooltip_event(hwnd, state, msg, wparam, lparam);
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let click_x = (lparam.0 as i16) as i32;
                if (REFRESH_CENTER_X - 10..=REFRESH_CENTER_X + 10).contains(&click_x) {
                    begin_refresh(hwnd);
                } else {
                    print_details(hwnd);
                }
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

            let state_ptr = Box::into_raw(Box::new(AppState::loading()));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);
            create_tooltip(hwnd, hinst, &mut *state_ptr);

            if SetTimer(Some(hwnd), REFRESH_TIMER_ID, REFRESH_INTERVAL_MS, None) == 0 {
                eprintln!("failed to start usage refresh timer");
            }
            relocate_widget(hwnd);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
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
}

#[cfg(windows)]
fn main() {
    windows_shell::run();
}
