# Command line

`:` opens a command line in the status bar, like ranger and vim. `<Enter>` runs it, `<Esc>`
or `<C-c>` cancels, and `<BS>` on an empty line closes it. Editing keys: `<Left>`/`<Right>`,
`<Home>`/`<End>` (also `<C-a>`/`<C-e>`), `<Del>`, `<C-w>` (delete word), `<C-u>`/`<C-k>`
(delete to start / end).

`<Up>` and `<Down>` (also `<C-p>` and `<C-n>`) step through the lines run before, newest first.
Only lines starting with what was typed are visited, so `cd<Up>` recalls the last `cd`;
stepping past the newest line brings back what was typed. A line run again moves to the
end instead of being kept twice. The last 100 lines are kept in
`$XDG_DATA_HOME/tansu/history` across sessions; `save_console_history = false` keeps them
for the session only. Lines run from key bindings and scripts are not recorded.

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
| `search <pattern>`                 | Select the best match in the current directory       |
| `sort <type>`                      | Sort by `name`, `size`, `mtime` or `ctime`           |
| `mkdir <name>`                     | Create a directory, with missing parents             |
| `touch <name>`                     | Create an empty file, with missing parents           |
| `mkdir`, `touch`                   | Open the command line as `mkdir ` or `touch `        |
| `<action>`                         | Any action name below, e.g. `open`, `reload`, `quit` |
| `tab_open <n>`                     | Go to tab `n` (1 to 9), opening it                   |
| `tasklog [n]`                      | Show the log of a finished `shell -b` task in `less` |
| `q`                                | Close this tab, or quit when it is the only one      |
| `qa`, `quitall`                    | Quit                                                 |

The selected path is appended as the last argument. Arguments are split on whitespace; there
is no shell quoting or expansion. A foreground program (`:open_with nvim`, `:open_with less`)
gets the terminal while tansu waits, and the current directory is reloaded when it exits; a
non-zero exit status is reported in the status line. Use `-d` for GUI programs
(`:open_with -d mpv`) so tansu stays usable. `open_with` on a directory passes the directory
instead of entering it.

`rename` takes the rest of the line as the new name, so names may contain spaces. It refuses
to replace an existing entry, and the renamed entry stays selected. `r` opens the command line
as `rename <selected name>`.

`search` matches the names of the visible entries in the current directory. `/` opens the
command line as `search `, like `/` in vim and ranger, and `n` / `N` step through the matches,
wrapping around. The pattern stays active when changing directories, and while it is, entries
in the current column that do not match are drawn dim. `<Esc>` clears the search, and so does
cancelling the `/` prompt; `n`, `N` and `ms` then ask for a new search instead of reusing the
old pattern. `ms` marks every match.

- A pattern with `*` (any run of characters) or `?` (one character) must match the whole
  name: `*.rs`, `file?.txt`, `test_*`. `\` makes the next character literal (`why\?`).
- Any other pattern is fuzzy: its characters must appear in the name in order, not
  necessarily together (`mrs` matches `main.rs`). Matches are ranked as in fzf, preferring
  consecutive characters and the starts of words; `search` selects the best one and `n` goes
  on to the next best.
- Case is ignored unless the pattern contains an uppercase letter (vim's smartcase).

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
| `%D`, `%F` | Next tab's directory and selected path (this tab when it is alone)  |
| `%S`, `%P` | Full paths of the next tab's marked entries, or of its selected one |
| `%%`       | A literal `%`                                                       |

Other `%` sequences are kept as written (`date +%Y`), so write `%%d` for a literal `%d`. With
a single tab, `%S` and `%P` work like `%p`. `%t` (tagged files in ranger) expands to nothing,
and `%f` or `%F` without a selection is an error.

A first word starting with `-` holds flags; an uppercase flag cancels its lowercase one:

| Flag | Effect                                                                        |
|------|-------------------------------------------------------------------------------|
| none | Run on the terminal while tansu waits, then reload the directory              |
| `-w` | Same, then wait for `<Enter>` so the output can be read                       |
| `-p` | Show stdout and stderr in `less`                                              |
| `-s` | Run in the background with output discarded; a non-zero exit is reported      |
| `-b` | Run in the background as a task, with its output kept in a log                |
| `-f` | Run detached with output discarded; the directory is reloaded when it exits   |
| `-t` | Run in a new terminal window (`$TERMCMD`, else `$TERM` or `xterm`)            |
| `-r` | Run through `sudo` on the terminal (`sudo -b` with `-f`)                      |

`-b` shows the command in the task popup, where its output can be followed; see
[Shell tasks](file-operations.md#shell-tasks). It gives way to `-t`, `-p` and `-r`: with `-r`
the command runs on the terminal, since `sudo` may ask for a password.

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

Every action from [Configuration](configuration.md#action-names) can be run as a command.

Custom commands from the `[commands]` table run the same way, and shell commands can send
command lines back to tansu; see [Custom commands](commands.md).
