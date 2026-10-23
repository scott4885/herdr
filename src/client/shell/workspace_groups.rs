//! Fork-only: workspace groups in the client sidebar (rendering half).
//!
//! The server projects groups and effective colors into `ClientShellSnapshot.workspace_groups`.
//! This module turns that projection into grouped sidebar entries, group header rows, color
//! bars and drop slots. Everything here is pure: renderers only draw into the buffer and record
//! hits, and a snapshot without groups leaves the upstream code paths untouched.

use super::*;
use crate::api::schema::{AgentStatus, Method, WorkspaceGroupEditParams, WorkspaceGroupOp};
use crate::protocol::{normalize_workspace_color, ClientShellWorkspaceGroup, WORKSPACE_COLORS};
use ratatui::style::Color;

pub(super) const GROUP_COLLAPSE_PREFIX: &str = "group:";

const UNGROUPED_HEADER_TEXT: &str = "ungrouped";
const BAR_GLYPH: &str = "▌";
const EXPANDED_GLYPH: &str = "▾";
const COLLAPSED_GLYPH: &str = "▸";

/// Host background assumed for a dark appearance when the host did not report one.
const DARK_HOST_BACKGROUND: [u8; 3] = [0x1e, 0x1e, 0x2e];
/// Host background assumed for a light or unknown appearance.
const LIGHT_HOST_BACKGROUND: [u8; 3] = [0xff, 0xff, 0xff];
const TAB_FILL_TEXT: Color = Color::Rgb(0xff, 0xff, 0xff);
const BAR_MIN_CONTRAST: f64 = 3.0;
const BAR_BLEND_STEP: f64 = 0.15;
const BAR_MAX_BLEND_STEPS: usize = 8;

pub(super) fn group_collapse_key(group_id: &str) -> String {
    format!("{GROUP_COLLAPSE_PREFIX}{group_id}")
}

/// Region of every workspace, by snapshot index: the position of its group in
/// `workspace_groups.groups`, or `groups.len()` for the ungrouped region.
fn workspace_regions(snapshot: &ClientShellSnapshot) -> Vec<usize> {
    let groups = &snapshot.workspace_groups.groups;
    let mut membership = HashMap::<&str, usize>::new();
    for (region, group) in groups.iter().enumerate() {
        for workspace_id in &group.workspace_ids {
            membership.entry(workspace_id.as_str()).or_insert(region);
        }
    }
    snapshot
        .workspaces
        .iter()
        .map(|workspace| {
            membership
                .get(workspace.workspace_id.as_str())
                .copied()
                .unwrap_or(groups.len())
        })
        .collect()
}

/// None when the snapshot has no groups (upstream `workspace_entries` then runs unchanged).
///
/// With groups, workspaces are bucketed by the snapshot's membership (groups in snapshot
/// order, then ungrouped, Vec order preserved inside each region), and upstream's worktree
/// family rule runs inside each region. A collapsed `group:<id>` hides every member except
/// the focused workspace, which is emitted un-indented.
pub(super) fn grouped_entries(
    snapshot: &ClientShellSnapshot,
    collapsed_groups: &HashSet<String>,
) -> Option<Vec<WorkspaceEntry>> {
    let groups = &snapshot.workspace_groups.groups;
    if groups.is_empty() {
        return None;
    }
    let mut buckets = vec![Vec::new(); groups.len() + 1];
    for (index, region) in workspace_regions(snapshot).into_iter().enumerate() {
        buckets[region].push(index);
    }
    let mut entries = Vec::with_capacity(snapshot.workspaces.len());
    for (region, members) in buckets.iter().enumerate() {
        let collapsed = groups
            .get(region)
            .is_some_and(|group| collapsed_groups.contains(&group_collapse_key(&group.group_id)));
        if collapsed {
            if let Some(focused) = members
                .iter()
                .copied()
                .find(|index| snapshot.workspaces[*index].focused)
            {
                entries.push(WorkspaceEntry {
                    index: focused,
                    indented: false,
                    last_child: false,
                });
            }
            continue;
        }
        push_region_entries(snapshot, members, collapsed_groups, &mut entries);
    }
    Some(entries)
}

/// Upstream's `workspace_entries` family rule, scoped to the workspaces of one region.
fn push_region_entries(
    snapshot: &ClientShellSnapshot,
    members: &[usize],
    collapsed_groups: &HashSet<String>,
    entries: &mut Vec<WorkspaceEntry>,
) {
    let workspaces = &snapshot.workspaces;
    let is_linked = |index: usize| {
        workspaces[index]
            .worktree
            .as_ref()
            .is_some_and(|worktree| worktree.is_linked_worktree)
    };
    let mut keyed = HashMap::<&str, Vec<usize>>::new();
    for &index in members {
        if let Some(worktree) = &workspaces[index].worktree {
            keyed.entry(&worktree.key).or_default().push(index);
        }
    }
    let families = keyed
        .iter()
        .filter(|(_, indices)| {
            indices.iter().any(|index| is_linked(*index))
                && indices.iter().any(|index| !is_linked(*index))
        })
        .map(|(key, _)| *key)
        .collect::<HashSet<_>>();
    let mut emitted = HashSet::<&str>::new();
    for &index in members {
        let Some(worktree) = workspaces[index]
            .worktree
            .as_ref()
            .filter(|worktree| families.contains(worktree.key.as_str()))
        else {
            entries.push(WorkspaceEntry {
                index,
                indented: false,
                last_child: false,
            });
            continue;
        };
        if !emitted.insert(&worktree.key) {
            continue;
        }
        let Some(family) = keyed.get(worktree.key.as_str()) else {
            continue;
        };
        for parent in family.iter().copied().filter(|member| !is_linked(*member)) {
            entries.push(WorkspaceEntry {
                index: parent,
                indented: false,
                last_child: false,
            });
        }
        if collapsed_groups.contains(&worktree.key) {
            if let Some(active) = family
                .iter()
                .copied()
                .find(|member| workspaces[*member].focused && is_linked(*member))
            {
                entries.push(WorkspaceEntry {
                    index: active,
                    indented: true,
                    last_child: true,
                });
            }
            continue;
        }
        let children = family
            .iter()
            .copied()
            .filter(|member| is_linked(*member))
            .collect::<Vec<_>>();
        for (child_index, child) in children.iter().enumerate() {
            entries.push(WorkspaceEntry {
                index: *child,
                indented: true,
                last_child: child_index + 1 == children.len(),
            });
        }
    }
}

/// RGB of a palette color name; an unknown name has no color (and draws no bar).
pub(super) fn color_rgb(name: &str) -> Option<[u8; 3]> {
    let name = normalize_workspace_color(name)?;
    WORKSPACE_COLORS
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, rgb)| *rgb)
}

/// Effective color of one workspace (its override, else its group color), as projected.
pub(super) fn workspace_color_rgb(
    snapshot: &ClientShellSnapshot,
    workspace_id: &str,
) -> Option<[u8; 3]> {
    snapshot
        .workspace_groups
        .workspace_colors
        .iter()
        .find(|(candidate, _)| candidate == workspace_id)
        .and_then(|(_, name)| color_rgb(name))
}

/// (fg, bg) of the focused tab: the focused workspace's raw color with white text (at least
/// 4.92:1 for every palette color), or upstream's accent fill without a color.
pub(super) fn active_tab_colors(
    snapshot: &ClientShellSnapshot,
    palette: &Palette,
) -> (Color, Color) {
    match snapshot
        .focused_workspace_id
        .as_deref()
        .and_then(|workspace_id| workspace_color_rgb(snapshot, workspace_id))
    {
        Some([red, green, blue]) => (TAB_FILL_TEXT, Color::Rgb(red, green, blue)),
        None => (super::panel_contrast_fg(palette), palette.accent),
    }
}

