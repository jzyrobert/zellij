---
title: "feat: Web client path deep-linking via ?path= query argument"
type: feat
status: completed
date: 2026-05-17
deepened: 2026-05-17
---

# feat: Web client path deep-linking via `?path=` query argument

## Summary

Let users deep-link the Zellij web client to a filesystem path: visiting `<base_url>/?path=/srv/projects/foo` or `<base_url>/work?path=/srv/projects/foo` lands the user in a shell whose working directory is the supplied path. The mechanism reuses the existing `CliAssets.cwd` plumbing for new sessions (the web flow currently hardcodes `cwd: None`) and adds a single new IPC message variant — `ChangeFocusedPaneCwdIfShellMsg` at protobuf field 21 — for the "existing session, no foreground command" case, which delegates the safety check (is the focused pane at a shell prompt?) to the server side that already tracks `terminal_foreground_cmds`. The browser strips `?path=` from the URL via `history.replaceState` after it has been forwarded, so refreshing the tab doesn't re-trigger the deep link.

To stay portable across shells without per-shell escape logic, the validator rejects single quotes, common shell metacharacters, and `..` path components — the on-wire payload is therefore always the fixed shape `cd '<path>'\n` with no escape pass needed. The cd-if-shell action only fires when the focused pane's spawn command is in a sh-family allowlist (bash, zsh, sh, dash, ksh, ash, mksh, fish); for csh/tcsh/PowerShell/cmd/nushell the action is a silent no-op. New-session deep linking is unaffected by shell choice because it flows through `CliAssets.cwd`, not through a typed command.

---

## Problem Frame

Today the only way to start a Zellij web client in a specific directory is to attach to a session that was previously created with that cwd (via the CLI `zellij --new-session-with-layout` or by `cd`-ing inside the running session). There is no URL-level mechanism to express "open the web client here", which makes it impossible to:

- Link to a project directory from a dashboard, README, or chat message.
- Bookmark a "start in `~/code/foo`" launch in the browser.
- Drive the web client from external automation that already knows a target path.

The web client already accepts a session name in its URL (`/{session}` → `serve_html` → `/ws/terminal/{session}`). Adding a `?path=` query argument extends that with one more dimension of launch state. The minimal change touches three layers: the frontend reads `location.search` and forwards the path on the WebSocket connect URL; the WebSocket handler validates and forwards it to the session-listener thread; the session-listener thread routes it either into `CliAssets.cwd` (new session) or into a new "cd if shell" IPC message (existing session). `CliAssets.cwd` already flows all the way to `zellij-server/src/lib.rs:995-997` where it becomes the default cwd for every spawned terminal in the new session, so no server-side schema change is needed for the new-session path. The existing-session path needs one new `ClientToServerMsg` variant and a small server-side router branch.

---

## Requirements

- R1. Visiting `<base_url>/?path=<path>` creates a new session whose initial pane(s) have working directory `<path>` (skipping the welcome plugin when a path is supplied). Cites origin: feature description from user prompt.
- R2. Visiting `<base_url>/{session_name}?path=<path>` attaches to `session_name` if it exists, or creates it if not; on creation the cwd is `<path>`; on attach, if (a) the focused pane has no foreground command (server-side `terminal_foreground_cmds[focused_terminal]` is `Some(empty_vec)`) AND (b) the pane's spawn command basename is in the sh-family allowlist (R9), the server writes the fixed-shape payload `cd '<path>'\n` to that pane. Otherwise the action is a silent no-op.
- R3. The deep-link path is URL-decoded and then validated as follows. All rules must hold; failure produces silent drop for shape-invalid paths (control bytes, null bytes — looks like an attack probe) and a `WebServerToWebClientControlMessage::LogError` surface for semantically-invalid paths (relative, oversized, contains rejected metacharacters). The validator's `log::warn!` records the rejection reason and a fingerprint (length + first-byte category) of the input, never the full path, so log readers cannot enumerate the filesystem via repeated invalid requests.
  - Must be absolute (first byte is `b'/'`).
  - Length ≤ 4096 bytes.
  - Contains no null byte, no `\r`/`\n`, no other ASCII control character (< `0x20`).
  - Contains no single quote (`'`) — collapses the on-wire payload to a fixed shape with no escape pass.
  - Contains no shell metacharacter from `;`, `&`, `|`, `` ` ``, `$`, `!`, `<`, `>`, `*`, `?`, `(`, `)`, `{`, `}`, `\`, `"` — defense-in-depth against non-sh-family shells (the U4 allowlist is the primary defense; this is belt-and-braces).
  - Contains no `..` path component (after `PathBuf` component decomposition) — deep-link URLs must be self-evident; obscuring final cwd via traversal is rejected even though the authenticated user could otherwise do it.
- R4. After the path has been forwarded to the WebSocket on initial connect, the browser strips `?path=…` from the address bar via `history.replaceState`, so reload, bookmark, and PWA `start_url` semantics all stay clean. The forwarded path is not echoed back into the URL even on session switch (`SwitchedSession` control message handling unchanged). If the initial WebSocket connect fails after `?path=` is stripped, the user must reload with the original deep link; the stripped state is final and not restored on connect failure (documented one-shot semantic).
- R5. Existing web-client behaviour is unchanged when no `?path=` is present: routes (`/`, `/{session}`, `/ws/terminal`, `/ws/terminal/{session}`) keep their current semantics, the welcome plugin still shows on `/`, and existing tests in `zellij-client/src/web_client/unit/web_client_tests.rs` still pass without modification.
- R6. The `base_url` reverse-proxy contract is preserved. Deep links work under both `base_url = "/"` and a non-trivial `base_url` (e.g., `"/zellij/"`) without per-deployment changes.
- R7. Read-only clients silently ignore `?path=` for both new-session creation (already blocked by the existing read-only-cannot-create-session guard at `server_listener.rs:105-109`) and existing-session attach. The U4 server route resolves the read-only state via the connection's authenticated `ClientId` (the same lookup that gates `Action::WriteCharsToPaneId` today), NOT via any field in the deep-link message itself, so a read-only client cannot impersonate a non-read-only one. The read-only check fires before the foreground-cmd lookup, so the variant cannot be used by a read-only viewer as a process-presence oracle.
- R8. Unit tests cover: query-arg parsing, every R3 validation rejection (relative path, control chars, null bytes, single quote, each shell metacharacter, `..` component, oversized path), new-session cwd propagation into `CliAssets`, existing-session attach with empty foreground cmd + allowlisted shell triggers a `cd`, existing-session attach with active foreground cmd is a no-op, existing-session attach with a non-allowlisted shell (csh/tcsh/PowerShell/cmd/nushell) is a no-op, read-only attach with `?path=` is a no-op, the protobuf wire-format round-trip for `ChangeFocusedPaneCwdIfShellMsg`, the AttachClient → ChangeFocusedPaneCwdIfShell ordering race, the `SwitchedSession` round-trip does not re-apply the original deep link, and the WebSocket-connect-fails-after-strip UX is asserted by a comment-documented test.
- R9. The cd-if-shell action only fires when the focused pane's spawn command basename matches one of: `bash`, `zsh`, `sh`, `dash`, `ksh`, `ash`, `mksh`, `fish`. For all other shells (`csh`, `tcsh`, `nushell`/`nu`, `powershell`/`pwsh`, `cmd`, anything else) the server logs at trace level and drops the request. The allowlist lives in a single named constant (e.g., `CD_IF_SHELL_ALLOWLIST`) in `zellij-server/src/route.rs` so future shell additions are a one-line change.
- R10. Cross-version IPC compatibility: when the connected `zellij-server` predates this change, the newer client's `ChangeFocusedPaneCwdIfShell` message is silently dropped by prost's unknown-oneof decoding (existing behavior; see `zellij-utils/src/ipc.rs:98-176`). This matches the deep-link feature's best-effort posture (R3) — no user-visible error is raised, and the user can still `cd` manually. A `cargo test -p zellij-utils ipc::tests::roundtrip_tests` case for the new variant locks the wire-format contract so a future schema edit cannot silently break it.

---

## Scope Boundaries

