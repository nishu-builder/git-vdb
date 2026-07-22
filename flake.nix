{
  description = "git-vdb — a Git-native vector database";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    crane.url = "github:ipetkov/crane";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      crane,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };

        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;
        src = pkgs.lib.cleanSourceWith {
          src = ./.;
          filter =
            path: type:
            let
              relativePath = pkgs.lib.removePrefix (toString ./.) (toString path);
            in
            craneLib.filterCargoSources path type
            || builtins.elem relativePath [
              "/docs"
              "/docs/format.md"
              "/docs/snapshots.md"
            ];
        };

        commonArgs = {
          inherit src;
          strictDeps = true;
          pname = "git-vdb";
          version = "0.1.1";
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        gitVdb = craneLib.buildPackage (
          commonArgs
          // {
            inherit cargoArtifacts;
            doCheck = false;
          }
        );
      in
      {
        packages = {
          default = gitVdb;
          git-vdb = gitVdb;
        };

        apps.default = flake-utils.lib.mkApp { drv = gitVdb; };

        checks = {
          package = gitVdb;

          fmt = craneLib.cargoFmt { inherit src; };

          clippy = craneLib.cargoClippy (
            commonArgs
            // {
              inherit cargoArtifacts;
              cargoClippyExtraArgs = "--all-targets --all-features -- --deny warnings";
            }
          );

          test = craneLib.cargoTest (
            commonArgs
            // {
              inherit cargoArtifacts;
              cargoTestExtraArgs = "--all-targets --all-features";
              nativeBuildInputs = [ pkgs.git ];
            }
          );

          doc = craneLib.cargoDoc (
            commonArgs
            // {
              inherit cargoArtifacts;
              RUSTDOCFLAGS = "-D warnings";
              cargoDocExtraArgs = "--all-features --no-deps";
            }
          );
        };

        devShells.default = craneLib.devShell {
          checks = self.checks.${system};
          packages = [
            pkgs.curl
            pkgs.git
            pkgs.rust-analyzer
            pkgs.uv
          ];
        };
      }
    );
}
