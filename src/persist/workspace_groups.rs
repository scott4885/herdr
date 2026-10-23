//! Fork-only sidecar that persists workspace groups next to `session.json`.
//!
//! The file is keyed by workspace id only and never touches the session schema,
//! so upstream herdr keeps loading `session.json` unchanged. A file that cannot be
//! parsed, or that a newer version wrote, is renamed aside and never overwritten.

use std::path::{Path, PathBuf};

pub(crate) const SIDECAR_FILE_NAME: &str = "fork-workspace-groups.json";
pub(crate) const SIDECAR_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct WorkspaceGroupsFile {
    pub(crate) version: u32, // required; missing => unparseable => set aside
    #[serde(default)]
    pub(crate) groups: Vec<WorkspaceGroupFileEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) deleted_templates: Vec<String>,
    #[serde(default)]
    pub(crate) workspaces: std::collections::BTreeMap<String, WorkspaceRecordFileEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct WorkspaceGroupFileEntry {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) color: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) template: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct WorkspaceRecordFileEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) project: Option<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub(crate) tabs: std::collections::BTreeMap<String, usize>,
}

#[derive(Debug)]
pub(crate) enum SidecarLoad {
    Missing,
    Loaded(WorkspaceGroupsFile),
    SetAside { moved_to: PathBuf, reason: String }, // start empty
    Blocked { reason: String }, // unreadable and could not be renamed: start empty, persist_blocked = true
}

pub(crate) fn sidecar_path() -> PathBuf {
    crate::session::data_dir().join(SIDECAR_FILE_NAME)
}

