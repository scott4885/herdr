//! Fork-only client shell tests: workspace groups and sidebar tab rows (D3).
//! Compose tests render snapshot -> compose -> cells, colors, hits; snapshots carry the
//! server's group projection directly, so no server is involved.

use super::*;
use crate::api::schema::{Method, ResponseResult, WorkspaceGroupOp};
use crate::client::shell::workspace_groups::{
    color_rgb, group_collapse_key, grouped_entries, host_background_rgb, readable,
};
use crate::protocol::{CellData, ClientShellWorkspaceGroup, ClientShellWorkspaceGroups};
use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

const WIDTH: u16 = 106;

fn workspace(workspace_id: &str, label: &str) -> ClientShellWorkspace {
    let mut workspace = snapshot().workspaces[0].clone();
    workspace.workspace_id = workspace_id.into();
    workspace.active_tab_id = format!("{workspace_id}_tab");
    workspace.label = label.into();
    workspace.branch = None;
    workspace.focused = false;
    workspace
}

fn in_worktree(
    mut workspace: ClientShellWorkspace,
    key: &str,
    linked: bool,
) -> ClientShellWorkspace {
    workspace.worktree = Some(ClientShellWorktree {
        key: key.into(),
        label: key.into(),
        is_linked_worktree: linked,
    });
    workspace
}

fn group(group_id: &str, name: &str, color: &str, members: &[&str]) -> ClientShellWorkspaceGroup {
    ClientShellWorkspaceGroup {
        group_id: group_id.into(),
        name: name.into(),
        color: color.into(),
        template: None,
        workspace_ids: members.iter().map(|member| (*member).to_owned()).collect(),
    }
}

/// The fixture snapshot with `workspaces` (which must keep `ws_1`, the owner of the fixture's
/// tab and pane) and the given group projection. The focused workspace follows the flags.
fn grouped_snapshot(
    workspaces: Vec<ClientShellWorkspace>,
    groups: Vec<ClientShellWorkspaceGroup>,
    colors: &[(&str, &str)],
) -> ClientShellSnapshot {
    let mut snapshot = snapshot();
    snapshot.focused_workspace_id = workspaces
        .iter()
        .find(|workspace| workspace.focused)
        .map(|workspace| workspace.workspace_id.clone());
    snapshot.workspaces = workspaces;
    snapshot.workspace_groups = ClientShellWorkspaceGroups {
        groups,
        workspace_colors: colors
            .iter()
            .map(|(workspace_id, color)| ((*workspace_id).to_owned(), (*color).to_owned()))
            .collect(),
    };
    snapshot
}

fn shell_state(snapshot: ClientShellSnapshot, collapsed: &[&str]) -> ClientShellState {
    let mut state = shell_state_with_config(&Config::default(), snapshot);
    state
        .collapsed_groups
        .extend(collapsed.iter().map(|key| (*key).to_owned()));
    state
}

fn shell_state_with_config(config: &Config, snapshot: ClientShellSnapshot) -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(config));
    state.set_snapshot(Box::new(snapshot));
    state.set_pane_surface(surface());
    state
}

fn composed(
    snapshot: ClientShellSnapshot,
    collapsed: &[&str],
    height: u16,
) -> (ClientShellState, FrameData) {
    let mut state = shell_state(snapshot, collapsed);
    let frame = state.compose(WIDTH, height).expect("composed frame").frame;
    (state, frame)
}

/// Visible workspace cards, top to bottom, as (workspace id, indented).
fn card_order(state: &ClientShellState) -> Vec<(String, bool)> {
    state
        .hits
        .workspaces
        .iter()
        .map(|hit| (hit.workspace_id.clone(), hit.indented))
        .collect()
}

fn entry_shape(
    snapshot: &ClientShellSnapshot,
    entries: &[WorkspaceEntry],
) -> Vec<(String, bool, bool)> {
    entries
        .iter()
        .map(|entry| {
            (
                snapshot.workspaces[entry.index].workspace_id.clone(),
                entry.indented,
                entry.last_child,
            )
        })
        .collect()
}

fn worktree_fixtures() -> Vec<Vec<ClientShellWorkspace>> {
    let mut focused = workspace("ws_1", "one");
    focused.focused = true;
    let mut focused_child = in_worktree(workspace("ws_c2", "child-two"), "repo", true);
    focused_child.focused = true;
    vec![
        vec![
            focused.clone(),
            workspace("ws_2", "two"),
            workspace("ws_3", "three"),
        ],
        vec![
            in_worktree(focused.clone(), "repo", false),
            in_worktree(workspace("ws_c1", "child-one"), "repo", true),
            workspace("ws_plain", "plain"),
            in_worktree(workspace("ws_c2", "child-two"), "repo", true),
        ],
        vec![
            in_worktree(focused.clone(), "repo", false),
            in_worktree(workspace("ws_dup", "duplicate"), "repo", false),
            in_worktree(workspace("ws_c1", "child-one"), "repo", true),
        ],
        vec![
            in_worktree(workspace("ws_c1", "child-one"), "repo", true),
            in_worktree(focused.clone(), "other", false),
            in_worktree(workspace("ws_p", "parent"), "repo", false),
            in_worktree(workspace("ws_o1", "other-child"), "other", true),
            workspace("ws_plain", "plain"),
            in_worktree(workspace("ws_o2", "other-child-2"), "other", true),
        ],
        vec![
            focused.clone(),
            in_worktree(workspace("ws_orphan", "orphan"), "lonely", true),
            in_worktree(workspace("ws_solo", "solo"), "single", false),
            in_worktree(workspace("ws_solo2", "solo-two"), "single", false),
        ],
        vec![
            in_worktree(workspace("ws_1", "one"), "repo", false),
            in_worktree(workspace("ws_c1", "child-one"), "repo", true),
            focused_child,
            in_worktree(workspace("ws_p2", "parent-two"), "other", false),
            in_worktree(workspace("ws_o1", "other-child"), "other", true),
        ],
    ]
}

#[test]
fn grouped_entries_match_upstream_for_single_region() {
    for workspaces in worktree_fixtures() {
        let upstream_snapshot = grouped_snapshot(workspaces.clone(), Vec::new(), &[]);
        let grouped = grouped_snapshot(workspaces, vec![group("g1", "Empty", "blue", &[])], &[]);
        let keys = upstream_snapshot
            .workspaces
            .iter()
            .filter_map(|workspace| {
                workspace
                    .worktree
                    .as_ref()
                    .map(|worktree| worktree.key.clone())
            })
            .collect::<HashSet<_>>();
        let mut collapse_states = vec![HashSet::new(), keys.clone()];
        collapse_states.extend(keys.iter().map(|key| HashSet::from([key.clone()])));
        for collapsed in collapse_states {
            let upstream = render::workspace_entries(&upstream_snapshot, &collapsed);
            let fork = grouped_entries(&grouped, &collapsed).expect("groups present");
            assert_eq!(
                entry_shape(&grouped, &fork),
                entry_shape(&upstream_snapshot, &upstream),
                "collapsed {collapsed:?}"
            );
        }
    }
}

#[test]
fn display_order_buckets_by_group_even_when_vec_order_drifted() {
    let mut focused = workspace("ws_1", "one");
    focused.focused = true;
    let snapshot = grouped_snapshot(
        vec![
            focused,
            workspace("ws_2", "two"),
            workspace("ws_3", "three"),
            workspace("ws_4", "four"),
            workspace("ws_5", "five"),
        ],
        vec![
            group("gA", "Work", "blue", &["ws_3", "ws_4"]),
            group("gB", "Personal", "green", &["ws_2"]),
        ],
        &[],
    );
    assert!(grouped_entries(&snapshot, &HashSet::new()).is_some());
    let (state, _) = composed(snapshot, &[], 30);
    assert_eq!(
        card_order(&state),
        [
            ("ws_3".to_owned(), false),
            ("ws_4".to_owned(), false),
            ("ws_2".to_owned(), false),
            ("ws_1".to_owned(), false),
            ("ws_5".to_owned(), false),
        ]
    );
}

#[test]
fn multi_parent_family_across_groups_renders_each_parent_in_its_group() {
    let mut parent = in_worktree(workspace("ws_1", "parent-one"), "repo", false);
    parent.focused = true;
    let snapshot = grouped_snapshot(
        vec![
            parent,
            in_worktree(workspace("ws_2", "parent-two"), "repo", false),
            in_worktree(workspace("ws_3", "child"), "repo", true),
        ],
        vec![
            group("gA", "Work", "blue", &["ws_1", "ws_3"]),
            group("gB", "Personal", "green", &["ws_2"]),
        ],
        &[],
    );
    let (state, _) = composed(snapshot, &[], 30);
    assert_eq!(
        card_order(&state),
        [
            ("ws_1".to_owned(), false),
            ("ws_3".to_owned(), true),
            ("ws_2".to_owned(), false),
        ]
    );
}

#[test]
fn split_family_child_renders_top_level_and_ignores_worktree_collapse() {
    let mut parent = in_worktree(workspace("ws_1", "parent"), "repo", false);
    parent.focused = true;
    let snapshot = grouped_snapshot(
        vec![
            parent,
            in_worktree(workspace("ws_2", "child"), "repo", true),
            workspace("ws_3", "plain"),
        ],
        vec![
            group("gA", "Work", "blue", &["ws_1"]),
            group("gB", "Personal", "green", &["ws_2"]),
        ],
        &[],
    );
    for collapsed in [&[][..], &["repo"][..]] {
        let (state, _) = composed(snapshot.clone(), collapsed, 30);
        assert_eq!(
            card_order(&state),
            [
                ("ws_1".to_owned(), false),
                ("ws_2".to_owned(), false),
                ("ws_3".to_owned(), false),
            ],
            "collapsed {collapsed:?}"
        );
    }
}

#[test]
fn navigation_skips_collapsed_group_members() {
    let mut focused = workspace("ws_2", "two");
    focused.focused = true;
    let snapshot = grouped_snapshot(
        vec![
            workspace("ws_1", "one"),
            focused,
            workspace("ws_3", "three"),
            workspace("ws_4", "four"),
            workspace("ws_5", "five"),
        ],
        vec![
            group("gA", "Work", "blue", &["ws_1", "ws_2"]),
            group("gB", "Personal", "green", &["ws_3", "ws_4"]),
        ],
        &[],
    );
    let key = group_collapse_key("gB");
    let mut state = shell_state(snapshot, &[key.as_str()]);
    state.compose(WIDTH, 30).expect("composed frame");
    assert_eq!(
        card_order(&state),
        [
            ("ws_1".to_owned(), false),
            ("ws_2".to_owned(), false),
            ("ws_5".to_owned(), false),
        ]
    );
    for (action, expected) in [
        (crate::input::KeybindAction::NextWorkspace, "ws_5"),
        (crate::input::KeybindAction::SwitchWorkspace(2), "ws_5"),
        (crate::input::KeybindAction::PreviousWorkspace, "ws_1"),
    ] {
        let mut outcome = ClientShellInput::default();
        state.record_binding(crate::input::KeybindMatch::Action(action), &mut outcome);
        assert!(
            matches!(
                &outcome.actions[..],
                [ClientShellAction::Endpoint { request, .. }]
                    if matches!(
                        &request.method,
                        crate::api::schema::Method::WorkspaceFocus(target)
                            if target.workspace_id == expected
                    )
            ),
            "{action:?}"
        );
    }
}

fn cell(frame: &FrameData, x: u16, y: u16) -> &crate::protocol::CellData {
    &frame.cells[usize::from(y) * usize::from(frame.width) + usize::from(x)]
}

fn row_text(frame: &FrameData, rect: Rect) -> String {
    frame_rows(frame)[usize::from(rect.y)]
        .chars()
        .skip(usize::from(rect.x))
        .take(usize::from(rect.width))
        .collect()
}

fn card(state: &ClientShellState, workspace_id: &str) -> Rect {
    state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == workspace_id)
        .unwrap_or_else(|| panic!("{workspace_id} card is visible"))
        .rect
}

fn host_bg(state: &ClientShellState) -> [u8; 3] {
    host_background_rgb(state.host_background, state.host_appearance)
}

fn rgb(name: &str) -> [u8; 3] {
    color_rgb(name).expect("palette color")
}

fn u32_color(color: ratatui::style::Color) -> u32 {
    crate::protocol::color_to_u32(color)
}

fn is_bold(cell: &crate::protocol::CellData) -> bool {
    cell.modifier & Modifier::BOLD.bits() != 0
}

#[test]
fn group_headers_render_bar_name_toggle_in_group_order_with_ungrouped_last() {
    let mut focused = workspace("ws_1", "alpha");
    focused.focused = true;
    let snapshot = grouped_snapshot(
        vec![
            focused,
            workspace("ws_2", "beta"),
            workspace("ws_3", "gamma"),
            workspace("ws_4", "delta"),
        ],
        vec![
            group("gA", "Work", "blue", &["ws_1", "ws_4"]),
            group("gB", "Personal", "green", &["ws_3"]),
        ],
        &[("ws_1", "blue"), ("ws_4", "blue"), ("ws_3", "green")],
    );
    let (state, frame) = composed(snapshot, &[], 30);
    let palette = &state.config.palette;
    let headers = &state.hits.workspace_groups;
    assert_eq!(
        headers
            .iter()
            .map(|header| header.group_id.as_deref())
            .collect::<Vec<_>>(),
        [Some("gA"), Some("gB"), None]
    );
    assert!(headers
        .iter()
        .all(|header| header.endpoint_id == ClientEndpointId::Local && !header.collapsed));
    let rows = [
        headers[0].rect.y,
        card(&state, "ws_1").y,
        card(&state, "ws_4").y,
        headers[1].rect.y,
        card(&state, "ws_3").y,
        headers[2].rect.y,
        card(&state, "ws_2").y,
    ];
    assert!(rows.windows(2).all(|pair| pair[0] < pair[1]), "{rows:?}");
    assert_eq!(headers[0].rect.y + 1, card(&state, "ws_1").y);

    for (header, name, color) in [
        (headers[0].rect, "Work", "blue"),
        (headers[1].rect, "Personal", "green"),
    ] {
        let text = row_text(&frame, header);
        assert!(text.starts_with(&format!("▌ {name}")), "{text:?}");
        assert!(text.trim_end().ends_with('▾'), "{text:?}");
        let bar = cell(&frame, header.x, header.y);
        assert_eq!(bar.symbol, "▌");
        assert_eq!(
            bar.fg,
            u32_color(readable(rgb(color), palette.sidebar_bg, host_bg(&state)))
        );
        let name_cell = cell(&frame, header.x + 2, header.y);
        assert_eq!(name_cell.fg, u32_color(palette.text));
        assert!(is_bold(name_cell));
        let toggle = cell(&frame, header.right() - 1, header.y);
        assert_eq!(toggle.symbol, "▾");
        assert_eq!(toggle.fg, u32_color(palette.accent));
    }

    let ungrouped = headers[2].rect;
    let text = row_text(&frame, ungrouped);
    assert_eq!(text.trim_end(), "  ungrouped");
    assert_eq!(cell(&frame, ungrouped.x, ungrouped.y).symbol, " ");
    assert_eq!(
        cell(&frame, ungrouped.x + 2, ungrouped.y).fg,
        u32_color(palette.overlay0)
    );
}