fn channel_luminance(channel: u8) -> f64 {
    let value = f64::from(channel) / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// WCAG 2.x relative luminance.
fn relative_luminance(rgb: [u8; 3]) -> f64 {
    0.2126 * channel_luminance(rgb[0])
        + 0.7152 * channel_luminance(rgb[1])
        + 0.0722 * channel_luminance(rgb[2])
}

/// WCAG 2.x contrast ratio.
fn contrast_ratio(first: [u8; 3], second: [u8; 3]) -> f64 {
    let (first, second) = (relative_luminance(first), relative_luminance(second));
    let (lighter, darker) = if first >= second {
        (first, second)
    } else {
        (second, first)
    };
    (lighter + 0.05) / (darker + 0.05)
}

fn blend(rgb: [u8; 3], target: [u8; 3], amount: f64) -> [u8; 3] {
    let mix = |from: u8, to: u8| {
        let value = f64::from(from) + (f64::from(to) - f64::from(from)) * amount;
        value.round().clamp(0.0, 255.0) as u8
    };
    [
        mix(rgb[0], target[0]),
        mix(rgb[1], target[1]),
        mix(rgb[2], target[2]),
    ]
}

/// A bar color with at least 3:1 contrast against the cell it is drawn on.
///
/// The background is the cell's real color; any non-RGB cell background (every built-in
/// theme's `Color::Reset` sidebar, or a named terminal color) uses the host background.
/// The color is blended 15% per step toward black when the background is lighter than the
/// color and toward white when it is darker, until it reaches 3:1, at most 8 steps.
pub(super) fn readable(rgb: [u8; 3], cell_bg: Color, host_bg: [u8; 3]) -> Color {
    let background = match cell_bg {
        Color::Rgb(red, green, blue) => [red, green, blue],
        _ => host_bg,
    };
    let target = if relative_luminance(rgb) <= relative_luminance(background) {
        [0, 0, 0]
    } else {
        [0xff, 0xff, 0xff]
    };
    let mut color = rgb;
    for _ in 0..BAR_MAX_BLEND_STEPS {
        if contrast_ratio(color, background) >= BAR_MIN_CONTRAST {
            break;
        }
        color = blend(color, target, BAR_BLEND_STEP);
    }
    Color::Rgb(color[0], color[1], color[2])
}

/// The host background behind `Color::Reset` cells: the reported OSC 11 background, else the
/// host appearance (dark => `#1e1e2e`, light or unknown => `#ffffff`).
pub(super) fn host_background_rgb(
    reported: Option<crate::terminal_theme::RgbColor>,
    appearance: Option<crate::terminal_theme::HostAppearance>,
) -> [u8; 3] {
    match (reported, appearance) {
        (Some(color), _) => [color.r, color.g, color.b],
        (None, Some(crate::terminal_theme::HostAppearance::Dark)) => DARK_HOST_BACKGROUND,
        (None, _) => LIGHT_HOST_BACKGROUND,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WorkspaceGroupHit {
    pub(super) rect: Rect,
    pub(super) endpoint_id: ClientEndpointId,
    pub(super) group_id: Option<String>,
    pub(super) collapsed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WorkspaceDropSlot {
    pub(super) endpoint_id: ClientEndpointId,
    pub(super) row: u16,
    pub(super) group_id: Option<String>,
    pub(super) before_workspace_id: Option<String>,
}

/// Shared inputs for drawing the headers, bars and slots of one sidebar section.
pub(super) struct GroupPaint<'a> {
    snapshot: &'a ClientShellSnapshot,
    palette: &'a Palette,
    indicators: crate::config::StatusIndicatorStyle,
    host_bg: [u8; 3],
    endpoint_id: ClientEndpointId,
    /// Offline endpoint: header rows are dimmed like its workspace rows.
    stale: bool,
    /// Blank rows above every top-level card (`ui.sidebar.spaces.row_gap`).
    row_gap: u16,
    /// Another machine's row follows the section's last entry directly (multi-machine sidebar).
    next_machine_below: bool,
    /// Row of a workspace drag's drop target in this section; a header there is the target
    /// and is drawn on `surface1` (spec 8.2).
    drop_target_row: Option<u16>,
}

impl<'a> GroupPaint<'a> {
    pub(super) fn new(
        snapshot: &'a ClientShellSnapshot,
        config: &'a ClientShellConfig,
        host_bg: [u8; 3],
        endpoint_id: &ClientEndpointId,
        stale: bool,
    ) -> Self {
        Self {
            snapshot,
            palette: &config.palette,
            indicators: config.status_indicators,
            host_bg,
            endpoint_id: endpoint_id.clone(),
            stale,
            row_gap: config.spaces.row_gap,
            next_machine_below: false,
            drop_target_row: None,
        }
    }
}

/// The row a workspace drag's drop target is on, on the active endpoint. Group drags target
/// the row above a header instead, so they never fill one.
fn workspace_drop_target_row(state: &super::render::ShellRenderState<'_>) -> Option<u16> {
    state
        .dragged_workspace_id
        .and(state.workspace_drop_indicator_row)
}

/// Columns and bottom edge of one sidebar section: header hits cover `x..x + width`, drawing
/// (headers and the card's color bar) starts `indent` columns in, and nothing is drawn at or
/// below `bottom`.
#[derive(Debug, Clone, Copy)]
pub(super) struct HeaderColumns {
    x: u16,
    width: u16,
    indent: u16,
    bottom: u16,
}

impl HeaderColumns {
    pub(super) fn new(body: Rect, width: u16, indent: u16) -> Self {
        Self {
            x: body.x,
            width,
            indent,
            bottom: body.bottom(),
        }
    }
}

/// What one header row shows.
struct GroupHeader {
    /// Index into `workspace_groups.groups`; `None` for the "ungrouped" header.
    group: Option<usize>,
    collapsed: bool,
    member_count: usize,
    status: AgentStatus,
    /// Snapshot index of the first visible entry of this region, when the header sits above it.
    first_entry: Option<usize>,
}

/// Per-frame group decorations for one snapshot's entries, computed once per frame.
///
/// Header rows are decorations, not entries: they are added to the owning entry's row
/// height, so scroll metrics and reveal stay entry-based. A region's header sits above its
/// first visible entry; headers of empty groups and of collapsed groups with no visible member
/// stack above the next visible entry or trail after the last one (at the top when there is
/// no entry at all).
pub(super) struct GroupLayout {
    headers: Vec<GroupHeader>,
    /// Header range drawn above each entry, by snapshot workspace index.
    before: Vec<(usize, usize)>,
    /// Headers after the last entry, or at the top of an empty list.
    trailing: (usize, usize),
    /// Snapshot index of the last entry.
    last_entry: Option<usize>,
    /// Effective bar color by snapshot workspace index.
    colors: Vec<Option<[u8; 3]>>,
    /// Region by snapshot workspace index; empty without groups.
    regions: Vec<usize>,
    /// Whether the entry is the last visible one of its region, by snapshot workspace index.
    region_end: Vec<bool>,
}

impl GroupLayout {
    pub(super) fn new(
        snapshot: &ClientShellSnapshot,
        entries: &[WorkspaceEntry],
        collapsed: &HashSet<String>,
    ) -> Self {
        let groups = &snapshot.workspace_groups.groups;
        let workspace_colors = &snapshot.workspace_groups.workspace_colors;
        if groups.is_empty() && workspace_colors.is_empty() {
            // Upstream rendering: no headers, bars or slots, and no per-frame allocation.
            return Self {
                headers: Vec::new(),
                before: Vec::new(),
                trailing: (0, 0),
                last_entry: entries.last().map(|entry| entry.index),
                colors: Vec::new(),
                regions: Vec::new(),
                region_end: Vec::new(),
            };
        }
        let workspace_count = snapshot.workspaces.len();
        let colors_by_id = workspace_colors
            .iter()
            .filter_map(|(workspace_id, name)| Some((workspace_id.as_str(), color_rgb(name)?)))
            .collect::<HashMap<_, _>>();
        let colors = snapshot
            .workspaces
            .iter()
            .map(|workspace| colors_by_id.get(workspace.workspace_id.as_str()).copied())
            .collect::<Vec<_>>();
        let mut layout = Self {
            headers: Vec::new(),
            before: vec![(0, 0); workspace_count],
            trailing: (0, 0),
            last_entry: entries.last().map(|entry| entry.index),
            colors,
            regions: Vec::new(),
            region_end: vec![false; workspace_count],
        };
        if groups.is_empty() {
            return layout;
        }
        let ungrouped = groups.len();
        let regions = workspace_regions(snapshot);
        let mut member_counts = vec![0; ungrouped + 1];
        let mut statuses = vec![AgentStatus::Unknown; ungrouped + 1];
        for (workspace, region) in snapshot.workspaces.iter().zip(&regions) {
            member_counts[*region] += 1;
            if status_priority(workspace.agent_status) > status_priority(statuses[*region]) {
                statuses[*region] = workspace.agent_status;
            }
        }
        let header = |region: usize, first_entry: Option<usize>| GroupHeader {
            group: (region < ungrouped).then_some(region),
            collapsed: groups
                .get(region)
                .is_some_and(|group| collapsed.contains(&group_collapse_key(&group.group_id))),
            member_count: member_counts[region],
            status: statuses[region],
            first_entry,
        };
        let mut next_region = 0;
        for (position, entry) in entries.iter().enumerate() {
            let Some(region) = regions.get(entry.index).copied() else {
                continue;
            };
            let start = layout.headers.len();
            while next_region <= region {
                if next_region == region {
                    layout.headers.push(header(region, Some(entry.index)));
                } else if next_region < ungrouped {
                    layout.headers.push(header(next_region, None));
                }
                next_region += 1;
            }
            if let Some(range) = layout.before.get_mut(entry.index) {
                *range = (start, layout.headers.len());
            }
            let region_ends = entries
                .get(position + 1)
                .and_then(|next| regions.get(next.index))
                .is_none_or(|next| *next != region);
            if let Some(end) = layout.region_end.get_mut(entry.index) {
                *end = region_ends;
            }
        }
        let start = layout.headers.len();
        while next_region < ungrouped {
            layout.headers.push(header(next_region, None));
            next_region += 1;
        }
        layout.trailing = (start, layout.headers.len());
        layout.regions = regions;
        layout
    }

    /// Shows the "ungrouped" header while a workspace drag is in progress even when no
    /// ungrouped workspace is visible, so ungrouping stays a drop target.
    pub(super) fn with_ungrouped_drop_header(mut self, dragging: bool) -> Self {
        let shown = self.headers.iter().any(|header| header.group.is_none());
        if dragging && !self.regions.is_empty() && !shown {
            self.headers.push(GroupHeader {
                group: None,
                collapsed: false,
                member_count: 0,
                status: AgentStatus::Unknown,
                first_entry: None,
            });
            self.trailing.1 = self.headers.len();
        }
        self
    }

    fn row_count((start, end): (usize, usize)) -> u16 {
        end.saturating_sub(start).min(usize::from(u16::MAX)) as u16
    }

    /// Header rows drawn above the entry of this snapshot workspace index.
    pub(super) fn rows_before(&self, index: usize) -> u16 {
        self.before.get(index).copied().map_or(0, Self::row_count)
    }

    /// Header rows drawn below the entry of this snapshot workspace index (the trailing
    /// headers, below the last entry).
    pub(super) fn rows_after(&self, index: usize) -> u16 {
        if self.last_entry == Some(index) {
            Self::row_count(self.trailing)
        } else {
            0
        }
    }

    /// Entry row heights (in entry order) plus the header rows each entry carries.
    pub(super) fn with_header_rows(
        &self,
        entries: &[WorkspaceEntry],
        heights: Vec<u16>,
    ) -> Vec<u16> {
        heights
            .into_iter()
            .zip(entries)
            .map(|(height, entry)| {
                height
                    .saturating_add(self.rows_before(entry.index))
                    .saturating_add(self.rows_after(entry.index))
            })
            .collect()
    }

    /// Draws the headers above an entry from row `y` and returns the row of its card.
    pub(super) fn render_before(
        &self,
        buffer: &mut Buffer,
        index: usize,
        columns: HeaderColumns,
        y: u16,
        paint: &GroupPaint<'_>,
        hits: &mut ShellHitMap,
    ) -> u16 {
        let range = self.before.get(index).copied().unwrap_or((0, 0));
        self.render_headers(buffer, range, columns, y, paint, hits)
    }

    /// Draws the headers of a section without any entry from row `y`; returns the next row.
    pub(super) fn render_without_entries(
        &self,
        buffer: &mut Buffer,
        columns: HeaderColumns,
        y: u16,
        paint: &GroupPaint<'_>,
        hits: &mut ShellHitMap,
    ) -> u16 {
        if self.last_entry.is_some() {
            return y;
        }
        self.render_headers(buffer, self.trailing, columns, y, paint, hits)
    }

    /// Paints the color bar on every line of a drawn card (at the card's column 0), records
    /// its drop slots (above a top-level card, at the end of its region) and, below the last
    /// entry, draws the trailing headers. `card` is the whole card, label rows and any tab
    /// rows below them, and is decorated after all of its rows are drawn.
    pub(super) fn decorate_card(
        &self,
        buffer: &mut Buffer,
        entry: &WorkspaceEntry,
        card: Rect,
        columns: HeaderColumns,
        paint: &GroupPaint<'_>,
        hits: &mut ShellHitMap,
    ) {
        let index = entry.index;
        if let Some(rgb) = self.colors.get(index).copied().flatten() {
            let bar_x = columns.x.saturating_add(columns.indent);
            for y in card.y..card.bottom() {
                paint_bar(buffer, bar_x, y, rgb, paint.host_bg);
            }
        }
        let Some(region) = self.regions.get(index).copied() else {
            return;
        };
        let group_id = paint
            .snapshot
            .workspace_groups
            .groups
            .get(region)
            .map(|group| group.group_id.clone());
        if !entry.indented {
            let workspace_id = paint
                .snapshot
                .workspaces
                .get(index)
                .map(|workspace| workspace.workspace_id.clone());
            record_slot(
                hits,
                &paint.endpoint_id,
                card.y.saturating_sub(1),
                group_id.clone(),
                workspace_id,
            );
        }
        if self.region_end.get(index).copied().unwrap_or(false) {
            // The end slot takes the row below the card when it is free: the row gap above the
            // next region's header (a region's first entry is always top level), or, below the
            // section's last entry, the row trailing headers would start on unless the next
            // machine's row follows there. Otherwise, and when that row is outside the list
            // body (the footer row), it sits on the card's last line.
            let row_below_free = if self.last_entry == Some(index) {
                self.rows_after(index) == 0 && !paint.next_machine_below
            } else {
                paint.row_gap > 0
            };
            let row = if row_below_free && card.bottom() < columns.bottom {
                card.bottom()
            } else {
                card.bottom().saturating_sub(1)
            };
            record_slot(hits, &paint.endpoint_id, row, group_id, None);
        }
        if self.last_entry == Some(index) {
            self.render_headers(buffer, self.trailing, columns, card.bottom(), paint, hits);
        }
    }

    fn render_headers(
        &self,
        buffer: &mut Buffer,
        (start, end): (usize, usize),
        columns: HeaderColumns,
        mut y: u16,
        paint: &GroupPaint<'_>,
        hits: &mut ShellHitMap,
    ) -> u16 {
        for header in self.headers.get(start..end).unwrap_or_default() {
            if y >= columns.bottom || y >= buffer.area.bottom() {
                break;
            }
            draw_header_row(buffer, columns, y, header, paint);
            let group_id = header_group_id(header, paint).map(str::to_owned);
            hits.workspace_groups.push(WorkspaceGroupHit {
                rect: Rect::new(columns.x, y, columns.width, 1),
                endpoint_id: paint.endpoint_id.clone(),
                group_id: group_id.clone(),
                collapsed: header.collapsed,
            });
            let before_workspace_id = header
                .first_entry
                .filter(|_| !header.collapsed)
                .and_then(|index| paint.snapshot.workspaces.get(index))
                .map(|workspace| workspace.workspace_id.clone());
            record_slot(hits, &paint.endpoint_id, y, group_id, before_workspace_id);
            y = y.saturating_add(1);
        }
        y
    }

    /// Redraws the header a workspace drag targets over upstream's drop line, which the
    /// single-machine sidebar draws after its cards: the header itself is the target.
    pub(super) fn render_drop_target_header(
        &self,
        buffer: &mut Buffer,
        columns: HeaderColumns,
        state: &super::render::ShellRenderState<'_>,
        paint: &GroupPaint<'_>,
        hits: &ShellHitMap,
    ) {
        let Some(row) = workspace_drop_target_row(state) else {
            return;
        };
        let Some(hit) = hits
            .workspace_groups
            .iter()
            .find(|hit| hit.rect.y == row && hit.endpoint_id == paint.endpoint_id)
        else {
            return;
        };
        let Some(header) = self
            .headers
            .iter()
            .find(|header| header_group_id(header, paint) == hit.group_id.as_deref())
        else {
            return;
        };
        for x in hit.rect.left()..hit.rect.right() {
            if let Some(cell) = buffer.cell_mut((x, row)) {
                cell.reset();
            }
        }
        buffer.set_style(hit.rect, Style::default().bg(paint.palette.sidebar_bg));
        let paint = GroupPaint {
            endpoint_id: paint.endpoint_id.clone(),
            drop_target_row: Some(row),
            ..*paint
        };
        draw_header_row(buffer, columns, row, header, &paint);
    }
}

/// The group id of a header; `None` for the "ungrouped" header.
fn header_group_id<'a>(header: &GroupHeader, paint: &GroupPaint<'a>) -> Option<&'a str> {
    header
        .group
        .and_then(|group| paint.snapshot.workspace_groups.groups.get(group))
        .map(|group| group.group_id.as_str())
}

/// Draws one header row (from the section's indent), on `surface1` while it is a workspace
/// drag's drop target.
fn draw_header_row(
    buffer: &mut Buffer,
    columns: HeaderColumns,
    y: u16,
    header: &GroupHeader,
    paint: &GroupPaint<'_>,
) {
    let drawn = Rect::new(
        columns.x.saturating_add(columns.indent),
        y,
        columns.width.saturating_sub(columns.indent),
        1,
    );
    if paint.drop_target_row == Some(y) {
        buffer.set_style(drawn, Style::default().bg(paint.palette.surface1));
    }
    render_header(buffer, drawn, header, paint);
}

/// Group layouts of every machine section of the multi-machine sidebar, by endpoint index.
/// A collapsed machine, one without a snapshot and one whose server projects no groups or
/// colors draw nothing extra.
pub(super) struct EndpointGroups<'a> {
    sections: Vec<Option<(GroupLayout, GroupPaint<'a>)>>,
}

impl<'a> EndpointGroups<'a> {
    pub(super) fn new(
        state: &super::render::ShellRenderState<'_>,
        endpoints: &'a [ClientShellEndpoint],
        config: &'a ClientShellConfig,
    ) -> Self {
        let empty = HashSet::new();
        let dragging = state.dragged_workspace_id.is_some();
        let sections = endpoints
            .iter()
            .enumerate()
            .map(|(position, endpoint)| {
                if state.collapsed_endpoints.contains(&endpoint.endpoint_id) {
                    return None;
                }
                let snapshot = endpoint.snapshot.as_deref()?;
                let projection = &snapshot.workspace_groups;
                if projection.groups.is_empty() && projection.workspace_colors.is_empty() {
                    // Nothing to draw: skip the entry pass render_expanded already runs.
                    return None;
                }
                let collapsed = if endpoint.endpoint_id.is_local() {
                    state.collapsed_groups
                } else {
                    state
                        .remote_collapsed_groups
                        .get(&endpoint.endpoint_id)
                        .unwrap_or(&empty)
                };
                let entries = super::sidebar::workspace_entries(snapshot, collapsed);
                let layout = GroupLayout::new(snapshot, &entries, collapsed)
                    .with_ungrouped_drop_header(
                        dragging && &endpoint.endpoint_id == state.active_endpoint_id,
                    );
                let stale = endpoint.status != ClientEndpointStatus::Online;
                let mut paint = GroupPaint::new(
                    snapshot,
                    config,
                    state.host_background_rgb,
                    &endpoint.endpoint_id,
                    stale,
                );
                // Every machine has a row, drawn right below the previous section (no gap).
                paint.next_machine_below = position + 1 < endpoints.len();
                if &endpoint.endpoint_id == state.active_endpoint_id {
                    paint.drop_target_row = workspace_drop_target_row(state);
                }
                Some((layout, paint))
            })
            .collect();
        Self { sections }
    }

    fn section(&self, endpoint: usize) -> Option<&(GroupLayout, GroupPaint<'a>)> {
        self.sections.get(endpoint)?.as_ref()
    }

    /// Header rows of a machine section without any visible workspace.
    pub(super) fn rows_without_entries(&self, endpoint: usize) -> u16 {
        self.section(endpoint)
            .filter(|(layout, _)| layout.last_entry.is_none())
            .map_or(0, |(layout, _)| GroupLayout::row_count(layout.trailing))
    }

    /// Header rows above and below one workspace entry of a machine section.
    pub(super) fn rows_around(&self, endpoint: usize, index: usize) -> u16 {
        self.rows_before(endpoint, index)
            .saturating_add(self.rows_after(endpoint, index))
    }

    pub(super) fn rows_before(&self, endpoint: usize, index: usize) -> u16 {
        self.section(endpoint)
            .map_or(0, |(layout, _)| layout.rows_before(index))
    }

    pub(super) fn rows_after(&self, endpoint: usize, index: usize) -> u16 {
        self.section(endpoint)
            .map_or(0, |(layout, _)| layout.rows_after(index))
    }

    /// Draws the headers above one workspace entry; returns the row of its card.
    pub(super) fn render_before(
        &self,
        buffer: &mut Buffer,
        endpoint: usize,
        index: usize,
        columns: HeaderColumns,
        y: u16,
        hits: &mut ShellHitMap,
    ) -> u16 {
        match self.section(endpoint) {
            Some((layout, paint)) => layout.render_before(buffer, index, columns, y, paint, hits),
            None => y,
        }
    }

    /// Bar, drop slots and trailing headers of one drawn workspace card.
    pub(super) fn decorate_card(
        &self,
        buffer: &mut Buffer,
        endpoint: usize,
        entry: &WorkspaceEntry,
        card: Rect,
        columns: HeaderColumns,
        hits: &mut ShellHitMap,
    ) {
        if let Some((layout, paint)) = self.section(endpoint) {
            layout.decorate_card(buffer, entry, card, columns, paint, hits);
        }
    }

    /// Draws the headers of a machine section without any visible workspace below its
    /// machine row; returns the next row.
    pub(super) fn render_without_entries(
        &self,
        buffer: &mut Buffer,
        endpoint: usize,
        columns: HeaderColumns,
        y: u16,
        hits: &mut ShellHitMap,
    ) -> u16 {
        match self.section(endpoint) {
            Some((layout, paint)) => layout.render_without_entries(buffer, columns, y, paint, hits),
            None => y,
        }
    }
}

/// Records a drop slot; the first slot recorded on a row wins.
fn record_slot(
    hits: &mut ShellHitMap,
    endpoint_id: &ClientEndpointId,
    row: u16,
    group_id: Option<String>,
    before_workspace_id: Option<String>,
) {
    // Slots are recorded top to bottom per section, so a duplicate row can only be the last.
    if hits
        .workspace_drop_slots
        .last()
        .is_some_and(|slot| slot.row == row && &slot.endpoint_id == endpoint_id)
    {
        return;
    }
    hits.workspace_drop_slots.push(WorkspaceDropSlot {
        endpoint_id: endpoint_id.clone(),
        row,
        group_id,
        before_workspace_id,
    });
}

/// Sets only the symbol and foreground, so selected, focused and dragged backgrounds stay.
fn paint_bar(buffer: &mut Buffer, x: u16, y: u16, rgb: [u8; 3], host_bg: [u8; 3]) {
    if let Some(cell) = buffer.cell_mut((x, y)) {
        let color = readable(rgb, cell.bg, host_bg);
        cell.set_symbol(BAR_GLYPH);
        cell.set_fg(color);
    }
}

fn render_header(buffer: &mut Buffer, rect: Rect, header: &GroupHeader, paint: &GroupPaint<'_>) {
    use super::render::{display_width, put_segment, put_text};

    if rect.is_empty() {
        return;
    }
    let palette = paint.palette;
    let Some(group) = header
        .group
        .and_then(|group| paint.snapshot.workspace_groups.groups.get(group))
    else {
        put_text(
            buffer,
            rect.x.saturating_add(2),
            rect.y,
            rect.width.saturating_sub(2),
            UNGROUPED_HEADER_TEXT,
            Style::default().fg(palette.overlay0),
        );
        dim_if_stale(buffer, rect, paint);
        return;
    };
    if let Some(rgb) = color_rgb(&group.color) {
        paint_bar(buffer, rect.x, rect.y, rgb, paint.host_bg);
    }
    let toggle_x = rect.right().saturating_sub(1);
    let count = header
        .collapsed
        .then(|| format!(" ({})", header.member_count));
    let status = header
        .collapsed
        .then(|| status_icon(header.status, paint.indicators));
    let rollup_width = count.as_deref().map_or(0, display_width)
        + status.map_or(0, |icon| display_width(icon).saturating_add(1));
    let name_x = rect.x.saturating_add(2);
    let name_right = toggle_x
        .saturating_sub(1)
        .saturating_sub(rollup_width)
        .max(name_x);
    let mut x = put_segment(
        buffer,
        name_x,
        rect.y,
        name_right,
        &group.name,
        Style::default()
            .fg(palette.text)
            .add_modifier(Modifier::BOLD),
    );
    if let Some(count) = count.as_deref() {
        x = put_segment(
            buffer,
            x,
            rect.y,
            toggle_x,
            count,
            Style::default().fg(palette.overlay0),
        );
    }
    if let Some(icon) = status {
        x = put_segment(buffer, x, rect.y, toggle_x, " ", Style::default());
        put_segment(
            buffer,
            x,
            rect.y,
            toggle_x,
            icon,
            Style::default().fg(status_color(header.status, palette)),
        );
    }
    if toggle_x > name_x {
        put_text(
            buffer,
            toggle_x,
            rect.y,
            1,
            if header.collapsed {
                COLLAPSED_GLYPH
            } else {
                EXPANDED_GLYPH
            },
            Style::default().fg(palette.accent),
        );
    }
    dim_if_stale(buffer, rect, paint);
}

fn dim_if_stale(buffer: &mut Buffer, rect: Rect, paint: &GroupPaint<'_>) {
    if paint.stale {
        buffer.set_style(
            rect,
            Style::default()
                .fg(paint.palette.overlay0)
                .add_modifier(Modifier::DIM),
        );
    }
}

// Interactions (interaction half). Every organize action is one `workspace_group.edit` batch
// sent through the existing endpoint request path, which already shows "Action unavailable"
// when the endpoint does not advertise the method.

/// A left press on a group header, until it is released (toggle) or moved (group drag).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WorkspaceGroupPress {
    pub(super) endpoint_id: ClientEndpointId,
    pub(super) group_id: String,
    pub(super) start_column: u16,
    pub(super) start_row: u16,
}

