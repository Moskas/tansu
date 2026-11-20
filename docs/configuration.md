# Configuration

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
# Clicks and the mouse wheel. While enabled, the terminal's own text selection usually
# needs Shift held.
mouse_enabled = true

# Keep the command-line history in $XDG_DATA_HOME/tansu/history across sessions.
save_console_history = true

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
# or "reset" for the terminal default. The defaults are the terminal's own colors: its
# default foreground and the 16 palette colors, which a base16 scheme sets. "gray" is
# color 7 (base05), "darkgray" color 8 (base03), and the "light" names are colors 9-14.
[theme]
fg = "reset"
dim = "darkgray"
border = "gray"
header_user = "lightgreen"
header_path = "lightblue"
directory = "lightblue"   # bright blue, as in ranger
file = "reset"
executable = "lightgreen"
symlink = "lightcyan"
broken_link = "lightmagenta"
archive = "lightred"
image = "lightyellow"
video = "lightmagenta"
audio = "lightmagenta"
document = "reset"
code = "reset"
special = "yellow"
marked = "lightyellow"    # marked entries, drawn bold
tab_marks = "lightmagenta" # marked entries on a tab's label in the header
# cursor_fg = "black"     # unset: selected rows use reverse video, like ranger
# cursor_bg = "lightcyan"
status_fg = "reset"
status_perms = "lightcyan"
status_perms_other = "lightmagenta"
error = "lightred"
progress = "lightyellow"   # task progress bars
progress_track = "darkgray" # the empty part of a progress bar
success = "lightgreen"     # finished tasks
warning = "yellow"         # paste conflicts
help_key = "lightyellow"
help_group = "lightred"
vcs_sync = "lightgreen"
vcs_changed = "lightred"
vcs_staged = "lightgreen"
vcs_untracked = "lightcyan"
vcs_conflict = "lightmagenta"
vcs_ignored = "reset"
vcs_ahead = "lightblue"
vcs_behind = "lightred"
vcs_diverged = "lightmagenta"
vcs_info = "lightblue"
vcs_date = "lightcyan"
vcs_commit = "lightyellow"

# Key notation follows vim: plain characters, sequences ("gg"), and <...> for special
# keys and modifiers (<C-d>, <A-x>, <S-Tab>, <Enter>, <Esc>, <Space>, <lt>, <F5>).
# Values are action names or command lines (see commands.md); "none" removes a default
# binding.
[keys]
"J" = "half_page_down"
"K" = "half_page_up"
"<C-e>" = "move_down"
"<C-y>" = "move_up"
"<BS>" = "none"
"ot" = "sort mtime"
"gD" = "cd ~/Downloads"
"ge" = "edit"

# Custom commands: one command line, a list run in order, or a table with `run` and
# `description`. `%a` stands for the arguments typed after the name.
[commands]
edit = { run = "shell $EDITOR %s", description = "Edit the selection" }
```


## Action names

Action names: `move_up`, `move_down`, `move_top`, `move_bottom`, `half_page_up`,
`half_page_down`, `page_up`, `page_down`, `preview_up`, `preview_down`, `go_parent`, `open`,
`go_home`, `prompt_cd`, `prompt_z`, `toggle_hidden`, `toggle_metadata`, `rename`, `shell`,
`search`, `search_next`, `search_prev`, `clear_search`, `mark_matches`, `sort name`,
`sort size`, `sort mtime`, `sort ctime`, `copy`, `cut`, `paste`, `paste_overwrite`, `delete`,
`trash`, `mkdir`, `touch`, `extract`, `clear_buffer`, `yank_path`, `yank_dir`, `yank_name`,
`yank_name_without_ext`, `yank_file`, `reload`, `toggle_help`, `command_line`, `tab_new`,
`tab_close`, `tab_next`, `tab_prev`, `tab_restore`, `tab_open 1` .. `tab_open 9`, `quit`,
`quit_all`.

Keys can also run command lines and custom commands; see [Custom commands](commands.md).
The default bindings are listed in [Usage](usage.md#key-bindings). With Nix, the same
settings can be written by the [Home Manager module](installation.md#home-manager).
