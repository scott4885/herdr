//! Workspace groups: pure server model.
//!
//! One level of named, colored groups. `AppState.workspaces` is kept in region
//! order (members of `groups[0]`, then `groups[1]`, ..., then every ungrouped
//! workspace). Everything here is pure data and pure functions; loading, saving,
//! events and rendering live in `app::api::workspace_groups`.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::api::schema::WorkspaceGroupOp;
use crate::persist::workspace_groups::{
    WorkspaceGroupFileEntry, WorkspaceGroupsFile, WorkspaceRecordFileEntry, SIDECAR_VERSION,
};
use crate::protocol::{
    normalize_workspace_color, ClientShellWorkspaceGroup, ClientShellWorkspaceGroups,
};

pub(crate) const MAX_GROUP_NAME_CHARS: usize = 40;
pub(crate) const MAX_TAB_BINDINGS: usize = 32;
pub(crate) const DEFAULT_GROUP_COLOR_CYCLE: [&str; 9] = [
    "blue", "green", "purple", "orange", "teal", "pink", "gray", "red", "yellow",
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WorkspaceGroups {
    pub(crate) groups: Vec<WorkspaceGroup>, // Vec order = display order
    pub(crate) workspaces: BTreeMap<String, WorkspaceRecord>, // key = Workspace.id ("w3"), incl. linked children
    pub(crate) deleted_templates: Vec<String>, // projects.toml group names deleted in the UI
    // runtime only, never persisted:
    pub(crate) last_order: Option<Vec<String>>, // live ids at the last reconcile; None before the first
    pub(crate) pending_prune: Vec<String>,      // closed ids waiting for a settled session save
    pub(crate) persist_blocked: bool, // set when an unreadable sidecar could not be set aside; never save then
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceGroup {
    pub(crate) id: String, // "g19a3f2c1b7"
    pub(crate) name: String,
    pub(crate) color: String,            // canonical lowercase palette name
    pub(crate) template: Option<String>, // projects.toml group name (file spelling)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WorkspaceRecord {
    pub(crate) group: Option<String>,         // group id
    pub(crate) color: Option<String>,         // per-workspace override
    pub(crate) project: Option<String>,       // projects.toml project name
    pub(crate) tabs: BTreeMap<String, usize>, // tab label (file spelling) -> public tab number
}

/// Live facts the pure functions need about one workspace, in current `AppState.workspaces` order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceMeta {
    pub(crate) id: String,                   // Workspace.id
    pub(crate) worktree_key: Option<String>, // Workspace::worktree_space().map(|s| s.key)
    pub(crate) linked: bool,                 // worktree_space().is_linked_worktree
    pub(crate) tabs: Vec<(String, usize)>,   // (canonical public tab id "w3:t1", public tab number)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GroupEditError {
    WorkspaceNotFound(String),  // "workspace_not_found"
    TabNotFound(String),        // "tab_not_found"; holds the full message (see `tab_not_found`)
    GroupNotFound(String),      // "workspace_group_not_found"
    NameTaken(String),          // "workspace_group_name_taken"
    InvalidName,                // "invalid_workspace_group_name"
    InvalidColor(String),       // "invalid_color"
    TooManyTabBindings(String), // "too_many_tab_bindings"
}

impl GroupEditError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::WorkspaceNotFound(_) => "workspace_not_found",
            Self::TabNotFound(_) => "tab_not_found",
            Self::GroupNotFound(_) => "workspace_group_not_found",
            Self::NameTaken(_) => "workspace_group_name_taken",
            Self::InvalidName => "invalid_workspace_group_name",
            Self::InvalidColor(_) => "invalid_color",
            Self::TooManyTabBindings(_) => "too_many_tab_bindings",
        }
    }

    pub(crate) fn message(&self) -> String {
        match self {
            Self::WorkspaceNotFound(workspace_id) => format!("workspace {workspace_id} not found"),
            Self::TabNotFound(message) => message.clone(),
            Self::GroupNotFound(group_id) => format!("workspace group {group_id} not found"),
            Self::NameTaken(name) => format!("workspace group name {name} is already used"),
            Self::InvalidName => format!(
                "workspace group names must be 1-{MAX_GROUP_NAME_CHARS} characters without control characters"
            ),
            Self::InvalidColor(name) => format!(
                "unknown color {name}; use one of red, orange, yellow, green, teal, blue, purple, pink, gray"
            ),
            Self::TooManyTabBindings(workspace_id) => {
                format!("workspace {workspace_id} already has {MAX_TAB_BINDINGS} tab bindings")
            }
        }
    }

    /// A tab id that does not resolve to any tab.
    pub(crate) fn tab_not_found(tab_id: &str) -> Self {
        Self::TabNotFound(format!("tab {tab_id} not found"))
    }

    /// A tab id that resolves to a tab of another workspace.
    pub(crate) fn tab_not_found_in_workspace(tab_id: &str, workspace_id: &str) -> Self {
        Self::TabNotFound(format!(
            "tab {tab_id} not found in workspace {workspace_id}"
        ))
    }
}

impl WorkspaceMeta {
    /// Live facts of one workspace, including its canonical public tab ids.
    pub(crate) fn from_workspace(workspace: &crate::workspace::Workspace) -> Self {
        Self {
            tabs: workspace
                .tabs
                .iter()
                .map(|tab| {
                    (
                        crate::workspace::public_tab_id_for_number(&workspace.id, tab.number),
                        tab.number,
                    )
                })
                .collect(),
            ..Self::without_tabs(workspace)
        }
    }

    /// Identity and worktree facts only: enough for ordering and the client projection.
    pub(crate) fn without_tabs(workspace: &crate::workspace::Workspace) -> Self {
        let space = workspace.worktree_space();
        Self {
            id: workspace.id.clone(),
            worktree_key: space.map(|space| space.key.clone()),
            linked: space.is_some_and(|space| space.is_linked_worktree),
            tabs: Vec::new(),
        }
    }
}

impl WorkspaceRecord {
    fn is_empty(&self) -> bool {
        self.group.is_none()
            && self.color.is_none()
            && self.project.is_none()
            && self.tabs.is_empty()
    }
}

impl WorkspaceGroups {
    pub(crate) fn from_file(file: WorkspaceGroupsFile) -> Self {
        Self {
            groups: file
                .groups
                .into_iter()
                .map(|group| WorkspaceGroup {
                    id: group.id,
                    name: group.name,
                    color: group.color,
                    template: group.template,
                })
                .collect(),
            workspaces: file
                .workspaces
                .into_iter()
                .map(|(id, record)| {
                    (
                        id,
                        WorkspaceRecord {
                            group: record.group,
                            color: record.color,
                            project: record.project,
                            tabs: record.tabs,
                        },
                    )
                })
                .collect(),
            deleted_templates: file.deleted_templates,
            ..Self::default()
        }
    }

    pub(crate) fn to_file(&self) -> WorkspaceGroupsFile {
        WorkspaceGroupsFile {
            version: SIDECAR_VERSION,
            groups: self
                .groups
                .iter()
                .map(|group| WorkspaceGroupFileEntry {
                    id: group.id.clone(),
                    name: group.name.clone(),
                    color: group.color.clone(),
                    template: group.template.clone(),
                })
                .collect(),
            deleted_templates: self.deleted_templates.clone(),
            workspaces: self
                .workspaces
                .iter()
                .map(|(id, record)| {
                    (
                        id.clone(),
                        WorkspaceRecordFileEntry {
                            group: record.group.clone(),
                            color: record.color.clone(),
                            project: record.project.clone(),
                            tabs: record.tabs.clone(),
                        },
                    )
                })
                .collect(),
        }
    }

    pub(crate) fn effective_group(&self, workspace_id: &str) -> Option<&WorkspaceGroup> {
        let group_id = self.workspaces.get(workspace_id)?.group.as_deref()?;
        self.groups.iter().find(|group| group.id == group_id)
    }

