//! `herdr up` (fork): creates or adopts the groups, workspaces and tabs that
//! `projects.toml` declares, through the public API. The server never reads
//! the file; `up` never spawns a server, closes anything or changes focus.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::api::schema::{
    AgentStartParams, EmptyParams, Method, PaneSendInputParams, Request, ResponseResult,
    TabCreateParams, TabInfo, TabListParams, TabRenameParams, WorkspaceCreateParams,
    WorkspaceGroupEditParams, WorkspaceGroupInfo, WorkspaceGroupMemberInfo, WorkspaceGroupOp,
    WorkspaceGroupTabBinding, WorkspaceRenameParams,
};
use crate::app::workspace_groups::MAX_GROUP_NAME_CHARS;
use crate::protocol::normalize_workspace_color;

const COLOR_NAMES: &str = "red, orange, yellow, green, teal, blue, purple, pink, gray";

/// `projects.toml` (spec 11.1); arrays keep file order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectsFile {
    #[serde(default)]
    group: Vec<GroupSpec>,
    #[serde(default)]
    project: Vec<ProjectSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupSpec {
    name: String,
    color: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectSpec {
    name: String,
    label: Option<String>,
    cwd: String,
    group: Option<String>,
    color: Option<String>,
    #[serde(default)]
    tab: Vec<TabSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct TabSpec {
    label: String,
    agent: Option<String>,
    /// An `Option` so "args requires agent" is detectable.
    args: Option<Vec<String>>,
    agent_name: Option<String>,
    command: Option<String>,
    cwd: Option<String>,
}

fn parse_projects(text: &str) -> Result<ProjectsFile, String> {
    toml::from_str(text).map_err(|err| err.to_string())
}

/// Group and project names and tab labels compare case-insensitively everywhere (spec 11.3).
fn same_name(left: &str, right: &str) -> bool {
    left.to_lowercase() == right.to_lowercase()
}

/// Replicates upstream's private `valid_agent_name` (src/app/agents.rs), which
/// `agent.start` enforces: a lowercase ASCII letter, then lowercase letters,
/// digits, `-` or `_`, at most 32 characters.
fn valid_agent_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some('a'..='z'))
        && name.len() <= 32
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '_'))
}

/// `agent_name`, else `<project>-<label>` in file spelling (spec 11.1).
fn agent_name_for(project: &ProjectSpec, tab: &TabSpec) -> String {
    tab.agent_name
        .clone()
        .unwrap_or_else(|| format!("{}-{}", project.name, tab.label))
}

/// The server's group-name rule, checked here so a bad name is a file error
/// (exit 2) instead of a failed request halfway through a run. Surrounding
/// spaces are rejected because the server would trim them away from the name
/// but not from the template binding.
fn valid_group_name(name: &str) -> bool {
    let count = name.chars().count();
    name.trim() == name
        && (1..=MAX_GROUP_NAME_CHARS).contains(&count)
        && !name.chars().any(char::is_control)
}

fn check_color(color: &str) -> Result<(), String> {
    match normalize_workspace_color(color) {
        Some(_) => Ok(()),
        None => Err(format!("unknown color {color}; use one of {COLOR_NAMES}")),
    }
}

/// Spec 11.3: every check that needs neither the filesystem nor the server.
fn validate(file: &ProjectsFile) -> Result<(), String> {
    let mut group_names = BTreeSet::new();
    for group in &file.group {
        if !valid_group_name(&group.name) {
            return Err(format!(
                "group {:?}: names must be 1-40 characters without control characters or surrounding spaces",
                group.name
            ));
        }
        if !group_names.insert(group.name.to_lowercase()) {
            return Err(format!("duplicate group name {}", group.name));
        }
        if let Some(color) = &group.color {
            check_color(color).map_err(|err| format!("group {}: {err}", group.name))?;
        }
    }
    let mut project_names = BTreeSet::new();
    let mut agent_names: BTreeMap<String, String> = BTreeMap::new();
    for project in &file.project {
        if project.name.trim().is_empty() {
            return Err("project names must not be empty".into());
        }
        if !project_names.insert(project.name.to_lowercase()) {
            return Err(format!("duplicate project name {}", project.name));
        }
        if let Some(group) = &project.group {
            if !group_names.contains(&group.to_lowercase()) {
                return Err(format!("project {}: unknown group {group}", project.name));
            }
        }
        if let Some(color) = &project.color {
            check_color(color).map_err(|err| format!("project {}: {err}", project.name))?;
        }
        let mut labels = BTreeSet::new();
        for (position, tab) in project.tab.iter().enumerate() {
            if tab.label.trim().is_empty() {
                return Err(format!(
                    "project {}: tab labels must not be empty",
                    project.name
                ));
            }
            if !labels.insert(tab.label.to_lowercase()) {
                return Err(format!(
                    "project {}: duplicate tab label {}",
                    project.name, tab.label
                ));
            }
            let at = format!("project {} tab {}", project.name, tab.label);
            if position == 0 && tab.cwd.is_some() {
                return Err(format!(
                    "{at}: the first tab starts in the project cwd; remove its cwd"
                ));
            }
            if tab.agent.is_some() && tab.command.is_some() {
                return Err(format!("{at}: agent and command are mutually exclusive"));
            }
            if tab.agent.is_none() && tab.args.is_some() {
                return Err(format!("{at}: args requires agent"));
            }
            if tab.agent.is_none() && tab.agent_name.is_some() {
                return Err(format!("{at}: agent_name requires agent"));
            }
            let Some(kind) = &tab.agent else {
                continue;
            };
            if crate::detect::parse_agent_label(kind).is_none() {
                return Err(format!("{at}: unsupported agent kind {kind}"));
            }
            let name = agent_name_for(project, tab);
            if !valid_agent_name(&name) {
                return Err(format!(
                    "{at}: agent name {name} is not valid; set agent_name (a lowercase letter, then lowercase letters, digits, - or _, at most 32 characters)"
                ));
            }
            if let Some(owner) = agent_names.insert(name.clone(), at.clone()) {
                return Err(format!(
                    "{at}: agent name {name} is already used by {owner}"
                ));
            }
        }
    }
    Ok(())
}

/// One selection mode (spec 11.2); `--reapply` is separate.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Selection {
    Projects(Vec<String>),
    Group(String),
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ParsedArgs {
    Help,
    Run { selection: Selection, reapply: bool },
}

/// Indexes into `ProjectsFile.group` / `ProjectsFile.project`, in file order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Selected {
    groups: Vec<usize>,
    projects: Vec<usize>,
}

/// Local directory facts for one project, read before planning so `plan` stays pure.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProjectDir {
    /// `~`-expanded absolute cwd, sent to `workspace.create` and `tab.create`.
    path: String,
    /// `crate::worktree::canonical_or_original(path)`, compared with member cwds.
    key: String,
    exists: bool,
    /// Lowercase tab label -> expanded absolute tab cwd (only tabs that set `cwd`).
    tabs: BTreeMap<String, String>,
}

fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
    let args = super::expand_equals_args(args, &["--group"]);
    let mut projects = Vec::new();
    let mut group: Option<String> = None;
    let mut all = false;
    let mut reapply = false;
    let mut options_ended = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if options_ended {
            projects.push(arg.clone());
            continue;
        }
        match arg.as_str() {
            "--" => options_ended = true,
            "--help" | "-h" => return Ok(ParsedArgs::Help),
            "help" if args.len() == 1 => return Ok(ParsedArgs::Help),
            "--all" => all = true,
            "--reapply" => reapply = true,
            "--group" => {
                let Some(name) = iter.next() else {
                    return Err("--group requires a group name".into());
                };
                if group.replace(name.clone()).is_some() {
                    return Err("--group may be given only once".into());
                }
            }
            other if other.starts_with('-') => return Err(format!("unknown option: {other}")),
            _ => projects.push(arg.clone()),
        }
    }
    let modes = usize::from(!projects.is_empty()) + usize::from(group.is_some()) + usize::from(all);
    if modes != 1 {
        return Err("choose exactly one of <project>..., --group <group> or --all".into());
    }
    let selection = match group {
        Some(group) => Selection::Group(group),
        None if all => Selection::All,
        None => Selection::Projects(projects),
    };
    Ok(ParsedArgs::Run { selection, reapply })
}

/// Resolves a selection against the file (case-insensitive); unknown names are usage errors.
fn select(file: &ProjectsFile, selection: &Selection) -> Result<Selected, String> {
    let mut selected = Selected::default();
    match selection {
        Selection::All => {
            selected.groups = (0..file.group.len()).collect();
            selected.projects = (0..file.project.len()).collect();
        }
        Selection::Group(name) => {
            let Some(index) = file
                .group
                .iter()
                .position(|group| same_name(&group.name, name))
            else {
                return Err(format!("unknown group {name} in projects.toml"));
            };
            selected.groups.push(index);
            selected.projects = file
                .project
                .iter()
                .enumerate()
                .filter(|(_, project)| {
                    project
                        .group
                        .as_deref()
                        .is_some_and(|group| same_name(group, name))
                })
                .map(|(index, _)| index)
                .collect();
        }
        Selection::Projects(names) => {
            let mut projects = BTreeSet::new();
            for name in names {
                let Some(index) = file
                    .project
                    .iter()
                    .position(|project| same_name(&project.name, name))
                else {
                    return Err(format!("unknown project {name} in projects.toml"));
                };
                projects.insert(index);
            }
            let groups: BTreeSet<usize> = projects
                .iter()
                .filter_map(|&index| file.project.get(index)?.group.as_deref())
                .filter_map(|group| {
                    file.group
                        .iter()
                        .position(|spec| same_name(&spec.name, group))
                })
                .collect();
            selected.projects = projects.into_iter().collect();
            selected.groups = groups.into_iter().collect();
        }
    }
    Ok(selected)
}

fn expand_cwd(raw: &str) -> Result<PathBuf, String> {
    let path = crate::worktree::expand_tilde_path(raw);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(format!("cwd {raw} is not an absolute path"))
    }
}

/// Keyed by lowercase project name. A cwd that is not absolute after `~`
/// expansion is a file error (exit 2); a missing project directory is not.
fn prepare_dirs(file: &ProjectsFile) -> Result<BTreeMap<String, ProjectDir>, String> {
    let mut dirs = BTreeMap::new();
    for project in &file.project {
        let path =
            expand_cwd(&project.cwd).map_err(|err| format!("project {}: {err}", project.name))?;
        let mut tabs = BTreeMap::new();
        for tab in &project.tab {
            if let Some(cwd) = &tab.cwd {
                let tab_path = expand_cwd(cwd)
                    .map_err(|err| format!("project {} tab {}: {err}", project.name, tab.label))?;
                tabs.insert(
                    tab.label.to_lowercase(),
                    tab_path.to_string_lossy().into_owned(),
                );
            }
        }
        dirs.insert(
            project.name.to_lowercase(),
            ProjectDir {
                key: crate::worktree::canonical_or_original(&path)
                    .to_string_lossy()
                    .into_owned(),
                exists: path.is_dir(),
                path: path.to_string_lossy().into_owned(),
                tabs,
            },
        );
    }
    Ok(dirs)
}

/// What `herdr up` read before planning: the server's answers plus local directory facts.
#[derive(Debug, Default)]
struct ServerState {
    groups: Vec<WorkspaceGroupInfo>,
    deleted_templates: Vec<String>,
    /// `workspace_group.list` members, display order.
    members: Vec<WorkspaceGroupMemberInfo>,
    /// Workspace id -> canonical member cwd (`crate::worktree::canonical_or_original`).
    member_keys: BTreeMap<String, String>,
    /// `workspace.list`: workspace id -> label.
    labels: BTreeMap<String, String>,
    /// `agent.list`: named agents only.
    agents: Vec<LiveAgent>,
    /// `tab.list` (one call, every workspace), grouped by workspace id.
    tabs: BTreeMap<String, Vec<TabInfo>>,
    /// `prepare_dirs`, keyed by lowercase project name.
    project_dirs: BTreeMap<String, ProjectDir>,
}

/// How a report line is printed: Progress on stdout, the others on stderr; Skipped means exit 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Progress,
    Warning,
    Skipped,
}

/// A workspace that exists, or the one this run creates for a project (file spelling).
#[derive(Debug, Clone, PartialEq, Eq)]
enum WsRef {
    Existing(String),
    Created(String),
}

/// A group that exists, or the one this run creates for a template.
#[derive(Debug, Clone, PartialEq, Eq)]
enum GroupRef {
    Existing(String),
    Template(String),
}

/// One `workspace_group.edit` op with symbolic ids; the executor resolves them.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PlannedOp {
    CreateGroup {
        name: String,
        color: Option<String>,
        template: String,
    },
    UpdateGroup {
        group_id: String,
        name: Option<String>,
        color: Option<String>,
        template: Option<String>,
    },
    Place {
        workspace: WsRef,
        group: GroupRef,
    },
    SetColor {
        workspace: WsRef,
        color: String,
    },
    Bind {
        workspace: WsRef,
        project: Option<String>,
    },
    BindTab {
        workspace: WsRef,
        key: String,
        tab: TabRef,
    },
}

/// One unit of the plan: a printed line or one API request.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Report {
        level: Level,
        line: String,
    },
    /// One `workspace_group.edit` batch; `done` is printed after it succeeds.
    Edit {
        ops: Vec<PlannedOp>,
        done: Option<String>,
    },
    /// `workspace.create {cwd, label, focus: false}` (rule 3.4).
    CreateWorkspace {
        project: String,
        cwd: String,
        label: String,
    },
    /// `workspace.rename` (`--reapply` only).
    RenameWorkspace {
        workspace_id: String,
        label: String,
    },
    /// `tab.create {workspace_id, cwd, label: key, focus: false}` (rule 4.4).
    CreateTab {
        project: String,
        key: String,
        workspace: WsRef,
        cwd: String,
    },
    /// `tab.rename` of the root tab of a workspace created in this run.
    RenameTab {
        tab: NewTab,
        label: String,
    },
    /// `agent.start` or `pane.send_input` in a tab created in this run; `done` is printed on success.
    Launch {
        tab: NewTab,
        key: String,
        launch: TabLaunch,
        done: String,
    },
}

/// Lowercase and without a Windows verbatim prefix (`\\?\`, `\\?\UNC\`), so a
/// canonical path compares equal to its plain spelling: a member cwd that no
/// longer exists cannot be canonicalized, while the project cwd always is.
fn path_key(path: &str) -> String {
    let path = path.to_lowercase();
    if let Some(share) = path.strip_prefix(r"\\?\unc\") {
        return format!(r"\\{share}");
    }
    match path.strip_prefix(r"\\?\") {
        Some(rest) => rest.to_string(),
        None => path,
    }
}

/// Both sides are canonical strings; compared case-insensitively, component by component.
fn same_cwd(cwd: &str, project: &str) -> bool {
    Path::new(&path_key(cwd)) == Path::new(&path_key(project))
}

/// True when `cwd` equals or lies inside `project` (D17).
fn cwd_within(cwd: &str, project: &str) -> bool {
    Path::new(&path_key(cwd)).starts_with(path_key(project))
}

/// A member's canonical cwd when `read_state` recorded one, else its reported cwd.
fn member_key<'a>(state: &'a ServerState, member: &'a WorkspaceGroupMemberInfo) -> &'a str {
    state
        .member_keys
        .get(&member.workspace_id)
        .map_or(member.cwd.as_str(), String::as_str)
}

/// Pure planner (spec 11.4): groups first, then projects, both in file order.
fn plan(
    file: &ProjectsFile,
    selection: &Selection,
    state: &ServerState,
    reapply: bool,
) -> Vec<Step> {
    let selected = select(file, selection).unwrap_or_default();
    let mut steps = Vec::new();
    let mut targets: BTreeMap<String, GroupRef> = BTreeMap::new();
    for spec in selected
        .groups
        .iter()
        .filter_map(|&index| file.group.get(index))
    {
        if let Some(target) = plan_group(spec, selection, state, reapply, &mut steps) {
            targets.insert(spec.name.to_lowercase(), target);
        }
    }
    let mut members = state.members.clone();
    for spec in selected
        .projects
        .iter()
        .filter_map(|&index| file.project.get(index))
    {
        let group = spec
            .group
            .as_ref()
            .and_then(|name| targets.get(&name.to_lowercase()));
        plan_project(spec, group, state, &mut members, reapply, &mut steps);
    }
    steps
}