/// The drop slot nearest to the pointer while a workspace is dragged on the active endpoint, as
/// (before workspace id, indicator row). `None` when that endpoint projects no groups, so
/// upstream's drop targets apply unchanged.
pub(super) fn drop_target_at(
    state: &ClientShellState,
    point: (u16, u16),
) -> Option<(Option<String>, u16)> {
    let snapshot = state.snapshot.as_deref()?;
    if snapshot.workspace_groups.groups.is_empty() {
        return None;
    }
    state
        .hits
        .workspace_drop_slots
        .iter()
        .filter(|slot| {
            slot.endpoint_id == state.active_endpoint_id && slot.row < state.hits.new_workspace.y
        })
        .enumerate()
        .min_by_key(|(index, slot)| (point.1.abs_diff(slot.row), *index))
        .map(|(_, slot)| (slot.before_workspace_id.clone(), slot.row))
}

/// The `place` for a workspace dropped on the slot recorded at `row`. `None` without groups,
/// for a linked child (not draggable, as upstream) and for a drop on the workspace's own slot.
pub(super) fn workspace_drop_method(
    state: &ClientShellState,
    source_workspace_id: &str,
    row: u16,
) -> Option<Method> {
    let snapshot = state.snapshot.as_deref()?;
    if snapshot.workspace_groups.groups.is_empty() {
        return None;
    }
    let slot = state
        .hits
        .workspace_drop_slots
        .iter()
        .find(|slot| slot.endpoint_id == state.active_endpoint_id && slot.row == row)?;
    let linked = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == source_workspace_id)?
        .worktree
        .as_ref()
        .is_some_and(|worktree| worktree.is_linked_worktree);
    if linked || slot.before_workspace_id.as_deref() == Some(source_workspace_id) {
        return None;
    }
    Some(edit_method(vec![WorkspaceGroupOp::Place {
        workspace_id: source_workspace_id.to_owned(),
        group_id: slot.group_id.clone(),
        before_workspace_id: slot.before_workspace_id.clone(),
    }]))
}

