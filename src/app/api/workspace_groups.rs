//! App integration for workspace groups: sidecar load and save, record pruning
//! (spec 5.5), the once-per-loop reconcile (spec 6.2) and the client projection.

use std::collections::HashSet;

use crate::api::schema::{
    EventData, EventEnvelope, EventKind, ResponseResult, WorkspaceGroupInfo,
    WorkspaceGroupMemberInfo, WorkspaceGroupOp, WorkspaceGroupTabBinding,
};
use crate::app::workspace_groups::{self as model, GroupEditError, WorkspaceGroups, WorkspaceMeta};
use crate::app::App;
use crate::persist::workspace_groups::{self as sidecar, SidecarLoad};

impl App {
    /// Last step of `try_new`: read the sidecar before any API request can edit it.
    pub(crate) fn load_workspace_groups(&mut self) {
        if !self.policy.persist_session {
            return;
        }
        let path = sidecar::sidecar_path();
        let mut groups = match sidecar::load_sidecar(&path, unix_now().as_secs()) {
            SidecarLoad::Missing => WorkspaceGroups::default(),
            SidecarLoad::Loaded(file) => WorkspaceGroups::from_file(file),
            SidecarLoad::SetAside { moved_to, reason } => {
                tracing::warn!(
                    event = "workspace_groups.load", subsystem = "persist", outcome = "set_aside",
                    path = %path.display(), moved_to = %moved_to.display(), reason = %reason,
                    "workspace groups file was set aside; starting without groups"
                );
                WorkspaceGroups::default()
            }
            SidecarLoad::Blocked { reason } => {
                tracing::warn!(
                    event = "workspace_groups.load", subsystem = "persist", outcome = "blocked",
                    path = %path.display(), reason = %reason,
                    "workspace groups file could not be read or set aside; groups are not saved until restart"
                );
                WorkspaceGroups {
                    persist_blocked: true,
                    ..WorkspaceGroups::default()
                }
            }
        };

        // Pruning rule 2: workspaces exist here only after a successful session restore.
        let mut changed = false;
        if !self.state.workspaces.is_empty() {
            let live = self.workspace_group_metas();
            let restored: HashSet<&str> = live.iter().map(|meta| meta.id.as_str()).collect();
            let records = groups.workspaces.len();
            groups
                .workspaces
                .retain(|workspace_id, _| restored.contains(workspace_id.as_str()));
            let pruned = groups.workspaces.len() != records;
            let cleaned = model::cleanup_records(&mut groups, &live);
            changed = pruned || cleaned;
        }
        self.state.workspace_groups = groups;
        if changed {
            self.save_workspace_groups();
        }
    }

    /// First step of `sync_session_save_schedule`, once per server loop iteration.
    pub(crate) fn reconcile_workspace_groups(&mut self) -> bool {
        let groups = &self.state.workspace_groups;
        let unchanged = groups.last_order.as_ref().is_some_and(|last| {
            last.iter().map(String::as_str).eq(self
                .state
                .workspaces
                .iter()
                .map(|ws| ws.id.as_str()))
        });
        if unchanged && (groups.pending_prune.is_empty() || !self.session_save_settled()) {
            return false;
        }

        let live = self.workspace_group_metas();
        let settled = self.session_save_settled();
        let outcome = model::reconcile(&mut self.state.workspace_groups, &live, settled);
        let order_changed = self.state.move_workspace_block(&outcome.order, None);
        if outcome.records_changed {
            self.save_workspace_groups();
        }
        if order_changed {
            self.emit_workspace_group_reorder();
        }
        self.record_workspace_group_order();

        let changed = outcome.records_changed || order_changed;
        if changed {
            // The previous iteration already rendered the pre-normalization projection.
            self.render_dirty.request_generic();
            self.render_notify.notify_one();
        }
        changed
    }

    pub(crate) fn client_shell_workspace_groups(
        &self,
    ) -> crate::protocol::ClientShellWorkspaceGroups {
        let groups = &self.state.workspace_groups;
        if groups.groups.is_empty()
            && groups
                .workspaces
                .values()
                .all(|record| record.color.is_none())
        {
            return crate::protocol::ClientShellWorkspaceGroups::default();
        }
        // Runs per client snapshot: the projection reads ids only, so skip per-tab ids.
        let live: Vec<_> = self
            .state
            .workspaces
            .iter()
            .map(WorkspaceMeta::without_tabs)
            .collect();
        model::project_client_shell(groups, &live)
    }

    pub(super) fn handle_workspace_group_list(&self, id: String) -> String {
        super::responses::encode_success(id, self.workspace_groups_result())
    }

    pub(super) fn handle_workspace_group_edit(
        &mut self,
        id: String,
        params: crate::api::schema::WorkspaceGroupEditParams,
    ) -> String {
        // Fold in closes and new workspaces from earlier in this iteration first,
        // so setting `last_order` below cannot hide them from pruning or inference.
        self.reconcile_workspace_groups();
        let ops = match self.canonical_workspace_group_ops(params.ops) {
            Ok(ops) => ops,
            Err(err) => return encode_group_error(id, &err),
        };
        let live = self.workspace_group_metas();
        let (groups, order) =
            match model::plan_edit(&self.state.workspace_groups, &live, &ops, unix_now_ms()) {
                Ok(planned) => planned,
                Err(err) => return encode_group_error(id, &err),
            };

        let records_changed = groups.to_file() != self.state.workspace_groups.to_file();
        self.state.workspace_groups = groups;
        let order_changed = self.state.move_workspace_block(&order, None);
        if records_changed {
            self.save_workspace_groups();
        }
        if order_changed {
            self.emit_workspace_group_reorder();
        }
        self.record_workspace_group_order();
        super::responses::encode_success(id, self.workspace_groups_result())
    }

    fn canonical_workspace_group_ops(
        &self,
        ops: Vec<WorkspaceGroupOp>,
    ) -> Result<Vec<WorkspaceGroupOp>, GroupEditError> {
        ops.into_iter()
            .map(|op| {
                Ok(match op {
                    WorkspaceGroupOp::Create {
                        name,
                        color,
                        template,
                        workspace_ids,
                        before_group_id,
                    } => WorkspaceGroupOp::Create {
                        name,
                        color,
                        template,
                        workspace_ids: workspace_ids
                            .iter()
                            .map(|workspace_id| self.canonical_workspace_id(workspace_id))
                            .collect::<Result<_, _>>()?,
                        before_group_id,
                    },
                    WorkspaceGroupOp::Place {
                        workspace_id,
                        group_id,
                        before_workspace_id,
                    } => WorkspaceGroupOp::Place {
                        workspace_id: self.canonical_workspace_id(&workspace_id)?,
                        group_id,
                        before_workspace_id: before_workspace_id
                            .and_then(|before| self.canonical_workspace_id(&before).ok()),
                    },
                    WorkspaceGroupOp::SetColor {
                        workspace_id,
                        color,
                    } => WorkspaceGroupOp::SetColor {
                        workspace_id: self.canonical_workspace_id(&workspace_id)?,
                        color,
                    },
                    WorkspaceGroupOp::Bind {
                        workspace_id,
                        project,
                    } => WorkspaceGroupOp::Bind {
                        workspace_id: self.canonical_workspace_id(&workspace_id)?,
                        project,
                    },
                    WorkspaceGroupOp::BindTab {
                        workspace_id,
                        key,
                        tab_id,
                    } => WorkspaceGroupOp::BindTab {
                        workspace_id: self.canonical_workspace_id(&workspace_id)?,
                        key,
                        tab_id: tab_id
                            .map(|tab_id| self.canonical_tab_id(&tab_id))
                            .transpose()?,
                    },
                    op @ (WorkspaceGroupOp::Update { .. }
                    | WorkspaceGroupOp::Delete { .. }
                    | WorkspaceGroupOp::Move { .. }) => op,
                })
            })
            .collect()
    }

