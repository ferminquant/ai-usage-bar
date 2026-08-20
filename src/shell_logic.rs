use ai_usage_bar::{format_reset_label, ProviderCard, UsageSnapshot};
use chrono::Utc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UsageBand {
    Neutral,
    Green,
    Yellow,
    Red,
}

pub(crate) fn normalize_used_percent(used_percent: Option<f64>) -> Option<f64> {
    used_percent
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0.0, 100.0))
}

pub(crate) fn usage_band(used_percent: Option<f64>) -> UsageBand {
    match normalize_used_percent(used_percent) {
        None => UsageBand::Neutral,
        Some(percent) if percent >= 90.0 => UsageBand::Red,
        Some(percent) if percent >= 70.0 => UsageBand::Yellow,
        Some(_) => UsageBand::Green,
    }
}

/// Columns in the row-major provider-card grid.
pub(crate) const GRID_COLUMNS: usize = 2;

/// Pointer movement (squared pixel distance) that turns a header press into a
/// drag. Presses that stay under this threshold are ordinary clicks, so a
/// simple header click still focuses the provider.
pub(crate) const DRAG_THRESHOLD_PX: i32 = 5;

/// How a mouse release in the provider-card panel is routed.
///
/// The release handler must dispatch a click even when the press never
/// started a header gesture: the eye toggle, quota-row checkboxes, row
/// labels, and footer buttons all sit outside the draggable header, so they
/// are handled purely on release. This classification is the guard for that
/// path — a release with no gesture is always a plain click, never a no-op.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReleaseRoute {
    /// No gesture was started at press: plain click on the eye, a checkbox,
    /// a row, or a footer button. Must hit-test and dispatch on release.
    Click,
    /// Header press stayed within the drag threshold: provider focus/click.
    HeaderClick,
    /// Header gesture crossed the threshold: commit a reorder.
    Reorder,
}

/// Classify a release from the gesture that (may have) started at press.
/// `drag_active` is `None` when `WM_LBUTTONDOWN` did not start a header
/// gesture (a click on a checkbox, the eye, or a footer button).
pub(crate) fn release_route(drag_active: Option<bool>) -> ReleaseRoute {
    match drag_active {
        None => ReleaseRoute::Click,
        Some(false) => ReleaseRoute::HeaderClick,
        Some(true) => ReleaseRoute::Reorder,
    }
}

/// Pure rectangle geometry used by the drop model. Kept separate from the
/// Win32 `RECT` so the model is unit-testable on every platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SlotRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl SlotRect {
    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    /// Horizontal midpoint that splits a slot into insert-before / insert-after
    /// halves for the drop model.
    pub fn mid_x(self) -> i32 {
        (self.left + self.right) / 2
    }
}

/// One card in the drag flow, with its live slot geometry.
#[derive(Debug, Clone)]
pub(crate) struct DropCard<Id> {
    pub id: Id,
    pub rect: SlotRect,
    /// Hidden cards keep their grid slot so the two-column shape never
    /// collapses during a drag, but their slots are not drop targets.
    pub visible: bool,
}

/// Deterministic row-major two-column drop model for the provider grid.
///
/// The dragged provider is removed from the flow first; the remaining cards
/// are laid out left-to-right, top-to-bottom in two columns. A pointer maps to
/// a slot by halves: the left half of a slot inserts before that slot's card,
/// the right half inserts after it. The empty trailing slot left by an
/// incomplete last row, and the footer region below the last row, both append.
/// Hidden cards keep their slots but are not targets. Preview and commit both
/// resolve through the drop-grid resolvers + [`apply_drop`], so the visual
/// destination and the persisted order always agree.
pub(crate) struct DropGrid;

impl DropGrid {
    pub fn drop_index<Id>(
        cards: &[DropCard<Id>],
        grid: SlotRect,
        pointer: (i32, i32),
    ) -> Option<usize> {
        let (x, y) = pointer;
        if cards.is_empty() {
            return (x >= grid.left && x < grid.right && y >= grid.top).then_some(0);
        }

        for (index, card) in cards.iter().enumerate() {
            if card.rect.contains(x, y) {
                if !card.visible {
                    return None;
                }
                return Some(if x < card.rect.mid_x() {
                    index
                } else {
                    index + 1
                });
            }
        }

        // Outside the two-column grid there is no target.
        if x < grid.left || x >= grid.right || y < grid.top {
            return None;
        }

        // Inside the grid but not over a card: the bottom row's band (which
        // holds the empty trailing slot when the last row is incomplete) and
        // everything below it appends to the end of the flow.
        let last = cards.last().expect("cards are non-empty");
        if y >= last.rect.top {
            return Some(cards.len());
        }
        None
    }