fn edit_method(ops: Vec<WorkspaceGroupOp>) -> Method {
    Method::WorkspaceGroupEdit(WorkspaceGroupEditParams { ops })
}

impl ClientShellState {
    /// Sends one `workspace_group.edit` request to the active endpoint. Any organize edit
    /// clears a stashed "Undo delete group".
    pub(super) fn send_organize_method(
        &mut self,
        method: Method,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let sent =
            self.push_endpoint_method_with_kind(method, PendingEndpointKind::Generic, outcome);
        if sent {
            self.organize_undo = None;
        }
        sent
    }

    /// The stashed delete of the active endpoint, while that server has not restarted.
    fn organize_undo_available(&self) -> Option<&OrganizeUndo> {
        let boot_id = &self.snapshot.as_deref()?.boot_id;
        self.organize_undo
            .as_ref()
            .filter(|undo| undo.endpoint_id == self.active_endpoint_id && &undo.boot_id == boot_id)
    }

    /// Sends the stashed inverse of the last delete. Members no longer in the snapshot are
    /// dropped and a vanished `before_group_id` appends. The stash is kept until the server
    /// confirms the restore (`PendingEndpointKind::OrganizeUndo`), so a rejected batch, shown
    /// with the server message, can be retried.
    fn undo_delete_group(&mut self, outcome: &mut ClientShellInput) {
        let Some(snapshot) = self.snapshot.as_deref() else {
            return;
        };
        let Some(WorkspaceGroupOp::Create {
            name,
            color,
            template,
            workspace_ids,
            before_group_id,
        }) = self
            .organize_undo_available()
            .map(|undo| undo.restore.clone())
        else {
            return;
        };
        let workspace_ids = workspace_ids
            .into_iter()
            .filter(|workspace_id| {
                snapshot
                    .workspaces
                    .iter()
                    .any(|workspace| &workspace.workspace_id == workspace_id)
            })
            .collect();
        let before_group_id = before_group_id.filter(|before| {
            snapshot
                .workspace_groups
                .groups
                .iter()
                .any(|group| &group.group_id == before)
        });
        self.push_endpoint_method_with_kind(
            edit_method(vec![WorkspaceGroupOp::Create {
                name,
                color,
                template,
                workspace_ids,
                before_group_id,
            }]),
            PendingEndpointKind::OrganizeUndo,
            outcome,
        );
    }