    fn canonical_workspace_id(&self, workspace_id: &str) -> Result<String, GroupEditError> {
        self.parse_workspace_id(workspace_id)
            .map(|index| self.public_workspace_id(index))
            .ok_or_else(|| GroupEditError::WorkspaceNotFound(workspace_id.to_string()))
    }

    fn canonical_tab_id(&self, tab_id: &str) -> Result<String, GroupEditError> {
        self.parse_tab_id(tab_id)
            .and_then(|(ws_idx, tab_idx)| self.public_tab_id(ws_idx, tab_idx))
            .ok_or_else(|| GroupEditError::tab_not_found(tab_id))
    }

    fn workspace_groups_result(&self) -> ResponseResult {
        let groups = &self.state.workspace_groups;
        let workspaces = self
            .state
            .workspaces
            .iter()
            .map(|workspace| {
                let record = groups.workspaces.get(&workspace.id);
                WorkspaceGroupMemberInfo {
                    workspace_id: workspace.id.clone(),
                    group_id: groups
                        .effective_group(&workspace.id)
                        .map(|group| group.id.clone()),
                    color: record.and_then(|record| record.color.clone()),
                    project: record.and_then(|record| record.project.clone()),
                    cwd: workspace
                        .resolved_identity_cwd_from(&self.state.terminals, &self.terminal_runtimes)
                        .map(|cwd| cwd.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    linked: workspace
                        .worktree_space()
                        .is_some_and(|space| space.is_linked_worktree),
                    tabs: record
                        .into_iter()
                        .flat_map(|record| &record.tabs)
                        .filter(|(_, number)| {
                            workspace.tabs.iter().any(|tab| tab.number == **number)
                        })
                        .map(|(key, number)| WorkspaceGroupTabBinding {
                            key: key.clone(),
                            tab_id: crate::workspace::public_tab_id_for_number(
                                &workspace.id,
                                *number,
                            ),
                        })
                        .collect(),
                }
            })
            .collect();
        ResponseResult::WorkspaceGroups {
            groups: groups
                .groups
                .iter()
                .map(|group| WorkspaceGroupInfo {
                    group_id: group.id.clone(),
                    name: group.name.clone(),
                    color: group.color.clone(),
                    template: group.template.clone(),
                })
                .collect(),
            workspaces,
            deleted_templates: groups.deleted_templates.clone(),
        }
    }

    /// True once session.json no longer depends on an unsaved or in-flight write.
    fn session_save_settled(&self) -> bool {
        !self.state.session_dirty
            && !self.pane_exit_checkpoint_pending
            && self.session_save_deadline.is_none()
            && self
                .session_save_thread
                .as_ref()
                .is_none_or(std::thread::JoinHandle::is_finished)
    }

    fn workspace_group_metas(&self) -> Vec<WorkspaceMeta> {
        self.state
            .workspaces
            .iter()
            .map(WorkspaceMeta::from_workspace)
            .collect()
    }

    fn save_workspace_groups(&self) {
        let groups = &self.state.workspace_groups;
        if !self.policy.persist_session || groups.persist_blocked {
            return;
        }
        let path = sidecar::sidecar_path();
        if let Err(err) = sidecar::save_sidecar(&path, &groups.to_file()) {
            tracing::warn!(
                event = "workspace_groups.save", subsystem = "persist", outcome = "error",
                path = %path.display(), err = %err, "failed to save workspace groups"
            );
        }
    }

    /// The live order the next reconcile compares against; taken from state so a
    /// refused reorder cannot make every later iteration reconcile again.
    fn record_workspace_group_order(&mut self) {
        self.state.workspace_groups.last_order = Some(
            self.state
                .workspaces
                .iter()
                .map(|workspace| workspace.id.clone())
                .collect(),
        );
    }

    fn emit_workspace_group_reorder(&mut self) {
        let workspace_ids = self
            .state
            .workspaces
            .iter()
            .map(|workspace| workspace.id.clone())
            .collect();
        let workspaces = (0..self.state.workspaces.len())
            .map(|index| self.workspace_info(index))
            .collect();
        self.emit_event(EventEnvelope {
            event: EventKind::WorkspaceReordered,
            data: EventData::WorkspaceReordered {
                workspace_ids,
                before_workspace_id: None,
                workspaces,
            },
        });
    }
}

fn unix_now() -> std::time::Duration {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
}

/// Group ids come from this clock; a clock error yields 0 and ids still stay unique.
fn unix_now_ms() -> u64 {
    u64::try_from(unix_now().as_millis()).unwrap_or(u64::MAX)
}

fn encode_group_error(id: String, error: &GroupEditError) -> String {
    super::responses::encode_error(id, error.code(), error.message())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Instant;

    use super::*;
    use crate::app::workspace_groups::plan_edit;
    use crate::config::Config;
    use crate::events::AppEvent;
    use crate::persist::workspace_groups::WorkspaceGroupsFile;
    use crate::workspace::Workspace;

    const NOW_MS: u64 = 0x19a3f2c1b7;

    /// Points the config dir at a fresh temp dir for one test, under the shared env lock.
    struct TestConfigHome {
        path: PathBuf,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl TestConfigHome {
        fn new(name: &str) -> Self {
            let guard = crate::config::test_config_env_lock()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "herdr-workspace-groups-{name}-{}-{stamp}",
                std::process::id()
            ));
            std::env::set_var("XDG_CONFIG_HOME", &path);
            std::env::remove_var(crate::session::SESSION_ENV_VAR);
            Self {
                path,
                _guard: guard,
            }
        }
    }

    impl Drop for TestConfigHome {
        fn drop(&mut self) {
            std::env::remove_var("XDG_CONFIG_HOME");
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn test_app() -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state.default_shell = crate::app::exiting_test_command().into();
        app
    }

    /// A TEST-policy app that persists like production; `load_workspace_groups`
    /// then runs as the last step of `try_new` would.
    fn persistent_app(workspaces: Vec<Workspace>) -> App {
        let mut app = test_app();
        app.policy.persist_session = true;
        set_workspaces(&mut app, workspaces);
        app.load_workspace_groups();
        app.state.assert_invariants_for_test();
        app
    }

    /// Restarts like production: `App::try_new` reads session.json (`persist::load`,
    /// `parse_snapshot`), restores it with real panes running a short-lived command
    /// (`persist::restore`), then loads the sidecar. Needs a tokio runtime.
    fn restart_app() -> App {
        let mut config = Config::default();
        config.terminal.default_shell = crate::app::exiting_test_command().into();
        config.terminal.shell_mode = crate::config::ShellModeConfig::NonLogin;
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let app = App::try_new(
            &config,
            crate::app::AppPolicy {
                restore_session: true,
                persist_session: true,
                persist_plugin_registry: false,
                background_updates: false,
            },
            None,
            api_rx,
            crate::api::EventHub::default(),
        )
        .unwrap();
        app.state.assert_invariants_for_test();
        app
    }

