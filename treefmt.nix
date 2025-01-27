{ ... }:
{
  projectRootFile = "flake.nix";
  settings.global.excludes = [
    ".envrc"
    "LICENSE-APACHE"
    "LICENSE-MIT"
    "SECURITY.md"
  ];

  programs = {
    nixfmt.enable = true;
    taplo.enable = true;

    rustfmt = {
      enable = true;
      edition = "2024";
    };

    prettier = {
      enable = true;
      settings.proseWrap = "always";
    };
  };
}
