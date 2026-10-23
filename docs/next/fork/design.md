# Herdr fork design: workspace groups, sidebar tab rows, `herdr up`

Status: approved design (D1-D17), not yet implemented.
Base: upstream `master` at `5bd900d` (the commit every reference below was checked against).
Repository: public fork `github.com/scott4885/herdr`, upstream `github.com/herdrdev/herdr`.

Function, type and test names are authoritative. Line numbers are hints only ("near :NNN") and will drift with upstream.

---

## 1. Purpose and scope

The fork adds four things to herdr, aimed at one person who runs many coding-agent workspaces at once:

1. **Workspace groups.** One level of named, colored groups in the sidebar. Workspaces can be moved into, out of and between groups, and groups can be collapsed, by mouse and by keyboard. Groups are server state, so every attached client shows the same groups.
2. **Sidebar tab rows.** Workspaces with 2 or more tabs list their tabs as clickable child rows with per-tab agent status. This is a port of wkentaro/herdr commit `29484fc4`, restricted to multi-tab workspaces.
3. **`herdr up`.** A project file (`projects.toml`) declares groups, project workspaces and their tabs (agent or plain command). `herdr up` creates or adopts them idempotently, through the public API.
4. **Fork safety.** A clean-environment launcher for both terminal hosts, a self-update guard, and a documented way back to upstream.

Non-goals (full list in section 20):
- no change to the `session.json` schema or `SNAPSHOT_VERSION` (only the order of `workspaces[]` follows group order), the endpoint protocol version, config keys or keybinding config;
- no nested groups and no bulk actions on group members;
- no `herdr group` CLI; organizing happens in the UI, and declaration happens in `projects.toml`;
- no attempt to keep fork state in sync across a period of running upstream herdr (see D1).

---

## 2. Requirements

Each requirement names the check that proves it. Test names refer to section 17.

| # | Requirement | Check |
|--|--|--|
| R1 | One-level named groups. Create, rename, recolor, delete and reorder groups; move a workspace into, out of and between groups; collapse and expand a group. Every action is reachable by mouse and by keyboard. A linked worktree child created while the server runs joins its family's group when that group is unambiguous. When remote machines are configured, the multi-machine sidebar (which then also renders the local machine) shows each machine's groups. | Client UI tests; `linked_child_inherits_unambiguous_family_group_when_created_at_runtime`; `multi_machine_sidebar_renders_groups_per_endpoint`; smoke steps 3-7 |
| R2 | Groups (including empty ones), membership, group order, colors and `herdr up` bindings survive a server stop and restart and a server crash. Collapse state is client presentation: each attached client holds its own in memory and saves it to the per-socket client-shell preferences file, which a client reads when it attaches (last writer wins across hosts; not reloaded live). | Persistence tests; smoke step 14 |
| R3 | A failed, missing or newer-version `session.json` load never deletes group data at that startup. Records then stay in the sidecar until the first later startup that restores at least one workspace, which prunes records whose ids are not in that session (5.5 rule 2). Until then, a new workspace that reuses an old id adopts that record, and editing or closing it changes or prunes the record. | `failed_or_newer_session_load_keeps_records`; `records_survive_failed_load_until_first_successful_restore` |
| R4 | Sidebar order, workspace numbers, `herdr workspace list` and `session.json` order all follow group order, with ungrouped workspaces last. A reorder made by a group-unaware client is normalized within one server loop iteration. | Reconcile tests |
| R5 | 9 named colors. A group has a color; a workspace may override it. The color is drawn as a bar on every row of the workspace card and as the fill of the focused tab. Text is never colored. Every color is at least 4.5:1 on white. Bars are at least 3:1 against the background of the cell they are drawn on, in every built-in theme, including the host's own background behind `Color::Reset`. | Color tests; smoke step 9 |
| R6 | Workspaces with 2 or more tabs show one clickable row per tab, with that tab's agent status. Clicking focuses the tab (and activates its machine if it is remote). Single-tab workspaces get no tab rows; with no groups and no colors their cards render exactly as upstream, and otherwise differ only by the column-0 color bar (R5) and group headers. No upstream test assertion is edited for this feature. | Tab-row tests; unmodified upstream suite passes |
| R7 | `herdr up` creates or adopts groups, workspaces and tabs from `projects.toml` and starts agents through upstream `agent.start`. A second run reports "up to date" and changes nothing. It never deletes or closes anything, never reorders groups, and moves a workspace only when it creates or first binds it, or under `--reapply`. It never overrides a UI change unless `--reapply` is given or, for a group deleted in the UI, `--group <name>` selects it. Renaming a tab in the UI never causes a second agent launch. | `herdr up` plan tests; smoke step 10 |
| R8 | Windows Terminal and the VS Code integrated terminal can attach to the same server at the same time and show the same groups and colors live. The prefix key works in both. | Smoke checklist in both hosts |
| R9 | When herdr is started through `herdr-clean.cmd`, no pane inherits `ANTHROPIC_API_KEY`, `VSCODE_*`, `GIT_ASKPASS`, `CLAUDE_CODE_SSE_PORT` or `ELECTRON_RUN_AS_NODE`. | Smoke step 11 |
| R10 | Upstream herdr of the same base loads the fork's `session.json` unchanged, including pane history. The fork reads and writes nothing under `~/.claude`. | `fork_session_json_has_upstream_schema`, `layout_fingerprint_independent_of_groups`; manual abandon test; `git diff upstream/master...fork -- src \| Select-String '\.claude'` prints nothing |
| R11 | `herdr update` and `herdr channel set` refuse on fork builds, and `channel set` leaves `config.toml` untouched. | Update-guard tests |
| R12 | Mixed versions degrade cleanly: a fork client on an upstream server shows no groups and reports "Action unavailable" for organize actions; an upstream client on a fork server works, and its moves are normalized. | Compatibility tests |
| R13 | Fork logic lives in new files. Hand edits to upstream files stay under about 300 lines. No new crate dependencies, config keys or protocol version. No existing test fixture or digest is re-blessed; the generated `docs/next/api/herdr-api.schema.json` is regenerated and one new digest pin is added for `workspace_group.edit`. | At each rebase, `git diff --diff-filter=M --shortstat upstream/master...fork -- . ':!docs/next/api/herdr-api.schema.json'` reports at most about 300 insertions plus deletions, and `git diff upstream/master...fork -- Cargo.toml Cargo.lock` is empty |
| R14 | `just check` exits 0 on branch `fork`, both locally without admin rights and in GitHub Actions on the public fork. `just build` produces `herdr.exe` plus `conpty\` and `THIRD-PARTY-NOTICES\`. | Build log |
| R15 | No committed file contains employer or business names, personal usernames (except the 15.6 exceptions), user-specific absolute paths or secrets. | Before each push, `git grep -nIiE -f .git/info/hygiene-deny.txt fork` prints nothing (the deny list lives in `.git/info`, so it is never committed, and leaves out the 15.6 exceptions) |

---

## 3. Decisions log

| ID | Decision | Why |
|--|--|--|
| D1 | Simple identity: sidecar keyed by workspace id; no counter witness, no id high-water reservation, no dormant-record cap. Prune records of ids that close while the fork runs, and at startup only after a successful session restore. | The round-trip-with-upstream machinery cost code in upstream files for a rare case whose worst outcome is a wrong group, fixed by one drag. |
| D2 | Local no-admin build: per-user rustup, portable MSVC (VS 2022 / SDK 26100), Zig 0.16.0 zip, prebuilt `just` and `cargo-nextest`, all under `E:\tools`; xwin as fallback; GitHub Actions on the public fork as baseline. | 1-3 minute local edit-test cycles instead of 10-25 minutes through CI. The fork owner accepted the Microsoft Build Tools license. |
| D3 | Tab rows only for workspaces with 2 or more tabs; single-tab workspaces stay compact. | Matches wkentaro's earlier 0.8.x fork (`multi_tab = ws.tabs.len() > 1` in src/ui/sidebar.rs); commit 29484fc4 itself shows rows for every workspace, so the port adds a `len() >= 2` filter. Keeps the sidebar dense and needs zero edits to upstream test assertions. |
| D4 | Base is `upstream/master`, not the latest stable tag. Rebase with `git rebase upstream/master`, at least at each upstream stable release. | Stable releases are cut in an isolated checkout at a published preview tag (AGENTS.md near :292), so stable tags are not on master (`v0.9.5` = `2ecf19a9` is not an ancestor of master, which has 27 commits after it). The design was checked against master, the tab-rows port is resolved against master (8.4), and the next stable comes from master. |
| D5 | Public fork `scott4885/herdr` as `origin`, `herdrdev/herdr` as `upstream`. | Free Windows CI runners; Apache-2.0 allows it. Public-repo hygiene applies to every committed file. |
| D6 | Host in both Windows Terminal and the VS Code integrated terminal, through a required clean launcher. | Both are in daily use; the auto-spawned server inherits the first client's environment. |
| D7 | Self-update is blocked in fork builds. | An upstream self-update would silently replace the fork binary. |
| D8 | Group state lives in a fork-only sidecar, never in `session.json`. | Upstream keeps parsing the session, and `layout_fingerprint` (hash of the snapshot JSON) keeps matching, so pane history still restores. |
| D9 | Colors are stored server-side as a neutral name; RGB, contrast and theme adaptation stay client-side. | `herdr up` must set colors, and two hosts must agree. This is a stated deviation from AGENTS.md (near :80: "colors ... TUI/client"); the commit body says so. |
| D10 | One op-tagged `workspace_group.edit` method plus `workspace_group.list`, instead of one method per action. | One digest pin (only `edit` is advertised to client shells) and two dispatch arms (`list`, `edit`) for any number of actions; batches are all-or-nothing. |
| D11 | The tab-rows port moves into fork-only `sidebar_tabs.rs`. `workspace_rows` stays unmodified, so the workspace row keeps upstream's aggregate status for every workspace; tab rows add per-tab status beneath it. | Less upstream surface, and collapsed worktree parents keep their rolled-up status, which an upstream test asserts. |
| D12 | `herdr up` is a client of the public API, not a server feature. | Smaller server surface; the server never reads `projects.toml`. |
| D13 | Agent tabs launch through upstream `agent.start`; tabs are bound durably by public tab number in the sidecar. | Upstream already handles shell quoting, busy shells and managed-agent restore. Tab numbers are never reused within a workspace. |
| D14 | The environment is cleaned by a launcher, not in code. | Upstream deliberately keeps `ANTHROPIC_API_KEY` in pane environments (pane.rs test near :4354); changing that in code would be a behavior fork. |
| D15 | "Delete group" has no confirmation but can be undone from the menu. | Destructive actions get an undo; a confirm dialog slows the common case. |
| D16 | Group ids are time-based (`g` + hex unix-ms). | A stale collapse key in client prefs can never match a newly created group. |
| D17 | `herdr up` treats a project binding as stale when the bound workspace's cwd lies outside the project cwd, and never re-adopts that workspace in the same run (11.4 step 3.1). Approved by the fork owner. | Guards the one place where a misattached record (D1, 5.6) would do real harm: starting agents in the wrong workspace. |

---

## 4. Architecture overview

**Server** (`herdr server`, one per session):
- owns `AppState.workspace_groups` (pure data) and keeps `AppState.workspaces` in group order;
- loads and saves the sidecar `fork-workspace-groups.json`;
- runs the reconcile step once per loop iteration;
- serves `workspace_group.list` and `workspace_group.edit`;
- projects groups and effective colors into each client snapshot through one new field.

**Client** (`herdr` attached in a terminal host):
- builds grouped sidebar entries from the snapshot, draws headers, color bars and tab rows;
- handles header clicks, drags, menus and navigate-mode keys, and sends `workspace_group.edit`;
- keeps collapse state in its existing per-socket preferences file.

**CLI** (`herdr up`): reads `projects.toml`, plans against server state, and applies the plan through the public API.

```
 projects.toml ──► herdr up (CLI) ──── JSON API ────┐
                                                     ▼
 ┌──────────────────────── herdr server ────────────────────────┐
 │ AppState.workspaces (group order)   AppState.workspace_groups │
 │        │                                   ▲     │            │
 │        ▼                                   │     ▼            │
 │ session.json (schema unchanged)   API: list/edit  sidecar     │
 │                                   reconcile()  fork-workspace-│
 │                                   per loop     groups.json    │
 │ ClientShellSnapshot { ..., workspace_groups } (JSON)          │
 └───────────────┬───────────────────────────────┬──────────────┘
                 ▼                               ▼
   herdr client in Windows Terminal   herdr client in VS Code terminal
   grouped_entries, headers, bars,    (same code; collapse state per
   tab rows, menus, navigate keys      client in memory, saved per socket)
