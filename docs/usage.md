# Usage

The layout and conventions follow ranger: sizes use SI units (`48 k`), directory counts
include hidden entries, and git status markers sit at the right edge of each row.

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
| `<C-j>` / `<C-k>`           | Scroll the preview down / up  |
| `~`                         | Go to home directory          |
| `cd`                        | Change directory (`:cd `)     |
| `cz`                        | zoxide jump (`:z `)           |
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
| `pp`                        | Paste, asking about conflicts |
| `po`                        | Paste, overwriting            |
| `dD`                        | Delete permanently (asks)     |
| `dT`                        | Move to the trash (asks)      |
| `md`                        | Create a directory (`:mkdir`) |
| `mf`                        | Create a file (`:touch`)      |
| `x`                         | Extract the archive here      |
| `w`                         | Show the running tasks        |
| `uy`, `ud`                  | Clear the copy/cut buffer     |
| `yp`                        | Yank the selection's path     |
| `yd`                        | Yank the current directory    |
| `yn`                        | Yank the selection's name     |
| `y.`                        | Yank name without extension   |
| `yf`                        | Yank the selection as a file  |
| `!`, `s`                    | Run a shell command           |
| `/`                         | Search (opens `:search `)     |
| `n` / `N`                   | Next / previous search match  |
| `<Esc>`                     | Clear the search              |
| `ms`                        | Mark every search match       |
| `R`                         | Reload directory              |
| `?`                         | Toggle help overlay           |
| `:`                         | Open the command line         |
| `gn`, `<C-n>`               | New tab in this directory     |
| `gc`, `<C-w>`               | Close this tab                |
| `gt`, `<Tab>`               | Next tab                      |
| `gT`, `<S-Tab>`             | Previous tab                  |
| `<A-1>` .. `<A-9>`          | Go to tab 1-9, opening it     |
| `uq`                        | Reopen the last closed tab    |
| `q`                         | Close this tab, or quit       |
| `Q`, `<C-c>`                | Quit                          |

While a key sequence is unfinished, the status bar shows the keys typed so far (`y-`) and a
popup above it lists the keys that can follow and what they do, like which-key in vim and
emacs. Keys leading to longer sequences show as `+N bindings`. Set `which_key = false` to keep
only the status bar prefix.

Directories are always listed before files; the sort order applies within each group. Time
sorts put the newest entries first. Sorting by size orders files only, since a directory's own
size says nothing about its contents; directories stay in name order. Changing the order
keeps the selected entry under the cursor and applies to every column.

The current, parent and previewed directories are checked once a second and listed again
when another program adds, removes or renames entries in them, keeping the cursor and
marks. Changes to the contents of existing files are not noticed; `R` reloads by hand. Set
`auto_refresh = false` to turn the checks off.

