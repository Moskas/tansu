# tansu

A terminal file manager with Miller columns, in the spirit of ranger. Written in Rust with
ratatui and crossterm.

```
user@host /home/user/Repos/tansu/src
┌─────────┬────────────────────────────────┬──────────────────────────────────────┐
│  scri~  │  src                       16 ?│  fs                                 ✓│
│  seiri=✓│  target                     6 ·│  preview                            +│
│  tansu⌂?│  Cargo.toml             407 B ✓│  app.rs                             +│
│  tui  ⌂?│  SPEC.md               5.37 k ?│  main.rs                            ✓│
└─────────┴────────────────────────────────┴──────────────────────────────────────┘
drwxr-xr-x 1 user users 16 2026-11-16 12:12             67.6k sum, 95.4G free  1/10  All
```

The layout and conventions follow ranger: sizes use SI units (`48 k`), directory counts
include hidden entries, and git status markers sit at the right edge of each row.

## Build and run

With Nix:

```sh
nix develop          # or `direnv allow` with the provided .envrc
cargo run -- [path]  # opens at path, defaults to the current directory
```

Without Nix, a stable Rust toolchain (edition 2024, Rust 1.90+) is enough:

```sh
cargo build --release
./target/release/tansu ~/Downloads
```

If `path` is a file, tansu opens its directory with that file selected.

Icons use Nerd Font glyphs by default, picked by file name and extension with the same
tables as the ranger_devicons plugin. Set `icons = false` if your terminal font lacks them.

Git status markers need the `git` executable on `PATH`; without it, tansu shows none.

## Previews

The right column previews the selected entry: a directory listing, syntax-highlighted
text, an image, or a metadata block for everything else.

