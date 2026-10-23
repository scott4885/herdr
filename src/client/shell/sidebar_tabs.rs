// This file contains code ported and modified from wkentaro/herdr commit 29484fc4
// (Kentaro Wada), licensed under the Apache License, Version 2.0 (section 4(b)):
// the tab rows are restricted to workspaces with two or more tabs and moved out of
// sidebar.rs into this fork-only module.

//! Sidebar tab rows: one clickable row per tab under a workspace card with 2+ tabs.

use super::render::{put_segment, put_text};
use super::*;

/// Only workspaces with 2 or more tabs appear in the map (D3).
pub(in crate::client::shell) fn group_workspace_tabs(
    snapshot: &ClientShellSnapshot,
) -> HashMap<&str, Vec<&ClientShellTab>> {
    let mut tabs = HashMap::<&str, Vec<&ClientShellTab>>::new();
    for tab in &snapshot.tabs {
        tabs.entry(&tab.workspace_id).or_default().push(tab);
    }
    tabs.retain(|_, tabs| tabs.len() >= 2);
    tabs
}

/// `group_workspace_tabs` of each endpoint's snapshot, in endpoint order.
pub(in crate::client::shell) fn group_endpoint_workspace_tabs(
    endpoints: &[ClientShellEndpoint],
) -> Vec<HashMap<&str, Vec<&ClientShellTab>>> {
    endpoints
        .iter()
        .map(|endpoint| {
            endpoint
                .snapshot
                .as_deref()
                .map(group_workspace_tabs)
                .unwrap_or_default()
        })
        .collect()
}

/// The tab rows of one workspace; empty when it has fewer than 2 tabs.
pub(in crate::client::shell) fn tabs_for<'a>(
    workspace_tabs: &'a HashMap<&str, Vec<&'a ClientShellTab>>,
    workspace_id: &str,
) -> &'a [&'a ClientShellTab] {
    workspace_tabs
        .get(workspace_id)
        .map_or(&[][..], Vec::as_slice)
}

/// (label rows, whole card) heights of a workspace card, the card capped at `max`.
pub(in crate::client::shell) fn card_heights(
    label_rows: usize,
    tab_rows: usize,
    max: u16,
) -> (u16, u16) {
    let label = label_rows.max(1).min(u16::MAX as usize) as u16;
    let card = label
        .saturating_add(tab_rows.min(u16::MAX as usize) as u16)
        .min(max);
    (label.min(card), card)
}

/// The rows below a card's label rows, where its tab rows go.
pub(in crate::client::shell) fn tab_rows_area(label: Rect, card_height: u16) -> Rect {
    Rect::new(
        label.x,
        label.bottom(),
        label.width,
        card_height.saturating_sub(label.height),
    )
}

/// The whole card of a workspace hit, tab rows included.
pub(in crate::client::shell) fn card_rect(hit: &WorkspaceHit) -> Rect {
    Rect::new(
        hit.rect.x,
        hit.rect.y,
        hit.rect.width,
        hit.card_bottom.saturating_sub(hit.rect.y),
    )
}

impl ClientShellState {
    /// Focuses the tab whose sidebar row is at `point`. A tab row press never starts a drag.
    pub(in crate::client::shell) fn focus_sidebar_tab_at(
        &mut self,
        point: (u16, u16),
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some((_, endpoint_id, tab_id)) = self
            .hits
            .sidebar_tabs
            .iter()
            .find(|(rect, _, _)| super::contains(*rect, point))
            .cloned()
        else {
            return false;
        };
        self.focus_or_activate(endpoint_id, ClientEndpointFocusTarget::Tab(tab_id), outcome);
        true
    }
}

pub(in crate::client::shell) fn render_workspace_tabs(
    buffer: &mut Buffer,
    area: Rect,
    tabs: &[&ClientShellTab],
    entry: &WorkspaceEntry,
    endpoint_id: &ClientEndpointId,
    endpoint_active: bool,
    config: &ClientShellConfig,
    hits: &mut ShellHitMap,
) {
    let palette = &config.palette;
    for (index, tab) in tabs.iter().take(area.height as usize).enumerate() {
        let rect = Rect::new(area.x, area.y + index as u16, area.width, 1);
        let active = endpoint_active && tab.focused;
        if active {
            buffer.set_style(rect, Style::default().bg(palette.active_row_bg));
        }
        let prefix = if entry.indented {
            if entry.last_child {
                "      "
            } else {
                "   │  "
            }
        } else {
            "   "
        };
        let connector = if index + 1 == tabs.len() {
            "└─ "
        } else {
            "├─ "
        };
        let mut x = put_segment(
            buffer,
            rect.x,
            rect.y,
            rect.right(),
            prefix,
            Style::default().fg(palette.overlay0),
        );
        x = put_segment(
            buffer,
            x,
            rect.y,
            rect.right(),
            connector,
            Style::default().fg(palette.overlay0),
        );
        x = put_segment(
            buffer,
            x,
            rect.y,
            rect.right(),
            status_icon(tab.agent_status, config.status_indicators),
            Style::default().fg(status_color(tab.agent_status, palette)),
        );
        x = x.saturating_add(1);
        put_text(
            buffer,
            x,
            rect.y,
            rect.right().saturating_sub(x),
            &tab.label,
            if active {
                Style::default()
                    .fg(palette.text)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(palette.overlay0)
            },
        );
        hits.sidebar_tabs
            .push((rect, endpoint_id.clone(), tab.tab_id.clone()));
    }
}