    pub(crate) fn effective_color(&self, workspace_id: &str) -> Option<&str> {
        self.workspaces
            .get(workspace_id)
            .and_then(|record| record.color.as_deref())
            .or_else(|| {
                self.effective_group(workspace_id)
                    .map(|group| group.color.as_str())
            })
    }

    fn group_index(&self, group_id: &str) -> Option<usize> {
        self.groups.iter().position(|group| group.id == group_id)
    }

    fn require_group(&self, group_id: &str) -> Result<usize, GroupEditError> {
        self.group_index(group_id)
            .ok_or_else(|| GroupEditError::GroupNotFound(group_id.to_string()))
    }

    fn record_mut(&mut self, workspace_id: &str) -> &mut WorkspaceRecord {
        self.workspaces.entry(workspace_id.to_string()).or_default()
    }
}

/// Region ranks for one pass: group index for members, `groups.len()` for the ungrouped region.
struct Regions<'a> {
    by_group: HashMap<&'a str, usize>,
    model: &'a WorkspaceGroups,
}

impl<'a> Regions<'a> {
    fn new(model: &'a WorkspaceGroups) -> Self {
        let mut by_group = HashMap::with_capacity(model.groups.len());
        for (index, group) in model.groups.iter().enumerate() {
            by_group.entry(group.id.as_str()).or_insert(index);
        }
        Self { by_group, model }
    }

    fn of_group(&self, group_id: Option<&str>) -> usize {
        group_id
            .and_then(|group_id| self.by_group.get(group_id).copied())
            .unwrap_or(self.model.groups.len())
    }

    fn of(&self, workspace_id: &str) -> usize {
        self.of_group(
            self.model
                .workspaces
                .get(workspace_id)
                .and_then(|record| record.group.as_deref()),
        )
    }
}

fn sort_by_region(model: &WorkspaceGroups, order: &mut [String]) {
    let regions = Regions::new(model);
    order.sort_by_cached_key(|workspace_id| regions.of(workspace_id));
}

fn same_name(left: &str, right: &str) -> bool {
    left == right || left.to_lowercase() == right.to_lowercase()
}

fn validate_name(name: &str) -> Result<String, GroupEditError> {
    let name = name.trim();
    let chars = name.chars().count();
    if chars == 0 || chars > MAX_GROUP_NAME_CHARS || name.chars().any(char::is_control) {
        return Err(GroupEditError::InvalidName);
    }
    Ok(name.to_string())
}

fn ensure_name_free(
    model: &WorkspaceGroups,
    name: &str,
    except_group: Option<&str>,
) -> Result<(), GroupEditError> {
    if model
        .groups
        .iter()
        .any(|group| Some(group.id.as_str()) != except_group && same_name(&group.name, name))
    {
        return Err(GroupEditError::NameTaken(name.to_string()));
    }
    Ok(())
}

fn validate_color(color: &str) -> Result<&'static str, GroupEditError> {
    normalize_workspace_color(color).ok_or_else(|| GroupEditError::InvalidColor(color.to_string()))
}

/// First color of the cycle used by the fewest groups, so the cycle repeats once all are used.
fn default_group_color(model: &WorkspaceGroups) -> String {
    let mut best = DEFAULT_GROUP_COLOR_CYCLE[0];
    let mut best_uses = usize::MAX;
    for color in DEFAULT_GROUP_COLOR_CYCLE {
        let uses = model
            .groups
            .iter()
            .filter(|group| group.color == color)
            .count();
        if uses < best_uses {
            best = color;
            best_uses = uses;
        }
    }
    best.to_string()
}

fn new_group_id(model: &WorkspaceGroups, now_ms: u64) -> String {
    let mut ms = now_ms;
    loop {
        let id = format!("g{ms:x}");
        if model.group_index(&id).is_none() {
            return id;
        }
        ms = ms.wrapping_add(1);
    }
}

fn bind_template(model: &mut WorkspaceGroups, group_index: usize, template: String) {
    for (index, group) in model.groups.iter_mut().enumerate() {
        if index != group_index
            && group
                .template
                .as_deref()
                .is_some_and(|bound| same_name(bound, &template))
        {
            group.template = None;
        }
    }
    model
        .deleted_templates
        .retain(|deleted| !same_name(deleted, &template));
    if let Some(group) = model.groups.get_mut(group_index) {
        group.template = Some(template);
    }
}

fn require_live<'a>(
    live: &'a [WorkspaceMeta],
    workspace_id: &str,
) -> Result<&'a WorkspaceMeta, GroupEditError> {
    live.iter()
        .find(|meta| meta.id == workspace_id)
        .ok_or_else(|| GroupEditError::WorkspaceNotFound(workspace_id.to_string()))
}

/// Spec 6.5 Place. `order` must be in region order and stays in region order.
fn place_workspace(
    model: &mut WorkspaceGroups,
    live: &[WorkspaceMeta],
    order: &mut Vec<String>,
    workspace_id: &str,
    group_id: Option<&str>,
    before_workspace_id: Option<&str>,
) -> Result<(), GroupEditError> {
    let meta = require_live(live, workspace_id)?;
    if let Some(group_id) = group_id {
        model.require_group(group_id)?;
    }

    // A sole non-linked member of a worktree key takes the key's linked children
    // in its current region along; a linked child always moves alone.
    let mut unit = vec![meta.id.clone()];
    if let (false, Some(key)) = (meta.linked, meta.worktree_key.as_deref()) {
        let regions = Regions::new(model);
        let source = regions.of(&meta.id);
        let in_family_here = |other: &WorkspaceMeta| {
            other.worktree_key.as_deref() == Some(key) && regions.of(&other.id) == source
        };
        let sole_parent = !live
            .iter()
            .any(|other| other.id != meta.id && !other.linked && in_family_here(other));
        if sole_parent {
            for other_id in order.iter() {
                if let Some(other) = live.iter().find(|other| &other.id == other_id) {
                    if other.linked && in_family_here(other) {
                        unit.push(other.id.clone());
                    }
                }
            }
        }
    }

    for member in &unit {
        model.record_mut(member).group = group_id.map(str::to_string);
    }
    order.retain(|id| !unit.contains(id));

    let regions = Regions::new(model);
    let target = regions.of_group(group_id);
    let before_index = before_workspace_id
        .filter(|before| !unit.iter().any(|member| member == before))
        .filter(|before| regions.of(before) == target)
        .and_then(|before| order.iter().position(|id| id == before));
    let index = before_index.unwrap_or_else(|| {
        order
            .iter()
            .rposition(|id| regions.of(id) <= target)
            .map_or(0, |last| last + 1)
    });
    order.splice(index..index, unit);
    Ok(())
}