```

Data flow for one organize action: the client sends `workspace_group.edit {ops}` → the server validates the batch on a clone, commits it, reorders `workspaces`, saves the sidecar, emits `workspace.reordered` → the next render sends every client a snapshot with the new order and groups.

---

## 5. Data model and storage

### 5.1 Server model

New file `src/app/workspace_groups.rs`: pure data and pure functions, no I/O.

```rust
#[derive(Debug, Clone, Default)]
pub(crate) struct WorkspaceGroups {
    pub(crate) groups: Vec<WorkspaceGroup>,                    // Vec order = display order
    pub(crate) workspaces: BTreeMap<String, WorkspaceRecord>, // key = Workspace.id ("w3"), incl. linked worktree children
    pub(crate) deleted_templates: Vec<String>,                // projects.toml group names deleted in the UI
    // runtime only, never persisted:
    pub(crate) last_order: Option<Vec<String>>,               // live id sequence at the last reconcile; None before the first
    pub(crate) pending_prune: Vec<String>,                    // closed ids whose records wait for a session save (5.5 rule 1)
}
pub(crate) struct WorkspaceGroup {
    id: String,               // "g19a3f2c1b7"
    name: String,
    color: String,            // one of the 9 names
    template: Option<String>, // projects.toml group name this group is bound to
}
pub(crate) struct WorkspaceRecord {
    group: Option<String>,        // group id
    color: Option<String>,        // per-workspace override
    project: Option<String>,      // projects.toml project name
    tabs: BTreeMap<String, usize>,// projects.toml tab label -> public tab number
}
```

`AppState` gets one field, `workspace_groups`. It has 2 literal sites: the `AppState` literal in `App::try_new` and `AppState::test_new`.

Derived values (pure functions):
- **Effective group:** `record.group` if that group exists, else none.
- **Effective color:** the workspace override, else the group color.
- **Linked-child inference:** when a linked worktree child appears while the server runs (not at the first reconcile after startup) and has no record, collect the effective groups of the non-linked workspaces with the same `worktree_space.key`. If they are all one group, or all ungrouped, the child gets that. If they disagree, the child stays ungrouped.
- **Record cleanup:** a record with no group, color, project and tab bindings is dropped. Tab bindings to tab numbers that no longer exist are dropped on save. At most 32 tab bindings per record; a `bind_tab` beyond that fails with `too_many_tab_bindings`.
- **Names:** trimmed, 1-40 characters, no control characters, unique case-insensitively.
- **Colors:** one of the 9 names in section 9, case-insensitive, stored lowercase.
- **Default color for a new group:** the first name in [blue, green, purple, orange, teal, pink, gray, red, yellow] that the fewest groups use, so the first unused one while any is unused, and the cycle repeats once all are used. Red and yellow come last because they double as status colors.
- **Group ids:** `g` + lowercase hex unix milliseconds, incremented on collision.
- **`plan_edit(&WorkspaceGroups, &[WorkspaceMeta], ops, now_ms) -> Result<(WorkspaceGroups, Vec<String>), GroupEditError>`:** runs every op on a clone and returns the new model plus the desired workspace order. Time is passed in, so it is deterministic in tests.

### 5.2 Sidecar file

- Path: `crate::session::data_dir().join("fork-workspace-groups.json")`, which is `%APPDATA%\herdr\` for the default session and `%APPDATA%\herdr\sessions\<name>\` for a named session. Deleting a named session deletes it too.
- Debug builds use `herdr-dev` in place of `herdr` (`config::app_dir_name`), e.g. `%APPDATA%\herdr-dev\fork-workspace-groups.json`. The same holds for every `%APPDATA%\herdr` and `%LOCALAPPDATA%\herdr` path in this spec (`projects.toml`, client-shell preferences) when the binary comes from `cargo build`, `cargo run` or `just check`.
- The `fork-` prefix keeps it clear of any future upstream file.
- Code: new `src/persist/workspace_groups.rs`, registered in `src/persist.rs`.
- Format v1. Optional fields use `#[serde(default, skip_serializing_if = ...)]`:

```json
{
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
}
```

Empty groups (like "Scratch" with no members) persist until deleted.

### 5.3 Load

- `App::load_workspace_groups()` is called on the last line of `App::try_new`, after `refresh_restored_workspace_git_metadata()`.
- It is gated on `policy.persist_session`: if the server will write the file, it reads it first.
  - `AppPolicy::PRODUCTION` loads.
  - `AppPolicy::HANDOFF_REPLACEMENT` (unix live handoff: `restore_session = false`, `persist_session = true`) loads too, because `new_from_handoff` calls `try_new` first.
  - `AppPolicy::TEST` never touches disk.
- Because loading happens inside `try_new`, before any API request is drained, an early edit can never overwrite an unread file.
- Missing file: start empty.
- Unparseable file, or `version` greater than 1: rename it to `fork-workspace-groups.json.unloaded-<unix_secs>`, log `tracing::warn!`, start empty. The file is never overwritten in place.

### 5.4 Save

- Through the existing `persist::io::save_json_to_path` (temp file, `sync_all`, `platform::replace_file`, parent sync), the same writer `session.json` uses. Its visibility changes from private to `pub(super)` (1 line, near io.rs:49).
- Synchronous, and only when the content changed: after a successful `workspace_group.edit`, after startup pruning, and when reconcile changes records.

### 5.5 Pruning rules (D1)

A record is deleted in exactly three situations:

1. **In-process close, once session.json agrees.** Reconcile moves every id that was in `last_order` and is no longer live into the runtime-only `WorkspaceGroups.pending_prune`. Those records are dropped, and the sidecar saved, only after a session save that no longer lists them has completed (`session_save_settled()`: `!state.session_dirty && !pane_exit_checkpoint_pending && session_save_deadline.is_none()` and `session_save_thread` is `None` or finished; the thread handle is only reaped at the next save). While `pane_exit_checkpoint_pending` is set (exit reason `Interrupted`, e.g. `STATUS_CONTROL_C_EXIT` at Windows logoff or console close; unix `Handoff`), session.json intentionally still contains the workspace (`checkpoint_session_before_pane_exit`, `save_session_on_shutdown`), so the record must stay. If the process ends first, the record survives and rule 2 handles it at the next start. This covers every close path, including a workspace whose last pane exits.
2. **Startup after a successful restore.** In `load_workspace_groups`, if `state.workspaces` is non-empty, records whose ids are not among the restored workspaces are pruned. At the end of `try_new`, `state.workspaces` is non-empty only when `persist::load` parsed a `session.json` and restore produced at least one workspace. So nothing is pruned after a failed, missing or newer-version load, after a restore in which every workspace failed, or under handoff (whose workspaces arrive after `try_new`).
3. **Record cleanup.** An empty record (5.1) is dropped.

Records whose ids are absent at a startup that did not restore a session stay in the file untouched. They are pruned at the next startup that does restore one.

### 5.6 Accepted consequences of D1

