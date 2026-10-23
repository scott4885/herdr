//! Workspace group projection carried in every client shell snapshot.
//!
//! The server owns group state; clients receive groups and effective colors
//! through this one `#[serde(default)]` snapshot field, so older snapshots
//! decode to the empty default.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientShellWorkspaceGroups {
    #[serde(default)]
    pub groups: Vec<ClientShellWorkspaceGroup>, // display order
    #[serde(default)]
    pub workspace_colors: Vec<(String, String)>, // workspace_id -> effective color name, live workspaces with a color, display order
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientShellWorkspaceGroup {
    pub group_id: String,
    pub name: String,
    pub color: String,
    #[serde(default)]
    pub template: Option<String>, // lets client undo restore the binding
    #[serde(default)]
    pub workspace_ids: Vec<String>, // live explicit members incl. linked children, display order
}

pub const WORKSPACE_COLORS: [(&str, [u8; 3]); 9] = [
    ("red", [0xc8, 0x1e, 0x1e]),
    ("orange", [0xc2, 0x41, 0x0c]),
    ("yellow", [0xa1, 0x62, 0x07]),
    ("green", [0x15, 0x80, 0x3d]),
    ("teal", [0x0f, 0x76, 0x6e]),
    ("blue", [0x1d, 0x4e, 0xd8]),
    ("purple", [0x7e, 0x22, 0xce]),
    ("pink", [0xbe, 0x18, 0x5d]),
    ("gray", [0x57, 0x53, 0x4e]),
];

/// The single color parser: trims, matches case-insensitively, returns the canonical lowercase name.
pub fn normalize_workspace_color(name: &str) -> Option<&'static str> {
    let name = name.trim();
    WORKSPACE_COLORS
        .iter()
        .map(|(canonical, _)| *canonical)
        .find(|canonical| canonical.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work_group() -> ClientShellWorkspaceGroups {
        ClientShellWorkspaceGroups {
            groups: vec![ClientShellWorkspaceGroup {
                group_id: "g19a3f2c1b7".into(),
                name: "Work".into(),
                color: "blue".into(),
                template: Some("Work".into()),
                workspace_ids: vec!["w5".into(), "w3".into()],
            }],
            workspace_colors: vec![("w5".into(), "teal".into()), ("w3".into(), "blue".into())],
        }
    }

    #[test]
    fn workspace_groups_projection_serializes_to_contract_json() {
        let expected = r#"{"groups":[{"group_id":"g19a3f2c1b7","name":"Work","color":"blue","template":"Work","workspace_ids":["w5","w3"]}],"workspace_colors":[["w5","teal"],["w3","blue"]]}"#;

        assert_eq!(serde_json::to_string(&work_group()).unwrap(), expected);
        assert_eq!(
            serde_json::from_str::<ClientShellWorkspaceGroups>(expected).unwrap(),
            work_group()
        );
    }

    #[test]
    fn workspace_groups_projection_tolerates_missing_and_unknown_fields() {
        let decoded: ClientShellWorkspaceGroups = serde_json::from_str(
            r#"{"groups":[{"group_id":"g1","name":"Work","color":"blue","future":1}],"future":true}"#,
        )
        .unwrap();

        assert_eq!(decoded.groups[0].template, None);
        assert!(decoded.groups[0].workspace_ids.is_empty());
        assert!(decoded.workspace_colors.is_empty());
        assert_eq!(
            serde_json::from_str::<ClientShellWorkspaceGroups>("{}").unwrap(),
            ClientShellWorkspaceGroups::default()
        );
    }

    #[test]
    fn snapshot_without_workspace_groups_decodes_to_empty_default() {
        let upstream_snapshot = serde_json::json!({
            "boot_id": "boot-1",
            "revision": 1,
            "update_install_command": "herdr update",
            "latest_release_notes_available": false,
            "integration_updates_available": false,
            "worktree_directory": "/tmp/herdr-worktrees",
            "tab_bar_right": [],
            "tab_bar_right_separator": " ",
            "agent_order": [],
            "workspaces": [],
            "tabs": [],
            "panes": [],
            "agents": [],
            "commands": []
        });

        let decoded: crate::protocol::ClientShellSnapshot =
            serde_json::from_value(upstream_snapshot).unwrap();
        assert_eq!(
            decoded.workspace_groups,
            ClientShellWorkspaceGroups::default()
        );

        let mut with_groups = serde_json::to_value(&decoded).unwrap();
        with_groups["workspace_groups"] = serde_json::to_value(work_group()).unwrap();
        let decoded: crate::protocol::ClientShellSnapshot =
            serde_json::from_value(with_groups).unwrap();
        assert_eq!(decoded.workspace_groups, work_group());
    }

    #[test]
    fn workspace_color_palette_matches_contract_order() {
        let names: Vec<_> = WORKSPACE_COLORS.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            names,
            ["red", "orange", "yellow", "green", "teal", "blue", "purple", "pink", "gray"]
        );
    }

    #[test]
    fn normalize_workspace_color_trims_and_ignores_case() {
        assert_eq!(normalize_workspace_color("blue"), Some("blue"));
        assert_eq!(normalize_workspace_color("  Teal "), Some("teal"));
        assert_eq!(normalize_workspace_color("GRAY"), Some("gray"));
        for (name, _) in WORKSPACE_COLORS {
            assert_eq!(normalize_workspace_color(&name.to_uppercase()), Some(name));
        }
        for invalid in ["", " ", "grey", "magenta", "#1d4ed8", "blue2", "bl ue"] {
            assert_eq!(normalize_workspace_color(invalid), None, "{invalid:?}");
        }
    }
}