    fn shut_down(mut app: App) {
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    fn session_path() -> PathBuf {
        crate::session::data_dir().join("session.json")
    }

    fn session_json(app: &App) -> String {
        let snapshot = crate::persist::capture(
            &app.state.workspaces,
            &app.state.terminals,
            &app.terminal_runtimes,
            app.state.active,
            app.state.selected,
        );
        serde_json::to_string(&snapshot).unwrap()
    }

    /// One server loop iteration's reconcile and save scheduling.
    fn sync(app: &mut App) {
        app.sync_session_save_schedule();
        app.state.assert_invariants_for_test();
    }

    fn set_workspaces(app: &mut App, workspaces: Vec<Workspace>) {
        app.state.active = (!workspaces.is_empty()).then_some(0);
        app.state.selected = 0;
        app.state.workspaces = workspaces;
        app.state.ensure_test_terminals();
    }

    fn workspaces(names: &[&str]) -> Vec<Workspace> {
        names.iter().map(|name| Workspace::test_new(name)).collect()
    }

    fn with_id(name: &str, id: &str) -> Workspace {
        let mut workspace = Workspace::test_new(name);
        workspace.id = id.into();
        workspace
    }

    fn ids(app: &App) -> Vec<String> {
        app.state
            .workspaces
            .iter()
            .map(|ws| ws.id.clone())
            .collect()
    }

    fn id_of(app: &App, name: &str) -> String {
        app.state
            .workspaces
            .iter()
            .find(|ws| ws.custom_name.as_deref() == Some(name))
            .unwrap()
            .id
            .clone()
    }

    fn create(name: &str, workspace_ids: &[String]) -> WorkspaceGroupOp {
        WorkspaceGroupOp::Create {
            name: name.into(),
            color: None,
            template: None,
            workspace_ids: workspace_ids.to_vec(),
            before_group_id: None,
        }
    }

    /// Applies ops the way the edit handler commits them.
    fn edit(app: &mut App, ops: &[WorkspaceGroupOp]) {
        let live = app.workspace_group_metas();
        let (groups, order) = plan_edit(&app.state.workspace_groups, &live, ops, NOW_MS).unwrap();
        app.state.workspace_groups = groups;
        app.state.move_workspace_block(&order, None);
        app.save_workspace_groups();
        app.state.workspace_groups.last_order = Some(ids(app));
        app.state.assert_invariants_for_test();
    }

    fn saved() -> WorkspaceGroupsFile {
        match sidecar::load_sidecar(&sidecar::sidecar_path(), 0) {
            SidecarLoad::Loaded(file) => file,
            other => panic!("expected a saved sidecar, got {other:?}"),
        }
    }

    fn group_of(app: &App, workspace_id: &str) -> Option<String> {
        app.state
            .workspace_groups
            .effective_group(workspace_id)
            .map(|group| group.name.clone())
    }

    fn finish_session_save(app: &mut App) {
        app.start_background_session_save();
        if let Some(thread) = app.session_save_thread.take() {
            thread.join().unwrap();
        }
    }

    fn reordered_events(app: &App) -> Vec<Vec<String>> {
        app.event_hub
            .events_after(0)
            .into_iter()
            .filter_map(|(_, event)| match event.data {
                EventData::WorkspaceReordered {
                    workspace_ids,
                    before_workspace_id,
                    workspaces,
                } => {
                    assert_eq!(before_workspace_id, None);
                    assert_eq!(workspaces.len(), workspace_ids.len());
                    Some(workspace_ids)
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn no_disk_access_under_test_policy() {
        let _home = TestConfigHome::new("test-policy");
        let path = sidecar::sidecar_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ corrupt").unwrap();

        let mut app = test_app();
        set_workspaces(&mut app, workspaces(&["a", "b"]));
        app.load_workspace_groups();
        assert_eq!(app.state.workspace_groups, WorkspaceGroups::default());

        let a = id_of(&app, "a");
        edit(&mut app, &[create("Work", std::slice::from_ref(&a))]);
        app.state.close_workspaces(vec![0]);
        sync(&mut app);
        sync(&mut app);

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ corrupt");
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(
            names,
            [std::ffi::OsString::from(sidecar::SIDECAR_FILE_NAME)]
        );
    }

    #[tokio::test]
    async fn startup_prunes_absent_ids_only_after_successful_restore() {
        let _home = TestConfigHome::new("startup-prune");
        let mut app = persistent_app(workspaces(&["a", "b"]));
        let (a, b) = (id_of(&app, "a"), id_of(&app, "b"));
        edit(&mut app, &[create("Work", &[a.clone(), b.clone()])]);
        // b closes and the process dies before any reconcile: only session.json forgets b.
        let b_index = ids(&app).iter().position(|id| *id == b).unwrap();
        app.state.close_workspaces(vec![b_index]);
        app.state.assert_invariants_for_test();
        let session_without_b = session_json(&app);
        drop(app);
        let path = sidecar::sidecar_path();
        let before = std::fs::read_to_string(&path).unwrap();

        // No session.json yet: nothing restores, so nothing is pruned.
        let empty = restart_app();
        assert!(empty.state.workspaces.is_empty());
        assert_eq!(empty.state.workspace_groups.workspaces.len(), 2);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        drop(empty);

        std::fs::write(session_path(), session_without_b).unwrap();
        let restarted = restart_app();
        assert_eq!(ids(&restarted), std::slice::from_ref(&a));
        assert_eq!(group_of(&restarted, &a).as_deref(), Some("Work"));
        assert!(!restarted.state.workspace_groups.workspaces.contains_key(&b));
        let file = saved();
        assert_eq!(file.groups.len(), 1);
        assert_eq!(file.workspaces.keys().collect::<Vec<_>>(), [&a]);
        shut_down(restarted);
    }

    #[tokio::test]
    async fn failed_or_newer_session_load_keeps_records() {
        for name in ["missing", "corrupt", "newer"] {
            let _home = TestConfigHome::new(name);
            let mut app = persistent_app(vec![with_id("a", "w1"), with_id("b", "w2")]);
            edit(
                &mut app,
                &[create("Work", &["w1".to_string(), "w2".to_string()])],
            );
            // A session listing only w1: restoring it would prune the record of w2.
            app.state.close_workspaces(vec![1]);
            let only_w1 = session_json(&app);
            drop(app);
            let path = sidecar::sidecar_path();
            let before = std::fs::read_to_string(&path).unwrap();
            let mut newer: serde_json::Value = serde_json::from_str(&only_w1).unwrap();
            newer["version"] = 99.into();
            match name {
                "corrupt" => std::fs::write(session_path(), "{ not a session").unwrap(),
                "newer" => std::fs::write(session_path(), newer.to_string()).unwrap(),
                _ => {}
            }

            let app = restart_app();
            assert!(app.state.workspaces.is_empty(), "{name}");
            assert_eq!(app.state.workspace_groups.workspaces.len(), 2, "{name}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "{name}");
            drop(app);

            if name == "newer" {
                // Control: the same session at the supported version restores w1 and prunes w2.
                std::fs::write(session_path(), &only_w1).unwrap();
                let app = restart_app();
                assert_eq!(ids(&app), ["w1"]);
                assert!(!app.state.workspace_groups.workspaces.contains_key("w2"));
                shut_down(app);
            }
        }
    }

    #[tokio::test]
    async fn records_survive_failed_load_until_first_successful_restore() {
        let _home = TestConfigHome::new("survive-failed-load");
        let mut app = persistent_app(vec![with_id("a", "w1"), with_id("b", "w2")]);
        edit(
            &mut app,
            &[create("Work", &["w1".to_string(), "w2".to_string()])],
        );
        drop(app);

        // A failed load restores nothing; a new workspace reusing w1 adopts its record.
        let mut app = restart_app();
        assert!(app.state.workspaces.is_empty());
        set_workspaces(&mut app, vec![with_id("fresh", "w1")]);
        sync(&mut app);
        assert_eq!(group_of(&app, "w1").as_deref(), Some("Work"));
        assert!(app.state.workspace_groups.workspaces.contains_key("w2"));

        // Closing it prunes the adopted record once session.json agrees.
        app.state.workspaces.push(with_id("other", "w3"));
        app.state.ensure_test_terminals();
        sync(&mut app);
        app.state.close_workspaces(vec![0]);
        sync(&mut app);
        finish_session_save(&mut app);
        sync(&mut app);
        assert!(!saved().workspaces.contains_key("w1"));
        assert!(saved().workspaces.contains_key("w2"));
        drop(app);

        let restarted = restart_app();
        assert_eq!(ids(&restarted), ["w3"]);
        assert!(restarted.state.workspace_groups.workspaces.is_empty());
        assert!(saved().workspaces.is_empty());
        assert_eq!(saved().groups.len(), 1, "groups survive pruning");
        shut_down(restarted);
    }

    #[tokio::test]
    async fn membership_survives_restart_when_root_pane_cwd_changed() {
        let home = TestConfigHome::new("cwd-changed");
        let mut app = persistent_app(workspaces(&["a", "b", "c"]));
        let (a, b, c) = (id_of(&app, "a"), id_of(&app, "b"), id_of(&app, "c"));
        edit(
            &mut app,
            &[
                create("Work", &[a.clone(), c.clone()]),
                WorkspaceGroupOp::SetColor {
                    workspace_id: b.clone(),
                    color: Some("teal".into()),
                },
            ],
        );
        let moved_cwd = home.path.join("elsewhere");
        std::fs::create_dir_all(&moved_cwd).unwrap();
        let root = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0]
            .pane_state(root)
            .unwrap()
            .attached_terminal_id
            .clone();
        app.state.terminals.get_mut(&terminal_id).unwrap().cwd = moved_cwd.clone();
        app.save_session_now();
        drop(app);

        let snapshot = crate::persist::load().expect("session saved");
        assert_eq!(
            snapshot.workspaces[0].tabs[0]
                .panes
                .values()
                .next()
                .unwrap()
                .cwd,
            moved_cwd
        );
        let mut app = restart_app();
        assert_eq!(ids(&app), [a.clone(), c.clone(), b.clone()]);
        sync(&mut app);

        assert_eq!(ids(&app), [a.clone(), c.clone(), b.clone()]);
        assert_eq!(group_of(&app, &a).as_deref(), Some("Work"));
        assert_eq!(group_of(&app, &c).as_deref(), Some("Work"));
        assert_eq!(app.state.workspace_groups.effective_color(&b), Some("teal"));
        shut_down(app);
    }

    #[tokio::test]
    async fn membership_survives_restart_after_first_tab_moved_or_closed() {
        let _home = TestConfigHome::new("first-tab-moved");
        let mut first = Workspace::test_new("a");
        first.test_add_tab(Some("two"));
        first.test_add_tab(Some("three"));
        let mut app = persistent_app(vec![first, Workspace::test_new("b")]);
        let a = id_of(&app, "a");
        let bind_tab = |key: &str, number: usize| WorkspaceGroupOp::BindTab {
            workspace_id: a.clone(),
            key: key.into(),
            tab_id: Some(crate::workspace::public_tab_id_for_number(&a, number)),
        };
        edit(
            &mut app,
            &[
                create("Work", std::slice::from_ref(&a)),
                WorkspaceGroupOp::Bind {
                    workspace_id: a.clone(),
                    project: Some("api".into()),
                },
                bind_tab("one", 1),
                bind_tab("two", 2),
                bind_tab("three", 3),
            ],
        );

        let workspace = &mut app.state.workspaces[0];
        assert!(workspace.close_tab(0));
        assert!(workspace.move_tab(1, 0));
        app.state.assert_invariants_for_test();
        app.save_session_now();
        drop(app);

        let snapshot = crate::persist::load().expect("session saved");
        assert_eq!(snapshot.workspaces[0].public_tab_numbers, [3, 2]);
        let app = restart_app();
        let numbers: Vec<_> = app.state.workspaces[0]
            .tabs
            .iter()
            .map(|tab| tab.number)
            .collect();
        assert_eq!(numbers, [3, 2], "restore keeps public tab numbers");

        assert_eq!(group_of(&app, &a).as_deref(), Some("Work"));
        let record = &app.state.workspace_groups.workspaces[&a];
        assert_eq!(record.project.as_deref(), Some("api"));
        assert_eq!(
            record.tabs,
            std::collections::BTreeMap::from([("three".into(), 3), ("two".into(), 2)])
        );
        assert_eq!(saved().workspaces[&a].tabs.len(), 2);
        shut_down(app);
    }

    #[test]
    fn in_process_close_prunes_record_after_session_save() {
        let _home = TestConfigHome::new("in-process-close");
        let mut app = persistent_app(workspaces(&["a", "b"]));
        let (a, b) = (id_of(&app, "a"), id_of(&app, "b"));
        edit(&mut app, &[create("Work", &[a.clone(), b.clone()])]);
        sync(&mut app);

        app.state.close_workspaces(vec![1]);
        sync(&mut app);
        assert!(
            saved().workspaces.contains_key(&b),
            "session save still pending"
        );
        assert_eq!(
            app.state.workspace_groups.pending_prune,
            std::slice::from_ref(&b)
        );
        sync(&mut app);
        assert!(saved().workspaces.contains_key(&b));

        finish_session_save(&mut app);
        sync(&mut app);
        assert!(!saved().workspaces.contains_key(&b));
        assert!(saved().workspaces.contains_key(&a));
        assert!(app.state.workspace_groups.pending_prune.is_empty());
        let session = crate::persist::load().unwrap();
        assert_eq!(session.workspaces.len(), 1);
    }

    fn interrupted_exit_of_second_workspace(app: &mut App) -> String {
        let b = app.state.workspaces[1].id.clone();
        let pane_id = app.state.workspaces[1].tabs[0].root_pane;
        app.handle_internal_event(AppEvent::PaneDied {
            pane_id,
            exit_reason: crate::platform::ChildExitReason::Interrupted,
        });
        assert_eq!(app.state.workspaces.len(), 1);
        assert!(
            !app.state.session_dirty,
            "the checkpoint path clears session_dirty"
        );
        b
    }

    fn grouped_pair(app: &mut App) {
        let (a, b) = (
            app.state.workspaces[0].id.clone(),
            app.state.workspaces[1].id.clone(),
        );
        edit(app, &[create("Work", &[a, b])]);
        sync(app);
    }

    #[test]
    fn close_via_last_pane_exit_prunes_record_after_session_save() {
        let _home = TestConfigHome::new("last-pane-exit");
        let mut app = persistent_app(workspaces(&["a", "b"]));
        grouped_pair(&mut app);

        let b = interrupted_exit_of_second_workspace(&mut app);
        sync(&mut app);
        assert!(saved().workspaces.contains_key(&b));

        finish_session_save(&mut app);
        sync(&mut app);
        assert!(!saved().workspaces.contains_key(&b));
        assert_eq!(crate::persist::load().unwrap().workspaces.len(), 1);
    }

    #[test]
    fn interrupted_pane_exit_keeps_record_while_checkpoint_pending() {
        let _home = TestConfigHome::new("checkpoint-pending");
        let mut app = persistent_app(workspaces(&["a", "b"]));
        grouped_pair(&mut app);

        let b = interrupted_exit_of_second_workspace(&mut app);
        for _ in 0..3 {
            sync(&mut app);
            assert!(app.state.workspace_groups.workspaces.contains_key(&b));
        }
        assert!(saved().workspaces.contains_key(&b));
        let session = crate::persist::load().unwrap();
        assert!(session
            .workspaces
            .iter()
            .any(|ws| ws.id.as_deref() == Some(b.as_str())));
    }

    #[tokio::test]
    async fn shutdown_within_debounce_after_interrupted_exit_keeps_record() {
        let _home = TestConfigHome::new("shutdown-in-debounce");
        let mut app = persistent_app(workspaces(&["a", "b"]));
        grouped_pair(&mut app);

        let b = interrupted_exit_of_second_workspace(&mut app);
        sync(&mut app);
        app.save_session_on_shutdown();
        drop(app);
        assert!(saved().workspaces.contains_key(&b));

        let app = restart_app();
        assert!(
            ids(&app).contains(&b),
            "the checkpointed session still lists b"
        );
        assert_eq!(group_of(&app, &b).as_deref(), Some("Work"));
        shut_down(app);
    }

    #[tokio::test]
    async fn crash_between_sidecar_and_session_save_prunes_only_unsaved_workspace() {
        let _home = TestConfigHome::new("crash-between-saves");
        let mut app = persistent_app(workspaces(&["a"]));
        let a = id_of(&app, "a");
        edit(&mut app, &[create("Work", std::slice::from_ref(&a))]);
        app.save_session_now();

        app.state.workspaces.push(Workspace::test_new("unsaved"));
        app.state.ensure_test_terminals();
        let unsaved = id_of(&app, "unsaved");
        let work = app.state.workspace_groups.groups[0].id.clone();
        edit(
            &mut app,
            &[WorkspaceGroupOp::Place {
                workspace_id: unsaved.clone(),
                group_id: Some(work),
                before_workspace_id: None,
            }],
        );
        assert!(saved().workspaces.contains_key(&unsaved));
        drop(app); // crash: session.json never lists the new workspace

        let app = restart_app();
        assert_eq!(ids(&app), std::slice::from_ref(&a));
        assert_eq!(group_of(&app, &a).as_deref(), Some("Work"));
        assert!(!saved().workspaces.contains_key(&unsaved));
        assert!(saved().workspaces.contains_key(&a));
        shut_down(app);
    }

    #[test]
    fn reconcile_normalizes_after_upstream_workspace_move_and_requests_render() {
        let mut app = test_app();
        set_workspaces(&mut app, workspaces(&["a", "b", "c"]));
        let (a, b, c) = (id_of(&app, "a"), id_of(&app, "b"), id_of(&app, "c"));
        edit(&mut app, &[create("Work", &[a.clone(), b.clone()])]);
        sync(&mut app);
        let _ = app.render_dirty.take();

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "move".into(),
            method: crate::api::schema::Method::WorkspaceMove(
                crate::api::schema::WorkspaceMoveParams {
                    workspace_id: c.clone(),
                    insert_index: 0,
                },
            ),
        });
        assert!(response.contains("workspace_list"), "{response}");
        assert_eq!(ids(&app), [c.clone(), a.clone(), b.clone()]);
        let _ = app.render_dirty.take();

        sync(&mut app);

        assert_eq!(ids(&app), [a.clone(), b.clone(), c.clone()]);
        assert!(app.render_dirty.is_pending());
        assert_eq!(reordered_events(&app), [vec![a, b, c]]);
    }

    #[test]
    fn reconcile_is_noop_when_id_sequence_unchanged() {
        let mut app = test_app();
        set_workspaces(&mut app, workspaces(&["a", "b"]));
        let b = id_of(&app, "b");
        edit(&mut app, &[create("Work", std::slice::from_ref(&b))]);
        assert_eq!(ids(&app)[0], b);

        let groups = app.state.workspace_groups.clone();
        let _ = app.render_dirty.take();
        let events = app.event_hub.current_sequence();
        for _ in 0..3 {
            assert!(!app.reconcile_workspace_groups());
        }
        assert_eq!(app.state.workspace_groups, groups);
        assert!(!app.render_dirty.is_pending());
        assert_eq!(app.event_hub.current_sequence(), events);
    }

    #[test]
    fn new_workspace_lands_ungrouped_without_reorder() {
        let mut app = test_app();
        set_workspaces(&mut app, workspaces(&["a", "b"]));
        let a = id_of(&app, "a");
        edit(&mut app, &[create("Work", std::slice::from_ref(&a))]);
        sync(&mut app);
        let events = app.event_hub.current_sequence();

        app.state.workspaces.push(Workspace::test_new("new"));
        app.state.ensure_test_terminals();
        let new = id_of(&app, "new");
        sync(&mut app);

        assert_eq!(ids(&app).last(), Some(&new));
        assert!(group_of(&app, &new).is_none());
        assert!(reordered_events(&app).is_empty());
        assert_eq!(app.event_hub.current_sequence(), events);
        assert_eq!(
            app.state.workspace_groups.last_order.as_deref(),
            Some(ids(&app).as_slice())
        );
    }

    #[test]
    fn client_projection_is_empty_without_groups_or_colors() {
        let mut app = test_app();
        set_workspaces(&mut app, workspaces(&["a", "b"]));
        assert_eq!(
            app.client_shell_workspace_groups(),
            crate::protocol::ClientShellWorkspaceGroups::default()
        );

        let b = id_of(&app, "b");
        edit(&mut app, &[create("Work", std::slice::from_ref(&b))]);
        let projection = app.client_shell_workspace_groups();
        assert_eq!(projection.groups[0].workspace_ids, std::slice::from_ref(&b));
        assert_eq!(projection.workspace_colors, [(b, "blue".to_string())]);
    }

    #[test]
    fn reconcile_waits_for_settled_session_save_before_pruning() {
        let mut app = test_app();
        set_workspaces(&mut app, workspaces(&["a", "b"]));
        let b = id_of(&app, "b");
        edit(&mut app, &[create("Work", std::slice::from_ref(&b))]);
        sync(&mut app);
        let b_index = ids(&app).iter().position(|id| *id == b).unwrap();
        app.state.close_workspaces(vec![b_index]);

        app.session_save_deadline = Some(Instant::now());
        app.state.session_dirty = false;
        assert!(!app.reconcile_workspace_groups());
        assert!(app.state.workspace_groups.workspaces.contains_key(&b));
        assert_eq!(
            app.state.workspace_groups.pending_prune,
            std::slice::from_ref(&b)
        );
        assert!(
            !app.reconcile_workspace_groups(),
            "still waiting for the save"
        );

        app.session_save_deadline = None;
        assert!(app.reconcile_workspace_groups());
        assert!(!app.state.workspace_groups.workspaces.contains_key(&b));
    }

    fn request(app: &mut App, method: crate::api::schema::Method) -> serde_json::Value {
        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req".into(),
            method,
        });
        serde_json::from_str(&response).unwrap()
    }

    fn api_edit(app: &mut App, ops: Vec<WorkspaceGroupOp>) -> serde_json::Value {
        let response = request(
            app,
            crate::api::schema::Method::WorkspaceGroupEdit(
                crate::api::schema::WorkspaceGroupEditParams { ops },
            ),
        );
        app.state.assert_invariants_for_test();
        response
    }

    fn api_list(app: &mut App) -> serde_json::Value {
        request(
            app,
            crate::api::schema::Method::WorkspaceGroupList(crate::api::schema::EmptyParams {}),
        )
    }

    fn set_color(workspace_id: &str, color: Option<&str>) -> WorkspaceGroupOp {
        WorkspaceGroupOp::SetColor {
            workspace_id: workspace_id.into(),
            color: color.map(str::to_string),
        }
    }

    #[test]
    fn edit_emits_reordered_only_when_order_changes() {
        let mut app = test_app();
        set_workspaces(&mut app, workspaces(&["a", "b", "c"]));
        let (a, b, c) = (id_of(&app, "a"), id_of(&app, "b"), id_of(&app, "c"));
        sync(&mut app);

        let response = api_edit(&mut app, vec![create("Work", std::slice::from_ref(&c))]);
        assert_eq!(response["result"]["type"], "workspace_groups", "{response}");
        let work = response["result"]["groups"][0]["group_id"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(ids(&app), [c.clone(), a.clone(), b.clone()]);
        assert_eq!(
            reordered_events(&app),
            [vec![c.clone(), a.clone(), b.clone()]]
        );

        for ops in [
            vec![set_color(&a, Some("teal"))],
            vec![WorkspaceGroupOp::Update {
                group_id: work.clone(),
                name: Some("Work 2".into()),
                color: Some("green".into()),
                template: None,
            }],
            vec![WorkspaceGroupOp::Place {
                workspace_id: c.clone(),
                group_id: Some(work.clone()),
                before_workspace_id: None,
            }],
            Vec::new(),
        ] {
            let response = api_edit(&mut app, ops);
            assert_eq!(response["result"]["type"], "workspace_groups", "{response}");
            assert_eq!(reordered_events(&app).len(), 1, "{response}");
        }

        let response = api_edit(
            &mut app,
            vec![WorkspaceGroupOp::Place {
                workspace_id: b.clone(),
                group_id: Some(work.clone()),
                before_workspace_id: Some(c.clone()),
            }],
        );
        assert_eq!(response["result"]["workspaces"][0]["workspace_id"], b);
        assert_eq!(
            reordered_events(&app).last().unwrap(),
            &vec![b.clone(), c.clone(), a.clone()]
        );
        assert_eq!(reordered_events(&app).len(), 2);
        assert_eq!(
            app.state.workspace_groups.last_order.as_deref(),
            Some(ids(&app).as_slice())
        );
        assert!(!app.reconcile_workspace_groups());
    }

    #[test]
    fn edit_errors_use_contract_codes_and_commit_nothing() {
        let mut app = test_app();
        let mut first = Workspace::test_new("a");
        first.test_add_tab(None);
        set_workspaces(&mut app, vec![first, Workspace::test_new("b")]);
        let (a, b) = (id_of(&app, "a"), id_of(&app, "b"));
        api_edit(&mut app, vec![create("Work", std::slice::from_ref(&a))]);
        let before = app.state.workspace_groups.clone();
        let order = ids(&app);
        let events = app.event_hub.current_sequence();

        let foreign = crate::workspace::public_tab_id_for_number(&b, 1);
        for (ops, code, message) in [
            (
                vec![set_color(&b, Some("teal")), set_color("w_99", None)],
                "workspace_not_found",
                "workspace w_99 not found".to_string(),
            ),
            (
                vec![WorkspaceGroupOp::BindTab {
                    workspace_id: a.clone(),
                    key: "agent".into(),
                    tab_id: Some(format!("{a}:t9")),
                }],
                "tab_not_found",
                format!("tab {a}:t9 not found"),
            ),
            (
                vec![WorkspaceGroupOp::BindTab {
                    workspace_id: a.clone(),
                    key: "agent".into(),
                    tab_id: Some(foreign.clone()),
                }],
                "tab_not_found",
                format!("tab {foreign} not found in workspace {a}"),
            ),
            (
                vec![WorkspaceGroupOp::Delete {
                    group_id: "gmissing".into(),
                }],
                "workspace_group_not_found",
                "workspace group gmissing not found".to_string(),
            ),
            (
                vec![create("work", &[])],
                "workspace_group_name_taken",
                "workspace group name work is already used".to_string(),
            ),
            (
                vec![create(" ", &[])],
                "invalid_workspace_group_name",
                "workspace group names must be 1-40 characters without control characters"
                    .to_string(),
            ),
            (
                vec![set_color(&b, Some("magenta"))],
                "invalid_color",
                "unknown color magenta; use one of red, orange, yellow, green, teal, blue, purple, pink, gray"
                    .to_string(),
            ),
        ] {
            let response = api_edit(&mut app, ops);
            assert_eq!(response["error"]["code"], code, "{response}");
            assert_eq!(response["error"]["message"], message, "{response}");
            assert_eq!(app.state.workspace_groups, before, "{code}");
            assert_eq!(ids(&app), order, "{code}");
            assert_eq!(app.event_hub.current_sequence(), events, "{code}");
        }

        let full: Vec<_> = (0..crate::app::workspace_groups::MAX_TAB_BINDINGS)
            .map(|i| WorkspaceGroupOp::BindTab {
                workspace_id: a.clone(),
                key: format!("k{i}"),
                tab_id: Some(format!("{a}:t1")),
            })
            .collect();
        assert_eq!(
            api_edit(&mut app, full)["result"]["type"],
            "workspace_groups"
        );
        let response = api_edit(
            &mut app,
            vec![WorkspaceGroupOp::BindTab {
                workspace_id: a.clone(),
                key: "one-more".into(),
                tab_id: Some(format!("{a}:t1")),
            }],
        );
        assert_eq!(response["error"]["code"], "too_many_tab_bindings");
        assert_eq!(
            response["error"]["message"],
            format!("workspace {a} already has 32 tab bindings")
        );
    }

    #[test]
    fn edit_accepts_id_aliases_and_returns_canonical_ids() {
        let mut app = test_app();
        set_workspaces(&mut app, workspaces(&["a", "b"]));
        let (a, b) = (id_of(&app, "a"), id_of(&app, "b"));

        let response = api_edit(
            &mut app,
            vec![
                WorkspaceGroupOp::Create {
                    name: "Work".into(),
                    color: None,
                    template: None,
                    workspace_ids: vec!["2".into()],
                    before_group_id: None,
                },
                WorkspaceGroupOp::BindTab {
                    workspace_id: "w_1".into(),
                    key: "agent".into(),
                    tab_id: Some("1:1".into()),
                },
            ],
        );
        let result = &response["result"];
        assert_eq!(result["workspaces"][0]["workspace_id"], b, "{response}");
        assert_eq!(
            result["workspaces"][0]["group_id"],
            result["groups"][0]["group_id"]
        );
        assert_eq!(result["workspaces"][1]["workspace_id"], a);
        assert_eq!(
            result["workspaces"][1]["tabs"],
            serde_json::json!([{"key": "agent", "tab_id": format!("{a}:t1")}])
        );

        // An unknown before-target is not an error: the workspace goes to the region's end.
        let work = result["groups"][0]["group_id"]
            .as_str()
            .unwrap()
            .to_string();
        let response = api_edit(
            &mut app,
            vec![WorkspaceGroupOp::Place {
                workspace_id: a.clone(),
                group_id: Some(work),
                before_workspace_id: Some("w_99".into()),
            }],
        );
        assert_eq!(response["result"]["workspaces"][1]["workspace_id"], a);
    }

    #[test]
    fn list_reports_display_order_cwd_linked_and_live_tab_bindings() {
        let home = TestConfigHome::new("list");
        let mut parent = Workspace::test_new("parent");
        parent.test_add_tab(None);
        parent.worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
            key: "repo-key".into(),
            label: "repo".into(),
            repo_root: home.path.join("repo"),
            checkout_path: home.path.join("repo"),
            is_linked_worktree: false,
        });
        let mut child = Workspace::test_new("child");
        child.worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
            key: "repo-key".into(),
            label: "repo".into(),
            repo_root: home.path.join("repo"),
            checkout_path: home.path.join("repo-issue"),
            is_linked_worktree: true,
        });
        let mut app = test_app();
        set_workspaces(&mut app, vec![Workspace::test_new("plain"), parent, child]);
        let (plain, parent, child) = (
            id_of(&app, "plain"),
            id_of(&app, "parent"),
            id_of(&app, "child"),
        );
        let root = app.state.workspaces[1].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[1]
            .pane_state(root)
            .unwrap()
            .attached_terminal_id
            .clone();
        let parent_cwd = home.path.join("repo");
        app.state.terminals.get_mut(&terminal_id).unwrap().cwd = parent_cwd.clone();