#[test]
fn collapsed_group_hides_members_keeps_focused_and_rolls_up_status() {
    let mut focused = workspace("ws_1", "alpha");
    focused.focused = true;
    let mut blocked = workspace("ws_2", "beta");
    blocked.agent_status = AgentStatus::Blocked;
    let snapshot = grouped_snapshot(
        vec![
            focused,
            blocked,
            workspace("ws_3", "gamma"),
            workspace("ws_4", "delta"),
            workspace("ws_5", "epsilon"),
            workspace("ws_6", "zeta"),
        ],
        vec![
            group("gA", "Work", "blue", &["ws_1", "ws_2", "ws_3"]),
            group("gB", "Personal", "green", &["ws_4", "ws_5"]),
        ],
        &[],
    );
    let (key_a, key_b) = (group_collapse_key("gA"), group_collapse_key("gB"));
    let (state, frame) = composed(snapshot.clone(), &[key_a.as_str(), key_b.as_str()], 30);
    let palette = &state.config.palette;
    assert_eq!(
        card_order(&state),
        [("ws_1".to_owned(), false), ("ws_6".to_owned(), false)]
    );
    let headers = &state.hits.workspace_groups;
    assert_eq!(
        headers
            .iter()
            .map(|header| (header.group_id.as_deref(), header.collapsed))
            .collect::<Vec<_>>(),
        [(Some("gA"), true), (Some("gB"), true), (None, false)]
    );
    assert_eq!(headers[0].rect.y + 1, card(&state, "ws_1").y);
    assert_eq!(headers[1].rect.y + 1, headers[2].rect.y);
    assert_eq!(headers[2].rect.y + 1, card(&state, "ws_6").y);

    let work = headers[0].rect;
    let text = row_text(&frame, work);
    let icon = status_icon(AgentStatus::Blocked, state.config.status_indicators);
    assert!(text.starts_with(&format!("▌ Work (3) {icon}")), "{text:?}");
    assert!(text.trim_end().ends_with('▸'), "{text:?}");
    let icon_x = work.x + 2 + "Work (3) ".chars().count() as u16;
    assert_eq!(cell(&frame, icon_x, work.y).fg, u32_color(palette.red));
    assert!(row_text(&frame, headers[1].rect).starts_with("▌ Personal (2)"));

    // A focused linked child of a collapsed group is emitted un-indented.
    let parent = in_worktree(workspace("ws_1", "parent"), "repo", false);
    let mut child = in_worktree(workspace("ws_2", "child"), "repo", true);
    child.focused = true;
    let linked = grouped_snapshot(
        vec![parent, child],
        vec![group("gA", "Work", "blue", &["ws_1", "ws_2"])],
        &[],
    );
    let entries = grouped_entries(&linked, &HashSet::from([key_a.clone()])).expect("groups");
    assert_eq!(
        entry_shape(&linked, &entries),
        [("ws_2".to_owned(), false, false)]
    );
}

#[test]
fn empty_and_collapsed_groups_stack_headers() {
    let mut focused = workspace("ws_1", "alpha");
    focused.focused = true;
    let snapshot = grouped_snapshot(
        vec![
            focused.clone(),
            workspace("ws_2", "beta"),
            workspace("ws_3", "gamma"),
        ],
        vec![
            group("gE", "Empty", "gray", &[]),
            group("gC", "Closed", "red", &["ws_2"]),
            group("gM", "Main", "blue", &["ws_1", "ws_3"]),
        ],
        &[],
    );
    let key = group_collapse_key("gC");
    let (state, _) = composed(snapshot, &[key.as_str()], 30);
    let headers = &state.hits.workspace_groups;
    assert_eq!(
        headers
            .iter()
            .map(|header| header.group_id.as_deref())
            .collect::<Vec<_>>(),
        [Some("gE"), Some("gC"), Some("gM")]
    );
    assert_eq!(headers[0].rect.y, state.hits.workspace_body.y);
    assert_eq!(headers[0].rect.y + 1, headers[1].rect.y);
    assert_eq!(headers[1].rect.y + 1, headers[2].rect.y);
    assert_eq!(headers[2].rect.y + 1, card(&state, "ws_1").y);

    // Empty groups after the last visible entry trail below it.
    let trailing = grouped_snapshot(
        vec![focused.clone(), workspace("ws_2", "beta")],
        vec![
            group("gM", "Main", "blue", &["ws_1", "ws_2"]),
            group("gE", "Empty", "gray", &[]),
            group("gF", "Further", "teal", &[]),
        ],
        &[],
    );
    let (state, _) = composed(trailing, &[], 30);
    let headers = &state.hits.workspace_groups;
    assert_eq!(headers.len(), 3);
    assert_eq!(headers[1].rect.y, card(&state, "ws_2").bottom());
    assert_eq!(headers[2].rect.y, headers[1].rect.y + 1);

    // With no visible entry at all, headers start at the top of the list.
    let mut hidden = focused;
    hidden.focused = false;
    let empty = grouped_snapshot(
        vec![hidden],
        vec![
            group("gC", "Closed", "red", &["ws_1"]),
            group("gE", "Empty", "gray", &[]),
        ],
        &[],
    );
    let key = group_collapse_key("gC");
    let (state, _) = composed(empty, &[key.as_str()], 30);
    assert!(state.hits.workspaces.is_empty());
    let headers = &state.hits.workspace_groups;
    assert_eq!(headers.len(), 2);
    assert_eq!(headers[0].rect.y, state.hits.workspace_body.y);
    assert_eq!(headers[1].rect.y, headers[0].rect.y + 1);
}

#[test]
fn reveal_focused_workspace_accounts_for_header_rows() {
    let mut workspaces = Vec::new();
    let mut groups = Vec::new();
    for number in 1..=10 {
        let workspace_id = format!("ws_{number}");
        let mut member = workspace(&workspace_id, &format!("space-{number}"));
        member.focused = number == 10;
        workspaces.push(member);
        groups.push(group(
            &format!("g{number}"),
            &format!("Group {number}"),
            "blue",
            &[workspace_id.as_str()],
        ));
    }
    let snapshot = grouped_snapshot(workspaces, groups, &[]);
    let (state, _) = composed(snapshot, &[], 24);
    let body = state.hits.workspace_body;
    assert!(
        body.height < 20,
        "the fixture must overflow the workspace list: {body:?}"
    );
    let focused = card(&state, "ws_10");
    assert!(focused.bottom() <= body.bottom(), "{focused:?} in {body:?}");
    let header = state
        .hits
        .workspace_groups
        .iter()
        .find(|header| header.group_id.as_deref() == Some("g10"))
        .expect("focused workspace's header is visible");
    assert_eq!(header.rect.y + 1, focused.y);
}

#[test]
fn no_groups_renders_identically_to_upstream() {
    let mut parent = in_worktree(workspace("ws_1", "parent"), "repo", false);
    parent.focused = true;
    parent.branch = Some("main".into());
    let explicit = grouped_snapshot(
        vec![
            parent,
            in_worktree(workspace("ws_2", "child"), "repo", true),
            workspace("ws_3", "plain"),
        ],
        Vec::new(),
        &[],
    );
    // An upstream server sends no `workspace_groups` field at all.
    let mut json = serde_json::to_value(&explicit).expect("snapshot json");
    json.as_object_mut()
        .expect("snapshot object")
        .remove("workspace_groups");
    let upstream: ClientShellSnapshot = serde_json::from_value(json).expect("upstream snapshot");
    assert_eq!(
        upstream.workspace_groups,
        ClientShellWorkspaceGroups::default()
    );
    // A color projected for a workspace that is not live takes the per-workspace color path
    // (no early return) but must still draw exactly what the empty projection draws.
    let mut stale_color = upstream.clone();
    stale_color.workspace_groups.workspace_colors = vec![("ws_gone".into(), "teal".into())];
    for collapsed in [&[][..], &["repo"][..]] {
        let collapsed_set = collapsed
            .iter()
            .map(|key| (*key).to_owned())
            .collect::<HashSet<_>>();
        assert!(grouped_entries(&upstream, &collapsed_set).is_none());
        let (upstream_state, upstream_frame) = composed(upstream.clone(), collapsed, 30);
        let (state, frame) = composed(stale_color.clone(), collapsed, 30);
        assert_eq!(frame.cells, upstream_frame.cells);
        assert!(upstream_frame.cells.iter().all(|cell| cell.symbol != "▌"));
        for state in [&state, &upstream_state] {
            assert!(state.hits.workspace_groups.is_empty());
            assert!(state.hits.workspace_drop_slots.is_empty());
        }
        assert_eq!(card_order(&state), card_order(&upstream_state));
    }
}

#[test]
fn single_tab_workspace_differs_only_by_color_bar() {
    let mut focused = workspace("ws_1", "alpha");
    focused.focused = true;
    focused.branch = Some("main".into());
    let plain = grouped_snapshot(vec![focused, workspace("ws_2", "beta")], Vec::new(), &[]);
    assert_eq!(
        plain
            .tabs
            .iter()
            .filter(|tab| tab.workspace_id == "ws_1")
            .count(),
        1
    );
    let mut colored = plain.clone();
    colored.workspace_groups.workspace_colors = vec![("ws_1".into(), "teal".into())];
    let (_, plain_frame) = composed(plain, &[], 30);
    let (state, frame) = composed(colored, &[], 30);
    let rect = card(&state, "ws_1");
    assert_eq!(rect.height, 2);
    // The sidebar card; the tab bar's focused-tab fill is covered by its own test.
    let sidebar = state.layout(WIDTH, 30).sidebar;
    let differing = plain_frame
        .cells
        .iter()
        .zip(&frame.cells)
        .enumerate()
        .filter(|(_, (before, after))| before != after)
        .map(|(index, _)| {
            (
                (index % usize::from(frame.width)) as u16,
                (index / usize::from(frame.width)) as u16,
            )
        })
        .filter(|position| contains(sidebar, *position))
        .collect::<Vec<_>>();
    assert_eq!(differing, [(rect.x, rect.y), (rect.x, rect.y + 1)]);
    for y in rect.y..rect.bottom() {
        let before = cell(&plain_frame, rect.x, y);
        let after = cell(&frame, rect.x, y);
        assert_eq!(before.symbol, " ");
        assert_eq!(after.symbol, "▌");
        assert_eq!(after.bg, before.bg);
        assert_eq!(after.modifier, before.modifier);
    }
}

#[test]
fn member_rows_and_tab_rows_paint_effective_color_bar() {
    // Every line of a card gets the bar, down to its card_bottom: the label rows of grouped,
    // overridden, nested and uncolored cards, and the tab rows of 2+ tab workspaces.
    let mut focused = workspace("ws_1", "alpha");
    focused.focused = true;
    focused.branch = Some("main".into());
    let mut snapshot = grouped_snapshot(
        vec![
            focused,
            workspace("ws_2", "beta"),
            in_worktree(workspace("ws_3", "parent"), "repo", false),
            in_worktree(workspace("ws_4", "child"), "repo", true),
            workspace("ws_5", "loose"),
        ],
        vec![group(
            "gA",
            "Work",
            "blue",
            &["ws_1", "ws_2", "ws_3", "ws_4"],
        )],
        &[
            ("ws_1", "teal"),
            ("ws_2", "blue"),
            ("ws_3", "blue"),
            ("ws_4", "blue"),
        ],
    );
    for workspace_id in ["ws_2", "ws_4", "ws_5"] {
        snapshot.tabs.push(tab(
            workspace_id,
            &format!("{workspace_id}_tab"),
            1,
            "editor",
        ));
        snapshot.tabs.push(tab(
            workspace_id,
            &format!("{workspace_id}_logs"),
            2,
            "logs",
        ));
    }
    let (state, frame) = composed(snapshot, &[], 44);
    let palette = &state.config.palette;
    for (workspace_id, color, background, tab_rows) in [
        ("ws_1", "teal", palette.active_row_bg, 0),
        ("ws_2", "blue", palette.sidebar_bg, 2),
        ("ws_3", "blue", palette.sidebar_bg, 0),
        ("ws_4", "blue", palette.sidebar_bg, 2),
    ] {
        let hit = workspace_hit(&state, workspace_id);
        assert_eq!(
            hit.card_bottom,
            hit.rect.bottom() + tab_rows,
            "{workspace_id}"
        );
        for y in hit.rect.y..hit.card_bottom {
            let bar = cell(&frame, hit.rect.x, y);
            assert_eq!(bar.symbol, "▌", "{workspace_id} row {y}");
            assert_eq!(bar.bg, u32_color(background), "{workspace_id} row {y}");
            assert_eq!(
                bar.fg,
                u32_color(readable(rgb(color), background, host_bg(&state))),
                "{workspace_id} row {y}"
            );
        }
    }
    assert_eq!(card(&state, "ws_1").height, 2);
    let loose = workspace_hit(&state, "ws_5");
    assert_eq!(loose.card_bottom, loose.rect.bottom() + 2);
    for y in loose.rect.y..loose.card_bottom {
        assert_eq!(cell(&frame, loose.rect.x, y).symbol, " ", "ws_5 row {y}");
    }

    // Multi-machine sidebar: cards are nested two columns under their machine, and the bar
    // in the card's column 0 covers a remote card's tab rows as well.
    let mut remote_focused = workspace("ws_1", "alpha");
    remote_focused.focused = true;
    let mut remote = grouped_snapshot(
        vec![remote_focused, workspace("ws_2", "remote-beta")],
        vec![group("gR", "Remote work", "green", &["ws_2"])],
        &[("ws_2", "green")],
    );
    remote.tabs.push(tab("ws_2", "ws_2_tab", 1, "editor"));
    remote.tabs.push(tab("ws_2", "ws_2_logs", 2, "logs"));
    let (mut state, remote_id) = state_with_remote_snapshots(super::snapshot(), remote);
    let frame = state.compose(WIDTH, 40).expect("composed frame").frame;
    let hit = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.endpoint_id == remote_id && hit.workspace_id == "ws_2")
        .expect("remote two-tab card");
    assert_eq!(hit.card_bottom, hit.rect.bottom() + 2);
    let background = state.config.palette.sidebar_bg;
    for y in hit.rect.y..hit.card_bottom {
        let bar = cell(&frame, hit.rect.x + 2, y);
        assert_eq!(bar.symbol, "▌", "remote row {y}");
        assert_eq!(bar.bg, u32_color(background), "remote row {y}");
        assert_eq!(
            bar.fg,
            u32_color(readable(rgb("green"), background, host_bg(&state))),
            "remote row {y}"
        );
    }
}

#[test]
fn trailing_headers_and_end_slots_sit_below_tab_rows() {
    let mut focused = workspace("ws_1", "alpha");
    focused.focused = true;
    let mut snapshot = grouped_snapshot(
        vec![focused, workspace("ws_2", "beta")],
        vec![
            group("gA", "Work", "blue", &["ws_1", "ws_2"]),
            group("gE", "Empty", "gray", &[]),
        ],
        &[("ws_1", "blue"), ("ws_2", "blue")],
    );
    snapshot.tabs.push(tab("ws_2", "ws_2_tab", 1, "editor"));
    snapshot.tabs.push(tab("ws_2", "ws_2_logs", 2, "logs"));
    let (state, frame) = composed(snapshot.clone(), &[], 30);
    let first = card(&state, "ws_1");
    let beta = workspace_hit(&state, "ws_2");
    assert_eq!(
        sidebar_tab_rows(&state),
        [
            ("ws_2_tab", beta.rect.bottom()),
            ("ws_2_logs", beta.rect.bottom() + 1),
        ]
    );
    assert_eq!(beta.card_bottom, beta.rect.bottom() + 2);
    let headers = &state.hits.workspace_groups;
    assert_eq!(headers.len(), 2);
    let (work, empty) = (headers[0].rect.y, headers[1].rect);
    // The empty group's header trails below the last card's tab rows, and the region's end
    // slot sits on the card's last line, its last tab row.
    assert_eq!(empty.y, beta.card_bottom);
    assert!(row_text(&frame, empty).starts_with("▌ Empty"));
    assert_eq!(
        drop_slots(&state),
        [
            (work, Some("gA"), Some("ws_1")),
            (first.bottom() - 1, Some("gA"), Some("ws_2")),
            (beta.card_bottom - 1, Some("gA"), None),
            (empty.y, Some("gE"), None),
        ]
    );

    // A card is drawn only when its header rows, label rows and tab rows all fit the body.
    for height in 10..=30 {
        let (state, _) = composed(snapshot.clone(), &[], height);
        let body = state.hits.workspace_body;
        for hit in &state.hits.workspaces {
            assert!(
                hit.card_bottom <= body.bottom(),
                "height {height}: {} ends at {} below {body:?}",
                hit.workspace_id,
                hit.card_bottom
            );
        }
        assert!(
            state
                .hits
                .sidebar_tabs
                .iter()
                .all(|(rect, _, _)| rect.bottom() <= body.bottom()),
            "height {height}"
        );
        assert!(
            state
                .hits
                .workspace_groups
                .iter()
                .all(|header| header.rect.y < body.bottom()),
            "height {height}"
        );
    }
}

