# herdr fork guide

This is the guide for the herdr fork published at `github.com/scott4885/herdr`, branch `fork` (also pushed as `windows` so upstream's CI runs on it). The fork is based on upstream `herdrdev/herdr` `master` and adds workspace groups, sidebar tab rows, `herdr up`, and the pieces needed to run it safely from Windows Terminal and the VS Code integrated terminal.

| File | Purpose |
|--|--|
| [design.md](design.md) | approved design: requirements, decisions D1-D17, test names |
| [projects.example.toml](projects.example.toml) | example `projects.toml` for `herdr up` |
| [herdr-clean.cmd](herdr-clean.cmd) | clean launcher that both terminal hosts start herdr through |

## What the fork adds

**Workspace groups.** One level of named, colored groups in the sidebar.
- Create, rename, recolor, reorder and delete groups; move a workspace into, out of and between groups; collapse and expand a group. Every action works with the mouse and with the keyboard (table below). Deleting a group asks no question and can be undone from the menu.
- Groups are server state, so every attached client shows the same groups live. They are saved in `fork-workspace-groups.json`, next to `session.json` in the herdr data folder (`%APPDATA%\herdr\`, or `%APPDATA%\herdr\sessions\<name>\` for a named session). `session.json` keeps upstream's schema; only the order of its workspaces follows group order.
- Sidebar order, workspace numbers and `herdr workspace list` follow group order, with ungrouped workspaces last under a dim `ungrouped` header. A reorder made by a client that does not know about groups is normalized right away.
- Collapse state is per client: each client keeps its own and saves it in its preferences file for that server socket (last writer wins between hosts; it is not reloaded live).
- A linked worktree created while the server runs joins its family's group when that group is unambiguous.
- With remote machines configured, each machine's section in the sidebar shows that machine's groups. A machine that runs upstream herdr shows no groups.
- At or below `ui.mobile_width_threshold` columns (default 64) the mobile layout shows the grouped order flat, without headers or colors.

**Colors.** Nine names: red, orange, yellow, green, teal, blue, purple, pink, gray. A group has one; a workspace may override it.
- The color is drawn as a `▌` bar in column 0 of every row of the workspace card, and as the fill of the focused tab (with white text). Text is never colored.
- Every color is at least 4.5:1 on white. Bars are adjusted each frame to at least 3:1 against the cell they are drawn on, including the host's own background (as the terminal reports it, else assumed from its light or dark appearance).

**Sidebar tab rows.** A workspace with 2 or more tabs lists each tab as a clickable child row (`├─` / `└─`, that tab's agent status, the current tab in bold). Clicking a row focuses that tab, activating its machine first if it is remote. Single-tab workspaces stay compact. The rows are ported from wkentaro/herdr `29484fc4` (see Credits).

**`herdr up`.** A project file, `projects.toml`, declares groups, project workspaces and their tabs (an agent or a plain command). `herdr up` creates or adopts them idempotently through the public API (section below).

**Fork safety.**
- `herdr-clean.cmd` starts herdr without API keys or IDE variables in its environment (Hosting below).
- `herdr update` refuses with `self-update is disabled for this fork build; rebuild from the fork source, or reinstall upstream Herdr deliberately with its installer`, and `herdr channel set` refuses with `update channels do not apply to this fork build.` before it touches `config.toml`. The update notifier stays on as the signal to sync with upstream; `[update] version_check = false` silences it.
- Going back to upstream is documented and keeps your sessions (below).

### Organizing groups

| Action | Mouse | Keyboard (navigate mode, `prefix+w`) |
|--|--|--|
| Create a group | right-click a workspace, Organize..., Move to group..., type a new name | `m`, Move to group..., type a new name |
| Rename, recolor, delete a group | right-click its header | `m`, then the group items, or Edit group... |
| Move a workspace | drag it onto a header (top of the group, or the end when collapsed or empty), between members, below a group, or onto the `ungrouped` header | Shift+Up / Shift+Down, or Move to group... |
| Reorder groups | drag a header, or Move group up / Move group down | Edit group..., then Move group up / Move group down |
| Collapse or expand | click the header | Edit group..., Collapse; Expand all groups |
| Workspace color | Organize..., Workspace color... | `m`, Workspace color... |

- Move to group... takes a group name: an existing name (any case) moves the workspace there, a new name creates the group, an empty name ungroups the workspace.
- Edit group... takes a group name and opens that group's menu, so groups the workspace is not in, and empty groups, are reachable from the keyboard.
- Workspace color... offers the nine colors plus Group color, which removes the override.
- Shift+Up / Shift+Down move the selected workspace one slot in the grouped order, crossing into the neighboring group (including an empty one) at a group's edge. Entering a collapsed group expands it. Navigate mode stays on, so the key repeats.
- Your own keybindings, the navigate keys and digits always win over `m` and Shift+Up / Shift+Down.
- A client connected to an upstream server shows no groups and answers organize actions with `Action unavailable`.

## Install from a release build

1. Get a release build:
   - local: `just build` (see Building from source) writes `herdr.exe`, `conpty\` and `THIRD-PARTY-NOTICES\` to `target\release\`. The first run downloads the pinned ConPTY package from github.com.
   - CI: `gh workflow run build-artifacts-manual.yml -R scott4885/herdr -f build_group=windows -f libghostty_optimize=ReleaseFast -f libghostty_simd=true`, then `gh run download` and unzip `herdr-windows-x86_64.zip`, which holds the same three items.
2. Stop the running server: `herdr server stop`. Workspaces and agent sessions are restored on the next start.
3. Copy the build and the launcher into `%LOCALAPPDATA%\Programs\Herdr\bin`, the folder upstream's installer also uses. Upstream's installer makes that folder a junction to its own release folder under `%USERPROFILE%\.herdr\packages\standalone\releases\`, and copying through the junction would overwrite upstream's `herdr.exe` there. So the script removes only the junction (upstream's release stays as it is) and creates a real folder. The repository stores `herdr-clean.cmd` with LF line endings, so the script writes the installed copy with CRLF. Run it from the repository root:

   ```powershell
   $bin = "$env:LOCALAPPDATA\Programs\Herdr\bin"
   $build = 'target\release'   # or the unzipped CI artifact folder
   $item = Get-Item -LiteralPath $bin -Force -ErrorAction SilentlyContinue
   if ($item -and $item.LinkType -eq 'Junction') { cmd /c rmdir "$bin" }   # removes the link only
   New-Item -ItemType Directory -Force $bin | Out-Null
   Copy-Item -Force "$build\herdr.exe" $bin
   foreach ($dir in 'conpty', 'THIRD-PARTY-NOTICES') {
       New-Item -ItemType Directory -Force "$bin\$dir" | Out-Null
       Copy-Item -Recurse -Force "$build\$dir\*" "$bin\$dir"
   }
   Get-Content docs\next\fork\herdr-clean.cmd | Set-Content -Encoding ascii "$bin\herdr-clean.cmd"
   ```

4. Add that folder to your user PATH (no admin rights needed). The script edits the registry value directly, so entries such as `%USERPROFILE%\.cargo\bin` keep their `%` form. It also drops the entries upstream's installer puts first on PATH (under `%USERPROFILE%\.herdr\packages\standalone\`), which would otherwise run upstream's `herdr.exe` in shells and panes. Sign out and back in afterwards, so Windows Terminal, VS Code and new shells see the change:

   ```powershell
   $bin = "$env:LOCALAPPDATA\Programs\Herdr\bin"
   $upstream = "$env:USERPROFILE\.herdr\packages\standalone\"
   $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
   try {
       $raw = $key.GetValue('Path', $null, 'DoNotExpandEnvironmentNames')
       $kind = if ($null -eq $raw) { 'ExpandString' } else { $key.GetValueKind('Path') }
       $parts = @("$raw" -split ';' | Where-Object {
           $_ -and -not [Environment]::ExpandEnvironmentVariables($_).StartsWith($upstream, 'OrdinalIgnoreCase')
       })
       $full = @($parts | ForEach-Object { [Environment]::ExpandEnvironmentVariables($_).TrimEnd('\') })
       if ($full -notcontains $bin) { $parts += $bin }
       $key.SetValue('Path', ($parts -join ';'), $kind)
   } finally {
       $key.Dispose()
   }
   ```

5. Set up the terminal hosts (next section) and start herdr from one of their profiles.

To update the fork later, rebuild from the fork source and repeat steps 2-3. Never use upstream's installer for that; it installs upstream herdr.

## Hosting in Windows Terminal and VS Code

Both hosts run the same `herdr.exe` client against one server, so groups, order, colors and bindings are shared live. The fork ships no host-specific code.

### The clean launcher (required)

The server that herdr starts automatically inherits the whole environment of the client that started it, and every pane inherits the server's environment. Upstream strips only a few agent-session variables from panes and deliberately keeps `ANTHROPIC_API_KEY`. So an API key exported in the parent environment would reach every agent pane, and a server first started from a VS Code terminal would hand every pane that window's `GIT_ASKPASS` and IDE-bridge variables, which break when the window closes.

[herdr-clean.cmd](herdr-clean.cmd) clears `ANTHROPIC_API_KEY`, `GIT_ASKPASS`, `CLAUDE_CODE_SSE_PORT`, `ELECTRON_RUN_AS_NODE` and every `VSCODE_*` variable inside its own `setlocal` scope, then starts `%LOCALAPPDATA%\Programs\Herdr\bin\herdr.exe` with your arguments. As a batch file it needs no execution-policy change.

- Both host profiles below launch herdr through it. Add more `set NAME=` lines if your parent environment carries other keys that panes should not see.
- It protects the server only if the first launch goes through it. If in doubt, run `herdr server stop` and relaunch from a profile: workspaces and agent sessions are restored.

### herdr config

In `%APPDATA%\herdr\config.toml` (existing keys only; the fork adds none):

```toml
[keys]
prefix = ["ctrl+b", "f12"]   # several prefixes are accepted; F12 is the fallback

[theme]
name = "catppuccin-latte"
```

Set the theme explicitly. With `auto_switch`, herdr falls back to the dark palette when the host's appearance is not detected; switch to `auto_switch = true` only after confirming by hand that both hosts report their appearance (the smoke checklist does not cover it).

### VS Code integrated terminal

In your user `settings.json`:

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

- Open herdr from the terminal's profile list (`herdr`).
- Ctrl+B is VS Code's sidebar toggle; the first line sends it to the terminal instead. `rightClickBehavior: nothing` leaves right-clicks to herdr's menus.
- Keep `terminal.integrated.minimumContrastRatio` at its default 4.5. Every fork color is at least 4.92:1 on white, so plain rows are unchanged; on catppuccin-latte's selected, focused and dragged rows VS Code adjusts some bars slightly, so they can look a little different from Windows Terminal.
- If Ctrl+B still does not reach herdr, use the F12 prefix, or add this to `keybindings.json`:

  ```jsonc
  { "key": "ctrl+b", "command": "workbench.action.terminal.sendSequence", "args": { "text": "\u0002" }, "when": "terminalFocus" }
  ```

- At 64 columns or fewer herdr switches to the mobile layout (grouped order, flat). Move the terminal into the editor area, or lower `ui.mobile_width_threshold`.

### Windows Terminal

Add a profile to `profiles.list` in Windows Terminal's `settings.json`:

```json
{ "name": "herdr", "commandline": "%LOCALAPPDATA%\\Programs\\Herdr\\bin\\herdr-clean.cmd",
  "startingDirectory": "%USERPROFILE%", "colorScheme": "One Half Light" }
```

Ctrl+B, F12 and Shift+Up/Down are expected to be unbound in Windows Terminal's defaults (smoke checks 1 and 6 confirm them). Shift+right-click is expected to reach Windows Terminal's own menu; confirm that by hand.

### Both at once

Attach from both hosts to the same server: an edit in either shows up in both. Collapse state is the exception: each client keeps its own, and the preferences file keeps whichever client saved last.

## herdr up

`herdr up` reads `%APPDATA%\herdr\projects.toml` (debug builds: `%APPDATA%\herdr-dev\`). Start from the example (run from the repository root; an existing `projects.toml` is kept):

```powershell
$projects = "$env:APPDATA\herdr\projects.toml"
if (-not (Test-Path -LiteralPath $projects)) { Copy-Item docs\next\fork\projects.example.toml $projects }
```

```text
herdr up <project>...      # named projects (case-insensitive)
herdr up --group <group>   # every project in that group; (re)creates the group even if deleted in the UI
herdr up --all             # every [[group]] and [[project]]
  --reapply                # re-assert file label/group/color on bound workspaces and name/color on bound groups,
                           # recreate groups deleted in the UI
```

The file:
- `[[group]]`: `name` (required, unique), `color` (one of the nine names).
- `[[project]]`: `name` (required, unique), `label` (default: the name), `cwd` (required, absolute after `~` expands to `%USERPROFILE%`), `group` (must name a `[[group]]`), `color` (a per-workspace override).
- `[[project.tab]]`: `label` (required, unique in the project), then either `agent` (with optional `args` and `agent_name`, default `<project>-<label>`) or `command` (typed into the shell, plus Enter), and an optional `cwd` (not on the first tab, which always starts in the project cwd).
- Names and labels are compared case-insensitively everywhere. Agent names follow upstream's rule (a lowercase letter, then lowercase letters, digits, `-` or `_`, at most 32 characters) and must be unique; a derived name that breaks the rule is a file error that asks for an explicit `agent_name`.

What it does:
- Groups match by the file name they were created from, so renaming a group in the UI keeps it bound. Workspaces are bound to their project and tabs to their label, by tab number, so a tab renamed in the UI is still found and never gets a second agent.
- An existing workspace is adopted when exactly one unbound workspace (not a linked worktree) has the project's cwd, else when exactly one has its label; several label matches skip the project as ambiguous, and no match creates a new workspace. A binding whose workspace now lies outside the project cwd is treated as stale and replaced.
- New agent tabs start through upstream's `agent.start` (retried for up to 15 s while the shell is busy, then the matching `herdr agent start` command is printed). New command tabs get the command typed in. Only tabs created in the same run are launched.
- Group, color and label are applied only when a workspace is created or first bound. After that, plain `up` never moves, recolors or renames it, so UI changes survive; `--reapply` asserts the file again. A group deleted in the UI stays deleted unless `--group <name>` or `--reapply` brings it back.
- It never deletes or closes anything, never reorders groups, never changes focus and never starts a server: start herdr from a host profile first. Panes inherit the server's environment, not the environment of the shell you run `up` from.
- A second run prints `up to date` for every project.

Exit codes: `0` applied or up to date; `1` server or API failure (the run stops at the first failed request), including `server does not support workspace groups (fork server required)` against an upstream server; `2` usage or file error, detected before any API call; `3` finished but skipped something (missing cwd, ambiguous adoption, a group or agent name already taken, or an agent start still busy).

## Going back to upstream or uninstalling

What upstream sees:
- `session.json` has upstream's schema and version, so upstream herdr of the same base loads it, including pane history. It shows the grouped order flat.
- Upstream never opens `fork-workspace-groups.json` or `projects.toml`, ignores the fork's collapse keys in client preferences, and finds no fork config keys.
- The fork never reads or writes `~/.claude`, panes run the unmodified `claude` CLI, and herdr's agent integrations are upstream's. The fork does not need `herdr integration install claude`; if you installed it, `herdr integration uninstall claude` removes it.

Steps:
1. `herdr server stop`.
2. Delete `%LOCALAPPDATA%\Programs\Herdr\bin`, which holds the fork's `herdr.exe`, `conpty\`, `THIRD-PARTY-NOTICES\` and `herdr-clean.cmd`. To go back to upstream, run upstream's installer only after that: it makes the folder a junction to its own release and puts that release first on your user PATH, but while the fork's folder is still there it stops with an error after it has already activated its release. To uninstall instead, also remove the folder from your user PATH. The host profiles still point at `herdr-clean.cmd`: point them at `herdr.exe` in the same folder, or remove them (step 4).
3. Optionally delete `%APPDATA%\herdr\fork-workspace-groups.json` and `projects.toml`.
4. Optionally remove the VS Code and Windows Terminal profile entries.

Returning to the fork later: install it again with Install steps 2-4 (step 4 matters, because upstream's installer put its own release first on PATH). Group records are keyed by workspace id, so workspaces upstream created appear ungrouped and anything upstream reordered is moved back into its group. After a period on upstream, a reissued workspace id can inherit an old record (a wrong group or color); one drag fixes a wrong group, and Workspace color..., Group color fixes a wrong color.

Manual abandon test: run an upstream release build of the same base against a copy of your herdr folder, and confirm workspaces, tabs and pane history restore. herdr uses `XDG_CONFIG_HOME\herdr` when `XDG_CONFIG_HOME` is set (also on Windows), so copy the `herdr` folder itself, not only its contents. Run it from a plain PowerShell window, or first `Remove-Item Env:HERDR_SOCKET_PATH, Env:HERDR_CLIENT_SOCKET_PATH -ErrorAction SilentlyContinue`, because herdr panes export those and would point the upstream build at the running fork server:

```powershell
$upstreamBuild = 'C:\path\to\upstream-build'      # folder holding the upstream herdr.exe
herdr server stop                                   # the fork server saves its session on exit
$scratch = Join-Path $env:TEMP ('herdr-abandon-' + (Get-Date -Format 'yyyyMMddHHmmss'))
New-Item -ItemType Directory $scratch | Out-Null
Copy-Item -Recurse "$env:APPDATA\herdr" "$scratch\herdr"
$env:XDG_CONFIG_HOME = $scratch
& "$upstreamBuild\herdr.exe"                        # check workspaces, tabs and pane history, then leave the client
& "$upstreamBuild\herdr.exe" server stop            # same shell, so this stops the test server
Remove-Item Env:XDG_CONFIG_HOME
```

## Building from source (no admin rights)

Upstream documents only the Visual Studio Build Tools route. The route below needs no admin rights: per-user Rust, a portable MSVC linker and SDK, and Zig as the only C/C++ compiler. This guide keeps the toolchain in `E:\tools`; any folder works if every path below changes with it. PowerShell is the reference shell; Git Bash also works because the linker path is absolute.

1. **Prerequisites (per user):** Python 3.11 or newer reachable as `python` (not the Microsoft Store alias), Bun 1.3.14, Git. `Get-ExecutionPolicy -List` should show MachinePolicy and UserPolicy as `Undefined`; a Group Policy value overrides the `-ExecutionPolicy Bypass` that `just lint` and `just check` use.
2. **Rust:** `rustup-init.exe -y --default-host x86_64-pc-windows-msvc --default-toolchain none --profile minimal` (always pass `-y`; the interactive "install Visual Studio" option needs admin rights). rustup and cargo live under `%USERPROFILE%\.rustup` and `%USERPROFILE%\.cargo`. Then, in the repository, `rustup toolchain install 1.96.1 --profile minimal -c clippy -c rustfmt` (the version `rust-toolchain.toml` pins).
3. **Portable MSVC:** download `portable-msvc.py` from the gist https://gist.github.com/7f3162ec2988e81e56d5c4e22cde9977 into `E:\tools\portable-msvc`, read it, then run `python -I portable-msvc.py --vs 2022 --sdk-version 26100 --host x64 --target x64` there and accept the Build Tools license. It writes `E:\tools\portable-msvc\msvc` and is not registered with Windows, so it never updates itself. Check that `link.exe`, `libcmt.lib`, `kernel32.lib` and `ucrt.lib` exist, then delete its `downloads\` folder.
4. **Zig 0.16.0:** the Windows x86_64 zip (SHA-256 `68659EB5F1E4EB1437A722F1DD889C5A322C9954607F5EDCF337BC3684A75A7E`), extracted to `E:\tools\zig-x86_64-windows-0.16.0`. Never move it afterwards: Zig's header cache keys include the install path.
5. **Zig libc file** `E:\tools\herdr-libc.txt`. `<V>` (`$V` below) is the single folder under `E:\tools\portable-msvc\msvc\VC\Tools\MSVC` (for example `14.44.35207`). The file must have LF line endings: Zig splits on `\n` only, and a CRLF file (as `Set-Content` writes) breaks the SIMD C++ build with `'cstring' file not found`. This writes it with LF:

   ```powershell
   $V = (Get-ChildItem -Directory 'E:\tools\portable-msvc\msvc\VC\Tools\MSVC').Name
   $lines = @(
       'include_dir=E:\tools\portable-msvc\msvc\Windows Kits\10\Include\10.0.26100.0\ucrt'
       "sys_include_dir=E:\tools\portable-msvc\msvc\VC\Tools\MSVC\$V\include"
       'crt_dir=E:\tools\portable-msvc\msvc\Windows Kits\10\Lib\10.0.26100.0\ucrt\x64'
       "msvc_lib_dir=E:\tools\portable-msvc\msvc\VC\Tools\MSVC\$V\lib\x64"
       'kernel32_lib_dir=E:\tools\portable-msvc\msvc\Windows Kits\10\Lib\10.0.26100.0\um\x64'
       'gcc_dir='
   )
   [IO.File]::WriteAllText('E:\tools\herdr-libc.txt', ($lines -join "`n") + "`n", [Text.Encoding]::ASCII)
   ```
6. **User cargo config** `%USERPROFILE%\.cargo\config.toml` (cargo merges it with the repository's `.cargo/config.toml`, which keeps `+crt-static`):

   ```toml
   [target.x86_64-pc-windows-msvc]
   linker = 'E:\tools\portable-msvc\msvc\VC\Tools\MSVC\<V>\bin\Hostx64\x64\link.exe'

   [env]
   ZIG = 'E:\tools\zig-x86_64-windows-0.16.0\zig.exe'
   LIBGHOSTTY_VT_WINDOWS_LIBC = 'E:\tools\herdr-libc.txt'
   LIB = 'E:\tools\portable-msvc\msvc\VC\Tools\MSVC\<V>\lib\x64;E:\tools\portable-msvc\msvc\Windows Kits\10\Lib\10.0.26100.0\ucrt\x64;E:\tools\portable-msvc\msvc\Windows Kits\10\Lib\10.0.26100.0\um\x64'
   ```

   The absolute linker path keeps Git Bash's `/usr/bin/link.exe` from shadowing MSVC's. No `cl.exe`, vcvars or MSVC on PATH is needed. If MSVC or the SDK is updated, update this file and `herdr-libc.txt` together.
7. **Tools:** `just` 1.59.0 and `cargo-nextest` 0.9.149 from their prebuilt Windows zips into `%USERPROFILE%\.cargo\bin`.

Build and check, from the repository root:

```powershell
$env:CARGO_PROFILE_DEV_DEBUG = '1'    # mirrors CI; smaller PDBs
$env:CARGO_PROFILE_TEST_DEBUG = '1'
cargo build --locked                  # debug build
just lint                             # cargo fmt --check + clippy -D warnings
just check                            # lint + tests + debug build
just build                            # release: target\release\herdr.exe + conpty\ + THIRD-PARTY-NOTICES\
```

- Never set `RUSTFLAGS`: it replaces `+crt-static`, and `just build` rejects a `herdr.exe` that imports the dynamic CRT.
- A one-off Zig fetch error (`failed caching recompressed tarball ... FileNotFound`) is not a code bug: run the command again. If the SIMD C++ step fails for another reason, `$env:LIBGHOSTTY_VT_SIMD = 'false'` is the fallback.
- Defender real-time scanning slows cargo and Zig, and exclusions need admin rights. Before debugging a local test failure, compare with the fork's CI result for the same commit.
- Debug builds use `herdr-dev` folders, never the installed server's state. When testing one from inside a herdr pane, first `Remove-Item Env:HERDR_SOCKET_PATH, Env:HERDR_CLIENT_SOCKET_PATH -ErrorAction SilentlyContinue`.
- Fallback when policy blocks the `msiexec /a` that `portable-msvc.py` uses: xwin 0.10.0 (`xwin --accept-license --arch x86_64 splat --copy --disable-symlinks --output E:\tools\xwin`), the libc file from `scripts/windows_cross.py` (`libc_contents`), `linker = 'rust-lld.exe'`, `LIB` pointed at the xwin `crt` and `sdk` lib folders, and `LIBGHOSTTY_VT_SIMD = 'false'` in the user cargo `[env]`.
- CI baseline: pushing `fork:windows` runs upstream's `ci.yml` on the public fork (ubuntu, macos and windows `check` jobs). That run is the check of record.

## Syncing with upstream

| Remote | URL | Use |
|--|--|--|
| `upstream` | `https://github.com/herdrdev/herdr.git` | fetch only |
| `origin` | `https://github.com/scott4885/herdr.git` | public fork; Actions enabled |
| `wkentaro` | `https://github.com/wkentaro/herdr.git` | optional; source of the tab-rows commit |

Branch `fork` tracks `upstream/master` and pushes to `origin` (`git config branch.fork.pushRemote origin`, `git config rerere.enabled true`). `origin/master` stays a plain mirror of upstream master. The base is master, not the latest stable tag: upstream cuts stable releases from a separate checkout, so stable tags are not on master.

Rebase at least at each upstream stable release (the update notifier is the signal), and whenever an upstream fix is wanted:

1. `git fetch upstream --tags`
2. `git switch fork`, then `git rebase upstream/master`.
3. Resolve conflicts. For `docs/next/api/herdr-api.schema.json` take upstream's version (`git checkout --ours docs/next/api/herdr-api.schema.json`; during a rebase "ours" is upstream); step 5 regenerates it. In `src/client/shell/sidebar.rs`, `endpoint_sidebar.rs` and `mouse.rs`, re-apply the fork's calls by hand; never resolve them blindly.
4. Right after the rebase, review with `git range-diff @{u} fork@{1} fork`: every fork commit should pair up, and changed pairs show what the conflict resolution altered.
5. `$env:HERDR_UPDATE_API_SCHEMA='1'; just test-one generated_protocol_schema_artifact_is_current; Remove-Item Env:HERDR_UPDATE_API_SCHEMA` (never leave the variable set: while set, that test rewrites the file and passes). Re-pin the `workspace_group.edit` digest only if that method's own shape changed. Commit both with `git commit --fixup=<sha of stack commit 2>`, then `git rebase --autosquash upstream/master`.
6. `just check`, then `just build`.
7. `herdr server stop`, install the build (Install, step 3), and run the smoke checklist.
8. `git push --force-with-lease origin fork fork:windows`. Upstream's CI workflow runs on pushes to `windows`, so the fork gets CI without editing `ci.yml`.

Fold later fixes into their stack commit with `git commit --fixup=<sha>` and `git rebase --autosquash upstream/master`.

The fork is kept as six commits on top of upstream, ordered so each file's fork hunks live in one commit:

1. `docs: add fork design spec`
2. `feat: persist workspace groups in a sidecar state file`
3. `feat: show workspace tab rows and groups in the sidebar`
4. `feat: add herdr up for project templates`
5. `chore: block self-update in fork builds`
6. `docs: add fork guide, example projects file and clean launcher`

Checks after every rebase: `git diff --diff-filter=M --shortstat upstream/master...fork -- . ':!docs/next/api/herdr-api.schema.json'` stays at about 300 changed lines or fewer, and `git diff upstream/master...fork -- Cargo.toml Cargo.lock` prints nothing (no new crates).

## Smoke checklist results

Run in both hosts attached to one server, with a light color scheme. Status: not run yet.

| # | Check | Windows Terminal | VS Code |
|--|--|--|--|
| 1 | Ctrl+B and F12 both enter prefix mode | not run | not run |
| 2 | `prefix+?` opens help | not run | not run |
| 3 | Drag a workspace into and out of a group | not run | not run |
| 4 | Drag a header to reorder groups | not run | not run |
| 5 | Right-click on a header opens the group menu | not run | not run |
| 6 | In `prefix+w`: Shift+Up/Down crosses a group boundary; `m` opens Organize; Edit group... reaches an empty group | not run | not run |
| 7 | Delete a group, then Undo delete group restores it | not run | not run |
| 8 | A workspace with 2+ tabs shows tab rows, a single-tab workspace does not, and clicking a tab row focuses that tab | not run | not run |
| 9 | Bars and the colored focused tab are readable, and all 9 colors are distinguishable side by side | not run | not run |
| 10 | `herdr up --all` creates tabs and starts agents; a second run prints "up to date"; renaming a tab and re-running starts nothing | not run | not run |
| 11 | With `ANTHROPIC_API_KEY` set in the parent environment and no server running, start herdr from the profile; in a PowerShell pane `Get-ChildItem Env: \| Where-Object Name -match '^(ANTHROPIC_API_KEY\|VSCODE_\|GIT_ASKPASS$\|CLAUDE_CODE_SSE_PORT$\|ELECTRON_RUN_AS_NODE$)'` prints nothing; repeat with the server first started from the other host | not run | not run |
| 12 | Both hosts attached at once show the same change within a second of an edit in either | not run | not run |
| 13 | Each agent session can be resumed from the VS Code extension for its folder | not run | not run |
| 14 | Create two groups (one empty), give one workspace a color override, run `herdr up --all`; (a) `herdr server stop` and relaunch from a host profile; (b) wait at least 10 s, end the herdr server process in Task Manager and relaunch; after each, all groups including the empty one, membership, group order, colors and collapse state return, and `herdr up --all` prints "up to date" | not run | not run |

Manual abandon test (Going back to upstream): not run yet.

## Credits

- herdr is developed by the herdr authors at `github.com/herdrdev/herdr` under the Apache-2.0 license; this fork keeps that license.
- The sidebar tab rows contain code ported and modified from wkentaro/herdr commit `29484fc4` (Kentaro Wada), licensed under Apache-2.0 (section 4(b) of the license); `src/client/shell/sidebar_tabs.rs` carries the attribution header.