        let response = api_edit(
            &mut app,
            vec![
                WorkspaceGroupOp::Create {
                    name: "Work".into(),
                    color: Some("blue".into()),
                    template: Some("Work".into()),
                    workspace_ids: vec![parent.clone()],
                    before_group_id: None,
                },
                WorkspaceGroupOp::Create {
                    name: "Infra".into(),
                    color: None,
                    template: Some("Infra".into()),
                    workspace_ids: Vec::new(),
                    before_group_id: None,
                },
                WorkspaceGroupOp::Bind {
                    workspace_id: parent.clone(),
                    project: Some("api".into()),
                },
                WorkspaceGroupOp::BindTab {
                    workspace_id: parent.clone(),
                    key: "dev".into(),
                    tab_id: Some(crate::workspace::public_tab_id_for_number(&parent, 2)),
                },
                WorkspaceGroupOp::BindTab {
                    workspace_id: parent.clone(),
                    key: "agent".into(),
                    tab_id: Some(crate::workspace::public_tab_id_for_number(&parent, 1)),
                },
                set_color(&plain, Some("teal")),
            ],
        );
        let infra = response["result"]["groups"][1]["group_id"]
            .as_str()
            .unwrap()
            .to_string();
        api_edit(&mut app, vec![WorkspaceGroupOp::Delete { group_id: infra }]);
        assert!(
            app.state.workspaces[0].close_tab(1),
            "tab 2 of the parent closes"
        );