#[test]
fn group_color_never_used_for_text_cells() {
    let mut focused = workspace("ws_1", "alpha");
    focused.focused = true;
    focused.branch = Some("main".into());
    let snapshot = grouped_snapshot(
        vec![
            focused,
            workspace("ws_2", "beta"),
            workspace("ws_3", "gamma"),
        ],
        vec![
            group("gA", "Work", "purple", &["ws_1"]),
            group("gB", "Personal", "orange", &["ws_2"]),
        ],
        &[("ws_1", "pink"), ("ws_2", "orange")],
    );
    let (state, frame) = composed(snapshot, &[], 30);
    let mut bar_colors = crate::protocol::WORKSPACE_COLORS
        .iter()
        .map(|(_, [red, green, blue])| u32_color(ratatui::style::Color::Rgb(*red, *green, *blue)))
        .collect::<HashSet<_>>();
    bar_colors.extend(
        frame
            .cells
            .iter()
            .filter(|cell| cell.symbol == "▌")
            .map(|cell| cell.fg),
    );
    assert!(frame.cells.iter().filter(|cell| cell.symbol == "▌").count() >= 4);
    let focused_tab = state
        .hits
        .tabs
        .iter()
        .find(|(_, tab_id)| tab_id == "tab_1")
        .map(|(rect, _)| *rect);
    for (index, cell) in frame.cells.iter().enumerate() {
        let position = (
            (index % usize::from(frame.width)) as u16,
            (index / usize::from(frame.width)) as u16,
        );
        if cell.symbol != "▌" {
            assert!(
                !bar_colors.contains(&cell.fg),
                "text cell {position:?} {:?} uses a group color",
                cell.symbol
            );
        }
        if !focused_tab.is_some_and(|tab| contains(tab, position)) {
            assert!(
                !bar_colors.contains(&cell.bg),
                "cell {position:?} is filled with a group color"
            );
        }
    }
}

#[test]
fn unknown_color_name_draws_no_bar() {
    assert_eq!(color_rgb("magenta"), None);
    assert_eq!(color_rgb(""), None);
    assert_eq!(color_rgb(" Teal "), Some([0x0f, 0x76, 0x6e]));
    let mut focused = workspace("ws_1", "alpha");
    focused.focused = true;
    let snapshot = grouped_snapshot(
        vec![focused, workspace("ws_2", "beta")],
        vec![group("gA", "Work", "magenta", &["ws_1", "ws_2"])],
        &[("ws_1", "magenta"), ("ws_2", "#ff0000")],
    );
    let (state, frame) = composed(snapshot, &[], 30);
    assert!(frame.cells.iter().all(|cell| cell.symbol != "▌"));
    let header = state.hits.workspace_groups[0].rect;
    assert!(row_text(&frame, header).starts_with("  Work"));
}

