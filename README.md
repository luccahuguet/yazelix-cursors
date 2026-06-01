# Yazelix Cursors

Standalone Yazelix cursor presets and terminal shader outputs

The user-facing command is `yzc`

```bash
nix run github:luccahuguet/yazelix-cursors#yzc -- --help
nix profile install github:luccahuguet/yazelix-cursors#yazelix_cursors
```

## What It Contains

- A reusable Yazelix cursor registry crate
- Data-driven cursor palette generation
- Ghostty-compatible cursor effect shader generation
- Packaged shader assets and generated shader examples
- A standalone `yzc` binary

## Standalone Ghostty-Compatible Usage

Initialize the shared cursor config:

```bash
yzc init
```

Generate a Ghostty include:

```bash
yzc generate ghostty
```

Then include it from Ghostty:

```conf
config-file = ~/.config/yazelix_ghostty_cursors/ghostty.conf
```

Day-to-day commands:

```bash
yzc list
yzc inspect
$EDITOR ~/.config/yazelix_ghostty_cursors/settings.jsonc
yzc generate ghostty
```

## Configuration

The standalone config still lives at the existing compatibility path:

```text
~/.config/yazelix_ghostty_cursors/settings.jsonc
```

The generated Ghostty include lives at:

```text
~/.config/yazelix_ghostty_cursors/ghostty.conf
```

Ghostty-compatible shader files are generated into:

```text
~/.config/yazelix_ghostty_cursors/shaders
```

## Cursor Options

Cursor trail selection supports:

- a named enabled cursor, such as `blaze`, `magma`, or `snow`
- `random`
- `none`

Trail and mode effects support:

- a named effect
- `random`
- `none`

Effects are global per generated Ghostty include. Ghostty does not support per-cursor effect switching inside one config include

## Compatibility

The repository is named `yazelix-cursors` because the cursor registry and shader assets are shared by Yazelix terminals, including Yazelix Terminal. The old `yazelix-ghostty-cursors` GitHub URL, Nix package output, and config directory remain compatibility surfaces for now.

## Boundary With Yazelix

`yazelix_cursors` owns reusable cursor registry validation, Ghostty-compatible shader generation, packaged assets, and the standalone `yzc` command

Yazelix consumes this crate for integrated cursor config, the config UI cursor tab, terminal materialization, and `yzx cursors`

The crate must not depend on:

- `yazelix_core`
- Zellij session state
- Home Manager install state
- Yazelix command palette or workspace orchestration

## Surfaces

- Product/repository: `yazelix-cursors`
- Command: `yzc`
- Rust crate: `yazelix_cursors`
- Compatibility Nix output: `yazelix_ghostty_cursors`
- Compatibility config directory: `~/.config/yazelix_ghostty_cursors`
- Integrated Yazelix command: `yzx cursors`

## Verification

From this repository:

```bash
cargo fmt --check
cargo check --all-targets
cargo test
cargo run --bin yzc -- --help
nix build .#yazelix_cursors
nix build .#yazelix_ghostty_cursors
nix run .#yzc -- --help
```
