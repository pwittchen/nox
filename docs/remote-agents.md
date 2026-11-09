# Remote agents over SSH — design

Status: **proposal** (not implemented). Once accepted, the behaviour below moves into SPEC.md
(§7.12, §7.13, §11) and the implementation notes into ARCH.md, as for any other feature.

## 1. Problem

Agent cooperation (SPEC §7.13) only works for agents running on the Mac that runs nox. A common
setup is different: the user opens a nox pane, runs `ssh vps`, and starts an agent (Claude Code,
Codex, …) **on the VPS**, because the code, data or hardware live there. That agent can't see
or talk to the agents in the user's local panes, for three reasons:

1. **Transport.** The control socket is a local Unix socket (`~/.nox/run/nox.sock`), and nox
   never listens on a network port (§7.13). The VPS can't reach it.
2. **Identity.** `NOX_SOCKET`, `NOX_TOKEN`, `NOX_PANE_ID`, … live in the local pane's
   environment. `ssh` doesn't pass them on.
3. **Binary.** Only the macOS `nox` CLI ships, in the app bundle. There is no Linux build.

On top of that, the local nox doesn't recognize the pane as an agent pane, because its
foreground process is `ssh` (§4.8). So the pane gets no agent state, no dots and no agents panel
entry, and other agents can't `send` to it.

## 2. Goals

- An agent running on a remote host, inside a nox pane over SSH, takes part in the local
  workspace's team. It can `msg`, `inbox`, `wait`, `status`, `ls`, `claim` and use `nox mcp`,
  the same as a local agent.
- Local agents see it as a normal agent pane: they can address it by name, wait for it, message
  it, and (if allowed) send to it.
- The remote side gets **less** authority than a local pane, by default. A compromised or shared
  VPS must not be able to run commands on the Mac.
- One command to start: `nox ssh vps`.
- Nothing listens on a network port, on either side.

## 3. Non-goals (v1)

- Running the nox **app** on Linux (still "Later" in ROADMAP).
- `mosh` / Eternal Terminal: they can't forward Unix sockets.
- Nested hops (from the VPS, `nox ssh` to a third host).
- Mapping file paths between hosts (e.g. the same repo cloned locally and remotely). See §10.
- Restoring a remote agent session after nox restarts. The pane comes back with `nox ssh vps`
  typed at the prompt but not run, like any other unknown program (§7.12).
- Remote `spawn` (starting a helper pane *on the VPS*). See §10.

## 4. Overview

```
 Mac                                               VPS
 ┌──────────────────────────── nox app ─────┐
 │ control socket   ~/.nox/run/nox.sock     │
 │ session socket   ~/.nox/run/remote/<s>.sock ◄──┐
 └──────────────────────────────────────────┘     │ ssh -R (Unix socket forward)
   pane 7 (workspace "api")                       │
   └─ nox ssh vps ── ssh ─────────────────────────┼──► sshd ── $SHELL -l
                                                  │              └─ claude
                                                  └─ /tmp/nox-<s>.sock ◄── nox (Linux CLI)
                                                        NOX_SOCKET=/tmp/nox-<s>.sock
```

1. In a local pane, `nox ssh vps` asks the app to open a **remote session** for that pane.
2. The app starts a **dedicated session socket** for that session alone, and returns its path
   and a random session id.
3. The CLI runs `ssh`, which forwards the session socket (not the main control socket) to a
   random path on the VPS and starts a login shell there with `NOX_SOCKET` and the ids in its
   environment.
4. On the VPS, the Linux `nox` CLI (and `nox mcp`) connect to that path. The app knows every
   connection on the session socket belongs to pane 7, as a **remote caller**, and applies the
   remote policy.
5. When `ssh` exits, the CLI closes its control connection and the app tears down the session
   socket. Anything still connected on the VPS gets disconnected.

## 5. Design

### 5.1 Session socket instead of forwarding the control socket

Forwarding `nox.sock` itself would be the simplest option (it's what the manual workaround in
§9 does), but it is the wrong boundary:

- The main socket allows `open` and `ls` without a token. Anyone on the VPS who can reach the
  forwarded path could use them.
- The remote side would hold a full pane token. A full pane token can `spawn` (run any
  command on the Mac), `read` any pane in scope and `send` into agents.
- Revoking access means revoking the pane's token, which would also break the local tools in
  that pane.

So each remote session gets its own listener (`nox_ipc::Server`, a second instance), bound to
`~/.nox/run/remote/<session>.sock` (directory `0700`, socket `0600`). Possession of that socket is
the credential:

- Every connection on it resolves to a fixed `Caller` with
  `origin: Origin::Remote { session, host }`. Its `token` field is ignored, so no secret has to
  travel to the VPS or show up in the remote `ps`.
- There is no unauthenticated mode on it. `open` is never allowed.
- Shutting down the listener revokes everything in one step.

On the VPS, `ssh -R` creates the forwarded socket with `StreamLocalBindMask` (default `0177`, so
`0600`), so only the remote user (and root) can connect. The remote path is
`/tmp/nox-<session>.sock`, where `<session>` is 128 random bits. `/tmp` always exists, and the
name is unguessable and never reused, so `StreamLocalBindUnlink` (an `sshd_config` option the
user may not control) isn't needed. The remote shell removes the file on exit (§5.3).

### 5.2 Protocol changes (`nox_ipc`)

- `Caller` gains `origin: Origin`, either `Local` or `Remote { session: u64 }`. The host name
  is kept app-side per session.
- New method **`remote_open { host }`**, allowed only for **local** callers with a valid token
  (so a remote session can't create further sessions). It returns
  `{ session, socket: <local path>, remote_socket: "/tmp/nox-<id>.sock", protocol, version }`.
  The response is a subscription, like `inbox --wait`. The session lives exactly as long as
  this connection stays open. When it closes, for whatever reason (ssh exited, the CLI was
  killed, the pane closed), the app ends the session.
- New method **`hello`**, valid on every socket. It returns the app version and protocol version
  (`v`). The remote CLI calls it first and warns on a protocol mismatch (newer CLIs keep
  speaking `v: 1` as long as the app does).
- A request the remote policy refuses fails with the existing `Forbidden` code, and the message
  names the setting that would allow it.

### 5.3 `nox ssh` (CLI)

```
nox ssh [--install] [ssh options…] <destination> [-- <remote command…>]
```

- Runs in a local pane only (it needs `NOX_TOKEN`). Outside nox, or with cooperation off, it
  prints a notice and just `exec`s `ssh` with the same arguments, so it is safe to alias
  `ssh=nox ssh`.
- Calls `remote_open`, then **spawns** (does not `exec`) `ssh` as a child with the user's
  arguments plus:
  - `-R <remote_socket>:<local socket>`
  - `-o ExitOnForwardFailure=yes`, so a refused forward (`AllowStreamLocalForwarding no`) fails
    loudly instead of silently disabling cooperation; the CLI then offers
    `nox ssh --no-cooperation`, which is plain `ssh`;
  - `-t`, and a remote command that sets the environment and starts the user's login shell
    (or the given remote command):
    ```sh
    NOX_SOCKET=/tmp/nox-<s>.sock NOX_REMOTE=<s> NOX_PANE_ID=… NOX_TAB_ID=… \
    NOX_WORKSPACE_ID=… NOX_WORKSPACE=… ; export NOX_SOCKET NOX_REMOTE …
    trap 'rm -f "$NOX_SOCKET"' EXIT; "${SHELL:-/bin/sh}" -l
    ```
    None of these values is secret (§5.1). `NOX_WORKSPACE_ROOT` is *not* passed, because it is a
    local path.
- While ssh runs, the CLI holds the `remote_open` connection, ignores `SIGINT` / `SIGQUIT` /
  `SIGTSTP` (they reach ssh through the foreground process group), forwards `SIGHUP` /
  `SIGTERM` to ssh, and exits with ssh's exit code.
- `ssh` is still the foreground program the app sees, with the `nox` CLI as its parent. The
  process lookup (§4.6) keeps showing the SSH icon and the host.

### 5.4 Linux CLI and installation

- `nox_cli` and `nox_ipc` are already portable: no macOS APIs, only `libc`, `serde`, `clap` and
  `uuid`. Release builds add static `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`
  binaries, built on the macOS runner with `cargo zigbuild`. They ship:
  - as release assets (`nox-cli-linux-x86_64.tar.gz`, …), and
  - inside the app bundle (`Contents/Resources/bin/linux-<arch>/nox`, about 2 MB each), so
    installing needs no network and always matches the app's version.
- `nox ssh --install <host>` detects the remote arch (`uname -m`) and streams the matching binary
  over the same SSH connection to `~/.local/bin/nox`, with no `scp` / `sftp` needed. It replaces
  an older copy, prints where it went, and warns if `~/.local/bin` isn't on the remote `PATH`.
  The remote shell command (§5.3) also prepends `~/.local/bin` to `PATH` for the session.
- Without `--install`, if `nox` is missing on the VPS, the session still works (the user can
  install it later). The remote shell prints a one-line hint once.
- CI: an `ubuntu-latest` job runs `cargo test -p nox_ipc -p nox_cli`, so portability doesn't
  regress.

### 5.5 Remote agent panes (detection and state)

A pane with an active remote session counts as an **agent pane** (§4.8) as soon as either:

1. an explicit `nox status` report arrives through its session socket (the agent's hooks on the
   VPS), or
2. its title matches a known agent pattern (§7.12, title source 3). Titles and OSC 9 / 777
   already pass through SSH unchanged, so Claude Code's `✳` and spinner titles work without any
   remote setup.

The agent kind (for the icon and default name) comes from the matched title pattern, or from a
new `--agent <kind>` option of `nox status`, which the hooks from Install Agent Hooks pass.
State sources are the same as for local agents (§7.12), except there's no foreground-process
signal.

The agent **exits** when any of these happens:
- `nox status exited` arrives (a new state, accepted only from remote callers; the remote hooks
  send it from `SessionEnd`);
- the title stops matching any agent pattern for 2 s, with no report in between;
- the remote session ends (ssh exited).

Once it's an agent pane, everything else applies unchanged: its name (`nox name` from the VPS
works), dots, the agents panel, `⌘J`, the attention sound, and `send` / `deliver = "type"` from
local agents (the bytes go into the PTY, through ssh, to the remote agent). Keep awake (§7.12)
counts remote agents as well.