- Text is highlighted by piping the first `max_lines` lines through
  [bat](https://github.com/sharkdp/bat) with `--style=plain --theme=ansi`, matching
  ranger's scope.sh. Without bat on `PATH`, previews fall back to plain text.
- Images (PNG, JPEG, GIF, WebP, BMP, TIFF, ICO, QOI, TGA, PNM) are drawn with the graphics
  protocol the terminal supports (kitty, sixel or iTerm2), detected at startup. Other
  terminals get unicode half blocks. Formats that cannot be decoded, such as SVG, fall back
  to the text or metadata preview. Images are scaled once with Lanczos to the column's pixel
  size, never enlarged.
- `i` switches file previews to a metadata block (size, mode, owner, modification time,
  and dimensions for images) and back. The mode stays on while moving; directories keep
  showing their contents.

## Git status markers

Inside a repository, each entry ends with its status. Directories take the most
significant status of their contents (ignored files do not count).

| Mark | Meaning   | Mark | Meaning                     |
|------|-----------|------|-----------------------------|
| `✓`  | clean     | `?`  | untracked                   |
| `+`  | modified  | `*`  | staged                      |
| `-`  | deleted   | `X`  | conflict                    |
| `·`  | ignored   | `!`  | unknown                     |

A directory that is itself a repository root gets a second mark in front, describing its
branch relative to the upstream: `=` in sync, `>` ahead, `<` behind, `Y` diverged, `⌂` no
upstream. Status comes from `git status` in the background and is refreshed on `R`.

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
| `i`                         | Toggle file metadata / preview|
| `on`                        | Sort by name                  |
| `os`                        | Sort by size, largest first   |
| `om`                        | Sort by modification time     |
| `oc`                        | Sort by change time           |
| `<Space>`                   | Mark / unmark, move down      |
| `v`                         | Invert marks in the directory |
| `uv`                        | Unmark all in the directory   |
| `r`                         | Rename the selection          |
| `yy`                        | Copy the marked or selected   |
| `dd`                        | Cut the marked or selected    |
| `pp`                        | Paste, never overwriting      |
| `po`                        | Paste, overwriting            |
| `uy`, `ud`                  | Clear the copy/cut buffer     |
| `yp`                        | Yank the selection's path     |
| `yd`                        | Yank the current directory    |
| `yn`                        | Yank the selection's name     |
| `y.`                        | Yank name without extension   |
| `yf`                        | Yank the selection as a file  |
| `!`, `s`                    | Run a shell command           |
| `R`                         | Reload directory              |
| `?`                         | Toggle help overlay           |
| `:`                         | Open the command line         |
| `q`, `<C-c>`                | Quit                          |

While a key sequence is unfinished, the status bar shows the keys typed so far (`y-`) and a
popup above it lists the keys that can follow and what they do, like which-key in vim and
emacs. Keys leading to longer sequences show as `+N bindings`. Set `which_key = false` to keep
only the status bar prefix.

Directories are always listed before files; the sort order applies within each group. Time
sorts put the newest entries first. Sorting by size orders files only, since a directory's own
size says nothing about its contents; directories stay in name order. Changing the order
keeps the selected entry under the cursor and applies to every column.

Files are opened with the configured opener (default `xdg-open`), detached from the
terminal so tansu stays usable.

## Marking

As in ranger, `<Space>` marks the selected entry, or unmarks it if it is marked, and moves
the cursor down. `v` toggles the mark of every visible entry in the directory: with nothing
marked it marks everything, and pressed again it unmarks everything. `uv` unmarks every entry
in the directory, whatever was marked before. Marked entries are drawn
bold in the `marked` theme color, and the status bar shows their count and size.

Marks belong to their directory and are kept when you leave it and come back. Entries that
disappear (renamed, moved or deleted) lose their mark, and hidden entries are never acted on.
While the current directory has marked entries, `yy`, `dd`, `yp`, `yn`, `y.`, `yf` and the
`%s` and `%p` shell macros act on all of them instead of the selection.

## Copy, cut and paste

`yy` copies and `dd` cuts the selection (or the marked entries) into a buffer, replacing what
was in it; `pp` pastes it
into the current directory. A cut followed by a paste is a move. The buffer survives
directory changes. After a copy it can be pasted again; after a cut it is emptied once the
move succeeds. `uy` or `ud` clears it.

`pp` never overwrites. As in ranger, a name that is taken gets a suffix: `name_`, then
`name_0`, `name_1`, and so on. Copying an entry into its own directory therefore makes a
suffixed duplicate, and moving it there does nothing. `po` overwrites instead: files are
replaced, and a directory is merged into an existing directory of the same name. A file never
replaces a directory.

Directories are copied recursively. Symlinks are copied as links, and permissions and
modification times are kept. Fifos, sockets and device files are refused. Moves use `rename`
and fall back to copy-and-delete across filesystems. A directory cannot be pasted into
itself or its own subdirectories. Pastes run in the background, one at a time, and the
pasted entry is selected when done.

## Clipboard

The `y` keys write to the system clipboard. `yp` yanks the selection's full path, `yd` the
current directory's path, `yn` the selection's name and `y.` that name without its last
extension. `yf` yanks the selected file itself as a `text/uri-list` (`file:///...`), so
pasting into a graphical file manager, browser or chat client pastes the file. With marked
entries, `yp`, `yn` and `y.` yank one line per entry and `yf` yanks all the files.

Clipboard access goes through `wl-copy` (wl-clipboard) when `WAYLAND_DISPLAY` is set, and
otherwise through `xclip` or `xsel` when `DISPLAY` is set. Without any of these, text is sent
to the terminal as an OSC 52 escape sequence. That also works over SSH, in terminals that
support it (kitty, WezTerm, foot, Alacritty, Windows Terminal). Inside tmux, OSC 52 needs
`set -g set-clipboard on`. `yf` needs `wl-copy` or `xclip`, because neither `xsel` nor
OSC 52 can set a content type.

## Command line

`:` opens a command line in the status bar, like ranger and vim. `<Enter>` runs it, `<Esc>`
or `<C-c>` cancels, and `<BS>` on an empty line closes it. Editing keys: `<Left>`/`<Right>`,
`<Home>`/`<End>` (also `<C-a>`/`<C-e>`), `<Del>`, `<C-w>` (delete word), `<C-u>`/`<C-k>`
(delete to start / end).

| Command                            | Effect                                               |
|------------------------------------|------------------------------------------------------|
| `cd [path]`                        | Change directory; no path goes home, `-` goes back   |
| `z [keywords...]`                  | Jump to the best zoxide match for the keywords       |
| `open_with <program> [args...]`    | Run the program on the selection in the foreground   |
| `open_with -d <program> [args...]` | Run the program on the selection, detached           |
| `rename <new name>`                | Rename the selection within its directory            |
| `rename`                           | Open the command line as `rename <selected name>`    |
| `shell [-flags] <command>`         | Run a command line through the shell                 |
| `shell`                            | Open the command line as `shell `                    |
| `sort <type>`                      | Sort by `name`, `size`, `mtime` or `ctime`           |
| `<action>`                         | Any action name below, e.g. `open`, `reload`, `quit` |
| `q`                                | Quit                                                 |

The selected path is appended as the last argument. Arguments are split on whitespace; there
is no shell quoting or expansion. A foreground program (`:open_with nvim`, `:open_with less`)
gets the terminal while tansu waits, and the current directory is reloaded when it exits; a
non-zero exit status is reported in the status line. Use `-d` for GUI programs
(`:open_with -d mpv`) so tansu stays usable. `open_with` on a directory passes the directory
instead of entering it.

`rename` takes the rest of the line as the new name, so names may contain spaces. It refuses
to replace an existing entry, and the renamed entry stays selected. `r` opens the command line
as `rename <selected name>`.

`shell` works like ranger's. The rest of the line is run with `$SHELL -c` (`/bin/sh` when
`$SHELL` is fish or unset) in the current directory, so pipes, quoting and globs work as in a
shell. These macros are expanded first, each value shell-quoted:

| Macro      | Expands to                                                          |
|------------|---------------------------------------------------------------------|
| `%f`       | Name of the selected entry                                          |
| `%s`       | Names of the marked entries, or of the selected one                 |
| `%p`       | Full paths of the marked entries, or of the selected one            |
| `%d`       | The current directory                                               |
| `%c`       | Full paths in the copy/cut buffer (`yy`, `dd`)                      |
| `%D`, `%F` | Next tab's directory and file path; tansu has one tab, so this one  |
| `%%`       | A literal `%`                                                       |

Other `%` sequences are kept as written (`date +%Y`), so write `%%d` for a literal `%d`. `%S`
and `%P` work like `%p`. `%t` (tagged files in ranger) expands to nothing, and `%f` or `%F`
without a selection is an error.

A first word starting with `-` holds flags; an uppercase flag cancels its lowercase one:

| Flag | Effect                                                                        |
|------|-------------------------------------------------------------------------------|
| none | Run on the terminal while tansu waits, then reload the directory              |
| `-w` | Same, then wait for `<Enter>` so the output can be read                       |
| `-p` | Show stdout and stderr in `less`                                              |
| `-s` | Run in the background with output discarded; a non-zero exit is reported      |
| `-f` | Run detached with output discarded; the directory is reloaded when it exits   |
| `-t` | Run in a new terminal window (`$TERMCMD`, else `$TERM` or `xterm`)            |
| `-r` | Run through `sudo` on the terminal (`sudo -b` with `-f`)                      |

Examples: `:shell -w make`, `:shell -p git log -- %f`, `:shell -f chmod +x %s`,
`:shell $SHELL` (a subshell in the current directory). `!` and `s` open the command line as
`shell `.

`cd` takes the rest of the line as the path. It accepts absolute and relative paths and `~`;
`..` is resolved on the typed path, so symlinked directories are kept as typed. A path to a
file opens its directory with the file selected.