#[test]
fn workspace_group_drop_slots_cover_headers_cards_and_region_ends() {
    let mut focused = workspace("ws_1", "alpha");
    focused.focused = true;
    let snapshot = grouped_snapshot(
        vec![
            focused,
            workspace("ws_2", "beta"),
            workspace("ws_3", "gamma"),
        ],
        vec![
            group("gA", "Work", "blue", &["ws_1", "ws_2"]),
            group("gE", "Empty", "gray", &[]),
        ],
        &[],
    );
    let (state, _) = composed(snapshot, &[], 30);
    let headers = &state.hits.workspace_groups;
    let (work, empty, ungrouped) = (headers[0].rect.y, headers[1].rect.y, headers[2].rect.y);
    let (first, second, third) = (
        card(&state, "ws_1"),
        card(&state, "ws_2"),
        card(&state, "ws_3"),
    );
    let slots = state
        .hits
        .workspace_drop_slots
        .iter()
        .map(|slot| {
            assert_eq!(slot.endpoint_id, ClientEndpointId::Local);
            (
                slot.row,
                slot.group_id.as_deref(),
                slot.before_workspace_id.as_deref(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        slots,
        [
            (work, Some("gA"), Some("ws_1")),
            (first.bottom() - 1, Some("gA"), Some("ws_2")),
            (second.bottom() - 1, Some("gA"), None),
            (empty, Some("gE"), None),
            (ungrouped, None, Some("ws_3")),
            (third.bottom(), None, None),
        ]
    );
}

/// Recorded drop slots, top to bottom, as (row, group id, before workspace id).
fn drop_slots(state: &ClientShellState) -> Vec<(u16, Option<&str>, Option<&str>)> {
    state
        .hits
        .workspace_drop_slots
        .iter()
        .map(|slot| {
            (
                slot.row,
                slot.group_id.as_deref(),
                slot.before_workspace_id.as_deref(),
            )
        })
        .collect()
}

#[test]
fn workspace_group_end_slots_use_the_row_gap_and_stay_inside_the_list() {
    let mut focused = workspace("ws_1", "alpha");
    focused.focused = true;
    let snapshot = grouped_snapshot(
        vec![
            focused,
            workspace("ws_2", "beta"),
            workspace("ws_3", "gamma"),
        ],
        vec![group("gA", "Work", "blue", &["ws_1", "ws_2"])],
        &[],
    );

    // With a row gap, a region's end slot takes the blank row above the next header.
    let mut config = Config::default();
    config.ui.sidebar.spaces.row_gap = 1;
    let mut state = shell_state_with_config(&config, snapshot.clone());
    state.compose(WIDTH, 30).expect("composed frame");
    let headers = &state.hits.workspace_groups;
    let (work, ungrouped) = (headers[0].rect.y, headers[1].rect.y);
    let (first, second, third) = (
        card(&state, "ws_1"),
        card(&state, "ws_2"),
        card(&state, "ws_3"),
    );
    assert_eq!(second.y, first.bottom() + 1);
    assert_eq!(ungrouped, second.bottom() + 1);
    assert!(third.bottom() < state.hits.workspace_body.bottom());
    assert_eq!(
        drop_slots(&state),
        [
            (work, Some("gA"), Some("ws_1")),
            (first.bottom(), Some("gA"), Some("ws_2")),
            (second.bottom(), Some("gA"), None),
            (ungrouped, None, Some("ws_3")),
            (third.bottom(), None, None),
        ]
    );

    // No slot lands on the footer row below the list body: when the last card ends on the
    // body's last row, the end slot sits on the card's last line.
    let mut footer_case = false;
    for height in 12..=40 {
        let mut state = shell_state(snapshot.clone(), &[]);
        state.compose(WIDTH, height).expect("composed frame");
        let body = state.hits.workspace_body;
        let slots = drop_slots(&state);
        assert!(
            slots.iter().all(|(row, _, _)| *row < body.bottom()),
            "height {height}: {slots:?} below {body:?}"
        );
        let Some(last) = state
            .hits
            .workspaces
            .last()
            .filter(|hit| hit.workspace_id == "ws_3" && hit.rect.bottom() == body.bottom())
        else {
            continue;
        };
        footer_case = true;
        assert_eq!(
            slots.last(),
            Some(&(last.rect.bottom() - 1, None, None)),
            "height {height}"
        );
    }
    assert!(
        footer_case,
        "some height must end the last card on the footer"
    );
}

#[test]
fn workspace_drag_shows_ungrouped_header_without_ungrouped_workspaces() {
    let mut focused = workspace("ws_1", "alpha");
    focused.focused = true;
    let snapshot = grouped_snapshot(
        vec![focused, workspace("ws_2", "beta")],
        vec![group("gA", "Work", "blue", &["ws_1", "ws_2"])],
        &[],
    );
    let mut state = shell_state(snapshot, &[]);
    state.compose(WIDTH, 30).expect("composed frame");
    let header_ids = |state: &ClientShellState| {
        state
            .hits
            .workspace_groups
            .iter()
            .map(|header| header.group_id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(header_ids(&state), [Some("gA".to_owned())]);
    let second = card(&state, "ws_2");
    assert_eq!(
        drop_slots(&state).last(),
        Some(&(second.bottom(), Some("gA"), None))
    );

    state.chrome_drag = Some(ClientChromeDrag::Workspace {
        source_workspace_id: "ws_2".into(),
        target: None,
    });
    let frame = state.compose(WIDTH, 30).expect("composed frame").frame;
    assert_eq!(header_ids(&state), [Some("gA".to_owned()), None]);
    let second = card(&state, "ws_2");
    let ungrouped = state.hits.workspace_groups[1].rect;
    assert_eq!(ungrouped.y, second.bottom());
    assert_eq!(row_text(&frame, ungrouped).trim_end(), "  ungrouped");
    let slots = drop_slots(&state);
    assert_eq!(
        slots[slots.len() - 2..],
        [
            (second.bottom() - 1, Some("gA"), None),
            (ungrouped.y, None, None),
        ]
    );
}

// Replicates the private `remote_profile` / `state_with_remote` fixtures of tests/endpoints.rs
// (copied, not shared, so that upstream file stays untouched), with caller-provided snapshots.
fn remote_profile_fixture() -> SavedSshEndpoint {
    SavedSshEndpoint {
        id: crate::client::endpoint::ProfileId::parse("0123456789abcdef0123456789abcdef")
            .expect("profile id"),
        label: "Build".into(),
        target: "dev@build.example".into(),
        session: "agents".into(),
        enabled: true,
    }
}

fn state_with_remote_snapshots(
    local: ClientShellSnapshot,
    mut remote: ClientShellSnapshot,
) -> (ClientShellState, ClientEndpointId) {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let profile = remote_profile_fixture();
    let endpoint_id = ClientEndpointId::Ssh(profile.id.clone());
    state.set_endpoint_catalog(&[profile]);
    state.set_endpoint_status(&endpoint_id, ClientEndpointStatus::Online);
    state.set_snapshot(Box::new(local));
    state.set_pane_surface(surface());
    remote.boot_id = "remote-boot".into();
    state.set_endpoint_snapshot(&endpoint_id, Box::new(remote));
    (state, endpoint_id)
}

#[test]
fn multi_machine_sidebar_renders_groups_per_endpoint() {
    let mut focused = workspace("ws_1", "alpha");
    focused.focused = true;
    let local = grouped_snapshot(
        vec![focused.clone(), workspace("ws_2", "beta")],
        vec![group("gL", "Local work", "blue", &["ws_1"])],
        &[("ws_1", "blue")],
    );
    let remote = grouped_snapshot(
        vec![
            focused.clone(),
            workspace("ws_2", "remote-beta"),
            workspace("ws_3", "remote-gamma"),
        ],
        vec![
            group("gR", "Remote work", "green", &["ws_2"]),
            group("gS", "Remote spare", "teal", &["ws_3"]),
        ],
        &[("ws_2", "green"), ("ws_3", "teal")],
    );
    let (mut state, remote_id) = state_with_remote_snapshots(local.clone(), remote);
    // Collapse is per endpoint: the local gR key leaves the remote gR group expanded.
    state
        .remote_collapsed_groups
        .entry(remote_id.clone())
        .or_default()
        .insert(group_collapse_key("gS"));
    state.collapsed_groups.insert(group_collapse_key("gR"));
    let frame = state.compose(WIDTH, 40).expect("composed frame").frame;

    let headers = state
        .hits
        .workspace_groups
        .iter()
        .map(|header| {
            (
                header.endpoint_id.clone(),
                header.group_id.clone(),
                header.collapsed,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        headers,
        [
            (ClientEndpointId::Local, Some("gL".to_owned()), false),
            (ClientEndpointId::Local, None, false),
            (remote_id.clone(), Some("gR".to_owned()), false),
            (remote_id.clone(), Some("gS".to_owned()), true),
            (remote_id.clone(), None, false),
        ]
    );
    let machine_row = |endpoint_id: &ClientEndpointId| {
        state
            .hits
            .machines
            .iter()
            .find(|machine| &machine.endpoint_id == endpoint_id)
            .expect("machine row")
            .rect
            .y
    };
    let card_of = |endpoint_id: &ClientEndpointId, workspace_id: &str| {
        state
            .hits
            .workspaces
            .iter()
            .find(|hit| &hit.endpoint_id == endpoint_id && hit.workspace_id == workspace_id)
            .map(|hit| hit.rect)
    };
    let local_header = state.hits.workspace_groups[0].rect;
    let local_card = card_of(&ClientEndpointId::Local, "ws_1").expect("local member");
    assert!(machine_row(&ClientEndpointId::Local) < local_header.y);
    assert_eq!(local_header.y + 1, local_card.y);
    let remote_header = state.hits.workspace_groups[2].rect;
    assert!(machine_row(&remote_id) < remote_header.y);
    let remote_card = card_of(&remote_id, "ws_2").expect("remote member");
    assert_eq!(remote_header.y + 1, remote_card.y);
    assert!(
        card_of(&remote_id, "ws_3").is_none(),
        "collapsed remote group"
    );

    // Cards are nested two columns under their machine; the bar sits in the card's column 0.
    let palette = &state.config.palette;
    for (rect, color, background) in [
        (local_card, "blue", palette.active_row_bg),
        (remote_card, "green", palette.sidebar_bg),
    ] {
        let bar = cell(&frame, rect.x + 2, rect.y);
        assert_eq!(bar.symbol, "▌");
        assert_eq!(bar.bg, u32_color(background));
        assert_eq!(
            bar.fg,
            u32_color(readable(rgb(color), background, host_bg(&state)))
        );
    }
    assert!(row_text(&frame, local_header)
        .trim_start()
        .starts_with("▌ Local work"));
    assert!(state
        .hits
        .workspace_drop_slots
        .iter()
        .any(|slot| slot.endpoint_id == remote_id && slot.group_id.as_deref() == Some("gR")));

    // A machine whose server projects no groups (an upstream server) shows none.
    let mut upstream = snapshot();
    upstream.boot_id = "remote-boot".into();
    upstream.revision = 2;
    upstream.workspaces[0].label = "remote-workspace".into();
    state.set_endpoint_snapshot(&remote_id, Box::new(upstream));
    let frame = state.compose(WIDTH, 40).expect("composed frame").frame;
    assert!(state
        .hits
        .workspace_groups
        .iter()
        .all(|header| header.endpoint_id == ClientEndpointId::Local));
    let remote_card = card_of_state(&state, &remote_id, "ws_1");
    assert_eq!(cell(&frame, remote_card.x + 2, remote_card.y).symbol, " ");
}

fn card_of_state(
    state: &ClientShellState,
    endpoint_id: &ClientEndpointId,
    workspace_id: &str,
) -> Rect {
    state
        .hits
        .workspaces
        .iter()
        .find(|hit| &hit.endpoint_id == endpoint_id && hit.workspace_id == workspace_id)
        .expect("card")
        .rect
}

#[test]
fn machine_section_end_slot_stays_off_the_next_machine_row() {
    let mut focused = workspace("ws_1", "alpha");
    focused.focused = true;
    let local = grouped_snapshot(
        vec![focused.clone(), workspace("ws_2", "beta")],
        vec![group("gL", "Local work", "blue", &["ws_1"])],
        &[],
    );
    let remote = grouped_snapshot(
        vec![focused, workspace("ws_2", "remote-beta")],
        vec![group("gR", "Remote work", "green", &["ws_2"])],
        &[],
    );
    let (mut state, remote_id) = state_with_remote_snapshots(local, remote);
    state.compose(WIDTH, 40).expect("composed frame");
    let remote_machine = state
        .hits
        .machines
        .iter()
        .find(|machine| machine.endpoint_id == remote_id)
        .expect("remote machine row")
        .rect
        .y;
    let end_slot = |endpoint_id: &ClientEndpointId| {
        state
            .hits
            .workspace_drop_slots
            .iter()
            .find(|slot| {
                &slot.endpoint_id == endpoint_id
                    && slot.group_id.is_none()
                    && slot.before_workspace_id.is_none()
            })
            .expect("ungrouped end slot")
            .row
    };

    // The local section's last card ends right above the remote machine's row, so its end
    // slot sits on the card's last line and no slot lands on the machine row.
    let local_last = card_of_state(&state, &ClientEndpointId::Local, "ws_2");
    assert_eq!(local_last.bottom(), remote_machine);
    assert_eq!(end_slot(&ClientEndpointId::Local), local_last.bottom() - 1);
    assert!(state
        .hits
        .workspace_drop_slots
        .iter()
        .all(|slot| slot.row != remote_machine));

    // The last section still takes the free row below its last card.
    let remote_last = card_of_state(&state, &remote_id, "ws_1");
    assert!(remote_last.bottom() < state.hits.workspace_body.bottom());
    assert_eq!(end_slot(&remote_id), remote_last.bottom());
}

#[test]
fn offline_machine_dims_its_group_headers() {
    let mut focused = workspace("ws_1", "alpha");
    focused.focused = true;
    let local = grouped_snapshot(
        vec![focused.clone()],
        vec![group("gL", "Local work", "blue", &["ws_1"])],
        &[],
    );
    let remote = grouped_snapshot(
        vec![focused, workspace("ws_2", "remote-beta")],
        vec![group("gR", "Remote work", "green", &["ws_2"])],
        &[],
    );
    let (mut state, remote_id) = state_with_remote_snapshots(local, remote);
    state.set_endpoint_status(&remote_id, ClientEndpointStatus::Reconnecting);
    let frame = state.compose(WIDTH, 40).expect("composed frame").frame;
    let palette = &state.config.palette;
    let header = |endpoint_id: &ClientEndpointId| {
        state
            .hits
            .workspace_groups
            .iter()
            .find(|header| &header.endpoint_id == endpoint_id && header.group_id.is_some())
            .expect("group header")
            .rect
    };
    let is_dim = |cell: &crate::protocol::CellData| cell.modifier & Modifier::DIM.bits() != 0;
    // Header hits span the section; the drawn header is nested two columns, its name two more.
    let remote_header = header(&remote_id);
    assert!(row_text(&frame, remote_header)
        .trim_start()
        .starts_with("▌ Remote work"));
    let remote_name = cell(&frame, remote_header.x + 4, remote_header.y);
    assert_eq!(remote_name.fg, u32_color(palette.overlay0));
    assert!(is_dim(remote_name));
    let local_header = header(&ClientEndpointId::Local);
    let local_name = cell(&frame, local_header.x + 4, local_header.y);
    assert_eq!(local_name.fg, u32_color(palette.text));
    assert!(!is_dim(local_name));
}

#[test]
fn machine_section_without_visible_workspace_shows_its_group_headers() {
    let mut focused = workspace("ws_1", "alpha");
    focused.focused = true;
    let local = grouped_snapshot(vec![focused], Vec::new(), &[]);
    let remote = grouped_snapshot(
        vec![workspace("ws_1", "remote-alpha")],
        vec![group("gR", "Remote work", "green", &["ws_1"])],
        &[],
    );
    let (mut state, remote_id) = state_with_remote_snapshots(local, remote);
    state
        .remote_collapsed_groups
        .entry(remote_id.clone())
        .or_default()
        .insert(group_collapse_key("gR"));
    let frame = state.compose(WIDTH, 40).expect("composed frame").frame;
    assert!(state
        .hits
        .workspaces
        .iter()
        .all(|hit| hit.endpoint_id != remote_id));
    let headers = state
        .hits
        .workspace_groups
        .iter()
        .map(|header| {
            (
                header.endpoint_id.clone(),
                header.group_id.clone(),
                header.collapsed,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(headers, [(remote_id.clone(), Some("gR".to_owned()), true)]);
    let header = state.hits.workspace_groups[0].rect;
    let machine_row = state
        .hits
        .machines
        .iter()
        .find(|machine| machine.endpoint_id == remote_id)
        .expect("remote machine row")
        .rect
        .y;
    assert_eq!(header.y, machine_row + 1);
    assert!(state
        .hits
        .machines
        .iter()
        .all(|machine| machine.rect.y != header.y));
    assert!(row_text(&frame, header)
        .trim_start()
        .starts_with("▌ Remote work (1)"));
}

#[test]
fn focused_tab_uses_workspace_color_with_contrast_text() {
    let mut focused = workspace("ws_1", "alpha");
    focused.focused = true;
    let colored = grouped_snapshot(
        vec![focused.clone(), workspace("ws_2", "beta")],
        vec![group("gA", "Work", "green", &["ws_1"])],
        &[("ws_1", "teal")],
    );
    let (state, frame) = composed(colored, &[], 30);
    let (tab, _) = state
        .hits
        .tabs
        .iter()
        .find(|(_, tab_id)| tab_id == "tab_1")
        .expect("focused tab");
    let teal = rgb("teal");
    for x in tab.x..tab.right() {
        let fill = cell(&frame, x, tab.y);
        assert_eq!(
            fill.bg,
            u32_color(ratatui::style::Color::Rgb(teal[0], teal[1], teal[2]))
        );
        assert_eq!(
            fill.fg,
            u32_color(ratatui::style::Color::Rgb(0xff, 0xff, 0xff))
        );
    }

    // Without a color the focused tab keeps upstream's accent fill.
    let plain = grouped_snapshot(
        vec![focused, workspace("ws_2", "beta")],
        vec![group("gA", "Work", "unknown", &["ws_1"])],
        &[],
    );
    let (state, frame) = composed(plain, &[], 30);
    let (tab, _) = state
        .hits
        .tabs
        .iter()
        .find(|(_, tab_id)| tab_id == "tab_1")
        .expect("focused tab");
    let palette = &state.config.palette;
    let fill = cell(&frame, tab.x, tab.y);
    assert_eq!(fill.bg, u32_color(palette.accent));
    assert_eq!(fill.fg, u32_color(panel_contrast_fg(palette)));
}

#[test]
fn mobile_layout_shows_grouped_order_flat_without_headers() {
    let mut focused = workspace("ws_1", "alpha");
    focused.focused = true;
    let snapshot = grouped_snapshot(
        vec![
            focused,
            workspace("ws_2", "beta"),
            workspace("ws_3", "gamma"),
            workspace("ws_4", "delta"),
        ],
        vec![
            group("gA", "Work", "blue", &["ws_4"]),
            group("gB", "Personal", "green", &["ws_2", "ws_3"]),
        ],
        &[("ws_4", "blue"), ("ws_2", "green"), ("ws_3", "green")],
    );
    let key = group_collapse_key("gB");
    let mut state = shell_state(snapshot, &[key.as_str()]);
    state.compose(44, 30).expect("mobile layout");
    let switch = state.hits.mobile_switch;
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: switch.x,
        row: switch.y,
        modifiers: crossterm::event::KeyModifiers::empty(),
    })]);
    let frame = state.compose(44, 30).expect("mobile switcher").frame;
    let order = state
        .hits
        .mobile_targets
        .iter()
        .filter_map(|(_, target)| match target {
            ClientMobileTarget::Workspace { workspace_id, .. } => Some(workspace_id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(order, ["ws_4", "ws_2", "ws_3", "ws_1"]);
    assert!(state.hits.workspace_groups.is_empty());
    assert!(frame.cells.iter().all(|cell| cell.symbol != "▌"));

    // Mobile navigation walks the same flat grouped order, collapsed members included.
    let mut outcome = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::NextWorkspace),
        &mut outcome,
    );
    assert!(outcome.actions.iter().any(|action| matches!(
        action,
        ClientShellAction::Endpoint { request, .. }
            if matches!(
                &request.method,
                crate::api::schema::Method::WorkspaceFocus(target)
                    if target.workspace_id == "ws_4"
            )
    )));
}

// Sidebar tab rows (D3).

/// The fixture snapshot locally and remotely, where the remote workspace has a second tab,
/// so only it gets tab rows.
fn state_with_two_tab_remote() -> (ClientShellState, ClientEndpointId) {
    let mut remote = snapshot();
    remote.workspaces[0].label = "remote-workspace".into();
    remote.tabs.push(tab("ws_1", "tab_2", 2, "logs"));
    state_with_remote_snapshots(snapshot(), remote)
}

fn numbered_workspace(
    id: &str,
    number: usize,
    label: &str,
    status: AgentStatus,
) -> ClientShellWorkspace {
    let mut workspace = snapshot().workspaces[0].clone();
    workspace.workspace_id = id.into();
    workspace.active_tab_id = format!("{id}_tab");
    workspace.number = number;
    workspace.label = label.into();
    workspace.branch = None;
    workspace.focused = false;
    workspace.agent_status = status;
    workspace
}

fn tab(workspace_id: &str, tab_id: &str, number: usize, label: &str) -> ClientShellTab {
    ClientShellTab {
        tab_id: tab_id.into(),
        workspace_id: workspace_id.into(),
        number,
        label: label.into(),
        custom_label: false,
        zoomed: false,
        focused: false,
        agent_status: AgentStatus::Idle,
    }
}

fn worktree(key: &str, linked: bool) -> Option<ClientShellWorktree> {
    Some(ClientShellWorktree {
        key: key.into(),
        label: key.into(),
        is_linked_worktree: linked,
    })
}

/// One tab per workspace: a plain workspace, a working one, and a worktree family.
fn single_tab_snapshot() -> ClientShellSnapshot {
    let mut projected = snapshot();
    projected
        .workspaces
        .push(numbered_workspace("ws_2", 2, "api", AgentStatus::Working));
    projected.tabs.push(tab("ws_2", "tab_2", 1, "server"));
    let mut parent = numbered_workspace("ws_3", 3, "repo", AgentStatus::Idle);
    parent.worktree = worktree("repo", false);
    parent.branch = Some("main".into());
    projected.workspaces.push(parent);
    projected.tabs.push(tab("ws_3", "tab_3", 1, "parent"));
    let mut child = numbered_workspace("ws_4", 4, "repo-feature", AgentStatus::Blocked);
    child.worktree = worktree("repo", true);
    child.branch = Some("worktree/feature".into());
    projected.workspaces.push(child);
    projected.tabs.push(tab("ws_4", "tab_4", 1, "child"));
    projected
}

/// `single_tab_snapshot` with a second tab ("tests", blocked) on `ws_2`.
fn two_tab_snapshot() -> ClientShellSnapshot {
    let mut projected = single_tab_snapshot();
    let mut second = tab("ws_2", "tab_5", 2, "tests");
    second.agent_status = AgentStatus::Blocked;
    projected.tabs.insert(2, second);
    projected
}

fn compose_sidebar_with(
    config: &Config,
    projected: ClientShellSnapshot,
    collapsed_groups: &[&str],
) -> (ClientShellState, FrameData) {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(config));
    state.collapsed_groups = collapsed_groups.iter().map(|key| (*key).into()).collect();
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    let frame = state.compose(106, 24).expect("sidebar frame").frame;
    (state, frame)
}

fn compose_sidebar(
    projected: ClientShellSnapshot,
    collapsed_groups: &[&str],
) -> (ClientShellState, FrameData) {
    compose_sidebar_with(&Config::default(), projected, collapsed_groups)
}

/// Sidebar columns left of its separator.
fn sidebar_width(state: &ClientShellState) -> usize {
    usize::from(state.sidebar_width.saturating_sub(1))
}

fn sidebar_rows(state: &ClientShellState, frame: &FrameData) -> Vec<String> {
    let width = sidebar_width(state);
    frame_rows(frame)
        .into_iter()
        .map(|row| row.chars().take(width).collect::<String>())
        .collect()
}

fn sidebar_cells<'a>(state: &ClientShellState, frame: &'a FrameData) -> Vec<&'a CellData> {
    let width = sidebar_width(state);
    frame
        .cells
        .chunks(usize::from(frame.width))
        .flat_map(|row| row.iter().take(width))
        .collect()
}

fn workspace_rects(state: &ClientShellState) -> Vec<(String, Rect)> {
    state
        .hits
        .workspaces
        .iter()
        .map(|hit| (hit.workspace_id.clone(), hit.rect))
        .collect()
}

fn workspace_hit<'a>(state: &'a ClientShellState, workspace_id: &str) -> &'a WorkspaceHit {
    state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == workspace_id)
        .expect("workspace hit")
}

fn sidebar_tab_rows(state: &ClientShellState) -> Vec<(&str, u16)> {
    state
        .hits
        .sidebar_tabs
        .iter()
        .map(|(rect, _, tab_id)| (tab_id.as_str(), rect.y))
        .collect()
}

#[test]
fn single_tab_workspace_without_groups_renders_identically_to_upstream() {
    // Captured from upstream 5bd900d with this fixture.
    const UPSTREAM_ROWS: [&str; 12] = [
        " spaces                  ",
        "                         ",
        " ○ client-shell          ",
        "   main                  ",
        " ● api                   ",
        " ○ repo                 ▾",
        "   main                  ",
        "   └─ ● feature          ",
        "                         ",
        "                         ",
        "                         ",
        " new                 menu",
    ];
    let (state, frame) = compose_sidebar(single_tab_snapshot(), &[]);
    assert_eq!(sidebar_rows(&state, &frame)[..12], UPSTREAM_ROWS);
    assert_eq!(
        workspace_rects(&state),
        [
            ("ws_1".to_string(), Rect::new(0, 2, 25, 2)),
            ("ws_2".to_string(), Rect::new(0, 4, 25, 1)),
            ("ws_3".to_string(), Rect::new(0, 5, 25, 2)),
            ("ws_4".to_string(), Rect::new(0, 7, 25, 1)),
        ]
    );

    // Tab rows read nothing but the snapshot's tabs, so a one-tab workspace must render
    // cell for cell like a workspace whose tabs are absent, expanded and collapsed.
    for collapsed in [&[][..], &["repo"][..]] {
        let (state, frame) = compose_sidebar(single_tab_snapshot(), collapsed);
        let mut without_tabs = single_tab_snapshot();
        without_tabs.tabs.clear();
        let (bare_state, bare_frame) = compose_sidebar(without_tabs, collapsed);
        assert!(state.hits.sidebar_tabs.is_empty());
        assert_eq!(workspace_rects(&state), workspace_rects(&bare_state));
        assert_eq!(
            sidebar_cells(&state, &frame),
            sidebar_cells(&bare_state, &bare_frame)
        );
    }
}

#[test]
fn sidebar_tab_rows_only_for_workspaces_with_two_or_more_tabs() {
    let projected = two_tab_snapshot();
    let grouped = super::super::sidebar_tabs::group_workspace_tabs(&projected);
    assert_eq!(grouped.keys().copied().collect::<Vec<_>>(), ["ws_2"]);
    assert_eq!(
        grouped["ws_2"]
            .iter()
            .map(|tab| tab.tab_id.as_str())
            .collect::<Vec<_>>(),
        ["tab_2", "tab_5"]
    );

    let (state, frame) = compose_sidebar(projected, &[]);
    let rows = sidebar_rows(&state, &frame);
    assert_eq!(sidebar_tab_rows(&state), [("tab_2", 5), ("tab_5", 6)]);
    assert!(state
        .hits
        .sidebar_tabs
        .iter()
        .all(|(rect, endpoint_id, _)| endpoint_id.is_local() && rect.width == 25));
    assert_eq!(rows[4], " ● api                   ");
    assert_eq!(rows[5], "   ├─ ○ server           ");
    assert_eq!(rows[6], "   └─ ● tests            ");
    assert_eq!(rows[7], " ○ repo                 ▾");
    assert_eq!(
        workspace_rects(&state),
        [
            ("ws_1".to_string(), Rect::new(0, 2, 25, 2)),
            ("ws_2".to_string(), Rect::new(0, 4, 25, 1)),
            ("ws_3".to_string(), Rect::new(0, 7, 25, 2)),
            ("ws_4".to_string(), Rect::new(0, 9, 25, 1)),
        ]
    );
}

#[test]
fn workspace_row_keeps_aggregate_status() {
    // A two-tab worktree parent keeps upstream's status on its label row, including the
    // status rolled up from its collapsed linked child; tab rows add per-tab status below.
    let mut projected = single_tab_snapshot();
    let mut notes = tab("ws_3", "tab_5", 2, "notes");
    notes.agent_status = AgentStatus::Done;
    projected.tabs.insert(3, notes);
    for (collapsed, expected) in [
        (&[][..], AgentStatus::Idle),
        (&["repo"][..], AgentStatus::Blocked),
    ] {
        let (state, frame) = compose_sidebar(projected.clone(), collapsed);
        let palette = &state.config.palette;
        let parent = workspace_hit(&state, "ws_3").rect;
        let status_cell = &frame.cells
            [usize::from(parent.y) * usize::from(frame.width) + usize::from(parent.x + 1)];
        assert_eq!(
            status_cell.symbol,
            status_icon(expected, state.config.status_indicators)
        );
        assert_eq!(
            status_cell.fg,
            crate::protocol::color_to_u32(status_color(expected, palette))
        );
        assert_eq!(
            sidebar_tab_rows(&state),
            [("tab_3", parent.bottom()), ("tab_5", parent.bottom() + 1)]
        );
        let done_icon = state.hits.sidebar_tabs[1].0;
        let done_cell = &frame.cells
            [usize::from(done_icon.y) * usize::from(frame.width) + usize::from(done_icon.x + 6)];
        assert_eq!(
            done_cell.fg,
            crate::protocol::color_to_u32(status_color(AgentStatus::Done, palette))
        );
    }
}

#[test]
fn disabled_mouse_has_no_sidebar_tab_hits() {
    let mut config = Config::default();
    config.ui.mouse_capture = false;
    let (state, frame) = compose_sidebar_with(&config, two_tab_snapshot(), &[]);
    assert!(state.hits.sidebar_tabs.is_empty());
    let rows = sidebar_rows(&state, &frame);
    assert_eq!(rows[5], "   ├─ ○ server           ");
    assert_eq!(rows[6], "   └─ ● tests            ");
}

fn left_mouse(kind: MouseEventKind, column: u16, row: u16) -> RawInputEvent {
    RawInputEvent::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::empty(),
    })
}

