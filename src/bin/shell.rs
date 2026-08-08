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
        build_registry, build_tray_view_focused_window, default_config_path, provider_display_name,
        window_display_name, is_allowed_browser_url, AppConfig, KIMI_CONSOLE_URL,
        OLLAMA_USAGE_URL, OpenCodeResetSettings, Provider, RefreshPolicy, RefreshService,
        UsageSnapshot,
    };
    use chrono::{DateTime, Duration as ChronoDuration, NaiveDateTime, TimeZone, Utc};
    use chrono_tz::America::Toronto;
    use std::ffi::c_void;
    use std::path::PathBuf;
    use std::ptr::null_mut;
    use std::sync::{Arc, Once};
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
    /// Dynamic "Show <provider>" items: MENU_SHOW_PROVIDER_BASE + index.
    const MENU_SHOW_PROVIDER_BASE: usize = 1100;
    const MENU_SHOW_PROVIDER_MAX: usize = 8;
    const MENU_SHOW_WINDOW_BASE: usize = 1200;
    const MENU_SHOW_WINDOW_MAX: usize = 8;
    const STATUS_TIMER_ID: usize = 4;
    const STATUS_INTERVAL_MS: u32 = 2_500;
    const CF_UNICODETEXT_FORMAT: u32 = 13;

    const WM_APP_REFRESH_DONE: u32 = 0x8000 + 1;
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
    }

    struct RefreshPayload {
        result: std::result::Result<Vec<UsageSnapshot>, String>,
    }

    impl AppState {
        fn loading(
            refresh_service: Arc<RefreshService>,
            config: AppConfig,
            config_path: PathBuf,
        ) -> Self {
            Self {
                refresh_service,
                config,
                config_path,
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
                (text_w + TOOLTIP_CHROME_PAD_X).clamp(40, TOOLTIP_MAX_WIDTH_PX + TOOLTIP_CHROME_PAD_X),
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
    fn tooltip_origin_clear_of_pill(
        widget_rect: RECT,
        tip_w: i32,
        tip_h: i32,
    ) -> (i32, i32) {
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
        let current_view = build_tray_view_focused_window(
            &state.snapshots,
            state.focus_provider.as_ref(),
            state.focus_window.as_deref(),
            chrono::Utc::now(),
        );
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
                let view = build_tray_view_focused_window(
                    &snapshots,
                    state.focus_provider.as_ref(),
                    state.focus_window.as_deref(),
                    chrono::Utc::now(),
                );
                state.snapshots = snapshots;
                state.apply_view(view);
            }
            Err(error) => {
                let view = build_tray_view_focused_window(
                    &state.snapshots,
                    state.focus_provider.as_ref(),
                    state.focus_window.as_deref(),
                    chrono::Utc::now(),
                );
                let safe_error = ai_usage_bar::redact_sensitive_text(&error.to_string());
                state.tooltip = if state.snapshots.is_empty() {
                    format!("AI Usage Bar — refresh failed: {safe_error}")
                } else {
                    format!("{}\nRefresh failed: {safe_error}", view.tooltip)
                };
                eprintln!("refresh error: {safe_error}");
            }
        }

        update_tooltip(hwnd, state);
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }

    fn set_focus_provider(hwnd: HWND, provider: Provider) {
        let Some(state) = app_state(hwnd) else {
            return;
        };
        // Clear any transient status so the pill only shows the provider name.
        state.status = None;
        unsafe {
            let _ = KillTimer(Some(hwnd), STATUS_TIMER_ID);
        }
        state.focus_provider = Some(provider);
        state.focus_window = None;
        let view = build_tray_view_focused_window(
            &state.snapshots,
            state.focus_provider.as_ref(),
            state.focus_window.as_deref(),
            chrono::Utc::now(),
        );
        state.apply_view(view);
        update_tooltip(hwnd, state);
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }

    fn set_focus_window(hwnd: HWND, window: &str) {
        let Some(state) = app_state(hwnd) else {
            return;
        };
        state.status = None;
        unsafe {
            let _ = KillTimer(Some(hwnd), STATUS_TIMER_ID);
        }
        state.focus_window = Some(window.to_string());
        let view = build_tray_view_focused_window(
            &state.snapshots,
            state.focus_provider.as_ref(),
            state.focus_window.as_deref(),
            chrono::Utc::now(),
        );
        state.apply_view(view);
        update_tooltip(hwnd, state);
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
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
        let next = match current.as_ref().and_then(|c| {
            state
                .switchable_providers
                .iter()
                .position(|p| p == c)
        }) {
            Some(idx) => state.switchable_providers
                [(idx + 1) % state.switchable_providers.len()]
            .clone(),
            None => state.switchable_providers[0].clone(),
        };
        set_focus_provider(hwnd, next);
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
        let total_minutes = value
            .signed_duration_since(now)
            .num_minutes()
            .max(0);
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
                "Paste a countdown such as \"2 days 10 hours\" or \"5 hours 0 minutes\""
                    .to_string()
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
                        "Use days, hours, and minutes, for example \"2 days 10 hours\""
                            .to_string(),
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
            .ok_or_else(|| "That local time is ambiguous or invalid because of a daylight-saving transition".to_string())
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

    fn run_opencode_reset_dialog(parent: HWND, settings: OpenCodeResetSettings) -> Option<OpenCodeResetSettings> {
        static REGISTER_CLASS: Once = Once::new();
        unsafe {
            let hinst = GetModuleHandleW(None).ok().map(|module| HINSTANCE(module.0))?;
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
            let edit_style = WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32);
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
            state.refresh_service = Arc::new(RefreshService::new(registry, RefreshPolicy::default()));
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
        let Some(settings) = app_state_ref(hwnd).map(|state| state.config.opencode_reset_settings()) else {
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
            _ => None,
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

            // Keep wide strings alive until TrackPopupMenu returns.
            let mut provider_labels: Vec<Vec<u16>> = Vec::new();
            let mut window_labels: Vec<Vec<u16>> = Vec::new();
            let switchable = app_state_ref(hwnd)
                .map(|s| s.switchable_providers.clone())
                .unwrap_or_default();
            let focused = app_state_ref(hwnd).and_then(|s| s.focus_provider.clone());
            if !switchable.is_empty() {
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, w!(""));
                for (index, provider) in switchable
                    .iter()
                    .take(MENU_SHOW_PROVIDER_MAX)
                    .enumerate()
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

            match command {
                MENU_REFRESH => begin_refresh(hwnd),
                MENU_OPEN_OLLAMA_USAGE => open_ollama_usage_page(hwnd),
                MENU_OPEN_KIMI_CONSOLE => open_kimi_console(hwnd),
                MENU_CONFIG_OPENCODE_RESETS => open_opencode_reset_dialog(hwnd),
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
                id if (MENU_SHOW_WINDOW_BASE
                    ..MENU_SHOW_WINDOW_BASE + MENU_SHOW_WINDOW_MAX)
                    .contains(&id) =>
                {
                    if let Some(window) = available_windows.get(id - MENU_SHOW_WINDOW_BASE) {
                        set_focus_window(hwnd, window);
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
                paint_widget(
                    hwnd,
                    used_percent,
                    refreshing,
                    status.as_deref(),
                );
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
                show_context_menu(hwnd);
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

            let refresh_service = Arc::new(RefreshService::new(
                registry,
                RefreshPolicy::default(),
            ));
            let state_ptr = Box::into_raw(Box::new(AppState::loading(
                refresh_service,
                config,
                config_path,
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
    }
}

#[cfg(windows)]
fn main() {
    windows_shell::run();
}
