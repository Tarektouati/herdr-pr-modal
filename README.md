# herdr-pr-modal

Open any pull request in its own worktree, straight from a [Herdr](https://herdr.dev) popup.

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust 2024](https://img.shields.io/badge/rust-2024_edition-orange.svg)](https://www.rust-lang.org)
[![Herdr ≥ 0.9.0](https://img.shields.io/badge/herdr-%E2%89%A5%200.9.0-8a5cf6.svg)](https://herdr.dev)
![Platforms](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey.svg)

Press a key in any pane inside a git repo: a popup lists the repo's open pull
requests (GitHub) or merge requests (GitLab), styled like Herdr's own keybinds
help. Press Enter on one and it is checked out in its own git worktree and
opened as a Herdr workspace. If that PR already has a worktree, you are
switched to it instead; nothing is created twice.

```
┌────────────────────────────────────────────────────────────────────────┐
│open PRs · acme/app                                           esc close │
│ / press / to filter by number, title, branch or author                 │
│                                                                        │
│ mine                                                                   │
│ #412 ● Add OAuth login  you · feat/oauth [CI ✓]                        │
│                                                                        │
│ review requested                                                       │
│ #409   Fix race in queue  sam · fix/queue-race [draft] [CI …]          │
│                                                                        │
│ others                                                                 │
│ #398   Bump deps  renovate · renovate/all [CI ✗]                       │
│                                                                        │
│ search / · move j/k/↑↓ · open enter · refresh r · close esc            │
└────────────────────────────────────────────────────────────────────────┘
```

`●` = a local worktree already exists for that PR.

## Features

- **GitHub and GitLab**, including GitHub Enterprise and self-hosted GitLab.
- **Grouped list**: your PRs, PRs awaiting your review, then everything else.
- **At a glance**: CI status, draft marker, author and branch; live filter with `/`.
- **One worktree per PR**: same-repo and fork PRs alike, reused on every later open.
- **Safe**: never runs a destructive git command, never touches an existing worktree.
- **No tokens**: all API calls go through `gh` / `glab`, which own authentication.
- **Native look**: colors follow your Herdr theme.

## Requirements

- Herdr ≥ 0.9.0, macOS or Linux.
- `git` and `curl`.
- GitHub: [`gh`](https://cli.github.com), logged in for the repo's host
  (`gh auth login --hostname <host>`).
- GitLab: [`glab`](https://gitlab.com/gitlab-org/cli), logged in for the
  repo's host (`glab auth login --hostname <host>`).

You only need the CLI for the forge you use.

No Rust toolchain is needed: install downloads a prebuilt binary for macOS
(arm64, x86_64) or Linux (arm64, x86_64). On other platforms, or when the
download fails, it builds from source if `cargo` is installed.

## Install

```sh
herdr plugin install tarektouati/herdr-pr-modal
```

From source:

```sh
git clone https://github.com/tarektouati/herdr-pr-modal
cd herdr-pr-modal
HERDR_PR_MODAL_FROM_SOURCE=1 bash herdr/install.sh   # cargo build → bin/herdr-pr-modal
herdr plugin link "$PWD"
```

Uninstall:

```sh
herdr plugin uninstall tarektouati.pr-modal
```

## Keybinding

Herdr plugin manifests cannot declare keys. Print the snippet:

```sh
"$(herdr plugin list --json | jq -r '.result.plugins[] | select(.plugin_id=="tarektouati.pr-modal") | .plugin_root')/bin/herdr-pr-modal" setup
```

which prints (use `setup --key <key>` for another key):

```toml
# Add to ~/.config/herdr/config.toml, then run: herdr server reload-config
[[keys.command]]
key = "prefix+alt+p"
type = "plugin_action"
command = "tarektouati.pr-modal.open"
description = "open PRs"
```

Paste it into `~/.config/herdr/config.toml` and run `herdr server reload-config`.
`setup` only prints; it never edits your config.

## Keys

| key | action |
|---|---|
| `j` / `k`, `↓` / `↑` | move |
| `pgdn` / `pgup` | move a page |
| `g` / `G` | first / last |
| `/` | filter live by number (`12`, `#12`), title, branch or author |
| `enter` | open the PR's worktree workspace |
| `r` | refresh (bypasses the cache) |
| `esc` | clear the filter first, then close |

## What Enter does

1. If a worktree already has the PR's branch checked out (`git worktree list`),
   Herdr focuses its workspace (`herdr worktree open --path … --focus`),
   opening it first if it is closed. Nothing is fetched or changed.
2. Otherwise the PR head is fetched from the remote:
   - same-repo PR: `origin/<branch>` → local `<branch>` with upstream tracking;
   - fork PR: `pull/<n>/head` (GitHub) or `merge-requests/<n>/head` (GitLab) →
     local `pr/<n>-<branch-slug>`.

   A missing local branch is created. A local branch that is behind the PR
   head is fast-forwarded. A local branch with commits the PR lacks is left
   exactly as it is.
3. `herdr worktree create --branch <branch> --focus` creates the checkout
   under Herdr's `[worktrees] directory` (default `~/.herdr/worktrees/<repo>/<branch-slug>`)
   and opens it as a workspace grouped under the repo's workspace.

The plugin never runs a destructive git command, and never modifies or
removes an existing worktree. If the fetch or the checkout fails, the git
error is shown in the modal and nothing else happens.

The target repo is the working directory of the pane the key was pressed in.

## Config

Optional, at `$(herdr plugin config-dir tarektouati.pr-modal)/config.toml`:

```toml
width = "70%"             # popup outer size: cells (100) or percent ("70%")
height = "70%"
cache_ttl_secs = 60       # reuse a repo's PR list for this long; r refreshes
provider = "auto"         # "github" | "gitlab" to override host detection
remote = "origin"         # remote PRs target and heads are fetched from
# worktree_dir = "~/src/worktrees"     # default: Herdr's [worktrees] directory
# post_create_command = "pnpm install" # typed into new workspaces; off by default
```

The provider is detected from the remote's host name (`github` / `gitlab` in
it). Self-hosted instances with other host names need `provider` set.

The popup's colors come from your Herdr theme (`[theme] name`,
`[theme.custom]`, and the legacy `[ui] accent`), resolved from
`~/.config/herdr/config.toml` (or `$HERDR_CONFIG_PATH`). With
`theme.auto_switch`, the popup uses `theme.name`, because plugins cannot see
the terminal's light/dark state.

## Troubleshooting

- **"gh is not installed" / "not logged in"**: the modal shows the exact command to
  fix it (install the CLI, or `gh auth login` / `glab auth login` for that host).
- **Wrong or no forge detected on a self-hosted instance**: set
  `provider = "github"` or `"gitlab"` in the config.
- **Install fails with "download failed" / "cargo is not installed"**: the
  prebuilt binary could not be fetched (offline, proxy, unsupported
  platform). Install Rust with [rustup](https://rustup.rs) so the install can
  build from source, then reinstall.
- **Nothing happens on the key**: check the binding was added and the config
  reloaded (`herdr server reload-config`), then look at
  `herdr plugin log list --plugin tarektouati.pr-modal`.

## Contributing

Issues and pull requests are welcome. For bugs or ideas, please
[open an issue](https://github.com/tarektouati/herdr-pr-modal/issues).

Before opening a PR, make sure these pass:

```sh
cargo test                                  # unit + fixture + real-git tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

After changing code, rerun `HERDR_PR_MODAL_FROM_SOURCE=1 bash herdr/install.sh`
so the linked plugin picks up the new binary.

`tests/fixtures/` holds recorded `gh` / GitLab JSON. `tests/manifest.rs` checks
that the ids and commands in `herdr-plugin.toml` match the code.

## Releasing

1. Bump `version` in both `Cargo.toml` and `herdr-plugin.toml` (they must match).
2. Commit, then tag with the bare version and push:
   `git tag 0.1.1 && git push origin main 0.1.1`.

The `release` workflow builds the four binaries, attaches them with checksums
to a draft release, and publishes it once all are attached. `herdr/install.sh`
downloads from the release named after the manifest version.

## License

MIT. See [LICENSE](LICENSE).

## Acknowledgements

- The built-in theme palettes in `src/theme/builtin.rs` are ported from
  [Herdr](https://github.com/herdrdev/herdr) (Apache-2.0).
- The plugin layout was informed by
  [herdr-plugin-loopreview](https://github.com/loopkeep/herdr-plugin-loopreview),
  [herdr-quick-actions](https://github.com/enekos/herdr-quick-actions) and
  [herdr-plugin-picker](https://github.com/purehate/herdr-plugin-picker).