**Install Agent Hooks…** can't write files on the VPS. Settings → Agents gets **Copy Remote Hook
Setup**, which copies a shell snippet (the same hook JSON plus `--agent`) to paste on the VPS.

### 5.6 Policy: what a remote caller may do

New setting `agents.remote_allow` (a list of method names). The default is cooperative but not
able to act on the Mac:

| Method                      | Default | Notes                                                       |
|-----------------------------|:-------:|-------------------------------------------------------------|
| `hello`, `ls`, `name`, `status` | ✓   | `ls` hides `cwd` / branch of *local* panes unless `read` is allowed |
| `msg`, `inbox`              | ✓       | the inbox belongs to the pane, so it survives reconnects    |
| `wait`                      | ✓       |                                                             |
| `claim`, `release`, `claims`| ✓       | host-namespaced, see 5.7                                    |
| `notify`                    | ✓       | already possible through OSC 777 anyway                     |
| `read`                      | ✗       | local pane contents may hold secrets                        |
| `send`                      | ✗       | typing into local agents is close to running commands       |
| `spawn`                     | ✗       | runs a command on the Mac                                   |
| `open`, `remote_open`       | never   | not configurable                                            |

The scope rules (`agents.scope`) and the loop guard apply on top, as for local callers.
`agents.remote_cooperation = false` turns `nox ssh` into plain `ssh` (no session socket).

### 5.7 Claims across hosts

A remote claim's paths are absolutized **by the remote CLI** against its own cwd, and stored as
`<host>:<abs path>` (`vps:/srv/api/src/parser/`). The app can't resolve them, since they are
not local paths.

- Claims conflict only with claims on the same host. A local claim on `src/parser/` and a
  remote claim on `vps:/srv/api/src/parser/` never conflict in v1. Path mapping (§10) would add
  that.
- Two remote sessions to the **same** host (same `host` as typed, normalized by
  `ssh -G <destination>` → `hostname` + `user`) share a namespace, so two agents on one VPS do
  coordinate with each other.
- Claims are released when the session ends, in addition to the existing rules (§7.13).
- The pane header lock and the agents panel show them with the host prefix.

### 5.8 UI

- The pane header shows a remote badge (`⇄ vps`, `text-muted`) while a session is active. Hover
  shows host, session start time and the allowed methods.
- The agents panel and `nox ls` list remote agents with `@vps` after the name.
- Activity log entries from remote callers are tagged with the host.
- When a remote caller is refused (`Forbidden`), the Activity log records it once per method
  and session, so the user can see what the remote agent tried to do and which setting would
  allow it.

### 5.9 Lifecycle and failure handling

