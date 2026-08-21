#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

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
        build_registry, build_tray_view_focused_window, default_config_path,
        filter_snapshots_for_view, format_reset_label, is_allowed_browser_url,
        provider_display_name, switchable_providers_for_snapshots, window_display_name, AppConfig,
        Freshness, MetricKind, OpenCodeResetSettings, Provider, RefreshPolicy, RefreshService,
        ResolvedView, UsageSnapshot, KIMI_CONSOLE_URL, OLLAMA_USAGE_URL,
    };
    use chrono::{DateTime, Duration as ChronoDuration, NaiveDateTime, TimeZone, Utc};
    use chrono_tz::America::Toronto;
    use std::ffi::c_void;
    use std::path::PathBuf;
    use std::ptr::null_mut;
    use std::sync::{Arc, Once};
    use std::thread;

    use super::shell_logic::{
        apply_drop, normalize_used_percent, release_route, render_detail_text, swap_drop,
        usage_band, DropCard, DropGrid, ReleaseRoute, SlotRect, UsageBand, DRAG_THRESHOLD_PX,
        GRID_COLUMNS,
    };

    use windows::core::*;
    use windows::Win32::Foundation::*;
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::UI::Controls::*;
    use windows::Win32::UI::HiDpi::*;
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    use windows::Win32::UI::Shell::ShellExecuteW;
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
    const MENU_OPEN_OLLAMA_USAGE: usize = 1004;
    const MENU_OPEN_KIMI_CONSOLE: usize = 1005;
    const MENU_CONFIG_OPENCODE_RESETS: usize = 1006;
    const MENU_TOGGLE_STARTUP: usize = 1007;
    const MENU_TOGGLE_DISABLED: usize = 1008;
    /// Dynamic "Show <provider>" items: MENU_SHOW_PROVIDER_BASE + index.
    const MENU_SHOW_PROVIDER_BASE: usize = 1100;
    const MENU_SHOW_PROVIDER_MAX: usize = 8;
    const MENU_SHOW_WINDOW_BASE: usize = 1200;
    const MENU_SHOW_WINDOW_MAX: usize = 8;
    /// Provider visibility controls also toggle provider refresh enablement;
    /// window and metric checkboxes remain display-only.
    const MENU_VISIBLE_PROVIDER_BASE: usize = 1300;
    const MENU_VISIBLE_PROVIDER_MAX: usize = 8;
    const MENU_VISIBLE_WINDOW_BASE: usize = 1400;
    const MENU_VISIBLE_WINDOW_MAX: usize = 64;
    const MENU_VISIBLE_METRIC_BASE: usize = 1500;
    const MENU_VISIBLE_METRIC_MAX: usize = 64;
    const STATUS_TIMER_ID: usize = 4;
    const STATUS_INTERVAL_MS: u32 = 2_500;
    const CF_UNICODETEXT_FORMAT: u32 = 13;

    const WM_APP_REFRESH_DONE: u32 = 0x8000 + 1;
    const WM_APP_PANEL_REBUILD: u32 = 0x8000 + 2;
    const OPENCODE_RESET_DIALOG_CLASS: PCWSTR = w!("AIUsageBarOpenCodeResetDialog");
    const RESET_DIALOG_SAVE: usize = 1;
    const RESET_DIALOG_CANCEL: usize = 2;
    const RESET_DIALOG_WEEKLY_EDIT: usize = 10;
    const RESET_DIALOG_MONTHLY_EDIT: usize = 11;

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
        config: AppConfig,
        config_path: PathBuf,
        known_providers: Vec<Provider>,
        snapshots: Vec<UsageSnapshot>,
        used_percent: Option<f64>,
        /// Pill subtitle, e.g. "Codex" / "Grok".
        pill_status: String,
        /// Which provider drives the compact pill (None = auto first).
        focus_provider: Option<Provider>,
        /// Optional quota window for the focused provider. Ollama exposes
        /// `session`/`weekly`; Kimi exposes `5-hour`/`weekly` and an optional
        /// `total` when the managed endpoint reports it.
        focus_window: Option<String>,
        switchable_providers: Vec<Provider>,
        tooltip: String,
        tooltip_hwnd: Option<HWND>,
        tooltip_text: Vec<u16>,
        tooltip_visible: bool,
        refresh_in_flight: bool,
        status: Option<String>,
        panel_hwnd: Option<HWND>,
    }

    struct RefreshPayload {
        result: std::result::Result<Vec<UsageSnapshot>, String>,
    }

    impl AppState {
        fn loading(
            refresh_service: Arc<RefreshService>,
            config: AppConfig,
            config_path: PathBuf,
            known_providers: Vec<Provider>,
        ) -> Self {
            Self {
                refresh_service,
                config,
                config_path,
                known_providers,
                snapshots: Vec::new(),
                used_percent: None,
                pill_status: "Loading…".to_string(),
                focus_provider: None,
                focus_window: None,
                switchable_providers: Vec::new(),
                tooltip: "AI Usage Bar — loading…".to_string(),
                tooltip_hwnd: None,
                tooltip_text: Vec::new(),
                tooltip_visible: false,
                refresh_in_flight: false,
                status: None,
                panel_hwnd: None,
            }
        }

        fn apply_view(&mut self, view: ai_usage_bar::TrayViewModel) {
            self.used_percent = view.used_percent;
            self.pill_status = view.status_label;
            self.tooltip = view.tooltip;
            self.switchable_providers = view.switchable_providers;
            if self.focus_provider.is_none() {
                self.focus_provider = view.focus_provider;
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

    /// Ask an already-open provider panel to rebuild itself after the parent
    /// window's view state changes. The panel owns its message loop, so this
    /// keeps refresh results and native-menu changes visible without closing
    /// and reopening the panel.
    fn request_panel_rebuild(hwnd: HWND) {
        let Some(panel) = app_state_ref(hwnd).and_then(|state| state.panel_hwnd) else {
            return;
        };
        unsafe {
            let _ = PostMessageW(Some(panel), WM_APP_PANEL_REBUILD, WPARAM(0), LPARAM(0));
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

    fn paint_widget(hwnd: HWND, used_percent: Option<f64>, refreshing: bool, status: Option<&str>) {
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
            let status_label = status.unwrap_or(if refreshing { "…" } else { "—" });
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
            // Bottom-right style dock: immediately left of the tray icons,
            // vertically centered on the taskbar band (original product placement).
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

    /// Wide enough for multi-provider lines with day-countdown reset strings
    /// without mid-date wrapping (e.g. `3d 5h left · Tue 13:28 EST`).
    const TOOLTIP_MAX_WIDTH_PX: i32 = 640;
    /// Gap between the tooltip bottom edge and the pill top edge.
    const TOOLTIP_PILL_GAP_PX: i32 = 8;
    /// Extra chrome around measured text (border + padding the tip control adds).
    const TOOLTIP_CHROME_PAD_X: i32 = 16;
    const TOOLTIP_CHROME_PAD_Y: i32 = 12;

    fn make_tool_info(hwnd: HWND, text: &mut [u16]) -> TTTOOLINFOW {
        TTTOOLINFOW {
            // comctl32 currently accepts the V2 TOOLINFO layout (through
            // lParam) but rejects the newer lpReserved-inclusive size.
            cbSize: (std::mem::size_of::<TTTOOLINFOW>() - std::mem::size_of::<*mut c_void>())
                as u32,
            // TTF_TRANSPARENT: mouse hits the pill underneath so hover chrome
            // never steals clicks from the bar.
            uFlags: TTF_TRACK | TTF_ABSOLUTE | TTF_TRANSPARENT,
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

    /// Measure tooltip text with the system status/tooltip font via
    /// `DrawTextW(DT_CALCRECT)`. Returns (width, height) including chrome pad.
    fn measure_tooltip_text_size(text: &str) -> (i32, i32) {
        unsafe {
            let hdc = GetDC(None);
            if hdc.is_invalid() {
                // Last-resort fallback only if GDI is unavailable.
                let lines = text.lines().count().max(1) as i32;
                return (240, lines * 20 + TOOLTIP_CHROME_PAD_Y);
            }

            let mut metrics = NONCLIENTMETRICSW {
                cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
                ..Default::default()
            };
            let font = if SystemParametersInfoW(
                SPI_GETNONCLIENTMETRICS,
                metrics.cbSize,
                Some(&mut metrics as *mut _ as *mut _),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
            .is_ok()
            {
                // Status font matches the standard tooltip face.
                CreateFontIndirectW(&metrics.lfStatusFont)
            } else {
                CreateFontW(
                    -12,
                    0,
                    0,
                    0,
                    FW_NORMAL.0 as i32,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET,
                    OUT_DEFAULT_PRECIS,
                    CLIP_DEFAULT_PRECIS,
                    DEFAULT_QUALITY,
                    DEFAULT_PITCH.0 as u32 | FF_DONTCARE.0 as u32,
                    w!("Segoe UI"),
                )
            };

            let old_font = SelectObject(hdc, font.into());
            let mut wide = to_wide(text);
            // DrawText expects a writable buffer; exclude the trailing NUL from
            // the effective length by using the full slice (NUL is fine).
            let mut text_rect = RECT {
                left: 0,
                top: 0,
                right: TOOLTIP_MAX_WIDTH_PX - TOOLTIP_CHROME_PAD_X,
                bottom: 0,
            };
            let _ = DrawTextW(
                hdc,
                &mut wide,
                &mut text_rect,
                DT_CALCRECT | DT_LEFT | DT_TOP | DT_WORDBREAK | DT_NOPREFIX | DT_EXPANDTABS,
            );
            SelectObject(hdc, old_font);
            let _ = DeleteObject(font.into());
            let _ = ReleaseDC(None, hdc);

            let text_w = (text_rect.right - text_rect.left).max(1);
            let text_h = (text_rect.bottom - text_rect.top).max(1);
            (
                (text_w + TOOLTIP_CHROME_PAD_X)
                    .clamp(40, TOOLTIP_MAX_WIDTH_PX + TOOLTIP_CHROME_PAD_X),
                text_h + TOOLTIP_CHROME_PAD_Y,
            )
        }
    }

    /// Read the live tooltip window size after it has been activated/laid out.
    fn tooltip_window_size(tooltip_hwnd: HWND) -> Option<(i32, i32)> {
        unsafe {
            let mut tip_rect = RECT::default();
            if GetWindowRect(tooltip_hwnd, &mut tip_rect).is_err() {
                return None;
            }
            let w = tip_rect.right - tip_rect.left;
            let h = tip_rect.bottom - tip_rect.top;
            if w <= 0 || h <= 0 {
                return None;
            }
            Some((w, h))
        }
    }

    /// Compute top-left track point so the tip box never intersects the pill.
    fn tooltip_origin_clear_of_pill(widget_rect: RECT, tip_w: i32, tip_h: i32) -> (i32, i32) {
        let gap = TOOLTIP_PILL_GAP_PX;
        // Prefer fully above: tip.bottom + gap == widget.top
        let above_y = widget_rect.top - tip_h - gap;
        if above_y >= 0 {
            return (widget_rect.left, above_y);
        }

        // Not enough room above (unusual for taskbar dock): place to the left.
        let left_x = widget_rect.left - tip_w - gap;
        if left_x >= 0 {
            let y = (widget_rect.bottom - tip_h).max(0);
            return (left_x, y);
        }

        // Last resort: pin at screen origin above as much as possible.
        (0, 0)
    }

    fn set_tracking_tooltip_origin(tooltip_hwnd: HWND, x: i32, y: i32) {
        // TTM_TRACKPOSITION packs screen coords as signed 16-bit halves.
        let packed = ((y as u16 as u32) << 16) | (x as u16 as u32);
        unsafe {
            let _ = SendMessageW(
                tooltip_hwnd,
                TTM_TRACKPOSITION,
                None,
                Some(LPARAM(packed as isize)),
            );
        }
    }

    /// Place the tracking tooltip so it does not cover the pill.
    ///
    /// Uses GDI-measured text size first, then corrects with the live
    /// tooltip `GetWindowRect` after activation so layout/DPI are exact.
    fn place_tooltip_clear_of_pill(hwnd: HWND, tooltip_hwnd: HWND, text: &str) {
        unsafe {
            let mut widget_rect = RECT::default();
            if GetWindowRect(hwnd, &mut widget_rect).is_err() {
                return;
            }

            let measured = measure_tooltip_text_size(text);
            let (tip_w, tip_h) = tooltip_window_size(tooltip_hwnd).unwrap_or(measured);
            let (x, y) = tooltip_origin_clear_of_pill(widget_rect, tip_w, tip_h);
            set_tracking_tooltip_origin(tooltip_hwnd, x, y);

            // After moving, re-read the real window box (font/DPI/margins) and
            // correct once more if the tip still intersects the pill.
            if let Some((live_w, live_h)) = tooltip_window_size(tooltip_hwnd) {
                let (cx, cy) = tooltip_origin_clear_of_pill(widget_rect, live_w, live_h);
                if (cx, cy) != (x, y) {
                    set_tracking_tooltip_origin(tooltip_hwnd, cx, cy);
                }

                // Final intersection guard: if still overlapping, force above
                // using the live height only (ignore left placement failures).
                let mut tip_rect = RECT::default();
                if GetWindowRect(tooltip_hwnd, &mut tip_rect).is_ok() {
                    let overlaps = tip_rect.left < widget_rect.right
                        && tip_rect.right > widget_rect.left
                        && tip_rect.top < widget_rect.bottom
                        && tip_rect.bottom > widget_rect.top;
                    if overlaps {
                        let forced_y = widget_rect.top - live_h - TOOLTIP_PILL_GAP_PX;
                        set_tracking_tooltip_origin(
                            tooltip_hwnd,
                            widget_rect.left,
                            forced_y.max(0),
                        );
                    }
                }
            }
        }
    }

    fn set_tooltip_visible(hwnd: HWND, state: &mut AppState, visible: bool) {
        let Some(tooltip_hwnd) = state.tooltip_hwnd else {
            return;
        };

        unsafe {
            let state_changed = state.tooltip_visible != visible;
            if visible && !state_changed {
                // Already showing: keep position honest as text/size changes.
                place_tooltip_clear_of_pill(hwnd, tooltip_hwnd, &state.tooltip);
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
                // Pre-position from measured text so the first paint is clear.
                place_tooltip_clear_of_pill(hwnd, tooltip_hwnd, &state.tooltip);
                let mut tooltip_text = state.tooltip_text.clone();
                let mut tool = make_tool_info(hwnd, &mut tooltip_text);
                let _ = SendMessageW(
                    tooltip_hwnd,
                    TTM_TRACKACTIVATE,
                    Some(WPARAM(1)),
                    Some(LPARAM(&mut tool as *mut TTTOOLINFOW as isize)),
                );
                // Activate can reflow the tip; re-measure live window size.
                let _ = SendMessageW(tooltip_hwnd, TTM_UPDATE, None, None);
                place_tooltip_clear_of_pill(hwnd, tooltip_hwnd, &state.tooltip);
            } else {
                let _ = KillTimer(Some(hwnd), TOOLTIP_POLL_TIMER_ID);
                let mut tooltip_text = state.tooltip_text.clone();
                let mut tool = make_tool_info(hwnd, &mut tooltip_text);
                let _ = SendMessageW(
                    tooltip_hwnd,
                    TTM_TRACKACTIVATE,
                    Some(WPARAM(0)),
                    Some(LPARAM(&mut tool as *mut TTTOOLINFOW as isize)),
                );
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
        if state.tooltip_visible {
            place_tooltip_clear_of_pill(hwnd, tooltip_hwnd, &state.tooltip);
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
            let _ = SendMessageW(
                tooltip_hwnd,
                TTM_SETMAXTIPWIDTH,
                None,
                Some(LPARAM(TOOLTIP_MAX_WIDTH_PX as isize)),
            );
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

    fn toggle_startup_registration(hwnd: HWND) {
        let enabled = match ai_usage_bar::startup::auto_start_enabled() {
            Ok(enabled) => enabled,
            Err(error) => {
                eprintln!("could not read startup registration: {error}");
                if let Some(state) = app_state(hwnd) {
                    set_status(hwnd, state, "Could not read startup setting");
                }
                return;
            }
        };
        let next_enabled = !enabled;
        match ai_usage_bar::startup::set_auto_start_enabled(next_enabled) {
            Ok(()) => {
                if let Some(state) = app_state(hwnd) {
                    set_status(
                        hwnd,
                        state,
                        if next_enabled {
                            "Run on Windows startup enabled"
                        } else {
                            "Run on Windows startup disabled"
                        },
                    );
                }
                request_panel_rebuild(hwnd);
            }
            Err(error) => {
                eprintln!("could not update startup registration: {error}");
                if let Some(state) = app_state(hwnd) {
                    set_status(hwnd, state, "Could not update startup setting");
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
        recompute_view(state);
        state.tooltip = if state.snapshots.is_empty() {
            "AI Usage Bar — refreshing…".to_string()
        } else {
            format!("{}\nRefreshing…", state.tooltip)
        };
        update_tooltip(hwnd, state);
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        request_panel_rebuild(hwnd);

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
                let resolved = state.config.resolved_view(&state.known_providers);
                let active_providers: Vec<Provider> = state
                    .known_providers
                    .iter()
                    .filter(|provider| provider_is_active(state, provider, &resolved))
                    .cloned()
                    .collect();
                state.snapshots = snapshots
                    .into_iter()
                    .filter(|snapshot| active_providers.contains(&snapshot.provider))
                    .collect();
                recompute_view(state);
            }
            Err(error) => {
                recompute_view(state);
                let safe_error = ai_usage_bar::redact_sensitive_text(&error.to_string());
                state.tooltip = if state.snapshots.is_empty() {
                    format!("AI Usage Bar — refresh failed: {safe_error}")
                } else {
                    format!("{}\nRefresh failed: {safe_error}", state.tooltip)
                };
                eprintln!("refresh error: {safe_error}");
            }
        }

        update_tooltip(hwnd, state);
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        request_panel_rebuild(hwnd);
    }

    fn set_focus_provider(hwnd: HWND, provider: Provider) {
        let Some(state) = app_state(hwnd) else {
            return;
        };
        // Clear any transient status so the pill only shows the provider name.
        clear_status_for_window(hwnd, state);
        let same_provider = state.focus_provider.as_ref() == Some(&provider);
        state.focus_provider = Some(provider);
        if !same_provider {
            // A provider switch starts at the provider's default window.
            // Reselecting the same provider keeps its focused window (Kimi
            // 5-hour/weekly, Ollama session/weekly, OpenCode 5-hour/weekly/
            // monthly) instead of snapping back to the default.
            state.focus_window = None;
        }
        let previous_config = state.config.clone();
        let focused = state.focus_provider.clone();
        let window = state.focus_window.clone();
        state
            .config
            .set_view_defaults(focused.as_ref(), window.as_deref());
        if let Err(error) = state.config.save(&state.config_path) {
            eprintln!("could not persist display default: {error}");
            state.config = previous_config;
            set_status(hwnd, state, "Could not save display preference");
        }
        recompute_view(state);
        update_tooltip(hwnd, state);
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        request_panel_rebuild(hwnd);
    }

    fn set_focus_window(hwnd: HWND, window: &str) {
        let Some(state) = app_state(hwnd) else {
            return;
        };
        clear_status_for_window(hwnd, state);
        state.focus_window = Some(window.to_string());
        let previous_config = state.config.clone();
        let focused = state.focus_provider.clone();
        state
            .config
            .set_view_defaults(focused.as_ref(), Some(window));
        if let Err(error) = state.config.save(&state.config_path) {
            eprintln!("could not persist display default: {error}");
            state.config = previous_config;
            set_status(hwnd, state, "Could not save display preference");
        }
        recompute_view(state);
        update_tooltip(hwnd, state);
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        request_panel_rebuild(hwnd);
    }

    fn cycle_focus_provider(hwnd: HWND) {
        let Some(state) = app_state(hwnd) else {
            return;
        };
        if state.switchable_providers.len() < 2 {
            begin_refresh(hwnd);
            return;
        }
        let current = state.focus_provider.clone();
        let next = match current
            .as_ref()
            .and_then(|c| state.switchable_providers.iter().position(|p| p == c))
        {
            Some(idx) => {
                state.switchable_providers[(idx + 1) % state.switchable_providers.len()].clone()
            }
            None => state.switchable_providers[0].clone(),
        };
        set_focus_provider(hwnd, next);
    }

    /// A provider is active in the shell only when it is enabled and not
    /// hidden. The two settings are kept in sync by the provider checkbox, but
    /// treating legacy hidden entries as inactive keeps older configs safe.
    fn provider_is_active(state: &AppState, provider: &Provider, view: &ResolvedView) -> bool {
        state.config.provider_enabled(provider) && !view.is_provider_hidden(provider)
    }

    /// Apply the persisted view settings to the raw refresh report and
    /// normalize focus after a provider/window was hidden. Disabled providers
    /// are removed by the refresh service; raw snapshots for active providers
    /// remain available for display filters.
    fn recompute_view(state: &mut AppState) {
        // Resolve against every compiled provider, not only providers with a
        // current snapshot. This keeps disabled providers addressable through
        // the panel's restore toggle; `available` remains the narrower set
        // used for focus/cycling.
        let all_providers = state.known_providers.clone();
        let available = switchable_providers_for_snapshots(&state.snapshots);
        let resolved = state.config.resolved_view(&all_providers);
        let filtered = filter_snapshots_for_view(&state.snapshots, &resolved);
        // Keep every active provider in the selector even when a row filter
        // removes some (or all) of its snapshots. Disabled providers are
        // available through the panel's `Disabled` toggle instead.
        let visible_candidates: Vec<Provider> = available
            .iter()
            .filter(|provider| provider_is_active(state, provider, &resolved))
            .cloned()
            .collect();
        let ordered_visible = ordered_providers(&visible_candidates, &resolved);

        state.focus_provider = state
            .focus_provider
            .clone()
            .filter(|provider| visible_candidates.contains(provider))
            .or_else(|| {
                resolved
                    .default_provider
                    .clone()
                    .filter(|provider| visible_candidates.contains(provider))
            })
            .or_else(|| ordered_visible.first().cloned());

        let default_window = state
            .focus_provider
            .as_ref()
            .filter(|provider| resolved.default_provider.as_ref() == Some(provider))
            .and(resolved.default_window.as_deref());
        let focus_windows = state
            .focus_provider
            .as_ref()
            .map(|provider| available_windows(&filtered, provider))
            .unwrap_or_default();
        state.focus_window = state
            .focus_window
            .as_deref()
            .filter(|window| focus_windows.iter().any(|candidate| candidate == window))
            .map(str::to_string)
            .or_else(|| {
                default_window
                    .filter(|window| focus_windows.iter().any(|candidate| candidate == window))
                    .map(str::to_string)
            })
            .or_else(|| focus_windows.first().cloned());

        let mut view = build_tray_view_focused_window(
            &filtered,
            state.focus_provider.as_ref(),
            state.focus_window.as_deref(),
            Utc::now(),
        );
        view.switchable_providers = ordered_visible;
        state.apply_view(view);
    }

    fn ordered_providers(available: &[Provider], view: &ResolvedView) -> Vec<Provider> {
        let mut ordered = Vec::with_capacity(available.len());
        if let Some(configured) = &view.provider_order {
            for provider in configured {
                if available.contains(provider) && !ordered.contains(provider) {
                    ordered.push(provider.clone());
                }
            }
        }
        for provider in available {
            if !ordered.contains(provider) {
                ordered.push(provider.clone());
            }
        }
        ordered
    }

    fn available_windows(snapshots: &[UsageSnapshot], provider: &Provider) -> Vec<String> {
        let mut windows = Vec::new();
        for snapshot in snapshots {
            if &snapshot.provider != provider
                || snapshot.unit != "percent"
                || snapshot.used.is_none()
            {
                continue;
            }
            let Some(label) = snapshot.window_label.as_deref() else {
                continue;
            };
            let Some(canonical) = canonical_window_key(provider, label) else {
                continue;
            };
            if !windows.iter().any(|existing| existing == canonical) {
                windows.push(canonical.to_string());
            }
        }
        windows.sort_by_key(|window| window_sort_key(window));
        windows
    }

    fn available_metrics(snapshots: &[UsageSnapshot], provider: &Provider) -> Vec<MetricKind> {
        let mut metrics = Vec::new();
        for snapshot in snapshots
            .iter()
            .filter(|snapshot| &snapshot.provider == provider)
        {
            // Quota is the product's primary signal and health is diagnostic;
            // neither is an optional display toggle. Optional balances and
            // counters (for example Credits) can be hidden per provider.
            if matches!(snapshot.metric_kind, MetricKind::Quota | MetricKind::Health) {
                continue;
            }
            if !metrics.contains(&snapshot.metric_kind) {
                metrics.push(snapshot.metric_kind);
            }
        }
        metrics.sort_by_key(|metric| metric.as_str());
        metrics
    }

    fn metric_display_name(metric: MetricKind) -> &'static str {
        match metric {
            MetricKind::Quota => "Quota",
            MetricKind::Credits => "Credits",
            MetricKind::Spend => "Spend",
            MetricKind::Tokens => "Tokens",
            MetricKind::Requests => "Requests",
            MetricKind::Health => "Health",
        }
    }

    fn clear_status_for_window(hwnd: HWND, state: &mut AppState) {
        state.status = None;
        unsafe {
            let _ = KillTimer(Some(hwnd), STATUS_TIMER_ID);
        }
    }

    fn finish_view_change(hwnd: HWND, state: &mut AppState, previous_config: AppConfig) {
        if let Err(error) = state.config.save(&state.config_path) {
            eprintln!("could not persist display preference: {error}");
            state.config = previous_config;
            set_status(hwnd, state, "Could not save display preference");
            return;
        }
        clear_status_for_window(hwnd, state);
        recompute_view(state);
        update_tooltip(hwnd, state);
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        request_panel_rebuild(hwnd);
    }

    /// Toggle a provider as one user-facing control: checking it makes the
    /// provider visible and enabled; unchecking it hides the provider and
    /// disables refresh scheduling. Disabled providers can be revealed with
    /// the panel's compact `Disabled` toggle and restored from their card.
    fn toggle_provider_control(hwnd: HWND, provider: Provider) {
        let Some(state) = app_state(hwnd) else {
            return;
        };
        if !state.known_providers.contains(&provider) {
            return;
        }
        let resolved = state.config.resolved_view(&state.known_providers);
        let next_enabled =
            !state.config.provider_enabled(&provider) || resolved.is_provider_hidden(&provider);
        let mut hidden: Vec<Provider> = state
            .known_providers
            .iter()
            .filter(|candidate| resolved.is_provider_hidden(candidate))
            .cloned()
            .collect();
        if next_enabled {
            hidden.retain(|candidate| candidate != &provider);
        } else {
            if !hidden.contains(&provider) {
                hidden.push(provider.clone());
            }
        }

        let previous_config = state.config.clone();
        let previous_runtime_enabled = state
            .refresh_service
            .provider_enabled(&provider)
            .unwrap_or_else(|_| previous_config.provider_enabled(&provider));
        state.config.set_provider_enabled(&provider, next_enabled);
        state.config.set_view_hidden_providers(&hidden);
        if let Err(error) = state
            .refresh_service
            .set_provider_enabled(&provider, next_enabled)
        {
            eprintln!("could not update provider state: {error}");
            state.config = previous_config;
            set_status(hwnd, state, "Could not update provider");
            return;
        }
        if let Err(error) = state.config.save(&state.config_path) {
            eprintln!("could not persist provider state: {error}");
            let _ = state
                .refresh_service
                .set_provider_enabled(&provider, previous_runtime_enabled);
            state.config = previous_config;
            set_status(hwnd, state, "Could not save provider setting");
            return;
        }

        if !next_enabled {
            // Drop any live or in-flight result already held by the shell so
            // disabling a provider takes effect immediately in the UI.
            state
                .snapshots
                .retain(|snapshot| snapshot.provider != provider);
        }
        clear_status_for_window(hwnd, state);
        recompute_view(state);
        update_tooltip(hwnd, state);
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        request_panel_rebuild(hwnd);
        set_status(
            hwnd,
            state,
            if next_enabled {
                "Provider enabled"
            } else {
                "Provider disabled"
            },
        );
        if next_enabled {
            begin_refresh(hwnd);
        }
    }

    fn toggle_disabled_providers_visibility(hwnd: HWND) {
        let Some(state) = app_state(hwnd) else {
            return;
        };
        let resolved = state.config.resolved_view(&state.known_providers);
        let previous_config = state.config.clone();
        state
            .config
            .set_view_show_disabled_providers(!resolved.show_disabled_providers);
        finish_view_change(hwnd, state, previous_config);
    }

    fn toggle_window_visibility(hwnd: HWND, provider: Provider, window: String) {
        let Some(state) = app_state(hwnd) else {
            return;
        };
        let all_windows = available_windows(&state.snapshots, &provider);
        if !all_windows.contains(&window) {
            return;
        }
        let resolved = state
            .config
            .resolved_view(&switchable_providers_for_snapshots(&state.snapshots));
        let mut visible: Vec<String> = resolved
            .windows_for(&provider)
            .map(|windows| {
                windows
                    .iter()
                    .filter(|candidate| all_windows.contains(candidate))
                    .cloned()
                    .collect()
            })
            .unwrap_or_else(|| all_windows.clone());

        if let Some(index) = visible.iter().position(|candidate| candidate == &window) {
            if visible.len() <= 1 {
                set_status(hwnd, state, "Keep at least one usage window visible");
                return;
            }
            visible.remove(index);
        } else {
            visible.push(window);
            visible.sort_by_key(|candidate| window_sort_key(candidate));
        }

        let all_visible = visible.len() == all_windows.len()
            && all_windows
                .iter()
                .all(|candidate| visible.contains(candidate));
        let visible_refs: Vec<&str> = visible.iter().map(String::as_str).collect();
        let previous_config = state.config.clone();
        state.config.set_view_visible_windows(
            &provider,
            if all_visible {
                None
            } else {
                Some(&visible_refs)
            },
        );
        finish_view_change(hwnd, state, previous_config);
    }

    fn toggle_metric_visibility(hwnd: HWND, provider: Provider, metric: MetricKind) {
        let Some(state) = app_state(hwnd) else {
            return;
        };
        let all_metrics = available_metrics(&state.snapshots, &provider);
        if !all_metrics.contains(&metric) {
            return;
        }
        let resolved = state
            .config
            .resolved_view(&switchable_providers_for_snapshots(&state.snapshots));
        let mut visible: Vec<MetricKind> = all_metrics
            .iter()
            .copied()
            .filter(|candidate| resolved.is_metric_visible(&provider, *candidate))
            .collect();

        if let Some(index) = visible.iter().position(|candidate| candidate == &metric) {
            visible.remove(index);
        } else {
            visible.push(metric);
            visible.sort_by_key(|candidate| candidate.as_str());
        }

        let all_visible = visible.len() == all_metrics.len()
            && all_metrics
                .iter()
                .all(|candidate| visible.contains(candidate));
        let default_all_visible = all_metrics
            .iter()
            .all(|candidate| ResolvedView::default_metric_visible(&provider, *candidate));
        let previous_config = state.config.clone();
        state.config.set_view_visible_metrics(
            &provider,
            if all_visible && default_all_visible {
                None
            } else {
                Some(&visible)
            },
        );
        finish_view_change(hwnd, state, previous_config);
    }

    /// True when `order` is a permutation of exactly `available` (same length,
    /// no duplicates, same members). The panel only produces permutations, but
    /// the guard keeps a stale/foreign order from being persisted.
    fn is_full_permutation(order: &[Provider], available: &[Provider]) -> bool {
        order.len() == available.len()
            && order.iter().all(|provider| available.contains(provider))
            && available.iter().all(|provider| order.contains(provider))
    }

    fn reorder_provider(hwnd: HWND, order: Vec<Provider>) {
        let Some(state) = app_state(hwnd) else {
            return;
        };
        let available = state.known_providers.clone();
        let resolved = state.config.resolved_view(&available);
        let visible: Vec<Provider> = available
            .iter()
            .filter(|provider| provider_is_active(state, provider, &resolved))
            .cloned()
            .collect();
        if !is_full_permutation(&order, &visible) {
            return;
        }
        // Disabled providers are not active card slots, so drag-and-drop only
        // sends the active permutation. Keep disabled providers in the
        // persisted order as a stable tail so re-enabling one later does not
        // lose it.
        let mut full_order = order;
        for provider in available {
            if !full_order.contains(&provider) {
                full_order.push(provider);
            }
        }
        let previous_config = state.config.clone();
        state.config.set_view_provider_order(&full_order);
        finish_view_change(hwnd, state, previous_config);
    }

    struct ResetDialogState {
        parent: HWND,
        weekly_edit: HWND,
        monthly_edit: HWND,
        current: OpenCodeResetSettings,
        result: Option<OpenCodeResetSettings>,
    }

    fn reset_dialog_state(hwnd: HWND) -> Option<&'static mut ResetDialogState> {
        let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut ResetDialogState;
        if ptr.is_null() {
            None
        } else {
            Some(unsafe { &mut *ptr })
        }
    }

    fn format_reset_countdown(value: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
        let Some(value) = value else {
            return String::new();
        };
        let total_minutes = value.signed_duration_since(now).num_minutes().max(0);
        let days = total_minutes / (24 * 60);
        let hours = (total_minutes / 60) % 24;
        let minutes = total_minutes % 60;
        if days > 0 {
            format!("{days} days {hours} hours")
        } else {
            format!("{hours} hours {minutes} minutes")
        }
    }

    fn parse_reset_countdown(
        value: &str,
        now: DateTime<Utc>,
    ) -> std::result::Result<Option<DateTime<Utc>>, String> {
        let lower = value.trim().to_ascii_lowercase();
        let countdown = lower
            .strip_prefix("resets in ")
            .unwrap_or(lower.as_str())
            .trim();
        let tokens: Vec<&str> = countdown.split_whitespace().collect();
        if tokens.len() < 2 || !tokens.len().is_multiple_of(2) {
            return Err(
                "Paste a countdown such as \"2 days 10 hours\" or \"5 hours 0 minutes\""
                    .to_string(),
            );
        }

        let mut total_seconds = 0_i64;
        for pair in tokens.chunks_exact(2) {
            let amount = pair[0].parse::<i64>().map_err(|_| {
                "Paste a countdown such as \"2 days 10 hours\" or \"5 hours 0 minutes\"".to_string()
            })?;
            if amount < 0 {
                return Err("Countdown values cannot be negative".to_string());
            }
            let seconds_per_unit = match pair[1] {
                "day" | "days" => 24 * 60 * 60,
                "hour" | "hours" => 60 * 60,
                "minute" | "minutes" => 60,
                _ => {
                    return Err(
                        "Use days, hours, and minutes, for example \"2 days 10 hours\"".to_string(),
                    )
                }
            };
            let seconds = amount
                .checked_mul(seconds_per_unit)
                .and_then(|seconds| total_seconds.checked_add(seconds))
                .ok_or_else(|| "That countdown is too large".to_string())?;
            total_seconds = seconds;
        }

        Ok(Some(now + ChronoDuration::seconds(total_seconds)))
    }

    fn parse_reset_anchor(
        raw: &str,
        now: DateTime<Utc>,
    ) -> std::result::Result<Option<DateTime<Utc>>, String> {
        let value = raw.trim();
        if value.is_empty() {
            return Ok(None);
        }
        // OpenCode displays reset values as a relative countdown. Accept the
        // whole copied line as well as just its value.
        let lower = value.to_ascii_lowercase();
        let countdown = lower
            .strip_prefix("resets in ")
            .unwrap_or(lower.as_str())
            .trim();
        if countdown
            .split_whitespace()
            .next()
            .and_then(|token| token.parse::<i64>().ok())
            .is_some()
        {
            return parse_reset_countdown(value, now);
        }
        if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
            return Ok(Some(parsed.with_timezone(&Utc)));
        }
        let local = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M")
            .map_err(|_| "Paste a countdown such as \"2 days 10 hours\" or \"5 hours 0 minutes\"; RFC3339 UTC is also accepted".to_string())?;
        Toronto
            .from_local_datetime(&local)
            .single()
            .map(|instant| Some(instant.with_timezone(&Utc)))
            .ok_or_else(|| {
                "That local time is ambiguous or invalid because of a daylight-saving transition"
                    .to_string()
            })
    }

    fn window_text(hwnd: HWND) -> String {
        unsafe {
            let length = GetWindowTextLengthW(hwnd).max(0) as usize;
            let mut buffer = vec![0u16; length + 1];
            let written = GetWindowTextW(hwnd, &mut buffer).max(0) as usize;
            String::from_utf16_lossy(&buffer[..written.min(buffer.len())])
        }
    }

    fn reset_dialog_close(hwnd: HWND, save: bool) {
        let Some(state) = reset_dialog_state(hwnd) else {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return;
        };
        if save {
            state.result = Some(state.current);
        }
        unsafe {
            let _ = EnableWindow(state.parent, true);
            let _ = SetForegroundWindow(state.parent);
            let _ = DestroyWindow(hwnd);
        }
    }

    unsafe extern "system" fn reset_dialog_wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_COMMAND => {
                let command = wparam.0 & 0xffff;
                if command == RESET_DIALOG_CANCEL {
                    reset_dialog_close(hwnd, false);
                    return LRESULT(0);
                }
                if command == RESET_DIALOG_SAVE {
                    let Some(state) = reset_dialog_state(hwnd) else {
                        return LRESULT(0);
                    };
                    let now = Utc::now();
                    let weekly = match parse_reset_anchor(&window_text(state.weekly_edit), now) {
                        Ok(value) => value,
                        Err(error) => {
                            let text = to_wide(&error);
                            let _ = MessageBoxW(
                                Some(hwnd),
                                PCWSTR(text.as_ptr()),
                                w!("Invalid weekly reset"),
                                MB_OK | MB_ICONERROR,
                            );
                            return LRESULT(0);
                        }
                    };
                    let monthly = match parse_reset_anchor(&window_text(state.monthly_edit), now) {
                        Ok(value) => value,
                        Err(error) => {
                            let text = to_wide(&error);
                            let _ = MessageBoxW(
                                Some(hwnd),
                                PCWSTR(text.as_ptr()),
                                w!("Invalid monthly reset"),
                                MB_OK | MB_ICONERROR,
                            );
                            return LRESULT(0);
                        }
                    };
                    state.current = OpenCodeResetSettings {
                        weekly_reset_at: weekly,
                        monthly_reset_at: monthly,
                    };
                    reset_dialog_close(hwnd, true);
                    return LRESULT(0);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_CLOSE => {
                reset_dialog_close(hwnd, false);
                LRESULT(0)
            }
            WM_NCDESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn create_reset_dialog_control(
        parent: HWND,
        class: PCWSTR,
        text: &str,
        style: WINDOW_STYLE,
        ex_style: WINDOW_EX_STYLE,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        id: usize,
        hinst: HINSTANCE,
    ) -> Option<HWND> {
        let text = to_wide(text);
        unsafe {
            CreateWindowExW(
                ex_style,
                class,
                PCWSTR(text.as_ptr()),
                style,
                x,
                y,
                width,
                height,
                Some(parent),
                Some(HMENU(id as *mut c_void)),
                Some(hinst),
                None,
            )
            .ok()
        }
    }

    fn run_opencode_reset_dialog(
        parent: HWND,
        settings: OpenCodeResetSettings,
    ) -> Option<OpenCodeResetSettings> {
        static REGISTER_CLASS: Once = Once::new();
        unsafe {
            let hinst = GetModuleHandleW(None)
                .ok()
                .map(|module| HINSTANCE(module.0))?;
            REGISTER_CLASS.call_once(|| {
                let class = WNDCLASSW {
                    lpfnWndProc: Some(reset_dialog_wnd_proc),
                    hInstance: hinst,
                    lpszClassName: OPENCODE_RESET_DIALOG_CLASS,
                    style: CS_HREDRAW | CS_VREDRAW,
                    hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                    ..Default::default()
                };
                let _ = RegisterClassW(&class);
            });

            let mut parent_rect = RECT::default();
            let _ = GetWindowRect(parent, &mut parent_rect);
            let width = 500;
            let height = 290;
            let x = parent_rect.left + ((parent_rect.right - parent_rect.left - width) / 2);
            let y = (parent_rect.top - height - 8).max(0);
            let dialog = CreateWindowExW(
                WS_EX_DLGMODALFRAME | WS_EX_TOOLWINDOW,
                OPENCODE_RESET_DIALOG_CLASS,
                w!("OpenCode reset anchors"),
                WS_CAPTION | WS_SYSMENU | WS_POPUP | WS_VISIBLE,
                x,
                y,
                width,
                height,
                Some(parent),
                None,
                Some(hinst),
                None,
            )
            .ok()?;

            let mut state = Box::new(ResetDialogState {
                parent,
                weekly_edit: HWND(null_mut()),
                monthly_edit: HWND(null_mut()),
                current: settings,
                result: None,
            });
            SetWindowLongPtrW(dialog, GWLP_USERDATA, (&mut *state) as *mut _ as isize);

            let label_style = WS_CHILD | WS_VISIBLE;
            let edit_style =
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32);
            let button_style = WS_CHILD | WS_VISIBLE | WS_TABSTOP;
            let now = Utc::now();
            let _ = create_reset_dialog_control(
                dialog,
                w!("STATIC"),
                "Weekly reset countdown (paste from OpenCode):",
                label_style,
                WINDOW_EX_STYLE(0),
                18,
                20,
                460,
                22,
                0,
                hinst,
            );
            let Some(weekly_edit) = create_reset_dialog_control(
                dialog,
                w!("EDIT"),
                &format_reset_countdown(settings.weekly_reset_at, now),
                edit_style,
                WS_EX_CLIENTEDGE,
                18,
                45,
                460,
                24,
                RESET_DIALOG_WEEKLY_EDIT,
                hinst,
            ) else {
                let _ = DestroyWindow(dialog);
                return None;
            };
            state.weekly_edit = weekly_edit;
            let _ = create_reset_dialog_control(
                dialog,
                w!("STATIC"),
                "Monthly reset countdown (paste from OpenCode):",
                label_style,
                WINDOW_EX_STYLE(0),
                18,
                82,
                460,
                22,
                0,
                hinst,
            );
            let Some(monthly_edit) = create_reset_dialog_control(
                dialog,
                w!("EDIT"),
                &format_reset_countdown(settings.monthly_reset_at, now),
                edit_style,
                WS_EX_CLIENTEDGE,
                18,
                107,
                460,
                24,
                RESET_DIALOG_MONTHLY_EDIT,
                hinst,
            ) else {
                let _ = DestroyWindow(dialog);
                return None;
            };
            state.monthly_edit = monthly_edit;
            let _ = create_reset_dialog_control(
                dialog,
                w!("STATIC"),
                "Examples: \"2 days 10 hours\" or \"5 hours 0 minutes\".",
                label_style,
                WINDOW_EX_STYLE(0),
                18,
                140,
                460,
                22,
                0,
                hinst,
            );
            let _ = create_reset_dialog_control(
                dialog,
                w!("STATIC"),
                "Blank restores the built-in weekly/monthly defaults.",
                label_style,
                WINDOW_EX_STYLE(0),
                18,
                163,
                460,
                22,
                0,
                hinst,
            );
            let _ = create_reset_dialog_control(
                dialog,
                w!("STATIC"),
                "The rolling 5-hour reset is derived from local activity.",
                label_style,
                WINDOW_EX_STYLE(0),
                18,
                186,
                460,
                22,
                0,
                hinst,
            );
            let _ = create_reset_dialog_control(
                dialog,
                w!("BUTTON"),
                "Save",
                button_style | WINDOW_STYLE(BS_DEFPUSHBUTTON as u32),
                WINDOW_EX_STYLE(0),
                270,
                220,
                85,
                28,
                RESET_DIALOG_SAVE,
                hinst,
            );
            let _ = create_reset_dialog_control(
                dialog,
                w!("BUTTON"),
                "Cancel",
                button_style,
                WINDOW_EX_STYLE(0),
                363,
                220,
                85,
                28,
                RESET_DIALOG_CANCEL,
                hinst,
            );

            let _ = EnableWindow(parent, false);
            let _ = SetForegroundWindow(dialog);
            let _ = SetFocus(Some(state.weekly_edit));
            let mut message = MSG::default();
            while IsWindow(Some(dialog)).as_bool() {
                let result = GetMessageW(&mut message, None, 0, 0);
                if result.0 <= 0 {
                    break;
                }
                if !IsDialogMessageW(dialog, &message).as_bool() {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            if IsWindow(Some(dialog)).as_bool() {
                let _ = DestroyWindow(dialog);
            }
            let _ = EnableWindow(parent, true);
            let _ = SetForegroundWindow(parent);
            state.result.take()
        }
    }

    fn save_opencode_reset_settings(hwnd: HWND, settings: OpenCodeResetSettings) {
        let result = (|| {
            let state = app_state(hwnd).ok_or_else(|| "widget state is unavailable".to_string())?;
            let mut config = state.config.clone();
            config.set_opencode_reset_settings(settings);
            config
                .save(&state.config_path)
                .map_err(|error| format!("could not save settings: {error}"))?;
            let registry = build_registry(&config)
                .map_err(|error| format!("could not reload providers: {error}"))?;
            state.config = config;
            state.refresh_service =
                Arc::new(RefreshService::new(registry, RefreshPolicy::default()));
            Ok::<(), String>(())
        })();
        match result {
            Ok(()) => {
                begin_refresh(hwnd);
                if let Some(state) = app_state(hwnd) {
                    set_status(hwnd, state, "OpenCode reset settings saved");
                }
            }
            Err(error) => {
                eprintln!(
                    "OpenCode reset settings failed: {}",
                    ai_usage_bar::redact_sensitive_text(&error)
                );
                if let Some(state) = app_state(hwnd) {
                    set_status(hwnd, state, "Could not save OpenCode reset settings");
                }
            }
        }
    }

    fn open_opencode_reset_dialog(hwnd: HWND) {
        let Some(settings) =
            app_state_ref(hwnd).map(|state| state.config.opencode_reset_settings())
        else {
            return;
        };
        if let Some(updated) = run_opencode_reset_dialog(hwnd, settings) {
            save_opencode_reset_settings(hwnd, updated);
        }
    }

    fn open_allowed_browser_url(hwnd: HWND, url: &str, failure_status: &str) {
        if !is_allowed_browser_url(url) {
            if let Some(state) = app_state(hwnd) {
                set_status(hwnd, state, "Blocked unallowlisted browser destination");
            }
            return;
        }
        let url = to_wide(url);
        let launched = unsafe {
            ShellExecuteW(
                Some(hwnd),
                w!("open"),
                PCWSTR(url.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        if launched.0 as usize <= 32 {
            if let Some(state) = app_state(hwnd) {
                set_status(hwnd, state, failure_status);
            }
        }
    }

    fn open_ollama_usage_page(hwnd: HWND) {
        open_allowed_browser_url(hwnd, OLLAMA_USAGE_URL, "Could not open Ollama usage page");
    }

    fn open_kimi_console(hwnd: HWND) {
        open_allowed_browser_url(hwnd, KIMI_CONSOLE_URL, "Could not open Kimi Console");
    }

    fn canonical_window_key(provider: &Provider, label: &str) -> Option<&'static str> {
        match provider {
            Provider::OllamaCloud => match label {
                "session" => Some("session"),
                "weekly" => Some("weekly"),
                _ => None,
            },
            Provider::Kimi => match label {
                "5-hour" => Some("5-hour"),
                "weekly" | "primary" => Some("weekly"),
                "total" | "monthly" => Some("total"),
                _ => None,
            },
            Provider::OpenCodeGo => match label {
                "5-hour" => Some("5-hour"),
                "weekly" => Some("weekly"),
                "monthly" | "total" => Some("monthly"),
                _ => None,
            },
            Provider::Codex | Provider::GrokConsumer | Provider::GrokApi => match label {
                "primary" | "weekly" => Some("primary"),
                _ => None,
            },
        }
    }

    fn window_sort_key(window: &str) -> u8 {
        match window {
            "session" | "5-hour" => 0,
            "weekly" => 1,
            "total" | "monthly" => 2,
            _ => 3,
        }
    }

    const CONTROL_PANEL_CLASS: PCWSTR = w!("AIUsageBarControlPanel");
    const PANEL_CARD_WIDTH: i32 = 400;
    const PANEL_MARGIN: i32 = 18;
    const PANEL_GAP: i32 = 14;
    const PANEL_ROW_HEIGHT: i32 = 34;
    const PANEL_HEADER_HEIGHT: i32 = 64;
    const PANEL_CARD_PADDING: i32 = 14;

    #[derive(Clone, Debug, PartialEq)]
    enum PanelAction {
        None,
        FocusProvider(Provider),
        FocusWindow(Provider, String),
        ToggleProvider(Provider),
        ToggleWindow(Provider, String),
        ToggleMetric(Provider, MetricKind),
        ToggleDisabledProviders,
        Refresh,
        ToggleStartup,
        CopyDetails,
        Quit,
        OpenOllamaUsage,
        OpenKimiConsole,
        ConfigureOpenCodeResets,
        /// Persist a complete provider order produced by a drag. Carrying the
        /// full order (rather than a "move before" pair) lets the commit reuse
        /// the exact drop index the preview showed.
        Reorder(Vec<Provider>),
    }

    #[derive(Clone)]
    struct PanelRow {
        rect: RECT,
        label: String,
        value: String,
        checked: bool,
        toggleable: bool,
        /// Action fired by the row's checkbox (visibility toggle).
        action: PanelAction,
        /// Action fired by the row label area. Quota windows focus the window;
        /// optional metric rows keep the visibility toggle for the whole row.
        focus_action: Option<PanelAction>,
        /// Whether this quota window is the focused window for its provider.
        focused: bool,
    }

    #[derive(Clone)]
    struct PanelCard {
        rect: RECT,
        provider: Provider,
        title: String,
        visible: bool,
        focused: bool,
        eye_rect: RECT,
        rows: Vec<PanelRow>,
        /// Placeholder slot shown at the current drop index while dragging. It
        /// has no provider/rows and is painted as an outline only.
        placeholder: bool,
    }

    struct PanelButton {
        rect: RECT,
        label: String,
        action: PanelAction,
    }

    /// State of an in-flight header gesture. The lifecycle is explicit:
    /// `pressed` (captured, below the movement threshold) becomes `active`
    /// once the pointer crosses [`DRAG_THRESHOLD_PX`], and ends exactly once
    /// on mouse-up, capture loss, Escape, or panel rebuild.
    struct PanelDragState {
        provider: Provider,
        press_point: POINT,
        pointer: POINT,
        active: bool,
        /// Full provider order captured at press (a permutation of the grid).
        origin_order: Vec<Provider>,
        /// Origin cards (rects, rows, visibility) captured at press.
        origin_cards: Vec<PanelCard>,
        /// Flow cards (origin minus the grabbed provider) reflowed into the
        /// two-column grid. This stable flow is only used for empty-slot
        /// insertion; card-on-card drops use the origin card rectangles so a
        /// target never shifts out from under the pointer.
        flow: Vec<PanelCard>,
        /// Drop index in flow space for an empty-slot insertion.
        drop_index: Option<usize>,
        /// Provider currently under the pointer, if any. Card-on-card drops
        /// swap this provider with the grabbed one.
        drop_target: Option<Provider>,
        /// The grabbed card's origin rect (floating preview anchor).
        grabbed_rect: RECT,
    }

    struct PanelLayout {
        width: i32,
        height: i32,
        cards: Vec<PanelCard>,
        buttons: Vec<PanelButton>,
    }

    struct PanelState {
        parent: HWND,
        layout: PanelLayout,
        result: Option<PanelAction>,
        drag: Option<PanelDragState>,
        /// A rebuild was requested while a drag owned the mouse; run it once
        /// the gesture ends instead of rebuilding mid-drag.
        rebuild_pending: bool,
    }

    fn rect_contains(rect: RECT, point: POINT) -> bool {
        point.x >= rect.left && point.x < rect.right && point.y >= rect.top && point.y < rect.bottom
    }

    fn panel_snapshot_value(snapshot: &UsageSnapshot, now: DateTime<Utc>) -> String {
        if snapshot.unit == "percent" {
            let remaining = snapshot
                .remaining
                .or_else(|| snapshot.used.map(|used| 100.0 - used));
            let amount = remaining
                .map(|value| format!("{:.0}% left", value.clamp(0.0, 100.0)))
                .unwrap_or_else(|| "—".to_string());
            let reset = snapshot
                .resets_at
                .map(|value| format_reset_label(Some(&value.to_rfc3339()), now))
                .unwrap_or_else(|| "—".to_string());
            return format!("{amount} · {reset}");
        }

        let amount = snapshot
            .used
            .map(|value| format!("{value:.0} {}", snapshot.unit))
            .unwrap_or_else(|| "—".to_string());
        if snapshot.unlimited {
            format!("{amount} · unlimited")
        } else {
            amount
        }
    }

    fn panel_status_row(snapshot: &UsageSnapshot) -> PanelRow {
        let value = if let Some(error) = &snapshot.error {
            error.code.as_str().replace('_', " ")
        } else {
            match snapshot.freshness {
                Freshness::Unavailable => "Unavailable".to_string(),
                Freshness::NotConfigured => "Not configured".to_string(),
                Freshness::NotApplicable => "Not applicable".to_string(),
                Freshness::Stale => "Stale".to_string(),
                Freshness::Cached => "Cached".to_string(),
                Freshness::Live => "No usage data".to_string(),
            }
        };
        PanelRow {
            rect: RECT::default(),
            label: "Status".to_string(),
            value,
            checked: false,
            toggleable: false,
            action: PanelAction::None,
            focus_action: None,
            focused: false,
        }
    }

    fn panel_disabled_status_row() -> PanelRow {
        PanelRow {
            rect: RECT::default(),
            label: "Status".to_string(),
            value: "Disabled".to_string(),
            checked: false,
            toggleable: false,
            action: PanelAction::None,
            focus_action: None,
            focused: false,
        }
    }

    fn panel_card_rows(
        snapshots: &[UsageSnapshot],
        provider: &Provider,
        view: &ResolvedView,
        focused_provider: Option<&Provider>,
        focus_window: Option<&str>,
        now: DateTime<Utc>,
    ) -> Vec<PanelRow> {
        let mut rows = Vec::new();
        for window in available_windows(snapshots, provider) {
            let snapshot = snapshots.iter().find(|candidate| {
                &candidate.provider == provider
                    && candidate.metric_kind == MetricKind::Quota
                    && candidate
                        .window_label
                        .as_deref()
                        .and_then(|label| canonical_window_key(provider, label))
                        == Some(window.as_str())
            });
            let value = snapshot
                .map(|snapshot| panel_snapshot_value(snapshot, now))
                .unwrap_or_else(|| "—".to_string());
            let checked = view
                .windows_for(provider)
                .map(|windows| windows.iter().any(|candidate| candidate == &window))
                .unwrap_or(true);
            rows.push(PanelRow {
                rect: RECT::default(),
                label: window_display_name(provider, &window),
                value,
                checked,
                toggleable: true,
                // Clicking the label focuses the window; the checkbox toggles
                // visibility. Focus and visibility stay independent, so Kimi's
                // 5-hour and weekly rows (and other multi-window providers)
                // remain visible and independently focusable.
                action: PanelAction::ToggleWindow(provider.clone(), window.clone()),
                focus_action: Some(PanelAction::FocusWindow(provider.clone(), window.clone())),
                focused: focused_provider == Some(provider)
                    && focus_window == Some(window.as_str()),
            });
        }

        for metric in available_metrics(snapshots, provider) {
            let snapshot = snapshots.iter().find(|candidate| {
                &candidate.provider == provider && candidate.metric_kind == metric
            });
            let value = snapshot
                .map(|snapshot| panel_snapshot_value(snapshot, now))
                .unwrap_or_else(|| "—".to_string());
            rows.push(PanelRow {
                rect: RECT::default(),
                label: metric_display_name(metric).to_string(),
                value,
                checked: view.is_metric_visible(provider, metric),
                toggleable: true,
                action: PanelAction::ToggleMetric(provider.clone(), metric),
                focus_action: None,
                focused: false,
            });
        }
        if available_windows(snapshots, provider).is_empty() {
            if let Some(snapshot) = snapshots
                .iter()
                .find(|candidate| &candidate.provider == provider)
            {
                if !rows.iter().any(|row| row.label == "Status") {
                    rows.push(panel_status_row(snapshot));
                }
            }
        }
        rows
    }

    /// Lay `cards` into a row-major grid with `columns` columns, updating each
    /// card's rect, eye rect, and row rects. Card heights are preserved.
    fn reflow_grid(cards: &mut [PanelCard], columns: usize) {
        if cards.is_empty() {
            return;
        }
        let columns = columns.max(1);
        let row_count = cards.len().div_ceil(columns);
        let mut row_heights = vec![0_i32; row_count];
        for (index, card) in cards.iter().enumerate() {
            let row = index / columns;
            row_heights[row] = row_heights[row].max(card.rect.bottom - card.rect.top);
        }
        let mut row_tops = Vec::with_capacity(row_heights.len());
        let mut top = PANEL_MARGIN + PANEL_HEADER_HEIGHT;
        for row_height in row_heights {
            row_tops.push(top);
            top += row_height + PANEL_GAP;
        }

        for (index, card) in cards.iter_mut().enumerate() {
            let column = index % columns;
            let row = index / columns;
            let left = PANEL_MARGIN + column as i32 * (PANEL_CARD_WIDTH + PANEL_GAP);
            let card_height = card.rect.bottom - card.rect.top;
            card.rect = RECT {
                left,
                top: row_tops[row],
                right: left + PANEL_CARD_WIDTH,
                bottom: row_tops[row] + card_height,
            };
            card.eye_rect = RECT {
                left: card.rect.right - 42,
                top: card.rect.top + 14,
                right: card.rect.right - 12,
                bottom: card.rect.top + 42,
            };
            let mut row_y = card.rect.top + PANEL_HEADER_HEIGHT;
            for item in &mut card.rows {
                item.rect = RECT {
                    left: card.rect.left + PANEL_CARD_PADDING,
                    top: row_y,
                    right: card.rect.right - PANEL_CARD_PADDING,
                    bottom: row_y + PANEL_ROW_HEIGHT,
                };
                row_y += PANEL_ROW_HEIGHT;
            }
        }
    }

    /// Recompute card and footer rectangles after a provider order or card
    /// height changes. Rows are laid out in a deterministic row-major grid;
    /// this makes the visual order match the order shown by drag-and-drop.
    fn reflow_panel_layout(layout: &mut PanelLayout) {
        let columns = layout.cards.len().clamp(1, 2);
        reflow_grid(&mut layout.cards, columns);
        layout.width =
            PANEL_MARGIN * 2 + columns as i32 * PANEL_CARD_WIDTH + (columns as i32 - 1) * PANEL_GAP;

        let card_bottom = layout
            .cards
            .iter()
            .map(|card| card.rect.bottom)
            .max()
            .unwrap_or(PANEL_MARGIN + PANEL_HEADER_HEIGHT);
        let footer_y = card_bottom + 4 + 8;
        let button_width = 108;
        let buttons_per_row = ((layout.width - PANEL_MARGIN * 2 + PANEL_GAP)
            / (button_width + PANEL_GAP))
            .max(1) as usize;
        for (index, button) in layout.buttons.iter_mut().enumerate() {
            let column = index % buttons_per_row;
            let row = index / buttons_per_row;
            let left = PANEL_MARGIN + column as i32 * (button_width + PANEL_GAP);
            let top = footer_y + row as i32 * (32 + PANEL_GAP);
            button.rect = RECT {
                left,
                top,
                right: left + button_width,
                bottom: top + 32,
            };
        }
        let footer_rows = layout.buttons.len().div_ceil(buttons_per_row);
        layout.height = footer_y
            + footer_rows.max(1) as i32 * 32
            + footer_rows.saturating_sub(1) as i32 * PANEL_GAP
            + PANEL_MARGIN;
    }

    fn build_control_panel_layout(hwnd: HWND) -> Option<PanelLayout> {
        let state = app_state_ref(hwnd)?;
        let available = state.known_providers.clone();
        let view = state.config.resolved_view(&available);
        let now = Utc::now();
        let mut cards = Vec::new();
        for provider in ordered_providers(&available, &view) {
            let active = provider_is_active(state, &provider, &view);
            if !active && !view.show_disabled_providers {
                continue;
            }
            let rows = if active {
                panel_card_rows(
                    &state.snapshots,
                    &provider,
                    &view,
                    state.focus_provider.as_ref(),
                    state.focus_window.as_deref(),
                    now,
                )
            } else {
                vec![panel_disabled_status_row()]
            };
            let card_height = PANEL_HEADER_HEIGHT
                + (rows.len() as i32 * PANEL_ROW_HEIGHT)
                + PANEL_CARD_PADDING * 2;
            cards.push(PanelCard {
                rect: RECT {
                    left: 0,
                    top: 0,
                    right: PANEL_CARD_WIDTH,
                    bottom: card_height,
                },
                title: provider_display_name(&provider).to_string(),
                visible: active,
                focused: active && state.focus_provider.as_ref() == Some(&provider),
                eye_rect: RECT::default(),
                provider,
                rows,
                placeholder: false,
            });
        }

        let mut footer_actions = vec![
            (
                if view.show_disabled_providers {
                    "Disabled ✓".to_string()
                } else {
                    "Disabled".to_string()
                },
                PanelAction::ToggleDisabledProviders,
            ),
            ("Refresh".to_string(), PanelAction::Refresh),
            (
                if ai_usage_bar::startup::auto_start_enabled().unwrap_or(false) {
                    "Startup ✓".to_string()
                } else {
                    "Startup".to_string()
                },
                PanelAction::ToggleStartup,
            ),
            ("Copy".to_string(), PanelAction::CopyDetails),
            ("Quit".to_string(), PanelAction::Quit),
        ];
        match state.focus_provider.as_ref() {
            Some(Provider::OllamaCloud) => {
                footer_actions.push(("Open usage".to_string(), PanelAction::OpenOllamaUsage));
            }
            Some(Provider::Kimi) => {
                footer_actions.push(("Open console".to_string(), PanelAction::OpenKimiConsole));
            }
            Some(Provider::OpenCodeGo) => {
                footer_actions.push((
                    "Reset times…".to_string(),
                    PanelAction::ConfigureOpenCodeResets,
                ));
            }
            _ => {}
        }
        let mut layout = PanelLayout {
            width: 0,
            height: 0,
            cards,
            buttons: footer_actions
                .into_iter()
                .map(|(label, action)| PanelButton {
                    rect: RECT::default(),
                    label: label.to_string(),
                    action,
                })
                .collect(),
        };
        reflow_panel_layout(&mut layout);
        Some(layout)
    }

    fn panel_state(hwnd: HWND) -> Option<&'static mut PanelState> {
        let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut PanelState;
        if ptr.is_null() {
            None
        } else {
            Some(unsafe { &mut *ptr })
        }
    }

    fn panel_window_position(parent: HWND, width: i32, height: i32) -> (i32, i32) {
        unsafe {
            let mut widget_rect = RECT::default();
            let _ = GetWindowRect(parent, &mut widget_rect);
            let (_, work) = panel_monitor_work_area(parent).unwrap_or((
                RECT {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                },
                RECT {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                },
            ));
            let mut x = widget_rect.left;
            let mut y = widget_rect.top - height - 8;
            if y < work.top {
                y = widget_rect.bottom + 8;
            }
            x = x.clamp(work.left, (work.right - width).max(work.left));
            y = y.clamp(work.top, (work.bottom - height).max(work.top));
            (x, y)
        }
    }

    fn position_control_panel_window(panel: HWND, parent: HWND, width: i32, height: i32) {
        let (x, y) = panel_window_position(parent, width, height);
        unsafe {
            let _ = SetWindowPos(
                panel,
                Some(HWND_TOPMOST),
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
    }

    fn rebuild_control_panel(panel: HWND) {
        let Some(parent) = panel_state(panel).map(|state| state.parent) else {
            return;
        };
        // Never rebuild mid-drag: the gesture owns the layout. Defer the
        // rebuild until the drag ends so a refresh result cannot wipe the
        // drag state (which previously caused repeated-drag lockups).
        if let Some(state) = panel_state(panel) {
            if state.drag.is_some() {
                state.rebuild_pending = true;
                return;
            }
        }
        let Some(layout) = build_control_panel_layout(parent) else {
            unsafe {
                let _ = DestroyWindow(panel);
            }
            return;
        };
        let (width, height) = (layout.width, layout.height);
        if let Some(state) = panel_state(panel) {
            state.layout = layout;
            state.drag = None;
            state.rebuild_pending = false;
        }
        position_control_panel_window(panel, parent, width, height);
        unsafe {
            let _ = InvalidateRect(Some(panel), None, false);
        }
    }

    fn draw_checkbox(hdc: HDC, rect: RECT, checked: bool, enabled: bool) {
        let color = if enabled { COLOR_MUTED } else { COLOR_BORDER };
        stroke_round_rect(hdc, rect, 3, color);
        if checked {
            draw_text(
                hdc,
                RECT {
                    left: rect.left,
                    top: rect.top - 1,
                    right: rect.right,
                    bottom: rect.bottom + 1,
                },
                "✓",
                13,
                FW_BOLD,
                if enabled { COLOR_GREEN } else { COLOR_NEUTRAL },
                DT_SINGLELINE | DT_CENTER | DT_VCENTER,
            );
        }
    }

    /// Checkbox hit/draw box for a quota or metric row.
    fn row_checkbox_rect(row_rect: RECT) -> RECT {
        RECT {
            left: row_rect.right - 20,
            top: row_rect.top + 7,
            right: row_rect.right - 4,
            bottom: row_rect.top + 23,
        }
    }

    /// Radio-style focus control for a quota row. The row itself remains a
    /// focus target for an easy click, but the explicit dot makes it clear
    /// which window currently drives the compact bar; the square checkbox to
    /// its right continues to control visibility independently.
    fn row_focus_rect(row_rect: RECT) -> RECT {
        RECT {
            left: row_rect.right - 48,
            top: row_rect.top + 7,
            right: row_rect.right - 30,
            bottom: row_rect.top + 25,
        }
    }

    /// Paint one provider card (grid card or the floating grab preview) at
    /// `rect`. Row/eye geometry is derived from `rect` so the same routine
    /// works for the grid and for the pointer-following grab card.
    fn paint_card(hdc: HDC, card: &PanelCard, rect: RECT, lifted: bool) {
        let background = if card.visible {
            COLOR_BACKGROUND
        } else {
            COLOR_OUTER
        };
        let border = if lifted {
            COLOR_YELLOW
        } else if card.focused {
            COLOR_GREEN
        } else if card.visible {
            COLOR_CARD_BORDER
        } else {
            COLOR_BORDER
        };
        fill_round_rect(hdc, rect, 10, background);
        stroke_round_rect(hdc, rect, 10, border);
        let eye_rect = RECT {
            left: rect.right - 42,
            top: rect.top + 14,
            right: rect.right - 12,
            bottom: rect.top + 42,
        };
        draw_text(
            hdc,
            RECT {
                left: rect.left + PANEL_CARD_PADDING,
                top: rect.top + 14,
                right: eye_rect.left - 4,
                bottom: rect.top + 48,
            },
            &card.title,
            17,
            FW_BOLD,
            if card.visible {
                COLOR_TEXT
            } else {
                COLOR_NEUTRAL
            },
            DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
        );
        draw_checkbox(hdc, eye_rect, card.visible, true);

        for (index, row) in card.rows.iter().enumerate() {
            let row_rect = RECT {
                left: rect.left + PANEL_CARD_PADDING,
                top: rect.top + PANEL_HEADER_HEIGHT + index as i32 * PANEL_ROW_HEIGHT,
                right: rect.right - PANEL_CARD_PADDING,
                bottom: rect.top
                    + PANEL_HEADER_HEIGHT
                    + index as i32 * PANEL_ROW_HEIGHT
                    + PANEL_ROW_HEIGHT,
            };
            let text_color = if card.visible && row.checked {
                COLOR_TEXT
            } else {
                COLOR_NEUTRAL
            };
            let focused = row.focused && card.visible;
            draw_text(
                hdc,
                RECT {
                    left: row_rect.left,
                    top: row_rect.top,
                    right: row_rect.left + 82,
                    bottom: row_rect.bottom,
                },
                &row.label,
                13,
                if focused { FW_BOLD } else { FW_NORMAL },
                if focused { COLOR_GREEN } else { text_color },
                DT_SINGLELINE | DT_VCENTER | DT_LEFT,
            );
            draw_text(
                hdc,
                RECT {
                    left: row_rect.left + 80,
                    top: row_rect.top,
                    right: row_focus_rect(row_rect).left - 6,
                    bottom: row_rect.bottom,
                },
                &row.value,
                12,
                FW_NORMAL,
                if card.visible && row.checked {
                    COLOR_MUTED
                } else {
                    COLOR_BORDER
                },
                DT_SINGLELINE | DT_VCENTER | DT_RIGHT | DT_END_ELLIPSIS,
            );
            if row.focus_action.is_some() {
                let focus_rect = row_focus_rect(row_rect);
                draw_text(
                    hdc,
                    focus_rect,
                    if focused { "●" } else { "○" },
                    14,
                    FW_BOLD,
                    if card.visible {
                        if focused {
                            COLOR_GREEN
                        } else {
                            COLOR_MUTED
                        }
                    } else {
                        COLOR_BORDER
                    },
                    DT_SINGLELINE | DT_VCENTER | DT_CENTER,
                );
            }
            if row.toggleable {
                draw_checkbox(hdc, row_checkbox_rect(row_rect), row.checked, card.visible);
            }
        }
    }

    /// Dashed outline for the empty drop slot during a drag.
    fn paint_placeholder(hdc: HDC, rect: RECT) {
        unsafe {
            let pen = CreatePen(PS_DASH, 1, COLOR_YELLOW);
            let old_pen = SelectObject(hdc, pen.into());
            let old_brush = SelectObject(hdc, GetStockObject(NULL_BRUSH));
            let _ = RoundRect(hdc, rect.left, rect.top, rect.right, rect.bottom, 10, 10);
            SelectObject(hdc, old_brush);
            SelectObject(hdc, old_pen);
            let _ = DeleteObject(pen.into());
        }
    }

    fn paint_control_panel(hwnd: HWND) {
        let Some(state) = panel_state(hwnd) else {
            return;
        };
        unsafe {
            let mut paint = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut paint);
            // Fill the real client rect so a mid-drag preview (which never
            // resizes the popup) cannot leave stale bands below the cards.
            let mut client = RECT {
                left: 0,
                top: 0,
                right: state.layout.width,
                bottom: state.layout.height,
            };
            let _ = GetClientRect(hwnd, &mut client);
            let buffer = if client.right > 0 && client.bottom > 0 {
                let buffer_dc = CreateCompatibleDC(Some(hdc));
                if buffer_dc.is_invalid() {
                    None
                } else {
                    let bitmap = CreateCompatibleBitmap(hdc, client.right, client.bottom);
                    if bitmap.is_invalid() {
                        let _ = DeleteDC(buffer_dc);
                        None
                    } else {
                        let old_bitmap = SelectObject(buffer_dc, bitmap.into());
                        Some((buffer_dc, bitmap, old_bitmap))
                    }
                }
            } else {
                None
            };
            let paint_hdc = buffer
                .as_ref()
                .map(|(buffer_dc, _, _)| *buffer_dc)
                .unwrap_or(hdc);

            fill_rect(paint_hdc, client, COLOR_OUTER);
            draw_text(
                paint_hdc,
                RECT {
                    left: PANEL_MARGIN,
                    top: 10,
                    right: state.layout.width - PANEL_MARGIN,
                    bottom: 34,
                },
                "AI Usage",
                20,
                FW_BOLD,
                COLOR_TEXT,
                DT_SINGLELINE | DT_VCENTER | DT_LEFT,
            );
            draw_text(
                paint_hdc,
                RECT {
                    left: PANEL_MARGIN,
                    top: 34,
                    right: state.layout.width - PANEL_MARGIN,
                    bottom: 52,
                },
                "Click a row or its dot to choose the bar timeframe · provider checks hide + disable · drag headers",
                12,
                FW_NORMAL,
                COLOR_MUTED,
                DT_SINGLELINE | DT_VCENTER | DT_LEFT,
            );

            let drop_target = state
                .drag
                .as_ref()
                .and_then(|drag| drag.drop_target.as_ref());
            for card in &state.layout.cards {
                if card.placeholder {
                    paint_placeholder(paint_hdc, card.rect);
                } else {
                    paint_card(
                        paint_hdc,
                        card,
                        card.rect,
                        drop_target == Some(&card.provider),
                    );
                }
            }

            for button in &state.layout.buttons {
                fill_round_rect(paint_hdc, button.rect, 6, COLOR_BACKGROUND);
                stroke_round_rect(paint_hdc, button.rect, 6, COLOR_CARD_BORDER);
                draw_text(
                    paint_hdc,
                    button.rect,
                    &button.label,
                    13,
                    FW_NORMAL,
                    COLOR_TEXT,
                    DT_SINGLELINE | DT_CENTER | DT_VCENTER,
                );
            }

            // The grabbed card floats above the grid, following the pointer
            // with the grab offset. It was removed from the flow, so it is
            // painted exactly once, on top.
            if let Some(drag) = &state.drag {
                if drag.active {
                    let dx = drag.pointer.x - drag.press_point.x;
                    let dy = drag.pointer.y - drag.press_point.y;
                    let rect = RECT {
                        left: drag.grabbed_rect.left + dx,
                        top: drag.grabbed_rect.top + dy,
                        right: drag.grabbed_rect.right + dx,
                        bottom: drag.grabbed_rect.bottom + dy,
                    };
                    if let Some(grabbed) = drag
                        .origin_cards
                        .iter()
                        .find(|card| card.provider == drag.provider)
                    {
                        paint_card(paint_hdc, grabbed, rect, true);
                    }
                }
            }

            if let Some((buffer_dc, bitmap, old_bitmap)) = buffer {
                let _ = BitBlt(
                    hdc,
                    0,
                    0,
                    client.right,
                    client.bottom,
                    Some(buffer_dc),
                    0,
                    0,
                    SRCCOPY,
                );
                SelectObject(buffer_dc, old_bitmap);
                let _ = DeleteObject(bitmap.into());
                let _ = DeleteDC(buffer_dc);
            }
            let _ = EndPaint(hwnd, &paint);
        }
    }

    fn panel_action_at(state: &PanelState, point: POINT) -> Option<PanelAction> {
        for card in &state.layout.cards {
            if card.placeholder {
                continue;
            }
            if rect_contains(card.eye_rect, point) {
                return Some(PanelAction::ToggleProvider(card.provider.clone()));
            }
            if !rect_contains(card.rect, point) {
                continue;
            }
            for row in &card.rows {
                if !rect_contains(row.rect, point) {
                    continue;
                }
                if !card.visible || !row.toggleable {
                    continue;
                }
                // Checkbox toggles visibility; the quota label focuses the
                // window. Optional metric rows keep the toggle for the whole
                // row (they have no window focus concept).
                if let Some(focus) = &row.focus_action {
                    if rect_contains(row_focus_rect(row.rect), point) {
                        return Some(focus.clone());
                    }
                }
                if rect_contains(row_checkbox_rect(row.rect), point) {
                    return Some(row.action.clone());
                }
                if let Some(focus) = &row.focus_action {
                    return Some(focus.clone());
                }
                return Some(row.action.clone());
            }
            return Some(if card.visible {
                PanelAction::FocusProvider(card.provider.clone())
            } else {
                PanelAction::ToggleProvider(card.provider.clone())
            });
        }
        state
            .layout
            .buttons
            .iter()
            .find(|button| rect_contains(button.rect, point))
            .map(|button| button.action.clone())
    }

    fn panel_drag_card_at(state: &PanelState, point: POINT) -> Option<Provider> {
        state.layout.cards.iter().find_map(|card| {
            if card.placeholder || !card.visible {
                return None;
            }
            let header = RECT {
                left: card.rect.left,
                top: card.rect.top,
                right: card.rect.right,
                bottom: card.rect.top + PANEL_HEADER_HEIGHT,
            };
            (rect_contains(header, point) && !rect_contains(card.eye_rect, point))
                .then(|| card.provider.clone())
        })
    }

    /// A placeholder card for the current drop slot. It carries the grabbed
    /// card's height so the grid rows keep their shape while the rest of the
    /// flow reflows around the slot.
    fn placeholder_card(drag: &PanelDragState) -> PanelCard {
        PanelCard {
            rect: RECT {
                left: 0,
                top: 0,
                right: PANEL_CARD_WIDTH,
                bottom: drag.grabbed_rect.bottom - drag.grabbed_rect.top,
            },
            provider: drag.provider.clone(),
            title: String::new(),
            visible: false,
            focused: false,
            eye_rect: RECT::default(),
            rows: Vec::new(),
            placeholder: true,
        }
    }

    fn to_slot_rect(rect: RECT) -> SlotRect {
        SlotRect {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        }
    }

    /// The two-column grid bounds that the drop model resolves against.
    fn panel_grid_bounds(cards: &[PanelCard]) -> SlotRect {
        SlotRect {
            left: PANEL_MARGIN,
            top: cards
                .iter()
                .map(|card| card.rect.top)
                .min()
                .unwrap_or(PANEL_MARGIN + PANEL_HEADER_HEIGHT),
            right: PANEL_MARGIN + 2 * PANEL_CARD_WIDTH + PANEL_GAP,
            bottom: cards
                .iter()
                .map(|card| card.rect.bottom)
                .max()
                .unwrap_or(PANEL_MARGIN + PANEL_HEADER_HEIGHT),
        }
    }

    fn panel_drop_cards(cards: &[PanelCard]) -> Vec<DropCard<Provider>> {
        cards
            .iter()
            .map(|card| DropCard {
                id: card.provider.clone(),
                rect: to_slot_rect(card.rect),
                visible: card.visible,
            })
            .collect()
    }

    fn preview_cards_for_drag(drag: &PanelDragState) -> Vec<PanelCard> {
        // Keep the grid in its origin arrangement while dragging. The grabbed
        // card leaves a stable outline at its original slot, and the real card
        // follows the pointer. This prevents the card under the pointer from
        // moving every time the candidate drop index changes.
        let mut cards = drag.origin_cards.clone();
        if let Some(card) = cards.iter_mut().find(|card| card.provider == drag.provider) {
            let rect = card.rect;
            let mut placeholder = placeholder_card(drag);
            placeholder.rect = rect;
            *card = placeholder;
        }
        cards
    }

    fn card_drop_target_at(drag: &PanelDragState, point: POINT) -> Option<Provider> {
        drag.origin_cards.iter().find_map(|card| {
            (card.provider != drag.provider && card.visible && rect_contains(card.rect, point))
                .then(|| card.provider.clone())
        })
    }

    /// Resolve an empty-slot insertion against the stable flow captured when
    /// the drag began. Card-on-card targets are handled separately as swaps.
    fn resolve_insert_index_at(drag: &PanelDragState, point: POINT) -> Option<usize> {
        // Releasing over the grabbed card's origin is a cancellation, not an
        // append: the dragged card is intentionally absent from the flow used
        // for empty-slot hit-testing.
        if drag
            .origin_cards
            .iter()
            .find(|card| card.provider == drag.provider)
            .is_some_and(|card| rect_contains(card.rect, point))
        {
            return None;
        }
        if card_drop_target_at(drag, point).is_some() {
            return None;
        }
        let grid = panel_grid_bounds(&drag.flow);
        let drop_cards = panel_drop_cards(&drag.flow);
        DropGrid::drop_index(&drop_cards, grid, (point.x, point.y))
    }

    fn resolve_insert_index(drag: &PanelDragState) -> Option<usize> {
        resolve_insert_index_at(drag, drag.pointer)
    }

    /// Replace the live layout with the stable origin grid plus a placeholder
    /// at the grabbed card's original slot. The popup is never resized or
    /// repositioned here; that happens once on commit via the panel rebuild.
    fn rebuild_preview_cards(state: &mut PanelState) {
        let Some(drag) = state.drag.as_ref() else {
            return;
        };
        let cards = preview_cards_for_drag(drag);
        state.layout.cards = cards;
        reflow_panel_layout(&mut state.layout);
    }

    /// Update the stable card target or empty-slot index from the current
    /// pointer. The origin grid is only rebuilt when that target changes;
    /// pointer movement itself only repaints the floating card.
    fn update_drag_preview(state: &mut PanelState) -> bool {
        let Some(drag) = state.drag.as_ref() else {
            return false;
        };
        let target = card_drop_target_at(drag, drag.pointer);
        let index = target
            .is_none()
            .then(|| resolve_insert_index(drag))
            .flatten();
        if state
            .drag
            .as_ref()
            .is_some_and(|drag| drag.drop_target == target && drag.drop_index == index)
        {
            return false;
        }
        if let Some(drag) = state.drag.as_mut() {
            drag.drop_target = target;
            drag.drop_index = index;
        }
        rebuild_preview_cards(state);
        true
    }

    /// Restore the layout captured at press (used for cancelled drags and
    /// capture loss).
    fn restore_origin_layout(state: &mut PanelState, origin_cards: &[PanelCard]) {
        state.layout.cards = origin_cards.to_vec();
        reflow_panel_layout(&mut state.layout);
    }

    /// Route a mouse release to a panel action, returning the action to
    /// dispatch and whether the pre-press layout must be restored (a
    /// cancelled gesture). Releases are classified by the gesture that
    /// started at press, so every click reaches the hit-tester:
    ///
    /// - No gesture (`Click`): the press landed on the eye toggle, a
    ///   quota/metric row checkbox, a row, or a footer button — all of which
    ///   live outside the draggable header. Hit-test the release point
    ///   directly; this is the path that toggles provider and quota-window
    ///   visibility, so it must run even though `state.drag` is `None`.
    /// - Inactive header gesture (`HeaderClick`): provider focus/visibility
    ///   hit test at the release point; a miss on empty chrome restores the
    ///   pre-press layout.
    /// - Active drag (`Reorder`): swap or empty-slot insertion; releasing
    ///   with no target restores the origin layout.
    fn resolve_lbutton_release(
        state: &PanelState,
        drag: Option<&PanelDragState>,
        point: POINT,
    ) -> (Option<PanelAction>, bool) {
        match release_route(drag.map(|drag| drag.active)) {
            ReleaseRoute::Click => (panel_action_at(state, point), false),
            ReleaseRoute::HeaderClick => {
                let action = panel_action_at(state, point);
                let restore = action.is_none();
                (action, restore)
            }
            ReleaseRoute::Reorder => {
                let drag = drag.expect("Reorder route implies a started gesture");
                // A card-on-card drop is a direct swap. The target remains at
                // its origin rectangle throughout the gesture, so the
                // committed order cannot rotate unrelated cards through the
                // two-column grid.
                let action = if let Some(target) = card_drop_target_at(drag, point) {
                    let mut order = drag.origin_order.clone();
                    swap_drop(&mut order, &drag.provider, &target)
                        .then_some(PanelAction::Reorder(order))
                } else {
                    resolve_insert_index_at(drag, point).and_then(|index| {
                        let mut order = drag.origin_order.clone();
                        apply_drop(&mut order, &drag.provider, index)
                            .then_some(PanelAction::Reorder(order))
                    })
                };
                let restore = action.is_none();
                (action, restore)
            }
        }
    }

    fn point_from_lparam(lparam: LPARAM) -> POINT {
        POINT {
            x: (lparam.0 as i16) as i32,
            y: ((lparam.0 >> 16) as i16) as i32,
        }
    }

    unsafe extern "system" fn control_panel_wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_PAINT => {
                paint_control_panel(hwnd);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_MOUSEMOVE => {
                if let Some(state) = panel_state(hwnd) {
                    if let Some(drag) = &mut state.drag {
                        drag.pointer = point_from_lparam(lparam);
                        if !drag.active {
                            let dx = drag.pointer.x - drag.press_point.x;
                            let dy = drag.pointer.y - drag.press_point.y;
                            // Small movement threshold: ordinary header
                            // clicks stay clicks (provider focus); only real
                            // drags enter the floating preview.
                            if dx * dx + dy * dy >= DRAG_THRESHOLD_PX * DRAG_THRESHOLD_PX {
                                drag.active = true;
                                update_drag_preview(state);
                                let _ = InvalidateRect(Some(hwnd), None, false);
                            }
                        } else {
                            update_drag_preview(state);
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                    }
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
            WM_LBUTTONDOWN => {
                let point = point_from_lparam(lparam);
                if let Some(state) = panel_state(hwnd) {
                    if state.drag.is_none() {
                        if let Some(provider) = panel_drag_card_at(state, point) {
                            let Some(card) = state
                                .layout
                                .cards
                                .iter()
                                .find(|card| card.provider == provider)
                            else {
                                return LRESULT(0);
                            };
                            let grabbed_rect = card.rect;
                            let origin_order: Vec<Provider> = state
                                .layout
                                .cards
                                .iter()
                                .filter(|card| card.visible)
                                .map(|card| card.provider.clone())
                                .collect();
                            let origin_cards = state.layout.cards.clone();
                            // Flow = origin minus the grabbed provider,
                            // reflowed in two columns so the drop model sees
                            // the same geometry the preview paints.
                            let mut flow: Vec<PanelCard> = origin_cards
                                .iter()
                                .filter(|card| card.provider != provider)
                                .cloned()
                                .collect();
                            reflow_grid(&mut flow, GRID_COLUMNS);
                            state.drag = Some(PanelDragState {
                                press_point: point,
                                pointer: point,
                                active: false,
                                origin_order,
                                origin_cards,
                                flow,
                                drop_index: None,
                                drop_target: None,
                                grabbed_rect,
                                provider,
                            });
                            let _ = SetCapture(hwnd);
                        }
                    }
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let point = point_from_lparam(lparam);
                let mut rebuild = false;
                if let Some(state) = panel_state(hwnd) {
                    let drag = state.drag.take();
                    let (action, restore) = resolve_lbutton_release(state, drag.as_ref(), point);
                    if restore {
                        // Cancelled gesture (invalid drop target or a header
                        // click that landed on empty chrome): restore the
                        // pre-press layout.
                        let drag = drag.expect("restore is only requested for started gestures");
                        restore_origin_layout(state, &drag.origin_cards);
                        rebuild = state.rebuild_pending;
                    }
                    if let Some(action) = action {
                        state.result = Some(action);
                    }
                }
                let _ = ReleaseCapture();
                // A rebuild deferred during the drag runs now only when there
                // is no action; action paths rebuild after dispatch.
                if rebuild {
                    rebuild_control_panel(hwnd);
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_APP_PANEL_REBUILD => {
                if let Some(state) = panel_state(hwnd) {
                    if state.drag.is_some() {
                        // Deferred rebuild: the drag owns the layout.
                        state.rebuild_pending = true;
                        return LRESULT(0);
                    }
                }
                rebuild_control_panel(hwnd);
                LRESULT(0)
            }
            WM_KEYDOWN if wparam.0 == VK_ESCAPE.0 as usize => {
                if let Some(state) = panel_state(hwnd) {
                    if let Some(drag) = state.drag.take() {
                        restore_origin_layout(state, &drag.origin_cards);
                    }
                }
                let _ = ReleaseCapture();
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_KILLFOCUS => {
                let _ = ReleaseCapture();
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_CAPTURECHANGED => {
                // Fires when the gesture ends by ReleaseCapture or when the
                // system takes capture away (alt-tab, other window). Mouse-up
                // already takes the drag, so this runs at most once per
                // gesture and only for genuinely lost capture.
                let mut rebuild = false;
                if let Some(state) = panel_state(hwnd) {
                    if let Some(drag) = state.drag.take() {
                        restore_origin_layout(state, &drag.origin_cards);
                        rebuild = state.rebuild_pending;
                    }
                }
                if rebuild {
                    rebuild_control_panel(hwnd);
                }
                LRESULT(0)
            }
            WM_NCDESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    fn dispatch_panel_action(hwnd: HWND, action: PanelAction) {
        match action {
            PanelAction::FocusProvider(provider) => set_focus_provider(hwnd, provider),
            // Focusing a window also focuses its provider, then selects the
            // window; reselecting the same provider keeps its focused window.
            PanelAction::FocusWindow(provider, window) => {
                set_focus_provider(hwnd, provider);
                set_focus_window(hwnd, &window);
            }
            PanelAction::None => {}
            PanelAction::ToggleProvider(provider) => toggle_provider_control(hwnd, provider),
            PanelAction::ToggleDisabledProviders => toggle_disabled_providers_visibility(hwnd),
            PanelAction::ToggleWindow(provider, window) => {
                toggle_window_visibility(hwnd, provider, window)
            }
            PanelAction::ToggleMetric(provider, metric) => {
                toggle_metric_visibility(hwnd, provider, metric)
            }
            PanelAction::Refresh => begin_refresh(hwnd),
            PanelAction::ToggleStartup => toggle_startup_registration(hwnd),
            PanelAction::CopyDetails => copy_details_to_clipboard(hwnd),
            PanelAction::OpenOllamaUsage => open_ollama_usage_page(hwnd),
            PanelAction::OpenKimiConsole => open_kimi_console(hwnd),
            PanelAction::ConfigureOpenCodeResets => open_opencode_reset_dialog(hwnd),
            PanelAction::Quit => unsafe {
                let _ = DestroyWindow(hwnd);
            },
            PanelAction::Reorder(order) => reorder_provider(hwnd, order),
        }
    }

    fn show_control_panel(hwnd: HWND) -> bool {
        if let Some(state) = app_state(hwnd) {
            set_tooltip_visible(hwnd, state, false);
        }
        let Some(layout) = build_control_panel_layout(hwnd) else {
            return false;
        };

        unsafe {
            let hinst = match GetModuleHandleW(None) {
                Ok(module) => HINSTANCE(module.0),
                Err(_) => return false,
            };
            static REGISTER_CLASS: Once = Once::new();
            REGISTER_CLASS.call_once(|| {
                let class = WNDCLASSW {
                    lpfnWndProc: Some(control_panel_wnd_proc),
                    hInstance: hinst,
                    hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                    lpszClassName: CONTROL_PANEL_CLASS,
                    style: CS_HREDRAW | CS_VREDRAW,
                    ..Default::default()
                };
                let _ = RegisterClassW(&class);
            });

            let mut state = Box::new(PanelState {
                parent: hwnd,
                layout,
                result: None,
                drag: None,
                rebuild_pending: false,
            });
            let state_ptr = (&mut *state) as *mut PanelState;
            let (x, y) = panel_window_position(hwnd, state.layout.width, state.layout.height);
            let panel = match CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                CONTROL_PANEL_CLASS,
                w!("AI Usage"),
                WS_POPUP | WS_BORDER | WS_CLIPCHILDREN,
                x,
                y,
                state.layout.width,
                state.layout.height,
                Some(hwnd),
                None,
                Some(hinst),
                None,
            ) {
                Ok(panel) => panel,
                Err(_) => return false,
            };
            SetWindowLongPtrW(panel, GWLP_USERDATA, state_ptr as isize);
            if let Some(app) = app_state(hwnd) {
                app.panel_hwnd = Some(panel);
            }
            position_control_panel_window(panel, hwnd, state.layout.width, state.layout.height);
            let _ = SetForegroundWindow(panel);
            let _ = SetFocus(Some(panel));

            let mut message = MSG::default();
            let mut parent_alive = true;
            while IsWindow(Some(panel)).as_bool() {
                let result = GetMessageW(&mut message, None, 0, 0);
                if result.0 <= 0 {
                    if result.0 == 0 {
                        PostQuitMessage(message.wParam.0 as i32);
                    }
                    break;
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);

                let action = panel_state(panel).and_then(|state| state.result.take());
                if let Some(action) = action {
                    if matches!(action, PanelAction::Quit) {
                        let _ = DestroyWindow(panel);
                        dispatch_panel_action(hwnd, action);
                        parent_alive = false;
                        break;
                    }
                    dispatch_panel_action(hwnd, action);
                    if IsWindow(Some(panel)).as_bool() {
                        rebuild_control_panel(panel);
                    }
                }
            }
            if parent_alive && IsWindow(Some(hwnd)).as_bool() {
                if let Some(app) = app_state(hwnd) {
                    if app.panel_hwnd == Some(panel) {
                        app.panel_hwnd = None;
                    }
                }
            }
            drop(state);
            if parent_alive && IsWindow(Some(hwnd)).as_bool() {
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
                ensure_widget_topmost(hwnd);
            }
            true
        }
    }

    fn panel_monitor_work_area(hwnd: HWND) -> Option<(RECT, RECT)> {
        unsafe {
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
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

    fn show_context_menu(hwnd: HWND) {
        // Dismiss hover tip first so the menu is not stacked under/over it.
        if let Some(state) = app_state(hwnd) {
            set_tooltip_visible(hwnd, state, false);
        }

        unsafe {
            let Ok(menu) = CreatePopupMenu() else {
                return;
            };
            let _ = AppendMenuW(menu, MF_STRING, MENU_REFRESH, w!("Refresh"));
            let startup_flags = if ai_usage_bar::startup::auto_start_enabled().unwrap_or(false) {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING
            };
            let _ = AppendMenuW(
                menu,
                startup_flags,
                MENU_TOGGLE_STARTUP,
                w!("Run on Windows startup"),
            );

            let known_providers = app_state_ref(hwnd)
                .map(|state| state.known_providers.clone())
                .unwrap_or_default();
            let resolved_view = app_state_ref(hwnd)
                .map(|state| state.config.resolved_view(&known_providers))
                .unwrap_or_default();
            let disabled_flags = if resolved_view.show_disabled_providers {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING
            };
            let _ = AppendMenuW(
                menu,
                disabled_flags,
                MENU_TOGGLE_DISABLED,
                if resolved_view.show_disabled_providers {
                    w!("Hide disabled providers")
                } else {
                    w!("Show disabled providers")
                },
            );

            // Keep wide strings alive until TrackPopupMenu returns.
            let mut provider_labels: Vec<Vec<u16>> = Vec::new();
            let mut window_labels: Vec<Vec<u16>> = Vec::new();
            let mut visibility_labels: Vec<Vec<u16>> = Vec::new();
            let mut visibility_window_commands: Vec<(Provider, String)> = Vec::new();
            let mut visibility_metric_commands: Vec<(Provider, MetricKind)> = Vec::new();
            let switchable = app_state_ref(hwnd)
                .map(|s| s.switchable_providers.clone())
                .unwrap_or_default();
            let focused = app_state_ref(hwnd).and_then(|s| s.focus_provider.clone());
            if !switchable.is_empty() {
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, w!(""));
                for (index, provider) in switchable.iter().take(MENU_SHOW_PROVIDER_MAX).enumerate()
                {
                    let checked = focused.as_ref() == Some(provider);
                    let flags = if checked {
                        MF_STRING | MF_CHECKED
                    } else {
                        MF_STRING
                    };
                    // Short names only: "Codex" / "Grok", not "Show …".
                    let label = provider_display_name(provider).to_string();
                    provider_labels.push(to_wide(&label));
                    let _ = AppendMenuW(
                        menu,
                        flags,
                        MENU_SHOW_PROVIDER_BASE + index,
                        PCWSTR(provider_labels[index].as_ptr()),
                    );
                }
            }

            // Provider controls below use the same hide-and-disable behavior
            // as the expanded panel. Window and metric controls remain
            // display-only.
            let all_providers = app_state_ref(hwnd)
                .map(|state| {
                    known_providers
                        .iter()
                        .filter(|provider| {
                            provider_is_active(state, provider, &resolved_view)
                                || resolved_view.show_disabled_providers
                        })
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            // All provider/window/metric controls live in one top-level "Show/hide"
            // submenu. Provider visibility is global; window and optional
            // metric visibility are nested under the provider they affect so
            // labels such as "Show 5-hour" cannot be mistaken for a global
            // setting.
            let submenu = CreatePopupMenu().ok();
            if let Some(submenu) = submenu.as_ref() {
                for (index, provider) in all_providers
                    .iter()
                    .take(MENU_VISIBLE_PROVIDER_MAX)
                    .enumerate()
                {
                    let flags = if !resolved_view.is_provider_hidden(provider) {
                        MF_STRING | MF_CHECKED
                    } else {
                        MF_STRING
                    };
                    let label = format!("Show {}", provider_display_name(provider));
                    visibility_labels.push(to_wide(&label));
                    let _ = AppendMenuW(
                        *submenu,
                        flags,
                        MENU_VISIBLE_PROVIDER_BASE + index,
                        PCWSTR(visibility_labels.last().unwrap().as_ptr()),
                    );
                }

                if !all_providers.is_empty() {
                    let _ = AppendMenuW(*submenu, MF_SEPARATOR, 0, w!(""));
                }

                for provider in all_providers.iter().take(MENU_VISIBLE_PROVIDER_MAX) {
                    let Ok(provider_menu) = CreatePopupMenu() else {
                        continue;
                    };
                    let mut has_items = false;
                    let all_windows = available_windows(
                        app_state_ref(hwnd)
                            .map(|state| state.snapshots.as_slice())
                            .unwrap_or_default(),
                        provider,
                    );
                    let visible_windows = resolved_view.windows_for(provider);
                    for window in all_windows.iter().take(MENU_SHOW_WINDOW_MAX) {
                        if visibility_window_commands.len() >= MENU_VISIBLE_WINDOW_MAX {
                            break;
                        }
                        let checked = visible_windows
                            .map(|windows| windows.iter().any(|candidate| candidate == window))
                            .unwrap_or(true);
                        let flags = if checked {
                            MF_STRING | MF_CHECKED
                        } else {
                            MF_STRING
                        };
                        let label = format!("Show {}", window_display_name(provider, window));
                        visibility_labels.push(to_wide(&label));
                        let index = visibility_window_commands.len();
                        let appended = AppendMenuW(
                            provider_menu,
                            flags,
                            MENU_VISIBLE_WINDOW_BASE + index,
                            PCWSTR(visibility_labels.last().unwrap().as_ptr()),
                        )
                        .is_ok();
                        if appended {
                            visibility_window_commands.push((provider.clone(), window.clone()));
                            has_items = true;
                        }
                    }

                    let all_metrics = app_state_ref(hwnd)
                        .map(|state| available_metrics(&state.snapshots, provider))
                        .unwrap_or_default();
                    if !all_metrics.is_empty() && has_items {
                        let _ = AppendMenuW(provider_menu, MF_SEPARATOR, 0, w!(""));
                    }
                    for metric in all_metrics {
                        if visibility_metric_commands.len() >= MENU_VISIBLE_METRIC_MAX {
                            break;
                        }
                        let flags = if resolved_view.is_metric_visible(provider, metric) {
                            MF_STRING | MF_CHECKED
                        } else {
                            MF_STRING
                        };
                        let label = format!("Show {}", metric_display_name(metric));
                        visibility_labels.push(to_wide(&label));
                        let index = visibility_metric_commands.len();
                        let appended = AppendMenuW(
                            provider_menu,
                            flags,
                            MENU_VISIBLE_METRIC_BASE + index,
                            PCWSTR(visibility_labels.last().unwrap().as_ptr()),
                        )
                        .is_ok();
                        if appended {
                            visibility_metric_commands.push((provider.clone(), metric));
                            has_items = true;
                        }
                    }

                    if has_items {
                        if GetMenuItemCount(Some(*submenu)) > 0 {
                            let _ = AppendMenuW(*submenu, MF_SEPARATOR, 0, w!(""));
                        }
                        let label = to_wide(provider_display_name(provider));
                        let attached = AppendMenuW(
                            *submenu,
                            MF_POPUP,
                            provider_menu.0 as usize,
                            PCWSTR(label.as_ptr()),
                        )
                        .is_ok();
                        if attached {
                            visibility_labels.push(label);
                        } else {
                            let _ = DestroyMenu(provider_menu);
                        }
                    } else {
                        let _ = DestroyMenu(provider_menu);
                    }
                }
            }

            let available_windows = focused
                .as_ref()
                .and_then(|provider| {
                    app_state_ref(hwnd).map(|state| {
                        let mut labels = Vec::new();
                        for snapshot in &state.snapshots {
                            if &snapshot.provider != provider
                                || snapshot.unit != "percent"
                                || snapshot.used.is_none()
                            {
                                continue;
                            }
                            let Some(label) = snapshot.window_label.as_deref() else {
                                continue;
                            };
                            let Some(canonical) = canonical_window_key(provider, label) else {
                                continue;
                            };
                            if !labels.iter().any(|existing| existing == canonical) {
                                labels.push(canonical.to_string());
                            }
                        }
                        labels.sort_by_key(|label| window_sort_key(label));
                        labels
                    })
                })
                .unwrap_or_default();
            if !available_windows.is_empty() {
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, w!(""));
                let selected_window = focused
                    .as_ref()
                    .and_then(|provider| {
                        app_state_ref(hwnd).and_then(|state| {
                            state.focus_window.as_deref().and_then(|window| {
                                canonical_window_key(provider, window).map(str::to_string)
                            })
                        })
                    })
                    .unwrap_or_else(|| {
                        if focused.as_ref().is_some_and(|provider| {
                            matches!(provider, Provider::Kimi | Provider::OpenCodeGo)
                        }) {
                            "5-hour".to_string()
                        } else {
                            "session".to_string()
                        }
                    });
                for (index, window) in available_windows
                    .iter()
                    .take(MENU_SHOW_WINDOW_MAX)
                    .enumerate()
                {
                    let checked = selected_window.as_str() == window.as_str();
                    let flags = if checked {
                        MF_STRING | MF_CHECKED
                    } else {
                        MF_STRING
                    };
                    let label = focused
                        .as_ref()
                        .map(|provider| window_display_name(provider, window))
                        .unwrap_or_else(|| window.to_string());
                    window_labels.push(to_wide(&label));
                    let _ = AppendMenuW(
                        menu,
                        flags,
                        MENU_SHOW_WINDOW_BASE + index,
                        PCWSTR(window_labels[index].as_ptr()),
                    );
                }
            }

            // Attach the submenu only when it holds at least one control. If
            // attaching it fails, destroy the unattached popup immediately;
            // the parent menu can only clean up submenus it owns.
            let _ = submenu.and_then(|handle| {
                if GetMenuItemCount(Some(handle)) <= 0 {
                    let _ = DestroyMenu(handle);
                    return None;
                }

                let separator_added = AppendMenuW(menu, MF_SEPARATOR, 0, w!("")).is_ok();
                let attached = separator_added
                    && AppendMenuW(menu, MF_POPUP, handle.0 as usize, w!("Show/hide")).is_ok();
                if attached {
                    Some(handle)
                } else {
                    let _ = DestroyMenu(handle);
                    None
                }
            });

            let ollama_focused = focused.as_ref() == Some(&Provider::OllamaCloud);
            let kimi_focused = focused.as_ref() == Some(&Provider::Kimi);
            let opencode_focused = focused.as_ref() == Some(&Provider::OpenCodeGo);
            if ollama_focused || kimi_focused || opencode_focused {
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, w!(""));
                if ollama_focused {
                    let _ = AppendMenuW(
                        menu,
                        MF_STRING,
                        MENU_OPEN_OLLAMA_USAGE,
                        w!("Open Ollama usage page"),
                    );
                }
                if kimi_focused {
                    let _ = AppendMenuW(
                        menu,
                        MF_STRING,
                        MENU_OPEN_KIMI_CONSOLE,
                        w!("Open Kimi Console"),
                    );
                }
                if opencode_focused {
                    let _ = AppendMenuW(
                        menu,
                        MF_STRING,
                        MENU_CONFIG_OPENCODE_RESETS,
                        w!("Configure OpenCode reset times…"),
                    );
                }
            }

            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, w!(""));
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                MENU_COPY_DETAILS,
                w!("Copy details to clipboard"),
            );
            let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Quit"));

            // Anchor the menu fully *above* the pill. Popup menus are not
            // topmost, so any geometric overlap with the pill paints under it.
            // Do **not** demote the pill from HWND_TOPMOST — near the taskbar
            // that makes the pill vanish under the tray until restored.
            let mut widget_rect = RECT::default();
            let (menu_x, menu_y) = if GetWindowRect(hwnd, &mut widget_rect).is_ok() {
                // TPM_BOTTOMALIGN: bottom edge of the menu sits at (x, y), so
                // the whole menu grows upward and clears the pill.
                (widget_rect.left, widget_rect.top - 2)
            } else {
                let mut point = POINT::default();
                let _ = GetCursorPos(&mut point);
                (point.x, point.y)
            };
            // Without foreground ownership, Win32 popup menus ignore outside
            // clicks and stay open until an item is chosen.
            let _ = SetForegroundWindow(hwnd);
            let command = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_BOTTOMALIGN,
                menu_x,
                menu_y,
                Some(0),
                hwnd,
                None,
            )
            .0 as usize;
            // Required so the next click is delivered and the menu fully tears down.
            let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(menu);
            // Keep labels live until here.
            let _ = provider_labels;
            let _ = window_labels;
            let _ = visibility_labels;

            match command {
                MENU_REFRESH => begin_refresh(hwnd),
                MENU_OPEN_OLLAMA_USAGE => open_ollama_usage_page(hwnd),
                MENU_OPEN_KIMI_CONSOLE => open_kimi_console(hwnd),
                MENU_CONFIG_OPENCODE_RESETS => open_opencode_reset_dialog(hwnd),
                MENU_TOGGLE_STARTUP => toggle_startup_registration(hwnd),
                MENU_TOGGLE_DISABLED => toggle_disabled_providers_visibility(hwnd),
                MENU_COPY_DETAILS => copy_details_to_clipboard(hwnd),
                MENU_QUIT => {
                    let _ = DestroyWindow(hwnd);
                }
                id if (MENU_SHOW_PROVIDER_BASE
                    ..MENU_SHOW_PROVIDER_BASE + MENU_SHOW_PROVIDER_MAX)
                    .contains(&id) =>
                {
                    let index = id - MENU_SHOW_PROVIDER_BASE;
                    if let Some(provider) = switchable.get(index).cloned() {
                        set_focus_provider(hwnd, provider);
                    }
                }
                id if (MENU_SHOW_WINDOW_BASE..MENU_SHOW_WINDOW_BASE + MENU_SHOW_WINDOW_MAX)
                    .contains(&id) =>
                {
                    if let Some(window) = available_windows.get(id - MENU_SHOW_WINDOW_BASE) {
                        set_focus_window(hwnd, window);
                    }
                }
                id if (MENU_VISIBLE_PROVIDER_BASE
                    ..MENU_VISIBLE_PROVIDER_BASE + MENU_VISIBLE_PROVIDER_MAX)
                    .contains(&id) =>
                {
                    if let Some(provider) = all_providers.get(id - MENU_VISIBLE_PROVIDER_BASE) {
                        toggle_provider_control(hwnd, provider.clone());
                    }
                }
                id if (MENU_VISIBLE_WINDOW_BASE
                    ..MENU_VISIBLE_WINDOW_BASE + MENU_VISIBLE_WINDOW_MAX)
                    .contains(&id) =>
                {
                    if let Some((provider, window)) =
                        visibility_window_commands.get(id - MENU_VISIBLE_WINDOW_BASE)
                    {
                        toggle_window_visibility(hwnd, provider.clone(), window.clone());
                    }
                }
                id if (MENU_VISIBLE_METRIC_BASE
                    ..MENU_VISIBLE_METRIC_BASE + MENU_VISIBLE_METRIC_MAX)
                    .contains(&id) =>
                {
                    if let Some((provider, metric)) =
                        visibility_metric_commands.get(id - MENU_VISIBLE_METRIC_BASE)
                    {
                        toggle_metric_visibility(hwnd, provider.clone(), *metric);
                    }
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
                        // Pill text is only the provider name (or "…" while refreshing).
                        let label = if state.refresh_in_flight {
                            "…"
                        } else {
                            state.pill_status.as_str()
                        };
                        (
                            state.used_percent,
                            state.refresh_in_flight,
                            Some(label.to_string()),
                        )
                    })
                    .unwrap_or((None, false, None));
                paint_widget(hwnd, used_percent, refreshing, status.as_deref());
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
                // Left-click cycles the compact pill across providers when more
                // than one is available; otherwise it refreshes.
                cycle_focus_provider(hwnd);
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                // Swallow further mouse-move tip until the menu is done.
                if !show_control_panel(hwnd) {
                    show_context_menu(hwnd);
                }
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == REFRESH_TIMER_ID => {
                begin_refresh(hwnd);
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == POSITION_TIMER_ID => {
                relocate_widget(hwnd);
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

            let config_path = default_config_path();
            let config = match AppConfig::load(&config_path) {
                Ok(config) => config,
                Err(error) => {
                    eprintln!(
                        "failed to load provider configuration: {}",
                        ai_usage_bar::redact_sensitive_text(&error.to_string())
                    );
                    return;
                }
            };
            let registry = match build_registry(&config) {
                Ok(registry) => registry,
                Err(error) => {
                    eprintln!(
                        "failed to build provider registry: {}",
                        ai_usage_bar::redact_sensitive_text(&error.to_string())
                    );
                    return;
                }
            };

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

            let known_providers = registry.registered_providers();
            let refresh_service = Arc::new(RefreshService::new(registry, RefreshPolicy::default()));
            let state_ptr = Box::into_raw(Box::new(AppState::loading(
                refresh_service,
                config,
                config_path,
                known_providers,
            )));
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
        fn canonical_weekly_aliases_cover_codex_and_grok_cards() {
            assert_eq!(
                canonical_window_key(&Provider::Codex, "primary"),
                Some("primary")
            );
            assert_eq!(
                canonical_window_key(&Provider::GrokConsumer, "primary"),
                Some("primary")
            );
            assert_eq!(
                canonical_window_key(&Provider::GrokApi, "weekly"),
                Some("primary")
            );
        }

        #[test]
        fn provider_reorder_uses_insert_at_index_without_losing_cards() {
            let mut order = vec![Provider::Codex, Provider::GrokApi, Provider::Kimi];
            // Drag Codex to flow index 1 (flow is [GrokApi, Kimi]): Codex lands
            // between GrokApi and Kimi.
            assert!(apply_drop(&mut order, &Provider::Codex, 1));
            assert_eq!(
                order,
                vec![Provider::GrokApi, Provider::Codex, Provider::Kimi]
            );
            // Appending at the flow length moves the dragged card last.
            let mut order = vec![Provider::Codex, Provider::GrokApi, Provider::Kimi];
            assert!(apply_drop(&mut order, &Provider::Codex, 2));
            assert_eq!(
                order,
                vec![Provider::GrokApi, Provider::Kimi, Provider::Codex]
            );
            // A no-op drop (drag to its own flow index) still returns true.
            let mut order = vec![Provider::Codex, Provider::GrokApi, Provider::Kimi];
            assert!(apply_drop(&mut order, &Provider::GrokApi, 1));
            assert_eq!(
                order,
                vec![Provider::Codex, Provider::GrokApi, Provider::Kimi]
            );
        }

        #[test]
        fn drag_resolution_maps_pointer_halves_and_hidden_slots() {
            let grid = SlotRect {
                left: 18,
                top: 82,
                right: 832,
                bottom: 396,
            };
            let cards = vec![
                DropCard {
                    id: Provider::Codex,
                    rect: SlotRect {
                        left: 18,
                        top: 82,
                        right: 418,
                        bottom: 232,
                    },
                    visible: true,
                },
                DropCard {
                    id: Provider::Kimi,
                    rect: SlotRect {
                        left: 432,
                        top: 82,
                        right: 832,
                        bottom: 232,
                    },
                    visible: false,
                },
            ];

            // Left half of Codex -> insert before it.
            assert_eq!(DropGrid::drop_index(&cards, grid, (100, 100)), Some(0));
            // Right half of Codex -> insert after it (before hidden Kimi).
            assert_eq!(DropGrid::drop_index(&cards, grid, (300, 100)), Some(1));
            // Hidden Kimi keeps its slot but is not a drop target.
            assert_eq!(DropGrid::drop_index(&cards, grid, (600, 100)), None);
            // The footer region below the row appends.
            assert_eq!(DropGrid::drop_index(&cards, grid, (100, 400)), Some(2));
        }

        #[test]
        fn reorder_action_accepts_only_full_permutations() {
            let available = vec![
                Provider::Codex,
                Provider::GrokApi,
                Provider::Kimi,
                Provider::OllamaCloud,
            ];
            let ok = vec![
                Provider::OllamaCloud,
                Provider::Codex,
                Provider::GrokApi,
                Provider::Kimi,
            ];
            assert!(is_full_permutation(&ok, &available));
            assert!(!is_full_permutation(&ok[..3], &available));
            let mut duplicated = ok.clone();
            duplicated[0] = Provider::Codex;
            assert!(!is_full_permutation(&duplicated, &available));
            let mut foreign = ok.clone();
            foreign[0] = Provider::GrokConsumer;
            assert!(!is_full_permutation(&foreign, &available));
        }

        #[test]
        fn clipboard_payload_uses_the_same_detail_renderer_as_the_menu_action() {
            assert_eq!(render_detail_text(&[]), "No provider data");
        }

        #[test]
        fn reset_dialog_accepts_dashboard_countdowns() {
            let now = Utc.timestamp_opt(1_786_000_000, 0).unwrap();
            assert_eq!(
                parse_reset_anchor("5 hours 0 minutes", now).unwrap(),
                Some(now + ChronoDuration::hours(5))
            );
            assert_eq!(
                parse_reset_anchor("Resets in 2 days 10 hours", now).unwrap(),
                Some(now + ChronoDuration::days(2) + ChronoDuration::hours(10))
            );
            assert_eq!(
                parse_reset_anchor("29 days 0 hours", now).unwrap(),
                Some(now + ChronoDuration::days(29))
            );
        }

        #[test]
        fn reset_dialog_displays_copyable_dashboard_countdowns() {
            let now = Utc.timestamp_opt(1_786_000_000, 0).unwrap();
            assert_eq!(
                format_reset_countdown(Some(now + ChronoDuration::hours(5)), now),
                "5 hours 0 minutes"
            );
            assert_eq!(
                format_reset_countdown(
                    Some(now + ChronoDuration::days(2) + ChronoDuration::hours(10)),
                    now,
                ),
                "2 days 10 hours"
            );
            assert_eq!(
                format_reset_countdown(Some(now + ChronoDuration::days(29)), now),
                "29 days 0 hours"
            );
        }

        // --- Mouse release routing -------------------------------------------

        fn test_card(provider: Provider, top: i32, left: i32) -> PanelCard {
            let rect = RECT {
                left,
                top,
                right: left + PANEL_CARD_WIDTH,
                bottom: top + PANEL_HEADER_HEIGHT + PANEL_ROW_HEIGHT + PANEL_CARD_PADDING * 2,
            };
            PanelCard {
                rect,
                title: provider_display_name(&provider).to_string(),
                visible: true,
                focused: false,
                eye_rect: RECT {
                    left: rect.right - 42,
                    top: rect.top + 14,
                    right: rect.right - 12,
                    bottom: rect.top + 42,
                },
                provider: provider.clone(),
                rows: vec![PanelRow {
                    rect: RECT {
                        left: rect.left + PANEL_CARD_PADDING,
                        top: rect.top + PANEL_HEADER_HEIGHT,
                        right: rect.right - PANEL_CARD_PADDING,
                        bottom: rect.top + PANEL_HEADER_HEIGHT + PANEL_ROW_HEIGHT,
                    },
                    label: "primary".to_string(),
                    value: "50% left".to_string(),
                    checked: true,
                    toggleable: true,
                    action: PanelAction::ToggleWindow(provider.clone(), "primary".to_string()),
                    focus_action: Some(PanelAction::FocusWindow(
                        provider.clone(),
                        "primary".to_string(),
                    )),
                    focused: false,
                }],
                placeholder: false,
            }
        }

        fn test_layout() -> PanelLayout {
            let mut cards = vec![
                test_card(Provider::Codex, 82, 18),
                test_card(Provider::GrokApi, 82, 432),
            ];
            reflow_grid(&mut cards, GRID_COLUMNS);
            let mut layout = PanelLayout {
                width: 0,
                height: 0,
                cards,
                buttons: vec![PanelButton {
                    rect: RECT::default(),
                    label: "Refresh".to_string(),
                    action: PanelAction::Refresh,
                }],
            };
            reflow_panel_layout(&mut layout);
            layout
        }

        fn test_state() -> PanelState {
            PanelState {
                parent: HWND::default(),
                layout: test_layout(),
                result: None,
                drag: None,
                rebuild_pending: false,
            }
        }

        #[test]
        fn release_without_drag_routes_checkbox_eye_and_row_clicks() {
            // Regression: the drag refactor routed releases only through an
            // existing header gesture (`state.drag.take()`). Clicks on the
            // checkbox, eye toggle, rows, or footer buttons never started a
            // gesture, so their releases were dropped and the checkmarks no
            // longer toggled provider/quota-window visibility. Every release
            // must reach the hit-tester, even with `drag == None`.
            let state = test_state();
            let card = state.layout.cards[0].clone();
            let row = &card.rows[0];

            // Checkbox hit area: toggles quota-window visibility.
            let checkbox = row_checkbox_rect(row.rect);
            let checkbox_center = POINT {
                x: (checkbox.left + checkbox.right) / 2,
                y: (checkbox.top + checkbox.bottom) / 2,
            };
            let (action, restore) = resolve_lbutton_release(&state, None, checkbox_center);
            assert_eq!(
                action,
                Some(PanelAction::ToggleWindow(
                    Provider::Codex,
                    "primary".to_string()
                ))
            );
            assert!(!restore, "plain clicks never restore the layout");

            // Row label area: focuses the window.
            let label_point = POINT {
                x: row.rect.left + 10,
                y: (row.rect.top + row.rect.bottom) / 2,
            };
            let (action, _) = resolve_lbutton_release(&state, None, label_point);
            assert_eq!(
                action,
                Some(PanelAction::FocusWindow(
                    Provider::Codex,
                    "primary".to_string()
                ))
            );

            // The explicit radio-style focus control must use the same action
            // as the row label, so choosing a timeframe is discoverable while
            // the square checkbox remains visibility-only.
            let focus_control = row_focus_rect(row.rect);
            let focus_point = POINT {
                x: (focus_control.left + focus_control.right) / 2,
                y: (focus_control.top + focus_control.bottom) / 2,
            };
            let (action, _) = resolve_lbutton_release(&state, None, focus_point);
            assert_eq!(
                action,
                Some(PanelAction::FocusWindow(
                    Provider::Codex,
                    "primary".to_string()
                ))
            );

            // Eye toggle: provider visibility.
            let eye = POINT {
                x: (card.eye_rect.left + card.eye_rect.right) / 2,
                y: (card.eye_rect.top + card.eye_rect.bottom) / 2,
            };
            let (action, _) = resolve_lbutton_release(&state, None, eye);
            assert_eq!(action, Some(PanelAction::ToggleProvider(Provider::Codex)));
        }

        #[test]
        fn release_with_inactive_header_gesture_focuses_provider() {
            let state = test_state();
            let card = state.layout.cards[0].clone();
            let header_point = POINT {
                x: card.rect.left + 10,
                y: (card.rect.top + (card.rect.top + PANEL_HEADER_HEIGHT)) / 2,
            };
            // A press on the header started a gesture; staying under the drag
            // threshold means the release is a header click (provider focus).
            let drag = PanelDragState {
                provider: Provider::Codex,
                press_point: header_point,
                pointer: header_point,
                active: false,
                origin_order: state
                    .layout
                    .cards
                    .iter()
                    .map(|card| card.provider.clone())
                    .collect(),
                origin_cards: state.layout.cards.clone(),
                flow: state
                    .layout
                    .cards
                    .iter()
                    .filter(|card| card.provider != Provider::Codex)
                    .cloned()
                    .collect(),
                drop_index: None,
                drop_target: None,
                grabbed_rect: card.rect,
            };
            let (action, restore) = resolve_lbutton_release(&state, Some(&drag), header_point);
            assert_eq!(action, Some(PanelAction::FocusProvider(Provider::Codex)));
            assert!(!restore, "a header click on the header is not a cancel");
        }

        #[test]
        fn active_drag_release_commits_reorder_not_a_click() {
            let state = test_state();
            let cards = state.layout.cards.clone();
            let origin_order: Vec<Provider> =
                cards.iter().map(|card| card.provider.clone()).collect();
            let flow: Vec<PanelCard> = cards
                .iter()
                .filter(|card| card.provider != Provider::Codex)
                .cloned()
                .collect();
            let drag = PanelDragState {
                provider: Provider::Codex,
                press_point: POINT {
                    x: cards[0].rect.left + 10,
                    y: cards[0].rect.top + 10,
                },
                pointer: POINT {
                    x: cards[1].rect.left + 10,
                    y: cards[1].rect.top + 10,
                },
                active: true,
                origin_order,
                origin_cards: cards.clone(),
                flow,
                drop_index: None,
                drop_target: None,
                grabbed_rect: cards[0].rect,
            };
            // Releasing over the other card swaps the two providers instead of
            // firing a click on the dragged card's slot.
            let release = POINT {
                x: cards[1].rect.left + 10,
                y: cards[1].rect.top + 10,
            };
            let (action, restore) = resolve_lbutton_release(&state, Some(&drag), release);
            assert!(
                matches!(
                    action,
                    Some(PanelAction::Reorder(order))
                        if order == vec![Provider::GrokApi, Provider::Codex]
                ),
                "dropping Codex on GrokApi must swap the two providers"
            );
            assert!(!restore, "a successful drop is not a cancel");
        }
    }
}

#[cfg(windows)]
fn main() {
    windows_shell::run();
}