/// Spec 11.4 step 2 for one `[[group]]`. Returns where its projects go; None = ungrouped.
fn plan_group(
    spec: &GroupSpec,
    selection: &Selection,
    state: &ServerState,
    reapply: bool,
    steps: &mut Vec<Step>,
) -> Option<GroupRef> {
    let name = spec.name.as_str();
    let color = spec.color.as_deref().and_then(normalize_workspace_color);
    // 2.1: the group bound to this template, whatever the UI renamed it to.
    if let Some(group) = state.groups.iter().find(|group| {
        group
            .template
            .as_deref()
            .is_some_and(|template| same_name(template, name))
    }) {
        let before = steps.len();
        if reapply {
            reapply_group(group, name, color, state, steps);
        }
        if steps.len() == before {
            steps.push(Step::Report {
                level: Level::Progress,
                line: format!("group {name}: up to date ({})", group.group_id),
            });
        }
        return Some(GroupRef::Existing(group.group_id.clone()));
    }
    // 2.2: deleted in the UI, and neither `--group <it>` nor `--reapply` restores it.
    let restore =
        reapply || matches!(selection, Selection::Group(selected) if same_name(selected, name));
    if !restore
        && state
            .deleted_templates
            .iter()
            .any(|template| same_name(template, name))
    {
        steps.push(Step::Report {
            level: Level::Progress,
            line: format!(
                "group {name} was deleted in the UI; run herdr up --group {name} to restore it"
            ),
        });
        return None;
    }
    // 2.3: adopt an unbound group with the same name; a group bound elsewhere keeps it.
    if let Some(group) = state
        .groups
        .iter()
        .find(|group| same_name(&group.name, name))
    {
        if let Some(template) = &group.template {
            steps.push(Step::Report {
                level: Level::Skipped,
                line: format!("group {name}: name taken by a group bound to {template}; skipped"),
            });
            return None;
        }
        let rename = (reapply && group.name != name).then(|| name.to_string());
        let recolor = color
            .filter(|color| reapply && *color != group.color)
            .map(str::to_string);
        steps.push(Step::Edit {
            ops: vec![PlannedOp::UpdateGroup {
                group_id: group.group_id.clone(),
                name: rename,
                color: recolor,
                template: Some(name.to_string()),
            }],
            done: Some(format!("group {name}: adopted {}", group.group_id)),
        });
        return Some(GroupRef::Existing(group.group_id.clone()));
    }
    // 2.4: create it, bound to the template.
    steps.push(Step::Edit {
        ops: vec![PlannedOp::CreateGroup {
            name: name.to_string(),
            color: color.map(str::to_string),
            template: name.to_string(),
        }],
        done: Some(format!("group {name}: created")),
    });
    Some(GroupRef::Template(name.to_string()))
}

/// `--reapply` on a group bound by template: the file's name and color where they differ.
fn reapply_group(
    group: &WorkspaceGroupInfo,
    name: &str,
    color: Option<&str>,
    state: &ServerState,
    steps: &mut Vec<Step>,
) {
    let mut rename = (group.name != name).then(|| name.to_string());
    if rename.is_some()
        && state
            .groups
            .iter()
            .any(|other| other.group_id != group.group_id && same_name(&other.name, name))
    {
        steps.push(Step::Report {
            level: Level::Skipped,
            line: format!("group {name}: name taken by another group; not renamed"),
        });
        rename = None;
    }
    let recolor = color
        .filter(|color| *color != group.color)
        .map(str::to_string);
    if rename.is_none() && recolor.is_none() {
        return;
    }
    steps.push(Step::Edit {
        ops: vec![PlannedOp::UpdateGroup {
            group_id: group.group_id.clone(),
            name: rename,
            color: recolor,
            template: None,
        }],
        done: Some(format!("group {name}: reapplied name and color")),
    });
}

/// Rules 3.2 and 3.3 (spec 11.4).
enum Adoption {
    Found {
        workspace_id: String,
        by: &'static str,
    },
    Ambiguous(usize),
    Missing,
}

/// Unbound, non-linked members not excluded by a stale binding in this run:
/// a single cwd match, else a single label match inside the project cwd.
fn find_adoption(
    members: &[WorkspaceGroupMemberInfo],
    excluded: &BTreeSet<String>,
    state: &ServerState,
    dir: &ProjectDir,
    label: &str,
) -> Adoption {
    let candidates: Vec<&WorkspaceGroupMemberInfo> = members
        .iter()
        .filter(|member| {
            !member.linked && member.project.is_none() && !excluded.contains(&member.workspace_id)
        })
        .collect();
    let by_cwd: Vec<&WorkspaceGroupMemberInfo> = candidates
        .iter()
        .copied()
        .filter(|member| same_cwd(member_key(state, member), &dir.key))
        .collect();
    if let [member] = by_cwd.as_slice() {
        return Adoption::Found {
            workspace_id: member.workspace_id.clone(),
            by: "cwd",
        };
    }
    // Phase decision 1: D17 also bounds label adoption, or the next run would call it stale.
    let by_label: Vec<&WorkspaceGroupMemberInfo> = candidates
        .iter()
        .copied()
        .filter(|member| {
            cwd_within(member_key(state, member), &dir.key)
                && state
                    .labels
                    .get(&member.workspace_id)
                    .is_some_and(|current| same_name(current, label))
        })
        .collect();
    match by_label.as_slice() {
        [] => Adoption::Missing,
        [member] => Adoption::Found {
            workspace_id: member.workspace_id.clone(),
            by: "label",
        },
        several => Adoption::Ambiguous(several.len()),
    }
}

/// Place and set_color for an adopted or `--reapply`ed workspace. Without `force`
/// only what the UI left unset is filled: an ungrouped workspace, a missing override.
fn membership_ops(
    member: &WorkspaceGroupMemberInfo,
    group: Option<&GroupRef>,
    color: Option<&str>,
    force: bool,
) -> Vec<PlannedOp> {
    let workspace = WsRef::Existing(member.workspace_id.clone());
    let mut ops = Vec::new();
    if let Some(group) = group {
        let in_group = matches!(
            group,
            GroupRef::Existing(id) if member.group_id.as_deref() == Some(id.as_str())
        );
        if member.group_id.is_none() || (force && !in_group) {
            ops.push(PlannedOp::Place {
                workspace: workspace.clone(),
                group: group.clone(),
            });
        }
    }
    if let Some(color) = color {
        if member.color.is_none() || (force && member.color.as_deref() != Some(color)) {
            ops.push(PlannedOp::SetColor {
                workspace,
                color: color.to_string(),
            });
        }
    }
    ops
}

/// `--reapply` on a bound workspace: the file's label, group and color where they differ.
fn reapply_workspace(
    name: &str,
    label: &str,
    member: &WorkspaceGroupMemberInfo,
    group: Option<&GroupRef>,
    color: Option<&str>,
    state: &ServerState,
    steps: &mut Vec<Step>,
) {
    let workspace_id = &member.workspace_id;
    let mut changed = Vec::new();
    if state.labels.get(workspace_id).map(String::as_str) != Some(label) {
        steps.push(Step::RenameWorkspace {
            workspace_id: workspace_id.clone(),
            label: label.to_string(),
        });
        changed.push("label");
    }
    let ops = membership_ops(member, group, color, true);
    if ops.iter().any(|op| matches!(op, PlannedOp::Place { .. })) {
        changed.push("group");
    }
    if ops
        .iter()
        .any(|op| matches!(op, PlannedOp::SetColor { .. }))
    {
        changed.push("color");
    }
    if !ops.is_empty() {
        steps.push(Step::Edit { ops, done: None });
    }
    if !changed.is_empty() {
        steps.push(Step::Report {
            level: Level::Progress,
            line: format!(
                "project {name}: reapplied {} ({workspace_id})",
                changed.join(", ")
            ),
        });
    }
}

/// Rule 3.4: `workspace.create`, one batch binding it (and its root tab to the
/// first `[[project.tab]]`), the root tab's rename and launch, then the other tabs.
fn plan_create(
    spec: &ProjectSpec,
    dir: &ProjectDir,
    group: Option<&GroupRef>,
    color: Option<&str>,
    state: &ServerState,
    steps: &mut Vec<Step>,
) {
    let name = spec.name.as_str();
    let label = spec.label.as_deref().unwrap_or(name);
    steps.push(Step::CreateWorkspace {
        project: name.to_string(),
        cwd: dir.path.clone(),
        label: label.to_string(),
    });
    let workspace = WsRef::Created(name.to_string());
    let root = NewTab::Root(name.to_string());
    let mut ops = vec![PlannedOp::Bind {
        workspace: workspace.clone(),
        project: Some(name.to_string()),
    }];
    if let Some(group) = group {
        ops.push(PlannedOp::Place {
            workspace: workspace.clone(),
            group: group.clone(),
        });
    }
    if let Some(color) = color {
        ops.push(PlannedOp::SetColor {
            workspace: workspace.clone(),
            color: color.to_string(),
        });
    }
    let Some(first) = spec.tab.first() else {
        steps.push(Step::Edit { ops, done: None });
        return;
    };
    ops.push(PlannedOp::BindTab {
        workspace: workspace.clone(),
        key: first.label.clone(),
        tab: TabRef::New(root.clone()),
    });
    steps.push(Step::Edit { ops, done: None });
    steps.push(Step::RenameTab {
        tab: root.clone(),
        label: first.label.clone(),
    });
    // Rule 4.1 for the root tab of a workspace created in this run: launch it as in 4.4.
    match agent_conflict(spec, first, state) {
        Some(line) => steps.push(Step::Report {
            level: Level::Skipped,
            line,
        }),
        None => push_launch(spec, first, root, steps),
    }
    for tab in spec.tab.iter().skip(1) {
        plan_new_tab(spec, dir, tab, &workspace, state, steps);
    }
}

/// Spec 11.4 step 3 for one selected project. `members` is the planner's
/// working copy: queued binds are applied to it so later rules see them.
fn plan_project(
    spec: &ProjectSpec,
    group: Option<&GroupRef>,
    state: &ServerState,
    members: &mut [WorkspaceGroupMemberInfo],
    reapply: bool,
    steps: &mut Vec<Step>,
) {
    let name = spec.name.as_str();
    let Some(dir) = state.project_dirs.get(&name.to_lowercase()) else {
        steps.push(Step::Report {
            level: Level::Skipped,
            line: format!("project {name}: cwd was not prepared; skipped"),
        });
        return;
    };
    if !dir.exists {
        steps.push(Step::Report {
            level: Level::Skipped,
            line: format!("project {name}: cwd {} does not exist; skipped", dir.path),
        });
        return;
    }
    let first_step = steps.len();
    let label = spec.label.as_deref().unwrap_or(name);
    let color = spec.color.as_deref().and_then(normalize_workspace_color);
    let mut excluded = BTreeSet::new();
    let mut bound = None;
    // Rule 3.1 with D17: a binding whose workspace cwd lies outside the project cwd is stale.
    if let Some(member) = members.iter_mut().find(|member| {
        member
            .project
            .as_deref()
            .is_some_and(|project| same_name(project, name))
    }) {
        if cwd_within(member_key(state, member), &dir.key) {
            bound = Some(member.workspace_id.clone());
        } else {
            steps.push(Step::Report {
                level: Level::Warning,
                line: format!(
                    "warning: {name} was bound to {} (cwd {}); rebinding",
                    member.workspace_id, member.cwd
                ),
            });
            steps.push(Step::Edit {
                ops: vec![PlannedOp::Bind {
                    workspace: WsRef::Existing(member.workspace_id.clone()),
                    project: None,
                }],
                done: None,
            });
            member.project = None;
            member.tabs.clear();
            excluded.insert(member.workspace_id.clone());
        }
    }
    let workspace_id = match bound {
        Some(workspace_id) => {
            if reapply {
                if let Some(member) = members
                    .iter()
                    .find(|member| member.workspace_id == workspace_id)
                {
                    reapply_workspace(name, label, member, group, color, state, steps);
                }
            }
            workspace_id
        }
        None => match find_adoption(members, &excluded, state, dir, label) {
            Adoption::Found { workspace_id, by } => {
                if let Some(member) = members
                    .iter_mut()
                    .find(|member| member.workspace_id == workspace_id)
                {
                    let mut ops = vec![PlannedOp::Bind {
                        workspace: WsRef::Existing(workspace_id.clone()),
                        project: Some(name.to_string()),
                    }];
                    ops.extend(membership_ops(member, group, color, reapply));
                    steps.push(Step::Edit {
                        ops,
                        done: Some(format!("project {name}: adopted {workspace_id} by {by}")),
                    });
                    if reapply && state.labels.get(&workspace_id).map(String::as_str) != Some(label)
                    {
                        steps.push(Step::RenameWorkspace {
                            workspace_id: workspace_id.clone(),
                            label: label.to_string(),
                        });
                    }
                    member.project = Some(name.to_string());
                }
                workspace_id
            }
            Adoption::Ambiguous(count) => {
                steps.push(Step::Report {
                    level: Level::Skipped,
                    line: format!(
                        "project {name}: {count} workspaces match label {label}; skipped"
                    ),
                });
                return;
            }
            Adoption::Missing => {
                plan_create(spec, dir, group, color, state, steps);
                return;
            }
        },
    };
    let bindings = members
        .iter()
        .find(|member| member.workspace_id == workspace_id)
        .map(|member| member.tabs.clone())
        .unwrap_or_default();
    let tabs_start = steps.len();
    plan_tabs(spec, dir, &workspace_id, &bindings, state, steps);
    if steps.len() == first_step {
        steps.push(Step::Report {
            level: Level::Progress,
            line: format!("project {name}: up to date ({workspace_id})"),
        });
    } else if tabs_start == first_step {
        // Only tab work: name the project first so its indented tab lines have a heading.
        steps.insert(
            tabs_start,
            Step::Report {
                level: Level::Progress,
                line: format!("project {name}: bound to {workspace_id}"),
            },
        );
    }
}

/// One named agent from `agent.list`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveAgent {
    name: String,
    workspace_id: String,
    tab_id: String,
    pane_id: String,
}

