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
- Terminal target contracts for Ghostty, mars, Rio, Ratty, and protocol cursor positions
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
config-file = ~/.config/yazelix_cursors/ghostty.conf
```

Day-to-day commands:

```bash
yzc list
yzc list-targets
yzc inspect
$EDITOR ~/.config/yazelix_cursors/settings.jsonc
yzc generate ghostty
```

Generate a launch-local Rio-compatible config from a base `config.toml`:

```bash
yzc materialize rio-compatible-config --source-config ~/.config/rio/config.toml
```

The command prints the generated config directory. Launch Rio-derived
terminals with their config-home environment variable pointing at that
directory.

## Configuration

The standalone config still lives at the existing compatibility path:

```text
~/.config/yazelix_cursors/settings.jsonc
```

The generated Ghostty include lives at:

```text
~/.config/yazelix_cursors/ghostty.conf
```

Ghostty-compatible shader files are generated into:

```text
~/.config/yazelix_cursors/shaders
```

## Cursor Options

Cursor trail selection supports:

- a named enabled cursor, such as `blaze`, `magma`, `snow`, `ice`, or `midnight`
- `random`
- `none`

When a Yazelix consumer passes light or auto appearance context, `random` skips `snow` when another enabled cursor is available. Explicit `snow` selections are still honored.

Trail and mode effects support:

- a named effect
- `random`
- `none`

Effects are global per generated Ghostty include. Ghostty does not support per-cursor effect switching inside one config include

## Terminal Targets

`yzc list-targets` prints the child-owned target contract used by package consumers:

- `ghostty` emits a Ghostty include plus palette and effect shader files
- `rio-compatible-config` materializes launch-local `[colors].cursor`
  for Rio and Rio-derived terminals
- `mars` consumes the Ghostty-compatible shader files with the Rio trail uniform ABI
- `rio` documents the Rio-compatible shader ABI surface
- `ratty` has an explicit experimental no-op target slot
- `protocol_cursor_positions` documents protocol-backed multi-cursor output as separate from GLSL shaders

## Compatibility

The repository is named `yazelix-cursors` because the cursor registry and shader assets are shared by Yazelix terminals, including Yazelix Terminal. The old `yazelix-ghostty-cursors` package/config names are not exposed as compatibility aliases.

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
- Nix output: `yazelix_cursors`
- Config directory: `~/.config/yazelix_cursors`
- Integrated Yazelix command: `yzx cursors`

## Verification

From this repository:

```bash
cargo fmt --check
cargo check --all-targets
cargo test
cargo run --bin yzc -- --help
cargo run --bin yzc -- list-targets
nix build .#yazelix_cursors
nix run .#yzc -- --help
```
