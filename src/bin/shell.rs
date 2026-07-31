use ai_usage_bar::{build_tray_view, fetch_codex_snapshots, ProviderCard, UsageSnapshot};
use std::sync::{Arc, Mutex};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIconBuilder, TrayIconEvent};

#[derive(Debug, Clone)]
enum AppEvent {
    Refresh,
    RefreshDone,
    Quit,
    ShowDetails,
}

fn render_detail_text(snapshots: &[UsageSnapshot]) -> String {
    let cards = ProviderCard::from_snapshots(snapshots);
    let mut lines = Vec::new();
    for card in cards {
        lines.push(format!("=== {} ({}) ===", card.provider, card.account_id));
        for m in &card.metrics {
            let used = m.used.as_deref().unwrap_or("?");
            let limit = m.limit.as_deref().unwrap_or("?");
            let unit = &m.unit;
            let resets = m.resets_at.as_deref().unwrap_or("?");
            lines.push(format!(
                "  [{}] {:?} {} — {}/{} {}, resets {}",
                m.label, m.metric_kind, m.window_kind, used, limit, unit, resets
            ));
            lines.push(format!("    observed: {}", m.observed_at));
            lines.push(format!("    source: {}, confidence: {}", m.source, m.confidence));
            if m.unlimited {
                lines.push("    unlimited: true".to_string());
            }
            if let Some(err) = &m.error {
                lines.push(format!("    error: {err}"));
            }
        }
        lines.push(String::new());
    }
    lines.join("\n")
}

fn build_icon(_emoji: &str, used_percent: Option<f64>) -> Option<tray_icon::Icon> {
    let rgba = render_progress_icon(_emoji, used_percent);
    tray_icon::Icon::from_rgba(rgba, 64, 64).ok()
}

fn render_progress_icon(_emoji: &str, used_percent: Option<f64>) -> Vec<u8> {
    let mut buf = vec![0u8; 64 * 64 * 4];
    let bg = [0x33, 0x33, 0x33, 0xff];
    let border = [0x88, 0x88, 0x88, 0xff];

    let pct = used_percent.unwrap_or(0.0).clamp(0.0, 100.0);
    let fill_color = if pct >= 90.0 {
        [0xf4, 0x43, 0x36, 0xff]
    } else if pct >= 70.0 {
        [0xff, 0xc1, 0x07, 0xff]
    } else {
        [0x4c, 0xaf, 0x50, 0xff]
    };

    let x0 = 8i32;
    let x1 = 56i32;
    let y0 = 8i32;
    let y1 = 56i32;
    let fill_w = ((x1 - x0) as f64 * pct / 100.0) as i32;
    let fill_x1 = (x0 + fill_w).min(x1);

    for y in 0..64i32 {
        for x in 0..64i32 {
            let idx = ((y * 64 + x) * 4) as usize;
            if x < x0 || x >= x1 || y < y0 || y >= y1 {
                continue;
            }
            let is_border = x == x0 || x == x1 - 1 || y == y0 || y == y1 - 1;
            if is_border {
                buf[idx..idx + 4].copy_from_slice(&border);
            } else if x < fill_x1 {
                buf[idx..idx + 4].copy_from_slice(&fill_color);
            } else {
                buf[idx..idx + 4].copy_from_slice(&bg);
            }
        }
    }
    buf
}

fn main() {
    let snapshots: Arc<Mutex<Vec<UsageSnapshot>>> = Arc::new(Mutex::new(Vec::new()));

    let mut event_loop_builder = EventLoopBuilder::<AppEvent>::with_user_event();
    let event_loop = event_loop_builder.build();
    let proxy = event_loop.create_proxy();

    let menu = Menu::new();
    let refresh_item = MenuItem::new("Refresh", true, None);
    let detail_item = MenuItem::new("Print details to console", true, None);
    let quit_item = MenuItem::new("Quit", true, None);
    menu.append(&refresh_item).unwrap();
    menu.append(&detail_item).unwrap();
    menu.append(&PredefinedMenuItem::separator()).unwrap();
    menu.append(&quit_item).unwrap();

    let refresh_id = refresh_item.id().clone();
    let detail_id = detail_item.id().clone();
    let quit_id = quit_item.id().clone();

    let tray = TrayIconBuilder::new()
        .with_tooltip("AI Usage Bar — loading...")
        .with_menu(Box::new(menu))
        .with_title("AI Usage Bar")
        .build()
        .expect("failed to build tray icon");

    if let Some(icon) = build_icon("\u{2014}", None) {
        let _ = tray.set_icon(Some(icon));
    }

    let proxy_for_tray = proxy.clone();
    TrayIconEvent::set_event_handler(Some(move |_| {
        if let Ok(TrayIconEvent::Click { button, .. }) = TrayIconEvent::receiver().try_recv() {
            if button == tray_icon::MouseButton::Left {
                let _ = proxy_for_tray.send_event(AppEvent::ShowDetails);
            }
        }
    }));

    let proxy_for_menu = proxy.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if event.id == refresh_id {
            let _ = proxy_for_menu.send_event(AppEvent::Refresh);
        } else if event.id == detail_id {
            let _ = proxy_for_menu.send_event(AppEvent::ShowDetails);
        } else if event.id == quit_id {
            let _ = proxy_for_menu.send_event(AppEvent::Quit);
        }
    }));

    let _ = proxy.send_event(AppEvent::Refresh);

    let snaps_arc = Arc::clone(&snapshots);
    let proxy_arc = event_loop.create_proxy();

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        if let tao::event::Event::UserEvent(ev) = event {
            match ev {
                AppEvent::Refresh => {
                    let snaps = Arc::clone(&snaps_arc);
                    let proxy = proxy_arc.clone();
                    std::thread::spawn(move || {
                        let result = fetch_codex_snapshots();
                        let mut snaps_guard = snaps.lock().unwrap();
                        match result {
                            Ok(s) => *snaps_guard = s,
                            Err(e) => {
                                eprintln!("refresh error: {e}");
                                *snaps_guard = Vec::new();
                            }
                        }
                        let _ = proxy.send_event(AppEvent::RefreshDone);
                    });
                }
                AppEvent::RefreshDone => {
                    let snaps = snaps_arc.lock().unwrap();
                    let vm = build_tray_view(&snaps);
                    let _ = tray.set_tooltip(Some(&vm.tooltip));
                    if let Some(icon) = build_icon(&vm.icon_text, vm.used_percent) {
                        let _ = tray.set_icon(Some(icon));
                    }
                }
                AppEvent::ShowDetails => {
                    let snaps = snaps_arc.lock().unwrap();
                    let detail = render_detail_text(&snaps);
                    println!("{detail}");
                }
                AppEvent::Quit => {
                    *control_flow = ControlFlow::Exit;
                }
            }
        }
    });
}