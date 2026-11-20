# Custom commands

tansu doesn't embed a scripting language. Custom commands, the equivalent of ranger's
`commands.py`, are built from three pieces:

- key bindings to any [command line](command-line.md), not just to action names
- a `[commands]` table that names a list of command lines
- shell commands that read tansu's state from environment variables and send command lines
  back through `$TANSU_CMD`

Scripts can be written in any language: shell, Python, or anything else on `PATH`.

For a script that takes a while, use `shell -b`: it runs as a task whose output can be
followed, paused and cancelled from the task popup (see
[Shell tasks](file-operations.md#shell-tasks)). `shell -s` suits quick scripts whose output
doesn't matter.

## Binding keys to command lines

A `[keys]` value that isn't an action name is a command line. It runs as if typed after `:`.
Its first word must be a built-in command or a custom one; anything else is reported as a
config warning and left unbound.

```toml
[keys]
"ge" = "shell -w $EDITOR %s"
"gD" = "cd ~/Downloads"
"gl" = "shell -p git log --oneline -- %f"
"cw" = "bulk_rename"
```

The help overlay (`?`) lists these bindings first, followed by the built-in actions. The help
and which-key popups show the command line, or the custom command's description when it has
one.

## The `[commands]` table

Each entry names a custom command. Its value takes one of three forms: one command line, a
list of lines run in order, or a table with `run` and an optional `description`.

```toml
[commands]
edit = "shell -w $EDITOR %s"
up_and_reload = ["cd ..", "reload"]
proj = { run = "cd ~/Projects/%a", description = "Go to a project" }
```

A custom command runs from the command line (`:proj tansu`) or from a key binding
(`"gp" = "proj tansu"`). `<Tab>` completes custom command names along with the built-in ones.

- `%a` (or `%{a}`) is replaced by the arguments typed after the command's name, exactly as
  typed. Other `%` macros are expanded later by `shell`, as described in
  [Command line](command-line.md).
- The lines run in order. A line that fails right away, such as a `cd` to a missing
  directory or a shell macro with nothing selected, stops the rest. A shell command's exit
  status is only known later, so it never stops the sequence.
- A line that opens the command line for input, such as a bare `rename` or `shell`, does
  not wait for it: the next line runs right away.
- A custom command can run another one, up to 8 levels deep. A command that runs itself
  stops at that limit with an error.
- A custom command can't use a built-in command's name (`echo` included), and its name
  can't contain whitespace. Entries that break either rule are skipped with a warning.

## Scripts

Every `shell` command gets these environment variables:

| Variable          | Value                                                                      |
|-------------------|----------------------------------------------------------------------------|
| `TANSU_DIR`       | The current directory                                                      |
| `TANSU_SELECTED`  | Full path of the selected entry (unset when nothing is selected)           |
| `TANSU_SELECTION` | Full paths of the marked entries, or of the selected one, one per line     |
| `TANSU_CMD`       | A file for command lines to run once the command exits                     |
| `TANSU_LEVEL`     | How deeply tansu is nested (1 inside the first tansu), like `RANGER_LEVEL` |
| `TANSU_CHOOSER`   | `1` while tansu runs as a file chooser (`--choosefile`, `--choosefiles`)   |

Each line written into `$TANSU_CMD` is run as a tansu command line after the shell command
exits, in order. Blank lines are skipped. Up to 64 KiB is read, and the file is deleted
afterwards. The lines run even when the command exits with a non-zero status.

When the lines are read depends on the flag. With no flag, `-w`, `-s` and `-b`, they're read
when the command exits. With `-p`, they're read when `less` is closed. With `-f` and `-t`, they're read
when the detached process exits. A terminal emulator that hands the window to an already
running instance and returns at once (some do with `-t`) exits before the script writes
anything.

With `-r`, `sudo` resets the environment by default, so the elevated command does not see
the `TANSU_*` variables. A process that writes into `$TANSU_CMD` after tansu has read it
(output still going after `less` is closed, or a writer left in the background) recreates the
file; those lines are not run and the file is left behind in `$XDG_RUNTIME_DIR`.

A file path given to `cd` selects that file in its directory, so `cd` is enough to point tansu
at a result.

### Example: jump to a file with fzf

`~/.config/tansu/fzf-jump`:

```sh
#!/bin/sh
f=$(fd . "$TANSU_DIR" | fzf) || exit 0
printf 'cd %s\n' "$f" >> "$TANSU_CMD"
```

```toml
[commands]
fzf = { run = "shell ~/.config/tansu/fzf-jump", description = "Jump to a file with fzf" }

[keys]
"<C-f>" = "fzf"
```

### Example: bulk rename in `$EDITOR`

`~/.config/tansu/bulk-rename`:

```sh
#!/bin/sh
# Edit the selected names; each changed line renames the file on the same line.
old=$(mktemp) new=$(mktemp)
printf '%s\n' "$TANSU_SELECTION" | xargs -d '\n' -n1 basename > "$old"
cp "$old" "$new"
"${EDITOR:-vi}" "$new"
paste -d '\n' "$old" "$new" | while IFS= read -r a && IFS= read -r b; do
  [ "$a" = "$b" ] || mv -n -- "$TANSU_DIR/$a" "$TANSU_DIR/$b"
done
rm -f "$old" "$new"
```

```toml
[commands]
bulk_rename = "shell ~/.config/tansu/bulk-rename"

[keys]
"cw" = "bulk_rename"
```

With Nix, the scripts can be written from the Home Manager configuration with
`xdg.configFile`, and the commands go in `programs.tansu.settings.commands`.