- No new `zellij.kdl` configuration surface. The deep-link feature is unconditional — if `?path=` is present and valid, it is applied; otherwise current behaviour. No per-deployment allowlist of paths.
- No multi-path / multi-pane semantics. A single `?path=` applies to a single new session or a single focused pane on attach. We do not split panes or open tabs to host the path.
- No file-open semantics. `?path=` is always interpreted as a directory cwd. Even if the path points at a file, we still `cd` to it (the shell will error) or use it as cwd for the new session's initial pane (the server's cwd resolution will fall back to home).
- No tilde expansion (`~/foo`). Frontend cannot resolve the server's `$HOME`, and server-side tilde expansion is one more validation surface than the MVP justifies. Users encode absolute paths.
- No URL-fragment based deep links (`/#path=…`). Fragments are not sent to the server in the HTTP request and would force a client-side-only mechanism; query args are simpler.
- No protocol-handler registration (`zellij://path=…`). PWA install + standard `?path=` URLs are enough; a custom scheme is a separate, OS-specific concern.
- No retroactive deep linking. If the user navigates to `/foo?path=/bar` after the page has already loaded a session, the path is ignored (deep linking only applies on initial WebSocket connect). Single-page route changes are not part of the current web-client navigation model anyway.

### Deferred to Follow-Up Work

- A more permissive path mode (relative paths, tilde, environment-variable expansion, single quotes via per-shell escape). Useful if users start hitting the absolute-only restriction in practice. Tilde expansion is the most likely first ask — defer until requested.
- Welcome-screen integration that pre-fills "new session in <path>" when `/?path=` is opened but the user has multiple sessions and wants to pick one. The current plan bypasses welcome when a path is given; a future iteration could show welcome with the path "primed".
- "Open file" mode (`?file=/path/to/foo.rs`) that opens an editor on the file. Different action shape, separate plan.
- Cd-if-shell support for csh/tcsh, nushell, PowerShell, cmd, and any other non-sh-family shells. Requires per-shell escape rules and a richer shell-detection mechanism (`SHELL` env, parent process inspection). Defer until requested by a real user; today the affected users get the new-session cwd path which is shell-agnostic.
- Frame-based deep-link handoff: send the path inside the first WebSocket message rather than as a URL query argument, eliminating the access-log / reverse-proxy-log exposure described under Risk Analysis. Larger client/server change for a marginal logging-privacy gain; revisit if a user reports the log leak as a concrete operational issue.
- A `path` field on `ConnectToSession` that survives `SwitchSession` so deep-link semantics also apply when the server tells a client to switch sessions. The current plan keeps the path strictly first-attach-only; cross-session propagation needs more thought about whether it's even desirable. Note that `ConnectToSession.cwd` is already live in the CLI/plugin flow (see Context & Research) — extending those semantics to the web flow is a separate scope decision.

---

## Context & Research

### Relevant Code and Patterns

- `zellij-client/assets/index.js:13` — `const sessionName = location.pathname.split("/").pop();`. Single point where the JS reads route state from the URL. The new query-arg parsing lives next to it.
- `zellij-client/assets/websockets.js:104-110` — `wsTerminalUrl` is built as `${url}${queryString}` where `queryString = "?web_client_id=…"`. The path query arg is appended here.
- `zellij-client/assets/websockets.js:291-295` — `SwitchedSession` handler does `window.location.href = …`. Unchanged, but its existence is the reason the `?path=` strip via `history.replaceState` must happen *before* any session switch can re-render the URL.
- `zellij-client/src/web_client/types.rs:177-179` — `TerminalParams { web_client_id }`. The new optional `path` field is added here as `Option<String>` (axum's `Query` extractor handles optionality).
- `zellij-client/src/web_client/websocket_handlers.rs:37-46` and `141-195` — `ws_handler_terminal` → `handle_ws_terminal`. This is where `params.path` is unpacked and passed (after validation) into `zellij_server_listener`.
- `zellij-client/src/web_client/server_listener.rs:23-47` — `zellij_server_listener` signature is the chokepoint that owns the session-state lifecycle. A new `initial_cwd: Option<PathBuf>` parameter slots in alongside `session_name`. `build_initial_connection` is called on line 41 — that is where the cwd becomes part of `ConnectToSession`.
- `zellij-client/src/web_client/session_management.rs:14-45` — `build_initial_connection`. Today's `should_start_with_welcome_screen` branch always picks the welcome layout when `session_name.is_none()`. When `initial_cwd.is_some()`, we override that to "new session, default layout, cwd set" so the deep link doesn't dead-end on welcome.
- `zellij-client/src/web_client/session_management.rs:58-134` — `create_first_message`. The two `cwd: None` lines (104, 123) are the literal substitution targets for the new session path: the `FirstClientConnected` branch sets `cwd: initial_cwd`, the `AttachClient` branch keeps `None` (cwd is not the lever used for existing-session deep linking — see the new IPC variant below).
- `zellij-utils/src/data.rs:2942-2948` — `ConnectToSession { name, tab_position, pane_id, layout, cwd }`. `cwd` is already part of the struct and is **live** in the CLI/plugin flow: `src/commands.rs:743-744` reads it into `new_session_cwd` for the CLI reconnect/`SwitchSession` loop, `zellij-server/src/plugins/zellij_exports.rs:382-389` and `zellij-server/src/route.rs:1158-1164` propagate it when a plugin calls `SwitchSession`. The web flow's `build_initial_connection` currently uses `..Default::default()` so the field is `None` there today; setting it as part of this plan is additive and aligned with the existing cross-binary semantics (cwd survives `SwitchSession` and becomes the receiving client's `new_session_cwd`). The plan keeps the web flow's behavior conservative — `initial_cwd` is consumed on first-attach only and never re-applied across `SwitchedSession` — but a future plan could extend the field to survive switches without changing the field itself.
- `zellij-server/src/lib.rs:995-997` — `let cwd = cli_assets.cwd.or_else(|| runtime_config_options.default_cwd);` is where the new-session cwd lands. No server change needed for R1/R2 (new-session creation).
- `zellij-server/src/pty.rs:209, 2138-2176` — `terminal_foreground_cmds: HashMap<u32, Vec<String>>` is the periodic foreground-command scan. `Some(empty_vec)` for a terminal means "no foreground child of the shell" — the precise signal we use for "no running command" in R2's cd-if-shell check. **Missing map entry** means "no scan has run for this terminal yet" and is distinct from `Some(empty_vec)`; see U4 for the deferred-retry handling.
- `zellij-server/src/background_jobs.rs:114, 184-196` — `UPDATE_AND_REPORT_CWDS_INTERVAL_MS = 1000`. The foreground-command scan cadence is 1 second, not the 200ms mentioned during earlier estimation. The U4 design routes around this with a deferred-retry path so a freshly-spawned pane's first-second deep-link is not silently dropped.
- `zellij-server/src/pty.rs:208 (terminal_cmds)` — tracks the spawn command per terminal (the shell that was originally launched in the pane). The U4 shell-allowlist check reads the basename of this command, NOT `terminal_foreground_cmds` (which contains *children* of the shell, not the shell itself).
- `zellij-utils/src/client_server_contract/client_to_server.proto:6-28` — `ClientToServerMsg` is a protobuf `oneof` with existing tags 1..=20 (last is `host_terminal_theme_changed = 20`). Existing variants follow the convention `XxxYyyMsg xxx_yyy_zzz = N;` (e.g., `FirstClientConnectedMsg first_client_connected = 7;`). The new variant takes tag **21** and follows the same naming.
- `zellij-utils/assets/prost_ipc/client_server_contract.rs` — 3250-line **checked-in generated file**. Editing the `.proto` is not enough; the regenerated `.rs` must also be committed. The regen command is `cargo xtask` (verify the exact subcommand in `xtask/` before implementing).
- `zellij-utils/src/ipc/tests/roundtrip_tests.rs` — every existing `ClientToServerMsg` variant has a wire-format round-trip test. The new variant follows the same pattern and gives us a backstop against silent drift between `.proto` and the generated `.rs`.
- `zellij-utils/src/ipc.rs` (search for `pub enum ClientToServerMsg`) — the place to add the new `ChangeFocusedPaneCwdIfShell { path: PathBuf }` variant. The companion router branch lives in `zellij-server/src/route.rs` (search for the existing `Action::WriteChars` and `Action::WriteCharsToPaneId` handlers around lines 259-286 for the pattern).
- `zellij-utils/src/input/actions.rs:127-136` — `WriteChars` and `WriteCharsToPaneId` actions. The new IPC message ultimately reuses the same screen-instruction tail (write bytes to a specific terminal id) and only adds the foreground-command and shell-allowlist guards on the way in.
- `zellij-client/src/web_client/server_listener.rs:181-189` — existing `Log` / `LogError` control-message routing. U2 reuses this surface for semantically-invalid deep-link paths (so the user sees feedback in the terminal area before the shell prompt) while keeping shape-invalid drops silent.
- `zellij-client/src/web_client/server_listener.rs:147-155` — `ServerToClientMsg::Connected` is sent after the attach handshake completes. U4 uses this as the trigger to send `ChangeFocusedPaneCwdIfShell`, ensuring `active_panes[client_id]` is populated server-side before the route handler runs.
- `zellij-client/src/web_client/unit/web_client_tests.rs:258-465` — `test_full_session_flow` is the canonical end-to-end pattern: spawn `serve_web_client` on `127.0.0.1:0`, drive HTTP login + `/session` POST + WebSocket connect with cookie. New deep-link tests reuse `MockSessionManager` / `MockClientOsApiFactory` to assert on the first message sent to the (mocked) server.
- `docs/plans/2026-05-17-001-feat-pwa-support-web-client-plan.md` — sibling PWA plan in the same branch, same author, same conventions. Mirror its frontmatter, section order, test-mode (loopback `TcpListener` + isahc + mocks), and prose style.