fn apply_op(
    model: &mut WorkspaceGroups,
    live: &[WorkspaceMeta],
    order: &mut Vec<String>,
    op: &WorkspaceGroupOp,
    now_ms: u64,
) -> Result<(), GroupEditError> {
    match op {
        WorkspaceGroupOp::Create {
            name,
            color,
            template,
            workspace_ids,
            before_group_id,
        } => {
            let name = validate_name(name)?;
            ensure_name_free(model, &name, None)?;
            let color = match color {
                Some(color) => validate_color(color)?.to_string(),
                None => default_group_color(model),
            };
            let id = new_group_id(model, now_ms);
            let index = before_group_id
                .as_deref()
                .and_then(|before| model.group_index(before))
                .unwrap_or(model.groups.len());
            model.groups.insert(
                index,
                WorkspaceGroup {
                    id: id.clone(),
                    name,
                    color,
                    template: None,
                },
            );
            if let Some(template) = template {
                bind_template(model, index, template.clone());
            }
            sort_by_region(model, order);
            for workspace_id in workspace_ids {
                place_workspace(model, live, order, workspace_id, Some(&id), None)?;
            }
        }
        WorkspaceGroupOp::Update {
            group_id,
            name,
            color,
            template,
        } => {
            let index = model.require_group(group_id)?;
            let name = name
                .as_deref()
                .map(|name| {
                    let name = validate_name(name)?;
                    ensure_name_free(model, &name, Some(group_id))?;
                    Ok(name)
                })
                .transpose()?;
            let color = color.as_deref().map(validate_color).transpose()?;
            if let Some(group) = model.groups.get_mut(index) {
                if let Some(name) = name {
                    group.name = name;
                }
                if let Some(color) = color {
                    group.color = color.to_string();
                }
            }
            if let Some(template) = template {
                bind_template(model, index, template.clone());
            }
        }
        WorkspaceGroupOp::Delete { group_id } => {
            let index = model.require_group(group_id)?;
            let group = model.groups.remove(index);
            if let Some(template) = group.template {
                if !model
                    .deleted_templates
                    .iter()
                    .any(|deleted| same_name(deleted, &template))
                {
                    model.deleted_templates.push(template);
                }
            }
            for record in model.workspaces.values_mut() {
                if record.group.as_deref() == Some(group.id.as_str()) {
                    record.group = None;
                }
            }
            // The deleted region sat ahead of the ungrouped one, so a stable sort
            // puts its former members at the head of the ungrouped region.
            sort_by_region(model, order);
        }
        WorkspaceGroupOp::Move {
            group_id,
            before_group_id,
        } => {
            let index = model.require_group(group_id)?;
            // Moving a group in front of itself leaves it where it is.
            if before_group_id.as_deref() != Some(group_id.as_str()) {
                let group = model.groups.remove(index);
                let target = before_group_id
                    .as_deref()
                    .and_then(|before| model.group_index(before))
                    .unwrap_or(model.groups.len());
                model.groups.insert(target, group);
                sort_by_region(model, order);
            }
        }
        WorkspaceGroupOp::Place {
            workspace_id,
            group_id,
            before_workspace_id,
        } => place_workspace(
            model,
            live,
            order,
            workspace_id,
            group_id.as_deref(),
            before_workspace_id.as_deref(),
        )?,
        WorkspaceGroupOp::SetColor {
            workspace_id,
            color,
        } => {
            require_live(live, workspace_id)?;
            let color = color
                .as_deref()
                .map(validate_color)
                .transpose()?
                .map(str::to_string);
            model.record_mut(workspace_id).color = color;
        }
        WorkspaceGroupOp::Bind {
            workspace_id,
            project,
        } => {
            require_live(live, workspace_id)?;
            bind_project(model, workspace_id, project.as_deref());
        }
        WorkspaceGroupOp::BindTab {
            workspace_id,
            key,
            tab_id,
        } => {
            let meta = require_live(live, workspace_id)?;
            match tab_id {
                Some(tab_id) => bind_tab(model, meta, key, tab_id)?,
                None => {
                    if let Some(record) = model.workspaces.get_mut(workspace_id) {
                        record.tabs.retain(|bound, _| !same_name(bound, key));
                    }
                }
            }
        }
    }
    Ok(())
}

fn bind_project(model: &mut WorkspaceGroups, workspace_id: &str, project: Option<&str>) {
    let Some(project) = project else {
        if let Some(record) = model.workspaces.get_mut(workspace_id) {
            record.project = None;
            record.tabs.clear();
        }
        return;
    };
    for (id, record) in &mut model.workspaces {
        if id != workspace_id
            && record
                .project
                .as_deref()
                .is_some_and(|bound| same_name(bound, project))
        {
            record.project = None;
            record.tabs.clear();
        }
    }
    model.record_mut(workspace_id).project = Some(project.to_string());
}

fn bind_tab(
    model: &mut WorkspaceGroups,
    meta: &WorkspaceMeta,
    key: &str,
    tab_id: &str,
) -> Result<(), GroupEditError> {
    let number = meta
        .tabs
        .iter()
        .find(|(live_tab_id, _)| live_tab_id == tab_id)
        .map(|(_, number)| *number)
        .ok_or_else(|| GroupEditError::tab_not_found_in_workspace(tab_id, &meta.id))?;
    let record = model.record_mut(&meta.id);
    record
        .tabs
        .retain(|_, bound| meta.tabs.iter().any(|(_, live)| live == bound));
    let existing = record
        .tabs
        .keys()
        .find(|bound| same_name(bound, key))
        .cloned();
    match existing {
        Some(existing) => {
            record.tabs.remove(&existing);
        }
        None if record.tabs.len() >= MAX_TAB_BINDINGS => {
            return Err(GroupEditError::TooManyTabBindings(meta.id.clone()));
        }
        None => {}
    }
    record.tabs.insert(key.to_string(), number);
    Ok(())
}

pub(crate) fn plan_edit(
    model: &WorkspaceGroups,
    live: &[WorkspaceMeta],
    ops: &[WorkspaceGroupOp], // ids already canonicalized by the handler
    now_ms: u64,
) -> Result<(WorkspaceGroups, Vec<String>), GroupEditError> {
    let mut next = model.clone();
    let mut order = normalized_order(&next, live);
    for op in ops {
        apply_op(&mut next, live, &mut order, op, now_ms)?;
    }
    if !ops.is_empty() {
        cleanup_records(&mut next, live);
    }
    sort_by_region(&next, &mut order);
    Ok((next, order))
}

pub(crate) fn normalized_order(model: &WorkspaceGroups, live: &[WorkspaceMeta]) -> Vec<String> {
    let mut order: Vec<String> = live.iter().map(|meta| meta.id.clone()).collect();
    sort_by_region(model, &mut order);
    order
}

pub(crate) fn project_client_shell(
    model: &WorkspaceGroups,
    live: &[WorkspaceMeta],
) -> ClientShellWorkspaceGroups {
    if model.groups.is_empty()
        && !live.iter().any(|meta| {
            model
                .workspaces
                .get(&meta.id)
                .is_some_and(|record| record.color.is_some())
        })
    {
        return ClientShellWorkspaceGroups::default();
    }

    let regions = Regions::new(model);
    let mut groups: Vec<ClientShellWorkspaceGroup> = model
        .groups
        .iter()
        .map(|group| ClientShellWorkspaceGroup {
            group_id: group.id.clone(),
            name: group.name.clone(),
            color: group.color.clone(),
            template: group.template.clone(),
            workspace_ids: Vec::new(),
        })
        .collect();
    let mut workspace_colors = Vec::new();
    for meta in live {
        if let Some(group) = groups.get_mut(regions.of(&meta.id)) {
            group.workspace_ids.push(meta.id.clone());
        }
        if let Some(color) = model.effective_color(&meta.id) {
            workspace_colors.push((meta.id.clone(), color.to_string()));
        }
    }
    ClientShellWorkspaceGroups {
        groups,
        workspace_colors,
    }
}

/// Result of one reconcile pass (spec 6.2); the caller applies `order` with
/// `AppState::move_workspace_block` and saves when `records_changed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReconcileOutcome {
    pub(crate) records_changed: bool,
    pub(crate) order: Vec<String>,
}

