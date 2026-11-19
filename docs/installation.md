# Installation

## Build and run

With Nix:

```sh
nix run .            # the wrapped package, see below
nix develop          # or `direnv allow` with the provided .envrc
cargo run -- [path]  # opens at path, defaults to the current directory
```

The flake provides two packages: `tansu` (also `default`), wrapped so the preview tools below
are on its `PATH`, and `tansu-unwrapped`, which uses whatever the system provides. The
wrapper appends to `PATH`, so tools already installed take precedence. The dev shell
includes the tools as well, so the tests that need them run there.

Without Nix, a stable Rust toolchain (edition 2024, Rust 1.90+) is enough:

```sh
cargo build --release
./target/release/tansu ~/Downloads
```

If `path` is a file, tansu opens its directory with that file selected.

Icons use Nerd Font glyphs by default, picked by file name and extension with the same
tables as the ranger_devicons plugin. Set `icons = false` if your terminal font lacks them.

Git status markers need the `git` executable on `PATH`; without it, tansu shows none.

### Optional runtime dependencies

tansu runs these programs when they are on `PATH`. None is required: without one, only the
feature it provides is missing, and previews fall back to plain text or a metadata block.

| Program                    | Used for                                    | Package             |
|----------------------------|---------------------------------------------|---------------------|
| `git`                      | Git status markers                          | `git`               |
| `bat`                      | Syntax highlighting of text previews        | `bat`               |
| `ffprobe`                  | Video and audio details, music tags         | `ffmpeg`            |
| `ffmpegthumbnailer`        | Video frames                                | `ffmpegthumbnailer` |
| `ffmpeg`                   | Video frames, embedded audio covers         | `ffmpeg`            |
| `pdftoppm`, `pdfinfo`      | PDF first pages and details                 | `poppler-utils`     |
| `zoxide`                   | `:z` and recording visited directories      | `zoxide`            |
| `7zz` or `7z`              | 7z, rar, iso and other archives             | `_7zz` or `p7zip`   |
| `unrar`                    | rar archives (preferred for them)           | `unrar` (unfree)    |
| `bsdtar`                   | Archives when 7-Zip is missing              | `libarchive`        |
| `wl-copy`, `xclip`, `xsel` | The `y` clipboard keys (OSC 52 without)     | same names [^clip]  |
| `xdg-open`                 | The default opener                          | `xdg-utils`         |

[^clip]: `wl-copy` comes in the `wl-clipboard` package; `xclip` and `xsel` in their own.

The wrapped Nix package includes `bat`, `ffmpeg`, `ffmpegthumbnailer`, `poppler-utils` and
`libarchive`.

## Home Manager

The flake provides a Home Manager module. It installs tansu and, when `settings` is not
empty, writes `config.toml` from it; every key of the configuration below can be set.

```nix
# flake.nix
inputs.tansu.url = "path:/path/to/tansu"; # or a git URL

# home.nix (with `inputs` passed to Home Manager)
imports = [ inputs.tansu.homeManagerModules.default ];

programs.tansu = {
  enable = true;
  # Without the preview tools:
  # package = inputs.tansu.packages.${pkgs.stdenv.hostPlatform.system}.tansu-unwrapped;
  settings = {
    show_hidden = true;
    sort = "mtime";
    preview.max_lines = 500;
    theme.directory = "#83a598";
    keys = {
      J = "half_page_down";
      K = "half_page_up";
      "<BS>" = "none";
    };
  };
};
```

| Option                    | Default                     | Description                            |
|---------------------------|-----------------------------|----------------------------------------|
| `programs.tansu.enable`   | `false`                     | Install tansu                          |
| `programs.tansu.package`  | the wrapped `tansu` package | Package to install                     |
| `programs.tansu.settings` | `{ }`                       | `config.toml`; not written while empty |

See [Configuration](configuration.md) for every key `settings` accepts.
