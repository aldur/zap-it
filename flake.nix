{
  description = "ZapIt";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";

    crane = { url = "github:ipetkov/crane"; };

    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";

    flake-utils.url = "github:numtide/flake-utils";

    advisory-db = {
      url = "github:rustsec/advisory-db";
      flake = false;
    };
  };

  outputs =
    { self, nixpkgs, crane, rust-overlay, flake-utils, advisory-db, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let

        overlays = [ (import rust-overlay) ];

        pkgs = import nixpkgs { inherit system overlays; };

        inherit (pkgs) lib;

        # tell crane to use our toolchain
        rustToolchain = pkgs.pkgsBuildHost.rust-bin.fromRustupToolchainFile
          ./rust-toolchain.toml;
        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        # NOTE: We require the developer to have run `cargo sqlx prepare`
        # but we also `check` it below.
        sqlxFilter = path: _type:
          (builtins.match ".*.sqlx/.*.json$" path) != null;
        migrationsFilter = path: _type:
          (builtins.match ".*migrations/.*.sql$" path) != null;

        sourceFilter = path: type:
          (sqlxFilter path type) || (migrationsFilter path type)
          || (craneLib.filterCargoSources path type);

        unfilteredSrc = craneLib.path ./.;
        src = lib.cleanSourceWith {
          src = unfilteredSrc;
          filter = sourceFilter;
        };

        assetsFile = pkgs.runCommand "assets" { } ''
          mkdir $out
          cp -R ${craneLib.path ./.}/assets $out/assets
        '';

        # Common arguments can be set here to avoid repeating them later
        commonArgs = {
          inherit src;

          buildInputs = [
            # Add additional build inputs here
          ] ++ lib.optionals pkgs.stdenv.isDarwin [
            # Additional darwin specific inputs can be set here
          ];

          # Additional environment variables can be set directly
          # Ensure that even in case there's a `.env` file
          # or the variable DATABASE_URL is set, we rely on `.sqlx/*`
          SQLX_OFFLINE = "true";
        };

        # Build *just* the cargo dependencies, so we can reuse
        # all of that work (e.g. via cachix) when running in CI
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        name = "zap-it";

        # Build the actual crate itself, reusing the dependency
        # artifacts from above.
        zap-it = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          meta = { mainProgram = name; };
        });
      in {
        checks = {
          # Build the crate as part of `nix flake check` for convenience
          inherit zap-it;

          # Run clippy (and deny all warnings) on the crate source,
          # again, resuing the dependency artifacts from above.
          #
          # Note that this is done as a separate derivation so that
          # we can block the CI if there are issues here, but not
          # prevent downstream consumers from building our crate by itself.
          zap-it-clippy = craneLib.cargoClippy (commonArgs // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs =
              "--all-targets --all-features -- --deny warnings -W clippy::pedantic";
          });

          zap-it-sqlx = craneLib.mkCargoDerivation (commonArgs // {

            buildPhaseCargoCommand = ''
              mkdir -p $out
              export DATABASE_URL="sqlite:$out/db.sqlite3"
              sqlx --version

              SQLX_CLI_VERSION=$(sqlx --version | cut -d " " -f 2)
              SQLX_VERSION=$(yq -p toml -oy '.package[] | select(.name == "sqlx") | .version' Cargo.lock)
              if [[ $SQLX_CLI_VERSION != $SQLX_VERSION ]]; then
                echo "sqlx-cli version mismatches sqlx's: $SQLX_CLI_VERSION vs $SQLX_VERSION"
                exit 1
              fi

              sqlx database create
              sqlx migrate run
              cargo sqlx prepare --check
            '';

            inherit cargoArtifacts;
            # doInstallCargoArtifacts = false;

            nativeBuildInputs = with pkgs; [ sqlx-cli yq-go ];

            pnameSuffix = "-sqlx";
          });

          zap-it-doc =
            craneLib.cargoDoc (commonArgs // { inherit cargoArtifacts; });

          # Check formatting
          zap-it-fmt = craneLib.cargoFmt { inherit src; };

          # Audit dependencies
          zap-it-audit = craneLib.cargoAudit {
            inherit src advisory-db;
            # Not our dependency.
            cargoAuditExtraArgs = " --ignore RUSTSEC-2023-0071";
          };

          # Run tests with cargo-nextest
          # Consider setting `doCheck = false` on `zap-it` if you do not want
          # the tests to run twice
          # NOTE: Disabled since there are no tests atm
          # zap-it-nextest = craneLib.cargoNextest (commonArgs // {
          #   inherit cargoArtifacts;
          #   partitions = 1;
          #   partitionType = "count";
          # });
        } // lib.optionalAttrs (system == "x86_64-linux") {
          # NB: cargo-tarpaulin only supports x86_64 systems
          # Check code coverage (note: this will not upload coverage anywhere)
          zap-it-coverage =
            craneLib.cargoTarpaulin (commonArgs // { inherit cargoArtifacts; });
        };

        packages = {
          default = zap-it;

          dockerImage = pkgs.dockerTools.streamLayeredImage {
            inherit name;
            tag = "latest";
            contents = [ zap-it assetsFile ];
            config = { Cmd = [ "${zap-it}/bin/${name}" ]; };
          };

          # zap-it-llvm-coverage = craneLibLLvmTools.cargoLlvmCov (commonArgs // {
          #   inherit cargoArtifacts;
          # });
        };

        devShells.default = craneLib.devShell {
          # Inherit inputs from checks.
          checks = self.checks.${system};

          # Additional dev-shell environment variables can be set directly
          # MY_CUSTOM_DEVELOPMENT_VAR = "something else";

          # Extra inputs can be added here; cargo and rustc are provided by default.
          packages = [ pkgs.neovim pkgs.ripgrep pkgs.fzf pkgs.git ];
        };
      });
}