        let response = api_list(&mut app);
        let result = &response["result"];
        assert_eq!(result["type"], "workspace_groups", "{response}");
        assert_eq!(
            result["groups"],
            serde_json::json!([{
                "group_id": app.state.workspace_groups.groups[0].id,
                "name": "Work",
                "color": "blue",
                "template": "Work"
            }])
        );
        assert_eq!(result["deleted_templates"], serde_json::json!(["Infra"]));
        let members = result["workspaces"].as_array().unwrap();
        let order: Vec<_> = members
            .iter()
            .map(|member| member["workspace_id"].as_str().unwrap())
            .collect();
        assert_eq!(order, [parent.as_str(), child.as_str(), plain.as_str()]);
        assert_eq!(members[0]["group_id"], result["groups"][0]["group_id"]);
        assert_eq!(members[0]["project"], "api");
        assert_eq!(members[0]["linked"], false);
        assert_eq!(members[0]["cwd"], parent_cwd.to_string_lossy().as_ref());
        assert_eq!(
            members[0]["tabs"],
            serde_json::json!([{
                "key": "agent",
                "tab_id": crate::workspace::public_tab_id_for_number(&parent, 1)
            }])
        );
        assert_eq!(members[1]["linked"], true);
        assert_eq!(members[1]["group_id"], result["groups"][0]["group_id"]);
        assert_eq!(members[2]["color"], "teal");
        assert!(members[2].get("group_id").is_none());
        assert!(members[2].get("project").is_none());
        assert_eq!(members[2]["tabs"], serde_json::json!([]));
        assert!(members[2]["cwd"]
            .as_str()
            .is_some_and(|cwd| !cwd.is_empty()));
    }

    #[test]
    fn edit_first_reconciles_closes_and_linked_children_from_the_same_iteration() {
        let home = TestConfigHome::new("edit-reconciles-first");
        let worktree = |linked: bool| crate::workspace::WorktreeSpaceMembership {
            key: "repo-key".into(),
            label: "repo".into(),
            repo_root: home.path.join("repo"),
            checkout_path: home.path.join(if linked { "repo-issue" } else { "repo" }),
            is_linked_worktree: linked,
        };
        let mut parent = Workspace::test_new("parent");
        parent.worktree_space = Some(worktree(false));
        let mut app = persistent_app(vec![
            parent,
            Workspace::test_new("b"),
            Workspace::test_new("c"),
        ]);
        let (parent, b, c) = (id_of(&app, "parent"), id_of(&app, "b"), id_of(&app, "c"));
        edit(&mut app, &[create("Work", &[parent.clone(), b.clone()])]);
        sync(&mut app);

        // Earlier in the same API drain, with no sync in between: b closes and a
        // linked child of the grouped parent appears.
        let b_index = ids(&app).iter().position(|id| *id == b).unwrap();
        app.state.close_workspaces(vec![b_index]);
        let mut child = Workspace::test_new("child");
        child.worktree_space = Some(worktree(true));
        app.state.workspaces.push(child);
        app.state.ensure_test_terminals();
        let child = id_of(&app, "child");

        let response = api_edit(&mut app, vec![set_color(&c, Some("teal"))]);
        assert_eq!(response["result"]["type"], "workspace_groups", "{response}");
        assert_eq!(
            app.state.workspace_groups.pending_prune,
            std::slice::from_ref(&b)
        );
        assert_eq!(group_of(&app, &child).as_deref(), Some("Work"));
        assert_eq!(ids(&app), [parent.clone(), child.clone(), c.clone()]);
        assert!(saved().workspaces.contains_key(&b), "session save pending");

        sync(&mut app);
        finish_session_save(&mut app);
        sync(&mut app);
        assert!(app.state.workspace_groups.pending_prune.is_empty());
        let file = saved();
        assert!(!file.workspaces.contains_key(&b));
        assert!(file.workspaces.contains_key(&child));
        assert_eq!(file.workspaces[&c].color.as_deref(), Some("teal"));
    }

    #[tokio::test]
    async fn adversarial_tab_identity_binds_public_tab_numbers_across_restart() {
        let _home = TestConfigHome::new("adversarial-identity");
        let mut app = test_app();
        app.policy.persist_session = true;
        app.state = crate::app::AppState::test_with_adversarial_identity_state();
        app.state.workspaces.push(Workspace::test_new("plain"));
        app.state.ensure_test_terminals();
        app.load_workspace_groups();
        app.state.assert_invariants_for_test();
        sync(&mut app);

        let ws = app.state.workspaces[0].id.clone();
        let plain = id_of(&app, "plain");
        let numbers: Vec<_> = app.state.workspaces[0]
            .tabs
            .iter()
            .map(|tab| tab.number)
            .collect();
        let (survivor, second) = (numbers[0], numbers[1]);
        assert_ne!(survivor, 1, "the fixture separates position from number");
        assert_ne!(second, 2, "the fixture separates position from number");
        let survivor_tab = crate::workspace::public_tab_id_for_number(&ws, survivor);
        let second_tab = crate::workspace::public_tab_id_for_number(&ws, second);
        assert_eq!(
            app.public_tab_id(0, 0).as_deref(),
            Some(survivor_tab.as_str())
        );

        let response = api_edit(
            &mut app,
            vec![
                create("Work", std::slice::from_ref(&plain)),
                WorkspaceGroupOp::BindTab {
                    workspace_id: ws.clone(),
                    key: "agent".into(),
                    tab_id: Some(survivor_tab.clone()),
                },
                // Positional aliases resolve against the pre-edit order: workspace 1, tab 2.
                WorkspaceGroupOp::BindTab {
                    workspace_id: "1".into(),
                    key: "dev".into(),
                    tab_id: Some("1:2".into()),
                },
            ],
        );
        assert_eq!(response["result"]["type"], "workspace_groups", "{response}");
        assert_eq!(ids(&app), [plain.clone(), ws.clone()]);
        let bindings = std::collections::BTreeMap::from([
            ("agent".to_string(), survivor),
            ("dev".to_string(), second),
        ]);
        assert_eq!(app.state.workspace_groups.workspaces[&ws].tabs, bindings);

        let work = app.state.workspace_groups.groups[0].id.clone();
        api_edit(
            &mut app,
            vec![WorkspaceGroupOp::Place {
                workspace_id: ws.clone(),
                group_id: Some(work),
                before_workspace_id: Some(plain.clone()),
            }],
        );
        assert_eq!(ids(&app), [ws.clone(), plain.clone()]);
        sync(&mut app);
        assert_eq!(saved().workspaces[&ws].tabs, bindings);

        let listed_tabs = serde_json::json!([
            {"key": "agent", "tab_id": survivor_tab},
            {"key": "dev", "tab_id": second_tab},
        ]);
        let listed = api_list(&mut app);
        assert_eq!(listed["result"]["workspaces"][0]["workspace_id"], ws);
        assert_eq!(listed["result"]["workspaces"][0]["tabs"], listed_tabs);

        app.save_session_now();
        drop(app);
        let mut app = restart_app();
        assert_eq!(ids(&app), [ws.clone(), plain.clone()]);
        let restored: Vec<_> = app.state.workspaces[0]
            .tabs
            .iter()
            .map(|tab| tab.number)
            .collect();
        assert_eq!(restored, numbers);
        sync(&mut app);

        assert_eq!(group_of(&app, &ws).as_deref(), Some("Work"));
        assert_eq!(group_of(&app, &plain).as_deref(), Some("Work"));
        assert_eq!(app.state.workspace_groups.workspaces[&ws].tabs, bindings);
        // Restored panes run a command that exits at once; `handle_api_request` would
        // drain those exits first and close the workspaces, so ask the handler directly.
        let listed: serde_json::Value =
            serde_json::from_str(&app.handle_workspace_group_list("req".into())).unwrap();
        assert_eq!(listed["result"]["workspaces"][0]["workspace_id"], ws);
        assert_eq!(listed["result"]["workspaces"][0]["tabs"], listed_tabs);
        shut_down(app);
    }

    #[test]
    fn empty_groups_persist_across_restart() {
        let _home = TestConfigHome::new("empty-groups");
        let mut app = persistent_app(workspaces(&["a"]));
        let a = id_of(&app, "a");
        let response = api_edit(
            &mut app,
            vec![
                create("Scratch", &[]),
                create("Work", std::slice::from_ref(&a)),
            ],
        );
        assert_eq!(response["result"]["groups"][0]["name"], "Scratch");
        drop(app);

        let mut app = persistent_app(vec![with_id("a", &a)]);
        let response = api_list(&mut app);
        let names: Vec<_> = response["result"]["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|group| group["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names, ["Scratch", "Work"]);
        assert_eq!(group_of(&app, &a).as_deref(), Some("Work"));
    }

    #[test]
    fn edit_saves_sidecar_only_when_content_changes() {
        let _home = TestConfigHome::new("save-on-change");
        let mut app = persistent_app(workspaces(&["a"]));
        let a = id_of(&app, "a");
        let path = sidecar::sidecar_path();
        assert!(!path.exists(), "loading never creates the file");

        api_edit(&mut app, vec![create("Work", std::slice::from_ref(&a))]);
        assert!(path.exists());
        std::fs::remove_file(&path).unwrap();

        let work = app.state.workspace_groups.groups[0].id.clone();
        for ops in [
            Vec::new(),
            vec![WorkspaceGroupOp::Place {
                workspace_id: a.clone(),
                group_id: Some(work.clone()),
                before_workspace_id: None,
            }],
            vec![WorkspaceGroupOp::Update {
                group_id: work.clone(),
                name: Some("Work".into()),
                color: None,
                template: None,
            }],
        ] {
            assert_eq!(
                api_edit(&mut app, ops)["result"]["type"],
                "workspace_groups"
            );
            assert!(!path.exists(), "an edit that changes nothing does not save");
        }

        api_edit(&mut app, vec![set_color(&a, Some("red"))]);
        assert_eq!(saved().workspaces[&a].color.as_deref(), Some("red"));
    }

    #[test]
    fn blocked_sidecar_is_never_saved() {
        let _home = TestConfigHome::new("blocked");
        let mut app = persistent_app(workspaces(&["a"]));
        app.state.workspace_groups.persist_blocked = true;
        let a = id_of(&app, "a");

        let response = api_edit(&mut app, vec![create("Work", std::slice::from_ref(&a))]);

        assert_eq!(response["result"]["groups"][0]["name"], "Work");
        assert!(!sidecar::sidecar_path().exists());
    }

    /// The handoff policy cannot be named off unix; this checks the same flags on every platform.
    fn assert_policy_loads_sidecar_before_first_edit(policy: crate::app::AppPolicy) {
        let _home = TestConfigHome::new("handoff-policy");
        let mut app = persistent_app(Vec::new());
        api_edit(&mut app, vec![create("Earlier", &[])]);
        drop(app);

        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::try_new(
            &Config::default(),
            policy,
            None,
            api_rx,
            crate::api::EventHub::default(),
        )
        .unwrap();
        assert!(app.state.workspaces.is_empty());
        assert_eq!(app.state.workspace_groups.groups.len(), 1);

        api_edit(&mut app, vec![create("Later", &[])]);
        let names: Vec<_> = saved().groups.into_iter().map(|group| group.name).collect();
        assert_eq!(names, ["Earlier", "Later"]);
    }

    #[test]
    fn persisting_policy_without_restore_loads_sidecar_before_first_edit() {
        assert_policy_loads_sidecar_before_first_edit(crate::app::AppPolicy {
            restore_session: false,
            persist_session: true,
            persist_plugin_registry: false,
            background_updates: false,
        });
    }

    #[cfg(unix)]
    #[test]
    fn handoff_replacement_policy_loads_sidecar_before_first_edit() {
        assert_policy_loads_sidecar_before_first_edit(crate::app::AppPolicy::HANDOFF_REPLACEMENT);
    }
}