/// Reconcile the model with the live workspace sequence (spec 6.2). The caller
/// skips this when the id sequence is unchanged and no prune is due, and sets
/// `last_order` after applying the order.
pub(crate) fn reconcile(
    model: &mut WorkspaceGroups,
    live: &[WorkspaceMeta],
    session_save_settled: bool,
) -> ReconcileOutcome {
    let before = model.workspaces.clone();
    let live_ids: HashSet<&str> = live.iter().map(|meta| meta.id.as_str()).collect();

    // In-process close (5.5 rule 1): wait until session.json no longer lists the id.
    if let Some(previous) = &model.last_order {
        for id in previous {
            if !live_ids.contains(id.as_str()) && !model.pending_prune.contains(id) {
                model.pending_prune.push(id.clone());
            }
        }
    }
    model
        .pending_prune
        .retain(|id| !live_ids.contains(id.as_str()));
    if session_save_settled {
        for id in std::mem::take(&mut model.pending_prune) {
            model.workspaces.remove(&id);
        }
    }

    // Linked-child inference runs only for workspaces that appear at runtime.
    if let Some(previous) = &model.last_order {
        let previous: HashSet<&str> = previous.iter().map(String::as_str).collect();
        let inferred: Vec<(String, String)> = live
            .iter()
            .filter(|meta| {
                meta.linked
                    && !previous.contains(meta.id.as_str())
                    && !model.workspaces.contains_key(&meta.id)
            })
            .filter_map(|meta| Some((meta.id.clone(), family_group(model, live, meta)?)))
            .collect();
        for (workspace_id, group_id) in inferred {
            model.record_mut(&workspace_id).group = Some(group_id);
        }
    }

    let records_changed = model.workspaces != before;
    if records_changed {
        cleanup_records(model, live);
    }
    ReconcileOutcome {
        records_changed,
        order: normalized_order(model, live),
    }
}

/// The one effective group shared by every non-linked workspace of the child's
/// worktree key; none when they disagree, are ungrouped, or do not exist.
fn family_group(
    model: &WorkspaceGroups,
    live: &[WorkspaceMeta],
    child: &WorkspaceMeta,
) -> Option<String> {
    let key = child.worktree_key.as_deref()?;
    let mut family = live
        .iter()
        .filter(|other| !other.linked && other.worktree_key.as_deref() == Some(key))
        .map(|other| model.effective_group(&other.id).map(|group| &group.id));
    let first = family.next()??;
    family
        .all(|group| group == Some(first))
        .then(|| first.clone())
}