- **After running upstream herdr in between:** workspaces upstream created come back ungrouped; records of workspaces upstream closed are pruned at the fork's next successful startup. Rarely, upstream reissues an id (it closed the highest-numbered workspaces, restarted, and created new ones), and the old record attaches to the new workspace: wrong group or color, fixed by one drag.
- **After a failed or newer-version `session.json` load:** records stay, but new workspaces are numbered from `w1` again and adopt old records. Editing or closing such a workspace changes or prunes the old record, and the next startup that restores any workspace prunes every record not in that session. To recover, stop the server before creating any workspace and copy the preserved session (upstream keeps the last 3 in `session-backups\`) back to `session.json`. The records then reattach.
- A misattached record can also carry a `herdr up` project binding. `herdr up` checks that binding against the workspace's cwd before trusting it (D17; section 11.4).

### 5.7 Other state

- **session.json:** `SessionSnapshot`, `WorkspaceSnapshot`, `migrate_*` and `parse_snapshot` are untouched; `SNAPSHOT_VERSION` stays 3. The only fork effect is that `workspaces[]` is written in group order.
- **Collapse state:** client presentation, using the existing `ClientShellState.collapsed_groups` / `remote_collapsed_groups` with keys `group:<group_id>`, persisted by the existing `persist_chrome_preferences` into the client-shell preferences file under `%LOCALAPPDATA%\herdr\client-shell\`.
- **projects.toml:** `config_dir()\projects.toml`, read only by `herdr up`. herdr never writes or watches it.
- **config.toml:** no new keys.
- **Undo stash:** client memory only (the last group deletion); never persisted.
- **`~/.claude`:** never read or written.

---

## 6. Ordering invariant and reconcile

### 6.1 Invariant I1 (server)

`AppState.workspaces` is in region order: members of `groups[0]`, then `groups[1]`, and so on, then every ungrouped workspace.
- A workspace's region is its own effective group. Linked worktree children have their own records.
- Inside a region, upstream's relative order is preserved.
- Empty groups occupy no indices.

**Normalize** takes a stable sort by region rank. If the result differs from the current order, it calls the existing `AppState::move_workspace_block(&all_ids_in_desired_order, None)` (actions.rs near :507), which keeps `active` and `selected` by id, marks the session dirty, and returns false when nothing changes.

### 6.2 Where it runs

1. **`workspace_group.edit` handler**, after `plan_edit` commits (section 7.3).
2. **`App::reconcile_workspace_groups() -> bool`**, called on the first line of `App::sync_session_save_schedule` (app/session.rs near :21). The headless loop calls that once per iteration, right after the API drain (headless.rs near :494).

Reconcile is triggered by a change in the live id sequence, or by pending prunes once the session save has settled, not by `session_dirty`. A workspace that closes through its last pane exiting goes through `finish_checkpointed_pane_exit`, which clears `session_dirty`, so a dirty-flag gate would miss that close.

```text
reconcile_workspace_groups():
  live = ids of state.workspaces, in order
  if last_order == Some(live) && (pending_prune.is_empty() || !session_save_settled()):
      return false                                    // O(n) compare, no allocation
  initial = last_order.is_none()
  pending_prune.extend(last_order − live)             // in-process close (5.5 rule 1)
  if session_save_settled():                          // session.json no longer lists them
      drop records in pending_prune; pending_prune.clear()
  if !initial:
      for id in live − last_order, linked child, no record: infer group (5.1)
  normalize()                                         // 6.1
  if records changed and persist_session: save sidecar
  if order changed: emit workspace.reordered {workspace_ids: full order, before_workspace_id: None, workspaces}
  if anything changed: render_dirty.request_generic(); render_notify.notify_one()
  last_order = Some(ids after normalize)
```

The render request matters. Without it, clients would keep the pre-normalization projection: `drain_server_events` already rendered in the previous iteration and a timer wake-up does not render. The pattern is the one app/api.rs already uses (near :263).

The first iteration after startup runs the same code with `initial = true`, so a `session.json` reordered by upstream is regrouped, and restored linked children without records stay ungrouped (an ungrouped child has an empty record, which cleanup drops, so inferring at startup would re-group a child the user ungrouped).

Paths covered without hooking them: `workspace.create` (appends to the ungrouped tail), worktree create and open, every close path, pane-move rollback, and `workspace.move` / `workspace.move_block` from group-unaware clients (api/workspaces.rs `handle_workspace_move`, `handle_workspace_move_block`). Those moves are snapped back into their regions one iteration later and re-projected.

### 6.3 Consequences

- `session.json`, `herdr workspace list`, the collapsed rail and public workspace numbers (index + 1) all follow region order.
- Inside a region, worktree families render as upstream draws them (parents, then children), so a family's numbering can be non-monotonic on screen, exactly as upstream.
- Upstream herdr reading `session.json` sees the same order, flat.

### 6.4 Display invariant I2 (client)

`grouped_entries()` never trusts the Vec order. It buckets workspaces by the snapshot's membership (groups in snapshot order, then ungrouped, relative order preserved). Inside each region it applies upstream's family rule from `workspace_entries`, scoped to that region:
- a worktree key forms a family in a region only if the region holds at least one linked and at least one non-linked workspace with that key;
- parents are emitted at the first member's position, then children, the last marked `last_child`;
- collapsing the worktree key hides that region-family's children, except a focused linked child.

A linked child whose parents all sit in other regions renders top-level in its own region and is never hidden by a worktree collapse. This matters when one git repository holds many project folders: every workspace in it shares one worktree key (the canonical git common dir).

Accepted edge case: when a family is split across groups, upstream's worktree toggle and the status a collapsed parent rolls up still span the whole family, children in other regions included.

### 6.5 Op semantics

- **Place** (used by drop, keyboard and `herdr up`):
  - Placing a non-linked workspace also moves the linked children with the same key in the same region, but only when it is the only non-linked member of that key there. Otherwise children stay put: `WorktreeSpaceMembership` has no parent pointer, so the server cannot tell which parent a child came from.
  - Placing a linked child (Organize menu or keyboard) moves only that child. Linked children stay non-draggable by mouse, as upstream.
  - If `before_workspace_id` is in the target region and not in the moved set, the unit goes before it; otherwise it goes to the end of the region.
- **Delete group:** members become ungrouped at the head of the ungrouped region; a bound template is added to `deleted_templates`.
- **Move group:** reorders `groups`; moving a group before itself is a no-op.
- **Create:** appends the group (or inserts it before `before_group_id`), clears a matching tombstone, then places `workspace_ids` in order.
- **Bind:** `bind{project: p}` on workspace W clears `p`, and that record's tab bindings, from every other workspace (project names compared case-insensitively). W keeps its own tab bindings, even when it was bound to another project before. Only `bind{project: null}` clears W's project and tab bindings. `herdr up` never depends on the kept bindings: it binds only workspaces it creates or that are unbound, and clears a stale binding with `bind{project: null}` first (11.4 step 3.1).

**Batch atomicity:** every op is validated on a clone, and nothing commits if any op fails. There is no protection against stale-but-valid ops from a second host: ops carry ids only, and the endpoint `boot_id` check detects restarts, not drift. A stale drag lands relative to the current order.

**Ungrouped region:** always last; it is the inbox for new workspaces.

---

## 7. API and CLI

### 7.1 Methods

Two lines in `define_operations!` (src/api/schema.rs), inserted directly after `WorkspaceClose`, not at the tail:

```text
WorkspaceGroupList(EmptyParams) => "workspace_group.list", false, false;
WorkspaceGroupEdit(WorkspaceGroupEditParams) => "workspace_group.edit", true, true;
```

The two flags are `changes_ui` and `client_shell` (macro in src/api/schema/operations.rs): `list` changes no UI and is not offered to client shells; `edit` does both.

`Method` is serde-tagged (`tag = "method"`), so position is wire-neutral, and the client-shell method list is sorted automatically. A mid-list anchor avoids conflicting with every upstream method added at the tail. Only `edit` is admitted for client shells; `list` serves `herdr up`.

### 7.2 Schema

New file `src/api/schema/workspace_groups.rs` (serde + schemars):

```rust
pub struct WorkspaceGroupEditParams { pub ops: Vec<WorkspaceGroupOp> }

#[serde(tag = "op", rename_all = "snake_case")]
pub enum WorkspaceGroupOp {
    Create   { name: String, color: Option<String>, template: Option<String>,
               #[serde(default)] workspace_ids: Vec<String>, before_group_id: Option<String> },
    Update   { group_id: String, name: Option<String>, color: Option<String>, template: Option<String> },
    Delete   { group_id: String },
    Move     { group_id: String, before_group_id: Option<String> },
    Place    { workspace_id: String, group_id: Option<String>, before_workspace_id: Option<String> },
    SetColor { workspace_id: String, color: Option<String> },        // None = use group color
    Bind     { workspace_id: String, project: Option<String> },      // None also clears tab bindings
    BindTab  { workspace_id: String, key: String, tab_id: Option<String> }, // tab must belong to the workspace
}

pub struct WorkspaceGroupInfo { group_id, name, color, template: Option<String> }
pub struct WorkspaceGroupMemberInfo {
    workspace_id, group_id: Option<String>, color: Option<String> /* override */, project: Option<String>,
    cwd: String,  // Workspace::resolved_identity_cwd_from, for herdr up adoption
    #[serde(default)] linked: bool,
    #[serde(default)] tabs: Vec<WorkspaceGroupTabBinding /* { key, tab_id } */>, // live bindings only
}
```

- One new response variant, `ResponseResult::WorkspaceGroups { groups, workspaces /* display order */, #[serde(default)] deleted_templates }`, placed near `WorkspaceInfo`. `ResponseResult` is `tag = "type"`, so position is irrelevant.
- Errors use the existing `{code, message}` shape: `workspace_not_found`, `tab_not_found`, `workspace_group_not_found`, `workspace_group_name_taken`, `invalid_workspace_group_name`, `invalid_color`, `too_many_tab_bindings`.

### 7.3 Handlers

New file `src/app/api/workspace_groups.rs`, plus `mod workspace_groups;` and 2 dispatch arms in src/app/api.rs placed right after the `workspace.*` arms (not before `_ =>`).

`handle_workspace_group_edit`:
1. Run `reconcile_workspace_groups()` first, so closes and new workspaces from earlier in the same loop iteration are pruned or inferred before step 6 sets `last_order`.
2. Resolve public ids (`parse_workspace_id`, `parse_tab_id`).
3. `plan_edit` on a clone with `now_ms`. Any error returns `{code, message}` and leaves state untouched.
4. Commit the model, then `move_workspace_block(desired_order, None)`.
5. Save the sidecar if it changed and `persist_session` is set.
6. Emit `workspace.reordered` if the order changed (same payload as api/workspaces.rs), set `last_order`, and return `ResponseResult::WorkspaceGroups`.

`changes_ui = true` makes the headless loop re-project. No new `EventKind`.

`Method::WorkspaceGroupEdit(_)` is added to `shell_endpoint_claims_geometry` (src/server/headless/client_views.rs), next to `Method::WorkspaceMove(_) | Method::WorkspaceMoveBlock(_)`. A group drag replaces upstream's `workspace.move` drag in fork clients, so the client the user acts in claims shell-tab geometry exactly as it does for an upstream drag.

### 7.4 Client projection

New file `src/protocol/workspace_groups.rs`:

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientShellWorkspaceGroups {
    #[serde(default)] pub groups: Vec<ClientShellWorkspaceGroup>,
    #[serde(default)] pub workspace_colors: Vec<(String, String)>, // workspace_id -> effective color
}
pub struct ClientShellWorkspaceGroup {
    pub group_id: String, pub name: String, pub color: String,
    #[serde(default)] pub template: Option<String>,   // lets client undo restore the binding
    #[serde(default)] pub workspace_ids: Vec<String>, // explicit members incl. linked children
}
pub const WORKSPACE_COLORS: [(&str, [u8; 3]); 9];
```

- `ClientShellSnapshot` (protocol/wire.rs near :916) gets a new last field, `#[serde(default)] pub workspace_groups: ClientShellWorkspaceGroups`. Use `serde(default)` only, never `skip_serializing_if`, so the bincode roundtrip test stays valid.
- Snapshots travel to clients as JSON (`protocol::endpoint::snapshot_message`), so older snapshots decode to the empty default.
- The field must be added at the 6 literal sites: `server/client_shell.rs`, `protocol/wire.rs` (test), `protocol/endpoint.rs` (test), `client/shell/tests/mod.rs`, and 2 in `client/endpoint/activation_tests.rs`.
- The server returns the default immediately when there are no groups and no colors.
- An unknown color name resolves to no bar.

### 7.5 Contract tests and schema

No fixture is re-blessed.
- `operation_ui_impact_preserves_existing_classification` (api/schema/operations.rs) gains `"workspace_group.edit"` after `"workspace.close"`. It compares sets, so position does not matter.
- `advertised_client_shell_method_shapes_stay_at_the_v1_contract` (server/client_commands.rs) gains its own pin, `assert_eq!(actual.remove("workspace_group.edit").as_deref(), Some("<digest>"))`, placed before upstream's pins, following the `pane.clear` pattern. To fill it, set the pin to `Some("")`, run `just test-one advertised_client_shell_method_shapes_stay_at_the_v1_contract`, and paste the digest from the failing assertion's left value.
- Regenerate `docs/next/api/herdr-api.schema.json` with `$env:HERDR_UPDATE_API_SCHEMA='1'; just test-one generated_protocol_schema_artifact_is_current; Remove-Item Env:HERDR_UPDATE_API_SCHEMA`. Never leave the variable set: while it is set, `generated_protocol_schema_artifact_is_current` rewrites the file and passes, hiding a stale schema from `just check`.
- These must stay green unchanged: `frozen_generation_one_snapshot_decodes`, `snapshot_json_tolerates_future_fields_and_command_actions`, the wire bincode roundtrip.

### 7.6 Compatibility

- Fork client, upstream server: the method is not advertised, so organize actions show the existing "Action unavailable" notice and no groups are drawn; drags fall back to upstream `workspace.move`.
- Upstream client, fork server: it ignores the method and the field. Its moves are normalized one loop iteration later.

### 7.7 CLI wiring

- Only one new command, `herdr up`, in new file `src/cli/up.rs`, wired with `mod up;` and an `"up"` arm in `cli::maybe_run`.
- It is not added to the clap root. `spec::print_requested_help` returns false for unknown root commands, so `herdr up --help` is handled inside the arm, and the CLI contract tests stay unchanged.
- No `herdr group` command.

---

## 8. Client sidebar

### 8.1 Entries

One 3-line hook at the top of `workspace_entries` (client/shell/sidebar.rs near :458):

```rust
if let Some(entries) = super::workspace_groups::grouped_entries(snapshot, collapsed_groups) {
    return entries;
}
```

- `grouped_entries` returns `None` when the snapshot has no groups, so upstream's body runs unchanged.
- With groups it builds entries per region (6.4), and a collapsed `group:<id>` hides all members except the focused workspace, which is then emitted un-indented.
- Every caller gets this with no edits: keyboard navigation (`navigation_workspace_entries`, which passes the real collapsed set outside the mobile layout), number keys, `move_navigate_workspace`, mouse hit-testing and both renderers.
- **Mobile and endpoint navigation** (`mobile.rs`, `endpoint_navigation.rs`) pass empty collapsed sets, so they show the grouped order flat with no headers. The mobile layout applies at or below `ui.mobile_width_threshold` columns (default 64).
- **Characterization test:** with one empty group present and every workspace ungrouped, `grouped_entries` must equal upstream `workspace_entries` for every existing worktree fixture and collapse state. This catches upstream drift in the family rule at each rebase.

### 8.2 Group headers

Headers are decorations, not entries.
- `GroupLayout::new(snapshot, &entries, collapsed)` is computed once per frame from each entry's region.
- A region's header is drawn above its first visible entry. Headers of empty groups, and of collapsed groups with no visible member, stack above the next visible entry or trail after the last one. With no entries at all, headers start at the top of the list.
- Header rows are added to the entry's row height, so scroll metrics and reveal stay entry-based.
- Header row: `▌` in `readable(group color)`, the name in `palette.text` bold, `▾`/`▸` in `palette.accent` at the right, `surface1` background while it is a drop target. When collapsed it also shows ` (n)` in `overlay0` and the most urgent member status (existing `status_icon` / `status_color` / `status_priority`), so attention stays visible.
- The "ungrouped" header is dim `overlay0` text with no bar or toggle. It shows only when a group exists and either ungrouped workspaces are visible or a workspace drag is in progress.

### 8.3 Color bar

- `▌` (U+258C) in `readable(effective color)` at column 0 of every card line, on label rows and tab rows. Upstream leaves that column blank.
- Only the symbol and foreground are set, so selected, focused and dragged backgrounds are kept.

### 8.4 Tab rows (D3: 2 or more tabs only)

Source: wkentaro/herdr `29484fc4` (based on v0.9.0 `b99002ac`). The raw commit does not apply at `5bd900d` (conflicts in sidebar.rs, endpoint_sidebar.rs, mouse.rs, tests/endpoints.rs and tests/client_mode.rs; for example sidebar.rs now matches selection with `state.selected_workspace_id.is_some_and(|target| target.matches(...))`), and no resolved patch is kept in the repository. Port it with `git cherry-pick -n 29484fc4`, keep only the src/client/shell/{sidebar,endpoint_sidebar,mouse,render,state}.rs hunks, resolve them against master, move `group_workspace_tabs` and `render_workspace_tabs` into `sidebar_tabs.rs`, add the `len() >= 2` filter, and drop the test and `.mdx` hunks.

- `group_workspace_tabs` and `render_workspace_tabs` move into new `src/client/shell/sidebar_tabs.rs`, with an Apache-2.0 attribution header (section 15.5).
- `group_workspace_tabs` keeps only workspaces with 2 or more tabs (a `len() >= 2` filter). It is shared by `render_sidebar` and `endpoint_sidebar::render_expanded`.
- A single-tab workspace gets no tab rows, so its card is exactly upstream's, apart from the color bar (8.3).
- `workspace_rows` is not modified (D11). The workspace row keeps upstream's aggregate status for every workspace, including the status rolled up into a collapsed worktree parent. The port's hunk that stripped `StateIcon`/`StateText` is dropped.
- Each tab row shows a tree indent, `├─` or `└─`, the per-tab status icon from `tab.agent_status` (Working yellow, Blocked red, Done teal, Idle green; existing helpers), and the label. The tab the client is on is bold on `active_row_bg`.
- `WorkspaceHit.card_bottom` records the bottom of the whole card; `WorkspaceHit.rect` still covers only the label rows.
- Clicking a tab row calls the existing `focus_or_activate(endpoint, Tab(tab_id))` and never starts a drag: `TabFocus` on the same endpoint, `ActivateEndpoint` for a remote one.
- `ShellHitMap.sidebar_tabs` is cleared when mouse capture is off.
- Not applied from the port: the edits to upstream tests (`agents_worktrees_notifications.rs`, `endpoints.rs`, `mouse_selection.rs`, `tests/client_mode.rs`) and the website `.mdx` hunks. The port's own tests move into the fork's test file and are adjusted to the 2+ rule.

### 8.5 Tab bar

`active_tab_colors(snapshot, palette)` (3 lines at the focused-tab style in client/shell/tabs.rs): with no color, upstream's (`palette.accent`, `panel_contrast_fg`); otherwise the raw named RGB with white text, at least 4.92:1.

### 8.6 Collapsed rail and other paths

- The collapsed sidebar (`render_collapsed_sidebar`) shows numbers in group order and no colors or headers.
- The multi-machine sidebar (`render_expanded`, used for every section, the local machine included, whenever a remote machine is configured) gets the same header, bar, slot and tab-row calls per machine section, using that endpoint's snapshot and collapse set. A section whose endpoint is an upstream server shows no groups. An offline machine's color bars and group headers are dimmed like upstream dims its stale cards (intended).
- Cost: O(workspaces + tabs + groups) per frame.

---

## 9. Colors

### 9.1 Palette

9 names, each at least 4.5:1 on `#ffffff`, so VS Code's default `terminal.integrated.minimumContrastRatio` (4.5) leaves them unchanged on white cells. On catppuccin-latte's selected, focused and dragged row backgrounds it adjusts some bars slightly (9.3). Contrast values recomputed for this spec (WCAG 2.x).

| name | hex | vs `#ffffff` | vs `#1e1e2e` (dark) |
|--|--|--|--|
| red | `#c81e1e` | 5.74 | 2.86 |
| orange | `#c2410c` | 5.18 | 3.17 |
| yellow | `#a16207` | 4.92 | 3.33 |
| green | `#15803d` | 5.02 | 3.27 |
| teal | `#0f766e` | 5.47 | 3.00 |
| blue | `#1d4ed8` | 6.70 | 2.45 |
| purple | `#7e22ce` | 6.98 | 2.35 |
| pink | `#be185d` | 6.04 | 2.72 |
| gray | `#57534e` | 7.63 | 2.15 |

- Minimum pairwise ΔE76 is 16.3 (red vs orange).
- No hex input is accepted; the server validates names only (D9).

### 9.2 `readable(rgb, cell_bg, host_bg)`

Used for header bars and row bars; re-evaluated every frame.
- **Background:** the cell's real background: `palette.sidebar_bg` for plain rows and headers, `selection_bg` for the selected card, `surface1` while dragged or a drop target, and `active_row_bg` for the focused card (sidebar.rs near :317). Every built-in theme sets `sidebar_bg = Color::Reset`, so for `Reset` use the client's host background: the OSC 11 default background if the host reported one (`terminal_theme` `background`), else the host appearance (Dark → assume `#1e1e2e`, Light or unknown → assume `#ffffff`). For `Rgb`, use the value.
- **Blend:** 15% per step toward black when the background is at least as light as the bar color (WCAG relative luminance), otherwise toward white, until contrast is at least 3:1, at most 8 steps. Comparing with the bar color rather than a fixed light/dark split also lets mid-tone backgrounds (for example dracula's `surface1`) reach 3:1.
- On a white host background all 9 stay raw (at least 4.92:1). On catppuccin-latte, `selection_bg` (3.17-4.91:1) and `active_row_bg` (4.05-6.27:1) need no blending; on `surface1` (2.71-4.20:1) orange, yellow and green are blended.
- Text is never colored. Group names and workspace and tab labels keep the theme tokens.

### 9.3 VS Code

Leave `terminal.integrated.minimumContrastRatio` at its default 4.5. On an unhighlighted white row nothing changes, because every color is at least 4.92:1. On catppuccin-latte selected, focused and dragged rows several colors fall to 2.7-4.5:1, so VS Code adjusts them and those bars look slightly different from Windows Terminal. `colors_stay_distinct_after_simulated_vscode_min_contrast` ports xterm.js `ensureContrastRatio` and requires a pairwise ΔE76 after adjustment of at least 14 against `#ffffff`, `#1e1e2e` and latte `selection_bg` and `active_row_bg`, and of at least 13.5 against latte `surface1`. `surface1` is only the transient background of a dragged row or a drop target; there the palette measures 13.68 (red vs orange, with `readable()` applied first), so 14 cannot hold.

### 9.4 Fork owner's theme

`[theme] name = "catppuccin-latte"`, set explicitly. With `auto_switch`, herdr falls back to the dark palette when the host's appearance is not detected (app/mod.rs near :325). Switch to `auto_switch = true` only after the smoke checklist confirms both hosts report their appearance.

Glyphs `▌`, `▾`, `▸` are present in Cascadia Mono and Consolas.

---

## 10. Interactions

### 10.1 Mouse

About 9 call-outs in client/shell/mouse.rs; the logic lives in client/shell/workspace_groups.rs.

**Headers**
- Press on a header (checked before upstream's worktree group-toggle handling) records the press.
- Release without moving toggles `group:<id>` and calls `persist_chrome_preferences`.
- Moving 1 or more cells starts `ClientChromeDrag::WorkspaceGroup`. Drop targets are the row above each group header plus an end slot: the row above the "ungrouped" header when it is drawn, else the row below the last card (tab rows included). Dropping sends `{ops:[{op:"move", group_id, before_group_id}]}`; a drop where the group already is sends nothing. composition.rs draws the accent drop line. Only the active endpoint's groups can be dragged.
- Right-click (checked before the workspace menu) opens the group menu: Rename group..., Group color..., Collapse / Expand, Move group up, Move group down, Delete group, and Undo delete group while one is stashed.

**Workspace drag**
- Upstream press, drag threshold and draggability are unchanged; linked children stay non-draggable.
- `workspace_drop_target_at` asks `workspace_groups::drop_target_at` first; it returns `None` without groups, so upstream runs untouched. Otherwise it picks the nearest recorded drop slot: above each top-level card; the end of each region; each header (top of the group, or append when collapsed or empty); the ungrouped header or tail (ungroups).
- Dropping sends `place`; a drop on the workspace's own slot sends nothing. Rows of another machine are rejected, as upstream does.

**Tab rows:** a click focuses the tab (8.4).

**Workspace right-click:** "Organize..." is appended to every workspace menu (tail of `items()` in context_menu.rs).

### 10.2 Keyboard

One 3-line hook in `route_navigate_key` (client/shell/input.rs), placed after user and prefix binding resolution fails, so user bindings, navigate keys and digits always win. Enter navigate mode with the existing `prefix+w`.
- **Shift+Up / Shift+Down** move the selected workspace one slot in grouped display order, as a `place`. Inside a group it swaps with its neighbor; from the top or bottom of a group it joins the end of the previous section or the start of the next one, including empty groups. Entering a collapsed group expands it (client-local). Navigate mode and selection are kept, so the key repeats. A linked child shown under its parents only changes regions this way (to the end of the previous one or the start of the next one); it never swaps within a region.
- **m** opens the Organize menu for the selected workspace, anchored at its row, and leaves navigate mode, so the menu takes the keys.
- Members of a collapsed group (except the focused one) are not selectable until it is expanded, matching what is drawn.

### 10.3 Organize menu

The same menu for `m` and right-click; all labels are `&'static str` (upstream's `ClientContextMenuItem.label` type is unchanged).
- **Move to group...** opens the existing Rename overlay titled "move to group", prefilled with the current group. An existing name (case-insensitive) sends `place`; a new name sends `create{name, workspace_ids:[W]}`; an empty name sends `place{group_id: null}`.
- **Workspace color...** opens a submenu with the 9 names plus "Group color", which clears the override.
- **Edit group...** opens the Rename overlay titled "edit group", prefilled with W's group. Enter opens that group's menu. An unknown name shows the existing Rejected notice (title "Action rejected", body "No group named X"). This makes empty groups, and groups W is not in, fully editable from the keyboard.
- The group items appear directly when W is grouped.
- **Undo delete group** appears while a deletion is stashed.
- **Expand all groups** appears when any group is collapsed.
- **Move group up** on the first group and **Move group down** on the last one send nothing.

### 10.4 Undo for Delete group

- Before sending `delete`, the client builds the inverse from the snapshot: `create{name, color, template, workspace_ids: members in order, before_group_id: next group}`, and stashes it as `organize_undo` with the endpoint id.
- "Undo delete group" sends it, restoring name, color, template binding (which clears the tombstone), membership, member order and group position. Workspace color overrides were never removed. Collapse state resets, because the new group id is new.
- The stash is cleared by any other organize edit, a change of the endpoint's `boot_id`, or using it successfully.
- At undo time the client drops members no longer in the current snapshot and uses `before_group_id` only if that group still exists (else it appends). If the server rejects the batch (for example because the name is now taken), the client shows the existing Rejected notice with the server message and keeps the stash.

### 10.5 Plumbing

All prompts reuse `ClientShellOverlay::Rename`; one arm in `save_rename_overlay` dispatches `OrganizePrompt`. Menus reuse `ClientContextMenuOverlay`. New client state (client/shell/state.rs, about 20 lines): `ShellHitMap.sidebar_tabs`, `WorkspaceHit.card_bottom`, `ShellHitMap.workspace_groups`, `workspace_drop_slots`, `workspace_group_press`, `organize_undo`, `ClientChromeDrag::WorkspaceGroup`, `ClientContextMenuTarget::Organize`, `ClientContextMenuAction::Organize` (Copy, u8 color indices), `ClientRenameTarget::Organize`. `OrganizeMenu::Workspace` and `OrganizeMenu::Group` carry an `OrganizeMenuOptions` captured when the menu opens (group items, Collapse or Expand, Undo delete group, Expand all groups), because `ClientContextMenuOverlay::items` has no access to the shell state.

### 10.6 Coverage

| Action | Mouse | Keyboard |
|--|--|--|
| Create group | Organize → Move to group (new name) | `m` → Move to group |
| Rename, recolor, delete group | header right-click | `m` → group items or Edit group... |
| Move workspace | drag | Shift+Up/Down, or Move to group |
| Reorder groups | header drag, or Move group up/down | Edit group... → Move group up/down |
| Collapse / expand | header click | Edit group... → Collapse; Expand all |
| Workspace color | Organize → Workspace color | `m` → Workspace color |

Everything persists server-side except collapse, which persists in client preferences.

---

## 11. projects.toml and `herdr up`

### 11.1 File

`%APPDATA%\herdr\projects.toml`, parsed with `toml` + serde, `deny_unknown_fields`. Arrays keep file order. A copy of this example ships as `docs/next/fork/projects.example.toml`.

```toml
[[group]]
name = "Work"            # required, unique (case-insensitive); also the template binding key
color = "blue"           # optional; one of: red orange yellow green teal blue purple pink gray

[[group]]
name = "Personal"
color = "green"

[[group]]
name = "Infra"
color = "gray"

[[project]]
name = "api"             # required, unique; selector and binding key
label = "API"            # optional (default = name); applied at creation or with --reapply
cwd = '~\src\api'        # required; must exist
group = "Work"           # optional; must name a [[group]]
color = "teal"           # optional per-workspace override

  [[project.tab]]
  label = "agent"        # required, unique within the project; also the tab binding key
  agent = "claude"       # optional: start an agent of this kind through agent.start
  args = []              # optional, only with agent
  # agent_name = "api-agent"   # optional; default "<project>-<label>"

  [[project.tab]]
  label = "dev"
  command = "npm run dev"     # optional; exclusive with agent; typed into the shell plus Enter
  # cwd = '~\src\api\server'  # optional; default = project cwd

[[project]]
name = "web"
cwd = '~\src\web'
group = "Work"
  [[project.tab]]
  label = "agent"
  agent = "claude"

[[project]]
name = "docs"
cwd = '~\src\docs'
group = "Personal"

[[project]]
name = "infra"
cwd = '~\src\infra'
group = "Infra"
  [[project.tab]]
  label = "agent"
  agent = "claude"
```

### 11.2 Command line

```text
herdr up <project>...      # named projects (case-insensitive)
herdr up --group <group>   # every project in that group; (re)creates the group even if tombstoned
herdr up --all             # every [[group]] and [[project]]
  --reapply                # re-assert file label/group/color on bound workspaces and name/color on bound groups,
                           # recreate tombstoned groups
```

- Exactly one selection mode is required.
- Exit codes: 0 when every selected project is applied or up to date; 1 on a server or API failure (the run stops at the first failed request and prints what was already applied); 2 on a usage or file error, detected before any API call; 3 when the run completed but skipped something (missing cwd, ambiguous adoption, group name taken, agent name taken, or an `agent.start` still busy after 15 s).
- Server not running: prints the existing `server_not_running` guidance. `up` never spawns a server, so it never decides the server's environment (section 12).
- Upstream server (`workspace_group.list` answered with error code `invalid_request`; upstream returns that for any method it cannot parse, and `unknown_method` only for the retired `pane.graphics.*` methods): prints "server does not support workspace groups (fork server required)" and exits 1.
- `up` never changes focus. It works from any shell; panes inherit the server's environment, not `up`'s.

### 11.3 Validation (before any API call)

- Unique group, project and tab names. Group and project names and tab labels are compared case-insensitively everywhere (uniqueness, selection, bindings); bindings store the file's spelling.
- Every `group` reference resolves.
- Colors come from the 9 names.
- Every cwd is absolute after expanding `~` to `%USERPROFILE%`. A project whose cwd does not exist is reported and skipped (exit 3).
- A `cwd` on a project's first `[[project.tab]]` is a file error (exit 2): the root tab always starts in the project cwd.
- `agent` and `command` are mutually exclusive; `args` requires `agent`.
- Agent names follow upstream's rule (`valid_agent_name` in app/agents.rs: a lowercase letter, then lowercase letters, digits, `-` or `_`, 1-32 characters) and are unique across the file. A derived name that fails `valid_agent_name` (too long, uppercase, spaces) is a file error that names the tab and asks for an explicit `agent_name`. Upstream agent names are unique across the whole server, so before launching, `up` checks `agent.list` and treats a name that is live outside the bound workspace as a conflict (11.4 step 4.4, Agent).

### 11.4 Algorithm

A pure `plan(file, selection, state, reapply) -> Vec<Step>` plus an executor over `cli::send_request`.

1. **Read state:** `workspace_group.list` (groups with template, `deleted_templates`, members with group, color override, project, cwd, linked flag and tab bindings), `workspace.list` (labels), `agent.list` (agent name to tab), and `tab.list` for each matched workspace.
2. **Groups** (those referenced by the selected projects, the selected group, or all with `--all`):
   1. match an existing group with `template == name`;
   2. else, if the name is in `deleted_templates` and neither `--group <it>` nor `--reapply` was given, skip it: its projects are created ungrouped, with the note `group Infra was deleted in the UI; run herdr up --group Infra to restore it`;
   3. else adopt an unbound group with the same name (case-insensitive) via `update{template}`;
   4. else `create{name, color, template}`.
   With `--reapply`, also `update{name, color}` where they differ.
   If the name is held by a group bound to another template, print `group Work: name taken by a group bound to Personal; skipped`, create its projects ungrouped, and exit 3.
3. **Workspaces:**
   1. The member bound to the project (`project == name`), **if its cwd equals or lies inside the project cwd** (both through `worktree::canonical_or_original`, compared case-insensitively). If it lies outside, the binding is stale (D17): print `warning: api was bound to w5 (cwd ...); rebinding`, queue `bind{workspace_id: w5, project: null}` (which also clears its tab bindings), and continue with rules 3.2-3.4 with w5 excluded from adoption in this run. The planner applies queued ops to its working copy of the state, so later rules see the result.
   2. Else adopt the single unbound, non-linked member whose cwd equals the project cwd. Cwd comes first because automatic labels are the repository root name, so many unrenamed workspaces share a label. Several matches: fall through to rule 3.3.
   3. Else adopt the single unbound, non-linked member whose label matches the project label (case-insensitive). Several matches: report the project as ambiguous and skip it (exit 3).
   4. Else create it with `workspace.create {cwd, label, focus: false}`.

   Group, color and label are applied only in the run that creates or first binds the workspace:
   - on create: one batch `[bind, place{group}?, set_color?, bind_tab{key: first tab label, tab_id: root tab}]`, then `tab.rename` of the root tab. If the project has no `[[project.tab]]`, the batch omits `bind_tab` and `tab.rename`, and nothing is launched;
   - on adoption: `[bind, place{group} only if currently ungrouped, set_color only if no override]`.

   After that, plain `up` never moves, recolors or renames a bound workspace, so UI ungrouping, Delete group and "Group color" all survive. `--reapply` re-asserts them (`workspace.rename`, `place` at the end of the group, `set_color`).
4. **Tabs**, in file order, inside the bound workspace:
   1. a binding key that points at a live tab number: the tab exists. If that tab is the root tab of a workspace created earlier in this same run, launch it exactly as in rule 4.4 (Agent or Command); otherwise launch nothing;
   2. an agent spec whose `agent_name` is live in this workspace: `bind_tab` to that agent's tab;
   3. an unbound tab whose label matches (case-insensitive): `bind_tab`;
   4. otherwise `tab.create {workspace_id, cwd, label, focus: false}`, then `bind_tab`, then launch:
      - **Agent:** `agent.start {name, kind, pane_id: root pane, args}`. This is upstream's path: it refuses a busy shell, quotes argv for the shell and marks the pane as a managed agent (name + kind). Name and kind are written to session.json once the launch is no longer pending; argv is not persisted, and a resume session is recorded only for `codex resume <id>` or from integration hook reports. On `agent_pane_busy` (for example while a PowerShell profile is still initializing) retry every 250 ms for up to 15 s, then print `run: herdr agent start <name> --kind <kind> --pane <id>`, followed by ` -- <args...>` when `args` is non-empty, and continue (exit 3). Upstream's own `herdr agent start` retries that error for about 2 s (cli/agent.rs `agent_start`); `up` uses a longer budget because a fresh tab's profile can take longer. On `agent_name_taken`, or when the pre-launch `agent.list` check finds the name live in another workspace (names are server-wide), do not retry: print `tab <label>: agent name <name> is already used by <pane_id> in <workspace_id>; set agent_name in projects.toml` and continue (exit 3).
      - **Command:** `pane.send_input {pane_id, text, keys: ["Enter"]}`. `keys` must be explicit: over JSON it defaults to empty (`PaneSendInputParams`); `["Enter"]` is only the CLI projection default.

   Launches happen only for tabs created in this run: tabs from `tab.create` (rule 4.4) and the root tab of a workspace from `workspace.create` (step 3.4).
5. **Output:** one line per action, for example `project api: adopted w4 by cwd`, `  tab agent: created, agent api-agent started`, `project web: up to date (w2)`.

### 11.5 Idempotency

After a successful run, groups resolve by template, workspaces by binding and tabs by binding (tab numbers are never reused within a workspace), so a second run prints "up to date".
- A tab renamed in the UI is still found, and no second agent starts.
- A closed tab or workspace is recreated; to retire one, remove it from the file.
- After a server restart, upstream restores workspaces, tabs and managed-agent records, and the bindings come back from the sidecar. Tabs are found by their sidecar tab binding; the agent-name rule (4.2) only matches agents that were active at the last session save.

The file and the UI do not fight: the file is read only on an explicit `up`; `up` never deletes or closes anything and never moves a workspace it has already bound (except with `--reapply`); it writes file values only at create, first bind or `--reapply`; it respects tombstones; groups and workspaces not in the file are never touched.

Panes run the unmodified agent CLI from PATH in the project cwd, so Claude Code stores transcripts where it always does, keyed by that folder, and its VS Code extension can list and resume them.

---

## 12. Hosting: Windows Terminal and VS Code

Both hosts run the same `herdr.exe` client against one server. Server state (groups, order, colors, bindings) is shared live. Collapse state lives in one preferences file per socket: last writer wins, and it is not reloaded live. The fork ships no host-specific code.

### 12.1 Clean launcher (required)

The auto-spawned server inherits the whole environment of whichever client starts it, and panes inherit the server's environment. Upstream strips only a few agent-session variables from panes (`apply_pane_launch_env` in pane.rs) and deliberately keeps `ANTHROPIC_API_KEY`. So an API key exported in the parent environment would be inherited by every agent pane, and a server first started from a VS Code terminal would hand every pane that window's `GIT_ASKPASS` and IDE-bridge variables, which break when the window closes.

`docs/next/fork/herdr-clean.cmd`, copied next to `herdr.exe`. As a batch file it needs no execution-policy change:

```bat
@echo off
setlocal
set ANTHROPIC_API_KEY=
set GIT_ASKPASS=
set CLAUDE_CODE_SSE_PORT=
set ELECTRON_RUN_AS_NODE=
for /f "delims==" %%v in ('set VSCODE_ 2^>nul') do set "%%v="
"%LOCALAPPDATA%\Programs\Herdr\bin\herdr.exe" %*
```

- Both host profiles launch through it. Add other variables to the list if the parent environment carries more keys.
- It protects the server only if the first launch goes through it. If in doubt, run `herdr server stop` and relaunch from a profile: workspaces and agent sessions are restored.

### 12.2 herdr config (existing keys only)

```toml
[keys]
prefix = ["ctrl+b", "f12"]   # several prefixes are accepted; F12 is the fallback

[theme]
name = "catppuccin-latte"
```

### 12.3 VS Code settings.json

```jsonc
"terminal.integrated.commandsToSkipShell": ["-workbench.action.toggleSidebarVisibility"],
"terminal.integrated.rightClickBehavior": "nothing",
"terminal.integrated.profiles.windows": {
  "herdr": {
    "path": "C:\\Windows\\System32\\cmd.exe",
    "args": ["/c", "${env:LOCALAPPDATA}\\Programs\\Herdr\\bin\\herdr-clean.cmd"]
  }
}
```

- Ctrl+B is VS Code's sidebar toggle; the first line sends it to the terminal instead.
- Keep `terminal.integrated.minimumContrastRatio` at its default (section 9.3).
- Fallbacks: the F12 prefix, or a keybindings.json entry `{ "key": "ctrl+b", "command": "workbench.action.terminal.sendSequence", "args": { "text": "\u0002" }, "when": "terminalFocus" }`.
- At 64 columns or fewer the mobile layout shows the grouped order flat. Move the terminal into the editor area, or lower `ui.mobile_width_threshold`.

### 12.4 Windows Terminal profile

```json
{ "name": "herdr", "commandline": "%LOCALAPPDATA%\\Programs\\Herdr\\bin\\herdr-clean.cmd",
  "startingDirectory": "%USERPROFILE%", "colorScheme": "One Half Light" }
```

Ctrl+B, F12 and Shift+Up/Down are expected to be unbound in Windows Terminal's defaults; Shift+right-click reaches Windows Terminal's own menu. Both are smoke-checklist items.

### 12.5 Smoke checklist (run in both hosts, light scheme)

1. Ctrl+B and F12 both enter prefix mode.
2. `prefix+?` opens help.
3. Drag a workspace into and out of a group.
4. Drag a header to reorder groups.
5. Right-click a header opens the group menu.
6. In `prefix+w`: Shift+Up/Down crosses a group boundary; `m` opens Organize; Edit group... reaches an empty group.
7. Delete a group, then Undo delete group restores it.
8. A workspace with 2+ tabs shows tab rows; a single-tab workspace does not; clicking a tab row focuses that tab.
9. Bars and the colored focused tab are readable, and all 9 colors are distinguishable side by side.
10. `herdr up --all` creates tabs and starts agents; a second run prints "up to date"; renaming a tab and re-running starts nothing.
11. With `ANTHROPIC_API_KEY` set in the parent environment and no server running, start herdr from the VS Code profile. In a PowerShell pane, `Get-ChildItem Env: | Where-Object Name -match '^(ANTHROPIC_API_KEY|VSCODE_|GIT_ASKPASS$|CLAUDE_CODE_SSE_PORT$|ELECTRON_RUN_AS_NODE$)'` prints nothing (in a cmd pane: `set ANTHROPIC_API_KEY` and the others each print 'Environment variable ... not defined'). Repeat with the server first started from Windows Terminal.
12. Both hosts attached at once show the same change within a second of an edit in either.
13. Each agent session can be resumed from the VS Code extension for its folder.
14. Create two groups (one empty), give one workspace a color override, run `herdr up --all`. (a) `herdr server stop` and relaunch from a host profile. (b) Wait at least 10 s (the session save is debounced by 5 s), end the herdr server process in Task Manager and relaunch. After each: all groups including the empty one, membership, group order, colors and collapse state return, and `herdr up --all` prints 'up to date'.

Record results in `docs/next/fork/README.md`.

---

## 13. Abandon-safety

**Upstream reads the fork's session.**
- No fields are added and `SNAPSHOT_VERSION` stays 3, so `parse_snapshot` accepts the file.
- `layout_fingerprint` matches, so `session-history.json` (pane history) restores.
- Rotating snapshots and `session-backups\` contain only upstream fields.
- Upstream shows the grouped order flat.
- Tests: `fork_session_json_has_upstream_schema`, `layout_fingerprint_independent_of_groups`.

**Fork-only state is ignored by upstream.** Upstream never opens `fork-workspace-groups.json` or `projects.toml`, carries `group:*` collapse keys in client prefs without matching them, and finds no fork config keys. Managed-agent fields written by `agent.start` are upstream's own.

**Returning to the fork.** Install it again as in section 14, including the PATH step, because upstream's installer put its own release first on the user PATH. Records are keyed only by workspace id, so changing a workspace's directory (a root-shell `cd`, a reordered first tab, 8.3 vs long path spellings) never detaches a record. Workspaces upstream created appear ungrouped; upstream reorders are normalized back into their regions; the accepted consequences of D1 (5.6) apply.

**Endpoint and protocol.** `PROTOCOL_VERSION` and the endpoint generation are unchanged; no fixture or digest is re-blessed; the new snapshot field is `#[serde(default)]` JSON. Mixed setups degrade to "no groups" or "Action unavailable".

**Claude Code untouched.** The fork never reads or writes `~/.claude`; panes run the unmodified `claude` CLI; `src/integration` is not modified. The fork does not need `herdr integration install claude`; if it was installed, that is upstream behavior and `herdr integration uninstall claude` reverses it.

**Switching back or uninstalling.**
1. `herdr server stop`.
2. Delete `%LOCALAPPDATA%\Programs\Herdr\bin`, which holds the fork's `herdr.exe`, `conpty\`, `THIRD-PARTY-NOTICES\` and `herdr-clean.cmd`. To switch back, run upstream's installer only after that: it makes the folder a junction to its own release and puts that release first on the user PATH, but while the fork's folder is still there it stops with an error after it has already activated its release. To uninstall instead, also remove the folder from the user PATH. Point the host profiles at `herdr.exe` in the same folder, or remove them (step 4).
3. Optionally delete `%APPDATA%\herdr\fork-workspace-groups.json` and `projects.toml`.
4. Optionally remove the VS Code and Windows Terminal profile entries.

**Data-loss guards.** A corrupt or newer sidecar is set aside, never overwritten. The sidecar is loaded before any API request can mutate it. A closed workspace's record is pruned only once `session.json` no longer lists it: after an in-process session save has completed (never while a pane-exit checkpoint is pending), or at a startup with a successful restore. Group deletion is undoable.

---

## 14. Update guard

Declare the constant at module level in src/update.rs, so `crate::update::FORK_BLOCKS_SELF_UPDATE` is reachable from src/cli.rs. The first statement of `pub fn self_update`, before `UpdateChannel::configured()`, is the guard:

```rust
pub(crate) const FORK_BLOCKS_SELF_UPDATE: bool = true;   // module level

pub fn self_update(options: SelfUpdateOptions) -> Result<Version, String> {
    if FORK_BLOCKS_SELF_UPDATE {
        return Err("self-update is disabled for this fork build; rebuild from the fork source, \
                    or reinstall upstream Herdr deliberately with its installer".into());
    }
    let channel = UpdateChannel::configured();
    // upstream body unchanged
```

- `herdr update` prints the message without the "update failed:" prefix (main.rs already special-cases messages starting with "self-update is disabled") and exits 1.
- `channel_set` (src/cli.rs) refuses first, before it writes `[update] channel` into `config.toml`:
  ```rust
  if crate::update::FORK_BLOCKS_SELF_UPDATE {
      eprintln!("update channels do not apply to this fork build.");
      return Ok(1);
  }
  ```
  Without this, upstream writes the config and then prints a misleading "update failed ... Run `herdr update` to retry".
- The update notifier stays on as the rebase signal; `[update] version_check = false` silences it.
- `Cargo.toml` keeps upstream's version string.
- Tests: `self_update_is_refused_for_fork_builds`, `channel_set_is_refused_before_writing_config`.

**Install:** `just build` produces `herdr.exe`, `conpty\` and `THIRD-PARTY-NOTICES\` in `%CARGO_TARGET_DIR%\release` (16.1 step 0). The first run downloads the pinned ConPTY nupkg from github.com (packaging/windows/conpty.json). Copy all three, plus `herdr-clean.cmd`, into `%LOCALAPPDATA%\Programs\Herdr\bin`, which upstream's installer also uses. Upstream's installer makes that folder a junction to its own release folder under `%USERPROFILE%\.herdr\packages\standalone\releases\`, and copying through the junction would overwrite upstream's `herdr.exe` there, so remove only the junction first (`cmd /c rmdir`; upstream's release stays as it is) and create a real folder. Add that folder to the user PATH (HKCU, no admin) and drop the entries upstream's installer puts first on PATH (under `%USERPROFILE%\.herdr\packages\standalone\`), which would otherwise run upstream's `herdr.exe` in shells and panes. Stop the server before copying. `docs/next/fork/README.md` has the exact PowerShell.

---

## 15. Git layout and upstream sync

### 15.1 Remotes and branches

| Remote | URL | Use |
|--|--|--|
| `upstream` | `https://github.com/herdrdev/herdr.git` | fetch only |
| `origin` | `https://github.com/scott4885/herdr.git` | public fork; Actions enabled |
| `wkentaro` | `https://github.com/wkentaro/herdr.git` | optional; source of the tab-rows commit |

- Work happens on branch `fork`, which tracks `upstream/master`. Set `git config branch.fork.pushRemote origin` so a plain `git push` goes to `origin/fork`, and `git config rerere.enabled true`.
- `origin/master` stays a plain mirror of upstream master (GitHub "Sync fork", or `git push origin upstream/master:master`).
- The clone is complete (not shallow), and upstream tags are fetched.

### 15.2 Why master (D4)

Upstream cuts stable releases in an isolated checkout at a published preview tag (AGENTS.md near :292), so stable tags are not on master: `v0.9.5` peels to `2ecf19a9`, which is not an ancestor of master, and master has 27 commits after it, including `refactor: share cli and api command contracts`. The design was checked against master, the tab-rows port is resolved against master (8.4), and the next stable is cut from master. Rebasing onto master also means a plain `git rebase upstream/master` replays only fork commits, because upstream never rewrites master.

### 15.3 Rebase procedure

Run at least at each upstream stable release (the update notifier is the signal), and whenever an upstream fix is wanted.

1. `git fetch upstream --tags`
2. `git switch fork`, then `git rebase upstream/master`.
3. Resolve conflicts. For `docs/next/api/herdr-api.schema.json`, take upstream's version (`git checkout --ours docs/next/api/herdr-api.schema.json`; during a rebase, "ours" is upstream); it is regenerated in step 5. For sidebar.rs, endpoint_sidebar.rs and mouse.rs, re-apply the fork's calls by hand; never resolve them blindly.
4. Immediately after the rebase finishes, review with `git range-diff @{u} fork@{1} fork`: base = new upstream, `fork@{1}` = the fork before the rebase, `fork` = the rebased fork. Every fork commit should pair up, and changed pairs show exactly what conflict resolution altered.
5. `$env:HERDR_UPDATE_API_SCHEMA='1'; just test-one generated_protocol_schema_artifact_is_current; Remove-Item Env:HERDR_UPDATE_API_SCHEMA` (never leave the variable set; see 7.5). Re-pin the `workspace_group.edit` digest only if that method's own shape changed. Commit the regenerated schema and any re-pinned digest with `git commit --fixup=<sha of commit 2>`, then run `git rebase --autosquash upstream/master`.
6. `just check`, then `just build`.
7. `herdr server stop`, copy the build in (section 14), smoke-check.
8. `git push --force-with-lease origin fork fork:windows`. Upstream's CI workflow runs on pushes to `master` and `windows`, so the `windows` branch runs CI on the fork without editing `ci.yml`.

Fold later fixes into their stack commit with `git commit --fixup=<sha>` and `git rebase --autosquash upstream/master` (git 2.44 or newer runs this without an editor).

### 15.4 Commit stack

Ordered so each hot file's hunks live in exactly one commit. Subjects follow AGENTS.md (near :237): lowercase conventional commits, no emojis, no AI co-author lines.

1. `docs: add fork design spec`: this file.
2. `feat: persist workspace groups in a sidecar state file`: server model, sidecar, reconcile, API schema and handlers, protocol field and its literal sites, contract-test entries, regenerated API schema. The body states the AGENTS.md colors deviation (D9) and its reason.
3. `feat: show workspace tab rows and groups in the sidebar`: `sidebar_tabs.rs` and the groups client together, so every hunk in sidebar.rs, endpoint_sidebar.rs and mouse.rs is resolved once per rebase. Body: "Includes sidebar tab rows ported from wkentaro/herdr 29484fc4 (Kentaro Wada), Apache-2.0."
4. `feat: add herdr up for project templates`
5. `chore: block self-update in fork builds`
6. `docs: add fork guide, example projects file and clean launcher`

### 15.5 Attribution

wkentaro/herdr and upstream are both Apache-2.0, and neither has a NOTICE file. `sidebar_tabs.rs` starts with a header stating that it contains code ported and modified from wkentaro/herdr commit `29484fc4` (Kentaro Wada), licensed under Apache-2.0 (section 4(b) of the license). The LICENSE file stays as is. `docs/next/fork/README.md` repeats the credit.

### 15.6 Public-repo hygiene

Every committed file is public. No employer or business names, no personal usernames or user-specific absolute paths (use `%USERPROFILE%`, `%APPDATA%`, `%LOCALAPPDATA%`, `E:\tools`), no secrets or tokens, and only generic example projects and groups.

Exceptions: the fork's own repository slug `scott4885/herdr` where docs name the repository (this spec's header and D5, remote URLs, `gh -R` arguments), and references to wkentaro/herdr as the source of the tab-rows port, with the Apache-2.0 attribution (Kentaro Wada) required by 15.5. Code and user-facing strings never name a person or account.

---

## 16. Build toolchain

Local build without admin rights (D2). Commands run in PowerShell started from the Start menu or VS Code, never from Git Bash: Git's `/usr/bin/link.exe` would shadow the MSVC linker. Nothing below has been executed yet; it comes from reading the build scripts and tool sources.

### 16.1 Setup (once)

0. **Per-user locations and prerequisites (no admin):**
   - In PowerShell run `[Environment]::SetEnvironmentVariable('RUSTUP_HOME','E:\tools\rustup','User')`, `[Environment]::SetEnvironmentVariable('CARGO_HOME','E:\tools\cargo','User')` and `[Environment]::SetEnvironmentVariable('CARGO_TARGET_DIR','E:\tools\target\herdr','User')`, add `E:\tools\cargo\bin` to the user PATH, then open a new PowerShell. `CARGO_TARGET_DIR` applies to every cargo build of this user, which cargo supports; `just build` honors it (scripts/package_windows_conpty.py).
   - **Python 3.11 or newer**, per-user: the python.org installer with "Install for me only" and install location `E:\tools\python`, added to the user PATH. It must be reachable as `python`, not the Microsoft Store alias (turn off the `python.exe` App execution alias if `python --version` opens the Store). `portable-msvc.py` (step 2), `just build` and the maintenance tests in `just check` use it, and those tests import `tomllib`.
   - Confirm `Get-ExecutionPolicy -List` shows MachinePolicy and UserPolicy as `Undefined`. A Group Policy value overrides `just check`'s `powershell.exe -ExecutionPolicy Bypass`, and then the CI baseline (16.5) is the check of record.
1. **Rust:** `rustup-init.exe -y --default-host x86_64-pc-windows-msvc --default-toolchain none --profile minimal`. Always pass `-y`; the interactive "install Visual Studio" option needs admin. Then, in the repo, `rustup toolchain install 1.96.1 --profile minimal -c clippy -c rustfmt` (pinned by `rust-toolchain.toml`).
2. **Portable MSVC:** download `portable-msvc.py` from the gist https://gist.github.com/7f3162ec2988e81e56d5c4e22cde9977 into `E:\tools\portable-msvc`, read it, then run `python -I portable-msvc.py --vs 2022 --sdk-version 26100 --host x64 --target x64` there and accept the license. It writes `E:\tools\portable-msvc\msvc`. Pin VS 2022 and SDK 26100: they match the `Windows ConPTY package` job (`runs-on: windows-2022`); `check (windows-latest)` follows whatever image windows-latest maps to. Newer MSVC STL headers may need a newer Clang than Zig 0.16 bundles. Verify that `link.exe`, `libcmt.lib`, `kernel32.lib` and `ucrt.lib` exist, then delete `downloads\`.
3. **Zig 0.16.0:** the Windows x86_64 zip, SHA-256 `68659EB5F1E4EB1437A722F1DD889C5A322C9954607F5EDCF337BC3684A75A7E`, extracted to `E:\tools\zig-x86_64-windows-0.16.0`. Never move it afterwards: Zig's header cache keys include the install path.
4. **Zig libc file** `E:\tools\herdr-libc.txt`, the same mapping as scripts/windows_cross.py `libc_contents`. `<V>` is the single folder under `E:\tools\portable-msvc\msvc\VC\Tools\MSVC`:
   ```text
   include_dir=E:\tools\portable-msvc\msvc\Windows Kits\10\Include\10.0.26100.0\ucrt
   sys_include_dir=E:\tools\portable-msvc\msvc\VC\Tools\MSVC\<V>\include
   crt_dir=E:\tools\portable-msvc\msvc\Windows Kits\10\Lib\10.0.26100.0\ucrt\x64
   msvc_lib_dir=E:\tools\portable-msvc\msvc\VC\Tools\MSVC\<V>\lib\x64
   kernel32_lib_dir=E:\tools\portable-msvc\msvc\Windows Kits\10\Lib\10.0.26100.0\um\x64
   gcc_dir=
   ```
   Zig cannot find an unregistered SDK on its own. Validate with `zig libc -target x86_64-windows-msvc E:\tools\herdr-libc.txt`.
5. **User cargo config** `%CARGO_HOME%\config.toml` (`E:\tools\cargo\config.toml`; TOML single-quoted literals, `<V>` as in step 4):
   ```toml
   [target.x86_64-pc-windows-msvc]
   linker = 'E:\tools\portable-msvc\msvc\VC\Tools\MSVC\<V>\bin\Hostx64\x64\link.exe'
   [env]
   ZIG = 'E:\tools\zig-x86_64-windows-0.16.0\zig.exe'
   LIBGHOSTTY_VT_WINDOWS_LIBC = 'E:\tools\herdr-libc.txt'
   LIB = 'E:\tools\portable-msvc\msvc\VC\Tools\MSVC\<V>\lib\x64;E:\tools\portable-msvc\msvc\Windows Kits\10\Lib\10.0.26100.0\ucrt\x64;E:\tools\portable-msvc\msvc\Windows Kits\10\Lib\10.0.26100.0\um\x64'
   ```
   `LIB` is the `msvc_lib_dir`, `crt_dir` and `kernel32_lib_dir` values from step 4, joined with `;`. The absolute linker path means rustc never searches PATH for `link.exe`. Cargo merges this with the repo's `.cargo/config.toml`, which keeps `+crt-static`. Never set `RUSTFLAGS`: it would replace that flag, and `just build` rejects a binary that imports the dynamic CRT. No `cl.exe` or vcvars is needed; Zig is the only C/C++ compiler in the build.
6. **Tools:** `just` 1.59.0 and `cargo-nextest` 0.9.149 from their prebuilt Windows zips into `E:\tools\cargo\bin`.
7. **Script runtimes:** Bun 1.3.14 (the CI pin in ci.yml `oven-sh/setup-bun`), from the Windows zip into `E:\tools\bun`, on the user PATH. `just check` runs `bun test` and `python -m unittest` (step 0) through `just test`.

### 16.2 Build and check

1. `$env:CARGO_PROFILE_DEV_DEBUG='1'; $env:CARGO_PROFILE_TEST_DEBUG='1'; cargo build --locked` (mirrors CI and keeps PDBs small). If the Zig SIMD C++ step fails, retry with `$env:LIBGHOSTTY_VT_SIMD='false'`.
2. Success criterion: `just check` (fmt, clippy `-D warnings`, nextest, the Python and Bun script suites, then a debug `cargo build --locked`) exits 0.
3. `just build` produces `%CARGO_TARGET_DIR%\release\herdr.exe` plus `conpty\` and `THIRD-PARTY-NOTICES\` next to it; the first run downloads the pinned ConPTY package from github.com.

### 16.3 Gotchas

- Defender real-time scanning slows cargo and Zig I/O; exclusions need admin.
- Long paths are not enabled; `CARGO_TARGET_DIR` is already short (step 0).
- `windows_check.ps1` clears the Zig cache once and retries a failed clippy run, so a cache failure is not necessarily a code bug.
- Before debugging a local test failure, compare with the fork's CI result for the same commit.
- Upstream documents only the Visual Studio Build Tools route; this route depends on the `LIBGHOSTTY_VT_WINDOWS_LIBC` hook and may need attention if upstream changes `build.rs`.

### 16.4 Fallback: xwin

If portable-msvc.py fails because `msiexec /a` (which it uses to unpack the SDK .msi files) is blocked by policy: xwin 0.10.0, `xwin --accept-license --arch x86_64 splat --copy --disable-symlinks --output E:\tools\xwin`, generate the libc file with the repo's `scripts/windows_cross.py` (`libc_contents`), set `linker = 'rust-lld.exe'`, and point `LIB` at the xwin `crt` and `sdk` lib folders. Also set `LIBGHOSTTY_VT_SIMD = 'false'` in the user cargo `[env]`, as upstream's xwin route does (`scripts/windows_cross.py` `lint()`). Trade-off: lld-link instead of the MSVC linker CI uses.

### 16.5 CI baseline

- The fork is public, so Windows runners are free, and Actions are enabled on it.
- Tests: push `fork:windows` (section 15.3); `ci.yml` runs `check (windows-latest)`. Its `timeout-minutes: 15` may kill a cold first run; raise it in a fork-only commit only if that happens.
- Binaries: `gh workflow run build-artifacts-manual.yml -R scott4885/herdr -f build_group=windows -f libghostty_optimize=ReleaseFast -f libghostty_simd=true`, then `gh run download`.

Budget: about 15-20 GB, mostly on `E:`. The fork adds no crate dependencies.

---

## 17. Testing strategy

All tests are inline `#[cfg(test)]` in the existing style. Commands: `just test-one <name>`; all fork tests with `cargo nextest run --locked -E 'test(workspace_group) | test(sidebar_tab) | test(/^cli::up::/) | test(fork) | test(self_update_is_refused) | test(channel_set_is_refused) | test(layout_fingerprint_independent_of_groups)'`; the client shell with `cargo test --locked --bin herdr client::shell` (herdr has no library target); then `just check`. Persistence and pruning tests use `AppState::test_new`, `Workspace::test_new`, `test_with_adversarial_identity_state` and `assert_invariants_for_test`, as AGENTS.md (near :157) asks for persisted-state changes.

**Characterization (protect upstream behavior)**
- `fork_session_json_has_upstream_schema`
- `layout_fingerprint_independent_of_groups`
- `grouped_entries_match_upstream_for_single_region`
- `no_groups_renders_identically_to_upstream`
- `single_tab_workspace_without_groups_renders_identically_to_upstream`
- `single_tab_workspace_differs_only_by_color_bar`
- The unmodified upstream suite. For D3, no upstream test assertion may be edited. If an upstream test fails because a 2+-tab fixture shifts sidebar geometry, stop and ask the fork owner before touching it.

**Model** (`src/app/workspace_groups.rs`)
- `normalize_is_identity_without_groups`
- `normalize_is_stable_idempotent_and_keeps_active_selected_by_id`
- `place_top_end_before_target_and_other_region_fallback`
- `place_parent_moves_colocated_children_only_when_sole_parent`
- `place_linked_child_moves_only_child`
- `linked_child_inherits_unambiguous_family_group_when_created_at_runtime`
- `linked_child_stays_ungrouped_when_family_groups_ambiguous`
- `linked_child_inference_skipped_on_initial_reconcile`
- `delete_group_ungroups_members_and_tombstones_template`
- `create_with_template_clears_tombstone_and_honors_before_group`
- `move_group_reorders_regions`
- `group_ids_time_based_and_unique`
- `names_unique_case_insensitive_and_bounded`
- `colors_limited_to_named_palette`
- `bind_tab_rejects_foreign_tab_and_drops_dead_numbers`
- `bind_none_clears_tab_bindings`
- `batch_is_all_or_nothing`

**Persistence and pruning** (`src/persist/workspace_groups.rs`, `src/app/api/workspace_groups.rs`)
- `missing_file_loads_empty`
- `corrupt_or_newer_file_is_set_aside_not_overwritten`
- `roundtrip_preserves_groups_records_tombstones`
- `empty_groups_persist_across_restart`
- `no_disk_access_under_test_policy`
- `handoff_replacement_policy_loads_sidecar_before_first_edit`
- `membership_survives_restart_when_root_pane_cwd_changed` (capture, `parse_snapshot`, restore, reconcile)
- `membership_survives_restart_after_first_tab_moved_or_closed`
- `startup_prunes_absent_ids_only_after_successful_restore`
- `failed_or_newer_session_load_keeps_records`
- `records_survive_failed_load_until_first_successful_restore`
- `in_process_close_prunes_record_after_session_save`
- `close_via_last_pane_exit_prunes_record_after_session_save` (the path that clears `session_dirty`)
- `interrupted_pane_exit_keeps_record_while_checkpoint_pending`
- `shutdown_within_debounce_after_interrupted_exit_keeps_record`
- `crash_between_sidecar_and_session_save_prunes_only_unsaved_workspace`

**API and reconcile**
- `edit_emits_reordered_only_when_order_changes`
- `reconcile_normalizes_after_upstream_workspace_move_and_requests_render`
- `reconcile_is_noop_when_id_sequence_unchanged`
- `new_workspace_lands_ungrouped_without_reorder`
- `list_reports_display_order_cwd_linked_and_live_tab_bindings`

**API contract:** the `changes_ui` set entry, the digest pin, the regenerated schema, and the unchanged frozen-fixture tests (7.5).

**Colors**
- `named_colors_at_least_4_5_on_white_and_distinct` (pairwise ΔE76 ≥ 15)
- `readable_reaches_3_to_1_on_sidebar_and_highlight_backgrounds_for_every_builtin_theme`
- `reset_background_uses_reported_host_background_else_appearance`
- `colors_stay_distinct_after_simulated_vscode_min_contrast` (against `#ffffff`, `#1e1e2e` and latte `selection_bg`, `active_row_bg`, `surface1`)
- `tab_fill_text_contrast_at_least_4_5`
- `unknown_color_name_draws_no_bar`

**Client shell compose tests** (`src/client/shell/tests/workspace_groups.rs`; they render with `state.compose(w, h)` and inspect the buffer and `state.hits`)
- Groups: `group_headers_render_bar_name_toggle_in_group_order_with_ungrouped_last`, `display_order_buckets_by_group_even_when_vec_order_drifted`, `multi_parent_family_across_groups_renders_each_parent_in_its_group`, `split_family_child_renders_top_level_and_ignores_worktree_collapse`, `collapsed_group_hides_members_keeps_focused_and_rolls_up_status`, `navigation_skips_collapsed_group_members`, `empty_and_collapsed_groups_stack_headers`, `reveal_focused_workspace_accounts_for_header_rows`, `multi_machine_sidebar_renders_groups_per_endpoint`.
- Colors: `member_rows_and_tab_rows_paint_effective_color_bar`, `group_color_never_used_for_text_cells`, `focused_tab_uses_workspace_color_with_contrast_text`.
- Mouse: `header_click_toggles_and_persists_group_key`, `drop_on_header_places_at_group_top`, `drop_between_members_places_before_target`, `drop_after_region_end_places_at_group_end`, `drop_on_ungrouped_tail_ungroups`, `header_drag_sends_group_move`, `right_click_header_opens_group_menu_and_rename_sends_update`.
- Menus and keys: `organize_move_to_group_creates_or_places_or_ungroups`, `edit_group_prompt_opens_menu_for_empty_group`, `delete_then_undo_restores_name_color_template_members_and_position`, `undo_after_member_closed_restores_remaining_members`, `navigate_shift_down_crosses_group_boundary_and_expands_collapsed_target`, `navigate_m_opens_organize_menu`, `user_binding_wins_over_navigate_organize_keys`.
- Compatibility: `upstream_endpoint_without_method_shows_action_unavailable_and_drags_upstream`.
- Tab rows (port, adapted to D3): `sidebar_tab_rows_only_for_workspaces_with_two_or_more_tabs`, `workspace_row_keeps_aggregate_status`, `sidebar_tabs_show_status_and_focus_background_workspace`, `card_bottom_covers_tab_rows`, `sidebar_tab_click_focuses_tab_without_drag`, `sidebar_tab_click_activates_its_endpoint`, `disabled_mouse_has_no_sidebar_tab_hits`, `workspace_drag_rejects_foreign_tab_rows`. The tests that need a remote machine replicate the small `state_with_remote` and `remote_profile` fixtures from `tests/endpoints.rs` (both private there) instead of editing that file.

**CLI contract and `herdr up`** (`src/cli/up.rs`)
- `parses_groups_projects_tabs_in_file_order`
- `rejects_unknown_keys_duplicates_unresolved_group_bad_color_agent_and_command_together`
- `derived_agent_names_follow_upstream_rule_and_are_unique`
- `requires_exactly_one_selection_mode`
- `help_is_handled_inside_the_up_arm`
- `plan_creates_everything_on_empty_state`, `plan_is_empty_after_apply`
- `plan_adopts_by_cwd_when_label_is_repo_name`, `plan_adopts_single_label_match_and_skips_ambiguous`
- `plan_treats_binding_outside_project_cwd_as_stale`, `plan_stale_binding_converges_in_one_run` (apply, re-plan, empty plan)
- `plan_launches_first_tab_in_root_tab_of_created_workspace`, `plan_project_without_tabs_creates_bare_workspace`
- `plan_matches_renamed_group_by_template`, `plan_matches_renamed_tab_by_binding`, `plan_binds_live_agent_tab_by_agent_name`
- `plan_respects_ui_ungroup_cleared_color_and_deleted_group`, `plan_group_selection_recreates_tombstoned_group`, `plan_reapply_restores_label_group_color`
- `agent_tabs_use_agent_start_with_stable_name_and_retry_busy`
- `commands_only_for_tabs_created_this_run_with_enter_key`
- `agent_name_taken_elsewhere_is_reported_not_retried`
- `exit_code_3_when_a_project_is_skipped`, `upstream_server_without_group_list_exits_1`
- The existing CLI contract tests (`tests/cli/contracts.rs`) must pass unchanged.

**Update guard:** `self_update_is_refused_for_fork_builds`, `channel_set_is_refused_before_writing_config`.

**Integration.** The existing integration suites (`tests/*.rs`, including the PTY-driven `client_mode` tests on Unix CI) must pass unchanged on the fork's CI run. A manual abandon test: copy `%APPDATA%\herdr` to a scratch folder, point `XDG_CONFIG_HOME` at it (herdr's `config_dir` honors it on Windows), run an upstream release build of the same base against it, and confirm workspaces, tabs and pane history restore. Run it from a plain PowerShell window, or first `Remove-Item Env:HERDR_SOCKET_PATH, Env:HERDR_CLIENT_SOCKET_PATH -ErrorAction SilentlyContinue`; panes export these and would point the upstream build at the running fork server. A debug upstream build looks for `herdr-dev` instead of `herdr` under the scratch folder (5.2).

**Real-terminal verification.** The smoke checklist (12.5) in Windows Terminal and in the VS Code integrated terminal, both attached to one server at the same time. Record results in the fork README.

---

## 18. File touch list

**New files (fork-only; never conflict on rebase)**

| Path | Est. LOC | Contents |
|--|--|--|
| `src/app/workspace_groups.rs` | 500 | model, validation, `plan_edit`, reconcile core, projection, tests |
| `src/app/api/workspace_groups.rs` | 250 | load/prune, list and edit handlers, `reconcile_workspace_groups`, tests |
| `src/persist/workspace_groups.rs` | 150 | sidecar v1 schema, load (set-aside), save, tests |
| `src/api/schema/workspace_groups.rs` | 110 | params, ops, info types |
| `src/protocol/workspace_groups.rs` | 45 | snapshot types, `WORKSPACE_COLORS` |
| `src/client/shell/workspace_groups.rs` | 800 | `grouped_entries`, `GroupLayout`, headers, bars, `readable`, drop slots, menus, undo, navigate keys |
| `src/client/shell/sidebar_tabs.rs` | 110 | ported tab rows (2+ tabs), attribution header |
| `src/client/shell/tests/workspace_groups.rs` | 650 | client compose tests incl. tab rows and replicated remote fixture |
| `src/cli/up.rs` | 655 | `herdr up`: parse, validate, plan, execute, tests |
| `docs/next/fork/design.md` | — | this spec |
| `docs/next/fork/README.md` | 270 | fork guide, hosting, smoke results, rebase steps, credits |
| `docs/next/fork/projects.example.toml` | 55 | example in section 11.1 |
| `docs/next/fork/herdr-clean.cmd` | 8 | clean launcher |
| **Subtotal (code + guide)** | **~3,600** | |

**Edited upstream files**

| Path | Est. LOC | Change |
|--|--|--|
| `src/app/mod.rs` | 3 | `mod workspace_groups;`, `AppState` literal field, `load_workspace_groups()` at end of `try_new` |
| `src/app/state.rs` | 4 | `AppState.workspace_groups` field, `test_new` literal |
| `src/app/session.rs` | 1 | first line of `sync_session_save_schedule` |
| `src/app/api.rs` | 7 | `mod workspace_groups;`, 2 dispatch arms after the `workspace.*` arms |
| `src/persist.rs` | 1 | `pub(crate) mod workspace_groups;` |
| `src/persist/io.rs` | 1 | `save_json_to_path` → `pub(super)` |
| `src/api/schema.rs` | 4 | mod/use, 2 `define_operations!` lines after `WorkspaceClose` |
| `src/api/schema/response.rs` | 6 | `ResponseResult::WorkspaceGroups` + import |
| `src/api/schema/operations.rs` | 1 | `changes_ui` set entry (test) |
| `src/server/client_commands.rs` | 5 | digest pin (test) |
| `src/server/client_shell.rs` | 1 | fill `workspace_groups` in the snapshot literal |
| `src/server/headless/client_views.rs` | 1 | add `WorkspaceGroupEdit` to `shell_endpoint_claims_geometry` (7.3) |
| `src/protocol/mod.rs` | 2 | mod/use |
| `src/protocol/wire.rs` | 4 | `ClientShellSnapshot` field + bincode test literal |
| `src/protocol/endpoint.rs` | 1 | test literal |
| `src/client/endpoint/activation_tests.rs` | 2 | 2 test literals |
| `src/client/shell.rs` | 2 | `mod workspace_groups; mod sidebar_tabs;` |
| `src/client/shell/state.rs` | 20 | hit fields, drag/menu/rename variants, undo stash, initializers |
| `src/client/shell/sidebar.rs` | 55 | entries hook, tab-row call sites, header rows in heights/fit/render, bar, slots |
| `src/client/shell/endpoint_sidebar.rs` | 50 | same per machine section |
| `src/client/shell/mouse.rs` | 65 | tab-row click, card-aware drop rejection, header press/drag/drop, slot drop, header right-click |
| `src/client/shell/render.rs` | 3 | clear new hits when mouse capture is off |
| `src/client/shell/composition.rs` | 4 | drop indicator for group drag |
| `src/client/shell/context_menu.rs` | 10 | "Organize..." tail, Organize target and actions |
| `src/client/shell/overlay_input.rs` | 2 | `save_rename_overlay` arm |
| `src/client/shell/input.rs` | 3 | `route_navigate_key` hook |
| `src/client/shell/tabs.rs` | 3 | `active_tab_colors()` |
| `src/client/shell/tests/mod.rs` | 2 | `mod workspace_groups;`, `snapshot()` literal field |
| `src/cli.rs` | 6 | `mod up;`, `"up"` arm, `channel_set` refusal |
| `src/update.rs` | 15 | const, guard, test |
| `docs/next/api/herdr-api.schema.json` | ~190 | regenerated, never hand-merged |
| **Subtotal (hand edits)** | **~286 in 30 files** | plus the regenerated schema |

Upstream test assertions edited: none. Upstream test files touched: only literal fields, the `mod` line, and the two API contract entries for the new method.

---

## 19. Risks and mitigations

| # | Risk | Mitigation |
|--|--|--|
| 1 | The ~170 lines in sidebar.rs, endpoint_sidebar.rs and mouse.rs conflict whenever upstream reworks sidebar rendering or press/drag handling. | All of them live in one commit (15.4); logic stays in fork files whose inputs are snapshot data; budget 15-45 minutes per rebase, up to a day for a large sidebar restructure. |
| 2 | Nothing is compiled or tested yet. The raw tab-rows commit does not apply at `5bd900d` and has to be resolved by hand against master (8.4); about 10K lines of newer upstream client-shell tests have not run against it. | Order of work starts with a green `just check` on unmodified master, then the port (`git cherry-pick -n 29484fc4`, source hunks only, 8.4), then the rest; CI on the fork as a second opinion. |
| 3 | An upstream test with a 2+-tab fixture may assert sidebar geometry that tab rows shift. | Stop and ask the fork owner; do not edit the assertion silently (R6). |
| 4 | D1: after upstream use, a reissued id can inherit an old record (wrong group or color). | One drag fixes it; `herdr up` ignores bindings whose cwd is outside the project (D17). |
| 5 | D1: after a failed or newer-version `session.json` load, new workspaces numbered from `w1` can pick up old records. | Stop the server and restore the preserved session from `session-backups\` before creating any workspace (5.6); otherwise fix by dragging. |
| 6 | The sidecar and `session.json` are separate files; a crash between their writes can leave a record for a workspace the session never saved, or a workspace with no record. | The first case is pruned at the next successful startup; the second shows the workspace ungrouped. |
| 7 | A split worktree family (child grouped apart from every parent) can show upstream's family toggle on a parent whose children are all in other regions. | Cosmetic; only happens when families are split deliberately. |
| 8 | `agent.start` sends the command as soon as the shell has no child process; a profile that is still loading without children gets type-ahead. | Upstream behavior for every `agent.start`; verify with the fork owner's PowerShell profile in the smoke checklist. |
| 9 | The clean launcher protects the server only if the first launch goes through it. | Smoke step 11; README says to `herdr server stop` and relaunch from a profile when in doubt. |
| 10 | VS Code and Windows Terminal key, mouse and appearance behaviors are assumptions until tested. | Smoke checklist; F12 prefix, `sendSequence`, keyboard `m` and an explicit light theme as fallbacks. |
| 11 | At 64 columns or fewer there are no headers or colors; collapse state is last-writer-wins between hosts. | Documented; widen the panel or lower `ui.mobile_width_threshold`. |
| 12 | On a dark host that reports neither its background nor its appearance, plain-row bars use raw colors (2.2-3.3:1), because `sidebar_bg` is `Reset` in every built-in theme (9.2). | The fork owner uses `catppuccin-latte` on light host schemes; the smoke checklist confirms both hosts report their appearance. |
| 13 | Normalization renumbers workspaces when groups change. | Stable ids such as `w3` are unaffected; scripts should use ids. |
| 14 | The portable MSVC + Zig route is unofficial. | xwin fallback; fork CI builds as baseline. |
| 15 | A future upstream root command named `up` would collide. | A textual conflict in `cli::maybe_run` during rebase is the first signal. If it merges cleanly, the duplicate arm is an `unreachable_patterns` warning (the first arm wins), which `just check` turns into an error through clippy `-D warnings`. Rename the fork's command then. |
| 16 | Concurrent edits from two hosts are not revision-checked. | A stale drag applies against the current order; ops are small and visible, so a wrong result is one more drag. |
| 17 | `herdr up`'s stale-binding check misfires if the first tab's root pane `cd`s outside the project. | The stale workspace is never re-adopted in the same run, so `up` binds or creates a workspace whose cwd is inside the project, and the next run is up to date. A misfire costs at most one extra workspace (and its launches) once, never a loop; close the extra workspace or `cd` the root pane back. |

---

## 20. Out of scope

- Nested groups, group-level actions on members (close all, broadcast input), per-group layouts.
- A `herdr group` CLI, or any scripting surface beyond `workspace_group.list`/`edit` and `herdr up`.
- Watching `projects.toml`, writing it from the UI, or deleting anything from it.
- Keeping fork records correct across a period of running upstream herdr (D1).
- Keyboard navigation onto tab rows, and tab rows for single-tab workspaces.
- Free-form hex colors and theme-specific palettes.
- Revision-checked concurrent edits between hosts.
- Changes to the `session.json` schema, the endpoint protocol version, config keys, keybinding config or website docs.
- Stripping environment variables in code (handled by the launcher).
- macOS and Linux hosting guidance (the code stays portable; only Windows hosting is documented and tested).
- Upstreaming the features; that may be offered later as separate pull requests.
