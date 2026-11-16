# tansu

A terminal file manager with Miller columns, in the spirit of ranger. Written in Rust with
ratatui and crossterm.

```
user@host /home/user/Downloads/m58
┌────────────┬──────────────────────────┬──────────────────────────────────┐
│ Downloads  │ horizon                5 │ [M3-58] MAD MIND MACHINE - X-C~  │
│ Music      │ m58                    2 │ [M3-58] MAD MIND MACHINE - E-L~  │
│ Pictures   │ archive.zip        127 M │                                  │
└────────────┴──────────────────────────┴──────────────────────────────────┘
drwxr-xr-x 1 user users 2 2026-10-27 09:40      5.58G sum, 96.1G free  6/44  Top
```

## Build and run

With Nix:

```sh
nix develop          # or `direnv allow` with the provided .envrc
cargo run -- [path]  # opens at path, defaults to the current directory
```

Without Nix, a stable Rust toolchain (edition 2024, Rust 1.88+) is enough:

```sh
cargo build --release
./target/release/tansu ~/Downloads
```

If `path` is a file, tansu opens its directory with that file selected.

Icons use Nerd Font glyphs by default. Set `icons = false` if your terminal font lacks them.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Key bindings

| Keys                        | Action                        |
|-----------------------------|-------------------------------|
| `j`, `<Down>`               | Move cursor down              |
| `k`, `<Up>`                 | Move cursor up                |
| `h`, `<Left>`, `<BS>`       | Go to parent directory        |
| `l`, `<Right>`, `<Enter>`   | Enter directory / open file   |
| `gg`, `<Home>`              | Jump to first entry           |
| `G`, `<End>`                | Jump to last entry            |
| `<C-d>` / `<C-u>`           | Half page down / up           |
| `<PageDown>` / `<PageUp>`   | Page down / up                |
| `~`                         | Go to home directory          |
| `.`                         | Toggle hidden files           |
| `r`                         | Reload directory              |
| `?`                         | Toggle help overlay           |
| `q`, `<C-c>`                | Quit                          |

Files are opened with the configured opener (default `xdg-open`), detached from the
terminal so tansu stays usable.

## Configuration

tansu reads `$XDG_CONFIG_HOME/tansu/config.toml` (usually `~/.config/tansu/config.toml`).
Every key is optional; a missing file means defaults. Out-of-range values (such as a zero
middle column ratio) are repaired and reported in the status line. A file that fails to
parse (bad syntax, unknown key, invalid color) is ignored entirely, including its key
bindings, and the error is shown in the status line.

```toml
# Show dotfiles on startup.
show_hidden = false

# Nerd Font icons; false switches to ASCII markers.
icons = true

# The selected file path is appended as the last argument.
opener = "xdg-open"

# Width ratios for parent : current : preview. 0 hides the parent or preview column.
column_ratios = [1, 3, 4]

[preview]
max_bytes = 65536     # bytes read from a file for the text preview
max_lines = 300       # lines kept from a text preview
max_entries = 10000   # entries listed for a directory preview

# Colors: "#rrggbb", a name ("red", "lightblue"), a 256-color index ("208"),
# or "reset" for the terminal default. Defaults are a Gruvbox-dark palette.
[theme]
fg = "#ebdbb2"
dim = "#928374"
border = "#d5c4a1"
header_user = "#b8bb26"
header_path = "#ebdbb2"
directory = "#ebdbb2"
file = "#ebdbb2"
executable = "#b8bb26"
symlink = "#8ec07c"
broken_link = "#cc241d"
archive = "#fb4934"
image = "#fabd2f"
video = "#d3869b"
audio = "#d3869b"
document = "#fe8019"
code = "#83a598"
special = "#d79921"
cursor_fg = "#282828"
cursor_bg = "#7c9f88"
status_fg = "#d5c4a1"
status_perms = "#98971a"
error = "#fb4934"
help_key = "#fabd2f"

# Key notation follows vim: plain characters, sequences ("gg"), and <...> for special
# keys and modifiers (<C-d>, <A-x>, <S-Tab>, <Enter>, <Esc>, <Space>, <lt>, <F5>).
# Values are action names; "none" removes a default binding.
[keys]
"J" = "half_page_down"
"K" = "half_page_up"
"<C-n>" = "move_down"
"<C-p>" = "move_up"
"<BS>" = "none"
```

Action names: `move_up`, `move_down`, `move_top`, `move_bottom`, `half_page_up`,
`half_page_down`, `page_up`, `page_down`, `go_parent`, `open`, `go_home`, `toggle_hidden`,
`reload`, `toggle_help`, `quit`.

## Architecture

Data flows one way: input event -> `Action` (via the keymap) -> `App` state update ->
render. Rendering only reads state. The app never performs I/O itself; it queues
`Command`s (list a directory, build a preview, count subdirectory items, open a file) that
the runtime executes in the background and answers with events.

| Module        | Purpose                                                            |
|---------------|--------------------------------------------------------------------|
| `main.rs`     | CLI argument, startup, event loop                                  |
| `terminal.rs` | Raw mode / alternate screen setup, restore on exit and on panic    |
| `app.rs`      | Application state, navigation, caches, command queue               |
| `action.rs`   | `Action` enum                                                      |
| `keymap.rs`   | Key notation parser, binding table, multi-key sequences            |
| `config.rs`   | Config file, defaults, theme                                       |
| `event.rs`    | Event enum and input thread                                        |
| `runtime.rs`  | Executes commands on background threads                            |
| `worker.rs`   | Latest-wins worker with cooperative cancellation                   |
| `nav.rs`      | Cursor and scroll offset math                                      |
| `opener.rs`   | Detached file opener                                               |
| `fs/`         | Entries, listing, natural sort, metadata formatting (no UI types)  |
| `preview/`    | `Previewer` trait, registry, text/directory/metadata previewers    |
| `ui/`         | Layout, columns, header, status bar, help overlay, icons           |

Responsiveness: directory listings run on their own threads; previews and item counts run
on latest-wins workers, so moving the cursor cancels stale work. Text previews read at most
`max_bytes`, directory previews at most `max_entries`.

### Extension points

- Previewers: implement `preview::Previewer` (`can_handle` + `preview`) and register it
  with a priority in `PreviewRegistry`. Returning `Ok(None)` passes the entry on to the
  next previewer.
- Actions: add a variant to `Action` with a name and description; it is then bindable from
  the config and shown in the help overlay.
- A scripting runtime can later hook in at the same seams: produce `Action`s, register
  previewers, and consume `Command`s.