/// Drop tab bindings to tab numbers that no longer exist in live workspaces and
/// every empty record. Returns true when anything was removed.
pub(crate) fn cleanup_records(model: &mut WorkspaceGroups, live: &[WorkspaceMeta]) -> bool {
    let mut changed = false;
    for meta in live {
        if let Some(record) = model.workspaces.get_mut(&meta.id) {
            let bindings = record.tabs.len();
            record
                .tabs
                .retain(|_, bound| meta.tabs.iter().any(|(_, number)| number == bound));
            changed |= record.tabs.len() != bindings;
        }
    }
    let records = model.workspaces.len();
    model.workspaces.retain(|_, record| !record.is_empty());
    changed || model.workspaces.len() != records
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 0x19a3f2c1b7;

    fn meta(id: &str) -> WorkspaceMeta {
        WorkspaceMeta {
            id: id.into(),
            worktree_key: None,
            linked: false,
            tabs: vec![(format!("{id}:t1"), 1)],
        }
    }

    fn parent(id: &str, key: &str) -> WorkspaceMeta {
        WorkspaceMeta {
            worktree_key: Some(key.into()),
            ..meta(id)
        }
    }

    fn child(id: &str, key: &str) -> WorkspaceMeta {
        WorkspaceMeta {
            worktree_key: Some(key.into()),
            linked: true,
            ..meta(id)
        }
    }

    fn live(ids: &[&str]) -> Vec<WorkspaceMeta> {
        ids.iter().map(|id| meta(id)).collect()
    }

    fn ids(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    fn create(name: &str, workspace_ids: &[&str]) -> WorkspaceGroupOp {
        WorkspaceGroupOp::Create {
            name: name.into(),
            color: None,
            template: None,
            workspace_ids: ids(workspace_ids),
            before_group_id: None,
        }
    }

    fn place(workspace_id: &str, group_id: Option<&str>, before: Option<&str>) -> WorkspaceGroupOp {
        WorkspaceGroupOp::Place {
            workspace_id: workspace_id.into(),
            group_id: group_id.map(str::to_string),
            before_workspace_id: before.map(str::to_string),
        }
    }

    fn bind_tab(workspace_id: &str, key: &str, tab_id: Option<&str>) -> WorkspaceGroupOp {
        WorkspaceGroupOp::BindTab {
            workspace_id: workspace_id.into(),
            key: key.into(),
            tab_id: tab_id.map(str::to_string),
        }
    }

    fn bind(workspace_id: &str, project: Option<&str>) -> WorkspaceGroupOp {
        WorkspaceGroupOp::Bind {
            workspace_id: workspace_id.into(),
            project: project.map(str::to_string),
        }
    }

    fn set_color(workspace_id: &str, color: Option<&str>) -> WorkspaceGroupOp {
        WorkspaceGroupOp::SetColor {
            workspace_id: workspace_id.into(),
            color: color.map(str::to_string),
        }
    }

    /// Applies ops and returns the new model with `live` reordered to the desired order.
    fn apply(
        model: &WorkspaceGroups,
        live: &[WorkspaceMeta],
        ops: &[WorkspaceGroupOp],
    ) -> (WorkspaceGroups, Vec<WorkspaceMeta>) {
        let (model, order) = plan_edit(model, live, ops, NOW).unwrap();
        assert_eq!(
            order.len(),
            live.len(),
            "order must list every live workspace"
        );
        let live = order
            .iter()
            .map(|id| live.iter().find(|meta| &meta.id == id).unwrap().clone())
            .collect();
        (model, live)
    }

    fn order_of(live: &[WorkspaceMeta]) -> Vec<&str> {
        live.iter().map(|meta| meta.id.as_str()).collect()
    }

    fn group_id(model: &WorkspaceGroups, name: &str) -> String {
        model
            .groups
            .iter()
            .find(|group| group.name == name)
            .unwrap()
            .id
            .clone()
    }

    fn members<'a>(model: &WorkspaceGroups, live: &'a [WorkspaceMeta], name: &str) -> Vec<&'a str> {
        let group = group_id(model, name);
        live.iter()
            .filter(|meta| model.effective_group(&meta.id).map(|g| &g.id) == Some(&group))
            .map(|meta| meta.id.as_str())
            .collect()
    }

    fn state_metas(state: &crate::app::AppState) -> Vec<WorkspaceMeta> {
        state
            .workspaces
            .iter()
            .map(WorkspaceMeta::from_workspace)
            .collect()
    }

    #[test]
    fn normalize_is_identity_without_groups() {
        let live = live(&["w1", "w2", "w3"]);
        let mut model = WorkspaceGroups::default();
        assert_eq!(normalized_order(&model, &live), ids(&["w1", "w2", "w3"]));

        model.workspaces.insert(
            "w3".into(),
            WorkspaceRecord {
                group: Some("gmissing".into()),
                color: Some("teal".into()),
                ..WorkspaceRecord::default()
            },
        );
        assert_eq!(normalized_order(&model, &live), ids(&["w1", "w2", "w3"]));
        assert!(model.effective_group("w3").is_none());
        assert_eq!(model.effective_color("w3"), Some("teal"));
    }

    #[test]
    fn normalize_is_stable_idempotent_and_keeps_active_selected_by_id() {
        let mut state = crate::app::AppState::test_new();
        state.workspaces = ["a", "b", "c", "d", "e"]
            .into_iter()
            .map(crate::workspace::Workspace::test_new)
            .collect();
        state.ensure_test_terminals();
        let id = |state: &crate::app::AppState, idx: usize| state.workspaces[idx].id.clone();
        let (c_id, d_id) = (id(&state, 2), id(&state, 3));
        state.active = Some(2);
        state.selected = 3;
        let (model, _) = apply(
            &WorkspaceGroups::default(),
            &state_metas(&state),
            &[create("Work", &[]), create("Later", &[])],
        );
        let (work, later) = (group_id(&model, "Work"), group_id(&model, "Later"));
        let mut model = model;
        for (name, group) in [("e", &later), ("c", &work), ("a", &work)] {
            let idx = state
                .workspaces
                .iter()
                .position(|ws| ws.custom_name.as_deref() == Some(name))
                .unwrap();
            model.workspaces.insert(
                state.workspaces[idx].id.clone(),
                WorkspaceRecord {
                    group: Some(group.clone()),
                    ..WorkspaceRecord::default()
                },
            );
        }

        let order = normalized_order(&model, &state_metas(&state));
        assert!(state.move_workspace_block(&order, None));
        let names: Vec<_> = state
            .workspaces
            .iter()
            .map(|ws| ws.custom_name.clone().unwrap())
            .collect();
        assert_eq!(names, ["a", "c", "e", "b", "d"]);
        assert_eq!(state.workspaces[state.active.unwrap()].id, c_id);
        assert_eq!(state.workspaces[state.selected].id, d_id);

        let again = normalized_order(&model, &state_metas(&state));
        assert_eq!(again, order);
        assert!(!state.move_workspace_block(&again, None));
        state.assert_invariants_for_test();
    }

    #[test]
    fn place_top_end_before_target_and_other_region_fallback() {
        let live = live(&["w1", "w2", "w3", "w4", "w5", "w6"]);
        let (model, live) = apply(
            &WorkspaceGroups::default(),
            &live,
            &[create("Work", &["w1", "w2"]), create("Other", &["w6"])],
        );
        let (work, other) = (group_id(&model, "Work"), group_id(&model, "Other"));
        assert_eq!(order_of(&live), ["w1", "w2", "w6", "w3", "w4", "w5"]);

        let (model, live) = apply(&model, &live, &[place("w5", Some(&work), Some("w1"))]);
        assert_eq!(members(&model, &live, "Work"), ["w5", "w1", "w2"]);

        let (model, live) = apply(&model, &live, &[place("w4", Some(&work), None)]);
        assert_eq!(members(&model, &live, "Work"), ["w5", "w1", "w2", "w4"]);

        let (model, live) = apply(&model, &live, &[place("w3", Some(&work), Some("w2"))]);
        assert_eq!(
            members(&model, &live, "Work"),
            ["w5", "w1", "w3", "w2", "w4"]
        );

        // A before-target in another region, an unknown one, or the moved workspace
        // itself falls back to the end of the target region.
        let (model, live) = apply(&model, &live, &[place("w5", Some(&work), Some("w6"))]);
        assert_eq!(
            members(&model, &live, "Work"),
            ["w1", "w3", "w2", "w4", "w5"]
        );
        let (model, live) = apply(&model, &live, &[place("w1", Some(&work), Some("w99"))]);
        assert_eq!(
            members(&model, &live, "Work"),
            ["w3", "w2", "w4", "w5", "w1"]
        );
        let (model, live) = apply(&model, &live, &[place("w3", Some(&work), Some("w3"))]);
        assert_eq!(
            members(&model, &live, "Work"),
            ["w2", "w4", "w5", "w1", "w3"]
        );

        // Ungrouped placement and an empty target region.
        let (model, live) = apply(&model, &live, &[place("w2", None, None)]);
        assert_eq!(order_of(&live), ["w4", "w5", "w1", "w3", "w6", "w2"]);
        let (model, live) = apply(&model, &live, &[place("w6", None, Some("w2"))]);
        assert_eq!(members(&model, &live, "Other"), Vec::<&str>::new());
        assert_eq!(order_of(&live), ["w4", "w5", "w1", "w3", "w6", "w2"]);
        let (model, live) = apply(&model, &live, &[place("w2", Some(&other), None)]);
        assert_eq!(order_of(&live), ["w4", "w5", "w1", "w3", "w2", "w6"]);

        assert_eq!(
            plan_edit(&model, &live, &[place("w2", Some("gmissing"), None)], NOW),
            Err(GroupEditError::GroupNotFound("gmissing".into()))
        );
        assert_eq!(
            plan_edit(&model, &live, &[place("w99", None, None)], NOW),
            Err(GroupEditError::WorkspaceNotFound("w99".into()))
        );
    }

    #[test]
    fn place_parent_moves_colocated_children_only_when_sole_parent() {
        let family = vec![
            parent("p", "k"),
            meta("x"),
            child("c1", "k"),
            child("c2", "k"),
        ];
        let (model, family) = apply(&WorkspaceGroups::default(), &family, &[create("Work", &[])]);
        let work = group_id(&model, "Work");
        let (model, family) = apply(&model, &family, &[place("p", Some(&work), None)]);
        assert_eq!(members(&model, &family, "Work"), ["p", "c1", "c2"]);
        assert_eq!(order_of(&family), ["p", "c1", "c2", "x"]);

        // Two non-linked members of the key in the source region: children stay.
        let shared = vec![parent("p", "k"), parent("q", "k"), child("c1", "k")];
        let (model, shared) = apply(&WorkspaceGroups::default(), &shared, &[create("Work", &[])]);
        let work = group_id(&model, "Work");
        let (model, shared) = apply(&model, &shared, &[place("p", Some(&work), None)]);
        assert_eq!(members(&model, &shared, "Work"), ["p"]);
        assert_eq!(order_of(&shared), ["p", "q", "c1"]);

        // A child that already sits in another region stays there.
        let split = vec![parent("p", "k"), child("c1", "k"), child("c2", "k")];
        let (model, split) = apply(
            &WorkspaceGroups::default(),
            &split,
            &[create("Other", &["c1"]), create("Work", &[])],
        );
        let work = group_id(&model, "Work");
        let (model, split) = apply(&model, &split, &[place("p", Some(&work), None)]);
        assert_eq!(members(&model, &split, "Work"), ["p", "c2"]);
        assert_eq!(members(&model, &split, "Other"), ["c1"]);
    }

    #[test]
    fn place_linked_child_moves_only_child() {
        let family = vec![parent("p", "k"), child("c1", "k"), child("c2", "k")];
        let (model, family) = apply(
            &WorkspaceGroups::default(),
            &family,
            &[create("Work", &["p"])],
        );
        assert_eq!(members(&model, &family, "Work"), ["p", "c1", "c2"]);

        let (model, family) = apply(&model, &family, &[place("c1", None, None)]);
        assert_eq!(members(&model, &family, "Work"), ["p", "c2"]);
        assert_eq!(order_of(&family), ["p", "c2", "c1"]);
        assert!(
            !model.workspaces.contains_key("c1"),
            "ungrouped child keeps no record"
        );
    }

    fn reconciled(model: &mut WorkspaceGroups, live: &[WorkspaceMeta]) -> ReconcileOutcome {
        let outcome = reconcile(model, live, true);
        model.last_order = Some(outcome.order.clone());
        outcome
    }

    #[test]
    fn linked_child_inherits_unambiguous_family_group_when_created_at_runtime() {
        let started = vec![parent("p", "k"), parent("q", "k"), meta("x")];
        let (mut model, started) = apply(
            &WorkspaceGroups::default(),
            &started,
            &[create("Work", &["p", "q"])],
        );
        reconciled(&mut model, &started);

        let mut grown = started.clone();
        grown.push(child("c", "k"));
        let outcome = reconciled(&mut model, &grown);
        assert!(outcome.records_changed);
        assert_eq!(outcome.order, ids(&["p", "q", "c", "x"]));
        assert_eq!(
            model.effective_group("c").map(|group| group.name.as_str()),
            Some("Work")
        );

        // An all-ungrouped family leaves the new child ungrouped without a record.
        let mut model = WorkspaceGroups::default();
        let family = vec![parent("p", "k")];
        reconciled(&mut model, &family);
        let outcome = reconciled(&mut model, &[parent("p", "k"), child("c", "k")]);
        assert!(!outcome.records_changed);
        assert!(model.workspaces.is_empty());
    }

    #[test]
    fn linked_child_stays_ungrouped_when_family_groups_ambiguous() {
        for other_parent_group in [Some("Other"), None] {
            let started = vec![parent("p", "k"), parent("q", "k")];
            let mut ops = vec![create("Work", &["p"])];
            if let Some(name) = other_parent_group {
                ops.push(create(name, &["q"]));
            }
            let (mut model, started) = apply(&WorkspaceGroups::default(), &started, &ops);
            reconciled(&mut model, &started);

            let mut grown = started.clone();
            grown.push(child("c", "k"));
            let outcome = reconciled(&mut model, &grown);
            assert!(!outcome.records_changed, "{other_parent_group:?}");
            assert!(
                model.effective_group("c").is_none(),
                "{other_parent_group:?}"
            );
            assert_eq!(outcome.order.last().map(String::as_str), Some("c"));
        }
    }

    #[test]
    fn linked_child_inference_skipped_on_initial_reconcile() {
        let restored = vec![parent("p", "k"), child("c", "k")];
        let (mut model, restored) = apply(
            &WorkspaceGroups::default(),
            &restored,
            &[create("Work", &[])],
        );
        let work = group_id(&model, "Work");
        model.workspaces.insert(
            "p".into(),
            WorkspaceRecord {
                group: Some(work),
                ..WorkspaceRecord::default()
            },
        );
        assert!(model.last_order.is_none());

        let outcome = reconciled(&mut model, &restored);
        assert!(!outcome.records_changed);
        assert!(model.effective_group("c").is_none());
        assert_eq!(outcome.order, ids(&["p", "c"]));
    }

    #[test]
    fn reconcile_prunes_closed_records_only_after_settled_save() {
        let started = live(&["w1", "w2"]);
        let (mut model, started) = apply(
            &WorkspaceGroups::default(),
            &started,
            &[create("Work", &["w1", "w2"])],
        );
        reconciled(&mut model, &started);

        let closed = live(&["w1"]);
        let outcome = reconcile(&mut model, &closed, false);
        model.last_order = Some(outcome.order.clone());
        assert!(!outcome.records_changed);
        assert_eq!(model.pending_prune, ids(&["w2"]));
        assert!(model.workspaces.contains_key("w2"));

        let outcome = reconcile(&mut model, &closed, true);
        assert!(outcome.records_changed);
        assert!(model.pending_prune.is_empty());
        assert!(!model.workspaces.contains_key("w2"));
        assert!(model.workspaces.contains_key("w1"));
    }

    #[test]
    fn delete_group_ungroups_members_and_tombstones_template() {
        let live = live(&["w1", "w2", "w3", "w4"]);
        let (model, live) = apply(
            &WorkspaceGroups::default(),
            &live,
            &[
                WorkspaceGroupOp::Create {
                    name: "Work".into(),
                    color: None,
                    template: Some("Work".into()),
                    workspace_ids: ids(&["w1", "w2"]),
                    before_group_id: None,
                },
                create("Other", &["w3"]),
                set_color("w1", Some("teal")),
            ],
        );
        let work = group_id(&model, "Work");

        let (model, live) = apply(
            &model,
            &live,
            &[WorkspaceGroupOp::Delete {
                group_id: work.clone(),
            }],
        );
        assert_eq!(order_of(&live), ["w3", "w1", "w2", "w4"]);
        assert_eq!(model.deleted_templates, ids(&["Work"]));
        assert_eq!(model.groups.len(), 1);
        assert!(model.effective_group("w1").is_none());
        assert_eq!(model.effective_color("w1"), Some("teal"));
        assert!(
            !model.workspaces.contains_key("w2"),
            "empty records are dropped"
        );
        assert_eq!(
            plan_edit(
                &model,
                &live,
                &[WorkspaceGroupOp::Delete {
                    group_id: work.clone()
                }],
                NOW
            ),
            Err(GroupEditError::GroupNotFound(work))
        );
    }

    #[test]
    fn create_with_template_clears_tombstone_and_honors_before_group() {
        let live = live(&["w1", "w2", "w3"]);
        let (mut model, live) = apply(
            &WorkspaceGroups::default(),
            &live,
            &[create("A", &["w1"]), create("B", &["w2"])],
        );
        model.deleted_templates = ids(&["Infra", "Work"]);
        let b = group_id(&model, "B");

        let (model, live) = apply(
            &model,
            &live,
            &[WorkspaceGroupOp::Create {
                name: "Infra 2".into(),
                color: Some("teal".into()),
                template: Some("infra".into()),
                workspace_ids: ids(&["w3"]),
                before_group_id: Some(b),
            }],
        );
        let names: Vec<_> = model
            .groups
            .iter()
            .map(|group| group.name.as_str())
            .collect();
        assert_eq!(names, ["A", "Infra 2", "B"]);
        assert_eq!(model.deleted_templates, ids(&["Work"]));
        assert_eq!(model.groups[1].template.as_deref(), Some("infra"));
        assert_eq!(order_of(&live), ["w1", "w3", "w2"]);

        let (model, _) = apply(
            &model,
            &live,
            &[WorkspaceGroupOp::Create {
                name: "Last".into(),
                color: None,
                template: None,
                workspace_ids: Vec::new(),
                before_group_id: Some("gmissing".into()),
            }],
        );
        assert_eq!(model.groups.last().unwrap().name, "Last");
    }

    #[test]
    fn templates_are_unique_case_insensitively() {
        let live = live(&["w1"]);
        let (model, live) = apply(
            &WorkspaceGroups::default(),
            &live,
            &[
                WorkspaceGroupOp::Create {
                    name: "A".into(),
                    color: None,
                    template: Some("Work".into()),
                    workspace_ids: Vec::new(),
                    before_group_id: None,
                },
                create("B", &[]),
            ],
        );
        let b = group_id(&model, "B");
        let mut model = model;
        model.deleted_templates = ids(&["WORK"]);

        let (model, _) = apply(
            &model,
            &live,
            &[WorkspaceGroupOp::Update {
                group_id: b,
                name: None,
                color: None,
                template: Some("work".into()),
            }],
        );
        assert_eq!(model.groups[0].template, None);
        assert_eq!(model.groups[1].template.as_deref(), Some("work"));
        assert!(model.deleted_templates.is_empty());
    }

    #[test]
    fn move_group_reorders_regions() {
        let live = live(&["w1", "w2", "w3", "w4"]);
        let (model, live) = apply(
            &WorkspaceGroups::default(),
            &live,
            &[
                create("A", &["w1"]),
                create("B", &["w2"]),
                create("C", &["w3"]),
            ],
        );
        let (a, c) = (group_id(&model, "A"), group_id(&model, "C"));
        let move_group = |group_id: &str, before: Option<&str>| WorkspaceGroupOp::Move {
            group_id: group_id.into(),
            before_group_id: before.map(str::to_string),
        };

        let (model, live) = apply(&model, &live, &[move_group(&c, Some(&a))]);
        let names: Vec<_> = model
            .groups
            .iter()
            .map(|group| group.name.as_str())
            .collect();
        assert_eq!(names, ["C", "A", "B"]);
        assert_eq!(order_of(&live), ["w3", "w1", "w2", "w4"]);

        let (model, live) = apply(&model, &live, &[move_group(&a, None)]);
        assert_eq!(order_of(&live), ["w3", "w2", "w1", "w4"]);
        let (model, live) = apply(&model, &live, &[move_group(&c, Some("gmissing"))]);
        assert_eq!(order_of(&live), ["w2", "w1", "w3", "w4"]);
        let (same, same_order) = apply(&model, &live, &[move_group(&a, Some(&a))]);
        assert_eq!(same.groups, model.groups);
        assert_eq!(order_of(&same_order), ["w2", "w1", "w3", "w4"]);
        assert_eq!(
            plan_edit(&model, &live, &[move_group("gmissing", None)], NOW),
            Err(GroupEditError::GroupNotFound("gmissing".into()))
        );
    }

    #[test]
    fn group_ids_time_based_and_unique() {
        let live = live(&["w1"]);
        let (model, _) = plan_edit(
            &WorkspaceGroups::default(),
            &live,
            &[create("A", &[]), create("B", &[])],
            NOW,
        )
        .unwrap();
        assert_eq!(model.groups[0].id, "g19a3f2c1b7");
        assert_eq!(model.groups[1].id, "g19a3f2c1b8");

        let (model, _) = plan_edit(&model, &live, &[create("C", &[])], NOW).unwrap();
        assert_eq!(model.groups[2].id, "g19a3f2c1b9");
        let (model, _) = plan_edit(&model, &live, &[create("D", &[])], 0x1_0000_0000).unwrap();
        assert_eq!(model.groups[3].id, "g100000000");
    }

    #[test]
    fn names_unique_case_insensitive_and_bounded() {
        let live = live(&["w1"]);
        let (model, _) = apply(
            &WorkspaceGroups::default(),
            &live,
            &[create("  Work  ", &[])],
        );
        assert_eq!(model.groups[0].name, "Work");
        let work = model.groups[0].id.clone();

        assert_eq!(
            plan_edit(&model, &live, &[create("work", &[])], NOW),
            Err(GroupEditError::NameTaken("work".into()))
        );
        for invalid in ["", "   ", "a\tb", "line\nbreak", &"x".repeat(41)] {
            assert_eq!(
                plan_edit(&model, &live, &[create(invalid, &[])], NOW),
                Err(GroupEditError::InvalidName),
                "{invalid:?}"
            );
        }
        let longest = "\u{e9}".repeat(MAX_GROUP_NAME_CHARS);
        assert!(plan_edit(&model, &live, &[create(&longest, &[])], NOW).is_ok());

        let rename = |name: &str| WorkspaceGroupOp::Update {
            group_id: work.clone(),
            name: Some(name.into()),
            color: None,
            template: None,
        };
        let (model, _) = apply(&model, &live, &[rename("WORK")]);
        assert_eq!(model.groups[0].name, "WORK");
        let (model, _) = apply(&model, &live, &[create("Other", &[])]);
        assert_eq!(
            plan_edit(&model, &live, &[rename(" other ")], NOW),
            Err(GroupEditError::NameTaken("other".into()))
        );
        assert_eq!(
            plan_edit(&model, &live, &[rename("")], NOW),
            Err(GroupEditError::InvalidName)
        );
    }

    #[test]
    fn colors_limited_to_named_palette() {
        let live = live(&["w1"]);
        let (model, _) = apply(
            &WorkspaceGroups::default(),
            &live,
            &[WorkspaceGroupOp::Create {
                name: "Work".into(),
                color: Some(" Teal ".into()),
                template: None,
                workspace_ids: Vec::new(),
                before_group_id: None,
            }],
        );
        assert_eq!(model.groups[0].color, "teal");
        let work = model.groups[0].id.clone();

        let (model, _) = apply(
            &model,
            &live,
            &[
                WorkspaceGroupOp::Update {
                    group_id: work.clone(),
                    name: None,
                    color: Some("BLUE".into()),
                    template: None,
                },
                set_color("w1", Some("Pink")),
            ],
        );
        assert_eq!(model.groups[0].color, "blue");
        assert_eq!(model.workspaces["w1"].color.as_deref(), Some("pink"));
        let (cleared, _) = apply(&model, &live, &[set_color("w1", None)]);
        assert!(!cleared.workspaces.contains_key("w1"));

        for invalid in ["magenta", "#1d4ed8", "", "grey"] {
            assert_eq!(
                plan_edit(&model, &live, &[set_color("w1", Some(invalid))], NOW),
                Err(GroupEditError::InvalidColor(invalid.into()))
            );
            assert_eq!(
                plan_edit(
                    &model,
                    &live,
                    &[WorkspaceGroupOp::Update {
                        group_id: work.clone(),
                        name: None,
                        color: Some(invalid.into()),
                        template: None,
                    }],
                    NOW
                ),
                Err(GroupEditError::InvalidColor(invalid.into()))
            );
        }
    }

    #[test]
    fn default_group_colors_follow_the_cycle() {
        let live = live(&["w1"]);
        let ops: Vec<_> = (0..10).map(|i| create(&format!("G{i}"), &[])).collect();
        let (model, _) = apply(&WorkspaceGroups::default(), &live, &ops);
        let colors: Vec<_> = model
            .groups
            .iter()
            .map(|group| group.color.as_str())
            .collect();
        assert_eq!(
            colors,
            [
                "blue", "green", "purple", "orange", "teal", "pink", "gray", "red", "yellow",
                "blue"
            ]
        );

        let (model, _) = apply(
            &WorkspaceGroups::default(),
            &live,
            &[
                WorkspaceGroupOp::Create {
                    name: "Explicit".into(),
                    color: Some("blue".into()),
                    template: None,
                    workspace_ids: Vec::new(),
                    before_group_id: None,
                },
                create("Next", &[]),
            ],
        );
        assert_eq!(model.groups[1].color, "green");
    }

    #[test]
    fn bind_tab_rejects_foreign_tab_and_drops_dead_numbers() {
        let mut w1 = meta("w1");
        w1.tabs = vec![("w1:t1".into(), 1), ("w1:t2".into(), 2)];
        let live = vec![w1, meta("w2")];

        let err = plan_edit(
            &WorkspaceGroups::default(),
            &live,
            &[bind_tab("w1", "agent", Some("w2:t1"))],
            NOW,
        )
        .unwrap_err();
        assert_eq!(err.code(), "tab_not_found");
        assert_eq!(err.message(), "tab w2:t1 not found in workspace w1");

        let (model, _) = apply(
            &WorkspaceGroups::default(),
            &live,
            &[bind_tab("w1", "agent", Some("w1:t2"))],
        );
        assert_eq!(
            model.workspaces["w1"].tabs,
            BTreeMap::from([("agent".into(), 2)])
        );

        let (model, _) = apply(&model, &live, &[bind_tab("w1", "AGENT", Some("w1:t1"))]);
        assert_eq!(
            model.workspaces["w1"].tabs,
            BTreeMap::from([("AGENT".into(), 1)])
        );

        let mut stale = model.clone();
        stale
            .workspaces
            .get_mut("w1")
            .unwrap()
            .tabs
            .insert("old".into(), 7);
        stale.workspaces.insert(
            "w9".into(),
            WorkspaceRecord {
                tabs: BTreeMap::from([("dormant".into(), 7)]),
                ..WorkspaceRecord::default()
            },
        );
        let (cleaned, _) = apply(&stale, &live, &[set_color("w2", Some("red"))]);
        assert_eq!(
            cleaned.workspaces["w1"].tabs,
            BTreeMap::from([("AGENT".into(), 1)])
        );
        assert_eq!(
            cleaned.workspaces["w9"].tabs,
            BTreeMap::from([("dormant".into(), 7)]),
            "records of workspaces that are not live keep their bindings"
        );

        let (removed, _) = apply(&model, &live, &[bind_tab("w1", "agent", None)]);
        assert!(!removed.workspaces.contains_key("w1"));

        let full_ops: Vec<_> = (0..MAX_TAB_BINDINGS)
            .map(|i| bind_tab("w1", &format!("k{i}"), Some("w1:t1")))
            .collect();
        let (full, _) = apply(&WorkspaceGroups::default(), &live, &full_ops);
        assert_eq!(full.workspaces["w1"].tabs.len(), MAX_TAB_BINDINGS);
        let err = plan_edit(
            &full,
            &live,
            &[bind_tab("w1", "one-more", Some("w1:t1"))],
            NOW,
        )
        .unwrap_err();
        assert_eq!(err, GroupEditError::TooManyTabBindings("w1".into()));
        assert!(plan_edit(&full, &live, &[bind_tab("w1", "K0", Some("w1:t2"))], NOW).is_ok());
    }

    #[test]
    fn bind_none_clears_tab_bindings() {
        let live = live(&["w1"]);
        let (model, _) = apply(
            &WorkspaceGroups::default(),
            &live,
            &[
                bind("w1", Some("api")),
                bind_tab("w1", "agent", Some("w1:t1")),
            ],
        );
        assert_eq!(model.workspaces["w1"].project.as_deref(), Some("api"));
        assert_eq!(model.workspaces["w1"].tabs.len(), 1);

        let (model, _) = apply(&model, &live, &[bind("w1", None)]);
        assert!(!model.workspaces.contains_key("w1"));
    }

    #[test]
    fn bind_project_is_unique_across_workspaces() {
        let live = live(&["w1", "w2"]);
        let (model, _) = apply(
            &WorkspaceGroups::default(),
            &live,
            &[
                bind("w1", Some("api")),
                bind_tab("w1", "agent", Some("w1:t1")),
                bind("w2", Some("web")),
                bind_tab("w2", "dev", Some("w2:t1")),
            ],
        );

        let (moved, _) = apply(&model, &live, &[bind("w2", Some("API"))]);
        assert!(
            !moved.workspaces.contains_key("w1"),
            "w1 loses api and its tabs"
        );
        assert_eq!(moved.workspaces["w2"].project.as_deref(), Some("API"));
        assert_eq!(
            moved.workspaces["w2"].tabs,
            BTreeMap::from([("dev".into(), 1)]),
            "binding a project never clears the workspace's own tab bindings"
        );

        let (same, _) = apply(&model, &live, &[bind("w1", Some("Api"))]);
        assert_eq!(same.workspaces["w1"].project.as_deref(), Some("Api"));
        assert_eq!(same.workspaces["w1"].tabs.len(), 1);

        // Op order inside a batch does not matter for a workspace's own bindings.
        let (tab_first, _) = apply(
            &WorkspaceGroups::default(),
            &live,
            &[
                bind_tab("w1", "agent", Some("w1:t1")),
                bind("w1", Some("api")),
            ],
        );
        assert_eq!(
            tab_first.workspaces["w1"].tabs,
            BTreeMap::from([("agent".into(), 1)])
        );
    }

    #[test]
    fn batch_is_all_or_nothing() {
        let live = live(&["w1", "w2"]);
        let model = WorkspaceGroups::default();
        let err = plan_edit(
            &model,
            &live,
            &[
                create("Work", &["w1"]),
                set_color("w2", Some("teal")),
                set_color("w2", Some("magenta")),
            ],
            NOW,
        )
        .unwrap_err();
        assert_eq!(err, GroupEditError::InvalidColor("magenta".into()));
        assert_eq!(model, WorkspaceGroups::default());

        let (unchanged, order) = plan_edit(&model, &live, &[], NOW).unwrap();
        assert_eq!(unchanged, model);
        assert_eq!(order, ids(&["w1", "w2"]));
    }

    #[test]
    fn group_edit_errors_use_contract_codes_and_messages() {
        let cases = [
            (
                GroupEditError::WorkspaceNotFound("w9".into()),
                "workspace_not_found",
                "workspace w9 not found",
            ),
            (
                GroupEditError::tab_not_found("w1:t9"),
                "tab_not_found",
                "tab w1:t9 not found",
            ),
            (
                GroupEditError::tab_not_found_in_workspace("w2:t1", "w1"),
                "tab_not_found",
                "tab w2:t1 not found in workspace w1",
            ),
            (
                GroupEditError::GroupNotFound("g1".into()),
                "workspace_group_not_found",
                "workspace group g1 not found",
            ),
            (
                GroupEditError::NameTaken("Work".into()),
                "workspace_group_name_taken",
                "workspace group name Work is already used",
            ),
            (
                GroupEditError::InvalidName,
                "invalid_workspace_group_name",
                "workspace group names must be 1-40 characters without control characters",
            ),
            (
                GroupEditError::InvalidColor("magenta".into()),
                "invalid_color",
                "unknown color magenta; use one of red, orange, yellow, green, teal, blue, purple, pink, gray",
            ),
            (
                GroupEditError::TooManyTabBindings("w3".into()),
                "too_many_tab_bindings",
                "workspace w3 already has 32 tab bindings",
            ),
        ];
        for (error, code, message) in cases {
            assert_eq!(error.code(), code);
            assert_eq!(error.message(), message);
        }
    }

    #[test]
    fn projection_lists_groups_members_and_effective_colors_in_display_order() {
        let live = vec![parent("w1", "k"), child("w2", "k"), meta("w3"), meta("w4")];
        assert_eq!(
            project_client_shell(&WorkspaceGroups::default(), &live),
            ClientShellWorkspaceGroups::default()
        );

        let (model, live) = apply(
            &WorkspaceGroups::default(),
            &live,
            &[
                WorkspaceGroupOp::Create {
                    name: "Work".into(),
                    color: Some("blue".into()),
                    template: Some("Work".into()),
                    workspace_ids: ids(&["w3", "w1"]),
                    before_group_id: None,
                },
                create("Empty", &[]),
                set_color("w3", Some("teal")),
            ],
        );
        let projection = project_client_shell(&model, &live);
        assert_eq!(projection.groups.len(), 2);
        assert_eq!(projection.groups[0].name, "Work");
        assert_eq!(projection.groups[0].template.as_deref(), Some("Work"));
        assert_eq!(projection.groups[0].workspace_ids, ids(&["w3", "w1", "w2"]));
        assert!(projection.groups[1].workspace_ids.is_empty());
        assert_eq!(
            projection.workspace_colors,
            vec![
                ("w3".to_string(), "teal".to_string()),
                ("w1".to_string(), "blue".to_string()),
                ("w2".to_string(), "blue".to_string()),
            ]
        );

        let mut colors_only = WorkspaceGroups::default();
        colors_only.workspaces.insert(
            "w4".into(),
            WorkspaceRecord {
                color: Some("red".into()),
                ..WorkspaceRecord::default()
            },
        );
        let projection = project_client_shell(&colors_only, &live);
        assert!(projection.groups.is_empty());
        assert_eq!(
            projection.workspace_colors,
            vec![("w4".to_string(), "red".to_string())]
        );
    }

    #[test]
    fn sidecar_conversion_roundtrips_persisted_fields_only() {
        let live = live(&["w1", "w2"]);
        let (mut model, _) = apply(
            &WorkspaceGroups::default(),
            &live,
            &[
                WorkspaceGroupOp::Create {
                    name: "Work".into(),
                    color: None,
                    template: Some("Work".into()),
                    workspace_ids: ids(&["w1"]),
                    before_group_id: None,
                },
                create("Scratch", &[]),
                set_color("w1", Some("teal")),
                bind("w1", Some("api")),
                bind_tab("w1", "agent", Some("w1:t1")),
            ],
        );
        model.deleted_templates = ids(&["Infra"]);
        let file = model.to_file();
        assert_eq!(file.version, SIDECAR_VERSION);
        assert_eq!(file.groups.len(), 2);
        assert_eq!(file.workspaces["w1"].project.as_deref(), Some("api"));

        model.last_order = Some(ids(&["w1"]));
        model.pending_prune = ids(&["w7"]);
        model.persist_blocked = true;
        let restored = WorkspaceGroups::from_file(model.to_file());
        assert_eq!(restored.to_file(), file);
        assert_eq!(restored.groups, model.groups);
        assert_eq!(restored.workspaces, model.workspaces);
        assert_eq!(restored.deleted_templates, model.deleted_templates);
        assert_eq!(restored.last_order, None);
        assert!(restored.pending_prune.is_empty());
        assert!(!restored.persist_blocked);
    }
}
