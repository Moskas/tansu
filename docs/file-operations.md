# File operations

## Copy, cut and paste

`yy` copies and `dd` cuts the selection (or the marked entries) into a buffer, replacing what
was in it; `pp` pastes it
into the current directory. A cut followed by a paste is a move. The buffer survives
directory changes. After a copy it can be pasted again; after a cut it is emptied once the
move succeeds. `uy` or `ud` clears it.

`pp` never overwrites without asking. When names are taken in the destination, nothing is
pasted until each conflict is answered in a popup that shows both entries' sizes and dates:

| Key   | Answer                                                                   |
|-------|--------------------------------------------------------------------------|
| `o`   | Overwrite (a directory is merged into)                                   |
| `s`   | Skip this entry                                                          |
| `n`   | Keep whichever is newer: overwrite if the incoming entry is newer        |
| `r`   | Rename: as in ranger, a suffix `name_`, then `name_0`, `name_1`, ...     |
| `a`   | Toggle applying the next answer to all remaining conflicts               |
| `X`   | Cancel the paste                                                         |
| `Esc` | Decide later: the status bar shows the paste waiting, `w` brings it back |

Copying an entry into its own directory asks too, offering only `s` and `r` (a suffixed
duplicate); moving it there does nothing. `po` overwrites without asking: files are
replaced, and a directory is merged into an existing directory of the same name. A file
never replaces a directory.

Directories are copied recursively. Symlinks are copied as links, and permissions and
modification times are kept. Fifos, sockets and device files are refused. Moves use `rename`
and fall back to copy-and-delete across filesystems. A directory cannot be pasted into
itself or its own subdirectories. The pasted entry is selected when done.

## Tasks

Pastes, deletions and extractions run in the background as tasks, one at a time; later
ones queue. A task still running after a moment opens a popup with its progress: the
current file, files and bytes done, speed and time left. With several tasks the popup lists
them all.

| Key        | In the task popup                                          |
|------------|------------------------------------------------------------|
| `b`, `Esc` | Send to the background                                     |
| `p`        | Pause or resume the selected task                          |
| `x`        | Cancel the selected task                                   |
| `l`        | Show the selected shell task's output in `less`            |
| `j`, `k`   | Select a task (several tasks)                              |

In the background, the status bar shows the running task's progress and how many more
there are; `w` opens the popup again. A finished task that was shown leaves a note in the
bottom right corner for a few seconds (longer for a failure).

Cancelling stops a task within a moment, and entries already done stay. Each file is copied
under a hidden name and renamed into place when complete, so a file being copied or
overwritten is either the old one or the new one, never half of each. A directory that a
copy created is removed when cancelled; a merge into an existing directory stops where it
is. A cancelled extraction leaves nothing behind. Deleting stops between entries.

A paused copy or extraction waits within a few MiB, a deletion before its next entry. An
archive unpacked by an external program is finished first. Quitting tansu stops running
tasks without that cleanup.

### Shell tasks

`:shell -b <command>` runs a shell command as a task. Shell tasks run alongside the file
tasks and each other instead of queueing. Their stdout and stderr go into a log, and the
popup shows the command, the last line of its output (the latest redraw of a progress bar
that rewrites its line with `\r`, without colors) and how long it has been running. `l` opens
the log in `less +F`, which follows new output; `<C-c>` stops following and `q` returns.

`p` stops and continues the command's whole process group with SIGSTOP and SIGCONT. `x`
sends it SIGTERM, then SIGKILL if it is still running 3 seconds later. When it exits, the
directory is reloaded, a failure is reported with the last line of output, and the command
lines it wrote into `$TANSU_CMD` are run (see [Custom commands](commands.md)).

Logs of the last 20 finished shell tasks are kept: `:tasklog` opens the latest in `less`,
`:tasklog 2` the one before, and so on. Logs live in `$XDG_RUNTIME_DIR` and are deleted when
tansu quits. Shell tasks still running then keep running, but their output is lost.

## Deleting and creating

`dD` deletes the marked entries, or the selected one, for good; `dT` moves them to the trash.
Both always ask first: the status bar shows the question and a list above it names every
entry, directories with a trailing `/` (they go with everything in them). `y` confirms; any
other key or a click cancels. The list is fixed when the question is asked, so nothing that
happens before the answer changes what is deleted. Symlinks are removed themselves, never
what they point to. `:delete` and `:trash` ask the same way.

The trash is the freedesktop.org home trash (`$XDG_DATA_HOME/Trash`, usually
`~/.local/share/Trash`), so desktop file managers and `trash-restore` can restore what tansu
trashed. Entries are renamed into it, never copied: one on another filesystem than the trash
is refused, and `dD` has to be used instead.

`md` opens the command line as `mkdir ` and `mf` as `touch `. The name may contain spaces and
`/`, and a leading `~` is the home directory, as in `:cd`; missing parent directories are
created, as in ranger. An existing entry is never
replaced. The new entry (or the directory leading to it) is selected afterwards.

## Bulk rename

`:bulk_rename` (or `:bulkrename`, as in ranger) opens the names of the marked entries, or of
the selected one, one per line, in the `editor` setting, else `$VISUAL`, else `$EDITOR`,
else `vi`. Each changed line renames the entry on the same line; then the status bar asks to
confirm and a list above it shows every `old -> new`. Nothing is renamed when the editor
exits with an error, the number of lines changed, or a new name is empty, contains `/` or is
given twice. Names can be swapped or rotated (`a` -> `b`, `b` -> `a`): those go through
temporary names. An existing entry is never replaced; when a new name is taken, nothing is
renamed. Marks and the cursor follow the renamed entries. It has no default key; bind one in
`[keys]`, e.g. `"cw" = "bulk_rename"`.

## Extracting archives

`x` (or `:extract`) extracts the selected archive, or every marked archive, into the
current directory; marked entries that are not archives are skipped. An archive holding a
single entry (one folder, or one file) puts that entry here as it is. Anything else goes
into a new folder named after the archive (`photos.tar.gz` makes `photos/`). A plain
compressed file (`notes.txt.gz`) is decompressed to `notes.txt`. Nothing is overwritten:
a taken name gets a suffix, as with `pp`. The preview's `Extracts to` row shows the name
beforehand (without that suffix).

Each archive is unpacked into a hidden folder in the current directory first and moved
into place when complete, so a failed extraction leaves nothing behind. Nothing is
written outside the destination: `../` and absolute paths, and paths through a link that
points out, are refused. Setuid and setgid bits are dropped, and zip and tar modification
times are kept.

When files in a zip are damaged (their data fails its checksum, as after a broken
download), nothing is extracted at first: the damaged files are listed above the status
bar, which asks whether to extract the rest. `y` extracts everything else and leaves the
damaged files out, so no corrupt file ends up on disk; any other key aborts.

zip file names are UTF-8, or, from tools on Japanese, Chinese or Korean Windows that
write their code page without saying so, in an encoding detected from the names
(Shift_JIS, GBK, Big5, ...), where `unzip` shows mojibake. The preview names that
encoding (`zip, Shift_JIS names`), and `\` separates folders like `/`.

zip archives using compression other than deflate, and the formats listed by external
programs, are extracted by 7-Zip, unrar or bsdtar. Those never ask for a password: an
encrypted archive fails with the program's message. Extractions run in the background,
one at a time, with pastes and deletions, and the result is selected when done.

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