| Event                                   | Effect                                                    |
|-----------------------------------------|-----------------------------------------------------------|
| ssh exits / CLI killed / pane closed    | session ends: listener closed, remote claims released, agent exited |
| network drop (ssh hangs)                | as above, once ssh notices (`ServerAliveInterval` is the user's choice) |
| `nox ssh vps` again in the same pane    | new session; same pane id, name and inbox, so unread messages are still there |
| tab moves to another workspace          | session follows the pane, as tokens do today              |
| nox app quits                           | sessions end; the remote CLI gets "nox is not running"    |
| `agents.cooperation` turned off         | all sessions end; `nox ssh` falls back to plain ssh       |

## 6. Security

- **No network listener**, on either side. All traffic goes through the user's own SSH
  connection.
- **Least authority by default**: a remote caller can't run anything on the Mac, can't read
  local panes, and can't type into them (§5.6). Each extra permission is an explicit setting.
- **Who on the VPS can use the session**: anyone who can connect to `/tmp/nox-<s>.sock`, which
  means the remote user and root. That's the same trust level as the remote user's shell
  itself. The design doesn't try to defend against root on the VPS beyond the policy above.
- **No secrets on the remote side**: the session id isn't a credential (the socket is), so
  nothing leaks through the remote `ps`, shell history or `/proc/<pid>/environ`.
- **Bounded lifetime**: access disappears with the ssh process. It doesn't outlive the
  connection.
- The loop guard and rate limits apply per pane, so a runaway remote agent is throttled like a
  local one.

## 7. Configuration (SPEC §11 additions)

```toml
[agents]
remote_cooperation = true              # nox ssh opens a session; false = plain ssh
remote_allow = ["ls", "name", "status", "msg", "inbox", "wait",
                "claim", "release", "claims", "notify"]   # add "read", "send", "spawn" to trust remote agents more
```

Both need the `nox_config` model, lenient loader, `settings_schema()` (Settings → Agents) and
`default_config.toml` entries, as usual.

## 8. Implementation plan

1. **Linux CLI.** Musl targets in `release.yml` (zigbuild), binaries in the bundle and release
   assets, an Ubuntu CI job for `nox_ipc` / `nox_cli`, and `hello`.
2. **Sessions.** `Origin` on `Caller`; multiple listeners in `nox_ipc` (one `Server` per session
   sharing the `Handler`); `remote_open`; the policy check in the app's handler;
   `agents.remote_*` config; `nox ssh` without `--install`. At this point the end-to-end flow
   works with a manually copied binary.
3. **Remote agent panes.** Detection (§5.5), `status --agent`, `exited` state, the header badge,
   panel and `ls` tags, Activity log tags, host-namespaced claims, and Copy Remote Hook Setup.
4. **Install.** `nox ssh --install`, the remote `PATH` hint, and protocol mismatch warnings.
5. SPEC / ARCH / ROADMAP updates land with each step.

**Tests**
- `nox_ipc`: a session listener resolves every connection to its fixed remote caller and
  ignores tokens; it closes when the `remote_open` connection drops; `remote_open` is refused
  from remote and untokened callers.
- App handler (pure parts in `nox_agents`): policy table, claim namespacing and normalization,
  agent detection from reports and titles without a foreground agent process.
- End to end without SSH: a test connects straight to the session socket (an `ssh -R` forward
  is transparent) and runs the CLI against it with `NOX_SOCKET` set.
- Optional, local only: `ssh localhost` round trip when `sshd` is enabled (skipped otherwise).

## 9. What works today (manual workaround)

Until this lands, users can approximate it with stock OpenSSH, at the cost of giving the VPS a
full pane token (see §5.1 for why that's too much authority):

```sh
cargo build -p nox_cli --release --target x86_64-unknown-linux-musl
scp target/x86_64-unknown-linux-musl/release/nox vps:~/.local/bin/
ssh -t -R /tmp/nox-$USER.sock:"$NOX_SOCKET" vps \
  "NOX_SOCKET=/tmp/nox-$USER.sock NOX_TOKEN=$NOX_TOKEN NOX_PANE_ID=$NOX_PANE_ID \
   NOX_TAB_ID=$NOX_TAB_ID NOX_WORKSPACE_ID=$NOX_WORKSPACE_ID exec \$SHELL -l"
```

## 10. Later / open questions

- **Path mapping**: `[[agents.remote_paths]] host = "vps", local = "~/src/api", remote =
  "/srv/api"`, so local and remote claims on the same repo conflict, and `NOX_WORKSPACE_ROOT` can
  be passed translated.
- **Remote spawn**: `nox spawn --remote` from a remote agent opens a local pane running
  `nox ssh <same host> -- <cmd>`, so helpers start on the VPS but stay visible locally. It
  needs its own permission, separate from `spawn`.
- **Restore**: relaunch `nox ssh <host> -- <resume command>` on restore when the remote agent
  reported a session id. This needs care with SSH prompts (passphrases, 2FA).
- **Nested hops**: allow `nox ssh` on the VPS to chain sessions (the session socket would
  forward again). Policy composition needs thought.
- **mosh / et**: not possible without an extra relay. Probably never.
- Is `/tmp` acceptable on hosts with a shared, non-sticky `/tmp`? An alternative is
  `$XDG_RUNTIME_DIR` when the remote side has one, but ssh needs the path before the remote
  shell runs, so it would need a probe round trip first.