fn focused_tab_request(outcome: &ClientShellInput) -> Option<&str> {
    match &outcome.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => match &request.method {
            crate::api::schema::Method::TabFocus(target) => Some(target.tab_id.as_str()),
            _ => None,
        },
        _ => None,
    }
}

#[test]
fn sidebar_tabs_show_status_and_focus_background_workspace() {
    // The focused workspace has one tab and gets no rows (D3); the background workspace
    // lists both of its tabs with their own status, and a click focuses that tab.
    let mut projected = snapshot();
    projected.tabs[0].label = "notes".into();
    projected.workspaces.push(numbered_workspace(
        "ws_2",
        2,
        "background",
        AgentStatus::Idle,
    ));
    projected.tabs.push(tab("ws_2", "tab_2", 1, "editor"));
    let mut blocked = tab("ws_2", "tab_3", 2, "tests");
    blocked.agent_status = AgentStatus::Blocked;
    projected.tabs.push(blocked);
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    let frame = state.compose(106, 32).expect("sidebar with tabs");
    let rows = frame_rows(&frame);
    let texts = state
        .hits
        .sidebar_tabs
        .iter()
        .map(|(rect, _, tab_id)| {
            let text = rows[usize::from(rect.y)]
                .chars()
                .skip(usize::from(rect.x))
                .take(usize::from(rect.width))
                .collect::<String>();
            (tab_id.as_str(), text.trim_end().to_string())
        })
        .collect::<Vec<_>>();
    assert_eq!(
        texts,
        [
            ("tab_2", "   ├─ ○ editor".to_string()),
            ("tab_3", "   └─ ● tests".to_string()),
        ]
    );

    let rect = state.hits.sidebar_tabs[1].0;
    let outcome = state.handle_raw_events(vec![left_mouse(
        MouseEventKind::Down(MouseButton::Left),
        rect.x + 4,
        rect.y,
    )]);
    assert_eq!(focused_tab_request(&outcome), Some("tab_3"));
    assert!(state.workspace_press.is_none());
    assert!(state.tab_press.is_none());
}

#[test]
fn sidebar_tab_click_focuses_tab_without_drag() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(two_tab_snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 24).expect("sidebar with tabs");
    let row = state.hits.sidebar_tabs[0].0;
    let target = workspace_hit(&state, "ws_1").rect;

    let down = state.handle_raw_events(vec![left_mouse(
        MouseEventKind::Down(MouseButton::Left),
        row.x + 7,
        row.y,
    )]);
    assert_eq!(focused_tab_request(&down), Some("tab_2"));
    assert!(state.workspace_press.is_none());
    assert!(state.tab_press.is_none());

    let drag = state.handle_raw_events(vec![left_mouse(
        MouseEventKind::Drag(MouseButton::Left),
        target.x + 1,
        target.y,
    )]);
    assert!(state.chrome_drag.is_none());
    assert!(drag.actions.is_empty());
    let up = state.handle_raw_events(vec![left_mouse(
        MouseEventKind::Up(MouseButton::Left),
        target.x + 1,
        target.y,
    )]);
    assert!(state.chrome_drag.is_none());
    assert!(up.actions.is_empty());
    assert!(up.requests.is_empty());
}

#[test]
fn sidebar_tab_click_activates_its_endpoint() {
    let (mut state, remote) = state_with_two_tab_remote();
    state.compose(100, 32).expect("multi-machine sidebar");
    let remote_card = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.endpoint_id == remote)
        .expect("remote workspace")
        .rect;
    assert_eq!(
        state
            .hits
            .sidebar_tabs
            .iter()
            .map(|(rect, endpoint_id, tab_id)| (endpoint_id, tab_id.as_str(), rect.y))
            .collect::<Vec<_>>(),
        [
            (&remote, "tab_1", remote_card.bottom()),
            (&remote, "tab_2", remote_card.bottom() + 1),
        ]
    );
    let rect = state.hits.sidebar_tabs[0].0;
    let outcome = state.handle_raw_events(vec![left_mouse(
        MouseEventKind::Down(MouseButton::Left),
        rect.x + 3,
        rect.y,
    )]);
    assert!(
        matches!(&outcome.actions[..], [ClientShellAction::ActivateEndpoint { endpoint_id, target: Some(ClientEndpointFocusTarget::Tab(tab_id)) }]
        if endpoint_id == &remote && tab_id == "tab_1")
    );
}

#[test]
fn card_bottom_covers_tab_rows() {
    let (state, _) = compose_sidebar(two_tab_snapshot(), &[]);
    let card = workspace_hit(&state, "ws_2");
    assert_eq!(card.rect, Rect::new(0, 4, 25, 1));
    assert_eq!(card.card_bottom, state.hits.sidebar_tabs[1].0.bottom());
    for workspace_id in ["ws_1", "ws_3", "ws_4"] {
        let hit = workspace_hit(&state, workspace_id);
        assert_eq!(hit.card_bottom, hit.rect.bottom(), "{workspace_id}");
    }

    // The drop slot after the last card sits below its tab rows.
    let mut projected = snapshot();
    projected
        .workspaces
        .push(numbered_workspace("ws_2", 2, "multi", AgentStatus::Idle));
    projected.tabs.push(tab("ws_2", "tab_2", 1, "editor"));
    projected.tabs.push(tab("ws_2", "tab_3", 2, "tests"));
    let (mut state, _) = compose_sidebar(projected, &[]);
    let card_bottom = workspace_hit(&state, "ws_2").card_bottom;
    assert_eq!(card_bottom, state.hits.sidebar_tabs[1].0.bottom());
    let source = workspace_hit(&state, "ws_1").rect;
    state.handle_raw_events(vec![left_mouse(
        MouseEventKind::Down(MouseButton::Left),
        source.x + 1,
        source.y,
    )]);
    state.handle_raw_events(vec![left_mouse(
        MouseEventKind::Drag(MouseButton::Left),
        source.x + 1,
        card_bottom,
    )]);
    let target = match &state.chrome_drag {
        Some(ClientChromeDrag::Workspace { target, .. }) => target.clone(),
        _ => None,
    };
    assert_eq!(target, Some((None, card_bottom)));

    // Multi-machine sidebar: the remote card ends below its tab rows.
    let (mut state, remote) = state_with_two_tab_remote();
    state.compose(100, 32).expect("multi-machine sidebar");
    let remote_card = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.endpoint_id == remote)
        .expect("remote workspace");
    assert_eq!(
        remote_card.card_bottom,
        state.hits.sidebar_tabs[1].0.bottom()
    );
    assert!(remote_card.rect.bottom() < remote_card.card_bottom);

    // The collapsed rail has no tab rows: every card ends at its row.
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.sidebar_collapsed = true;
    state.set_snapshot(Box::new(two_tab_snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 24).expect("collapsed sidebar");
    assert!(state.hits.sidebar_tabs.is_empty());
    assert!(!state.hits.workspaces.is_empty());
    assert!(state
        .hits
        .workspaces
        .iter()
        .all(|hit| hit.card_bottom == hit.rect.bottom()));
}

#[test]
fn workspace_drag_rejects_foreign_tab_rows() {
    let (mut state, remote) = state_with_two_tab_remote();
    state.compose(100, 32).expect("aggregate sidebar");
    let local = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.endpoint_id.is_local())
        .expect("local workspace")
        .rect;
    let remote_tab = state
        .hits
        .sidebar_tabs
        .iter()
        .find(|(_, endpoint_id, _)| endpoint_id == &remote)
        .expect("remote tab row")
        .0;

    state.handle_raw_events(vec![left_mouse(
        MouseEventKind::Down(MouseButton::Left),
        local.x + 1,
        local.y,
    )]);
    state.handle_raw_events(vec![left_mouse(
        MouseEventKind::Drag(MouseButton::Left),
        remote_tab.x + 1,
        remote_tab.y,
    )]);
    assert!(state.chrome_drag.is_none());

    // The same press still drags within its own machine section.
    state.handle_raw_events(vec![left_mouse(
        MouseEventKind::Drag(MouseButton::Left),
        local.x + 2,
        local.y,
    )]);
    assert!(matches!(
        state.chrome_drag,
        Some(ClientChromeDrag::Workspace { .. })
    ));
}

// Interactions: mouse, menus and keys. Every organize action is asserted by the exact
// `workspace_group.edit` ops it sends.

fn mouse_at(
    state: &mut ClientShellState,
    kind: MouseEventKind,
    column: u16,
    row: u16,
) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::empty(),
    })])
}

fn click(state: &mut ClientShellState, column: u16, row: u16) -> ClientShellInput {
    mouse_at(state, MouseEventKind::Down(MouseButton::Left), column, row);
    mouse_at(state, MouseEventKind::Up(MouseButton::Left), column, row)
}

fn drag(state: &mut ClientShellState, from: (u16, u16), to: (u16, u16)) -> ClientShellInput {
    mouse_at(
        state,
        MouseEventKind::Down(MouseButton::Left),
        from.0,
        from.1,
    );
    mouse_at(state, MouseEventKind::Drag(MouseButton::Left), to.0, to.1);
    // Redraw mid-drag, as the runtime does, so drag-only decorations record their slots.
    state.compose(WIDTH, 30).expect("dragging frame");
    mouse_at(state, MouseEventKind::Drag(MouseButton::Left), to.0, to.1);
    mouse_at(state, MouseEventKind::Up(MouseButton::Left), to.0, to.1)
}

fn header(state: &ClientShellState, group_id: Option<&str>) -> Rect {
    state
        .hits
        .workspace_groups
        .iter()
        .find(|hit| hit.group_id.as_deref() == group_id)
        .unwrap_or_else(|| panic!("{group_id:?} header is visible"))
        .rect
}

