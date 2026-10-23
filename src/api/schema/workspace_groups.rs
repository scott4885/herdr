//! Workspace group methods: `workspace_group.list` and the op-tagged `workspace_group.edit`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspaceGroupEditParams {
    pub ops: Vec<WorkspaceGroupOp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum WorkspaceGroupOp {
    Create {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<String>, // None => default cycle
        #[serde(default, skip_serializing_if = "Option::is_none")]
        template: Option<String>,
        #[serde(default)]
        workspace_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before_group_id: Option<String>, // None or unknown => append
    },
    Update {
        group_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>, // None => unchanged
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<String>, // None => unchanged
        #[serde(default, skip_serializing_if = "Option::is_none")]
        template: Option<String>, // None => unchanged; Some binds and clears a matching tombstone
    },
    Delete {
        group_id: String,
    },
    Move {
        group_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before_group_id: Option<String>, // None or unknown => last
    },
    Place {
        workspace_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group_id: Option<String>, // None => ungrouped region
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before_workspace_id: Option<String>,
    },
    SetColor {
        workspace_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<String>, // None => use group color
    },
    Bind {
        workspace_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project: Option<String>, // None also clears tab bindings
    },
    BindTab {
        workspace_id: String,
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tab_id: Option<String>, // None => remove the binding; tab must belong to workspace_id
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspaceGroupInfo {
    pub group_id: String,
    pub name: String,
    pub color: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspaceGroupMemberInfo {
    pub workspace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>, // effective group
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>, // override only, not effective
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub cwd: String, // Workspace::resolved_identity_cwd_from, for herdr up adoption
    #[serde(default)]
    pub linked: bool,
    #[serde(default)]
    pub tabs: Vec<WorkspaceGroupTabBinding>, // live bindings only, key order
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkspaceGroupTabBinding {
    pub key: String,
    pub tab_id: String, // public tab id, e.g. "w3:t1"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_group_ops_use_the_contract_wire_shapes() {
        let params: WorkspaceGroupEditParams = serde_json::from_value(serde_json::json!({"ops":[
          {"op":"create","name":"Work","color":"blue","template":"Work","workspace_ids":["w3"]},
          {"op":"place","workspace_id":"w5","group_id":"g19a3f2c1b7","before_workspace_id":"w3"},
          {"op":"place","workspace_id":"w7"},
          {"op":"update","group_id":"g19a3f2c1b7","name":"Work 2","color":"green"},
          {"op":"move","group_id":"g19a3f2c1b7","before_group_id":"g19a3f2c1b8"},
          {"op":"delete","group_id":"g19a3f2c1b8"},
          {"op":"set_color","workspace_id":"w5","color":"teal"},
          {"op":"set_color","workspace_id":"w3"},
          {"op":"bind","workspace_id":"w5","project":"api"},
          {"op":"bind_tab","workspace_id":"w5","key":"agent","tab_id":"w5:t1"}
        ]}))
        .unwrap();

        assert_eq!(params.ops.len(), 10);
        assert_eq!(
            params.ops[2],
            WorkspaceGroupOp::Place {
                workspace_id: "w7".into(),
                group_id: None,
                before_workspace_id: None,
            }
        );
        assert_eq!(
            serde_json::to_value(&params.ops[7]).unwrap(),
            serde_json::json!({"op":"set_color","workspace_id":"w3"})
        );
        assert_eq!(
            serde_json::to_value(WorkspaceGroupOp::Create {
                name: "Scratch".into(),
                color: None,
                template: None,
                workspace_ids: Vec::new(),
                before_group_id: None,
            })
            .unwrap(),
            serde_json::json!({"op":"create","name":"Scratch","workspace_ids":[]})
        );
        assert!(serde_json::from_value::<WorkspaceGroupOp>(
            serde_json::json!({"op":"rename","group_id":"g1"})
        )
        .is_err());
    }

    #[test]
    fn workspace_group_requests_parse_only_with_known_ops_and_required_fields() {
        let parse = |line: &str| serde_json::from_str::<crate::api::schema::Request>(line);

        let list = parse(r#"{"id":"up-1","method":"workspace_group.list","params":{}}"#).unwrap();
        assert!(matches!(
            list.method,
            crate::api::schema::Method::WorkspaceGroupList(_)
        ));
        let edit = parse(
            r#"{"id":"c-7","method":"workspace_group.edit","params":{"ops":[{"op":"delete","group_id":"g1"}]}}"#,
        )
        .unwrap();
        assert_eq!(
            edit.method,
            crate::api::schema::Method::WorkspaceGroupEdit(WorkspaceGroupEditParams {
                ops: vec![WorkspaceGroupOp::Delete {
                    group_id: "g1".into()
                }],
            })
        );

        // The API server answers these with `invalid_request`.
        for invalid in [
            r#"{"id":"x","method":"workspace_group.edit","params":{"ops":[{"op":"rename","group_id":"g1"}]}}"#,
            r#"{"id":"x","method":"workspace_group.edit","params":{"ops":[{"op":"place"}]}}"#,
            r#"{"id":"x","method":"workspace_group.edit","params":{}}"#,
        ] {
            assert!(parse(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn workspace_group_info_omits_unbound_template() {
        let group = WorkspaceGroupInfo {
            group_id: "g19a3f2c1b8".into(),
            name: "Scratch".into(),
            color: "gray".into(),
            template: None,
        };

        assert_eq!(
            serde_json::to_value(&group).unwrap(),
            serde_json::json!({"group_id":"g19a3f2c1b8","name":"Scratch","color":"gray"})
        );
    }

    #[test]
    fn workspace_group_member_info_omits_empty_optionals() {
        let member = WorkspaceGroupMemberInfo {
            workspace_id: "w7".into(),
            group_id: None,
            color: None,
            project: None,
            cwd: "C:\\src\\scratch".into(),
            linked: false,
            tabs: Vec::new(),
        };

        assert_eq!(
            serde_json::to_value(&member).unwrap(),
            serde_json::json!({"workspace_id":"w7","cwd":"C:\\src\\scratch","linked":false,"tabs":[]})
        );
    }
}
