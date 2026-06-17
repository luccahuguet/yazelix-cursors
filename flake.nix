{
  description = "Standalone Yazelix cursor presets and terminal shader outputs";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      fenix,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      mkPkgs = system: nixpkgs.legacyPackages.${system};
      yzcPackage =
        system: pkgs:
        let
          rustToolchain = fenix.packages.${system}.combine [
            fenix.packages.${system}.stable.cargo
            fenix.packages.${system}.stable.rustc
          ];
          rustPlatform = pkgs.makeRustPlatform {
            cargo = rustToolchain;
            rustc = rustToolchain;
          };
          source = pkgs.lib.cleanSourceWith {
            name = "yazelix-cursors-source";
            src = ./.;
            filter =
              path: _type:
              let
                relativePath = pkgs.lib.removePrefix ((toString ./.) + "/") (toString path);
              in
              relativePath != "target"
              && !pkgs.lib.hasPrefix "target/" relativePath
              && relativePath != ".git"
              && !pkgs.lib.hasPrefix ".git/" relativePath;
          };
        in
        rustPlatform.buildRustPackage {
          pname = "yazelix-cursors";
          version = "0.1.0";

          src = source;
          cargoLock = {
            lockFile = ./Cargo.lock;
            outputHashes = {
              "ratconfig-0.1.0" = "sha256-axG4lSlrxHF2C7BA678sxeIEo4yw79b0gtqPCIhngrU=";
            };
          };
          cargoBuildFlags = [
            "--bin"
            "yzc"
          ];
          dontStrip = true;

          postInstall = ''
            set -eu

            share_dir="$out/share/yazelix/yazelix_cursors"
            examples_dir="$share_dir/examples"
            work="$TMPDIR/yazelix_cursors_export"
            config_dir="$work/config"

            mkdir -p "$share_dir" "$examples_dir" "$config_dir"
            cp -R ${./assets/ghostty/shaders} "$share_dir/shaders"

            "$out/bin/yzc" --config-dir "$config_dir" --share-dir "$share_dir" init
            "$out/bin/yzc" --config-dir "$config_dir" --share-dir "$share_dir" generate ghostty

            chmod -R u+w "$share_dir/shaders"
            rm -rf "$share_dir/shaders"
            cp -R "$config_dir/shaders" "$share_dir/shaders"

            cat > "$examples_dir/ghostty_blaze_tail.conf" <<EOF
# Yazelix cursor shader example for Ghostty
#
# Add these lines to a Ghostty config to try the blaze palette with the tail effect
custom-shader = $share_dir/shaders/cursor_trail_blaze.glsl
custom-shader = $share_dir/shaders/generated_effects/tail.glsl
EOF

            cat > "$share_dir/README.md" <<EOF
# Yazelix Cursors

This package exports Yazelix cursor presets and complete Ghostty-compatible cursor shader files

The package also includes the \`yzc\` CLI for standalone cursor config:

\`\`\`bash
yzc init
yzc generate ghostty
\`\`\`

Then include the generated file from Ghostty:

\`\`\`conf
config-file = ~/.config/yazelix_cursors/ghostty.conf
\`\`\`

Use one cursor palette shader and one optional effect shader in your Ghostty config:

\`\`\`conf
custom-shader = $share_dir/shaders/cursor_trail_blaze.glsl
custom-shader = $share_dir/shaders/generated_effects/tail.glsl
\`\`\`

Generated shader root:

\`\`\`text
$share_dir/shaders
\`\`\`

Example config:

\`\`\`text
$examples_dir/ghostty_blaze_tail.conf
\`\`\`

This package does not mutate your Ghostty config and does not include Yazelix runtime reroll behavior
EOF

            required_files="
              $share_dir/shaders/cursor_trail_blaze.glsl
              $share_dir/shaders/cursor_trail_snow.glsl
              $share_dir/shaders/cursor_trail_ice.glsl
              $share_dir/shaders/cursor_trail_midnight.glsl
              $share_dir/shaders/cursor_trail_eclipse.glsl
              $share_dir/shaders/cursor_trail_magma.glsl
              $share_dir/shaders/generated_effects/tail.glsl
              $share_dir/shaders/generated_effects/ripple.glsl
              $examples_dir/ghostty_blaze_tail.conf
              $out/bin/yzc
            "
            for required in $required_files; do
              test -s "$required"
            done
            test ! -e "$share_dir/shaders/build_shaders.nu"
            grep -q "custom-shader = $share_dir/shaders/cursor_trail_blaze.glsl" "$examples_dir/ghostty_blaze_tail.conf"
          '';

          passthru.yazelixCursorPackageContract = {
            schemaVersion = 1;
            packageName = "yazelix-cursors";
            shareRoot = "share/yazelix/yazelix_cursors";
            shaderRoot = "share/yazelix/yazelix_cursors/shaders";
            generatedEffectRoot = "share/yazelix/yazelix_cursors/shaders/generated_effects";
            requiredTargets = [
              "ghostty"
              "yzxterm"
              "rio"
              "ratty"
              "protocol_cursor_positions"
            ];
            requiredShaderFiles = [
              "cursor_trail_common.glsl"
              "cursor_trail_reef.glsl"
              "upstream_effects/ripple_rectangle_cursor.glsl"
              "generated_effects/tail.glsl"
            ];
            forbiddenShaderFiles = [
              "build_shaders.nu"
            ];
          };

          meta = {
            description = "Standalone Yazelix cursor presets and terminal shader outputs";
            homepage = "https://github.com/luccahuguet/yazelix-cursors";
            license = pkgs.lib.licenses.asl20;
            mainProgram = "yzc";
          };
        };
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = mkPkgs system;
          yzc = yzcPackage system pkgs;
        in
        {
          default = yzc;
          yzc = yzc;
          yazelix_cursors = yzc;
        }
      );

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.yzc}/bin/yzc";
        };
        yzc = {
          type = "app";
          program = "${self.packages.${system}.yzc}/bin/yzc";
        };
        yazelix_cursors = {
          type = "app";
          program = "${self.packages.${system}.yzc}/bin/yzc";
        };
      });

      checks = forAllSystems (system: {
        yazelix_cursors = self.packages.${system}.yazelix_cursors;
        yzc = self.packages.${system}.yzc;
      });
    };
}