pub(crate) fn load_sidecar(path: &Path, now_unix_secs: u64) -> SidecarLoad {
    let reason = match std::fs::read_to_string(path) {
        Ok(content) => match serde_json::from_str::<WorkspaceGroupsFile>(&content) {
            Ok(file) if file.version <= SIDECAR_VERSION => return SidecarLoad::Loaded(file),
            Ok(file) => format!(
                "file version {} is newer than supported version {SIDECAR_VERSION}",
                file.version
            ),
            Err(err) => format!("failed to parse: {err}"),
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return SidecarLoad::Missing,
        Err(err) => format!("failed to read: {err}"),
    };
    set_aside(path, now_unix_secs, reason)
}

pub(crate) fn save_sidecar(path: &Path, file: &WorkspaceGroupsFile) -> std::io::Result<()> {
    super::io::save_json_to_path(path, file)
}

fn set_aside(path: &Path, now_unix_secs: u64, reason: String) -> SidecarLoad {
    let mut moved_name = path.as_os_str().to_owned();
    moved_name.push(format!(".unloaded-{now_unix_secs}"));
    let moved_to = PathBuf::from(moved_name);
    // Never replace an earlier set-aside copy; keep both files and stop saving instead.
    if moved_to.exists() {
        return SidecarLoad::Blocked {
            reason: format!("{reason}; {} already exists", moved_to.display()),
        };
    }
    match std::fs::rename(path, &moved_to) {
        Ok(()) => SidecarLoad::SetAside { moved_to, reason },
        Err(err) => SidecarLoad::Blocked {
            reason: format!("{reason}; could not rename it aside: {err}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const SPEC_EXAMPLE: &str = r#"{
  "version": 1,
  "groups": [
    {"id": "g19a3f2c1b7", "name": "Work", "color": "blue", "template": "Work"},
    {"id": "g19a3f2c1b8", "name": "Scratch", "color": "gray"}
  ],
  "deleted_templates": ["Infra"],
  "workspaces": {
    "w3": {"group": "g19a3f2c1b7", "color": "teal", "project": "api", "tabs": {"agent": 1, "dev": 2}},
    "w5": {"group": "g19a3f2c1b8"}
  }
}"#;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let unique = format!(
                "herdr-workspace-groups-tests-{}-{}-{}",
                name,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            );
            let dir = std::env::temp_dir().join(unique);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn sidecar(&self) -> PathBuf {
            self.0.join(SIDECAR_FILE_NAME)
        }

        fn file_names(&self) -> Vec<String> {
            let mut names: Vec<_> = std::fs::read_dir(&self.0)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn example_file() -> WorkspaceGroupsFile {
        WorkspaceGroupsFile {
            version: 1,
            groups: vec![
                WorkspaceGroupFileEntry {
                    id: "g19a3f2c1b7".into(),
                    name: "Work".into(),
                    color: "blue".into(),
                    template: Some("Work".into()),
                },
                WorkspaceGroupFileEntry {
                    id: "g19a3f2c1b8".into(),
                    name: "Scratch".into(),
                    color: "gray".into(),
                    template: None,
                },
            ],
            deleted_templates: vec!["Infra".into()],
            workspaces: BTreeMap::from([
                (
                    "w3".into(),
                    WorkspaceRecordFileEntry {
                        group: Some("g19a3f2c1b7".into()),
                        color: Some("teal".into()),
                        project: Some("api".into()),
                        tabs: BTreeMap::from([("agent".into(), 1), ("dev".into(), 2)]),
                    },
                ),
                (
                    "w5".into(),
                    WorkspaceRecordFileEntry {
                        group: Some("g19a3f2c1b8".into()),
                        ..WorkspaceRecordFileEntry::default()
                    },
                ),
            ]),
        }
    }

    #[test]
    fn missing_file_loads_empty() {
        let dir = TempDir::new("missing");

        assert!(matches!(
            load_sidecar(&dir.sidecar(), 1_700_000_000),
            SidecarLoad::Missing
        ));
        assert!(dir.file_names().is_empty(), "loading must not create files");
    }

    #[test]
    fn spec_example_parses_and_serializes_without_empty_optionals() {
        let parsed: WorkspaceGroupsFile = serde_json::from_str(SPEC_EXAMPLE).unwrap();
        assert_eq!(parsed, example_file());

        let value = serde_json::to_value(&parsed).unwrap();
        assert_eq!(
            value,
            serde_json::from_str::<serde_json::Value>(SPEC_EXAMPLE).unwrap()
        );
        assert_eq!(
            serde_json::to_value(WorkspaceGroupsFile {
                version: 1,
                groups: Vec::new(),
                deleted_templates: Vec::new(),
                workspaces: BTreeMap::new(),
            })
            .unwrap(),
            serde_json::json!({"version": 1, "groups": [], "workspaces": {}})
        );
    }

    #[test]
    fn unknown_fields_are_tolerated() {
        let parsed: WorkspaceGroupsFile = serde_json::from_str(
            r#"{"version":1,"future":true,"groups":[{"id":"g1","name":"Work","color":"blue","future":1}],
                "workspaces":{"w1":{"group":"g1","future":"x"}}}"#,
        )
        .unwrap();

        assert_eq!(parsed.groups[0].name, "Work");
        assert_eq!(parsed.workspaces["w1"].group.as_deref(), Some("g1"));
    }

    #[test]
    fn roundtrip_preserves_groups_records_tombstones() {
        let dir = TempDir::new("roundtrip");
        let path = dir.sidecar();

        save_sidecar(&path, &example_file()).unwrap();

        match load_sidecar(&path, 1_700_000_000) {
            SidecarLoad::Loaded(file) => assert_eq!(file, example_file()),
            other => panic!("expected loaded sidecar, got {other:?}"),
        }
        assert_eq!(dir.file_names(), [SIDECAR_FILE_NAME]);
    }

    #[test]
    fn corrupt_or_newer_file_is_set_aside_not_overwritten() {
        for (name, content) in [
            ("corrupt", "{ not json".to_string()),
            ("missing-version", r#"{"groups":[]}"#.to_string()),
            ("newer", r#"{"version":2,"groups":[]}"#.to_string()),
        ] {
            let dir = TempDir::new(name);
            let path = dir.sidecar();
            std::fs::write(&path, &content).unwrap();

            let moved_to = match load_sidecar(&path, 1_700_000_000) {
                SidecarLoad::SetAside { moved_to, reason } => {
                    assert!(!reason.is_empty(), "{name}");
                    moved_to
                }
                other => panic!("{name}: expected set-aside, got {other:?}"),
            };

            assert_eq!(
                moved_to,
                dir.0
                    .join(format!("{SIDECAR_FILE_NAME}.unloaded-1700000000")),
                "{name}"
            );
            assert_eq!(
                std::fs::read_to_string(&moved_to).unwrap(),
                content,
                "{name}"
            );
            assert!(!path.exists(), "{name}: original must be moved, not copied");

            save_sidecar(&path, &example_file()).unwrap();
            assert_eq!(
                std::fs::read_to_string(&moved_to).unwrap(),
                content,
                "{name}"
            );
            assert_eq!(
                dir.file_names(),
                [
                    SIDECAR_FILE_NAME.to_string(),
                    format!("{SIDECAR_FILE_NAME}.unloaded-1700000000")
                ],
                "{name}"
            );
        }
    }

    #[test]
    fn unrenamable_corrupt_file_blocks_persistence_and_keeps_both_files() {
        let dir = TempDir::new("blocked");
        let path = dir.sidecar();
        let earlier = dir
            .0
            .join(format!("{SIDECAR_FILE_NAME}.unloaded-1700000000"));
        std::fs::write(&path, "{ corrupt").unwrap();
        std::fs::write(&earlier, "earlier set-aside copy").unwrap();

        match load_sidecar(&path, 1_700_000_000) {
            SidecarLoad::Blocked { reason } => assert!(!reason.is_empty()),
            other => panic!("expected blocked sidecar, got {other:?}"),
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ corrupt");
        assert_eq!(
            std::fs::read_to_string(&earlier).unwrap(),
            "earlier set-aside copy"
        );
    }

    #[test]
    fn save_replaces_through_a_temporary_file() {
        let dir = TempDir::new("save");
        let path = dir.sidecar();
        let mut file = example_file();

        save_sidecar(&path, &file).unwrap();
        file.groups.pop();
        save_sidecar(&path, &file).unwrap();

        assert_eq!(dir.file_names(), [SIDECAR_FILE_NAME]);
        match load_sidecar(&path, 1) {
            SidecarLoad::Loaded(loaded) => assert_eq!(loaded, file),
            other => panic!("expected loaded sidecar, got {other:?}"),
        }
    }

    mod session_characterization {
        use crate::api::schema::WorkspaceGroupOp;
        use crate::app::workspace_groups::{plan_edit, WorkspaceGroups, WorkspaceMeta};
        use crate::app::AppState;
        use crate::persist::snapshot::{layout_fingerprint, parse_snapshot, SNAPSHOT_VERSION};
        use crate::workspace::Workspace;

        fn state(names: &[&str]) -> AppState {
            let mut state = AppState::test_new();
            state.workspaces = names.iter().map(|name| Workspace::test_new(name)).collect();
            state.active = Some(0);
            state.ensure_test_terminals();
            state
        }

        fn metas(state: &AppState) -> Vec<WorkspaceMeta> {
            state
                .workspaces
                .iter()
                .map(WorkspaceMeta::from_workspace)
                .collect()
        }

        /// Applies ops like the edit handler; returns whether the workspace order changed.
        fn apply(state: &mut AppState, ops: &[WorkspaceGroupOp]) -> bool {
            let (groups, order) =
                plan_edit(&state.workspace_groups, &metas(state), ops, 1).unwrap();
            state.workspace_groups = groups;
            state.move_workspace_block(&order, None)
        }

        fn session_json(state: &AppState) -> String {
            let snapshot = crate::persist::capture(
                &state.workspaces,
                &state.terminals,
                &crate::terminal::TerminalRuntimeRegistry::new(),
                state.active,
                state.selected,
            );
            serde_json::to_string(&snapshot).unwrap()
        }

        fn object_keys(value: &serde_json::Value, keys: &mut Vec<String>) {
            match value {
                serde_json::Value::Object(object) => {
                    for (key, value) in object {
                        keys.push(key.clone());
                        object_keys(value, keys);
                    }
                }
                serde_json::Value::Array(values) => {
                    for value in values {
                        object_keys(value, keys);
                    }
                }
                _ => {}
            }
        }

        fn key_set(json: &str) -> std::collections::BTreeSet<String> {
            let mut keys = Vec::new();
            object_keys(&serde_json::from_str(json).unwrap(), &mut keys);
            keys.into_iter().collect()
        }

        fn group_ops(state: &AppState) -> Vec<WorkspaceGroupOp> {
            let id = |index: usize| state.workspaces[index].id.clone();
            vec![
                WorkspaceGroupOp::Create {
                    name: "Work".into(),
                    color: Some("teal".into()),
                    template: Some("Work".into()),
                    workspace_ids: vec![id(0)],
                    before_group_id: None,
                },
                WorkspaceGroupOp::Create {
                    name: "Scratch".into(),
                    color: None,
                    template: None,
                    workspace_ids: Vec::new(),
                    before_group_id: None,
                },
                WorkspaceGroupOp::SetColor {
                    workspace_id: id(1),
                    color: Some("red".into()),
                },
                WorkspaceGroupOp::Bind {
                    workspace_id: id(2),
                    project: Some("api".into()),
                },
                WorkspaceGroupOp::BindTab {
                    workspace_id: id(2),
                    key: "agent".into(),
                    tab_id: Some(crate::workspace::public_tab_id_for_number(&id(2), 1)),
                },
            ]
        }

        fn place_last_in_first_group(state: &mut AppState) -> bool {
            let last = state.workspaces[2].id.clone();
            let first_group = state.workspace_groups.groups[0].id.clone();
            apply(
                state,
                &[WorkspaceGroupOp::Place {
                    workspace_id: last,
                    group_id: Some(first_group),
                    before_workspace_id: None,
                }],
            )
        }

        #[test]
        fn fork_session_json_has_upstream_schema() {
            let mut state = state(&["a", "b", "c"]);
            let upstream_json = session_json(&state);
            let c = state.workspaces[2].id.clone();
            let ops = group_ops(&state);
            assert!(!apply(&mut state, &ops));
            assert!(place_last_in_first_group(&mut state));

            let fork_json = session_json(&state);
            assert_eq!(key_set(&fork_json), key_set(&upstream_json));
            for key in key_set(&fork_json) {
                for fork_word in ["group", "color", "project", "template"] {
                    assert!(!key.contains(fork_word), "fork key {key} in session.json");
                }
            }

            let parsed = parse_snapshot(&fork_json).unwrap();
            assert_eq!(parsed.version, SNAPSHOT_VERSION);
            assert_eq!(SNAPSHOT_VERSION, 3);
            let order: Vec<_> = parsed
                .workspaces
                .iter()
                .map(|workspace| workspace.id.clone().unwrap())
                .collect();
            let expected: Vec<_> = state.workspaces.iter().map(|ws| ws.id.clone()).collect();
            assert_eq!(order, expected);
            assert_eq!(order[1], c, "workspaces[] is written in group order");
        }

        #[test]
        fn layout_fingerprint_independent_of_groups() {
            let mut state = state(&["a", "b", "c"]);
            let fingerprint = |state: &AppState| {
                layout_fingerprint(&parse_snapshot(&session_json(state)).unwrap()).unwrap()
            };
            let upstream_json = session_json(&state);
            let upstream_fingerprint = fingerprint(&state);

            let ops = group_ops(&state);
            assert!(!apply(&mut state, &ops), "these edits keep the order");
            assert_ne!(state.workspace_groups, WorkspaceGroups::default());
            assert_eq!(session_json(&state), upstream_json);
            assert_eq!(fingerprint(&state), upstream_fingerprint);

            // A group-driven reorder fingerprints like the same order without any group data.
            assert!(place_last_in_first_group(&mut state));
            let grouped_fingerprint = fingerprint(&state);
            let grouped_json = session_json(&state);
            state.workspace_groups = WorkspaceGroups::default();
            assert_eq!(session_json(&state), grouped_json);
            assert_eq!(fingerprint(&state), grouped_fingerprint);
        }
    }
}
