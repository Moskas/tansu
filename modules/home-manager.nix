# Home Manager module for tansu: installs the package and writes
# $XDG_CONFIG_HOME/tansu/config.toml from `programs.tansu.settings`.
{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.tansu;
  toml = pkgs.formats.toml { };
in
{
  options.programs.tansu = {
    enable = lib.mkEnableOption "tansu, a terminal file manager with Miller columns";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "tansu.packages.\${system}.default";
      description = ''
        The tansu package to install. The default is wrapped with its preview tools (bat,
        ffmpeg, ffmpegthumbnailer, poppler-utils); use
        `tansu.packages.''${system}.tansu-unwrapped` to rely on the tools on `PATH` instead.
      '';
    };

    settings = lib.mkOption {
      inherit (toml) type;
      default = { };
      example = lib.literalExpression ''
        {
          show_hidden = true;
          sort = "mtime";
          column_ratios = [ 1 3 4 ];
          scroll_offset = 4;
          preview.max_lines = 500;
          highlight.theme = "ansi";
          theme = {
            directory = "#83a598";
            marked = "#fabd2f";
          };
          keys = {
            J = "half_page_down";
            K = "half_page_up";
            "<BS>" = "none";
          };
        }
      '';
      description = ''
        Configuration written to {file}`$XDG_CONFIG_HOME/tansu/config.toml`. Every key is
        optional; see the README for the available keys, theme colors and action names.
        Nothing is written while this is empty, so a hand-written file is left alone.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];

    xdg.configFile."tansu/config.toml" = lib.mkIf (cfg.settings != { }) {
      source = toml.generate "tansu-config.toml" cfg.settings;
    };
  };
}
