# moterm

**English** | [日本語](README.md)

An SSH-only terminal client written in Rust.
Built for connecting to several machines over SSH to develop on them, moving files
to and from your local machine, and driving Claude Code / Codex on the remote side.

| Connected | SFTP |
|---|---|
| ![neo](docs/images/neo.png) | ![neo-sftp](docs/images/neo-sftp.png) |

## Features

- **Auth & connection**: SSH agent / public key (OpenSSH, PEM, rsa-sha2, automatic
  conversion of unencrypted `.ppk`) / password, TOFU host-key verification (shares
  `~/.ssh/known_hosts`), a master-password encrypted vault (Argon2id + XChaCha20),
  **bastion `proxy_jump`** (pure russh, no external ssh) / `proxy_command`, keepalive,
  **auto-reconnect** (backoff retry on unexpected drops; `reconnect = { enabled, max_retries, backoff_sec }`)
  / F5 manual reconnect. Password / passphrase / master-password prompts are neon modals
  (show toggle, click to submit).
- **SSH essentials**: static port forwarding -L/-R (F2 shows a neon status card at the
  top-right of the terminal), `on_connect` auto-input, session logging (raw bytes + plain text).
- **SFTP (F3)**: two-pane file manager (click / drag-and-drop transfer, progress bar,
  mirror sync `m`, mkdir/rename/delete, column sort, date column), drag-and-drop upload
  onto the window. **Embedded in the connect screen** (terminal⇄SFTP sub-tab switch,
  toolbar, Transfer Queue; auto-closes on disconnect).
- **Terminal**: full SGR attributes, 256-color / truecolor, CJK full-width, alternate
  screen, OSC 7/8/52, OSC 133 command boundaries, bracketed paste, mouse reporting
  (vim/htop etc., suppress with Shift), rows/cols follow window resize dynamically.
- **Tabs / panes**: variable-width tabs (drag to reorder, double-click to rename,
  right-click menu), binary-tree pane splitting, broadcast input (red band).
- **Scrollback & copy/paste**: wheel / scrollbar (thumb drag) / Shift+PageUp/Dn,
  Ctrl+Shift+F search (neon search bar with match count at the top center), drag /
  double / triple selection, Alt rectangular select, Ctrl+click to open URLs.
- **Fonts**: bundled **HackGen Console** by default (monospace + Japanese, SIL OFL).
  Works even when not installed on the OS thanks to binary embedding, and the default
  setup skips system-font scanning so **startup is fast**. Pick another font with
  `config.font` (Japanese still falls back to HackGen).
- **Appearance & misc**: window transparency / blur (DWM on Windows), 4 built-in color
  schemes, i18n (ja/en), IME, HiDPI, `config.keys` keybinding overrides, Ctrl+Shift+R
  config reload.

## Requirements

- Linux (X11/Wayland) / Windows 10+ / macOS. Rendering is CPU-based
  (winit + softbuffer + fontdue); no GPU required.
- Window transparency / blur takes effect only when a compositor (DWM on Windows) is active.
- SSH only, UTF-8 only (no telnet / serial / Shift_JIS).

## Build

```sh
cargo build --release -p mot-gui        # binary: target/release/moterm
```

- **Windows**: `scripts\windows-build.bat` (or `.ps1`). Requires only Rust (MSVC) + VS Build Tools (C++).
- **macOS**: `scripts/macos-bundle.sh` builds a Dock-icon `.app`. Replace the icon at
  `assets/icon.png`.
- On a host without a Rust toolchain (like this dev host), build via Docker:
  `docker build -t moterm-dev -f docker/dev.Dockerfile docker`, then
  `./docker/run.sh cargo build -p mot-gui`.

## Run

```sh
moterm                    # auto-discovers config (falls back to defaults if none)
moterm path/to/moterm.lua # specify a config file explicitly
```

Search order for `moterm.lua`: (1) the path given as an argument → (2) the directory of
the executable → (3) `~/.config/moterm/` (`%APPDATA%\moterm\` on Windows). `profiles.json`
and the encrypted vault live in the same place.

Minimal config (see `examples/moterm.demo.lua` for more):

```lua
local moterm = require "moterm"
local config = moterm.config()
config.profiles = {
  { name = "web1", group = "prod", host = "203.0.113.10", user = "admin",
    auth = { method = "publickey", key = "~/.ssh/id_ed25519" } },
  { name = "db1",  group = "prod", host = "10.0.0.5", user = "admin",
    proxy_jump = "admin@203.0.113.10",           -- via bastion
    auth = { method = "agent" } },
}
config.groups = { { name = "prod", label = "Production" } }
return config
```

### Key bindings (F1–F5, tab switching, copy/paste — 14 actions remappable via `config.keys`)

| Key | Action | Key | Action |
|---|---|---|---|
| F1 / Ctrl+T | Sidebar filter | F2 | Port-forward panel |
| F3 | SFTP file manager | F4 | Close tab |
| F5 | Reconnect | Ctrl+PageUp/Dn | Switch tab |
| Ctrl+Shift+C / V | Copy / paste | Ctrl+Shift+F | Scrollback search |
| Ctrl+Shift+D | SFTP-download selected path | Ctrl+Shift+R | Reload config |
| Shift+PageUp/Dn | Scrollback | Ctrl+Shift+B | Broadcast input |
| Ctrl+Shift+E / O | Split pane (horizontal / vertical) | Ctrl+Shift+X | Close pane |
| Ctrl+Shift+Arrows | Move pane focus | Ctrl+1–9 / Ctrl+Tab | Switch tab |
| Ctrl+Shift+P / N | Prev / next prompt (OSC 133) | | |


## Development & testing

The single verification entry point is `./check` (lint + typecheck + all tests; auto-detects the Docker environment):

```sh
./check                      # lint + typecheck + all tests (completion gate)
./check fix                  # auto-format
./docker/e2e.sh              # real-sshd integration test (pubkey / password+PF / SFTP / proxy_jump)
./docker/gui-e2e.sh          # GUI → real sshd end-to-end (Xvfb + xdotool, screenshot)
./docker/reconnect-e2e.sh    # auto-reconnect (drop → sshd restart → recover)
./docker/neo-sftp-e2e.sh     # SFTP rendering embedded in the connect screen
```


## License

MIT ([LICENSE](LICENSE))