/// The ops of every `workspace_group.edit` request in the outcome, in order.
fn sent_ops(outcome: &ClientShellInput) -> Vec<Vec<WorkspaceGroupOp>> {
    outcome
        .actions
        .iter()
        .filter_map(|action| match action {
            ClientShellAction::Endpoint { request, .. } => match &request.method {
                Method::WorkspaceGroupEdit(params) => Some(params.ops.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn three_groups() -> ClientShellSnapshot {
    let mut focused = workspace("ws_1", "alpha");
    focused.focused = true;
    grouped_snapshot(
        vec![
            focused,
            workspace("ws_2", "beta"),
            workspace("ws_3", "gamma"),
        ],
        vec![
            group("gA", "Work", "blue", &["ws_1"]),
            group("gB", "Personal", "green", &["ws_2"]),
            group("gC", "Infra", "teal", &["ws_3"]),
        ],
        &[],
    )
}

#[test]
fn header_click_toggles_and_persists_group_key() {
    let path = std::env::temp_dir().join(format!(
        "herdr-fork-group-header-click-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let mut snapshot = three_groups();
    snapshot.workspaces.push(workspace("ws_4", "delta"));
    let config =
        ClientShellConfig::from_config(&Config::default()).with_preferences_path(path.clone());
    let mut state = ClientShellState::new(config);
    state.set_snapshot(Box::new(snapshot));
    state.set_pane_surface(surface());
    state.compose(WIDTH, 30).expect("composed frame");
    let personal = header(&state, Some("gB"));
    let key = group_collapse_key("gB");

    let released = click(&mut state, personal.x + 3, personal.y);
    assert!(state.collapsed_groups.contains(&key));
    assert!(released.actions.is_empty(), "collapse is client-local");
    assert!(state.chrome_drag.is_none());
    let stored = std::fs::read_to_string(&path).expect("stored preferences");
    assert!(stored.contains(&key), "{stored}");

    state.compose(WIDTH, 30).expect("collapsed frame");
    assert!(state
        .hits
        .workspace_groups
        .iter()
        .any(|hit| hit.group_id.as_deref() == Some("gB") && hit.collapsed));
    assert!(state
        .hits
        .workspaces
        .iter()
        .all(|hit| hit.workspace_id != "ws_2"));
    let personal = header(&state, Some("gB"));
    click(&mut state, personal.right() - 1, personal.y);
    assert!(!state.collapsed_groups.contains(&key));
    let stored = std::fs::read_to_string(&path).expect("stored preferences");
    assert!(!stored.contains(&key), "{stored}");

    // The "ungrouped" header has no toggle.
    state.compose(WIDTH, 30).expect("expanded frame");
    let ungrouped = header(&state, None);
    let released = click(&mut state, ungrouped.x + 3, ungrouped.y);
    assert!(state.collapsed_groups.is_empty());
    assert!(released.actions.is_empty());
    std::fs::remove_file(path).expect("remove preferences");
}

#[test]
fn header_drag_sends_group_move() {
    let mut state = shell_state(three_groups(), &[]);
    state.compose(WIDTH, 30).expect("composed frame");
    let work = header(&state, Some("gA"));
    let personal = header(&state, Some("gB"));
    let infra = header(&state, Some("gC"));

    // Infra dragged onto the Personal header lands above it.
    mouse_at(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        infra.x + 3,
        infra.y,
    );
    mouse_at(
        &mut state,
        MouseEventKind::Drag(MouseButton::Left),
        infra.x + 3,
        personal.y,
    );
    assert!(matches!(
        &state.chrome_drag,
        Some(ClientChromeDrag::WorkspaceGroup {
            endpoint_id,
            group_id,
            target: Some((Some(before), row)),
        }) if *endpoint_id == ClientEndpointId::Local
            && group_id == "gC"
            && before == "gB"
            && *row == personal.y - 1
    ));
    let frame = state.compose(WIDTH, 30).expect("dragging frame").frame;
    let indicator = Rect::new(personal.x, personal.y - 1, personal.width, 1);
    assert!(row_text(&frame, indicator).contains('─'));
    let dropped = mouse_at(
        &mut state,
        MouseEventKind::Up(MouseButton::Left),
        infra.x + 3,
        personal.y,
    );
    assert_eq!(dropped.actions.len(), 1);
    assert_eq!(
        sent_ops(&dropped),
        [vec![WorkspaceGroupOp::Move {
            group_id: "gC".into(),
            before_group_id: Some("gB".into()),
        }]]
    );
    assert!(state.collapsed_groups.is_empty(), "a drag never toggles");

    // Work dragged below the last region moves to the end.
    state.compose(WIDTH, 30).expect("composed frame");
    let end = card(&state, "ws_3").bottom();
    let dropped = drag(&mut state, (work.x + 3, work.y), (work.x + 3, end));
    assert_eq!(
        sent_ops(&dropped),
        [vec![WorkspaceGroupOp::Move {
            group_id: "gA".into(),
            before_group_id: None,
        }]]
    );

    // Dropping a group where it already is sends nothing and does not toggle it.
    state.compose(WIDTH, 30).expect("composed frame");
    let dropped = drag(
        &mut state,
        (personal.x + 3, personal.y),
        (personal.x + 4, personal.y),
    );
    assert!(dropped.actions.is_empty());
    assert!(state.collapsed_groups.is_empty());
}

/// Work [ws_1, ws_2], Closed [ws_5], Empty [], then ws_3 and ws_4 ungrouped.
fn drop_fixture() -> ClientShellSnapshot {
    let mut focused = workspace("ws_1", "alpha");
    focused.focused = true;
    grouped_snapshot(
        vec![
            focused,
            workspace("ws_2", "beta"),
            workspace("ws_3", "gamma"),
            workspace("ws_4", "delta"),
            workspace("ws_5", "epsilon"),
        ],
        vec![
            group("gA", "Work", "blue", &["ws_1", "ws_2"]),
            group("gC", "Closed", "red", &["ws_5"]),
            group("gE", "Empty", "gray", &[]),
        ],
        &[],
    )
}

fn place(
    workspace_id: &str,
    group_id: Option<&str>,
    before_workspace_id: Option<&str>,
) -> Vec<Vec<WorkspaceGroupOp>> {
    vec![vec![WorkspaceGroupOp::Place {
        workspace_id: workspace_id.into(),
        group_id: group_id.map(str::to_owned),
        before_workspace_id: before_workspace_id.map(str::to_owned),
    }]]
}

/// Where a press grabs a workspace card.
fn grab(state: &ClientShellState, workspace_id: &str) -> (u16, u16) {
    let card = card(state, workspace_id);
    (card.x + 2, card.y)
}

#[test]
fn drop_on_header_places_at_group_top() {
    let key = group_collapse_key("gC");
    let mut state = shell_state(drop_fixture(), &[key.as_str()]);
    state.compose(WIDTH, 30).expect("composed frame");
    let work = header(&state, Some("gA"));
    let closed = header(&state, Some("gC"));
    let empty = header(&state, Some("gE"));

    let from = grab(&state, "ws_4");
    let dropped = drag(&mut state, from, (work.x + 3, work.y));
    assert_eq!(dropped.actions.len(), 1);
    assert_eq!(sent_ops(&dropped), place("ws_4", Some("gA"), Some("ws_1")));

    // A collapsed or empty group's header appends to that group.
    let from = grab(&state, "ws_4");
    let dropped = drag(&mut state, from, (closed.x + 3, closed.y));
    assert_eq!(sent_ops(&dropped), place("ws_4", Some("gC"), None));
    let from = grab(&state, "ws_3");
    let dropped = drag(&mut state, from, (empty.x + 3, empty.y));
    assert_eq!(sent_ops(&dropped), place("ws_3", Some("gE"), None));
}

#[test]
fn drop_between_members_places_before_target() {
    let mut state = shell_state(drop_fixture(), &[]);
    state.compose(WIDTH, 30).expect("composed frame");
    let second = card(&state, "ws_2");
    let fourth = card(&state, "ws_4");

    let from = grab(&state, "ws_4");
    let dropped = drag(&mut state, from, (second.x + 2, second.y - 1));
    assert_eq!(dropped.actions.len(), 1);
    assert_eq!(sent_ops(&dropped), place("ws_4", Some("gA"), Some("ws_2")));

    // Between ungrouped workspaces the place targets the ungrouped region.
    let from = grab(&state, "ws_1");
    let dropped = drag(&mut state, from, (fourth.x + 2, fourth.y - 1));
    assert_eq!(sent_ops(&dropped), place("ws_1", None, Some("ws_4")));

    // Dropping a workspace on its own slot sends nothing.
    let from = grab(&state, "ws_2");
    let dropped = drag(&mut state, from, (second.x + 2, second.y - 1));
    assert!(dropped.actions.is_empty());
}

#[test]
fn drop_after_region_end_places_at_group_end() {
    let mut state = shell_state(drop_fixture(), &[]);
    state.compose(WIDTH, 30).expect("composed frame");
    let second = card(&state, "ws_2");
    let fifth = card(&state, "ws_5");

    let from = grab(&state, "ws_4");
    let dropped = drag(&mut state, from, (second.x + 2, second.bottom() - 1));
    assert_eq!(dropped.actions.len(), 1);
    assert_eq!(sent_ops(&dropped), place("ws_4", Some("gA"), None));

    let from = grab(&state, "ws_1");
    let dropped = drag(&mut state, from, (fifth.x + 2, fifth.bottom() - 1));
    assert_eq!(sent_ops(&dropped), place("ws_1", Some("gC"), None));
}

#[test]
fn drop_on_ungrouped_tail_ungroups() {
    let mut state = shell_state(drop_fixture(), &[]);
    state.compose(WIDTH, 30).expect("composed frame");
    let tail = card(&state, "ws_4").bottom();
    let ungrouped = header(&state, None);

    let from = grab(&state, "ws_2");
    let dropped = drag(&mut state, from, (ungrouped.x + 3, tail));
    assert_eq!(dropped.actions.len(), 1);
    assert_eq!(sent_ops(&dropped), place("ws_2", None, None));

    // The "ungrouped" header places at the top of the ungrouped region.
    let from = grab(&state, "ws_2");
    let dropped = drag(&mut state, from, (ungrouped.x + 3, ungrouped.y));
    assert_eq!(sent_ops(&dropped), place("ws_2", None, Some("ws_3")));

    // With every workspace grouped, the "ungrouped" header appears while a workspace is
    // dragged, so ungrouping stays a drop target.
    let mut focused = workspace("ws_1", "alpha");
    focused.focused = true;
    let all_grouped = grouped_snapshot(
        vec![focused, workspace("ws_2", "beta")],
        vec![group("gA", "Work", "blue", &["ws_1", "ws_2"])],
        &[],
    );
    let mut state = shell_state(all_grouped, &[]);
    state.compose(WIDTH, 30).expect("composed frame");
    assert!(state
        .hits
        .workspace_groups
        .iter()
        .all(|hit| hit.group_id.is_some()));
    let (column, row) = grab(&state, "ws_2");
    let below = card(&state, "ws_2").bottom();
    mouse_at(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        column,
        row,
    );
    mouse_at(
        &mut state,
        MouseEventKind::Drag(MouseButton::Left),
        column,
        below,
    );
    state.compose(WIDTH, 30).expect("dragging frame");
    let ungrouped = header(&state, None);
    mouse_at(
        &mut state,
        MouseEventKind::Drag(MouseButton::Left),
        column,
        ungrouped.y,
    );
    let dropped = mouse_at(
        &mut state,
        MouseEventKind::Up(MouseButton::Left),
        column,
        ungrouped.y,
    );
    assert_eq!(sent_ops(&dropped), place("ws_2", None, None));
}

/// Work [ws_1, ws_2], Infra [ws_3], then ws_4 ungrouped; ws_2 and ws_3 have two tabs each, so
/// their cards carry two tab rows below the label row.
fn multi_tab_drop_fixture() -> ClientShellSnapshot {
    let mut focused = workspace("ws_1", "alpha");
    focused.focused = true;
    let mut snapshot = grouped_snapshot(
        vec![
            focused,
            workspace("ws_2", "beta"),
            workspace("ws_3", "gamma"),
            workspace("ws_4", "delta"),
        ],
        vec![
            group("gA", "Work", "blue", &["ws_1", "ws_2"]),
            group("gB", "Infra", "teal", &["ws_3"]),
        ],
        &[],
    );
    for workspace_id in ["ws_2", "ws_3"] {
        snapshot.tabs.push(tab(
            workspace_id,
            &format!("{workspace_id}_tab"),
            1,
            "editor",
        ));
        snapshot.tabs.push(tab(
            workspace_id,
            &format!("{workspace_id}_logs"),
            2,
            "logs",
        ));
    }
    snapshot
}

/// A workspace card's label rows and the row below its whole card (tab rows included).
fn card_geometry(state: &ClientShellState, workspace_id: &str) -> (Rect, u16) {
    let hit = workspace_hit(state, workspace_id);
    (hit.rect, hit.card_bottom)
}

#[test]
fn drops_around_multi_tab_cards_land_on_their_rows() {
    let mut state = shell_state(multi_tab_drop_fixture(), &[]);
    state.compose(WIDTH, 30).expect("composed frame");
    let (alpha, alpha_bottom) = card_geometry(&state, "ws_1");
    let (beta, beta_bottom) = card_geometry(&state, "ws_2");
    let (gamma, gamma_bottom) = card_geometry(&state, "ws_3");
    let (delta, _) = card_geometry(&state, "ws_4");
    assert_eq!(alpha_bottom, alpha.bottom());
    assert_eq!(beta_bottom, beta.bottom() + 2);
    assert_eq!(gamma_bottom, gamma.bottom() + 2);
    let (work, infra, ungrouped) = (
        header(&state, Some("gA")),
        header(&state, Some("gB")),
        header(&state, None),
    );
    // Each header follows the whole card above it, and the region end slots sit on the last
    // tab row of a multi-tab card, never on its label row.
    assert_eq!(infra.y, beta_bottom);
    assert_eq!(ungrouped.y, gamma_bottom);
    assert_eq!(
        drop_slots(&state),
        [
            (work.y, Some("gA"), Some("ws_1")),
            (beta.y - 1, Some("gA"), Some("ws_2")),
            (beta_bottom - 1, Some("gA"), None),
            (infra.y, Some("gB"), Some("ws_3")),
            (gamma_bottom - 1, Some("gB"), None),
            (ungrouped.y, None, Some("ws_4")),
            (delta.bottom(), None, None),
        ]
    );

    // On a multi-tab card's label row the drop goes before that card...
    let from = grab(&state, "ws_4");
    let dropped = drag(&mut state, from, (beta.x + 2, beta.y));
    assert_eq!(dropped.actions.len(), 1);
    assert_eq!(sent_ops(&dropped), place("ws_4", Some("gA"), Some("ws_2")));
    // ...and on either of its tab rows after it, at the end of its group.
    for row in [beta.bottom(), beta_bottom - 1] {
        let from = grab(&state, "ws_4");
        let dropped = drag(&mut state, from, (beta.x + 2, row));
        assert_eq!(
            sent_ops(&dropped),
            place("ws_4", Some("gA"), None),
            "row {row}"
        );
    }
    // The first tab row of a group's last card is nearer its region end than its header.
    let from = grab(&state, "ws_1");
    let dropped = drag(&mut state, from, (gamma.x + 2, gamma.bottom()));
    assert_eq!(sent_ops(&dropped), place("ws_1", Some("gB"), None));
    // Above the ungrouped header (the last tab row of Infra's card) still ends Infra.
    let from = grab(&state, "ws_4");
    let dropped = drag(&mut state, from, (gamma.x + 2, gamma_bottom - 1));
    assert_eq!(sent_ops(&dropped), place("ws_4", Some("gB"), None));

    // The drop line is drawn on the multi-tab card's last tab row and leaves its label row.
    let from = grab(&state, "ws_4");
    mouse_at(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        from.0,
        from.1,
    );
    mouse_at(
        &mut state,
        MouseEventKind::Drag(MouseButton::Left),
        beta.x + 2,
        beta_bottom - 1,
    );
    assert!(matches!(
        &state.chrome_drag,
        Some(ClientChromeDrag::Workspace { target: Some((None, row)), .. })
            if *row == beta_bottom - 1
    ));
    let frame = state.compose(WIDTH, 30).expect("dragging frame").frame;
    let last_tab_row = Rect::new(beta.x, beta_bottom - 1, beta.width, 1);
    assert!(row_text(&frame, last_tab_row).starts_with("──"));
    assert!(row_text(&frame, beta).contains("beta"));
    assert!(row_text(&frame, Rect::new(beta.x, beta.bottom(), beta.width, 1)).contains("editor"));
    mouse_at(
        &mut state,
        MouseEventKind::Up(MouseButton::Left),
        beta.x + 2,
        beta_bottom - 1,
    );

    // Tab rows stay tab hits and the header right below them stays a header hit.
    state.compose(WIDTH, 30).expect("composed frame");
    let pressed = mouse_at(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        beta.x + 4,
        beta_bottom - 1,
    );
    assert_eq!(focused_tab_request(&pressed), Some("ws_2_logs"));
    mouse_at(
        &mut state,
        MouseEventKind::Up(MouseButton::Left),
        beta.x + 4,
        beta_bottom - 1,
    );
    let toggled = click(&mut state, infra.x + 3, infra.y);
    assert!(toggled.actions.is_empty(), "{:?}", toggled.actions);
    assert!(state.collapsed_groups.contains(&group_collapse_key("gB")));
}

#[test]
fn group_drag_targets_follow_multi_tab_cards() {
    // Every workspace grouped, so no "ungrouped" header: the end target follows the last card.
    let mut snapshot = multi_tab_drop_fixture();
    snapshot.workspaces.truncate(3);
    let mut state = shell_state(snapshot, &[]);
    state.compose(WIDTH, 30).expect("composed frame");
    let (beta, beta_bottom) = card_geometry(&state, "ws_2");
    let (gamma, gamma_bottom) = card_geometry(&state, "ws_3");
    let work = header(&state, Some("gA"));
    let infra = header(&state, Some("gB"));
    assert_eq!(infra.y, beta_bottom);
    assert!(state
        .hits
        .workspace_groups
        .iter()
        .all(|hit| hit.group_id.is_some()));

    let target = |state: &ClientShellState| match &state.chrome_drag {
        Some(ClientChromeDrag::WorkspaceGroup { target, .. }) => target.clone(),
        _ => panic!("a group drag is in progress"),
    };
    mouse_at(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        work.x + 3,
        work.y,
    );
    // Above the Infra header is the last tab row of the multi-tab card before it.
    mouse_at(
        &mut state,
        MouseEventKind::Drag(MouseButton::Left),
        work.x + 3,
        beta.bottom(),
    );
    assert_eq!(target(&state), Some((Some("gB".into()), beta_bottom - 1)));
    // From a tab row of the last card, the end target sits below its tab rows.
    mouse_at(
        &mut state,
        MouseEventKind::Drag(MouseButton::Left),
        work.x + 3,
        gamma.bottom(),
    );
    assert_eq!(target(&state), Some((None, gamma_bottom)));
    let frame = state.compose(WIDTH, 30).expect("dragging frame").frame;
    let end = Rect::new(gamma.x, gamma_bottom, gamma.width, 1);
    assert!(row_text(&frame, end).starts_with("──"));
    for (tab_row, label) in (gamma.bottom()..gamma_bottom).zip(["editor", "logs"]) {
        let row = row_text(&frame, Rect::new(gamma.x, tab_row, gamma.width, 1));
        assert!(
            !row.starts_with("──") && row.contains(label),
            "tab row {tab_row}: {row}"
        );
    }
    let dropped = mouse_at(
        &mut state,
        MouseEventKind::Up(MouseButton::Left),
        work.x + 3,
        gamma.bottom(),
    );
    assert_eq!(
        sent_ops(&dropped),
        [vec![WorkspaceGroupOp::Move {
            group_id: "gA".into(),
            before_group_id: None,
        }]]
    );
}

#[test]
fn workspace_drag_target_header_draws_on_surface1() {
    let mut state = shell_state(multi_tab_drop_fixture(), &[]);
    state.compose(WIDTH, 30).expect("composed frame");
    let palette = state.config.palette.clone();
    let surface1 = u32_color(palette.surface1);
    let (beta, beta_bottom) = card_geometry(&state, "ws_2");
    let work = header(&state, Some("gA"));
    let infra = header(&state, Some("gB"));
    let ungrouped = header(&state, None);
    let row_bgs = |frame: &FrameData, rect: Rect| {
        (rect.x..rect.right())
            .map(|x| cell(frame, x, rect.y).bg)
            .collect::<HashSet<_>>()
    };

    // The targeted header keeps its bar, name and toggle (no drop line over it) on surface1.
    let frame = drag_ws_4_frame(&mut state, (infra.x + 3, infra.y), 30);
    assert!(row_text(&frame, infra).starts_with("▌ Infra"));
    assert!(!row_text(&frame, infra).contains('─'));
    assert_eq!(row_bgs(&frame, infra), HashSet::from([surface1]));
    let bar = cell(&frame, infra.x, infra.y);
    assert_eq!(
        bar.fg,
        u32_color(readable(rgb("teal"), palette.surface1, host_bg(&state)))
    );
    assert!(!row_bgs(&frame, work).contains(&surface1));
    assert!(!row_bgs(&frame, ungrouped).contains(&surface1));

    // So does the "ungrouped" header, which ungroups.
    let frame = drag_ws_4_frame(&mut state, (ungrouped.x + 3, ungrouped.y), 30);
    assert_eq!(row_text(&frame, ungrouped).trim_end(), "  ungrouped");
    assert_eq!(row_bgs(&frame, ungrouped), HashSet::from([surface1]));
    assert!(!row_bgs(&frame, infra).contains(&surface1));

    // A slot between cards keeps the drop line and leaves every header on the sidebar.
    let frame = drag_ws_4_frame(&mut state, (beta.x + 2, beta_bottom - 1), 30);
    assert!(row_text(&frame, Rect::new(beta.x, beta_bottom - 1, beta.width, 1)).starts_with("──"));
    for header in [work, infra, ungrouped] {
        assert!(!row_bgs(&frame, header).contains(&surface1), "{header:?}");
    }

    // A group drag targets the row above a header, never the header itself.
    mouse_at(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        work.x + 3,
        work.y,
    );
    mouse_at(
        &mut state,
        MouseEventKind::Drag(MouseButton::Left),
        work.x + 3,
        infra.y,
    );
    let frame = state
        .compose(WIDTH, 30)
        .expect("group dragging frame")
        .frame;
    assert!(row_text(&frame, infra).starts_with("▌ Infra"));
    assert!(!row_bgs(&frame, infra).contains(&surface1));

    // Multi-machine sidebar: the active machine's targeted header is filled the same way.
    let (mut state, _) = state_with_remote_snapshots(multi_tab_drop_fixture(), snapshot());
    state.compose(WIDTH, 40).expect("multi-machine frame");
    let infra = header(&state, Some("gB"));
    let frame = drag_ws_4_frame(&mut state, (infra.x + 3, infra.y), 40);
    let text = row_text(&frame, infra);
    let bar_x = infra.x + (text.chars().position(|symbol| symbol == '▌').expect("bar") as u16);
    assert!(text.trim_start().starts_with("▌ Infra"));
    assert_eq!(
        row_bgs(&frame, Rect::new(bar_x, infra.y, infra.right() - bar_x, 1)),
        HashSet::from([surface1])
    );
}

/// Drags ws_4 to `to` and returns the frame drawn mid-drag; the drop is then released and the
/// sidebar redrawn, so the hits match the resting layout again.
fn drag_ws_4_frame(state: &mut ClientShellState, to: (u16, u16), height: u16) -> FrameData {
    let from = grab(state, "ws_4");
    mouse_at(
        state,
        MouseEventKind::Down(MouseButton::Left),
        from.0,
        from.1,
    );
    mouse_at(state, MouseEventKind::Drag(MouseButton::Left), to.0, to.1);
    let frame = state.compose(WIDTH, height).expect("dragging frame").frame;
    mouse_at(state, MouseEventKind::Up(MouseButton::Left), to.0, to.1);
    state.compose(WIDTH, height).expect("composed frame");
    frame
}

fn menu_labels(state: &ClientShellState) -> Vec<&'static str> {
    match &state.overlay {
        Some(ClientShellOverlay::ContextMenu(menu)) => {
            menu.items().iter().map(|item| item.label).collect()
        }
        other => panic!("a menu is open, got {other:?}"),
    }
}

fn choose(state: &mut ClientShellState, label: &str) -> ClientShellInput {
    let labels = menu_labels(state);
    let index = labels
        .iter()
        .position(|candidate| *candidate == label)
        .unwrap_or_else(|| panic!("{label:?} in {labels:?}"));
    let mut outcome = ClientShellInput::default();
    state.activate_context_menu_item(index, &mut outcome);
    outcome
}

/// The open prompt's title and prefilled text.
fn prompt(state: &ClientShellState) -> (&'static str, String) {
    match &state.overlay {
        Some(ClientShellOverlay::Rename(rename)) => {
            (rename.title, rename.input.as_str().to_owned())
        }
        other => panic!("a prompt is open, got {other:?}"),
    }
}

fn submit_prompt(state: &mut ClientShellState, text: &str) -> ClientShellInput {
    match state.overlay.as_mut() {
        Some(ClientShellOverlay::Rename(rename)) => rename.input = TextEditor::new(text, false),
        other => panic!("a prompt is open, got {other:?}"),
    }
    let mut outcome = ClientShellInput::default();
    state.save_rename_overlay(&mut outcome);
    outcome
}

fn right_click(state: &mut ClientShellState, column: u16, row: u16) -> ClientShellInput {
    mouse_at(state, MouseEventKind::Down(MouseButton::Right), column, row)
}

/// Opens the Organize menu of a workspace through its right-click menu.
fn open_organize(state: &mut ClientShellState, workspace_id: &str) {
    state.overlay = None;
    state.compose(WIDTH, 30).expect("composed frame");
    let (column, row) = grab(state, workspace_id);
    right_click(state, column, row);
    assert_eq!(menu_labels(state).last(), Some(&"Organize..."));
    choose(state, "Organize...");
}

fn update(group_id: &str, name: Option<&str>, color: Option<&str>) -> Vec<Vec<WorkspaceGroupOp>> {
    vec![vec![WorkspaceGroupOp::Update {
        group_id: group_id.into(),
        name: name.map(str::to_owned),
        color: color.map(str::to_owned),
        template: None,
    }]]
}

const GROUP_ITEMS: [&str; 6] = [
    "Rename group...",
    "Group color...",
    "Collapse",
    "Move group up",
    "Move group down",
    "Delete group",
];

#[test]
fn right_click_header_opens_group_menu_and_rename_sends_update() {
    let mut state = shell_state(three_groups(), &[]);
    state.compose(WIDTH, 30).expect("composed frame");
    let personal = header(&state, Some("gB"));

    let opened = right_click(&mut state, personal.x + 3, personal.y);
    assert!(opened.actions.is_empty());
    assert_eq!(menu_labels(&state), GROUP_ITEMS);
    let Some(ClientShellOverlay::ContextMenu(menu)) = &state.overlay else {
        panic!("group menu");
    };
    assert_eq!((menu.x, menu.y), (personal.x + 3, personal.y));

    choose(&mut state, "Rename group...");
    assert_eq!(prompt(&state), ("rename group", "Personal".to_owned()));
    let renamed = submit_prompt(&mut state, "  Home ");
    assert_eq!(renamed.actions.len(), 1);
    assert_eq!(sent_ops(&renamed), update("gB", Some("Home"), None));
    assert!(state.overlay.is_none());

    // An unchanged or empty name sends nothing.
    right_click(&mut state, personal.x + 3, personal.y);
    choose(&mut state, "Rename group...");
    assert!(submit_prompt(&mut state, "Personal").actions.is_empty());
    right_click(&mut state, personal.x + 3, personal.y);
    choose(&mut state, "Rename group...");
    assert!(submit_prompt(&mut state, " ").actions.is_empty());

    // Color, reorder and delete items act on the clicked group.
    right_click(&mut state, personal.x + 3, personal.y);
    choose(&mut state, "Group color...");
    assert_eq!(
        menu_labels(&state),
        ["Red", "Orange", "Yellow", "Green", "Teal", "Blue", "Purple", "Pink", "Gray"]
    );
    assert_eq!(
        sent_ops(&choose(&mut state, "Teal")),
        update("gB", None, Some("teal"))
    );
    for (label, before_group_id) in [("Move group up", Some("gA")), ("Move group down", None)] {
        right_click(&mut state, personal.x + 3, personal.y);
        assert_eq!(
            sent_ops(&choose(&mut state, label)),
            [vec![WorkspaceGroupOp::Move {
                group_id: "gB".into(),
                before_group_id: before_group_id.map(str::to_owned),
            }]]
        );
    }
    right_click(&mut state, personal.x + 3, personal.y);
    assert_eq!(
        sent_ops(&choose(&mut state, "Delete group")),
        [vec![WorkspaceGroupOp::Delete {
            group_id: "gB".into(),
        }]]
    );

    // Collapse is client-local, and the menu then offers Expand.
    right_click(&mut state, personal.x + 3, personal.y);
    assert!(choose(&mut state, "Collapse").actions.is_empty());
    assert!(state.collapsed_groups.contains(&group_collapse_key("gB")));
    state.compose(WIDTH, 30).expect("collapsed frame");
    right_click(&mut state, personal.x + 3, personal.y);
    assert!(menu_labels(&state).contains(&"Expand"));

    // The first group cannot move up; the "ungrouped" header has no menu.
    state.overlay = None;
    let work = header(&state, Some("gA"));
    right_click(&mut state, work.x + 3, work.y);
    assert!(choose(&mut state, "Move group up").actions.is_empty());
    let mut snapshot = three_groups();
    snapshot.workspaces.push(workspace("ws_4", "delta"));
    let mut state = shell_state(snapshot, &[]);
    state.compose(WIDTH, 30).expect("composed frame");
    let ungrouped = header(&state, None);
    right_click(&mut state, ungrouped.x + 3, ungrouped.y);
    assert!(state.overlay.is_none());
}

#[test]
fn organize_move_to_group_creates_or_places_or_ungroups() {
    let mut state = shell_state(drop_fixture(), &[]);

    open_organize(&mut state, "ws_4");
    assert_eq!(
        menu_labels(&state),
        ["Move to group...", "Workspace color...", "Edit group..."]
    );
    choose(&mut state, "Move to group...");
    assert_eq!(prompt(&state), ("move to group", String::new()));
    assert_eq!(
        sent_ops(&submit_prompt(&mut state, "work")),
        place("ws_4", Some("gA"), None)
    );

    open_organize(&mut state, "ws_4");
    choose(&mut state, "Move to group...");
    assert_eq!(
        sent_ops(&submit_prompt(&mut state, " Ops ")),
        [vec![WorkspaceGroupOp::Create {
            name: "Ops".into(),
            color: None,
            template: None,
            workspace_ids: vec!["ws_4".into()],
            before_group_id: None,
        }]]
    );

    // A grouped workspace is prefilled with its group; an empty name ungroups it.
    open_organize(&mut state, "ws_2");
    let mut expected = vec!["Move to group...", "Workspace color...", "Edit group..."];
    expected.extend(GROUP_ITEMS);
    assert_eq!(menu_labels(&state), expected);
    choose(&mut state, "Move to group...");
    assert_eq!(prompt(&state), ("move to group", "Work".to_owned()));
    assert_eq!(
        sent_ops(&submit_prompt(&mut state, "")),
        place("ws_2", None, None)
    );

    // The group items of the Organize menu act on the workspace's group.
    open_organize(&mut state, "ws_2");
    assert_eq!(
        sent_ops(&choose(&mut state, "Move group down")),
        [vec![WorkspaceGroupOp::Move {
            group_id: "gA".into(),
            before_group_id: Some("gE".into()),
        }]]
    );

    // Workspace color sets or clears the override.
    open_organize(&mut state, "ws_2");
    choose(&mut state, "Workspace color...");
    assert_eq!(
        menu_labels(&state),
        [
            "Red",
            "Orange",
            "Yellow",
            "Green",
            "Teal",
            "Blue",
            "Purple",
            "Pink",
            "Gray",
            "Group color",
        ]
    );
    assert_eq!(
        sent_ops(&choose(&mut state, "Pink")),
        [vec![WorkspaceGroupOp::SetColor {
            workspace_id: "ws_2".into(),
            color: Some("pink".into()),
        }]]
    );
    open_organize(&mut state, "ws_2");
    choose(&mut state, "Workspace color...");
    assert_eq!(
        sent_ops(&choose(&mut state, "Group color")),
        [vec![WorkspaceGroupOp::SetColor {
            workspace_id: "ws_2".into(),
            color: None,
        }]]
    );

    // A collapsed group adds "Expand all groups", which clears every group key.
    state.collapsed_groups.insert(group_collapse_key("gC"));
    state.collapsed_groups.insert("repo".into());
    open_organize(&mut state, "ws_4");
    assert_eq!(menu_labels(&state).last(), Some(&"Expand all groups"));
    assert!(choose(&mut state, "Expand all groups").actions.is_empty());
    assert_eq!(state.collapsed_groups, HashSet::from(["repo".to_owned()]));
}

#[test]
fn edit_group_prompt_opens_menu_for_empty_group() {
    let mut state = shell_state(drop_fixture(), &[]);

    open_organize(&mut state, "ws_1");
    choose(&mut state, "Edit group...");
    assert_eq!(prompt(&state), ("edit group", "Work".to_owned()));

    open_organize(&mut state, "ws_3");
    choose(&mut state, "Edit group...");
    assert_eq!(prompt(&state), ("edit group", String::new()));
    assert!(submit_prompt(&mut state, "EMPTY").actions.is_empty());
    assert_eq!(menu_labels(&state), GROUP_ITEMS);
    choose(&mut state, "Rename group...");
    assert_eq!(prompt(&state), ("rename group", "Empty".to_owned()));
    assert_eq!(
        sent_ops(&submit_prompt(&mut state, "Spare")),
        update("gE", Some("Spare"), None)
    );

    // An unknown name shows the Rejected notice and sends nothing.
    open_organize(&mut state, "ws_3");
    choose(&mut state, "Edit group...");
    let rejected = submit_prompt(&mut state, "nope");
    assert!(rejected.actions.is_empty());
    assert!(state.overlay.is_none());
    let notice = state
        .visible_endpoint_notice
        .as_ref()
        .expect("rejected notice");
    assert_eq!(notice.key.kind, ClientEndpointNoticeKind::Rejected);
    assert_eq!(notice.body, "No group named nope");
}

#[test]
fn upstream_endpoint_without_method_shows_action_unavailable_and_drags_upstream() {
    let mut focused = workspace("ws_1", "alpha");
    focused.focused = true;
    let upstream = grouped_snapshot(vec![focused, workspace("ws_2", "beta")], Vec::new(), &[]);
    let mut state = shell_state(upstream, &[]);
    state.set_endpoint_methods(Some(vec![
        "workspace.move".into(),
        "workspace.focus".into(),
    ]));
    state.compose(WIDTH, 30).expect("composed frame");
    assert!(state.hits.workspace_groups.is_empty());
    assert!(state.hits.workspace_drop_slots.is_empty());

    open_organize(&mut state, "ws_2");
    choose(&mut state, "Move to group...");
    let unavailable = submit_prompt(&mut state, "Work");
    assert!(unavailable.actions.is_empty());
    let notice = state
        .visible_endpoint_notice
        .as_ref()
        .expect("unavailable notice");
    assert_eq!(notice.key.kind, ClientEndpointNoticeKind::Unsupported);
    assert_eq!(notice.title, "Action unavailable");

    // Drags fall back to upstream's workspace.move (the notice toast is dismissed first, as
    // it covers the top of the sidebar).
    state.visible_endpoint_notice = None;
    state.compose(WIDTH, 30).expect("composed frame");
    let first = card(&state, "ws_1");
    let from = grab(&state, "ws_2");
    let dropped = drag(&mut state, from, (first.x + 2, first.y - 1));
    assert!(
        matches!(
            dropped.actions.as_slice(),
            [ClientShellAction::Endpoint { request, .. }]
                if matches!(
                    &request.method,
                    Method::WorkspaceMove(params)
                        if params.workspace_id == "ws_2" && params.insert_index == 0
                )
        ),
        "{:?}",
        dropped.actions
    );
}

/// Work [ws_1], Personal [ws_2, ws_3] (bound to the projects.toml group "Personal"), Infra
/// [ws_4].
fn undo_fixture() -> ClientShellSnapshot {
    let mut focused = workspace("ws_1", "alpha");
    focused.focused = true;
    let mut personal = group("gB", "Personal", "green", &["ws_2", "ws_3"]);
    personal.template = Some("Personal".into());
    grouped_snapshot(
        vec![
            focused,
            workspace("ws_2", "beta"),
            workspace("ws_3", "gamma"),
            workspace("ws_4", "delta"),
        ],
        vec![
            group("gA", "Work", "blue", &["ws_1"]),
            personal,
            group("gC", "Infra", "teal", &["ws_4"]),
        ],
        &[
            ("ws_1", "blue"),
            ("ws_2", "pink"),
            ("ws_3", "green"),
            ("ws_4", "teal"),
        ],
    )
}

/// The snapshot the server projects next, with `change` applied (same revision, so the
/// fixture's pane surface stays current).
fn next_snapshot(
    state: &ClientShellState,
    change: impl FnOnce(&mut ClientShellSnapshot),
) -> Box<ClientShellSnapshot> {
    let mut snapshot = state.snapshot.as_deref().expect("snapshot").clone();
    change(&mut snapshot);
    Box::new(snapshot)
}

fn without_group(snapshot: &mut ClientShellSnapshot, group_id: &str) {
    snapshot
        .workspace_groups
        .groups
        .retain(|group| group.group_id != group_id);
}

fn request_id(outcome: &ClientShellInput) -> String {
    match outcome.actions.as_slice() {
        [ClientShellAction::Endpoint { request, .. }] => request.id.clone(),
        other => panic!("one request, got {other:?}"),
    }
}

fn delete_personal(state: &mut ClientShellState) {
    state.overlay = None;
    state.compose(WIDTH, 30).expect("composed frame");
    let personal = header(state, Some("gB"));
    right_click(state, personal.x + 3, personal.y);
    assert_eq!(
        sent_ops(&choose(state, "Delete group")),
        [vec![WorkspaceGroupOp::Delete {
            group_id: "gB".into(),
        }]]
    );
}

#[test]
fn delete_then_undo_restores_name_color_template_members_and_position() {
    let mut state = shell_state(undo_fixture(), &[]);
    delete_personal(&mut state);
    let deleted = next_snapshot(&state, |snapshot| without_group(snapshot, "gB"));
    state.set_snapshot(deleted);

    open_organize(&mut state, "ws_2");
    assert_eq!(
        menu_labels(&state),
        [
            "Move to group...",
            "Workspace color...",
            "Edit group...",
            "Undo delete group",
        ]
    );
    let restore = vec![vec![WorkspaceGroupOp::Create {
        name: "Personal".into(),
        color: Some("green".into()),
        template: Some("Personal".into()),
        workspace_ids: vec!["ws_2".into(), "ws_3".into()],
        before_group_id: Some("gC".into()),
    }]];
    let undo = choose(&mut state, "Undo delete group");
    assert_eq!(sent_ops(&undo), restore);

    // A rejected undo shows the server's message and keeps the stash.
    let (repaint, _) = state.handle_endpoint_result(
        "boot-1",
        &request_id(&undo),
        Err(ClientShellEndpointError {
            code: Some("workspace_group_name_taken".into()),
            message: "workspace group name Personal is already used".into(),
        }),
    );
    assert!(repaint);
    let notice = state
        .visible_endpoint_notice
        .as_ref()
        .expect("rejected notice");
    assert_eq!(notice.key.kind, ClientEndpointNoticeKind::Rejected);
    assert_eq!(notice.body, "workspace group name Personal is already used");
    state.compose(WIDTH, 30).expect("composed frame");
    let work = header(&state, Some("gA"));
    right_click(&mut state, work.x + 3, work.y);
    assert_eq!(menu_labels(&state).last(), Some(&"Undo delete group"));
    let undo = choose(&mut state, "Undo delete group");
    assert_eq!(sent_ops(&undo), restore);

    // A successful undo clears it.
    state.handle_endpoint_result("boot-1", &request_id(&undo), Ok(ResponseResult::Ok {}));
    open_organize(&mut state, "ws_2");
    assert!(!menu_labels(&state).contains(&"Undo delete group"));

    // Any other organize edit clears it too.
    let mut state = shell_state(undo_fixture(), &[]);
    delete_personal(&mut state);
    let deleted = next_snapshot(&state, |snapshot| without_group(snapshot, "gB"));
    state.set_snapshot(deleted);
    open_organize(&mut state, "ws_2");
    choose(&mut state, "Workspace color...");
    assert_eq!(sent_ops(&choose(&mut state, "Gray")).len(), 1);
    open_organize(&mut state, "ws_2");
    assert!(!menu_labels(&state).contains(&"Undo delete group"));
}

#[test]
fn undo_after_member_closed_restores_remaining_members() {
    let mut state = shell_state(undo_fixture(), &[]);
    delete_personal(&mut state);
    // ws_3 closed and Infra was deleted from another client in the meantime.
    let changed = next_snapshot(&state, |snapshot| {
        without_group(snapshot, "gB");
        without_group(snapshot, "gC");
        snapshot
            .workspaces
            .retain(|workspace| workspace.workspace_id != "ws_3");
    });
    state.set_snapshot(changed);

    open_organize(&mut state, "ws_2");
    assert_eq!(
        sent_ops(&choose(&mut state, "Undo delete group")),
        [vec![WorkspaceGroupOp::Create {
            name: "Personal".into(),
            color: Some("green".into()),
            template: Some("Personal".into()),
            workspace_ids: vec!["ws_2".into()],
            before_group_id: None,
        }]]
    );

    // A server restart (new boot id) drops the stash.
    let mut state = shell_state(undo_fixture(), &[]);
    delete_personal(&mut state);
    let restarted = next_snapshot(&state, |snapshot| {
        without_group(snapshot, "gB");
        snapshot.boot_id = "boot-2".into();
    });
    state.set_snapshot(restarted);
    state.set_pane_surface(surface());
    open_organize(&mut state, "ws_2");
    assert!(!menu_labels(&state).contains(&"Undo delete group"));
}

const SHIFT_UP: &[u8] = b"\x1b[1;2A";
const SHIFT_DOWN: &[u8] = b"\x1b[1;2B";

fn enter_navigate(state: &mut ClientShellState) {
    state.handle_input_bytes(&[0x02]);
    state.handle_input_bytes(b"w");
    assert_eq!(state.mode, ClientShellMode::Navigate);
}

fn selected(state: &ClientShellState) -> Option<&str> {
    state
        .navigate_workspace_id
        .as_ref()
        .map(|target| target.workspace_id.as_str())
}

/// Applies the server's next projection: group members and the ungrouped tail in display
/// order, with the workspace Vec reordered to match (as the server keeps it).
fn regroup(state: &mut ClientShellState, groups: &[(&str, &[&str])], ungrouped: &[&str]) {
    let order = groups
        .iter()
        .flat_map(|(_, members)| members.iter())
        .chain(ungrouped)
        .copied()
        .collect::<Vec<_>>();
    let next = next_snapshot(state, |snapshot| {
        for (group_id, members) in groups {
            if let Some(group) = snapshot
                .workspace_groups
                .groups
                .iter_mut()
                .find(|group| group.group_id == *group_id)
            {
                group.workspace_ids = members.iter().map(|id| (*id).to_owned()).collect();
            }
        }
        snapshot.workspaces.sort_by_key(|workspace| {
            order
                .iter()
                .position(|id| *id == workspace.workspace_id)
                .unwrap_or(usize::MAX)
        });
    });
    state.set_snapshot(next);
    state.compose(WIDTH, 30).expect("composed frame");
}

#[test]
fn navigate_shift_down_crosses_group_boundary_and_expands_collapsed_target() {
    let key = group_collapse_key("gC");
    let mut state = shell_state(drop_fixture(), &[key.as_str()]);
    state.compose(WIDTH, 30).expect("composed frame");
    enter_navigate(&mut state);
    assert_eq!(selected(&state), Some("ws_1"));

    // Inside a group it swaps with its neighbor; navigate mode and the selection stay.
    let moved = state.handle_input_bytes(SHIFT_DOWN);
    assert_eq!(sent_ops(&moved), place("ws_1", Some("gA"), None));
    assert_eq!(state.mode, ClientShellMode::Navigate);
    assert_eq!(selected(&state), Some("ws_1"));
    regroup(
        &mut state,
        &[("gA", &["ws_2", "ws_1"]), ("gC", &["ws_5"]), ("gE", &[])],
        &["ws_3", "ws_4"],
    );

    // From the bottom of a group it joins the start of the next one, expanding it.
    let moved = state.handle_input_bytes(SHIFT_DOWN);
    assert_eq!(sent_ops(&moved), place("ws_1", Some("gC"), Some("ws_5")));
    assert!(!state.collapsed_groups.contains(&key));
    regroup(
        &mut state,
        &[("gA", &["ws_2"]), ("gC", &["ws_1", "ws_5"]), ("gE", &[])],
        &["ws_3", "ws_4"],
    );
    let moved = state.handle_input_bytes(SHIFT_DOWN);
    assert_eq!(sent_ops(&moved), place("ws_1", Some("gC"), None));
    regroup(
        &mut state,
        &[("gA", &["ws_2"]), ("gC", &["ws_5", "ws_1"]), ("gE", &[])],
        &["ws_3", "ws_4"],
    );

    // Empty groups are a stop of their own, then the ungrouped region.
    let moved = state.handle_input_bytes(SHIFT_DOWN);
    assert_eq!(sent_ops(&moved), place("ws_1", Some("gE"), None));
    regroup(
        &mut state,
        &[("gA", &["ws_2"]), ("gC", &["ws_5"]), ("gE", &["ws_1"])],
        &["ws_3", "ws_4"],
    );
    let moved = state.handle_input_bytes(SHIFT_DOWN);
    assert_eq!(sent_ops(&moved), place("ws_1", None, Some("ws_3")));
    regroup(
        &mut state,
        &[("gA", &["ws_2"]), ("gC", &["ws_5"]), ("gE", &[])],
        &["ws_1", "ws_3", "ws_4"],
    );

    // Up from the top of a section joins the end of the previous one.
    let moved = state.handle_input_bytes(SHIFT_UP);
    assert_eq!(sent_ops(&moved), place("ws_1", Some("gE"), None));

    // Nothing moves past the last section or above the first one.
    regroup(
        &mut state,
        &[("gA", &["ws_2"]), ("gC", &["ws_5"]), ("gE", &[])],
        &["ws_3", "ws_4", "ws_1"],
    );
    assert!(state.handle_input_bytes(SHIFT_DOWN).actions.is_empty());
    regroup(
        &mut state,
        &[("gA", &["ws_1", "ws_2"]), ("gC", &["ws_5"]), ("gE", &[])],
        &["ws_3", "ws_4"],
    );
    assert!(state.handle_input_bytes(SHIFT_UP).actions.is_empty());
    assert_eq!(state.mode, ClientShellMode::Navigate);
}

#[test]
fn navigate_m_opens_organize_menu() {
    let mut state = shell_state(drop_fixture(), &[]);
    state.compose(WIDTH, 30).expect("composed frame");
    enter_navigate(&mut state);
    state.handle_input_bytes(b"\x1b[B");
    assert_eq!(selected(&state), Some("ws_2"));
    let row = card(&state, "ws_2");

    let opened = state.handle_input_bytes(b"m");
    assert!(opened.actions.is_empty());
    assert_ne!(state.mode, ClientShellMode::Navigate);
    let Some(ClientShellOverlay::ContextMenu(menu)) = &state.overlay else {
        panic!("organize menu");
    };
    assert_eq!((menu.x, menu.y), (row.x, row.y));
    let mut expected = vec!["Move to group...", "Workspace color...", "Edit group..."];
    expected.extend(GROUP_ITEMS);
    assert_eq!(menu_labels(&state), expected);
    choose(&mut state, "Move to group...");
    assert_eq!(prompt(&state), ("move to group", "Work".to_owned()));
}

#[test]
fn user_binding_wins_over_navigate_organize_keys() {
    let config: Config = toml::from_str(
        "[keys]\nnavigate_workspace_down = \"shift+down\"\nsplit_vertical = \"prefix+m\"\n",
    )
    .expect("keys config");
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(drop_fixture()));
    state.set_pane_surface(surface());
    state.compose(WIDTH, 30).expect("composed frame");
    enter_navigate(&mut state);

    let moved = state.handle_input_bytes(SHIFT_DOWN);
    assert!(sent_ops(&moved).is_empty());
    assert_eq!(selected(&state), Some("ws_2"));
    assert_eq!(state.mode, ClientShellMode::Navigate);

    let split = state.handle_input_bytes(b"m");
    assert!(sent_ops(&split).is_empty());
    assert!(!matches!(
        &state.overlay,
        Some(ClientShellOverlay::ContextMenu(_))
    ));
    assert!(split.actions.iter().any(|action| matches!(
        action,
        ClientShellAction::Endpoint { request, .. }
            if matches!(&request.method, Method::PaneSplit(_))
    )));

    // Digits keep switching workspaces in navigate mode.
    enter_navigate(&mut state);
    let switched = state.handle_input_bytes(b"2");
    assert!(sent_ops(&switched).is_empty());
    assert_eq!(state.mode, ClientShellMode::Terminal);
}