    /// Sends one `workspace_group.edit` batch to the active endpoint.
    pub(super) fn send_organize_ops(
        &mut self,
        ops: Vec<WorkspaceGroupOp>,
        outcome: &mut ClientShellInput,
    ) -> bool {
        self.send_organize_method(edit_method(ops), outcome)
    }

    /// Records a left press on a group header (asked before upstream's worktree group
    /// toggle). The "ungrouped" header has no toggle and is not pressable.
    pub(super) fn press_workspace_group_header(&mut self, point: (u16, u16)) -> bool {
        let Some((endpoint_id, group_id)) = self
            .hits
            .workspace_groups
            .iter()
            .find(|hit| super::contains(hit.rect, point))
            .and_then(|hit| Some((hit.endpoint_id.clone(), hit.group_id.clone()?)))
        else {
            return false;
        };
        self.workspace_group_press = Some(WorkspaceGroupPress {
            endpoint_id,
            group_id,
            start_column: point.0,
            start_row: point.1,
        });
        true
    }

    /// Turns a header press into a group drag once the pointer moves a cell. Only the active
    /// endpoint's groups can be dragged; moving a press on another machine's header drops it.
    pub(super) fn drag_workspace_group_press(
        &mut self,
        point: (u16, u16),
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(press) = self.workspace_group_press.as_ref() else {
            return false;
        };
        let moved = point.0.abs_diff(press.start_column) >= 1 || point.1 != press.start_row;
        if !moved {
            return true;
        }
        let Some(press) = self.workspace_group_press.take() else {
            return true;
        };
        if press.endpoint_id == self.active_endpoint_id {
            let target = self.group_drag_target_at(point);
            self.chrome_drag = Some(ClientChromeDrag::WorkspaceGroup {
                endpoint_id: press.endpoint_id,
                group_id: press.group_id,
                target,
            });
            outcome.repaint = true;
        }
        true
    }

    pub(super) fn update_workspace_group_drag(
        &mut self,
        point: (u16, u16),
        outcome: &mut ClientShellInput,
    ) {
        let target = self.group_drag_target_at(point);
        if let Some(ClientChromeDrag::WorkspaceGroup {
            target: current, ..
        }) = self.chrome_drag.as_mut()
        {
            *current = target;
        }
        outcome.repaint = true;
    }

    /// A header released without moving toggles its `group:<id>` collapse key, which is
    /// client-local and saved in the client preferences.
    pub(super) fn release_workspace_group_press(&mut self, outcome: &mut ClientShellInput) -> bool {
        let Some(press) = self.workspace_group_press.take() else {
            return false;
        };
        self.toggle_collapsed_group(&press.endpoint_id, group_collapse_key(&press.group_id));
        self.persist_chrome_preferences(outcome);
        outcome.repaint = true;
        true
    }

    /// Sends `move` for a dropped group header; a drop where the group already is sends
    /// nothing.
    pub(super) fn drop_workspace_group(
        &mut self,
        endpoint_id: ClientEndpointId,
        group_id: String,
        target: Option<(Option<String>, u16)>,
        outcome: &mut ClientShellInput,
    ) {
        outcome.repaint = true;
        let Some((before_group_id, _)) = target else {
            return;
        };
        if endpoint_id != self.active_endpoint_id {
            return;
        }
        let Some(groups) = self
            .snapshot
            .as_deref()
            .map(|snapshot| &snapshot.workspace_groups.groups)
        else {
            return;
        };
        let Some(position) = groups.iter().position(|group| group.group_id == group_id) else {
            return;
        };
        let unchanged = match before_group_id.as_deref() {
            Some(before) => {
                before == group_id
                    || groups
                        .get(position + 1)
                        .is_some_and(|next| next.group_id == before)
            }
            None => position + 1 == groups.len(),
        };
        if !unchanged {
            self.send_organize_ops(
                vec![WorkspaceGroupOp::Move {
                    group_id,
                    before_group_id,
                }],
                outcome,
            );
        }
    }

    /// Group drag targets of the active endpoint: the row above each group header, plus an
    /// end slot after the last group (above the "ungrouped" header when it is drawn, else
    /// below the last card, its tab rows included). The nearest target wins; ties go to the
    /// first.
    fn group_drag_target_at(&self, point: (u16, u16)) -> Option<(Option<String>, u16)> {
        let active = &self.active_endpoint_id;
        let mut targets = Vec::new();
        let mut ungrouped_row = None;
        let mut last_row = 0;
        for hit in self
            .hits
            .workspace_groups
            .iter()
            .filter(|hit| &hit.endpoint_id == active)
        {
            match &hit.group_id {
                Some(group_id) => {
                    targets.push((Some(group_id.clone()), hit.rect.y.saturating_sub(1)));
                    last_row = hit.rect.bottom().max(last_row);
                }
                None => {
                    ungrouped_row = ungrouped_row.or(Some(hit.rect.y.saturating_sub(1)));
                }
            }
        }
        if targets.is_empty() {
            return None;
        }
        let end = ungrouped_row.unwrap_or_else(|| {
            self.hits
                .workspaces
                .iter()
                .filter(|hit| &hit.endpoint_id == active)
                .map(|hit| hit.card_bottom)
                .fold(last_row, u16::max)
        });
        if end < self.hits.new_workspace.y {
            targets.push((None, end));
        }
        targets
            .into_iter()
            .enumerate()
            .min_by_key(|(index, (_, row))| (point.1.abs_diff(*row), *index))
            .map(|(_, target)| target)
    }
}

/// Where Shift+Up / Shift+Down moves a workspace, as (target group, before workspace): one
/// slot in grouped display order. Inside a region it swaps with its neighbor; from the top or
/// bottom of a region it joins the end of the previous region or the start of the next one,
/// empty groups included. A linked child shown under its parents only changes regions.
/// `None` above the first region and below the ungrouped one.
fn keyboard_place_target(
    snapshot: &ClientShellSnapshot,
    workspace_id: &str,
    down: bool,
) -> Option<(Option<String>, Option<String>)> {
    let groups = &snapshot.workspace_groups.groups;
    let ungrouped = groups.len();
    let regions = workspace_regions(snapshot);
    let entries = super::sidebar::workspace_entries(snapshot, &HashSet::new());
    let index = snapshot
        .workspaces
        .iter()
        .position(|workspace| workspace.workspace_id == workspace_id)?;
    let region = regions.get(index).copied().unwrap_or(ungrouped);
    let id = |index: usize| {
        snapshot
            .workspaces
            .get(index)
            .map(|workspace| workspace.workspace_id.clone())
    };
    let group_id = |region: usize| groups.get(region).map(|group| group.group_id.clone());
    let (regions, entries) = (&regions, &entries);
    let units_of = |region: usize| {
        entries
            .iter()
            .filter(move |entry| !entry.indented && regions.get(entry.index) == Some(&region))
            .map(|entry| entry.index)
    };
    let units = units_of(region).collect::<Vec<_>>();
    let position = units.iter().position(|unit| *unit == index);
    match position {
        Some(position) if down && position + 1 < units.len() => Some((
            group_id(region),
            units.get(position + 2).copied().and_then(id),
        )),
        Some(position) if !down && position > 0 => Some((
            group_id(region),
            units.get(position - 1).copied().and_then(id),
        )),
        _ if down => {
            if region >= ungrouped {
                return None;
            }
            let next = region + 1;
            Some((group_id(next), units_of(next).next().and_then(id)))
        }
        _ => Some((group_id(region.checked_sub(1)?), None)),
    }
}

/// Organize menu actions; color items carry an index into `WORKSPACE_COLORS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OrganizeAction {
    Open,
    MoveToGroup,
    WorkspaceColorMenu,
    SetWorkspaceColor(u8),
    UseGroupColor,
    EditGroup,
    RenameGroup,
    GroupColorMenu,
    SetGroupColor(u8),
    ToggleCollapse,
    MoveGroupUp,
    MoveGroupDown,
    DeleteGroup,
    UndoDeleteGroup,
    ExpandAllGroups,
}

/// The state-dependent items of a workspace or group menu, captured when the menu opens
/// (`ClientContextMenuOverlay::items` has no access to the shell state).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct OrganizeMenuOptions {
    /// The group items are shown: a group menu, or a workspace that is in a group.
    pub(super) group_items: bool,
    /// That group is collapsed ("Expand" instead of "Collapse").
    pub(super) collapsed: bool,
    /// A deleted group is stashed for "Undo delete group".
    pub(super) undo: bool,
    /// Some group of the endpoint is collapsed ("Expand all groups", workspace menu only).
    pub(super) expand_all: bool,
}

/// The inverse of the last "Delete group", built from the snapshot before the delete was sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OrganizeUndo {
    pub(super) endpoint_id: ClientEndpointId,
    pub(super) boot_id: String,
    pub(super) restore: crate::api::schema::WorkspaceGroupOp, // always a Create
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum OrganizeMenu {
    Workspace {
        endpoint_id: ClientEndpointId,
        workspace_id: String,
        options: OrganizeMenuOptions,
    },
    WorkspaceColor {
        endpoint_id: ClientEndpointId,
        workspace_id: String,
    },
    Group {
        endpoint_id: ClientEndpointId,
        group_id: String,
        options: OrganizeMenuOptions,
    },
    GroupColor {
        endpoint_id: ClientEndpointId,
        group_id: String,
    },
}

// The variant names are the fork plan's reserved names; each one names its prompt's action.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum OrganizePrompt {
    MoveToGroup {
        endpoint_id: ClientEndpointId,
        workspace_id: String,
    },
    EditGroup {
        endpoint_id: ClientEndpointId,
        workspace_id: String,
    },
    RenameGroup {
        endpoint_id: ClientEndpointId,
        group_id: String,
    },
}