/// A tab this run creates: the root tab of a created workspace, or a `tab.create`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum NewTab {
    Root(String),
    Created { project: String, key: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TabRef {
    Existing(String),
    New(NewTab),
}

/// What a tab created in this run starts (rule 4.4).
#[derive(Debug, Clone, PartialEq, Eq)]
enum TabLaunch {
    Agent {
        name: String,
        kind: String,
        args: Vec<String>,
    },
    Command {
        text: String,
    },
}

fn launch_for(project: &ProjectSpec, tab: &TabSpec) -> Option<TabLaunch> {
    if let Some(kind) = &tab.agent {
        return Some(TabLaunch::Agent {
            name: agent_name_for(project, tab),
            kind: kind.clone(),
            args: tab.args.clone().unwrap_or_default(),
        });
    }
    tab.command
        .as_ref()
        .map(|text| TabLaunch::Command { text: text.clone() })
}

/// The pre-launch check of rule 4.4: the tab's agent name is live elsewhere (names are server-wide).
fn agent_conflict(project: &ProjectSpec, tab: &TabSpec, state: &ServerState) -> Option<String> {
    let name = tab.agent.as_ref().map(|_| agent_name_for(project, tab))?;
    let holder = state.agents.iter().find(|agent| agent.name == name)?;
    Some(format!(
        "tab {}: agent name {name} is already used by {} in {}; set agent_name in projects.toml",
        tab.label, holder.pane_id, holder.workspace_id
    ))
}

/// The launch of a tab created in this run, or its "created" line when it launches nothing.
fn push_launch(project: &ProjectSpec, tab: &TabSpec, new_tab: NewTab, steps: &mut Vec<Step>) {
    let label = &tab.label;
    match launch_for(project, tab) {
        Some(launch) => {
            let done = match &launch {
                TabLaunch::Agent { name, .. } => {
                    format!("  tab {label}: created, agent {name} started")
                }
                TabLaunch::Command { .. } => format!("  tab {label}: created, command sent"),
            };
            steps.push(Step::Launch {
                tab: new_tab,
                key: label.clone(),
                launch,
                done,
            });
        }
        None => steps.push(Step::Report {
            level: Level::Progress,
            line: format!("  tab {label}: created"),
        }),
    }
}

fn bind_tab_step(workspace: &WsRef, tab: &TabSpec, tab_id: &str, by: &str) -> Step {
    Step::Edit {
        ops: vec![PlannedOp::BindTab {
            workspace: workspace.clone(),
            key: tab.label.clone(),
            tab: TabRef::Existing(tab_id.to_string()),
        }],
        done: Some(format!("  tab {}: bound {tab_id} by {by}", tab.label)),
    }
}

/// Rule 4.4: `tab.create`, `bind_tab`, then the launch. A name conflict skips the
/// whole tab, so a later run creates and launches it once `agent_name` is fixed.
fn plan_new_tab(
    project: &ProjectSpec,
    dir: &ProjectDir,
    tab: &TabSpec,
    workspace: &WsRef,
    state: &ServerState,
    steps: &mut Vec<Step>,
) {
    if let Some(line) = agent_conflict(project, tab, state) {
        steps.push(Step::Report {
            level: Level::Skipped,
            line,
        });
        return;
    }
    let new_tab = NewTab::Created {
        project: project.name.clone(),
        key: tab.label.clone(),
    };
    let cwd = dir
        .tabs
        .get(&tab.label.to_lowercase())
        .cloned()
        .unwrap_or_else(|| dir.path.clone());
    steps.push(Step::CreateTab {
        project: project.name.clone(),
        key: tab.label.clone(),
        workspace: workspace.clone(),
        cwd,
    });
    steps.push(Step::Edit {
        ops: vec![PlannedOp::BindTab {
            workspace: workspace.clone(),
            key: tab.label.clone(),
            tab: TabRef::New(new_tab.clone()),
        }],
        done: None,
    });
    push_launch(project, tab, new_tab, steps);
}

/// Spec 11.4 step 4 for a workspace that already exists, tabs in file order.
/// `bindings` lists live bindings only, so a listed key is a live tab (4.1).
fn plan_tabs(
    project: &ProjectSpec,
    dir: &ProjectDir,
    workspace_id: &str,
    bindings: &[WorkspaceGroupTabBinding],
    state: &ServerState,
    steps: &mut Vec<Step>,
) {
    let live_tabs = state
        .tabs
        .get(workspace_id)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut bound: BTreeSet<String> = bindings
        .iter()
        .map(|binding| binding.tab_id.clone())
        .collect();
    let workspace = WsRef::Existing(workspace_id.to_string());
    for tab in &project.tab {
        // 4.1: bound to a live tab. Renaming it in the UI never relaunches it.
        if bindings
            .iter()
            .any(|binding| same_name(&binding.key, &tab.label))
        {
            continue;
        }
        // 4.2: the tab's agent is live in this workspace.
        if tab.agent.is_some() {
            let name = agent_name_for(project, tab);
            if let Some(agent) = state.agents.iter().find(|agent| {
                agent.name == name
                    && agent.workspace_id == workspace_id
                    && !bound.contains(&agent.tab_id)
            }) {
                steps.push(bind_tab_step(&workspace, tab, &agent.tab_id, "agent name"));
                bound.insert(agent.tab_id.clone());
                continue;
            }
        }
        // 4.3: an unbound tab with the same label.
        if let Some(info) = live_tabs
            .iter()
            .find(|info| !bound.contains(&info.tab_id) && same_name(&info.label, &tab.label))
        {
            steps.push(bind_tab_step(&workspace, tab, &info.tab_id, "label"));
            bound.insert(info.tab_id.clone());
            continue;
        }
        // 4.4: create it.
        plan_new_tab(project, dir, tab, &workspace, state, steps);
    }
}

const EXIT_OK: i32 = 0;
const EXIT_FAILED: i32 = 1;
const EXIT_SKIPPED: i32 = 3;
const EXIT_USAGE: i32 = 2;

/// `projects.toml` lives in the herdr config dir (`crate::config::config_dir()`).
pub(super) const PROJECTS_FILE_NAME: &str = "projects.toml";

const USAGE: &str = "usage: herdr up <project>... [--reapply]
       herdr up --group <group> [--reapply]
       herdr up --all [--reapply]

Creates or adopts the groups, workspaces and tabs that projects.toml (in the
herdr config directory) declares, through the running server. It never
deletes, closes or reorders anything and never changes focus; a second run
reports up to date. --reapply re-asserts the file's labels, groups and colors
and restores groups deleted in the UI; --group <group> restores that group.";
const REQUEST_ID: &str = "cli:up";

/// Spec 11.4 4.4: retry `agent_pane_busy` every 250 ms for up to 15 s (upstream's own CLI uses 2 s).
const BUSY_RETRY_INTERVAL: Duration = Duration::from_millis(250);
const BUSY_RETRY_BUDGET: Duration = Duration::from_secs(15);
const BUSY_RETRY_LIMIT: u128 = BUSY_RETRY_BUDGET.as_millis() / BUSY_RETRY_INTERVAL.as_millis();

/// The executor's only contact with the outside world, so tests can script a
/// server. Requests and responses are in wire form, the JSON objects that
/// `super::send_request` exchanges with the server.
trait UpIo {
    fn send(&mut self, request: &serde_json::Value) -> std::io::Result<serde_json::Value>;
    fn pause(&mut self, duration: Duration);
    fn line(&mut self, to_stderr: bool, text: &str);
}

/// `Ok(Ok(response))` on success, `Ok(Err(response))` for an API error envelope.
type Answer = std::io::Result<Result<serde_json::Value, serde_json::Value>>;

fn send_wire(io: &mut impl UpIo, request: &serde_json::Value) -> Answer {
    let response = io.send(request)?;
    Ok(if response.get("error").is_some() {
        Err(response)
    } else {
        Ok(response)
    })
}

/// One request; the typed params are checked at compile time.
fn call(io: &mut impl UpIo, method: Method) -> Answer {
    let request = serde_json::to_value(Request {
        id: REQUEST_ID.into(),
        method,
    })
    .map_err(std::io::Error::other)?;
    send_wire(io, &request)
}

fn parse_result(response: &serde_json::Value) -> std::io::Result<ResponseResult> {
    serde_json::from_value(response["result"].clone()).map_err(std::io::Error::other)
}

/// The `ResponseResult::WorkspaceGroups` that `workspace_group.list` and `.edit` answer with,
/// as (groups, members in display order, deleted templates).
fn parse_groups(
    response: &serde_json::Value,
    method: &str,
) -> std::io::Result<(
    Vec<WorkspaceGroupInfo>,
    Vec<WorkspaceGroupMemberInfo>,
    Vec<String>,
)> {
    match parse_result(response)? {
        ResponseResult::WorkspaceGroups {
            groups,
            workspaces,
            deleted_templates,
        } => Ok((groups, workspaces, deleted_templates)),
        _ => Err(unexpected(method)),
    }
}

fn unexpected(method: &str) -> std::io::Error {
    std::io::Error::other(format!("{method} returned an unexpected result"))
}

fn unresolved(what: &str, name: &str) -> std::io::Error {
    std::io::Error::other(format!(
        "herdr up: {what} {name} was not created earlier in this run"
    ))
}

/// One read request; an API error envelope is printed and becomes exit 1.
fn read(io: &mut impl UpIo, method: Method) -> std::io::Result<Result<ResponseResult, i32>> {
    match call(io, method)? {
        Ok(response) => parse_result(&response).map(Ok),
        Err(response) => {
            io.line(true, &response.to_string());
            Ok(Err(EXIT_FAILED))
        }
    }
}

/// Spec 11.4 step 1. `Ok(Err(code))`: the failure was reported; the run ends with `code`.
fn read_state(io: &mut impl UpIo) -> std::io::Result<Result<ServerState, i32>> {
    let response = match call(io, Method::WorkspaceGroupList(EmptyParams::default()))? {
        Ok(response) => response,
        Err(response) => {
            // Upstream answers a method it cannot parse with `invalid_request` (spec 11.2).
            if response["error"]["code"].as_str() == Some("invalid_request") {
                io.line(
                    true,
                    "server does not support workspace groups (fork server required)",
                );
            } else {
                io.line(true, &response.to_string());
            }
            return Ok(Err(EXIT_FAILED));
        }
    };
    let (groups, members, deleted_templates) = parse_groups(&response, "workspace_group.list")?;
    let member_keys = members
        .iter()
        .map(|member| {
            let key = crate::worktree::canonical_or_original(Path::new(&member.cwd));
            (
                member.workspace_id.clone(),
                key.to_string_lossy().into_owned(),
            )
        })
        .collect();
    let labels = match read(io, Method::WorkspaceList(EmptyParams::default()))? {
        Ok(ResponseResult::WorkspaceList { workspaces }) => workspaces
            .into_iter()
            .map(|workspace| (workspace.workspace_id, workspace.label))
            .collect(),
        Ok(_) => return Err(unexpected("workspace.list")),
        Err(code) => return Ok(Err(code)),
    };
    let agents = match read(io, Method::AgentList(EmptyParams::default()))? {
        Ok(ResponseResult::AgentList { agents }) => agents
            .into_iter()
            .filter_map(|agent| {
                Some(LiveAgent {
                    name: agent.name?,
                    workspace_id: agent.workspace_id,
                    tab_id: agent.tab_id,
                    pane_id: agent.pane_id,
                })
            })
            .collect(),
        Ok(_) => return Err(unexpected("agent.list")),
        Err(code) => return Ok(Err(code)),
    };
    let mut tabs: BTreeMap<String, Vec<TabInfo>> = BTreeMap::new();
    match read(io, Method::TabList(TabListParams::default()))? {
        Ok(ResponseResult::TabList { tabs: all }) => {
            for tab in all {
                tabs.entry(tab.workspace_id.clone()).or_default().push(tab);
            }
        }
        Ok(_) => return Err(unexpected("tab.list")),
        Err(code) => return Ok(Err(code)),
    }
    Ok(Ok(ServerState {
        groups,
        deleted_templates,
        members,
        member_keys,
        labels,
        agents,
        tabs,
        project_dirs: BTreeMap::new(),
    }))
}

/// Ids of what this run created, for steps that refer to them symbolically.
#[derive(Debug, Default)]
struct Resolved {
    /// Project (file spelling) -> workspace id.
    workspaces: BTreeMap<String, String>,
    /// New tab -> (tab id, root pane id).
    tabs: BTreeMap<NewTab, (String, String)>,
    /// Lowercase template -> group id.
    groups: BTreeMap<String, String>,
}

impl Resolved {
    fn learn_groups(&mut self, groups: &[WorkspaceGroupInfo]) {
        for group in groups {
            if let Some(template) = &group.template {
                self.groups
                    .insert(template.to_lowercase(), group.group_id.clone());
            }
        }
    }

    fn workspace(&self, workspace: &WsRef) -> std::io::Result<String> {
        match workspace {
            WsRef::Existing(id) => Ok(id.clone()),
            WsRef::Created(project) => self
                .workspaces
                .get(project)
                .cloned()
                .ok_or_else(|| unresolved("workspace of project", project)),
        }
    }

    fn tab(&self, tab: &NewTab) -> std::io::Result<&(String, String)> {
        self.tabs
            .get(tab)
            .ok_or_else(|| unresolved("tab", &format!("{tab:?}")))
    }

    fn group(&self, group: &GroupRef) -> std::io::Result<String> {
        match group {
            GroupRef::Existing(id) => Ok(id.clone()),
            GroupRef::Template(template) => self
                .groups
                .get(&template.to_lowercase())
                .cloned()
                .ok_or_else(|| unresolved("group", template)),
        }
    }

    fn op(&self, op: &PlannedOp) -> std::io::Result<WorkspaceGroupOp> {
        Ok(match op {
            PlannedOp::CreateGroup {
                name,
                color,
                template,
            } => WorkspaceGroupOp::Create {
                name: name.clone(),
                color: color.clone(),
                template: Some(template.clone()),
                workspace_ids: Vec::new(),
                before_group_id: None,
            },
            PlannedOp::UpdateGroup {
                group_id,
                name,
                color,
                template,
            } => WorkspaceGroupOp::Update {
                group_id: group_id.clone(),
                name: name.clone(),
                color: color.clone(),
                template: template.clone(),
            },
            PlannedOp::Place { workspace, group } => WorkspaceGroupOp::Place {
                workspace_id: self.workspace(workspace)?,
                group_id: Some(self.group(group)?),
                before_workspace_id: None,
            },
            PlannedOp::SetColor { workspace, color } => WorkspaceGroupOp::SetColor {
                workspace_id: self.workspace(workspace)?,
                color: Some(color.clone()),
            },
            PlannedOp::Bind { workspace, project } => WorkspaceGroupOp::Bind {
                workspace_id: self.workspace(workspace)?,
                project: project.clone(),
            },
            PlannedOp::BindTab {
                workspace,
                key,
                tab,
            } => WorkspaceGroupOp::BindTab {
                workspace_id: self.workspace(workspace)?,
                key: key.clone(),
                tab_id: Some(match tab {
                    TabRef::Existing(id) => id.clone(),
                    TabRef::New(new_tab) => self.tab(new_tab)?.0.clone(),
                }),
            },
        })
    }
}

enum StepOutcome {
    Done,
    Skipped,
    Failed(serde_json::Value),
}

/// How an agent launch ended when it did not fail the run.
enum AgentOutcome {
    Started,
    Skipped,
}

/// `agent.start` in the root pane of a tab created in this run (spec 11.4 4.4, Agent):
/// `agent_pane_busy` is retried every 250 ms for up to 15 s, then the manual command is
/// printed; `agent_name_taken` is never retried. `Ok(Err(response))`: any other API error.
fn start_agent(
    io: &mut impl UpIo,
    key: &str,
    pane_id: &str,
    name: &str,
    kind: &str,
    args: &[String],
) -> std::io::Result<Result<AgentOutcome, serde_json::Value>> {
    let mut retries: u128 = 0;
    loop {
        let method = Method::AgentStart(AgentStartParams {
            name: name.to_string(),
            kind: kind.to_string(),
            pane_id: pane_id.to_string(),
            args: args.to_vec(),
            timeout_ms: None,
        });
        let response = match call(io, method)? {
            Ok(_) => return Ok(Ok(AgentOutcome::Started)),
            Err(response) => response,
        };
        match response["error"]["code"].as_str() {
            Some("agent_pane_busy") if retries < BUSY_RETRY_LIMIT => {
                retries += 1;
                io.pause(BUSY_RETRY_INTERVAL);
            }
            Some("agent_pane_busy") => {
                io.line(
                    true,
                    &format!(
                        "tab {key}: pane {pane_id} is still busy after {} s; agent {name} not started",
                        BUSY_RETRY_BUDGET.as_secs()
                    ),
                );
                let mut hint =
                    format!("run: herdr agent start {name} --kind {kind} --pane {pane_id}");
                if !args.is_empty() {
                    hint.push_str(" -- ");
                    hint.push_str(&args.join(" "));
                }
                io.line(true, &hint);
                return Ok(Ok(AgentOutcome::Skipped));
            }
            Some("agent_name_taken") => {
                let (holder_pane, holder_workspace) = match live_agent_named(io, name)? {
                    Some(agent) => (agent.pane_id, agent.workspace_id),
                    None => ("unknown".to_string(), "unknown".to_string()),
                };
                io.line(
                    true,
                    &format!(
                        "tab {key}: agent name {name} is already used by {holder_pane} in {holder_workspace}; set agent_name in projects.toml"
                    ),
                );
                return Ok(Ok(AgentOutcome::Skipped));
            }
            _ => return Ok(Err(response)),
        }
    }
}

/// The live agent holding `name`, from a fresh `agent.list` (None when it is gone or the list failed).
fn live_agent_named(io: &mut impl UpIo, name: &str) -> std::io::Result<Option<LiveAgent>> {
    let Ok(response) = call(io, Method::AgentList(EmptyParams::default()))? else {
        return Ok(None);
    };
    let ResponseResult::AgentList { agents } = parse_result(&response)? else {
        return Err(unexpected("agent.list"));
    };
    Ok(agents
        .into_iter()
        .find(|agent| agent.name.as_deref() == Some(name))
        .map(|agent| LiveAgent {
            name: name.to_string(),
            workspace_id: agent.workspace_id,
            tab_id: agent.tab_id,
            pane_id: agent.pane_id,
        }))
}

/// Runs one step. API errors become `StepOutcome::Failed`; transport errors are `Err`.
fn run_step(
    io: &mut impl UpIo,
    step: &Step,
    resolved: &mut Resolved,
) -> std::io::Result<StepOutcome> {
    match step {
        Step::Report { level, line } => {
            io.line(*level != Level::Progress, line);
            if *level == Level::Skipped {
                return Ok(StepOutcome::Skipped);
            }
        }
        Step::Edit { ops, done } => {
            let ops = ops
                .iter()
                .map(|op| resolved.op(op))
                .collect::<std::io::Result<Vec<_>>>()?;
            let method = Method::WorkspaceGroupEdit(WorkspaceGroupEditParams { ops });
            let response = match call(io, method)? {
                Ok(response) => response,
                Err(response) => return Ok(StepOutcome::Failed(response)),
            };
            let (groups, _, _) = parse_groups(&response, "workspace_group.edit")?;
            resolved.learn_groups(&groups);
            if let Some(done) = done {
                io.line(false, done);
            }
        }
        Step::CreateWorkspace {
            project,
            cwd,
            label,
        } => {
            let method = Method::WorkspaceCreate(WorkspaceCreateParams {
                source_workspace_id: None,
                cwd: Some(cwd.clone()),
                focus: false,
                label: Some(label.clone()),
                env: HashMap::new(),
            });
            let response = match call(io, method)? {
                Ok(response) => response,
                Err(response) => return Ok(StepOutcome::Failed(response)),
            };
            let result = &response["result"];
            let (Some(workspace_id), Some(tab_id), Some(pane_id)) = (
                result["workspace"]["workspace_id"].as_str(),
                result["tab"]["tab_id"].as_str(),
                result["root_pane"]["pane_id"].as_str(),
            ) else {
                return Err(unexpected("workspace.create"));
            };
            resolved
                .workspaces
                .insert(project.clone(), workspace_id.to_string());
            resolved.tabs.insert(
                NewTab::Root(project.clone()),
                (tab_id.to_string(), pane_id.to_string()),
            );
            io.line(false, &format!("project {project}: created {workspace_id}"));
        }
        Step::RenameWorkspace {
            workspace_id,
            label,
        } => {
            let method = Method::WorkspaceRename(WorkspaceRenameParams {
                workspace_id: workspace_id.clone(),
                label: label.clone(),
            });
            if let Err(response) = call(io, method)? {
                return Ok(StepOutcome::Failed(response));
            }
        }
        Step::CreateTab {
            project,
            key,
            workspace,
            cwd,
        } => {
            let method = Method::TabCreate(TabCreateParams {
                workspace_id: Some(resolved.workspace(workspace)?),
                cwd: Some(cwd.clone()),
                focus: false,
                label: Some(key.clone()),
                env: HashMap::new(),
            });
            let response = match call(io, method)? {
                Ok(response) => response,
                Err(response) => return Ok(StepOutcome::Failed(response)),
            };
            let result = &response["result"];
            let (Some(tab_id), Some(pane_id)) = (
                result["tab"]["tab_id"].as_str(),
                result["root_pane"]["pane_id"].as_str(),
            ) else {
                return Err(unexpected("tab.create"));
            };
            resolved.tabs.insert(
                NewTab::Created {
                    project: project.clone(),
                    key: key.clone(),
                },
                (tab_id.to_string(), pane_id.to_string()),
            );
        }
        Step::RenameTab { tab, label } => {
            let tab_id = resolved.tab(tab)?.0.clone();
            let method = Method::TabRename(TabRenameParams {
                tab_id,
                label: label.clone(),
            });
            if let Err(response) = call(io, method)? {
                return Ok(StepOutcome::Failed(response));
            }
        }
        Step::Launch {
            tab,
            key,
            launch,
            done,
        } => {
            let pane_id = resolved.tab(tab)?.1.clone();
            match launch {
                TabLaunch::Command { text } => {
                    // `keys` must be explicit: over JSON it defaults to empty (spec 11.4).
                    let method = Method::PaneSendInput(PaneSendInputParams {
                        pane_id,
                        text: text.clone(),
                        keys: vec!["Enter".to_string()],
                    });
                    if let Err(response) = call(io, method)? {
                        return Ok(StepOutcome::Failed(response));
                    }
                }
                TabLaunch::Agent { name, kind, args } => {
                    match start_agent(io, key, &pane_id, name, kind, args)? {
                        Ok(AgentOutcome::Started) => {}
                        Ok(AgentOutcome::Skipped) => return Ok(StepOutcome::Skipped),
                        Err(response) => return Ok(StepOutcome::Failed(response)),
                    }
                }
            }
            io.line(false, done);
        }
    }
    Ok(StepOutcome::Done)
}

/// Spec 11.4: read, plan, apply. Exit 0 applied or up to date; 1 at the first failed
/// request (the progress lines already printed say what was applied); 3 when
/// something was skipped.
fn execute(
    io: &mut impl UpIo,
    file: &ProjectsFile,
    selection: &Selection,
    reapply: bool,
    project_dirs: BTreeMap<String, ProjectDir>,
) -> std::io::Result<i32> {
    let mut state = match read_state(io)? {
        Ok(state) => state,
        Err(code) => return Ok(code),
    };
    state.project_dirs = project_dirs;
    let steps = plan(file, selection, &state, reapply);
    let mut resolved = Resolved::default();
    resolved.learn_groups(&state.groups);
    let mut skipped = false;
    for step in &steps {
        match run_step(io, step, &mut resolved)? {
            StepOutcome::Done => {}
            StepOutcome::Skipped => skipped = true,
            StepOutcome::Failed(response) => {
                io.line(true, &response.to_string());
                return Ok(EXIT_FAILED);
            }
        }
    }
    Ok(if skipped { EXIT_SKIPPED } else { EXIT_OK })
}

/// Production I/O: the shared CLI request path and real output.
struct LiveIo;

impl UpIo for LiveIo {
    fn send(&mut self, request: &serde_json::Value) -> std::io::Result<serde_json::Value> {
        // Decoded into the client's typed request, so a shape its schema does not
        // know fails here, before anything is sent.
        let request: Request =
            serde_json::from_value(request.clone()).map_err(std::io::Error::other)?;
        super::send_request(&request)
    }

    fn pause(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }

    fn line(&mut self, to_stderr: bool, text: &str) {
        if to_stderr {
            eprintln!("{text}");
        } else {
            println!("{text}");
        }
    }
}

/// Everything that can fail before the first request (exit 2): read, parse,
/// validate, select and prepare directories.
fn load_inputs(
    path: &Path,
    selection: &Selection,
) -> Result<(ProjectsFile, BTreeMap<String, ProjectDir>), String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "projects file not found: {} (an example ships as docs/next/fork/projects.example.toml in the fork source)",
                path.display()
            ));
        }
        Err(err) => return Err(format!("cannot read {}: {err}", path.display())),
    };
    let file = parse_projects(&text).map_err(|err| format!("{}: {err}", path.display()))?;
    validate(&file).map_err(|err| format!("{}: {err}", path.display()))?;
    select(&file, selection)?;
    let dirs = prepare_dirs(&file).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok((file, dirs))
}