    /// Resolve a pointer against the currently painted flow plus its
    /// placeholder. The returned index is in flow space (the placeholder is
    /// removed), so preview and commit use the same coordinate system even
    /// after cards shift to another row or column.
    pub fn preview_drop_index<Id>(
        cards: &[DropCard<Id>],
        placeholder_index: usize,
        grid: SlotRect,
        pointer: (i32, i32),
    ) -> Option<usize> {
        let (x, y) = pointer;
        let Some(flow_len) = cards.len().checked_sub(1) else {
            return (x >= grid.left && x < grid.right && y >= grid.top).then_some(0);
        };
        let placeholder_index = placeholder_index.min(flow_len);

        for (index, card) in cards.iter().enumerate() {
            if !card.rect.contains(x, y) {
                continue;
            }
            if index == placeholder_index {
                return Some(placeholder_index);
            }
            if !card.visible {
                return None;
            }

            let preview_index = if x < card.rect.mid_x() {
                index
            } else {
                index + 1
            };
            let flow_index = if preview_index > placeholder_index {
                preview_index.saturating_sub(1)
            } else {
                preview_index
            };
            return Some(flow_index.min(flow_len));
        }

        if x < grid.left || x >= grid.right || y < grid.top {
            return None;
        }

        // Empty slot / footer: append after all flow cards.
        Some(flow_len)
    }
}

/// Apply a drop to a full provider order using deterministic insert-at-index
/// semantics: remove `dragged` first, then insert it at `index` (an index in
/// the resulting flow, clamped so appends work). This is the single commit
/// resolver shared with the drag preview, so the persisted order always
/// matches the visual drop slot.
pub(crate) fn apply_drop<Id: PartialEq + Clone>(
    order: &mut Vec<Id>,
    dragged: &Id,
    index: usize,
) -> bool {
    let Some(source) = order.iter().position(|candidate| candidate == dragged) else {
        return false;
    };
    order.remove(source);
    let index = index.min(order.len());
    order.insert(index, dragged.clone());
    true
}

/// Swap a dragged provider with the card directly under the pointer.
///
/// Card-on-card drops are intentionally swaps rather than insertions: the
/// target card stays in its slot while the grabbed card is carried over it,
/// so dropping Grok on Ollama cannot rotate the cards between those slots.
pub(crate) fn swap_drop<Id: PartialEq>(order: &mut [Id], dragged: &Id, target: &Id) -> bool {
    let Some(dragged_index) = order.iter().position(|candidate| candidate == dragged) else {
        return false;
    };
    let Some(target_index) = order.iter().position(|candidate| candidate == target) else {
        return false;
    };
    if dragged_index == target_index {
        return false;
    }
    order.swap(dragged_index, target_index);
    true
}