/// Menu labels of the `WORKSPACE_COLORS` entries, in palette order.
const COLOR_LABELS: [&str; 9] = [
    "Red", "Orange", "Yellow", "Green", "Teal", "Blue", "Purple", "Pink", "Gray",
];

fn organize_item(label: &'static str, action: OrganizeAction) -> ClientContextMenuItem {
    ClientContextMenuItem {
        label,
        action: ClientContextMenuAction::Organize(action),
    }
}

/// The "Organize..." item appended to every workspace menu.
pub(super) fn organize_menu_item() -> ClientContextMenuItem {
    organize_item("Organize...", OrganizeAction::Open)
}

fn color_items(action: fn(u8) -> OrganizeAction) -> Vec<ClientContextMenuItem> {
    COLOR_LABELS
        .iter()
        .zip(0u8..)
        .map(|(label, index)| organize_item(label, action(index)))
        .collect()
}

fn group_items(options: OrganizeMenuOptions) -> Vec<ClientContextMenuItem> {
    use OrganizeAction as Action;
    vec![
        organize_item("Rename group...", Action::RenameGroup),
        organize_item("Group color...", Action::GroupColorMenu),
        organize_item(
            if options.collapsed {
                "Expand"
            } else {
                "Collapse"
            },
            Action::ToggleCollapse,
        ),
        organize_item("Move group up", Action::MoveGroupUp),
        organize_item("Move group down", Action::MoveGroupDown),
        organize_item("Delete group", Action::DeleteGroup),
    ]
}

fn color_name(index: u8) -> Option<&'static str> {
    WORKSPACE_COLORS
        .get(usize::from(index))
        .map(|(name, _)| *name)
}

/// The group a workspace is an explicit member of.
fn group_of<'a>(
    snapshot: &'a ClientShellSnapshot,
    workspace_id: &str,
) -> Option<&'a ClientShellWorkspaceGroup> {
    snapshot
        .workspace_groups
        .groups
        .iter()
        .find(|group| group.workspace_ids.iter().any(|id| id == workspace_id))
}

/// A group by name, trimmed and compared case-insensitively (server names are unique that way).
fn group_named<'a>(
    snapshot: &'a ClientShellSnapshot,
    name: &str,
) -> Option<&'a ClientShellWorkspaceGroup> {
    let name = name.trim().to_lowercase();
    snapshot
        .workspace_groups
        .groups
        .iter()
        .find(|group| group.name.to_lowercase() == name)
}

impl OrganizeMenu {
    fn endpoint_id(&self) -> &ClientEndpointId {
        match self {
            Self::Workspace { endpoint_id, .. }
            | Self::WorkspaceColor { endpoint_id, .. }
            | Self::Group { endpoint_id, .. }
            | Self::GroupColor { endpoint_id, .. } => endpoint_id,
        }
    }

    pub(super) fn items(&self) -> Vec<ClientContextMenuItem> {
        use OrganizeAction as Action;
        match self {
            Self::Workspace { options, .. } => {
                let mut items = vec![
                    organize_item("Move to group...", Action::MoveToGroup),
                    organize_item("Workspace color...", Action::WorkspaceColorMenu),
                    organize_item("Edit group...", Action::EditGroup),
                ];
                if options.group_items {
                    items.extend(group_items(*options));
                }
                if options.undo {
                    items.push(organize_item("Undo delete group", Action::UndoDeleteGroup));
                }
                if options.expand_all {
                    items.push(organize_item("Expand all groups", Action::ExpandAllGroups));
                }
                items
            }
            Self::Group { options, .. } => {
                let mut items = group_items(*options);
                if options.undo {
                    items.push(organize_item("Undo delete group", Action::UndoDeleteGroup));
                }
                items
            }
            Self::WorkspaceColor { .. } => {
                let mut items = color_items(Action::SetWorkspaceColor);
                items.push(organize_item("Group color", Action::UseGroupColor));
                items
            }
            Self::GroupColor { .. } => color_items(Action::SetGroupColor),
        }
    }
}

impl ClientShellState {
    fn organize_options(
        &self,
        snapshot: &ClientShellSnapshot,
        group_id: Option<&str>,
    ) -> OrganizeMenuOptions {
        let endpoint_id = &self.active_endpoint_id;
        let collapsed =
            |group_id: &str| self.group_is_collapsed(endpoint_id, &group_collapse_key(group_id));
        OrganizeMenuOptions {
            group_items: group_id.is_some(),
            collapsed: group_id.is_some_and(collapsed),
            undo: self.organize_undo_available().is_some(),
            expand_all: snapshot
                .workspace_groups
                .groups
                .iter()
                .any(|group| collapsed(&group.group_id)),
        }
    }

    /// The Organize menu of a workspace on the active endpoint.
    pub(super) fn organize_workspace_menu(&self, workspace_id: String) -> Option<OrganizeMenu> {
        let snapshot = self.snapshot.as_deref()?;
        let group_id = group_of(snapshot, &workspace_id).map(|group| group.group_id.as_str());
        Some(OrganizeMenu::Workspace {
            endpoint_id: self.active_endpoint_id.clone(),
            options: self.organize_options(snapshot, group_id),
            workspace_id,
        })
    }

    fn organize_group_menu(&self, group_id: String) -> Option<OrganizeMenu> {
        let snapshot = self.snapshot.as_deref()?;
        Some(OrganizeMenu::Group {
            endpoint_id: self.active_endpoint_id.clone(),
            options: self.organize_options(snapshot, Some(&group_id)),
            group_id,
        })
    }

    /// Right-click on a group header of the active endpoint (asked before the workspace
    /// menu) opens that group's menu. The "ungrouped" header has none.
    pub(super) fn open_workspace_group_menu_at(&mut self, point: (u16, u16)) -> bool {
        let Some(menu) = self
            .hits
            .workspace_groups
            .iter()
            .find(|hit| {
                hit.endpoint_id == self.active_endpoint_id && super::contains(hit.rect, point)
            })
            .and_then(|hit| hit.group_id.clone())
            .and_then(|group_id| self.organize_group_menu(group_id))
        else {
            return false;
        };
        self.open_organize_menu(menu, point);
        true
    }

    /// An Organize item chosen in a menu: "Organize..." of a workspace menu, or any item of an
    /// Organize menu. Submenus open where the chosen menu was.
    pub(super) fn activate_organize_menu_item(
        &mut self,
        menu: ClientContextMenuOverlay,
        action: OrganizeAction,
        outcome: &mut ClientShellInput,
    ) {
        let anchor = (menu.x, menu.y);
        let organize = match menu.target {
            ClientContextMenuTarget::Organize(organize) => Some(organize),
            ClientContextMenuTarget::Workspace { workspace_id, .. } => {
                self.organize_workspace_menu(workspace_id)
            }
            ClientContextMenuTarget::Tab { .. } | ClientContextMenuTarget::Pane { .. } => None,
        };
        if let Some(organize) = organize {
            self.run_organize_action_at(organize, action, anchor, outcome);
        }
    }

    /// Runs an Organize action, opening any menu at its target's sidebar row.
    pub(super) fn run_organize_action(
        &mut self,
        menu: OrganizeMenu,
        action: OrganizeAction,
        outcome: &mut ClientShellInput,
    ) {
        let anchor = self.organize_anchor(&menu);
        self.run_organize_action_at(menu, action, anchor, outcome);
    }

    fn organize_anchor(&self, menu: &OrganizeMenu) -> (u16, u16) {
        let endpoint_id = menu.endpoint_id();
        let row = match menu {
            OrganizeMenu::Workspace { workspace_id, .. }
            | OrganizeMenu::WorkspaceColor { workspace_id, .. } => self
                .hits
                .workspaces
                .iter()
                .find(|hit| &hit.endpoint_id == endpoint_id && &hit.workspace_id == workspace_id)
                .map(|hit| hit.rect),
            OrganizeMenu::Group { group_id, .. } | OrganizeMenu::GroupColor { group_id, .. } => {
                self.hits
                    .workspace_groups
                    .iter()
                    .find(|hit| {
                        &hit.endpoint_id == endpoint_id
                            && hit.group_id.as_deref() == Some(group_id.as_str())
                    })
                    .map(|hit| hit.rect)
            }
        };
        let rect = row.unwrap_or(self.hits.workspace_body);
        (rect.x, rect.y)
    }