/// `herdr up` with the projects file and I/O injected.
fn run_up(args: &[String], projects_path: &Path, io: &mut impl UpIo) -> std::io::Result<i32> {
    let (selection, reapply) = match parse_args(args) {
        Ok(ParsedArgs::Help) => {
            io.line(false, USAGE);
            return Ok(EXIT_OK);
        }
        Ok(ParsedArgs::Run { selection, reapply }) => (selection, reapply),
        Err(message) => {
            io.line(true, &message);
            io.line(true, USAGE);
            return Ok(EXIT_USAGE);
        }
    };
    let (file, project_dirs) = match load_inputs(projects_path, &selection) {
        Ok(inputs) => inputs,
        Err(message) => {
            io.line(true, &message);
            return Ok(EXIT_USAGE);
        }
    };
    execute(io, &file, &selection, reapply, project_dirs)
}

/// `herdr up` (spec 11): exit 0 applied or up to date, 1 server or API failure,
/// 2 usage or file error, 3 completed but skipped something. A dead server
/// surfaces as upstream's `server_not_running` error through `main`.
pub(super) fn run_up_command(args: &[String]) -> std::io::Result<i32> {
    let projects_path = crate::config::config_dir().join(PROJECTS_FILE_NAME);
    run_up(args, &projects_path, &mut LiveIo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const EXAMPLE: &str = r#"
[[group]]
name = "Work"
color = "blue"

[[group]]
name = "Personal"

[[project]]
name = "api"
label = "API"
cwd = '~\src\api'
group = "Work"
color = "teal"

  [[project.tab]]
  label = "agent"
  agent = "claude"
  args = ["--verbose"]

  [[project.tab]]
  label = "dev"
  command = "npm run dev"
  cwd = '~\src\api\server'

[[project]]
name = "web"
cwd = '~\src\web'
group = "Work"

  [[project.tab]]
  label = "agent"
  agent = "claude"
  agent_name = "web-main"
"#;

    fn parsed(text: &str) -> ProjectsFile {
        parse_projects(text).expect("projects.toml parses")
    }

    #[test]
    fn parses_groups_projects_tabs_in_file_order() {
        let file = parsed(EXAMPLE);
        assert_eq!(
            file.group
                .iter()
                .map(|group| group.name.as_str())
                .collect::<Vec<_>>(),
            ["Work", "Personal"]
        );
        assert_eq!(file.group[0].color.as_deref(), Some("blue"));
        assert_eq!(file.group[1].color, None);
        assert_eq!(
            file.project
                .iter()
                .map(|project| project.name.as_str())
                .collect::<Vec<_>>(),
            ["api", "web"]
        );
        let api = &file.project[0];
        assert_eq!(api.label.as_deref(), Some("API"));
        assert_eq!(api.cwd, r"~\src\api");
        assert_eq!(api.group.as_deref(), Some("Work"));
        assert_eq!(api.color.as_deref(), Some("teal"));
        assert_eq!(
            api.tab
                .iter()
                .map(|tab| tab.label.as_str())
                .collect::<Vec<_>>(),
            ["agent", "dev"]
        );
        assert_eq!(
            api.tab[0],
            TabSpec {
                label: "agent".into(),
                agent: Some("claude".into()),
                args: Some(vec!["--verbose".into()]),
                agent_name: None,
                command: None,
                cwd: None,
            }
        );
        assert_eq!(api.tab[1].command.as_deref(), Some("npm run dev"));
        assert_eq!(api.tab[1].cwd.as_deref(), Some(r"~\src\api\server"));
        assert_eq!(file.project[1].label, None);
        assert_eq!(
            file.project[1].tab[0].agent_name.as_deref(),
            Some("web-main")
        );
        assert_eq!(validate(&file), Ok(()));
        assert_eq!(parsed(""), ProjectsFile::default());
    }

    #[test]
    fn rejects_unknown_keys_duplicates_unresolved_group_bad_color_agent_and_command_together() {
        for text in [
            "[[project]]\nname = \"api\"\ncwd = \"/a\"\nlabels = \"x\"\n",
            "[[group]]\nname = \"Work\"\nicon = \"x\"\n",
            "[[project]]\nname = \"api\"\ncwd = \"/a\"\n[[project.tab]]\nlabel = \"a\"\nshell = \"pwsh\"\n",
            "[projects]\nname = \"api\"\n",
            "[[project]]\nname = \"api\"\n",
        ] {
            assert!(parse_projects(text).is_err(), "should not parse: {text}");
        }
        let project = "[[project]]\nname = \"api\"\ncwd = \"/a\"\n";
        let long_name = "x".repeat(41);
        let cases = [
            (
                "[[group]]\nname = \"Work\"\n[[group]]\nname = \"work\"\n".to_string(),
                "duplicate group name work",
            ),
            (
                format!("{project}[[project]]\nname = \"API\"\ncwd = \"/b\"\n"),
                "duplicate project name API",
            ),
            (
                format!(
                    "{project}[[project.tab]]\nlabel = \"dev\"\n[[project.tab]]\nlabel = \"Dev\"\n"
                ),
                "project api: duplicate tab label Dev",
            ),
            (
                format!("{project}group = \"Missing\"\n"),
                "project api: unknown group Missing",
            ),
            (
                "[[group]]\nname = \"Work\"\ncolor = \"magenta\"\n".to_string(),
                "group Work: unknown color magenta; use one of red, orange",
            ),
            (
                format!("{project}color = \"#ff0000\"\n"),
                "project api: unknown color #ff0000",
            ),
            (
                format!(
                    "{project}[[project.tab]]\nlabel = \"dev\"\nagent = \"claude\"\ncommand = \"npm run dev\"\n"
                ),
                "project api tab dev: agent and command are mutually exclusive",
            ),
            (
                format!("{project}[[project.tab]]\nlabel = \"dev\"\nargs = [\"-x\"]\n"),
                "project api tab dev: args requires agent",
            ),
            (
                format!("{project}[[project.tab]]\nlabel = \"dev\"\nagent_name = \"dev\"\n"),
                "project api tab dev: agent_name requires agent",
            ),
            (
                format!(
                    "{project}[[project.tab]]\nlabel = \"agent\"\nagent = \"claude\"\ncwd = \"/a/sub\"\n"
                ),
                "project api tab agent: the first tab starts in the project cwd",
            ),
            (
                format!("{project}[[project.tab]]\nlabel = \"agent\"\nagent = \"vim\"\n"),
                "project api tab agent: unsupported agent kind vim",
            ),
            (
                format!("[[group]]\nname = \"{long_name}\"\n"),
                "names must be 1-40 characters",
            ),
            (
                "[[group]]\nname = \" Work\"\n".to_string(),
                "names must be 1-40 characters",
            ),
            (
                "[[project]]\nname = \" \"\ncwd = \"/a\"\n".to_string(),
                "project names must not be empty",
            ),
            (
                format!("{project}[[project.tab]]\nlabel = \"\"\n"),
                "project api: tab labels must not be empty",
            ),
        ];
        for (text, expected) in cases {
            let error = validate(&parsed(&text)).expect_err(&text);
            assert!(error.contains(expected), "{text}\n=> {error}");
        }
    }

    #[test]
    fn derived_agent_names_follow_upstream_rule_and_are_unique() {
        let file = parsed(
            "[[project]]\nname = \"api\"\ncwd = \"/a\"\n[[project.tab]]\nlabel = \"agent\"\nagent = \"claude\"\n[[project.tab]]\nlabel = \"review\"\nagent = \"codex\"\nagent_name = \"api_review-2\"\n",
        );
        let api = &file.project[0];
        assert_eq!(agent_name_for(api, &api.tab[0]), "api-agent");
        assert_eq!(agent_name_for(api, &api.tab[1]), "api_review-2");
        assert_eq!(validate(&file), Ok(()));

        let longest = "a".repeat(32);
        for name in ["a", "api-agent", "x_1", longest.as_str()] {
            assert!(valid_agent_name(name), "{name}");
        }
        let too_long = "a".repeat(33);
        for name in [
            "",
            "1api",
            "Api",
            "api agent",
            "api.agent",
            "-api",
            too_long.as_str(),
        ] {
            assert!(!valid_agent_name(name), "{name}");
        }

        let long_label = "x".repeat(30);
        for (project, label) in [
            ("API", "agent"),
            ("api", "my agent"),
            ("api", long_label.as_str()),
        ] {
            let text = format!(
                "[[project]]\nname = \"{project}\"\ncwd = \"/a\"\n[[project.tab]]\nlabel = \"{label}\"\nagent = \"claude\"\n"
            );
            let error = validate(&parsed(&text)).expect_err(&text);
            assert!(error.contains("is not valid; set agent_name"), "{error}");
        }

        let duplicate = "[[project]]\nname = \"api\"\ncwd = \"/a\"\n[[project.tab]]\nlabel = \"agent\"\nagent = \"claude\"\n[[project]]\nname = \"web\"\ncwd = \"/b\"\n[[project.tab]]\nlabel = \"main\"\nagent = \"claude\"\nagent_name = \"api-agent\"\n";
        assert_eq!(
            validate(&parsed(duplicate)),
            Err(
                "project web tab main: agent name api-agent is already used by project api tab agent"
                    .to_string()
            )
        );
    }

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("herdr-up-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    #[test]
    fn requires_exactly_one_selection_mode() {
        let rejected: [&[&str]; 8] = [
            &[],
            &["--reapply"],
            &["api", "--all"],
            &["--group", "Work", "--all"],
            &["api", "--group", "Work"],
            &["--group"],
            &["--group", "Work", "--group", "Infra"],
            &["--bogus"],
        ];
        for values in rejected {
            assert!(parse_args(&args(values)).is_err(), "{values:?}");
        }
        assert_eq!(
            parse_args(&args(&[])),
            Err("choose exactly one of <project>..., --group <group> or --all".to_string())
        );
        assert_eq!(
            parse_args(&args(&["api", "web"])),
            Ok(ParsedArgs::Run {
                selection: Selection::Projects(vec!["api".into(), "web".into()]),
                reapply: false,
            })
        );
        assert_eq!(
            parse_args(&args(&["--group=Work", "--reapply"])),
            Ok(ParsedArgs::Run {
                selection: Selection::Group("Work".into()),
                reapply: true,
            })
        );
        assert_eq!(
            parse_args(&args(&["--all"])),
            Ok(ParsedArgs::Run {
                selection: Selection::All,
                reapply: false,
            })
        );
        assert_eq!(
            parse_args(&args(&["--", "--odd-name"])),
            Ok(ParsedArgs::Run {
                selection: Selection::Projects(vec!["--odd-name".into()]),
                reapply: false,
            })
        );
        for help in [&["--help"][..], &["api", "-h"], &["help"]] {
            assert_eq!(parse_args(&args(help)), Ok(ParsedArgs::Help), "{help:?}");
        }

        let file = parsed(EXAMPLE);
        assert_eq!(
            select(
                &file,
                &Selection::Projects(vec!["WEB".into(), "api".into(), "api".into()])
            ),
            Ok(Selected {
                groups: vec![0],
                projects: vec![0, 1],
            })
        );
        assert_eq!(
            select(&file, &Selection::Group("work".into())),
            Ok(Selected {
                groups: vec![0],
                projects: vec![0, 1],
            })
        );
        assert_eq!(
            select(&file, &Selection::All),
            Ok(Selected {
                groups: vec![0, 1],
                projects: vec![0, 1],
            })
        );
        assert_eq!(
            select(&file, &Selection::Projects(vec!["nope".into()])),
            Err("unknown project nope in projects.toml".to_string())
        );
        assert_eq!(
            select(&file, &Selection::Group("nope".into())),
            Err("unknown group nope in projects.toml".to_string())
        );
    }

    #[test]
    fn prepare_dirs_expands_tilde_checks_existence_and_rejects_relative_cwd() {
        let root = scratch_dir("prepare");
        let present = root.join("api");
        std::fs::create_dir_all(&present).expect("create project dir");
        let missing = root.join("missing");
        let text = format!(
            "[[project]]\nname = \"api\"\ncwd = '{}'\n[[project.tab]]\nlabel = \"agent\"\n[[project.tab]]\nlabel = \"Dev\"\ncwd = '{}'\n[[project]]\nname = \"Gone\"\ncwd = '{}'\n[[project]]\nname = \"home\"\ncwd = '~/herdr-up-test'\n",
            present.display(),
            present.join("server").display(),
            missing.display()
        );
        let dirs = prepare_dirs(&parsed(&text)).expect("absolute cwds prepare");
        let api = &dirs["api"];
        assert!(api.exists);
        assert_eq!(api.path, present.to_string_lossy());
        assert_eq!(
            api.key,
            crate::worktree::canonical_or_original(&present).to_string_lossy()
        );
        assert_eq!(api.tabs["dev"], present.join("server").to_string_lossy());
        assert!(!dirs["gone"].exists);
        let home = &dirs["home"];
        assert!(
            !home.path.starts_with('~') && std::path::Path::new(&home.path).is_absolute(),
            "{}",
            home.path
        );
        assert_eq!(
            prepare_dirs(&parsed("[[project]]\nname = \"rel\"\ncwd = \"src/rel\"\n")),
            Err("project rel: cwd src/rel is not an absolute path".to_string())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    const PLAN: &str = r#"
[[group]]
name = "Work"
color = "blue"

[[group]]
name = "Infra"
color = "gray"

[[project]]
name = "api"
label = "API"
cwd = "/src/api"
group = "Work"
color = "teal"

[[project]]
name = "docs"
cwd = "/src/docs"

[[project]]
name = "infra"
cwd = "/src/infra"
group = "Infra"
"#;

    /// Test directories: the file's cwd strings as given, all existing.
    fn dirs_for(file: &ProjectsFile) -> BTreeMap<String, ProjectDir> {
        file.project
            .iter()
            .map(|project| {
                let tabs = project
                    .tab
                    .iter()
                    .filter_map(|tab| Some((tab.label.to_lowercase(), tab.cwd.clone()?)))
                    .collect();
                (
                    project.name.to_lowercase(),
                    ProjectDir {
                        path: project.cwd.clone(),
                        key: project.cwd.clone(),
                        exists: true,
                        tabs,
                    },
                )
            })
            .collect()
    }

    fn state_for(file: &ProjectsFile) -> ServerState {
        ServerState {
            project_dirs: dirs_for(file),
            ..ServerState::default()
        }
    }

    fn with_member<'a>(
        state: &'a mut ServerState,
        id: &str,
        cwd: &str,
        label: &str,
    ) -> &'a mut WorkspaceGroupMemberInfo {
        state.labels.insert(id.to_string(), label.to_string());
        state.members.push(WorkspaceGroupMemberInfo {
            workspace_id: id.to_string(),
            group_id: None,
            color: None,
            project: None,
            cwd: cwd.to_string(),
            linked: false,
            tabs: Vec::new(),
        });
        state.members.last_mut().expect("member just pushed")
    }

    fn group_info(id: &str, name: &str, color: &str, template: Option<&str>) -> WorkspaceGroupInfo {
        WorkspaceGroupInfo {
            group_id: id.to_string(),
            name: name.to_string(),
            color: color.to_string(),
            template: template.map(str::to_string),
        }
    }

    fn report(level: Level, line: &str) -> Step {
        Step::Report {
            level,
            line: line.to_string(),
        }
    }

    fn edit(ops: Vec<PlannedOp>, done: Option<&str>) -> Step {
        Step::Edit {
            ops,
            done: done.map(str::to_string),
        }
    }

    fn existing(id: &str) -> WsRef {
        WsRef::Existing(id.to_string())
    }

    fn created(project: &str) -> WsRef {
        WsRef::Created(project.to_string())
    }

    fn bind(workspace: WsRef, project: Option<&str>) -> PlannedOp {
        PlannedOp::Bind {
            workspace,
            project: project.map(str::to_string),
        }
    }

    fn place(workspace: WsRef, group: GroupRef) -> PlannedOp {
        PlannedOp::Place { workspace, group }
    }

    fn set_color(workspace: WsRef, color: &str) -> PlannedOp {
        PlannedOp::SetColor {
            workspace,
            color: color.to_string(),
        }
    }

    fn create_workspace(project: &str, cwd: &str, label: &str) -> Step {
        Step::CreateWorkspace {
            project: project.to_string(),
            cwd: cwd.to_string(),
            label: label.to_string(),
        }
    }

    #[test]
    fn plan_project_without_tabs_creates_bare_workspace() {
        let file = parsed(PLAN);
        assert_eq!(
            plan(
                &file,
                &Selection::Projects(vec!["docs".into()]),
                &state_for(&file),
                false
            ),
            vec![
                create_workspace("docs", "/src/docs", "docs"),
                edit(vec![bind(created("docs"), Some("docs"))], None),
            ]
        );
    }

    #[test]
    fn plan_adopts_by_cwd_when_label_is_repo_name() {
        let file = parsed(
            "[[project]]\nname = \"api\"\ncwd = \"/src/mono/api\"\n[[project]]\nname = \"web\"\ncwd = \"/src/mono/web\"\n",
        );
        let mut state = state_for(&file);
        with_member(&mut state, "w1", "/src/mono/web", "mono");
        with_member(&mut state, "w2", "/src/mono/api", "mono");
        with_member(&mut state, "w3", "/src/mono", "mono");
        with_member(&mut state, "w4", "/src/mono/api", "mono").linked = true;
        assert_eq!(
            plan(&file, &Selection::All, &state, false),
            vec![
                edit(
                    vec![bind(existing("w2"), Some("api"))],
                    Some("project api: adopted w2 by cwd")
                ),
                edit(
                    vec![bind(existing("w1"), Some("web"))],
                    Some("project web: adopted w1 by cwd")
                ),
            ]
        );
    }

    #[test]
    fn plan_adopts_single_label_match_and_skips_ambiguous() {
        let file = parsed(
            "[[project]]\nname = \"api\"\nlabel = \"API\"\ncwd = \"/src/api\"\n[[project]]\nname = \"web\"\ncwd = \"/src/web\"\n[[project]]\nname = \"docs\"\ncwd = \"/src/docs\"\n",
        );
        let mut state = state_for(&file);
        with_member(&mut state, "w1", "/src/api/server", "api");
        with_member(&mut state, "w2", "/src/web/a", "web");
        with_member(&mut state, "w3", "/src/web/b", "Web");
        with_member(&mut state, "w4", "/src/docs", "scratch");
        with_member(&mut state, "w5", "/src/docs", "docs");
        // Same label, but outside the project cwd: never adopted (phase decision 1).
        with_member(&mut state, "w6", "/elsewhere", "docs");
        assert_eq!(
            plan(&file, &Selection::All, &state, false),
            vec![
                edit(
                    vec![bind(existing("w1"), Some("api"))],
                    Some("project api: adopted w1 by label")
                ),
                report(
                    Level::Skipped,
                    "project web: 2 workspaces match label web; skipped"
                ),
                edit(
                    vec![bind(existing("w5"), Some("docs"))],
                    Some("project docs: adopted w5 by label")
                ),
            ]
        );
    }

    #[test]
    fn plan_treats_binding_outside_project_cwd_as_stale() {
        let file = parsed(
            "[[project]]\nname = \"api\"\ncwd = \"/src/api\"\n[[project]]\nname = \"web\"\ncwd = \"/src/web\"\n",
        );
        let mut state = state_for(&file);
        with_member(&mut state, "w5", "/elsewhere", "api").project = Some("api".into());
        with_member(&mut state, "w6", "/src/api", "api");
        // Inside the project cwd (case-insensitive): a valid binding.
        with_member(&mut state, "w7", "/SRC/Web/Server", "web").project = Some("web".into());
        assert_eq!(
            plan(&file, &Selection::All, &state, false),
            vec![
                report(
                    Level::Warning,
                    "warning: api was bound to w5 (cwd /elsewhere); rebinding"
                ),
                edit(vec![bind(existing("w5"), None)], None),
                edit(
                    vec![bind(existing("w6"), Some("api"))],
                    Some("project api: adopted w6 by cwd")
                ),
                report(Level::Progress, "project web: up to date (w7)"),
            ]
        );

        // With no other candidate the stale workspace is still not re-adopted, even by label.
        let mut alone = state_for(&file);
        with_member(&mut alone, "w5", "/elsewhere", "api").project = Some("api".into());
        assert_eq!(
            plan(
                &file,
                &Selection::Projects(vec!["api".into()]),
                &alone,
                false
            ),
            vec![
                report(
                    Level::Warning,
                    "warning: api was bound to w5 (cwd /elsewhere); rebinding"
                ),
                edit(vec![bind(existing("w5"), None)], None),
                create_workspace("api", "/src/api", "api"),
                edit(vec![bind(created("api"), Some("api"))], None),
            ]
        );
    }

    #[test]
    fn plan_never_adopts_a_workspace_bound_to_another_project() {
        // `up` binds only unbound workspaces, so tab bindings never carry over from another
        // project: a workspace bound to a project outside the file is not adopted, although
        // its cwd and label both match.
        let file = parsed("[[project]]\nname = \"api\"\ncwd = \"/src/api\"\n");
        let mut state = state_for(&file);
        let other = with_member(&mut state, "w1", "/src/api", "api");
        other.project = Some("legacy".into());
        other.tabs = vec![WorkspaceGroupTabBinding {
            key: "agent".into(),
            tab_id: "w1:t1".into(),
        }];
        assert_eq!(
            plan(&file, &Selection::All, &state, false),
            vec![
                create_workspace("api", "/src/api", "api"),
                edit(vec![bind(created("api"), Some("api"))], None),
            ]
        );
    }

    #[test]
    fn plan_matches_renamed_group_by_template() {
        let file = parsed(PLAN);
        let mut state = state_for(&file);
        state.groups = vec![
            group_info("g1", "Job", "green", Some("Work")),
            group_info("g2", "Infra", "gray", Some("Ops")),
        ];
        assert_eq!(
            plan(&file, &Selection::All, &state, false),
            vec![
                report(Level::Progress, "group Work: up to date (g1)"),
                report(
                    Level::Skipped,
                    "group Infra: name taken by a group bound to Ops; skipped"
                ),
                create_workspace("api", "/src/api", "API"),
                edit(
                    vec![
                        bind(created("api"), Some("api")),
                        place(created("api"), GroupRef::Existing("g1".into())),
                        set_color(created("api"), "teal"),
                    ],
                    None
                ),
                create_workspace("docs", "/src/docs", "docs"),
                edit(vec![bind(created("docs"), Some("docs"))], None),
                create_workspace("infra", "/src/infra", "infra"),
                edit(vec![bind(created("infra"), Some("infra"))], None),
            ]
        );
    }

    #[test]
    fn plan_respects_ui_ungroup_cleared_color_and_deleted_group() {
        let file = parsed(PLAN);
        let mut state = state_for(&file);
        state.groups = vec![group_info("g1", "Work", "blue", Some("Work"))];
        state.deleted_templates = vec!["infra".into()];
        // Ungrouped and without a color override: both are UI choices after the first bind.
        with_member(&mut state, "w1", "/src/api", "API").project = Some("api".into());
        let docs = with_member(&mut state, "w2", "/src/docs", "docs");
        docs.project = Some("docs".into());
        docs.group_id = Some("g1".into());
        assert_eq!(
            plan(&file, &Selection::All, &state, false),
            vec![
                report(Level::Progress, "group Work: up to date (g1)"),
                report(
                    Level::Progress,
                    "group Infra was deleted in the UI; run herdr up --group Infra to restore it"
                ),
                report(Level::Progress, "project api: up to date (w1)"),
                report(Level::Progress, "project docs: up to date (w2)"),
                create_workspace("infra", "/src/infra", "infra"),
                edit(vec![bind(created("infra"), Some("infra"))], None),
            ]
        );
    }

    #[test]
    fn plan_group_selection_recreates_tombstoned_group() {
        let file = parsed(PLAN);
        let mut state = state_for(&file);
        state.deleted_templates = vec!["Infra".into()];
        assert_eq!(
            plan(&file, &Selection::Group("infra".into()), &state, false),
            vec![
                edit(
                    vec![PlannedOp::CreateGroup {
                        name: "Infra".into(),
                        color: Some("gray".into()),
                        template: "Infra".into(),
                    }],
                    Some("group Infra: created")
                ),
                create_workspace("infra", "/src/infra", "infra"),
                edit(
                    vec![
                        bind(created("infra"), Some("infra")),
                        place(created("infra"), GroupRef::Template("Infra".into())),
                    ],
                    None
                ),
            ]
        );
        // Selecting only the project respects the tombstone.
        assert_eq!(
            plan(
                &file,
                &Selection::Projects(vec!["infra".into()]),
                &state,
                false
            ),
            vec![
                report(
                    Level::Progress,
                    "group Infra was deleted in the UI; run herdr up --group Infra to restore it"
                ),
                create_workspace("infra", "/src/infra", "infra"),
                edit(vec![bind(created("infra"), Some("infra"))], None),
            ]
        );
        // An unbound group that already has the name is adopted, not duplicated.
        state.groups = vec![group_info("g7", "INFRA", "red", None)];
        assert_eq!(
            plan(&file, &Selection::Group("Infra".into()), &state, false)[0],
            edit(
                vec![PlannedOp::UpdateGroup {
                    group_id: "g7".into(),
                    name: None,
                    color: None,
                    template: Some("Infra".into()),
                }],
                Some("group Infra: adopted g7")
            )
        );
    }

    #[test]
    fn plan_reapply_restores_label_group_color() {
        let file = parsed(PLAN);
        let mut state = state_for(&file);
        state.groups = vec![group_info("g1", "Job", "green", Some("Work"))];
        state.deleted_templates = vec!["Infra".into()];
        let api = with_member(&mut state, "w1", "/src/api", "renamed");
        api.project = Some("api".into());
        api.color = Some("red".into());
        let docs = with_member(&mut state, "w3", "/src/docs", "docs");
        docs.project = Some("docs".into());
        docs.group_id = Some("g1".into());
        with_member(&mut state, "w2", "/src/infra", "infra").project = Some("infra".into());
        assert_eq!(
            plan(&file, &Selection::All, &state, true),
            vec![
                edit(
                    vec![PlannedOp::UpdateGroup {
                        group_id: "g1".into(),
                        name: Some("Work".into()),
                        color: Some("blue".into()),
                        template: None,
                    }],
                    Some("group Work: reapplied name and color")
                ),
                edit(
                    vec![PlannedOp::CreateGroup {
                        name: "Infra".into(),
                        color: Some("gray".into()),
                        template: "Infra".into(),
                    }],
                    Some("group Infra: created")
                ),
                Step::RenameWorkspace {
                    workspace_id: "w1".into(),
                    label: "API".into(),
                },
                edit(
                    vec![
                        place(existing("w1"), GroupRef::Existing("g1".into())),
                        set_color(existing("w1"), "teal"),
                    ],
                    None
                ),
                report(
                    Level::Progress,
                    "project api: reapplied label, group, color (w1)"
                ),
                report(Level::Progress, "project docs: up to date (w3)"),
                edit(
                    vec![place(existing("w2"), GroupRef::Template("Infra".into()))],
                    None
                ),
                report(Level::Progress, "project infra: reapplied group (w2)"),
            ]
        );
        // Plain `up` changes none of it.
        assert_eq!(
            plan(&file, &Selection::All, &state, false),
            vec![
                report(Level::Progress, "group Work: up to date (g1)"),
                report(
                    Level::Progress,
                    "group Infra was deleted in the UI; run herdr up --group Infra to restore it"
                ),
                report(Level::Progress, "project api: up to date (w1)"),
                report(Level::Progress, "project docs: up to date (w3)"),
                report(Level::Progress, "project infra: up to date (w2)"),
            ]
        );
    }

    const TABS: &str = r#"
[[group]]
name = "Work"
color = "blue"

[[project]]
name = "api"
cwd = "/src/api"
group = "Work"
color = "teal"

  [[project.tab]]
  label = "agent"
  agent = "claude"
  args = ["--continue"]

  [[project.tab]]
  label = "dev"
  command = "npm run dev"
  cwd = "/src/api/server"

  [[project.tab]]
  label = "notes"

[[project]]
name = "docs"
cwd = "/src/docs"
"#;

    fn root(project: &str) -> NewTab {
        NewTab::Root(project.to_string())
    }

    fn new_tab(project: &str, key: &str) -> NewTab {
        NewTab::Created {
            project: project.to_string(),
            key: key.to_string(),
        }
    }

    fn bind_tab(workspace: WsRef, key: &str, tab: TabRef) -> PlannedOp {
        PlannedOp::BindTab {
            workspace,
            key: key.to_string(),
            tab,
        }
    }

    fn binding(key: &str, tab_id: &str) -> WorkspaceGroupTabBinding {
        WorkspaceGroupTabBinding {
            key: key.to_string(),
            tab_id: tab_id.to_string(),
        }
    }

    fn tab_info(workspace_id: &str, number: usize, label: &str) -> TabInfo {
        TabInfo {
            tab_id: format!("{workspace_id}:t{number}"),
            workspace_id: workspace_id.to_string(),
            number,
            label: label.to_string(),
            focused: false,
            pane_count: 1,
            agent_status: crate::api::schema::AgentStatus::Idle,
        }
    }

    fn live_agent(name: &str, workspace_id: &str, tab_number: usize) -> LiveAgent {
        LiveAgent {
            name: name.to_string(),
            workspace_id: workspace_id.to_string(),
            tab_id: format!("{workspace_id}:t{tab_number}"),
            pane_id: format!("{workspace_id}:p{tab_number}"),
        }
    }

    fn launch_targets(steps: &[Step]) -> Vec<NewTab> {
        steps
            .iter()
            .filter_map(|step| match step {
                Step::Launch { tab, .. } => Some(tab.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn plan_creates_everything_on_empty_state() {
        let file = parsed(TABS);
        assert_eq!(
            plan(&file, &Selection::All, &state_for(&file), false),
            vec![
                edit(
                    vec![PlannedOp::CreateGroup {
                        name: "Work".into(),
                        color: Some("blue".into()),
                        template: "Work".into(),
                    }],
                    Some("group Work: created")
                ),
                create_workspace("api", "/src/api", "api"),
                edit(
                    vec![
                        bind(created("api"), Some("api")),
                        place(created("api"), GroupRef::Template("Work".into())),
                        set_color(created("api"), "teal"),
                        bind_tab(created("api"), "agent", TabRef::New(root("api"))),
                    ],
                    None
                ),
                Step::RenameTab {
                    tab: root("api"),
                    label: "agent".into(),
                },
                Step::Launch {
                    tab: root("api"),
                    key: "agent".into(),
                    launch: TabLaunch::Agent {
                        name: "api-agent".into(),
                        kind: "claude".into(),
                        args: vec!["--continue".into()],
                    },
                    done: "  tab agent: created, agent api-agent started".into(),
                },
                Step::CreateTab {
                    project: "api".into(),
                    key: "dev".into(),
                    workspace: created("api"),
                    cwd: "/src/api/server".into(),
                },
                edit(
                    vec![bind_tab(
                        created("api"),
                        "dev",
                        TabRef::New(new_tab("api", "dev"))
                    )],
                    None
                ),
                Step::Launch {
                    tab: new_tab("api", "dev"),
                    key: "dev".into(),
                    launch: TabLaunch::Command {
                        text: "npm run dev".into(),
                    },
                    done: "  tab dev: created, command sent".into(),
                },
                Step::CreateTab {
                    project: "api".into(),
                    key: "notes".into(),
                    workspace: created("api"),
                    cwd: "/src/api".into(),
                },
                edit(
                    vec![bind_tab(
                        created("api"),
                        "notes",
                        TabRef::New(new_tab("api", "notes"))
                    )],
                    None
                ),
                report(Level::Progress, "  tab notes: created"),
                create_workspace("docs", "/src/docs", "docs"),
                edit(vec![bind(created("docs"), Some("docs"))], None),
            ]
        );
    }

    #[test]
    fn plan_launches_first_tab_in_root_tab_of_created_workspace() {
        let file = parsed(TABS);
        let api = Selection::Projects(vec!["api".into()]);
        let created_plan = plan(&file, &api, &state_for(&file), false);
        assert_eq!(
            launch_targets(&created_plan),
            vec![root("api"), new_tab("api", "dev")]
        );
        assert!(!created_plan
            .iter()
            .any(|step| matches!(step, Step::CreateTab { key, .. } if key == "agent")));

        // An adopted workspace keeps its root tab: the agent gets a tab of its own.
        let mut state = state_for(&file);
        with_member(&mut state, "w1", "/src/api", "api");
        state
            .tabs
            .insert("w1".into(), vec![tab_info("w1", 1, "pwsh")]);
        let adopted = plan(&file, &api, &state, false);
        assert_eq!(
            adopted[1],
            edit(
                vec![
                    bind(existing("w1"), Some("api")),
                    place(existing("w1"), GroupRef::Template("Work".into())),
                    set_color(existing("w1"), "teal"),
                ],
                Some("project api: adopted w1 by cwd")
            )
        );
        assert_eq!(
            launch_targets(&adopted),
            vec![new_tab("api", "agent"), new_tab("api", "dev")]
        );
        assert!(!adopted
            .iter()
            .any(|step| matches!(step, Step::RenameTab { .. })));
    }

    #[test]
    fn plan_matches_renamed_tab_by_binding() {
        let file = parsed(TABS);
        let mut state = state_for(&file);
        state.groups = vec![group_info("g1", "Work", "blue", Some("Work"))];
        let api = with_member(&mut state, "w1", "/src/api", "api");
        api.project = Some("api".into());
        api.group_id = Some("g1".into());
        api.color = Some("teal".into());
        api.tabs = vec![
            binding("agent", "w1:t1"),
            binding("dev", "w1:t2"),
            binding("notes", "w1:t4"),
        ];
        with_member(&mut state, "w2", "/src/docs", "docs").project = Some("docs".into());
        state.tabs.insert(
            "w1".into(),
            vec![
                tab_info("w1", 1, "claude (renamed)"),
                tab_info("w1", 2, "server"),
                tab_info("w1", 4, "scratch"),
            ],
        );
        state.agents = vec![live_agent("api-agent", "w1", 1)];
        assert_eq!(
            plan(&file, &Selection::All, &state, false),
            vec![
                report(Level::Progress, "group Work: up to date (g1)"),
                report(Level::Progress, "project api: up to date (w1)"),
                report(Level::Progress, "project docs: up to date (w2)"),
            ]
        );

        // A closed tab (its binding is no longer listed) is recreated; nothing else relaunches.
        if let Some(api) = state.members.first_mut() {
            api.tabs.truncate(2);
        }
        state.tabs.insert(
            "w1".into(),
            vec![
                tab_info("w1", 1, "claude (renamed)"),
                tab_info("w1", 2, "server"),
            ],
        );
        assert_eq!(
            plan(
                &file,
                &Selection::Projects(vec!["api".into()]),
                &state,
                false
            ),
            vec![
                report(Level::Progress, "group Work: up to date (g1)"),
                report(Level::Progress, "project api: bound to w1"),
                Step::CreateTab {
                    project: "api".into(),
                    key: "notes".into(),
                    workspace: existing("w1"),
                    cwd: "/src/api".into(),
                },
                edit(
                    vec![bind_tab(
                        existing("w1"),
                        "notes",
                        TabRef::New(new_tab("api", "notes"))
                    )],
                    None
                ),
                report(Level::Progress, "  tab notes: created"),
            ]
        );
    }

    #[test]
    fn plan_binds_live_agent_tab_by_agent_name() {
        let file = parsed(TABS);
        let mut state = state_for(&file);
        state.groups = vec![group_info("g1", "Work", "blue", Some("Work"))];
        let api = with_member(&mut state, "w1", "/src/api", "api");
        api.project = Some("api".into());
        api.group_id = Some("g1".into());
        api.color = Some("teal".into());
        state.tabs.insert(
            "w1".into(),
            vec![
                tab_info("w1", 1, "pwsh"),
                tab_info("w1", 2, "work"),
                tab_info("w1", 3, "DEV"),
            ],
        );
        state.agents = vec![live_agent("api-agent", "w1", 2)];
        assert_eq!(
            plan(
                &file,
                &Selection::Projects(vec!["api".into()]),
                &state,
                false
            ),
            vec![
                report(Level::Progress, "group Work: up to date (g1)"),
                report(Level::Progress, "project api: bound to w1"),
                edit(
                    vec![bind_tab(
                        existing("w1"),
                        "agent",
                        TabRef::Existing("w1:t2".into())
                    )],
                    Some("  tab agent: bound w1:t2 by agent name")
                ),
                edit(
                    vec![bind_tab(
                        existing("w1"),
                        "dev",
                        TabRef::Existing("w1:t3".into())
                    )],
                    Some("  tab dev: bound w1:t3 by label")
                ),
                Step::CreateTab {
                    project: "api".into(),
                    key: "notes".into(),
                    workspace: existing("w1"),
                    cwd: "/src/api".into(),
                },
                edit(
                    vec![bind_tab(
                        existing("w1"),
                        "notes",
                        TabRef::New(new_tab("api", "notes"))
                    )],
                    None
                ),
                report(Level::Progress, "  tab notes: created"),
            ]
        );
    }

    #[test]
    fn plan_skips_tab_whose_agent_name_is_live_elsewhere() {
        let file = parsed(TABS);
        let api = Selection::Projects(vec!["api".into()]);
        let conflict = "tab agent: agent name api-agent is already used by w9:p1 in w9; set agent_name in projects.toml";
        let mut state = state_for(&file);
        state.agents = vec![live_agent("api-agent", "w9", 1)];

        // Created workspace: its root tab is bound and renamed, but nothing is launched in it.
        let steps = plan(&file, &api, &state, false);
        assert_eq!(steps[4], report(Level::Skipped, conflict));
        assert_eq!(launch_targets(&steps), vec![new_tab("api", "dev")]);

        // Existing workspace: the agent tab is not created at all.
        with_member(&mut state, "w1", "/src/api", "api");
        let steps = plan(&file, &api, &state, false);
        assert_eq!(steps[2], report(Level::Skipped, conflict));
        assert!(!steps
            .iter()
            .any(|step| matches!(step, Step::CreateTab { key, .. } if key == "agent")));
        assert_eq!(launch_targets(&steps), vec![new_tab("api", "dev")]);
    }

    const RUN: &str = r#"
[[group]]
name = "Work"
color = "blue"

[[project]]
name = "api"
cwd = "/herdr-up-test/api"
group = "Work"
color = "teal"

  [[project.tab]]
  label = "agent"
  agent = "claude"

  [[project.tab]]
  label = "dev"
  command = "npm run dev"

[[project]]
name = "docs"
cwd = "/herdr-up-test/docs"
"#;

    type FakeError = (&'static str, String);

    #[derive(Debug, Clone)]
    struct FakeTab {
        tab_id: String,
        label: String,
        pane_id: String,
    }

    #[derive(Debug)]
    struct FakeWorkspace {
        id: String,
        label: String,
        cwd: String,
        linked: bool,
        group_id: Option<String>,
        color: Option<String>,
        project: Option<String>,
        /// Binding key (stored spelling) -> tab id.
        bindings: BTreeMap<String, String>,
        tabs: Vec<FakeTab>,
    }

    /// An in-memory server that answers the wire requests `herdr up` sends, with
    /// the phase 02 op semantics `up` relies on, and records everything.
    #[derive(Debug, Default)]
    struct FakeServer {
        /// Answer `workspace_group.list` like an upstream server.
        upstream: bool,
        fail_tab_create: bool,
        groups: Vec<WorkspaceGroupInfo>,
        deleted_templates: Vec<String>,
        workspaces: Vec<FakeWorkspace>,
        agents: Vec<LiveAgent>,
        /// Every request, in wire form.
        requests: Vec<serde_json::Value>,
        lines: Vec<(bool, String)>,
        next_workspace: usize,
        next_group: usize,
        /// Pane id -> how many more `agent.start` calls answer `agent_pane_busy`.
        busy: BTreeMap<String, usize>,
        /// Agents that appear (and take their name) only when `agent.start` asks for it.
        race_agents: Vec<LiveAgent>,
        pauses: Vec<Duration>,
    }

    impl FakeServer {
        fn add_workspace(&mut self, label: &str, cwd: &str) -> String {
            self.next_workspace += 1;
            let id = format!("w{}", self.next_workspace);
            self.workspaces.push(FakeWorkspace {
                id: id.clone(),
                label: label.to_string(),
                cwd: cwd.to_string(),
                linked: false,
                group_id: None,
                color: None,
                project: None,
                bindings: BTreeMap::new(),
                tabs: vec![FakeTab {
                    tab_id: format!("{id}:t1"),
                    label: "shell".into(),
                    pane_id: format!("{id}:p1"),
                }],
            });
            id
        }

        fn add_tab(&mut self, workspace_id: &str, label: &str) -> Result<FakeTab, FakeError> {
            let workspace = self.workspace_mut(workspace_id)?;
            let number = workspace.tabs.len() + 1;
            let tab = FakeTab {
                tab_id: format!("{workspace_id}:t{number}"),
                label: label.to_string(),
                pane_id: format!("{workspace_id}:p{number}"),
            };
            workspace.tabs.push(tab.clone());
            Ok(tab)
        }

        fn workspace(&self, id: &str) -> &FakeWorkspace {
            self.workspaces
                .iter()
                .find(|workspace| workspace.id == id)
                .expect("fake workspace exists")
        }

        fn workspace_mut(&mut self, id: &str) -> Result<&mut FakeWorkspace, FakeError> {
            self.workspaces
                .iter_mut()
                .find(|workspace| workspace.id == id)
                .ok_or(("workspace_not_found", id.to_string()))
        }

        fn groups_result(&self) -> serde_json::Value {
            let workspaces: Vec<WorkspaceGroupMemberInfo> = self
                .workspaces
                .iter()
                .map(|workspace| WorkspaceGroupMemberInfo {
                    workspace_id: workspace.id.clone(),
                    group_id: workspace
                        .group_id
                        .clone()
                        .filter(|id| self.groups.iter().any(|group| group.group_id == *id)),
                    color: workspace.color.clone(),
                    project: workspace.project.clone(),
                    cwd: workspace.cwd.clone(),
                    linked: workspace.linked,
                    tabs: workspace
                        .bindings
                        .iter()
                        .filter(|(_, tab_id)| {
                            workspace.tabs.iter().any(|tab| tab.tab_id == **tab_id)
                        })
                        .map(|(key, tab_id)| WorkspaceGroupTabBinding {
                            key: key.clone(),
                            tab_id: tab_id.clone(),
                        })
                        .collect(),
                })
                .collect();
            json!({
                "type": "workspace_groups",
                "groups": self.groups,
                "workspaces": workspaces,
                "deleted_templates": self.deleted_templates,
            })
        }

        fn clear_template(&mut self, template: &str) {
            self.deleted_templates
                .retain(|deleted| !same_name(deleted, template));
            for group in &mut self.groups {
                if group
                    .template
                    .as_deref()
                    .is_some_and(|bound| same_name(bound, template))
                {
                    group.template = None;
                }
            }
        }

        fn apply(&mut self, op: &WorkspaceGroupOp) -> Result<(), FakeError> {
            match op {
                WorkspaceGroupOp::Create {
                    name,
                    color,
                    template,
                    ..
                } => {
                    if let Some(template) = template {
                        self.clear_template(template);
                    }
                    self.next_group += 1;
                    self.groups.push(WorkspaceGroupInfo {
                        group_id: format!("g{}", self.next_group),
                        name: name.clone(),
                        color: color.clone().unwrap_or_else(|| "blue".into()),
                        template: template.clone(),
                    });
                }
                WorkspaceGroupOp::Update {
                    group_id,
                    name,
                    color,
                    template,
                } => {
                    if let Some(template) = template {
                        self.clear_template(template);
                    }
                    let group = self
                        .groups
                        .iter_mut()
                        .find(|group| group.group_id == *group_id)
                        .ok_or(("workspace_group_not_found", group_id.clone()))?;
                    if let Some(name) = name {
                        group.name = name.clone();
                    }
                    if let Some(color) = color {
                        group.color = color.clone();
                    }
                    if template.is_some() {
                        group.template = template.clone();
                    }
                }
                WorkspaceGroupOp::Place {
                    workspace_id,
                    group_id,
                    ..
                } => self.workspace_mut(workspace_id)?.group_id = group_id.clone(),
                WorkspaceGroupOp::SetColor {
                    workspace_id,
                    color,
                } => self.workspace_mut(workspace_id)?.color = color.clone(),
                WorkspaceGroupOp::Bind {
                    workspace_id,
                    project,
                } => {
                    if let Some(project) = project {
                        for other in &mut self.workspaces {
                            if other.id != *workspace_id
                                && other
                                    .project
                                    .as_deref()
                                    .is_some_and(|bound| same_name(bound, project))
                            {
                                other.project = None;
                                other.bindings.clear();
                            }
                        }
                    }
                    let workspace = self.workspace_mut(workspace_id)?;
                    if project.is_none() {
                        workspace.bindings.clear();
                    }
                    workspace.project = project.clone();
                }
                WorkspaceGroupOp::BindTab {
                    workspace_id,
                    key,
                    tab_id,
                } => {
                    let workspace = self.workspace_mut(workspace_id)?;
                    workspace
                        .bindings
                        .retain(|existing, _| !same_name(existing, key));
                    if let Some(tab_id) = tab_id {
                        if !workspace.tabs.iter().any(|tab| tab.tab_id == *tab_id) {
                            return Err(("tab_not_found", tab_id.clone()));
                        }
                        workspace.bindings.insert(key.clone(), tab_id.clone());
                    }
                }
                WorkspaceGroupOp::Delete { .. } | WorkspaceGroupOp::Move { .. } => {
                    return Err((
                        "unsupported",
                        "herdr up never deletes or moves groups".into(),
                    ));
                }
            }
            Ok(())
        }

        fn fake_agent_start(
            &mut self,
            params: &AgentStartParams,
        ) -> Result<serde_json::Value, FakeError> {
            if let Some(remaining) = self.busy.get_mut(&params.pane_id) {
                if *remaining > 0 {
                    *remaining -= 1;
                    return Err((
                        "agent_pane_busy",
                        format!(
                            "agent target pane {} is not an available shell",
                            params.pane_id
                        ),
                    ));
                }
            }
            if let Some(index) = self
                .race_agents
                .iter()
                .position(|agent| agent.name == params.name)
            {
                let agent = self.race_agents.remove(index);
                self.agents.push(agent);
            }
            if let Some(holder) = self.agents.iter().find(|agent| agent.name == params.name) {
                return Err((
                    "agent_name_taken",
                    format!(
                        "agent name {} is already used; candidates: pane_id={}",
                        params.name, holder.pane_id
                    ),
                ));
            }
            let owner = self.workspaces.iter().find_map(|workspace| {
                workspace
                    .tabs
                    .iter()
                    .find(|tab| tab.pane_id == params.pane_id)
                    .map(|tab| (workspace.id.clone(), tab.tab_id.clone()))
            });
            let Some((workspace_id, tab_id)) = owner else {
                return Err(("agent_pane_not_found", params.pane_id.clone()));
            };
            self.agents.push(LiveAgent {
                name: params.name.clone(),
                workspace_id,
                tab_id,
                pane_id: params.pane_id.clone(),
            });
            Ok(json!({"type": "agent_started"}))
        }

        /// `workspace_group.*` by method name, everything else through the typed `Request`.
        fn answer(&mut self, request: &serde_json::Value) -> Result<serde_json::Value, FakeError> {
            match request["method"].as_str() {
                Some("workspace_group.list") if self.upstream => Err((
                    "invalid_request",
                    "unknown variant `workspace_group.list`".into(),
                )),
                Some("workspace_group.list") => Ok(self.groups_result()),
                Some("workspace_group.edit") => {
                    let ops: Vec<WorkspaceGroupOp> =
                        serde_json::from_value(request["params"]["ops"].clone())
                            .map_err(|err| ("invalid_request", err.to_string()))?;
                    for op in &ops {
                        self.apply(op)?;
                    }
                    Ok(self.groups_result())
                }
                _ => {
                    let request: Request = serde_json::from_value(request.clone())
                        .map_err(|err| ("invalid_request", err.to_string()))?;
                    self.answer_method(&request.method)
                }
            }
        }

        fn answer_method(&mut self, method: &Method) -> Result<serde_json::Value, FakeError> {
            match method {
                Method::WorkspaceList(_) => {
                    let workspaces: Vec<serde_json::Value> = self
                        .workspaces
                        .iter()
                        .enumerate()
                        .map(|(index, workspace)| {
                            json!({
                                "workspace_id": workspace.id,
                                "number": index + 1,
                                "label": workspace.label,
                                "focused": false,
                                "pane_count": workspace.tabs.len(),
                                "tab_count": workspace.tabs.len(),
                                "active_tab_id": workspace.tabs[0].tab_id,
                                "agent_status": "idle"
                            })
                        })
                        .collect();
                    Ok(json!({"type": "workspace_list", "workspaces": workspaces}))
                }
                Method::AgentList(_) => {
                    let agents: Vec<serde_json::Value> = self
                        .agents
                        .iter()
                        .map(|agent| {
                            json!({
                                "terminal_id": format!("term-{}", agent.pane_id),
                                "name": agent.name,
                                "agent": "claude",
                                "agent_status": "idle",
                                "workspace_id": agent.workspace_id,
                                "tab_id": agent.tab_id,
                                "pane_id": agent.pane_id,
                                "focused": false,
                                "revision": 0
                            })
                        })
                        .collect();
                    Ok(json!({"type": "agent_list", "agents": agents}))
                }
                Method::TabList(_) => {
                    let tabs: Vec<serde_json::Value> = self
                        .workspaces
                        .iter()
                        .flat_map(|workspace| {
                            workspace.tabs.iter().enumerate().map(move |(index, tab)| {
                                json!({
                                    "tab_id": tab.tab_id,
                                    "workspace_id": workspace.id,
                                    "number": index + 1,
                                    "label": tab.label,
                                    "focused": false,
                                    "pane_count": 1,
                                    "agent_status": "idle"
                                })
                            })
                        })
                        .collect();
                    Ok(json!({"type": "tab_list", "tabs": tabs}))
                }
                Method::WorkspaceCreate(params) => {
                    let id = self.add_workspace(
                        params.label.as_deref().unwrap_or("shell"),
                        params.cwd.as_deref().unwrap_or("/"),
                    );
                    let root = self.workspace(&id).tabs[0].clone();
                    Ok(json!({
                        "type": "workspace_created",
                        "workspace": {"workspace_id": id},
                        "tab": {"tab_id": root.tab_id},
                        "root_pane": {"pane_id": root.pane_id}
                    }))
                }
                Method::TabCreate(_) if self.fail_tab_create => {
                    Err(("tab_create_failed", "cwd unavailable".into()))
                }
                Method::TabCreate(params) => {
                    let workspace_id = params.workspace_id.clone().unwrap_or_default();
                    let tab =
                        self.add_tab(&workspace_id, params.label.as_deref().unwrap_or("shell"))?;
                    Ok(json!({
                        "type": "tab_created",
                        "tab": {"tab_id": tab.tab_id},
                        "root_pane": {"pane_id": tab.pane_id}
                    }))
                }
                Method::TabRename(params) => {
                    let tab = self
                        .workspaces
                        .iter_mut()
                        .flat_map(|workspace| workspace.tabs.iter_mut())
                        .find(|tab| tab.tab_id == params.tab_id)
                        .ok_or(("tab_not_found", params.tab_id.clone()))?;
                    tab.label = params.label.clone();
                    Ok(json!({"type": "ok"}))
                }
                Method::WorkspaceRename(params) => {
                    self.workspace_mut(&params.workspace_id)?.label = params.label.clone();
                    Ok(json!({"type": "ok"}))
                }
                Method::AgentStart(params) => self.fake_agent_start(params),
                Method::PaneSendInput(_) => Ok(json!({"type": "ok"})),
                other => Err((
                    "unsupported",
                    format!("the fake server does not answer {other:?}"),
                )),
            }
        }
    }

    impl UpIo for FakeServer {
        fn send(&mut self, request: &serde_json::Value) -> std::io::Result<serde_json::Value> {
            self.requests.push(request.clone());
            let id = request["id"].clone();
            Ok(match self.answer(request) {
                Ok(result) => json!({"id": id, "result": result}),
                Err((code, message)) => {
                    json!({"id": id, "error": {"code": code, "message": message}})
                }
            })
        }

        fn pause(&mut self, duration: Duration) {
            self.pauses.push(duration);
        }

        fn line(&mut self, to_stderr: bool, text: &str) {
            self.lines.push((to_stderr, text.to_string()));
        }
    }

    fn run(fake: &mut FakeServer, text: &str, selection: Selection) -> i32 {
        let file = parsed(text);
        execute(fake, &file, &selection, false, dirs_for(&file)).expect("execute")
    }

    fn lines(fake: &FakeServer) -> Vec<(bool, &str)> {
        fake.lines
            .iter()
            .map(|(to_stderr, line)| (*to_stderr, line.as_str()))
            .collect()
    }

    fn method_names(fake: &FakeServer) -> Vec<&str> {
        fake.requests
            .iter()
            .map(|request| request["method"].as_str().unwrap_or_default())
            .collect()
    }

    /// The requests that decode into this build's typed `Method`.
    fn methods(fake: &FakeServer) -> Vec<Method> {
        fake.requests
            .iter()
            .filter_map(|request| serde_json::from_value::<Request>(request.clone()).ok())
            .map(|request| request.method)
            .collect()
    }

    fn sends(fake: &FakeServer) -> Vec<PaneSendInputParams> {
        methods(fake)
            .into_iter()
            .filter_map(|method| match method {
                Method::PaneSendInput(params) => Some(params),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn plan_is_empty_after_apply() {
        let file = parsed(RUN);
        let mut fake = FakeServer::default();
        assert_eq!(run(&mut fake, RUN, Selection::All), EXIT_OK);
        assert_eq!(
            lines(&fake),
            vec![
                (false, "group Work: created"),
                (false, "project api: created w1"),
                (false, "  tab agent: created, agent api-agent started"),
                (false, "  tab dev: created, command sent"),
                (false, "project docs: created w2"),
            ]
        );
        assert_eq!(
            fake.groups,
            vec![group_info("g1", "Work", "blue", Some("Work"))]
        );
        let api = fake.workspace("w1");
        assert_eq!(
            (
                api.group_id.as_deref(),
                api.color.as_deref(),
                api.project.as_deref()
            ),
            (Some("g1"), Some("teal"), Some("api"))
        );
        assert_eq!(
            api.bindings,
            BTreeMap::from([
                ("agent".to_string(), "w1:t1".to_string()),
                ("dev".to_string(), "w1:t2".to_string()),
            ])
        );
        assert_eq!(
            api.tabs
                .iter()
                .map(|tab| tab.label.as_str())
                .collect::<Vec<_>>(),
            ["agent", "dev"]
        );
        assert_eq!(fake.agents, vec![live_agent("api-agent", "w1", 1)]);
        // `up` never changes focus.
        assert!(methods(&fake).iter().all(|method| match method {
            Method::WorkspaceCreate(params) => !params.focus,
            Method::TabCreate(params) => !params.focus,
            _ => true,
        }));

        let Ok(mut state) = read_state(&mut fake).expect("read state") else {
            panic!("the fake server answers every read");
        };
        state.project_dirs = dirs_for(&file);
        assert_eq!(
            plan(&file, &Selection::All, &state, false),
            vec![
                report(Level::Progress, "group Work: up to date (g1)"),
                report(Level::Progress, "project api: up to date (w1)"),
                report(Level::Progress, "project docs: up to date (w2)"),
            ]
        );

        let before = fake.requests.len();
        assert_eq!(run(&mut fake, RUN, Selection::All), EXIT_OK);
        let reads = [
            "workspace_group.list",
            "workspace.list",
            "agent.list",
            "tab.list",
        ];
        assert!(
            method_names(&fake)[before..]
                .iter()
                .all(|name| reads.contains(name)),
            "{:?}",
            &method_names(&fake)[before..]
        );
    }

    #[test]
    fn group_requests_follow_the_contract_wire_shapes() {
        // The index contract's JSON: the typed requests of `herdr up` keep the server's wire shapes.
        let mut fake = FakeServer::default();
        assert_eq!(
            run(&mut fake, RUN, Selection::Projects(vec!["api".into()])),
            EXIT_OK
        );
        let groups: Vec<&serde_json::Value> = fake
            .requests
            .iter()
            .filter(|request| {
                request["method"]
                    .as_str()
                    .is_some_and(|name| name.starts_with("workspace_group."))
            })
            .collect();
        assert_eq!(
            groups,
            [
                &json!({"id": "cli:up", "method": "workspace_group.list", "params": {}}),
                &json!({"id": "cli:up", "method": "workspace_group.edit", "params": {"ops": [
                    {"op": "create", "name": "Work", "color": "blue", "template": "Work", "workspace_ids": []}
                ]}}),
                &json!({"id": "cli:up", "method": "workspace_group.edit", "params": {"ops": [
                    {"op": "bind", "workspace_id": "w1", "project": "api"},
                    {"op": "place", "workspace_id": "w1", "group_id": "g1"},
                    {"op": "set_color", "workspace_id": "w1", "color": "teal"},
                    {"op": "bind_tab", "workspace_id": "w1", "key": "agent", "tab_id": "w1:t1"}
                ]}}),
                &json!({"id": "cli:up", "method": "workspace_group.edit", "params": {"ops": [
                    {"op": "bind_tab", "workspace_id": "w1", "key": "dev", "tab_id": "w1:t2"}
                ]}}),
            ]
        );

        // `update` and an unbinding `bind` (the D17 stale binding) on the wire.
        let mut stale = FakeServer::default();
        let old = stale.add_workspace("api", "/herdr-up-test/elsewhere");
        stale.workspace_mut(&old).expect("stale workspace").project = Some("api".into());
        stale.groups = vec![group_info("g7", "Job", "green", Some("Work"))];
        let file = parsed(RUN);
        assert_eq!(
            execute(&mut stale, &file, &Selection::All, true, dirs_for(&file)).expect("execute"),
            EXIT_OK
        );
        let edits: Vec<&serde_json::Value> = stale
            .requests
            .iter()
            .filter(|request| request["method"] == "workspace_group.edit")
            .map(|request| &request["params"]["ops"][0])
            .take(2)
            .collect();
        assert_eq!(
            edits,
            [
                &json!({"op": "update", "group_id": "g7", "name": "Work", "color": "blue"}),
                &json!({"op": "bind", "workspace_id": "w1"}),
            ]
        );
    }

    #[test]
    fn parse_groups_reads_the_server_workspace_groups_result() {
        // The server's own response type, so `herdr up` and the server share one wire form.
        let groups = vec![crate::api::schema::WorkspaceGroupInfo {
            group_id: "g1".into(),
            name: "Work".into(),
            color: "blue".into(),
            template: Some("Work".into()),
        }];
        let members = vec![crate::api::schema::WorkspaceGroupMemberInfo {
            workspace_id: "w1".into(),
            group_id: Some("g1".into()),
            color: None,
            project: Some("api".into()),
            cwd: "/src/api".into(),
            linked: false,
            tabs: vec![crate::api::schema::WorkspaceGroupTabBinding {
                key: "agent".into(),
                tab_id: "w1:t1".into(),
            }],
        }];
        let response = json!({"id": REQUEST_ID, "result": ResponseResult::WorkspaceGroups {
            groups: groups.clone(),
            workspaces: members.clone(),
            deleted_templates: vec!["Infra".into()],
        }});
        assert_eq!(
            parse_groups(&response, "workspace_group.list").expect("groups result"),
            (groups, members, vec!["Infra".to_string()])
        );

        let other = json!({"id": REQUEST_ID, "result": {"type": "ok"}});
        let err = parse_groups(&other, "workspace_group.edit").expect_err("not a groups result");
        assert_eq!(
            err.to_string(),
            "workspace_group.edit returned an unexpected result"
        );
    }

    #[test]
    fn plan_stale_binding_converges_in_one_run() {
        let file = parsed(RUN);
        let selection = Selection::Projects(vec!["api".into()]);
        let mut fake = FakeServer::default();
        let stale = fake.add_workspace("api", "/herdr-up-test/elsewhere");
        let record = fake.workspace_mut(&stale).expect("stale workspace");
        record.project = Some("api".into());
        record
            .bindings
            .insert("agent".into(), format!("{stale}:t1"));

        assert_eq!(run(&mut fake, RUN, selection.clone()), EXIT_OK);
        assert_eq!(
            lines(&fake),
            vec![
                (false, "group Work: created"),
                (
                    true,
                    "warning: api was bound to w1 (cwd /herdr-up-test/elsewhere); rebinding"
                ),
                (false, "project api: created w2"),
                (false, "  tab agent: created, agent api-agent started"),
                (false, "  tab dev: created, command sent"),
            ]
        );
        let old = fake.workspace("w1");
        assert_eq!((old.project.as_deref(), old.bindings.len()), (None, 0));
        assert_eq!(fake.workspace("w2").project.as_deref(), Some("api"));

        let Ok(mut state) = read_state(&mut fake).expect("read state") else {
            panic!("the fake server answers every read");
        };
        state.project_dirs = dirs_for(&file);
        assert_eq!(
            plan(&file, &selection, &state, false),
            vec![
                report(Level::Progress, "group Work: up to date (g1)"),
                report(Level::Progress, "project api: up to date (w2)"),
            ]
        );
    }

    #[test]
    fn commands_only_for_tabs_created_this_run_with_enter_key() {
        // An adopted workspace's existing "dev" tab is bound by label, never sent the command.
        let mut adopted = FakeServer::default();
        let existing_id = adopted.add_workspace("api", "/herdr-up-test/api");
        adopted.add_tab(&existing_id, "dev").expect("dev tab");
        assert_eq!(
            run(&mut adopted, RUN, Selection::Projects(vec!["api".into()])),
            EXIT_OK
        );
        assert!(sends(&adopted).is_empty(), "{:?}", sends(&adopted));
        assert!(lines(&adopted).contains(&(false, "  tab dev: bound w1:t2 by label")));

        // A dev tab created in this run gets the command once, with an explicit Enter key.
        let mut fresh = FakeServer::default();
        assert_eq!(run(&mut fresh, RUN, Selection::All), EXIT_OK);
        assert_eq!(
            sends(&fresh),
            vec![PaneSendInputParams {
                pane_id: "w1:p2".into(),
                text: "npm run dev".into(),
                keys: vec!["Enter".into()],
            }]
        );
        let before = fresh.requests.len();
        assert_eq!(run(&mut fresh, RUN, Selection::All), EXIT_OK);
        assert!(method_names(&fresh)[before..]
            .iter()
            .all(|name| *name != "pane.send_input"));
    }

    #[test]
    fn exit_code_3_when_a_project_is_skipped() {
        let file = parsed(RUN);
        let mut dirs = dirs_for(&file);
        if let Some(docs) = dirs.get_mut("docs") {
            docs.exists = false;
        }
        let mut fake = FakeServer::default();
        assert_eq!(
            execute(&mut fake, &file, &Selection::All, false, dirs).expect("execute"),
            EXIT_SKIPPED
        );
        assert!(lines(&fake).contains(&(
            true,
            "project docs: cwd /herdr-up-test/docs does not exist; skipped"
        )));
        assert!(lines(&fake).contains(&(false, "project api: created w1")));
        assert_eq!(
            method_names(&fake)
                .iter()
                .filter(|name| **name == "workspace.create")
                .count(),
            1
        );
    }

    #[test]
    fn upstream_server_without_group_list_exits_1() {
        let mut fake = FakeServer {
            upstream: true,
            ..FakeServer::default()
        };
        assert_eq!(run(&mut fake, RUN, Selection::All), EXIT_FAILED);
        assert_eq!(
            lines(&fake),
            vec![(
                true,
                "server does not support workspace groups (fork server required)"
            )]
        );
        assert_eq!(fake.requests.len(), 1);
    }

    #[test]
    fn api_error_stops_at_first_failed_request() {
        let mut fake = FakeServer {
            fail_tab_create: true,
            ..FakeServer::default()
        };
        assert_eq!(run(&mut fake, RUN, Selection::All), EXIT_FAILED);
        let (to_stderr, last) = fake.lines.last().cloned().expect("an error line");
        assert!(to_stderr && last.contains("tab_create_failed"), "{last}");
        assert_eq!(method_names(&fake).last(), Some(&"tab.create"));
        assert!(lines(&fake).contains(&(false, "  tab agent: created, agent api-agent started")));
        assert_eq!(
            method_names(&fake)
                .iter()
                .filter(|name| **name == "workspace.create")
                .count(),
            1
        );
    }

    fn agent_starts(fake: &FakeServer) -> Vec<AgentStartParams> {
        methods(fake)
            .into_iter()
            .filter_map(|method| match method {
                Method::AgentStart(params) => Some(params),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn agent_tabs_use_agent_start_with_stable_name_and_retry_busy() {
        let api = Selection::Projects(vec!["api".into()]);
        let mut fake = FakeServer::default();
        fake.busy.insert("w1:p1".into(), 3);
        assert_eq!(run(&mut fake, RUN, api.clone()), EXIT_OK);
        let starts = agent_starts(&fake);
        assert_eq!(starts.len(), 4);
        let expected = AgentStartParams {
            name: "api-agent".into(),
            kind: "claude".into(),
            pane_id: "w1:p1".into(),
            args: Vec::new(),
            timeout_ms: None,
        };
        assert!(
            starts.iter().all(|params| *params == expected),
            "{starts:?}"
        );
        assert_eq!(fake.pauses, vec![Duration::from_millis(250); 3]);
        assert!(lines(&fake).contains(&(false, "  tab agent: created, agent api-agent started")));

        // A pane that stays busy: 60 retries (15 s at 250 ms), then the manual command, exit 3.
        let text = RUN.replace(
            "agent = \"claude\"",
            "agent = \"claude\"\n  args = [\"--model\", \"opus\"]",
        );
        let mut stuck = FakeServer::default();
        stuck.busy.insert("w1:p1".into(), usize::MAX);
        assert_eq!(run(&mut stuck, &text, api), EXIT_SKIPPED);
        assert_eq!(agent_starts(&stuck).len(), 61);
        assert_eq!(stuck.pauses, vec![Duration::from_millis(250); 60]);
        assert!(lines(&stuck).contains(&(
            true,
            "tab agent: pane w1:p1 is still busy after 15 s; agent api-agent not started"
        )));
        assert!(lines(&stuck).contains(&(
            true,
            "run: herdr agent start api-agent --kind claude --pane w1:p1 -- --model opus"
        )));
        // The run continued with the next tab.
        assert!(lines(&stuck).contains(&(false, "  tab dev: created, command sent")));
    }

    #[test]
    fn agent_name_taken_elsewhere_is_reported_not_retried() {
        let api = Selection::Projects(vec!["api".into()]);
        let taken = "tab agent: agent name api-agent is already used by w1:p1 in w1; set agent_name in projects.toml";

        // Live elsewhere before the run: no agent.start at all.
        let mut fake = FakeServer::default();
        let other = fake.add_workspace("other", "/herdr-up-test/other");
        fake.agents.push(live_agent("api-agent", &other, 1));
        assert_eq!(run(&mut fake, RUN, api.clone()), EXIT_SKIPPED);
        assert!(agent_starts(&fake).is_empty());
        assert!(lines(&fake).contains(&(true, taken)));
        assert!(lines(&fake).contains(&(false, "  tab dev: created, command sent")));

        // Taken between planning and launch: one agent.start, no retry, holder looked up.
        let mut raced = FakeServer::default();
        let other = raced.add_workspace("other", "/herdr-up-test/other");
        raced.race_agents.push(live_agent("api-agent", &other, 1));
        assert_eq!(run(&mut raced, RUN, api), EXIT_SKIPPED);
        assert_eq!(agent_starts(&raced).len(), 1);
        assert!(raced.pauses.is_empty());
        assert!(lines(&raced).contains(&(true, taken)));
        assert!(lines(&raced).contains(&(false, "  tab dev: created, command sent")));
    }

    #[test]
    fn help_is_handled_inside_the_up_arm() {
        // `spec::print_requested_help` only knows clap root commands, so `herdr up --help`
        // reaches `maybe_run`'s `up` arm, where `run_up` answers it with no server or file.
        let missing = std::env::temp_dir()
            .join("herdr-up-help-no-such-dir")
            .join(PROJECTS_FILE_NAME);
        for flag in ["--help", "-h"] {
            let argv = args(&["herdr", "up", flag]);
            assert!(!super::super::spec::print_requested_help(&argv).expect("root help check"));
            let mut fake = FakeServer::default();
            assert_eq!(
                run_up(&argv[2..], &missing, &mut fake).expect("help"),
                EXIT_OK
            );
            assert_eq!(lines(&fake), vec![(false, USAGE)]);
            assert!(fake.requests.is_empty());
        }
        let mut fake = FakeServer::default();
        assert_eq!(
            run_up(&args(&["api", "--all"]), &missing, &mut fake).expect("usage"),
            EXIT_USAGE
        );
        assert_eq!(
            lines(&fake),
            vec![
                (
                    true,
                    "choose exactly one of <project>..., --group <group> or --all"
                ),
                (true, USAGE),
            ]
        );
        let mut fake = FakeServer::default();
        assert_eq!(
            run_up(&args(&["--all"]), &missing, &mut fake).expect("file error"),
            EXIT_USAGE
        );
        assert!(
            fake.lines[0].1.starts_with("projects file not found: "),
            "{:?}",
            fake.lines
        );
        assert!(fake.requests.is_empty());
    }

    #[test]
    fn load_inputs_reports_file_errors_before_any_request() {
        let root = scratch_dir("load");
        let path = root.join(PROJECTS_FILE_NAME);
        let missing = load_inputs(&path, &Selection::All).expect_err("missing file");
        assert!(
            missing.starts_with("projects file not found: "),
            "{missing}"
        );

        std::fs::write(
            &path,
            "[[project]]\nname = \"api\"\ncwd = \"/a\"\nbogus = 1\n",
        )
        .expect("write projects file");
        let unknown = load_inputs(&path, &Selection::All).expect_err("unknown key");
        assert!(unknown.contains("bogus"), "{unknown}");

        let cwd = root.join("api");
        std::fs::create_dir_all(&cwd).expect("project dir");
        std::fs::write(
            &path,
            format!("[[project]]\nname = \"api\"\ncwd = '{}'\n", cwd.display()),
        )
        .expect("write projects file");
        assert_eq!(
            load_inputs(&path, &Selection::Projects(vec!["web".into()])).map(|_| ()),
            Err("unknown project web in projects.toml".to_string())
        );
        let (file, dirs) =
            load_inputs(&path, &Selection::Projects(vec!["API".into()])).expect("valid file");
        assert_eq!(file.project[0].name, "api");
        assert!(dirs["api"].exists);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn run_up_is_up_to_date_on_the_second_run_with_real_directories() {
        // Real directories exercise the canonical keys on both sides (on Windows the
        // project path may be an 8.3 short path and canonicalizes to a \\?\ path).
        let root = scratch_dir("e2e");
        let api = root.join("api");
        std::fs::create_dir_all(&api).expect("project dir");
        let path = root.join(PROJECTS_FILE_NAME);
        std::fs::write(
            &path,
            format!(
                "[[project]]\nname = \"api\"\ncwd = '{}'\n[[project.tab]]\nlabel = \"dev\"\ncommand = \"npm run dev\"\n",
                api.display()
            ),
        )
        .expect("write projects file");
        let mut fake = FakeServer::default();
        assert_eq!(
            run_up(&args(&["api"]), &path, &mut fake).expect("first run"),
            EXIT_OK
        );
        assert_eq!(fake.workspace("w1").cwd, api.to_string_lossy());
        assert_eq!(
            run_up(&args(&["api"]), &path, &mut fake).expect("second run"),
            EXIT_OK
        );
        assert_eq!(
            fake.lines.last().map(|(_, line)| line.as_str()),
            Some("project api: up to date (w1)")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cwd_comparison_ignores_case_and_windows_verbatim_prefix() {
        assert_eq!(path_key(r"\\?\C:\Src\Api"), r"c:\src\api");
        assert_eq!(path_key(r"\\?\UNC\Host\Share\Api"), r"\\host\share\api");
        assert_eq!(path_key("/Src/Api"), "/src/api");
        assert!(same_cwd("/SRC/api/", "/src/api"));
        assert!(cwd_within("/src/api/server", "/SRC/API"));
        assert!(!cwd_within("/src/apix", "/src/api"));
        if cfg!(windows) {
            // A member cwd that no longer exists keeps its plain spelling, while the
            // project key is canonical (verbatim); the binding must not look stale.
            assert!(same_cwd(r"C:\src\api", r"\\?\C:\SRC\Api"));
            assert!(cwd_within(r"C:\src\api\server", r"\\?\C:\src\api"));
            assert!(cwd_within(r"\\?\UNC\host\share\api\x", r"\\host\share\api"));
            assert!(!cwd_within(r"C:\src\apix", r"\\?\C:\src\api"));
        }
    }
}