Files are opened with the configured opener (default `xdg-open`), detached from the
terminal so tansu stays usable. `[[open]]` rules can open files by name or MIME type instead,
for example text in a terminal editor; see [Open rules](configuration.md#open-rules).

## Tabs

Tabs work like ranger's. Each tab has its own directory, selection and previous directory
(`:cd -`); marks, the copy/cut buffer, the search and settings such as hidden files and the
sort order are shared. Tabs are numbered 1 to 9 and the numbers can have gaps: `<A-5>` goes
to tab 5, opening it on the current directory when it is not open. `gn` opens a tab with the
lowest free number.

With more than one tab open, the header lists them at its right end, taking at most half of
it. Each tab shows its number, its directory's name and its activity: a spinner while a task
started from the tab runs (`‖` when paused, `!` when it waits for an answer) and `+N` for
entries marked in the tab's directory (`4 site ⠋+3`). The current tab is highlighted and also
shows its task's progress (`5 src ⠋  62%`). As space runs out, the other tabs' names are cut
short, then dropped (`4⠋+3`). Then the header shows only the tabs around the current one,
between `‹` and `›`, with the current tab's position (`5/9`). Each arrow carries the activity
of the tabs hidden on its side. Last, the current tab's label drops its percent and its name.
Closing a tab hands its running tasks to the tab shown next.

Closing a tab goes to the next one, or the previous one when it was the last. `q` (and `:q`)
closes the tab and quits only when it is the only one; `Q`, `<C-c>` and `:qa` quit right away.
`uq` reopens closed tabs, the last closed first. `:tab_open N` goes to tab `N`.

## Mouse

Mouse input follows ranger:

- Left click selects an entry in the current column. Clicking an entry in the parent or
  preview column enters that column's directory with the entry selected.
- Right click opens an entry: files with the opener, directories by entering them. Over a
  file's preview it opens that file.
- Double click opens an entry in the current column.
- The wheel moves the cursor three rows. Over the parent column it steps through the
  sibling directories of the current one. Over a file's preview it scrolls the preview
  instead (text and metadata; images do not scroll).
- On the header, a click on a tab goes to that tab, a click on `‹` or `›` to the nearest tab
  hidden on that side, and the wheel steps through tabs.

A click closes the help overlay; the mouse is ignored while the command line is open. Set
`mouse_enabled = false` to leave the mouse to the terminal.

## Marking

As in ranger, `<Space>` marks the selected entry, or unmarks it if it is marked, and moves
the cursor down. `v` toggles the mark of every visible entry in the directory: with nothing
marked it marks everything, and pressed again it unmarks everything. `uv` unmarks every entry
in the directory, whatever was marked before. `ms` marks every entry matching the active
search, keeping the existing marks. Marked entries are drawn
bold in the `marked` theme color, and the status bar shows their count and size.

Marks belong to their directory and are kept when you leave it and come back. Entries that
disappear (renamed, moved or deleted) lose their mark, and hidden entries are never acted on.
While the current directory has marked entries, `yy`, `dd`, `yp`, `yn`, `y.`, `yf` and the
`%s` and `%p` shell macros act on all of them instead of the selection.

Marked entries are copied, moved, deleted or extracted together; see [File operations](file-operations.md).

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

## File chooser

tansu takes ranger's options for picking files from scripts and other programs:

| Option               | Effect                                                              |
|----------------------|---------------------------------------------------------------------|
| `--choosefile=FILE`  | Opening a file writes its path to FILE and quits                    |
| `--choosefiles=FILE` | Opening a file writes the chosen paths, one per line, and quits     |
| `--choosedir=FILE`   | On exit, writes the current directory to FILE                       |
| `--selectfile=PATH`  | Starts with PATH selected, like passing PATH                        |
| `--show-only-dirs`   | Lists directories only, for picking a directory                     |
| `--cmd=COMMAND`      | Runs a command line once the start directory is listed; repeatable  |

Options also take their value as a separate argument (`--choosefile FILE`). Entering a
directory works as usual; only opening a file chooses it, by key, `:open` or double click.
`--choosefiles` writes the marks left in other directories, then the marked entries of the
current directory, or the selected one when none are marked. `--choosefile` writes the path
without a trailing newline, as ranger does.

The chooser files are written only when a file is chosen, so quitting without one leaves
them untouched; `--choosedir` is written on every exit. A FILE that exists but is not
writable is an error before tansu starts.

`--cmd` lines run in order, after the start directory is listed, so they can use the
selection (`--cmd='search READ'`). `--cmd='echo Select a file'` shows a hint in the status
line. Scripts run from tansu see `TANSU_CHOOSER=1` while it is choosing.

### xdg-desktop-portal-termfilechooser

The `ranger-wrapper.sh` that ships with xdg-desktop-portal-termfilechooser works unchanged
with `cmd="tansu"`. It uses `--choosefile`, `--choosefiles`, `--choosedir`,
`--selectfile`, `--show-only-dirs` and `--cmd="echo ..."`. In the directory picker,
quitting chooses the current directory.

A shell function that changes to the directory tansu was left in:

```sh
t() {
  local dir
  dir=$(mktemp) || return
  tansu --choosedir="$dir" "$@" && cd -- "$(cat -- "$dir")"
  rm -f -- "$dir"
}
```

The UI is drawn on `/dev/tty`, not standard output, so the result can also go to a pipe or
a command substitution:

```sh
file=$(tansu --choosefile=/dev/stdout) && "$EDITOR" -- "$file"
tansu --choosefiles=/dev/stdout | xargs -d "\n" ls -l
```

Programs run in the foreground from tansu (`:open_with`, `:shell`) get the terminal as
stdin and stdout when tansu's own are redirected. Without a controlling terminal, tansu
draws on standard output.
