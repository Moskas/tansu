# Architecture

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Overview

Data flows one way: input event -> `Action` (via the keymap) -> `App` state update ->
render. Rendering only reads state. The app never performs I/O itself; it queues
`Command`s (list a directory, build a preview, count subdirectory items, check directories
for outside changes, query git, open a file, paste, write the clipboard) that the runtime
executes in the background and answers with events.

| Module        | Purpose                                                            |
|---------------|--------------------------------------------------------------------|
| `main.rs`     | Startup, event loop, chooser output files                          |
| `cli.rs`      | Command line arguments, including the ranger chooser options       |
| `terminal.rs` | Raw mode, alternate screen, drawing on `/dev/tty`, restore on exit |
| `app.rs`      | Application state, navigation, caches, command queue               |
| `action.rs`   | `Action` enum                                                      |
| `keymap.rs`   | Key notation parser, binding table, multi-key sequences            |
| `config.rs`   | Config file, defaults, theme                                       |
| `event.rs`    | Event enum and pausable input thread                               |
| `runtime.rs`  | Executes commands on background threads                            |
| `worker.rs`   | Latest-wins worker with cooperative cancellation                   |
| `jobs.rs`     | Tasks: progress, pause and cancel, the task popup's model          |
| `nav.rs`      | Cursor and scroll offset math                                      |
| `opener.rs`   | Detached and foreground program launching                          |
| `console.rs`  | Command-line editor and command parser                             |
| `search.rs`   | Search patterns: wildcards, fuzzy matching and ranking             |
| `clipboard.rs`| System clipboard through wl-copy, xclip, xsel or OSC 52            |
| `fs/`         | Entries, listing, sort, metadata, file ops, trash (no UI types)    |
| `archive/`    | Archive formats, listing and extraction, built in or via programs  |
| `preview/`    | `Previewer` trait, registry, previewers, ANSI, thumbnail cache     |
| `graphics.rs` | Encodes images for the terminal graphics protocol                  |
| `vcs/`        | Git status via the `git` CLI, per-path and per-directory status    |
| `ui/`         | Layout, columns, header, status bar, help, task popup, icons       |

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
- Custom commands: keys bind to command lines, `[commands]` names lists of them, and shell
  commands get tansu's state in `TANSU_*` variables and send command lines back through the
  `$TANSU_CMD` file (`opener::CommandFile`). There is no embedded scripting language; see
  [Custom commands](commands.md).