    fn open_organize_menu(&mut self, menu: OrganizeMenu, (x, y): (u16, u16)) {
        self.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
            target: ClientContextMenuTarget::Organize(menu),
            x,
            y,
            highlighted: 0,
        }));
    }

    fn open_organize_prompt(
        &mut self,
        title: &'static str,
        text: &str,
        replace_on_type: bool,
        prompt: OrganizePrompt,
    ) {
        self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            title,
            input: TextEditor::new(text, replace_on_type),
            target: ClientRenameTarget::Organize(prompt),
        }));
    }

    fn run_organize_action_at(
        &mut self,
        menu: OrganizeMenu,
        action: OrganizeAction,
        anchor: (u16, u16),
        outcome: &mut ClientShellInput,
    ) {
        use OrganizeAction as Action;

        outcome.repaint = true;
        let endpoint_id = self.active_endpoint_id.clone();
        if menu.endpoint_id() != &endpoint_id {
            return;
        }
        let Some(snapshot) = self.snapshot.as_deref() else {
            return;
        };
        let workspace_id = match &menu {
            OrganizeMenu::Workspace { workspace_id, .. }
            | OrganizeMenu::WorkspaceColor { workspace_id, .. } => Some(workspace_id.clone()),
            OrganizeMenu::Group { .. } | OrganizeMenu::GroupColor { .. } => None,
        };
        let group = match &menu {
            OrganizeMenu::Group { group_id, .. } | OrganizeMenu::GroupColor { group_id, .. } => {
                snapshot
                    .workspace_groups
                    .groups
                    .iter()
                    .find(|group| &group.group_id == group_id)
            }
            OrganizeMenu::Workspace { workspace_id, .. }
            | OrganizeMenu::WorkspaceColor { workspace_id, .. } => group_of(snapshot, workspace_id),
        }
        .cloned();
        let group_ids = snapshot
            .workspace_groups
            .groups
            .iter()
            .map(|group| group.group_id.clone())
            .collect::<Vec<_>>();
        let position = group.as_ref().and_then(|group| {
            group_ids
                .iter()
                .position(|group_id| *group_id == group.group_id)
        });
        match action {
            Action::Open => self.open_organize_menu(menu, anchor),
            Action::MoveToGroup | Action::EditGroup => {
                let Some(workspace_id) = workspace_id else {
                    return;
                };
                let text = group.map(|group| group.name).unwrap_or_default();
                let (title, prompt) = if action == Action::MoveToGroup {
                    (
                        "move to group",
                        OrganizePrompt::MoveToGroup {
                            endpoint_id,
                            workspace_id,
                        },
                    )
                } else {
                    (
                        "edit group",
                        OrganizePrompt::EditGroup {
                            endpoint_id,
                            workspace_id,
                        },
                    )
                };
                self.open_organize_prompt(title, &text, true, prompt);
            }
            Action::WorkspaceColorMenu => {
                if let Some(workspace_id) = workspace_id {
                    self.open_organize_menu(
                        OrganizeMenu::WorkspaceColor {
                            endpoint_id,
                            workspace_id,
                        },
                        anchor,
                    );
                }
            }
            Action::SetWorkspaceColor(_) | Action::UseGroupColor => {
                let color = match action {
                    Action::SetWorkspaceColor(index) => match color_name(index) {
                        Some(name) => Some(name.to_owned()),
                        None => return,
                    },
                    _ => None,
                };
                if let Some(workspace_id) = workspace_id {
                    self.send_organize_ops(
                        vec![WorkspaceGroupOp::SetColor {
                            workspace_id,
                            color,
                        }],
                        outcome,
                    );
                }
            }
            Action::RenameGroup => {
                if let Some(group) = group {
                    self.open_organize_prompt(
                        "rename group",
                        &group.name,
                        false,
                        OrganizePrompt::RenameGroup {
                            endpoint_id,
                            group_id: group.group_id,
                        },
                    );
                }
            }
            Action::GroupColorMenu => {
                if let Some(group) = group {
                    self.open_organize_menu(
                        OrganizeMenu::GroupColor {
                            endpoint_id,
                            group_id: group.group_id,
                        },
                        anchor,
                    );
                }
            }
            Action::SetGroupColor(index) => {
                if let (Some(group), Some(color)) = (group, color_name(index)) {
                    self.send_organize_ops(
                        vec![WorkspaceGroupOp::Update {
                            group_id: group.group_id,
                            name: None,
                            color: Some(color.to_owned()),
                            template: None,
                        }],
                        outcome,
                    );
                }
            }
            Action::ToggleCollapse => {
                if let Some(group) = group {
                    self.toggle_collapsed_group(&endpoint_id, group_collapse_key(&group.group_id));
                    self.persist_chrome_preferences(outcome);
                }
            }
            Action::MoveGroupUp | Action::MoveGroupDown => {
                let (Some(group), Some(position)) = (group, position) else {
                    return;
                };
                let before_group_id = if action == Action::MoveGroupUp {
                    let Some(previous) = position.checked_sub(1) else {
                        return;
                    };
                    group_ids.get(previous).cloned()
                } else {
                    if position + 1 >= group_ids.len() {
                        return;
                    }
                    group_ids.get(position + 2).cloned()
                };
                self.send_organize_ops(
                    vec![WorkspaceGroupOp::Move {
                        group_id: group.group_id,
                        before_group_id,
                    }],
                    outcome,
                );
            }
            Action::DeleteGroup => {
                let (Some(group), Some(position)) = (group, position) else {
                    return;
                };
                let boot_id = snapshot.boot_id.clone();
                let restore = WorkspaceGroupOp::Create {
                    name: group.name,
                    color: Some(group.color),
                    template: group.template,
                    workspace_ids: group.workspace_ids,
                    before_group_id: group_ids.get(position + 1).cloned(),
                };
                let sent = self.send_organize_ops(
                    vec![WorkspaceGroupOp::Delete {
                        group_id: group.group_id,
                    }],
                    outcome,
                );
                if sent {
                    self.organize_undo = Some(OrganizeUndo {
                        endpoint_id,
                        boot_id,
                        restore,
                    });
                }
            }
            Action::UndoDeleteGroup => self.undo_delete_group(outcome),
            Action::ExpandAllGroups => {
                for group_id in &group_ids {
                    let key = group_collapse_key(group_id);
                    if self.group_is_collapsed(&endpoint_id, &key) {
                        self.toggle_collapsed_group(&endpoint_id, key);
                    }
                }
                self.persist_chrome_preferences(outcome);
            }
        }
    }

    /// Navigate-mode keys, asked only after user bindings, navigate keys and digits failed:
    /// Shift+Up / Shift+Down move the selected workspace one slot in grouped display order
    /// (navigate mode and the selection stay, so the key repeats), `m` opens its Organize menu.
    pub(super) fn handle_organize_navigate_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        use crossterm::event::{KeyCode, KeyModifiers};

        let matches =
            |code, modifiers| crate::config::terminal_key_matches_combo(key, (code, modifiers));
        let down = if matches(KeyCode::Up, KeyModifiers::SHIFT) {
            false
        } else if matches(KeyCode::Down, KeyModifiers::SHIFT) {
            true
        } else if matches(KeyCode::Char('m'), KeyModifiers::empty()) {
            if let Some(menu) = self
                .organize_selected_workspace()
                .and_then(|workspace_id| self.organize_workspace_menu(workspace_id))
            {
                self.mode = self.copy_or_terminal_mode();
                self.navigate_workspace_id = None;
                self.run_organize_action(menu, OrganizeAction::Open, outcome);
            }
            return true;
        } else {
            return false;
        };
        outcome.repaint = true;
        let Some(workspace_id) = self.organize_selected_workspace() else {
            return true;
        };
        let Some((group_id, before_workspace_id)) = self
            .snapshot
            .as_deref()
            .and_then(|snapshot| keyboard_place_target(snapshot, &workspace_id, down))
        else {
            return true;
        };
        if let Some(key) = group_id.as_deref().map(group_collapse_key) {
            let endpoint_id = self.active_endpoint_id.clone();
            if self.group_is_collapsed(&endpoint_id, &key) {
                // Entering a collapsed group expands it (client-local).
                self.toggle_collapsed_group(&endpoint_id, key);
                self.persist_chrome_preferences(outcome);
            }
        }
        self.send_organize_ops(
            vec![WorkspaceGroupOp::Place {
                workspace_id,
                group_id,
                before_workspace_id,
            }],
            outcome,
        );
        true
    }

    /// The workspace navigate mode has selected on the active endpoint, else the focused one.
    fn organize_selected_workspace(&self) -> Option<String> {
        self.navigate_workspace_id
            .as_ref()
            .filter(|target| target.endpoint_id == self.active_endpoint_id)
            .map(|target| target.workspace_id.clone())
            .or_else(|| self.snapshot.as_deref()?.focused_workspace_id.clone())
    }

    /// Enter in an Organize prompt.
    pub(super) fn save_organize_prompt(
        &mut self,
        prompt: OrganizePrompt,
        text: &str,
        outcome: &mut ClientShellInput,
    ) {
        outcome.repaint = true;
        let text = text.trim();
        let endpoint_id = match &prompt {
            OrganizePrompt::MoveToGroup { endpoint_id, .. }
            | OrganizePrompt::EditGroup { endpoint_id, .. }
            | OrganizePrompt::RenameGroup { endpoint_id, .. } => endpoint_id,
        };
        if *endpoint_id != self.active_endpoint_id {
            return;
        }
        let Some(snapshot) = self.snapshot.as_deref() else {
            return;
        };
        let named = group_named(snapshot, text).map(|group| group.group_id.clone());
        match prompt {
            OrganizePrompt::MoveToGroup { workspace_id, .. } => {
                let op = match named {
                    _ if text.is_empty() => WorkspaceGroupOp::Place {
                        workspace_id,
                        group_id: None,
                        before_workspace_id: None,
                    },
                    Some(group_id) => WorkspaceGroupOp::Place {
                        workspace_id,
                        group_id: Some(group_id),
                        before_workspace_id: None,
                    },
                    None => WorkspaceGroupOp::Create {
                        name: text.to_owned(),
                        color: None,
                        template: None,
                        workspace_ids: vec![workspace_id],
                        before_group_id: None,
                    },
                };
                self.send_organize_ops(vec![op], outcome);
            }
            OrganizePrompt::EditGroup { .. } => {
                if text.is_empty() {
                    return;
                }
                match named.and_then(|group_id| self.organize_group_menu(group_id)) {
                    Some(menu) => self.run_organize_action(menu, OrganizeAction::Open, outcome),
                    None => {
                        self.push_endpoint_notice(
                            ClientEndpointNoticeKind::Rejected,
                            "workspace_group_not_found",
                            "Action rejected",
                            format!("No group named {text}"),
                        );
                    }
                }
            }
            OrganizePrompt::RenameGroup { group_id, .. } => {
                let unchanged = snapshot
                    .workspace_groups
                    .groups
                    .iter()
                    .any(|group| group.group_id == group_id && group.name == text);
                if !text.is_empty() && !unchanged {
                    self.send_organize_ops(
                        vec![WorkspaceGroupOp::Update {
                            group_id,
                            name: Some(text.to_owned()),
                            color: None,
                            template: None,
                        }],
                        outcome,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_theme::{HostAppearance, RgbColor};

    const WHITE: [u8; 3] = [0xff, 0xff, 0xff];
    const DARK: [u8; 3] = [0x1e, 0x1e, 0x2e];

    fn rgb_of(color: Color) -> [u8; 3] {
        match color {
            Color::Rgb(red, green, blue) => [red, green, blue],
            other => panic!("expected an RGB color, got {other:?}"),
        }
    }

    fn background(cell_bg: Color, host_bg: [u8; 3]) -> [u8; 3] {
        match cell_bg {
            Color::Rgb(red, green, blue) => [red, green, blue],
            _ => host_bg,
        }
    }

    /// CIE L*a*b* (D65) of an sRGB color, for CIE76 color differences.
    fn lab(rgb: [u8; 3]) -> [f64; 3] {
        let linear = |channel: u8| {
            let value = f64::from(channel) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        let [red, green, blue] = rgb.map(linear);
        let x = (0.4124 * red + 0.3576 * green + 0.1805 * blue) / 0.95047;
        let y = 0.2126 * red + 0.7152 * green + 0.0722 * blue;
        let z = (0.0193 * red + 0.1192 * green + 0.9505 * blue) / 1.08883;
        let f = |value: f64| {
            if value > 0.008856 {
                value.cbrt()
            } else {
                7.787 * value + 16.0 / 116.0
            }
        };
        let (fx, fy, fz) = (f(x), f(y), f(z));
        [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
    }

    fn delta_e76(first: [u8; 3], second: [u8; 3]) -> f64 {
        let (first, second) = (lab(first), lab(second));
        first
            .iter()
            .zip(second)
            .map(|(left, right)| (left - right).powi(2))
            .sum::<f64>()
            .sqrt()
    }

    fn min_pairwise_delta_e(colors: &[[u8; 3]]) -> (f64, usize, usize) {
        let mut minimum = (f64::MAX, 0, 0);
        for first in 0..colors.len() {
            for second in first + 1..colors.len() {
                let distance = delta_e76(colors[first], colors[second]);
                if distance < minimum.0 {
                    minimum = (distance, first, second);
                }
            }
        }
        minimum
    }

    // Port of xterm.js `rgba.ensureContrastRatio` (src/common/Color.ts), which VS Code's
    // `terminal.integrated.minimumContrastRatio` uses: xterm's own luminance threshold
    // (0.03928) and its 10% integer steps.
    fn xterm_luminance(rgb: [u8; 3]) -> f64 {
        let channel = |value: u8| {
            let value = f64::from(value) / 255.0;
            if value <= 0.03928 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        channel(rgb[0]) * 0.2126 + channel(rgb[1]) * 0.7152 + channel(rgb[2]) * 0.0722
    }

    fn xterm_contrast(first: f64, second: f64) -> f64 {
        if first < second {
            (second + 0.05) / (first + 0.05)
        } else {
            (first + 0.05) / (second + 0.05)
        }
    }

    fn reduce_luminance(bg: [u8; 3], mut fg: [u8; 3], ratio: f64) -> [u8; 3] {
        let bg_luminance = xterm_luminance(bg);
        while xterm_contrast(xterm_luminance(fg), bg_luminance) < ratio && fg != [0, 0, 0] {
            fg = fg.map(|channel| channel - (f64::from(channel) * 0.1).ceil() as u8);
        }
        fg
    }

    fn increase_luminance(bg: [u8; 3], mut fg: [u8; 3], ratio: f64) -> [u8; 3] {
        let bg_luminance = xterm_luminance(bg);
        while xterm_contrast(xterm_luminance(fg), bg_luminance) < ratio && fg != WHITE {
            fg = fg.map(|channel| {
                channel.saturating_add((f64::from(0xff - channel) * 0.1).ceil() as u8)
            });
        }
        fg
    }

    fn ensure_contrast_ratio(bg: [u8; 3], fg: [u8; 3], ratio: f64) -> [u8; 3] {
        let bg_luminance = xterm_luminance(bg);
        let fg_luminance = xterm_luminance(fg);
        if xterm_contrast(bg_luminance, fg_luminance) >= ratio {
            return fg;
        }
        let contrast = |color: [u8; 3]| xterm_contrast(bg_luminance, xterm_luminance(color));
        let (first, second) = if fg_luminance < bg_luminance {
            (
                reduce_luminance(bg, fg, ratio),
                increase_luminance(bg, fg, ratio),
            )
        } else {
            (
                increase_luminance(bg, fg, ratio),
                reduce_luminance(bg, fg, ratio),
            )
        };
        if contrast(first) >= ratio || contrast(first) > contrast(second) {
            first
        } else {
            second
        }
    }

    fn builtin_palettes() -> Vec<(&'static str, Palette)> {
        crate::config::THEME_NAMES
            .iter()
            .map(|name| (*name, Palette::from_name(name).expect("built-in theme")))
            .collect()
    }

    #[test]
    fn named_colors_at_least_4_5_on_white_and_distinct() {
        let colors = WORKSPACE_COLORS
            .iter()
            .map(|(name, rgb)| {
                assert_eq!(color_rgb(name), Some(*rgb));
                let contrast = contrast_ratio(*rgb, WHITE);
                assert!(contrast >= 4.5, "{name} on white: {contrast:.2}");
                *rgb
            })
            .collect::<Vec<_>>();
        let (distance, first, second) = min_pairwise_delta_e(&colors);
        assert!(
            distance >= 15.0,
            "{} vs {}: {distance:.2}",
            WORKSPACE_COLORS[first].0,
            WORKSPACE_COLORS[second].0
        );
    }

    #[test]
    fn readable_reaches_3_to_1_on_sidebar_and_highlight_backgrounds_for_every_builtin_theme() {
        for (theme, palette) in builtin_palettes() {
            let selection = if palette.selection_bg == Color::Reset {
                palette.active_row_bg
            } else {
                palette.selection_bg
            };
            let cells = [
                ("sidebar_bg", palette.sidebar_bg),
                ("selection", selection),
                ("surface1", palette.surface1),
                ("active_row_bg", palette.active_row_bg),
            ];
            for host_bg in [WHITE, DARK] {
                for (cell, cell_bg) in cells {
                    for (name, rgb) in WORKSPACE_COLORS {
                        let bar = rgb_of(readable(rgb, cell_bg, host_bg));
                        let contrast = contrast_ratio(bar, background(cell_bg, host_bg));
                        assert!(
                            contrast >= 3.0,
                            "{theme} {cell} host {host_bg:?}: {name} -> {bar:?} at {contrast:.2}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn reset_background_uses_reported_host_background_else_appearance() {
        let reported = RgbColor {
            r: 0x10,
            g: 0x20,
            b: 0x30,
        };
        assert_eq!(
            host_background_rgb(Some(reported), Some(HostAppearance::Light)),
            [0x10, 0x20, 0x30]
        );
        assert_eq!(host_background_rgb(None, Some(HostAppearance::Dark)), DARK);
        assert_eq!(
            host_background_rgb(None, Some(HostAppearance::Light)),
            WHITE
        );
        assert_eq!(host_background_rgb(None, None), WHITE);

        // On a white host every color already clears 3:1, so it stays raw.
        for (_, rgb) in WORKSPACE_COLORS {
            assert_eq!(
                readable(rgb, Color::Reset, WHITE),
                Color::Rgb(rgb[0], rgb[1], rgb[2])
            );
        }
        // Gray is 2.15:1 on the dark host: blended toward white behind Reset and behind any
        // named color, while an RGB cell background is used as it is.
        let gray = [0x57, 0x53, 0x4e];
        for cell_bg in [Color::Reset, Color::DarkGray, Color::Indexed(236)] {
            let bar = rgb_of(readable(gray, cell_bg, DARK));
            assert!(relative_luminance(bar) > relative_luminance(gray));
            assert!(contrast_ratio(bar, DARK) >= 3.0);
        }
        assert_eq!(
            readable(gray, Color::Rgb(0xff, 0xff, 0xff), DARK),
            Color::Rgb(gray[0], gray[1], gray[2])
        );
    }

    #[test]
    fn colors_stay_distinct_after_simulated_vscode_min_contrast() {
        let latte = Palette::catppuccin_latte();
        // Spec 9.3: a pairwise CIE76 distance of at least 14 on every background except latte
        // surface1 (dragged rows and drop targets), where it is 13.5: there this palette reaches
        // 13.68 (red vs orange) with readable() applied first, and 13.18 on raw colors.
        let backgrounds = [
            ("white", WHITE, 14.0),
            ("dark", DARK, 14.0),
            ("latte selection_bg", rgb_of(latte.selection_bg), 14.0),
            ("latte active_row_bg", rgb_of(latte.active_row_bg), 14.0),
            ("latte surface1", rgb_of(latte.surface1), 13.5),
        ];
        for (label, bg, floor) in backgrounds {
            let adjusted = WORKSPACE_COLORS
                .iter()
                .map(|(_, color)| {
                    let bar = rgb_of(readable(*color, Color::Rgb(bg[0], bg[1], bg[2]), bg));
                    ensure_contrast_ratio(bg, bar, 4.5)
                })
                .collect::<Vec<_>>();
            let (distance, first, second) = min_pairwise_delta_e(&adjusted);
            assert!(
                distance >= floor,
                "{label}: {} vs {} at {distance:.2}",
                WORKSPACE_COLORS[first].0,
                WORKSPACE_COLORS[second].0
            );
        }
    }

    #[test]
    fn tab_fill_text_contrast_at_least_4_5() {
        for (name, rgb) in WORKSPACE_COLORS {
            let contrast = contrast_ratio(WHITE, rgb);
            assert!(contrast >= 4.5, "white on {name}: {contrast:.2}");
        }
    }
}