pub(crate) fn render_detail_text(snapshots: &[UsageSnapshot]) -> String {
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
            let resets = format_reset_label(metric.resets_at.as_deref(), Utc::now());
            let value = if metric.unit == "percent" {
                format!(
                    "{}% left ({}% used)",
                    metric.remaining.as_deref().unwrap_or("?"),
                    metric.used.as_deref().unwrap_or("?")
                )
            } else {
                format!("{}{}", metric.used.as_deref().unwrap_or("?"), unit_display)
            };
            lines.push(format!(
                "  [{}] {:?} {} — {}, resets {}",
                metric.label, metric.metric_kind, metric.window_kind, value, resets
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

#[cfg(test)]
mod tests {
    use super::*;
    use ai_usage_bar::{
        Confidence, ErrorCode, Freshness, MetricKind, Provider, Source, UsageSnapshot, WindowKind,
    };
    use chrono::Utc;

    fn make_snapshot(
        used: Option<f64>,
        remaining: Option<f64>,
        metric_kind: MetricKind,
        unit: &str,
    ) -> UsageSnapshot {
        UsageSnapshot {
            provider: Provider::Codex,
            account_id: "codex-test".into(),
            metric_kind,
            window_kind: WindowKind::Weekly,
            unit: unit.into(),
            observed_at: Utc::now(),
            source: Source::Cli,
            freshness: Freshness::Live,
            confidence: Confidence::Exact,
            used,
            remaining,
            limit: Some(100.0),
            unlimited: false,
            resets_at: None,
            window_label: Some("primary".into()),
            error: None,
        }
    }

    #[test]
    fn normalizes_percentages_and_rejects_non_finite_values() {
        assert_eq!(normalize_used_percent(Some(-5.0)), Some(0.0));
        assert_eq!(normalize_used_percent(Some(42.5)), Some(42.5));
        assert_eq!(normalize_used_percent(Some(105.0)), Some(100.0));
        assert_eq!(normalize_used_percent(Some(f64::NAN)), None);
        assert_eq!(normalize_used_percent(Some(f64::INFINITY)), None);
        assert_eq!(normalize_used_percent(None), None);
    }

    #[test]
    fn selects_usage_bands_at_widget_thresholds() {
        assert_eq!(usage_band(None), UsageBand::Neutral);
        assert_eq!(usage_band(Some(69.9)), UsageBand::Green);
        assert_eq!(usage_band(Some(70.0)), UsageBand::Yellow);
        assert_eq!(usage_band(Some(89.9)), UsageBand::Yellow);
        assert_eq!(usage_band(Some(90.0)), UsageBand::Red);
        assert_eq!(usage_band(Some(150.0)), UsageBand::Red);
    }

    #[test]
    fn detail_payload_reports_remaining_and_used_percentages() {
        let text = render_detail_text(&[make_snapshot(
            Some(17.0),
            Some(83.0),
            MetricKind::Quota,
            "percent",
        )]);

        assert!(text.contains("83% left (17% used)"));
        assert!(!text.contains("17% left"));
        assert!(text.contains("source: cli, confidence: exact"));
    }

    #[test]
    fn detail_payload_keeps_credits_unlimited_and_error_metadata() {
        let mut snapshot = make_snapshot(Some(12.0), None, MetricKind::Credits, "USD");
        snapshot.unlimited = true;
        snapshot.error = Some(ai_usage_bar::AdapterError {
            code: ErrorCode::Timeout,
            message: Some("provider did not respond".into()),
        });

        let text = render_detail_text(&[snapshot]);

        assert!(text.contains("12USD"));
        assert!(text.contains("unlimited: true"));
        assert!(text.contains("error: timeout: provider did not respond"));
    }

    #[test]
    fn empty_detail_payload_is_explicit() {
        assert_eq!(render_detail_text(&[]), "No provider data");
    }

    // --- Drop-grid model ----------------------------------------------------

    fn flow_card(
        id: &'static str,
        left: i32,
        top: i32,
        width: i32,
        height: i32,
    ) -> DropCard<&'static str> {
        DropCard {
            id,
            rect: SlotRect {
                left,
                top,
                right: left + width,
                bottom: top + height,
            },
            visible: true,
        }
    }

    fn hidden_card(
        id: &'static str,
        left: i32,
        top: i32,
        width: i32,
        height: i32,
    ) -> DropCard<&'static str> {
        let mut card = flow_card(id, left, top, width, height);
        card.visible = false;
        card
    }

    /// Two-column grid geometry matching the panel constants: 400px cards,
    /// 14px gap, 18px margins, rows at 82 and 246, 150px tall cards.
    fn grid() -> SlotRect {
        SlotRect {
            left: 18,
            top: 82,
            right: 832,
            bottom: 396,
        }
    }

    fn two_row_flow() -> Vec<DropCard<&'static str>> {
        vec![
            flow_card("A", 18, 82, 400, 150),
            flow_card("B", 432, 82, 400, 150),
            flow_card("C", 18, 246, 400, 150),
        ]
    }

    #[test]
    fn empty_flow_drops_at_index_zero() {
        let cards: Vec<DropCard<&'static str>> = Vec::new();
        assert_eq!(DropGrid::drop_index(&cards, grid(), (100, 100)), Some(0));
        assert_eq!(DropGrid::drop_index(&cards, grid(), (900, 100)), None);
        assert_eq!(DropGrid::drop_index(&cards, grid(), (100, 50)), None);
    }

    #[test]
    fn pointer_maps_to_slot_halves_row_major() {
        let cards = two_row_flow();
        // A: left half -> before A; right half -> after A.
        assert_eq!(DropGrid::drop_index(&cards, grid(), (100, 100)), Some(0));
        assert_eq!(DropGrid::drop_index(&cards, grid(), (300, 100)), Some(1));
        // B: left half -> after A / before B; right half -> after B.
        assert_eq!(DropGrid::drop_index(&cards, grid(), (600, 100)), Some(1));
        assert_eq!(DropGrid::drop_index(&cards, grid(), (800, 100)), Some(2));
        // C: left half -> index 2; right half -> after C.
        assert_eq!(DropGrid::drop_index(&cards, grid(), (100, 300)), Some(2));
        assert_eq!(DropGrid::drop_index(&cards, grid(), (300, 300)), Some(3));
    }

    #[test]
    fn empty_trailing_slot_and_footer_append() {
        let cards = two_row_flow();
        // Empty trailing slot beside C (same row band, right column).
        assert_eq!(DropGrid::drop_index(&cards, grid(), (600, 300)), Some(3));
        // Footer region below the last row.
        assert_eq!(DropGrid::drop_index(&cards, grid(), (100, 400)), Some(3));
        assert_eq!(DropGrid::drop_index(&cards, grid(), (700, 500)), Some(3));
    }

    #[test]
    fn single_card_flow_keeps_an_empty_slot_for_append() {
        let cards = vec![flow_card("A", 18, 82, 400, 150)];
        // Left half -> before A; right half (empty slot) and below -> append.
        assert_eq!(DropGrid::drop_index(&cards, grid(), (100, 100)), Some(0));
        assert_eq!(DropGrid::drop_index(&cards, grid(), (300, 100)), Some(1));
        assert_eq!(DropGrid::drop_index(&cards, grid(), (600, 100)), Some(1));
        assert_eq!(DropGrid::drop_index(&cards, grid(), (100, 300)), Some(1));
    }

    #[test]
    fn hidden_slots_keep_geometry_but_are_not_targets() {
        let cards = vec![
            flow_card("A", 18, 82, 400, 150),
            hidden_card("B", 432, 82, 400, 150),
            flow_card("C", 18, 246, 400, 150),
        ];
        // B occupies its slot: pointers over it resolve to no target, while
        // A/C still resolve with B's slot preserved in the row-major indices.
        assert_eq!(DropGrid::drop_index(&cards, grid(), (600, 100)), None);
        assert_eq!(DropGrid::drop_index(&cards, grid(), (300, 100)), Some(1));
        assert_eq!(DropGrid::drop_index(&cards, grid(), (100, 300)), Some(2));
        // The trailing region still appends behind B's occupied slot.
        assert_eq!(DropGrid::drop_index(&cards, grid(), (100, 400)), Some(3));
    }

    #[test]
    fn outside_the_grid_is_not_a_target() {
        let cards = two_row_flow();
        // Above the first row / left of the grid / right of the grid.
        assert_eq!(DropGrid::drop_index(&cards, grid(), (100, 50)), None);
        assert_eq!(DropGrid::drop_index(&cards, grid(), (5, 200)), None);
        assert_eq!(DropGrid::drop_index(&cards, grid(), (900, 200)), None);
        // Inter-row gap between A/B and C is not a target either.
        assert_eq!(DropGrid::drop_index(&cards, grid(), (200, 232)), None);
    }

    #[test]
    fn apply_drop_uses_remove_then_insert_semantics() {
        let mut order = vec!["A", "B", "C", "D"];
        // Drop A at flow index 2 (flow is [B, C, D]): result [B, C, A, D].
        assert!(apply_drop(&mut order, &"A", 2));
        assert_eq!(order, vec!["B", "C", "A", "D"]);

        // Append: index 3 in flow space puts the dragged card last.
        let mut order = vec!["A", "B", "C", "D"];
        assert!(apply_drop(&mut order, &"A", 3));
        assert_eq!(order, vec!["B", "C", "D", "A"]);

        // Move the last card to the front.
        let mut order = vec!["A", "B", "C", "D"];
        assert!(apply_drop(&mut order, &"D", 0));
        assert_eq!(order, vec!["D", "A", "B", "C"]);

        // A missing dragged card is a no-op.
        let mut order = vec!["A", "B"];
        assert!(!apply_drop(&mut order, &"X", 0));
        assert_eq!(order, vec!["A", "B"]);

        // Over-large indices clamp to append.
        let mut order = vec!["A", "B"];
        assert!(apply_drop(&mut order, &"B", 99));
        assert_eq!(order, vec!["A", "B"]);

        // A single card drag is a no-op but still succeeds.
        let mut order = vec!["A"];
        assert!(apply_drop(&mut order, &"A", 0));
        assert_eq!(order, vec!["A"]);
    }

    #[test]
    fn card_drop_swaps_only_the_target_slot() {
        let mut order = vec!["Codex", "OpenCode", "Kimi", "Grok", "Ollama"];
        assert!(swap_drop(&mut order, &"Grok", &"Ollama"));
        assert_eq!(order, vec!["Codex", "OpenCode", "Kimi", "Ollama", "Grok"]);

        let mut order = vec!["Codex", "OpenCode", "Kimi", "Grok", "Ollama"];
        assert!(swap_drop(&mut order, &"Grok", &"Codex"));
        assert_eq!(order, vec!["Grok", "OpenCode", "Kimi", "Codex", "Ollama"]);
    }

    #[test]
    fn preview_and_commit_resolve_the_same_drop_index() {
        // The shell resolves the drop against the flow (dragged removed) and
        // commits with apply_drop on the same index. Both must agree with the
        // preview order built by inserting the dragged card at that index.
        let origin = vec!["A", "B", "C", "D"];
        let dragged = "A";
        let flow: Vec<DropCard<&str>> = vec![
            flow_card("B", 18, 82, 400, 150),
            flow_card("C", 432, 82, 400, 150),
            flow_card("D", 18, 246, 400, 150),
        ];
        // Pointer over B's right half -> insert after B.
        let index = DropGrid::drop_index(&flow, grid(), (300, 100)).unwrap();
        let mut committed = origin.clone();
        assert!(apply_drop(&mut committed, &dragged, index));
        assert_eq!(committed, vec!["B", "A", "C", "D"]);

        // Pointer in the trailing slot -> append.
        let index = DropGrid::drop_index(&flow, grid(), (600, 300)).unwrap();
        let mut committed = origin.clone();
        assert!(apply_drop(&mut committed, &dragged, index));
        assert_eq!(committed, vec!["B", "C", "D", "A"]);
    }

    #[test]
    fn preview_drop_tracks_cards_after_placeholder_shifts_columns() {
        // Placeholder at slot 0 paints B/C on the top row and C/D on the
        // bottom row. A pointer over C's right half inserts after C in flow
        // space, rather than being mistaken for the empty trailing slot from
        // the pre-preview geometry.
        let cards = vec![
            hidden_card("placeholder", 18, 82, 400, 150),
            flow_card("B", 432, 82, 400, 150),
            flow_card("C", 18, 246, 400, 150),
            flow_card("D", 432, 246, 400, 150),
        ];
        assert_eq!(
            DropGrid::preview_drop_index(&cards, 0, grid(), (300, 300)),
            Some(2)
        );

        // Once the placeholder moves to flow index 2, the same pointer is on
        // the placeholder and remains stable instead of oscillating.
        let cards = vec![
            flow_card("B", 18, 82, 400, 150),
            flow_card("C", 432, 82, 400, 150),
            hidden_card("placeholder", 18, 246, 400, 150),
            flow_card("D", 432, 246, 400, 150),
        ];
        assert_eq!(
            DropGrid::preview_drop_index(&cards, 2, grid(), (300, 300)),
            Some(2)
        );
    }

    #[test]
    fn drag_threshold_is_a_small_squared_distance() {
        // Shadow the constant so the assertions stay meaningful at runtime.
        let threshold = DRAG_THRESHOLD_PX;
        assert_eq!(threshold * threshold, 25);
        assert!(threshold <= 6, "clicks must survive small jitter");
    }

    #[test]
    fn release_with_no_gesture_is_a_plain_click() {
        // Regression guard: a press on a checkbox, the eye toggle, a row, or
        // a footer button never starts a header gesture. The release handler
        // must still route it as a click; dropping this route is what made
        // the panel checkmarks stop toggling visibility.
        assert_eq!(release_route(None), ReleaseRoute::Click);
        assert_eq!(release_route(Some(false)), ReleaseRoute::HeaderClick);
        assert_eq!(release_route(Some(true)), ReleaseRoute::Reorder);
    }
}
