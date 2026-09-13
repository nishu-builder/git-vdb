{
  description = "Optional git-vdb semantic-search worker for Caos";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, crane, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; overlays = [ (import rust-overlay) ]; };
        deepened = builtins.pathExists ./DEEP-DEPS/core-src;
        # Copy declared components by content. Referring to a subpath of the
        # enclosing flake would otherwise re-key the image for README/test edits.
        component = name: path: builtins.path { inherit name path; };
        workerFile = name: component ("git-vdb-caos-" + name) (./. + "/${name}");
        coreFile = mount: relative: component ("git-vdb-" + mount)
          (if deepened then ./. + "/DEEP-DEPS/${mount}" else ../.. + "/${relative}");
        toolchainSpec = builtins.fromTOML (builtins.readFile (coreFile "core-toolchain" "rust-toolchain.toml"));
        toolchain = pkgs.rust-bin.stable.${toolchainSpec.toolchain.channel}.minimal;
        craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
        # Explicit source assembly avoids pulling the whole consumer tree into
        # the image build or creating a dependency cycle through its tools.
        source = pkgs.runCommand "git-vdb-caos-source" {} ''
          mkdir -p $out/src $out/docs $out/integrations/caos/src
          cp -R ${coreFile "core-src" "src"}/. $out/src/
          cp -R ${coreFile "core-docs" "docs"}/. $out/docs/
          cp ${coreFile "core-manifest" "Cargo.toml"} $out/Cargo.toml
          cp ${coreFile "core-lock" "Cargo.lock"} $out/Cargo.lock
          cp ${coreFile "core-toolchain" "rust-toolchain.toml"} $out/rust-toolchain.toml
          cp ${coreFile "core-readme" "README.md"} $out/README.md
          cp ${coreFile "core-license" "LICENSE"} $out/LICENSE
          cp ${coreFile "core-llms" "llms.txt"} $out/llms.txt
          cp ${workerFile "Cargo.toml"} $out/integrations/caos/Cargo.toml
          cp ${workerFile "Cargo.lock"} $out/integrations/caos/Cargo.lock
          cp -R ${workerFile "src"}/. $out/integrations/caos/src/
        '';
        vendor = craneLib.vendorCargoDeps { cargoLock = workerFile "Cargo.lock"; };
        common = {
          pname = "git-vdb-caos";
          version = "0.1.0";
          src = source;
          strictDeps = true;
          cargoVendorDir = vendor;
          cargoExtraArgs = "--manifest-path integrations/caos/Cargo.toml";
        };
        artifacts = craneLib.buildDepsOnly common;
        worker = craneLib.buildPackage (common // {
          cargoArtifacts = artifacts;
          doCheck = false;
          # The build log names the executable precisely. The default installer
          # asks Cargo about the parent core manifest, outside this worker's lock.
          doNotPostBuildInstallCargoBinaries = true;
          installPhaseCommand = ''
            binary=$(${pkgs.jq}/bin/jq -r 'select(.reason == "compiler-artifact" and .target.name == "git-vdb-caos-worker" and .executable != null) | .executable' "$cargoBuildLog")
            test -f "$binary"
            install -Dm555 "$binary" "$out/bin/git-vdb-caos-worker"
          '';

        });
        modelLock = builtins.fromJSON (builtins.readFile (workerFile "model.lock.json"));
        weights = pkgs.fetchurl {
          inherit (modelLock.files."model.onnx") url sha256;
        };
        tokenizer = pkgs.fetchurl {
          inherit (modelLock.files."tokenizer.json") url sha256;
        };
        model = pkgs.runCommand "git-vdb-minilm-model" {} ''
          mkdir -p $out
          ln -s ${weights} $out/model.onnx
          ln -s ${tokenizer} $out/tokenizer.json
          cp ${workerFile "model.lock.json"} $out/model.lock.json
        '';
        python = pkgs.python3.withPackages (ps: [ ps.numpy ps.onnxruntime ps.tokenizers ]);
        embed = pkgs.writeShellScript "git-vdb-embed" ''
          export GIT_VDB_MODEL_DIR=${model}
          export TOKENIZERS_PARALLELISM=false
          exec ${python}/bin/python ${workerFile "embed.py"} "$@"
        '';
        root = pkgs.runCommand "git-vdb-caos-worker-root" {} ''
          mkdir -p $out
          ln -s ${worker}/bin/git-vdb-caos-worker $out/worker
          ln -s ${embed} $out/embed
        '';
      in {
        packages = {
          default = worker;
          inherit worker model;
          embedding = embed;
          caosImage = pkgs.dockerTools.buildLayeredImage {
            name = "git-vdb-caos";
            tag = "latest";
            contents = [ root ];
            config.Env = [ "TOKENIZERS_PARALLELISM=false" ];
          };
        };
        checks.worker = craneLib.cargoTest (common // { cargoArtifacts = artifacts; });
      });
}