### Institutional Learnings

- The web client deliberately uses a single chokepoint helper (`sendSizeUpdate` in `websockets.js:50`) for protocol-shaped messages so the contract lives in one place. The new path-forwarding logic follows the same instinct: the JS side has exactly one site that knows about `?path=` (in `index.js`), and the Rust side has exactly one site (`handle_ws_terminal`) that validates it.
- PR #4981 ("make web client use base URL when switching sessions") established that the web client must never construct absolute URLs that bypass `base_url`. The `?path=` query arg is a query-arg-only feature and does not touch path routing, so this constraint is satisfied trivially.
- The web flow's `ClientToServerMsg::FirstClientConnected` forces `web_server: Some(true)` and `web_sharing: Some(WebSharing::On)` (see `session_management.rs:80-82`). That precedent — the web client overriding config options at message-construction time — is the model we follow when synthesizing the cwd: derived once, baked into `CliAssets`, not stored as long-lived state.

### External References

- [URLSearchParams — MDN](https://developer.mozilla.org/en-US/docs/Web/API/URLSearchParams) — used to parse `?path=` on the frontend. Handles URL decoding implicitly.
- [history.replaceState — MDN](https://developer.mozilla.org/en-US/docs/Web/API/History/replaceState) — used to scrub the path query arg from the address bar after read.
- POSIX shell single-quoting rules — the server-side `cd '<path>'\n` payload uses single quotes with a `'\''` escape for any single quote inside the path. This is the simplest universally-portable shell escape and is what `shell-escape` crates and standard tools (e.g., GNU `printf %q` in single-quote mode) emit.

---

## Key Technical Decisions

- **Deep-link mechanism for new sessions: `CliAssets.cwd` (existing field, no schema change).** The web flow already constructs `CliAssets` in `create_first_message`. The two `cwd: None` lines flip to `cwd: initial_cwd.clone()` on the new-session branch only. The cwd then flows to `zellij-server/src/lib.rs:995-997` where it becomes the default cwd for every terminal spawned in the new session. This is the minimal, server-schema-free path for R1 and is shell-agnostic (no typed command is involved).
- **Deep-link mechanism for existing sessions: new IPC variant `ClientToServerMsg::ChangeFocusedPaneCwdIfShell { path: PathBuf }`, protobuf field 21 (`ChangeFocusedPaneCwdIfShellMsg change_focused_pane_cwd_if_shell = 21;`).** Sent by the web client **after the client observes `ServerToClientMsg::Connected`** (NOT immediately after sending `AttachClient`). This delays the second message until the server has installed `active_panes[client_id]`, eliminating the race where the route handler could otherwise see an unattached client. The server then inspects (in this order, short-circuiting on any failure): (1) the issuing client is not read-only (resolved via connection's `ClientId`, NOT via any message field), (2) the active pane is a `PaneId::Terminal(_)` not `PaneId::Plugin(_)`, (3) the spawn-command basename of that terminal is in `CD_IF_SHELL_ALLOWLIST` (R9), (4) `terminal_foreground_cmds.get(terminal_id) == Some(empty_vec)`. If all four hold, the server routes `WriteCharsToPaneId { chars: format!("cd '{}'\n", path), pane_id }` (no escape pass — the validator guarantees `'` is absent). If `terminal_foreground_cmds.get(terminal_id) == None` (no scan has run yet for this terminal — possible within the first 1000ms after spawn), the route handler enqueues a single retry on the next scan tick and decides then. After one retry it drops. Rationale: keeps the "is the shell idle?" check on the side with authoritative data; keeps the web client free of pty introspection; protects against the 1000ms `UPDATE_AND_REPORT_CWDS_INTERVAL_MS` false-drop window.
- **Path validator rejects single quotes and shell metacharacters so the on-wire payload is fixed-shape.** Combined with R3's `..` rejection and the U4 shell allowlist, this means the wire payload is always literally `cd '<validated path>'\n` — no per-shell escape table, no `'\''` quote-toggling, no shell-introspection at payload-build time. Rationale: per-shell quoting is the primary security and portability landmine for this feature; eliminating quotes from the input space eliminates the failure mode entirely. The cost is a slightly more restricted path space (paths with `'` cannot be deep-linked); the workaround is to rename or symlink.
- **Path validation is server-side authoritative; the frontend does only cosmetic prechecks.** The JS does a cosmetic check (truthy + length cap) before forwarding, but the authoritative validation runs in `handle_ws_terminal` immediately after `Query` extraction. Shape-invalid paths (control chars, null) → silent drop with fingerprinted log. Semantically-invalid paths (relative, oversized, contains rejected metacharacter, contains `..`) → `LogError` control message to the browser ("deep-link path rejected: <reason>") so the user sees feedback in the xterm area before the shell prompt. Rationale: trust boundary is the server; silent drop for shape attacks avoids enumeration, but typo'd deep links deserve a visible signal.
- **No welcome screen when `?path=` is given.** `build_initial_connection` today picks `LayoutInfo::BuiltIn("welcome")` whenever `session_name.is_none()`. When `initial_cwd.is_some()` and `session_name.is_none()`, we instead generate a unique session name and pick the default layout from config (or `None` for the framework's built-in default). Rationale: a deep link is an explicit signal that the user wants a shell at a specific cwd; bouncing them through the welcome plugin and asking them to confirm would defeat the purpose. R5 (no `?path=` → unchanged) keeps welcome intact for the default flow.
- **Path is URL-encoded once, by the frontend.** `URLSearchParams.set("path", rawPath)` does the encoding; the server side relies on axum's `Query` extractor's built-in URL-decoding once. We do not double-encode through `JSON.stringify` or stuff the path into a header. Rationale: query args are the natural transport; the existing `web_client_id` query arg already follows this pattern. A double-encoded path (`?path=%252Ftmp`) decodes once to `%2Ftmp`, fails the absolute-path check, and is rejected — accepted edge case, covered by a test.
- **`?path=` is stripped from the address bar after read via `history.replaceState`.** Done once, immediately after `URLSearchParams.get("path")` has been captured into a local variable, before any await. The replaced URL preserves session-name (if present) and drops all query args. Rationale: deep links should be one-shot. A user who refreshes the tab should re-attach to the same session and not re-`cd` (which would be confusing if they had since `cd`'d somewhere else). Trade-off documented in R4: if the initial connect fails, the user must reload with the original deep link rather than relying on the address bar to remember it. This is the intended behavior; do not re-write the URL on connect failure (would cause re-`cd` after manual retry).
- **`initial_cwd` is consumed via `Option::take()` exactly once before the reconnect loop.** Implemented as `let initial_cwd_first_shot = initial_cwd.take();` outside the loop, then passed into `build_initial_connection` on first iteration only. `SwitchedSession`-triggered reconnects (the only path that re-enters the loop body) see `None` unconditionally. Rationale: makes the "first-attach-only" invariant a Rust-level guarantee rather than a comment, and prevents a future refactor from accidentally re-firing the deep link.
- **Read-only clients silently ignore `?path=`.** For new sessions, the existing read-only-cannot-create-session guard at `server_listener.rs:105-109` already kicks in before any cwd application. For attach, the U4 route handler resolves the connection's authenticated `ClientId` (same mechanism that gates `Action::WriteCharsToPaneId` today) and short-circuits before the foreground-cmd lookup. Rationale: matches the existing security posture (read-only = no state mutation, including cwd nudges) and avoids leaking process-presence information to read-only viewers.

---

## High-Level Technical Design

This illustrates the intended approach and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.

```
Browser                          axum HTTP server                   Session listener thread             zellij-server
─────────                        ─────────────────                  ─────────────────────              ──────────────
location.search has ?path=/foo
  │
  ├── URLSearchParams.get("path")
  ├── history.replaceState (strip ?path=)
  └── WebSocket connect to:
        /ws/terminal/{session}
          ?web_client_id=…
          &path=%2Ffoo  ──────►   ws_handler_terminal
                                   │
                                   ├── Query<TerminalParams { web_client_id, path: Option<String> }>
                                   ├── validate_deep_link_path(path)
                                   │     ├── Ok(Some(PathBuf))   → forward
                                   │     ├── Ok(None)            → no path supplied, no message
                                   │     └── Err(reason)         → LogError("deep-link rejected: …") to browser
                                   └── zellij_server_listener(…, initial_cwd: validated)
                                                                       │
                                                                       ├── let initial_cwd = initial_cwd.take();   // consumed once
                                                                       ├── build_initial_connection(session_name, initial_cwd, &config)
                                                                       │    │
                                                                       │    └── if cwd.is_some() && session_name.is_none():
                                                                       │           ConnectToSession {
                                                                       │             name: Some(generated),
                                                                       │             layout: default_layout,
                                                                       │             cwd: Some(path),
                                                                       │             ..
                                                                       │           }
                                                                       │
                                                                       ├── session_exists?
                                                                       │    │
                                                                       │    ├── no  → create_first_message(should_create_new_session=true,
                                                                       │    │                              initial_cwd=Some(path), …)
                                                                       │    │         ⇒ FirstClientConnected { cli_assets { cwd: Some(path) } }
                                                                       │    │                                                  ──────────────►  spawns terminals with cwd
                                                                       │    │
                                                                       │    └── yes → AttachClient { cli_assets { cwd: None } }
                                                                       │             wait for ServerToClientMsg::Connected
                                                                       │             then (only when initial_cwd.is_some()):
                                                                       │             ChangeFocusedPaneCwdIfShell { path }    ──────────────►  resolve client_id → focused pane
                                                                       │                                                                       1. is_read_only?  → drop
                                                                       │                                                                       2. PaneId::Plugin?→ drop
                                                                       │                                                                       3. spawn_cmd basename ∉ allowlist?
                                                                       │                                                                          → drop
                                                                       │                                                                       4. terminal_foreground_cmds[id]?
                                                                       │                                                                          - Some(empty) → write "cd 'path'\n"
                                                                       │                                                                          - Some(cmd)   → drop (trace log)
                                                                       │                                                                          - None        → defer 1 scan tick,
                                                                       │                                                                                          re-check, then drop
                                                                       │
                                                                       └── 'reconnect_loop iteration N (N>1, triggered by SwitchSession):
                                                                              initial_cwd is None — deep link is not re-applied
```

---

## Implementation Units

### U1. Parse and forward `?path=` from the browser

**Goal:** Frontend reads the deep-link path from `location.search`, forwards it as a `path` query arg on the terminal WebSocket URL, and scrubs `?path=` from the address bar so reloads don't re-trigger it.

**Requirements:** R1, R2, R4, R6.

**Dependencies:** None.

**Files:**
- `zellij-client/assets/index.js` — read `URLSearchParams.get("path")` before constructing the WebSocket; call `history.replaceState` to strip query args (preserving the pathname so the session name in the URL stays); pass `deepLinkPath` into `initWebSockets`.
- `zellij-client/assets/websockets.js` — `initWebSockets` accepts a new `deepLinkPath` parameter and appends `&path=<encoded>` to `queryString` when truthy.
- `zellij-client/src/web_client/unit/web_client_tests.rs` — extend `test_index_html_…` style assertions to confirm `index.js` content (served via `get_static_asset` → `assets/index.js`) is unchanged for non-path requests; no new HTML assertion is needed.

**Approach:**
- One read of `URLSearchParams.get("path")` in `index.js`, immediately followed by `history.replaceState(null, "", location.pathname)`. The replacement intentionally drops all query args; the only one currently present on the page URL is `path` (the `?web_client_id=…` query arg lives on the WebSocket URL, not the page URL).
- The path is treated as opaque text on the frontend. No validation, no normalization. The frontend only refuses to forward if the string is empty after trimming. `URLSearchParams.set` and the WebSocket URL constructor handle encoding.
- `initWebSockets` builds `queryString` by composing key/value pairs through `URLSearchParams` (replacing today's hand-built `?web_client_id=…` concatenation) so the encoding for both `web_client_id` and the optional `path` flows through the same primitive.

**Patterns to follow:** `assets/utils.js:getBaseUrl` / `getWebSocketBaseUrl` are the existing one-screen-wide helpers for URL composition. Add `deepLinkPath` handling inline in `initWebSockets` rather than introducing a new utils helper — there is exactly one call site.

**Test scenarios:**
- Pure JS unit test (or in-test browser flow if the test suite stays Rust-side only): when `location.search = "?path=%2Ftmp%2Ffoo"`, the constructed WebSocket URL contains `&path=%2Ftmp%2Ffoo` after `web_client_id`.
- After deep-link forward, `location.search` is empty (`history.replaceState` was called with a query-less URL).
- When `location.search` is empty, the WebSocket URL is unchanged from current behavior (no trailing `&path=`).
- When `location.search = "?path="` (empty string after `=`), the WebSocket URL omits the path arg.
- When the session name is in the URL (`/work?path=/foo`), the replaced URL is `/work` (pathname preserved).
- Covers AE: a user opening `<base_url>/?path=/srv` in their browser observes the address bar become `<base_url>/` after the page finishes loading.

**Verification:** Manually open `http(s)://<host>/?path=/tmp` in a browser, confirm the address bar drops `?path=/tmp` once xterm finishes initializing, and confirm Chromium devtools "Network" tab shows the WebSocket request URL including `&path=%2Ftmp`. Existing tests in `web_client_tests.rs` still pass.

---

### U2. Wire path through axum query extraction and `zellij_server_listener`

**Goal:** Server-side extraction of the new `path` query arg, validation, and forwarding into the session-listener thread as `initial_cwd: Option<PathBuf>`.

**Requirements:** R3, R5, R6, R7.

**Dependencies:** U1 (the WebSocket URL must carry the new arg).

**Files:**
- `zellij-client/src/web_client/types.rs` — extend `TerminalParams { web_client_id, path: Option<String> }`. `Deserialize` derive already handles missing field as `None` because of `Option`.
- `zellij-client/src/web_client/utils.rs` — new function `validate_deep_link_path(raw: &str) -> Result<Option<PathBuf>, DeepLinkPathError>` plus the `DeepLinkPathError` enum (variants for each rejection class: `Empty`, `NotAbsolute`, `TooLong`, `ControlCharacter`, `SingleQuote`, `ShellMetacharacter(char)`, `ParentComponent`). `Ok(None)` means "no path supplied" (silent); `Ok(Some(_))` means "validated"; `Err(_)` means "semantically rejected, surface to user".
- `zellij-client/src/web_client/websocket_handlers.rs` — in `handle_ws_terminal`, call `validate_deep_link_path` on `params.path.as_deref()` before passing it into `zellij_server_listener`. On `Err(reason)`, emit a `WebServerToWebClientControlMessage::LogError { lines: vec![format!("deep-link path rejected: {}", reason)] }` to the client's control channel (after the channel is established) and pass `initial_cwd: None` to the listener. On `Ok(None)` or shape-invalid silent drops, pass `None` without LogError. Update `zellij_server_listener` call site (line 185-195) to pass the new `initial_cwd` argument.
- `zellij-client/src/web_client/server_listener.rs` — `zellij_server_listener` signature gains `initial_cwd: Option<PathBuf>`. **Implementation detail:** the parameter is moved into the listener thread, then `let initial_cwd_first_shot = initial_cwd.take();` is bound **before** the `'reconnect_loop` (not inside it). The first iteration of the loop reads `initial_cwd_first_shot.take()` exactly once and passes the value into `build_initial_connection`. Subsequent iterations always see `None`. This guarantees that a `SwitchedSession` re-entry, a panic-and-retry inside iteration 1, or any other re-entry pattern cannot re-fire the deep link.
- `zellij-client/src/web_client/unit/web_client_tests.rs` — new tests for `validate_deep_link_path` cases and for the LogError surface.

**Approach:**
- `validate_deep_link_path` rules (all must hold for `Ok(Some(_))`):
  - `raw` non-empty (else `Err(Empty)`; raw-empty is distinct from never-supplied — the absence case is handled before this function is called).
  - First byte is `b'/'` (else `Err(NotAbsolute)`).
  - Length ≤ 4096 bytes (else `Err(TooLong)`).
  - No byte is `< 0x20` (full ASCII control set, including null, CR, LF, BEL); else `Err(ControlCharacter)` (silent drop on the websocket_handlers side — this looks like an attack probe).
  - No `'` character (else `Err(SingleQuote)`).
  - No character in `{;, &, |, `, $, !, <, >, *, ?, (, ), {, }, \, "}` (else `Err(ShellMetacharacter(c))`).
  - `PathBuf::from(raw).components().all(|c| c != Component::ParentDir)` (else `Err(ParentComponent)`).
- The `handle_ws_terminal` wrapper decides what to do with each error:
  - `Err(ControlCharacter)` → silent drop. Log only the rejection class + length + first-byte category (e.g., "ControlCharacter, len=14, leading_byte_class=ascii_printable"). Never log the path itself; this prevents enumeration via log inspection.
  - All other `Err(_)` → emit `LogError` to the browser with a human-readable reason ("deep-link path rejected: must be absolute", "…must not contain shell metacharacters", etc.). Server-side log is at `debug` level with the same fingerprint shape (no full path).
  - `Ok(None)` → no message, no log (no path was supplied).
  - `Ok(Some(p))` → log at `debug` with fingerprint only ("deep-link accepted, len=N").
- The validator does not call `.exists()` — that is a TOCTOU race against the server-side spawn, and the server will gracefully fall back to home directory when the cwd does not exist (existing server behavior). Symlinks and unreadable directories are also fine; the server already handles those.

**Patterns to follow:** axum `Query<T>` extractor with `Deserialize` (existing pattern at `websocket_handlers.rs:40`). For LogError surfacing, the existing `server_listener.rs:181-189` shows the message shape. For validation, the codebase prefers `log::warn!` over panics for non-fatal request errors — see `websocket_handlers.rs:127` `log::error!("Failed to deserialize client msg: ...")`.

**Test scenarios:**
- `validate_deep_link_path("/tmp")` → `Ok(Some(PathBuf::from("/tmp")))`.
- `validate_deep_link_path("/home/user/with spaces and-éscapéd")` → `Ok(Some(...))` (spaces and UTF-8 allowed).
- `validate_deep_link_path("relative/path")` → `Err(NotAbsolute)`.
- `validate_deep_link_path("")` → `Err(Empty)`.
- `validate_deep_link_path("/path/with\0null")` → `Err(ControlCharacter)`.
- `validate_deep_link_path("/path/with\nnewline")` → `Err(ControlCharacter)`.
- `validate_deep_link_path("/path/with\rreturn")` → `Err(ControlCharacter)`.
- `validate_deep_link_path("/path/with\x07bell")` → `Err(ControlCharacter)`.
- `validate_deep_link_path("/path/with'quote")` → `Err(SingleQuote)`.
- `validate_deep_link_path("/path/with;semicolon")` → `Err(ShellMetacharacter(';'))`.
- `validate_deep_link_path("/path/with$dollar")` → `Err(ShellMetacharacter('$'))`.
- `validate_deep_link_path("/path/with`backtick`")` → `Err(ShellMetacharacter('` '`))`.
- `validate_deep_link_path("/path/with\\backslash")` → `Err(ShellMetacharacter('\\'))`.
- `validate_deep_link_path("/foo/../bar")` → `Err(ParentComponent)`.
- `validate_deep_link_path("/..")` → `Err(ParentComponent)`.
- `validate_deep_link_path(&"/".repeat(5000))` → `Err(TooLong)`.
- `validate_deep_link_path("%2Ftmp")` → `Err(NotAbsolute)` (double-encoded path; axum decodes the outer `%25` to `%`, leaving `%2Ftmp` which is not absolute).
- Integration: `handle_ws_terminal` invoked with `params.path = Some("/tmp")` passes `Some(PathBuf::from("/tmp"))` into `zellij_server_listener`, no LogError emitted.
- Integration: `handle_ws_terminal` invoked with `params.path = Some("relative")` passes `None` into `zellij_server_listener` AND emits one `LogError` control message containing "must be absolute".
- Integration: `handle_ws_terminal` invoked with `params.path = Some("/foo\0bar")` passes `None`, emits NO `LogError` (silent drop for shape-invalid), logs only fingerprint.
- Integration: `handle_ws_terminal` invoked with no `path` query arg behaves identically to today (regression guard for R5).
- `initial_cwd` lifetime: after the listener consumes it once, a synthetic `SwitchedSession` round-trip is observed not to re-apply the cwd (regression for the `Option::take()` discipline).
- Covers AE: a malicious URL `?path=%00` is dropped silently with no log leakage; a typo URL `?path=Users/foo` produces a visible "deep-link path rejected: must be absolute" message in the xterm area before the shell prompt.

**Verification:** Run `cargo test -p zellij-client web_client::unit::` — all pre-existing tests pass, new unit tests for `validate_deep_link_path` pass, and the new integration test exercising the path-extraction path returns a deterministic result.

---

### U3. Apply `initial_cwd` to new-session creation via `CliAssets.cwd`

**Goal:** When the deep-linked session is new (does not exist on the server yet), the `cli_assets.cwd` field of `ClientToServerMsg::FirstClientConnected` is populated with the validated path, which the server then uses as the default cwd for every spawned terminal in the new session.

**Requirements:** R1, R2 (new-session half), R6.

**Dependencies:** U2.

**Files:**
- `zellij-client/src/web_client/session_management.rs` — `build_initial_connection` gains an `initial_cwd: Option<PathBuf>` parameter. New branching: when `initial_cwd.is_some()` and `session_name.is_none()`, bypass the welcome layout and instead emit `ConnectToSession { name: Some(generated_name), layout: default_layout_from_config, cwd: initial_cwd, .. }`. `create_first_message` gains an `initial_cwd` parameter, used to populate `cli_assets.cwd` in the `FirstClientConnected` branch; the `AttachClient` branch keeps `cwd: None` (see U4 for existing-session handling).
- `zellij-client/src/web_client/server_listener.rs` — extract the cwd from the `reconnect_info.cwd` after `build_initial_connection`, thread it into `create_first_message`. The reconnect loop sets `initial_cwd = None` after the first iteration as described in U2.
- `zellij-client/src/web_client/unit/web_client_tests.rs` — new tests that drive `test_full_session_flow`-style mocks and assert the resulting `FirstClientConnected` message has the expected `cwd`.

**Approach:**
- `build_initial_connection` decision matrix:
  - `session_name: None`, `initial_cwd: None`: welcome screen (current behaviour).
  - `session_name: None`, `initial_cwd: Some(path)`: new session, default layout, `cwd: Some(path)` — bypasses welcome.
  - `session_name: Some(s)`, `initial_cwd: None`: attach-or-create with default layout (current behaviour).
  - `session_name: Some(s)`, `initial_cwd: Some(path)`: attach-or-create with default layout, `cwd: Some(path)` — if new session, becomes pane cwd; if existing, U4 handles the cd.
- `create_first_message`'s `FirstClientConnected` branch substitutes `cwd: initial_cwd` for today's hardcoded `cwd: None` at session_management.rs:104. The `AttachClient` branch (line 123) stays `cwd: None`.
- The `MockSessionManager::spawn_session_if_needed` interceptor in the existing tests already exposes the first message — extend the mock to capture `cli_assets.cwd` and assert on it.

**Patterns to follow:** `session_management.rs:14-45` for the `build_initial_connection` shape. Today it returns `Ok(Option<ConnectToSession>)` — keep the same Result/Option shape, just extend the inner variants.

**Test scenarios:**
- New session, `?path=/tmp/foo`: `MockSessionManager` receives `FirstClientConnected { cli_assets: CliAssets { cwd: Some(PathBuf::from("/tmp/foo")), .. }, .. }`.
- New session, no `?path=`: `cli_assets.cwd` is `None` (regression guard).
- `/?path=/tmp/foo`: session-name-less deep link bypasses welcome layout (`cli_assets.layout` is the configured default, not `LayoutInfo::BuiltIn("welcome")`).
- `/?` with no path: still gets welcome layout (regression guard for R5).
- Existing session, `?path=/tmp/foo`: `MockSessionManager` receives `AttachClient { cli_assets: CliAssets { cwd: None, .. }, .. }` — the cwd is *not* baked into `CliAssets` on attach (U4 handles attach).
- Read-only client, new session, `?path=/tmp/foo`: existing read-only-cannot-create-session guard fires and connection closes; cwd never reaches the mock.
- Covers AE: user visits `/?path=/srv/projects/foo`, lands in a fresh session whose initial pane's `PWD` env var is `/srv/projects/foo`.

**Verification:** New tests in `web_client_tests.rs` pass; `test_full_session_flow` still passes.

---

### U4. New IPC variant `ChangeFocusedPaneCwdIfShellMsg` (proto field 21) and server-side routing with shell allowlist + deferred-scan retry

**Goal:** When deep-linking to an existing session whose focused pane is (a) owned by a sh-family shell from the allowlist AND (b) has no foreground command, the server writes the fixed-shape payload `cd '<path>'\n` to that pane. Any other state — non-allowlisted shell, foreground command running, plugin pane, read-only client, freshly-spawned-pane-no-scan-yet (after one retry) — silently drops with a trace log.

**Requirements:** R2 (existing-session half), R7 (read-only no-op), R9 (shell allowlist), R10 (cross-version compat).

**Dependencies:** U2 (the path must already be validated and quote/metachar-free; this unit does not re-validate).

**Files:**
- `zellij-utils/src/client_server_contract/client_to_server.proto` — add `ChangeFocusedPaneCwdIfShellMsg change_focused_pane_cwd_if_shell = 21;` to the `ClientToServerMsg` oneof. Define the message body as `message ChangeFocusedPaneCwdIfShellMsg { string path = 1; }`. Field number 21 is the next unused tag after `host_terminal_theme_changed = 20`; field 1 inside the new message is `path` as a `string` (the U2 validator guarantees UTF-8 and absence of control bytes, so `string` rather than `bytes` is appropriate).
- `zellij-utils/assets/prost_ipc/client_server_contract.rs` — regenerated, committed alongside the .proto change. Verify with `git diff` that the generated diff is additive (new oneof case, new message struct, new conversion impl); no renumbered fields, no removed messages. Regeneration command: inspect `xtask/` for the prost-regen subcommand (likely `cargo xtask regenerate-prost` or similar — the exact name lives in xtask source, not in this plan; document the chosen command in the PR body).
- `zellij-utils/src/ipc.rs` — add `ChangeFocusedPaneCwdIfShell { path: PathBuf }` to the `ClientToServerMsg` enum.
- `zellij-utils/src/ipc/protobuf_conversion.rs` — add the `From`/`TryFrom` conversion between the new Rust variant and the new proto message.
- `zellij-utils/src/ipc/tests/roundtrip_tests.rs` — add a round-trip test case for the new variant (locks the wire-format contract; see R10).
- `zellij-server/src/route.rs` — new branch in the `ClientToServerMsg` match. Branch logic in this exact order, short-circuiting on the first failure:
  1. Look up `client_id`'s read-only status via the existing connection-table lookup (same call site as `Action::WriteCharsToPaneId` uses today). Read-only → trace log, return.
  2. Look up `active_panes[client_id]`. If missing (race with attach completion) → enqueue one retry on next screen-thread tick, return; if still missing on retry → trace log, return.
  3. If pane is `PaneId::Plugin(_)` → trace log, return.
  4. Look up the spawn-command basename via `terminal_cmds[terminal_id]` (the *shell* command, NOT `terminal_foreground_cmds` which tracks children). Match basename against `CD_IF_SHELL_ALLOWLIST = ["bash", "zsh", "sh", "dash", "ksh", "ash", "mksh", "fish"]`. Not in allowlist → trace log, return.
  5. Look up `terminal_foreground_cmds.get(terminal_id)`:
     - `Some(empty_vec)` → proceed.
     - `Some(non_empty)` → trace log (include the rejected foreground command basename for debuggability), return.
     - `None` → defer one scan tick (await `UPDATE_AND_REPORT_CWDS_INTERVAL_MS = 1000ms`, then re-evaluate exactly once). After retry, treat `None` as "still running, drop".
  6. Build chars: `format!("cd '{}'\n", path.display())`. No escape pass — the U2 validator guarantees `path` contains no `'`, no shell metachar, no control byte. Route as the equivalent of `Action::WriteCharsToPaneId { chars, pane_id }`, reusing the existing `ScreenInstruction` plumbing.
- `zellij-server/src/pty.rs` — expose two small helpers: `terminal_spawn_cmd_basename(terminal_id: u32) -> Option<String>` (reads `terminal_cmds[id]`, returns `Path::new(cmd[0]).file_name()` lowercased) and `foreground_cmd_state(terminal_id: u32) -> ForegroundCmdState` (returns `Idle | Running | Unknown` matching the three `get` cases above). Both are cheap synchronous reads — no new scan is triggered.
- `zellij-client/src/web_client/server_listener.rs` — after `session_manager.spawn_session_if_needed` returns, the listener loop drains server messages. **Trigger the deep-link send when `ServerToClientMsg::Connected` arrives** (not before): if `initial_cwd_first_shot.is_some()` and `!should_create_new_session`, send `ClientToServerMsg::ChangeFocusedPaneCwdIfShell { path }` via `os_input.send_to_server`. This guarantees server-side `active_panes[client_id]` is populated before the message arrives. The trigger fires exactly once per listener lifetime (gated by `initial_cwd_first_shot.take()`).
- `zellij-client/src/web_client/unit/web_client_tests.rs` — new tests that mock `session_exists -> true`, drive the full attach flow, and assert the second message sent by the listener is `ChangeFocusedPaneCwdIfShell` after a synthetic `ServerToClientMsg::Connected` is delivered.

**Approach:**
- **No shell escape pass.** The U2 validator rejects single quotes and shell metacharacters in the path; combined with the U4 shell allowlist (only sh-family shells, where single-quoted strings are literal), the wire payload is always exactly `cd '<path>'\n` with `<path>` substituted byte-for-byte. This eliminates the entire class of "did we escape this character correctly for this shell" bugs.
- **Shell allowlist via spawn command, not via `SHELL` env.** Zellij's existing tracking of `terminal_cmds` records what was actually spawned in the pane (which respects `default_shell` config, CLI overrides, and `chsh`). `SHELL` env at server-process start could be stale. The allowlist matches `Path::new(cmd[0]).file_name()` case-insensitively for robustness across `/bin/bash`, `/usr/bin/bash`, `bash`, `BASH`, etc.
- **Read-only check first**, before any pane lookup. Reading a process-presence answer back to a read-only viewer would be a side-channel; resolving read-only before foreground keeps the side-channel closed (see R7).
- **Deferred-scan retry on missing map entry** is bounded: one retry on next scan tick, hard cap of 1100ms total wait. After that, drop. Rationale: REL-001 — without the retry, deep-linking immediately after pane spawn has a ~1s false-drop window; with unbounded retries, a stuck pty thread could leak the deep link indefinitely. One retry is the sweet spot.
- **No new low-level write mechanism.** The chars reach the pty via the existing `Action::WriteCharsToPaneId` / `ScreenInstruction::WriteCharacter` plumbing (`route.rs:286`).
- **Cross-version compat**: a newer client sending `ChangeFocusedPaneCwdIfShell` to an older server is decoded by prost as an unknown oneof variant (no error, no crash, no log). The deep-link cd is silently dropped; the new-session cwd path still works because it flows through the unchanged `FirstClientConnected` variant.

**Patterns to follow:** Existing `Action::WriteCharsToPaneId` branch in `zellij-server/src/route.rs:286` — same `ScreenInstruction::WriteCharacter` target. For protobuf extensions, mirror `HostTerminalThemeChangedMsg host_terminal_theme_changed = 20;` at the end of the existing `oneof`. For the round-trip test, mirror the closest existing variant test in `zellij-utils/src/ipc/tests/roundtrip_tests.rs`.

**Test scenarios:**
- Existing session, focused pane = bash, empty `terminal_foreground_cmds`: server emits the equivalent of `WriteCharsToPaneId { chars: "cd '/tmp/foo'\n", pane_id: <focused> }`. Exact byte-for-byte payload asserted.
- Existing session, focused pane = bash, non-empty `terminal_foreground_cmds` (e.g., `vim`): no write is emitted; trace log records the rejected foreground command basename.
- Existing session, focused pane = bash, `terminal_foreground_cmds.get(id) == None` (race with scan): server defers one tick; after retry returns `None`, drop. (Test uses a controllable mock scan that returns `None` consistently.)
- Existing session, focused pane = bash, `terminal_foreground_cmds.get(id) == None` then `Some(empty)` after retry: server emits the write on the retry.
- Existing session, focused pane = csh: no write is emitted (allowlist miss). Same for `tcsh`, `nushell`, `nu`, `pwsh`, `powershell`, `cmd`, `cmd.exe`, and an unknown command like `/usr/local/bin/myshell`.
- Existing session, focused pane is a plugin: no write is emitted.
- Read-only client attaching to existing session with `?path=/tmp`: no write is emitted, AND the foreground-cmd lookup is not reached (asserted by spy/instrumentation that records the order of map accesses).
- Race regression: `ChangeFocusedPaneCwdIfShell` arriving before `active_panes[client_id]` is populated → server enqueues retry; after attach completes, retry succeeds and the write is emitted. (This validates the deferred-attach path even though the client is now expected to wait for `Connected` first; defense-in-depth.)
- Path with embedded space (`/tmp/with space`): the emitted chars are exactly `cd '/tmp/with space'\n`. (Single quote, no escape pass — the validator already rejected `'`.)
- Protobuf wire-format round-trip in `roundtrip_tests.rs`: a `ClientToServerMsg::ChangeFocusedPaneCwdIfShell { path: PathBuf::from("/tmp/foo") }` encoded to bytes and decoded back yields an equal value. Encoded bytes include the expected field-21 tag.
- `SwitchedSession` round-trip: after a successful deep-linked attach, a synthetic server-side `SwitchSession` is observed; the listener's next iteration does NOT re-send `ChangeFocusedPaneCwdIfShell` (`initial_cwd_first_shot` already consumed).
- Covers AE: user opens `<base_url>/work?path=/tmp/foo` while session "work" exists and is sitting at a bash prompt — the bash prompt shows `cd '/tmp/foo'` followed by a newline, the shell processes it, and `pwd` returns `/tmp/foo`. Same URL while session "work" is running `vim` produces no text injection.

**Verification:** `cargo test -p zellij-utils ipc` covers the new enum variant and round-trip; `cargo test -p zellij-server route` covers the routing including the allowlist, race, and read-only branches; `cargo test -p zellij-client web_client` covers the client-side trigger (gated on `Connected`). Manual smoke: attach to an existing bash session via `<base_url>/<name>?path=/some/dir` and confirm the `cd` lands; attach again while `vim` is open and confirm no `cd` text is injected; attach to a csh-default session and confirm no `cd` text is injected.

---

### U5. End-to-end test coverage and CHANGELOG entry

**Goal:** Backstop the feature with end-to-end style tests that exercise the full path through `serve_web_client` (HTTP login → `/session` POST → WebSocket connect with `?path=/tmp/x`) and assert the mocked session manager observed the cwd or the cd-if-shell trigger. Update `CHANGELOG.md` with a one-line entry following the project's convention.

**Requirements:** R8, R10.

**Dependencies:** U1, U2, U3, U4.

**Files:**
- `zellij-client/src/web_client/unit/web_client_tests.rs` — add multiple new tests following the `test_full_session_flow` template (loopback `TcpListener`, isahc HTTP, `connect_async_with_cookie` for WebSocket, `MockSessionManager` + `MockClientOsApiFactory` for assertions).
- `CHANGELOG.md` — add one line under the most recent unreleased section in the existing prose style: e.g., `* feat(web): support deep-linking to a directory via ?path= query parameter (PR-URL).` Match the formatting of the surrounding entries (which already exist for the PWA work — see commits `a5330e08`, `c25fa066`).

**Approach:**
- New-session E2E test: drive the WebSocket connect with `?web_client_id=…&path=%2Ftmp%2Fdeep` and assert `MockSessionManager` captures the `FirstClientConnected` message with `cli_assets.cwd == Some(PathBuf::from("/tmp/deep"))`.
- Existing-session E2E test: pre-seed `MockSessionManager` to return `session_exists(name) == true`; drive the WebSocket connect with `?path=/tmp/deep`; install a spy on `MockClientOsApiFactory::send_to_server` to capture both the `AttachClient` and the follow-up `ChangeFocusedPaneCwdIfShell` messages; assert their ordering, and that the second message is sent only after a synthetic `ServerToClientMsg::Connected` is delivered by the mock.
- LogError surface test: drive the WebSocket connect with `?path=relative`; assert the control-channel mock receives one `LogError` message whose `lines[0]` contains "must be absolute".
- Silent-drop test: drive the WebSocket connect with `?path=%00`; assert NO `LogError` is emitted and the session attaches normally.
- Comment-documented test for failed-connect UX: in a doc comment on `test_deep_link_new_session_cwd_propagates`, document that the `?path=` strip in `index.js` happens before await and is not reversed on WebSocket failure (the test does not exercise this since it lives below the browser layer; the expectation is documented for future debugging).
- CHANGELOG entry placement matches `b558b31e docs(changelog): update release` style — add to the existing unreleased section, do not create a new release header.

**Patterns to follow:** `test_full_session_flow` (`web_client_tests.rs:258`) for the WebSocket-flow scaffolding; `test_index_html_references_manifest_and_pwa_meta_tags` for HTML-content style assertions (not applicable here but same file pattern).

**Test scenarios:**
- `test_deep_link_new_session_cwd_propagates`: WebSocket URL with `&path=%2Ftmp%2Fdeep` → `MockSessionManager` saw `FirstClientConnected { cli_assets: CliAssets { cwd: Some("/tmp/deep"), .. }, .. }`.
- `test_deep_link_existing_session_emits_cd_if_shell`: WebSocket URL with `&path=%2Ftmp%2Fdeep`, session pre-existing, mock delivers `Connected` → second message sent to mock server is `ChangeFocusedPaneCwdIfShell { path: PathBuf::from("/tmp/deep") }`.
- `test_deep_link_existing_session_no_send_before_connected`: same as above but mock does NOT deliver `Connected` → no `ChangeFocusedPaneCwdIfShell` is sent within a bounded wait (regression guard for the race fix).
- `test_deep_link_invalid_path_relative_surfaces_log_error`: WebSocket URL with `&path=relative` → control channel receives a `LogError` with "must be absolute"; `MockSessionManager` saw `cwd: None`; no `ChangeFocusedPaneCwdIfShell` emitted.
- `test_deep_link_invalid_path_null_byte_is_silent_drop`: WebSocket URL with `&path=%00` → no `LogError` emitted, no `ChangeFocusedPaneCwdIfShell` emitted, session attaches normally.
- `test_deep_link_double_encoded_path_is_rejected`: WebSocket URL with `&path=%252Ftmp` → axum decodes once to `%2Ftmp`, validator rejects (not absolute), `LogError` surfaces.
- `test_deep_link_not_reapplied_on_switch_session`: drive a deep-link attach, then deliver a synthetic `SwitchedSession` via the mock; assert no second `ChangeFocusedPaneCwdIfShell` is sent and the second iteration's `CliAssets.cwd` is `None`.
- `test_full_session_flow` unchanged → regression guard for R5.
- Covers AE: the documented examples in R1 and R2 are each exercised at least once end-to-end.

**Verification:** `cargo test -p zellij-client web_client` is green. `cargo test -p zellij-utils ipc::tests::roundtrip_tests` includes the new variant. `git diff CHANGELOG.md` shows one new entry in the unreleased section, matching surrounding formatting.

---

## Risk Analysis & Mitigation

- **Risk: shell escape bypass / non-POSIX shell mis-execution** — A `cd '<path>'\n` payload landing at a `csh`, `tcsh`, PowerShell, `cmd`, or nushell prompt could syntax-error, produce a literal directory-named-`'tmp` cd, or in pathological cases (path containing `;` under cmd.exe) execute as multiple statements. The MVP plan assumed POSIX single-quoting is portable; it is not. **Mitigation:** the U2 validator rejects single quotes and a conservative shell-metacharacter set so the payload is fixed-shape with no escape pass needed; the U4 server route additionally restricts cd-if-shell to a sh-family allowlist (`bash`, `zsh`, `sh`, `dash`, `ksh`, `ash`, `mksh`, `fish`) matched against the pane's spawn command basename. Non-allowlisted shells silently no-op. Test coverage in U4 includes each non-allowlisted shell explicitly.
- **Risk: TOCTOU on path existence** — Validating the path exists at HTTP handler time and then having it disappear before `FirstClientConnected` is processed by the server. **Mitigation:** we do not validate existence (see U2 approach). The server's existing cwd resolution falls back to home dir if cwd is unreachable, so the worst case is a session in the home directory with a single warning log.
- **Risk: existing-session deep link races foreground-cmd scan** — A user opens `?path=…` immediately after a pane is spawned; the foreground-cmd scan runs every `UPDATE_AND_REPORT_CWDS_INTERVAL_MS = 1000ms`, so `terminal_foreground_cmds.get(id)` can legitimately return `None` for up to ~1s. Naively treating missing as "running" silently drops legitimate deep links in this window. **Mitigation:** U4 distinguishes `None` from `Some(empty_vec)` and defers one scan tick (1100ms bounded) on missing entries. After retry, treat `None` as still-running and drop. Test coverage in U4 includes the "missing → defer → still missing → drop" and "missing → defer → empty → write" branches.
- **Risk: race between `AttachClient` and `ChangeFocusedPaneCwdIfShell`** — The server's route handler for the cd-if-shell variant looks up `active_panes[client_id]`, but `AttachClient` and `ChangeFocusedPaneCwdIfShell` are processed by different actors on the server. FIFO IPC ordering guarantees arrival order but not side-effect order, so the second message could reach the route handler before attach has installed the pane mapping. **Mitigation:** the client waits for `ServerToClientMsg::Connected` (which is sent post-attach-handshake) before sending `ChangeFocusedPaneCwdIfShell`. As defense-in-depth, the server route also defers one screen-thread tick when `active_panes[client_id]` is missing.
- **Risk: read-only clients exploiting the new IPC variant** — A read-only client could send `ChangeFocusedPaneCwdIfShell` directly via a hand-crafted WebSocket payload, or use the variant as a process-presence side-channel against panes they otherwise have no write access to. **Mitigation:** the U4 route.rs branch resolves the connection's authenticated `ClientId` (NOT any field in the message) and short-circuits before the foreground-cmd lookup, so neither write effects nor process-presence answers reach a read-only viewer.
- **Risk: `?path=` value leaks into access logs / reverse-proxy logs** — Both axum's default tower-http logging and the reverse-proxies that the deployment supports record the full WebSocket-upgrade URL, including the `?path=` query argument. The path is PII-adjacent (project names, customer names, `/home/$user/…`). **Mitigation:** explicitly document in the PR body that the deep-link path is logged by HTTP infrastructure and should be treated as not-secret. The U2 server-side logs deliberately fingerprint (length + first-byte category) rather than recording the raw path, so the only path-leak surface is the access-log layer that the operator controls. Frame-based handoff is in Deferred to Follow-Up Work if a real user reports the log exposure as a concrete issue.
- **Risk: cross-version IPC compatibility** — A newer client could be deployed against an older server (or vice versa) that does not understand the new `ChangeFocusedPaneCwdIfShell` variant. **Mitigation:** prost decodes unknown oneof variants as `None`, so older servers silently drop the message (cd-if-shell becomes a no-op; new-session cwd still works because it flows through the unchanged `FirstClientConnected` variant). R10 makes this explicit; the U5 round-trip test in `zellij-utils/src/ipc/tests/roundtrip_tests.rs` locks the wire format so a future schema edit cannot silently break it.
- **Risk: `initial_cwd` re-application across reconnect-loop iterations** — A naive implementation that holds `initial_cwd` in a captured outer variable could re-apply the deep link across `SwitchedSession` cycles or panic-and-retry. **Mitigation:** U2 specifies `let initial_cwd_first_shot = initial_cwd.take();` outside the loop, with a single `.take()` inside the first iteration. Compile-time guarantee, not a runtime invariant. Test coverage in U5 includes `test_deep_link_not_reapplied_on_switch_session`.
- **Risk: PWA `start_url` interaction** — Installed PWA always launches with `start_url = "../"` (which resolves to `<base_url>/`). A user who installs the PWA after using a `?path=` link does *not* preserve the path in the install. **Mitigation:** explicitly documented in Scope Boundaries. No code change needed; the PWA plan's `start_url` is intentionally path-arg-free.
- **Risk: failed initial WebSocket connect leaves `?path=` stripped** — `history.replaceState` runs before the WebSocket connects. If the connect fails, reload does not restore the deep link; the user must paste the original URL again. **Mitigation:** R4 documents this as the intended one-shot semantic. Re-writing the URL on failure would risk re-`cd`'ing after a manual retry succeeds, which is worse.

---

## System-Wide Impact

- **`zellij-utils/src/ipc.rs`** gains a new `ClientToServerMsg::ChangeFocusedPaneCwdIfShell { path: PathBuf }` variant. All consumers of this enum must handle it — search for `match …` on `ClientToServerMsg` across the workspace (`zellij-server/src/route.rs`, `zellij-server/src/lib.rs`, IPC test fixtures). Older servers receiving the new variant fall through prost's unknown-oneof handling as `None` (no crash).
- **`zellij-utils/src/client_server_contract/client_to_server.proto`** gains a new oneof entry at field 21 (`ChangeFocusedPaneCwdIfShellMsg change_focused_pane_cwd_if_shell = 21;`) plus a new message definition. The checked-in generated `zellij-utils/assets/prost_ipc/client_server_contract.rs` is regenerated and committed.
- **`zellij-utils/src/ipc/protobuf_conversion.rs`** gains `From`/`TryFrom` conversions for the new variant. **`zellij-utils/src/ipc/tests/roundtrip_tests.rs`** gains a wire-format round-trip test case.
- **`zellij-server/src/route.rs`** gains a single new route branch that reads `terminal_cmds[id]` (for shell allowlist), `active_panes[client_id]` (with one-tick deferred retry if absent), and `terminal_foreground_cmds[id]` (with one-scan deferred retry if absent), then emits a `WriteChars`-style instruction or drops with trace log. No new long-lived state. No changes to the pty foreground-scan cadence.
- **`zellij-server/src/pty.rs`** gains two cheap synchronous read helpers (`terminal_spawn_cmd_basename`, `foreground_cmd_state`) for the route branch to query without triggering a new scan.
- **`zellij-client/src/web_client`** gains a new optional field on `TerminalParams`, a new validator in `utils.rs` (returns `Result<Option<PathBuf>, DeepLinkPathError>`), and changes to three call-chains: `ws_handler_terminal` → `zellij_server_listener` → `build_initial_connection` → `create_first_message`. The server-listener loop also gains a `Connected`-triggered deep-link send.
- **Frontend `index.js` / `websockets.js`** gain ~10 lines for parsing/forwarding/scrubbing the query arg. No new dependencies.
- **`CHANGELOG.md`** gains one line.
- **No config schema changes** (`zellij.kdl`, `Options`, `Config` all untouched).
- **No new HTTP routes** — the existing `/ws/terminal[/{session}]` endpoint absorbs the new optional query arg.

---

## Documentation Plan

- `CHANGELOG.md` — one-line entry as described in U5.
- README and `docs/`-level docs are not updated in this PR. The web client's URL contract is currently un-documented in-tree (the routes are visible from the code only). A follow-up doc update can land in a separate PR if the feature gets external user demand; this plan does not include it.
- Commit message + PR body — describe the two halves (new-session via `CliAssets.cwd`, existing-session via the new IPC variant) and the security-relevant decisions (path validation, shell escape, read-only no-op).

---

## Verification Strategy

- `cargo test -p zellij-client web_client::unit::` — exercises U1's WebSocket URL composition (via the existing tests that load `index.js` content), U2's validator + LogError surface, U3's `CliAssets.cwd` propagation, U4's client-side trigger (gated on `Connected`), U5's E2E flow including the no-reapply-on-SwitchSession regression and the silent-drop vs surface-drop split.
- `cargo test -p zellij-server route` — exercises U4's server-side branch (read-only guard fires before foreground lookup, plugin-pane guard, shell-allowlist guard for each non-allowlisted shell, foreground-cmd guard with `Some(empty)` / `Some(non_empty)` / `None`-then-retry branches, fixed-shape payload bytes).
- `cargo test -p zellij-utils ipc::tests::roundtrip_tests` — exercises the new `ChangeFocusedPaneCwdIfShellMsg` wire-format round-trip including the expected field-21 tag in encoded bytes.
- Regen verification: after editing the `.proto`, run the regen command discovered in `xtask/` (and documented in the PR body); `git diff zellij-utils/assets/prost_ipc/client_server_contract.rs` shows only additive changes — new oneof case, new message struct, new conversion impl — no renumbered fields, no removed messages.
- Cross-version smoke: build the new client against a binary built before the change (or against a `git stash`-removed-variant build); confirm the deep-link cd silently no-ops while the new-session cwd path still works.
- Manual smoke test in a browser: open `http://127.0.0.1:8082/?path=/tmp`, confirm a fresh session lands in `/tmp` (run `pwd` in the first pane). Open `/work?path=/tmp` while session "work" exists, is idle, and is running bash — confirm `cd '/tmp'` lands on the prompt. Open `/work?path=/tmp` while `vim` is running in the focused pane — confirm no text is injected. Repeat with `/work?path=/tmp` while session "work" is running `csh` — confirm no text is injected (allowlist miss). Open `/work?path=relative` and confirm a "deep-link path rejected: must be absolute" message appears in the xterm area. Open `/work?path=%00` and confirm the session attaches normally with no message. Verify the address bar drops `?path=…` after the page settles in all five cases. Verify PWA install (if testing under PWA support) still works under the deep-linked URL — `start_url` should still resolve to `<base_url>/`.
