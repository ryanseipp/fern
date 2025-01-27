{
  description = "F.E.R.N. Project - Fast, Efficient, Reliable, Networking software.";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";

    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    advisory-db = {
      url = "github:rustsec/advisory-db";
      flake = false;
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      treefmt-nix,
      crane,
      rust-overlay,
      advisory-db,
      ...
    }:
    let
      # TODO: Add more systems as capabilities increase
      supportedSystems = with flake-utils.lib.system; [
        x86_64-linux
      ];
    in
    flake-utils.lib.eachSystem supportedSystems (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        inherit (pkgs) lib;
        craneLib = (crane.mkLib pkgs).overrideToolchain (
          p:
          p.rust-bin.selectLatestNightlyWith (
            toolchain:
            toolchain.default.override {
              targets = [ "x86_64-unknown-linux-musl" ];
            }
          )
        );

        craneLibLLvmTools = (crane.mkLib pkgs).overrideToolchain (
          p:
          p.rust-bin.selectLatestNightlyWith (
            toolchain:
            toolchain.default.override {
              extensions = [ "llvm-tools" ];
              targets = [ "x86_64-unknown-linux-musl" ];
            }
          )
        );

        treefmtEval = treefmt-nix.lib.evalModule pkgs ./treefmt.nix;

        src = craneLib.cleanCargoSource ./.;

        commonArgs = {
          inherit src;
          strictDeps = true;
          # buildInputs = with pkgs; [ ] ++ lib.optionals stdenv.isDarwin [ libiconv ];
          # nativeBuildInputs = with pkgs; [];
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        crateArgs = commonArgs // {
          inherit cargoArtifacts;
          inherit (craneLib.crateNameFromCargoToml { inherit src; }) version;
          doCheck = false;
        };

        fileSetForCrate =
          crate:
          lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              (craneLib.fileset.commonCargoSources crate)
            ];
          };

        fern-test = craneLib.buildPackage (
          crateArgs
          // {
            pname = "fern-test";
            cargoExtraArgs = "-p fern-test";
            src = fileSetForCrate ./fern-test;

            CARGO_BUILD_TARGET = "x86_64-unknown-linux-musl";
            CARGO_BUILD_RUSTFLAGS = "-C target-feature=+crt-static";
          }
        );
        fern-uring = craneLib.buildPackage (
          crateArgs
          // {
            pname = "fern-uring";
            cargoExtraArgs = "-p fern-uring";
            src = fileSetForCrate ./fern-uring;
          }
        );
      in
      {
        formatter = treefmtEval.config.build.wrapper;

        packages = {
          inherit fern-test fern-uring;

          fern-coverage = craneLibLLvmTools.cargoLlvmCov (commonArgs // { inherit cargoArtifacts; });
        };

        apps = {
          fern-test = flake-utils.lib.mkApp {
            drv = fern-test;
          };
        };

        checks = {
          inherit fern-test fern-uring;

          clippy = craneLib.cargoClippy (
            commonArgs
            // {
              inherit cargoArtifacts;
              cargoClippyExtraArgs = "--all-targets -- --deny warnings";
            }
          );

          nextest = craneLib.cargoNextest (
            commonArgs
            // {
              inherit cargoArtifacts;
              partitions = 1;
              partitionType = "count";
              cargoNextestPartitionsExtraArgs = "--no-tests=pass";
            }
          );

          doc = craneLib.cargoDoc (commonArgs // { inherit cargoArtifacts; });
          audit = craneLib.cargoAudit { inherit src advisory-db; };
          deny = craneLib.cargoDeny { inherit src; };
          formatting = treefmtEval.config.build.check self;
        };

        devShells.default = craneLibLLvmTools.devShell {
          checks = self.checks.${system};

          packages = with pkgs; [
            cargo-deny
            cargo-outdated
            cargo-watch
            cargo-nextest
            cargo-llvm-cov
          ];
        };
      }
    );
}