`z` jumps through the [zoxide](https://github.com/ajeetdsouza/zoxide) database, like the shell
function: `:z` alone goes home, `:z -` goes back, and an existing directory path works like
`cd`. Anything else is passed to `zoxide query`, which picks the best match other than the
current directory. Every directory entered in tansu is added to the database with
`zoxide add`, so it ranks alongside directories visited in the shell; set `zoxide_add = false`
to only read it.

`<Tab>` completes the word being typed: command names first, then their arguments. `rename`
completes to the selected name, `sort` to the sort types, `cd` to directories at the typed
path (hidden ones once a `.` is typed), and `open_with [-d]` to the programs on `PATH`
(scanned the first time they are completed). `shell [-flags]` completes the program, then the
selected name after a space, and names in the current directory for a partial word. When
several candidates match, pressing `<Tab>` again cycles through them and `<S-Tab>` cycles
backward. Commands that take an argument complete with a trailing space, so the next `<Tab>`
goes on to the argument.

## Configuration

tansu reads `$XDG_CONFIG_HOME/tansu/config.toml` (usually `~/.config/tansu/config.toml`).
Every key is optional; a missing file means defaults. Out-of-range values (such as a zero
middle column ratio) are repaired and reported in the status line. A file that fails to
parse (bad syntax, unknown key, invalid color) is ignored entirely, including its key
bindings, and the error is shown in the status line.

```toml
# Show dotfiles on startup.
show_hidden = false

# Initial sort order: "name" (natural, case-insensitive), "size", "mtime" or "ctime".
# Directories always come first.
sort = "name"

# Nerd Font icons; false switches to ASCII markers.
icons = true

# The selected file path is appended as the last argument.
opener = "xdg-open"

# Width ratios for parent : current : preview. 0 hides the parent or preview column.
column_ratios = [1, 3, 4]

# Git status markers next to entries.
vcs_aware = true

# Add each directory entered to the zoxide database, like zoxide's shell hook.
zoxide_add = true

# Sizes in base 1024 with Ki/Mi units instead of base 1000 with k/M.
binary_size_prefix = false

# Rows kept visible above and below the cursor while scrolling. When the column is
# shorter than twice this, the cursor stays centered (ranger's scroll_offset).
scroll_offset = 8

# Draw image previews with the terminal's graphics protocol.
preview_images = true
# Show the keys that can follow an unfinished key sequence (`g`, `y`, ...).
which_key = true

[preview]
max_bytes = 65536     # bytes read from a file for the text preview
max_lines = 300       # lines kept from a text preview
max_entries = 10000   # entries listed for a directory preview

# Syntax highlighting of text previews.
[highlight]
enabled = true
command = "bat"
theme = "ansi"        # passed as --theme; "" uses bat's own configuration

# Colors: "#rrggbb", a name ("red", "lightblue"), a 256-color index ("208"),
# or "reset" for the terminal default. Defaults are a Gruvbox-dark palette.
[theme]
fg = "#ebdbb2"
dim = "#928374"
border = "#d5c4a1"
header_user = "#b8bb26"
header_path = "lightblue"
directory = "lightblue"   # terminal palette bright blue, as in ranger
file = "#ebdbb2"
executable = "#b8bb26"
symlink = "#8ec07c"
broken_link = "#d3869b"
archive = "#fb4934"
image = "#fabd2f"
video = "#d3869b"
audio = "#d3869b"
document = "#ebdbb2"
code = "#ebdbb2"
special = "#d79921"
marked = "#fabd2f"        # marked entries, drawn bold
# cursor_fg = "#282828"   # unset: selected rows use reverse video, like ranger
# cursor_bg = "#7c9f88"
status_fg = "#d5c4a1"
status_perms = "#8ec07c"
status_perms_other = "#d3869b"
error = "#fb4934"
help_key = "#fabd2f"
help_group = "#fb4934"
vcs_sync = "#b8bb26"
vcs_changed = "#fb4934"
vcs_staged = "#b8bb26"
vcs_untracked = "#8ec07c"
vcs_conflict = "#d3869b"
vcs_ignored = "#ebdbb2"
vcs_ahead = "#83a598"
vcs_behind = "#fb4934"
vcs_diverged = "#d3869b"
vcs_info = "#83a598"
vcs_date = "#8ec07c"
vcs_commit = "#fabd2f"

# Key notation follows vim: plain characters, sequences ("gg"), and <...> for special
# keys and modifiers (<C-d>, <A-x>, <S-Tab>, <Enter>, <Esc>, <Space>, <lt>, <F5>).
# Values are action names; "none" removes a default binding.
[keys]
"J" = "half_page_down"
"K" = "half_page_up"
"<C-n>" = "move_down"
"<C-p>" = "move_up"
"<BS>" = "none"
"ot" = "sort mtime"
```

Action names: `move_up`, `move_down`, `move_top`, `move_bottom`, `half_page_up`,
`half_page_down`, `page_up`, `page_down`, `go_parent`, `open`, `go_home`, `toggle_hidden`,
`toggle_metadata`, `rename`, `shell`, `sort name`, `sort size`, `sort mtime`, `sort ctime`,
`copy`, `cut`, `paste`, `paste_overwrite`, `clear_buffer`, `yank_path`, `yank_dir`, `yank_name`,
`yank_name_without_ext`, `yank_file`, `reload`, `toggle_help`, `command_line`, `quit`.

## Architecture

Data flows one way: input event -> `Action` (via the keymap) -> `App` state update ->
render. Rendering only reads state. The app never performs I/O itself; it queues
`Command`s (list a directory, build a preview, count subdirectory items, query git, open a
file, paste, write the clipboard) that the runtime executes in the background and answers
with events.

| Module        | Purpose                                                            |
|---------------|--------------------------------------------------------------------|
| `main.rs`     | CLI argument, startup, event loop                                  |
| `terminal.rs` | Raw mode / alternate screen setup, restore on exit and on panic    |
| `app.rs`      | Application state, navigation, caches, command queue               |
| `action.rs`   | `Action` enum                                                      |
| `keymap.rs`   | Key notation parser, binding table, multi-key sequences            |
| `config.rs`   | Config file, defaults, theme                                       |
| `event.rs`    | Event enum and pausable input thread                               |
| `runtime.rs`  | Executes commands on background threads                            |
| `worker.rs`   | Latest-wins worker with cooperative cancellation                   |
| `nav.rs`      | Cursor and scroll offset math                                      |
| `opener.rs`   | Detached and foreground program launching                          |
| `console.rs`  | Command-line editor and command parser                             |
| `clipboard.rs`| System clipboard through wl-copy, xclip, xsel or OSC 52            |
| `fs/`         | Entries, listing, natural sort, metadata, paste ops (no UI types)  |
| `preview/`    | `Previewer` trait, registry, built-in previewers, ANSI parsing     |
| `graphics.rs` | Encodes images for the terminal graphics protocol                  |
| `vcs/`        | Git status via the `git` CLI, per-path and per-directory status    |
| `ui/`         | Layout, columns, header, status bar, help overlay, icons           |

Responsiveness: directory listings run on their own threads; previews and item counts run
on latest-wins workers, so moving the cursor cancels stale work. Git status runs on its own
ordered worker with a short per-repository cache, so the three columns of one repository
cost a single `git status`. Text previews read at most `max_bytes`, directory previews at
most `max_entries`.

### Extension points

- Previewers: implement `preview::Previewer` (`can_handle` + `preview`) and register it
  with a priority in `PreviewRegistry`. Returning `Ok(None)` passes the entry on to the
  next previewer.
- Actions: add a variant to `Action` with a name and description; it is then bindable from
  the config and shown in the help overlay.
- A scripting runtime can later hook in at the same seams: produce `Action`s, register
  previewers, and consume `Command`s.
