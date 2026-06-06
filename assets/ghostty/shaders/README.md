# Cursor Trail Shaders

This directory contains cursor trail shaders for Ghostty terminal.

## Structure

The cursor trail shaders are built from a shared source file plus cursor registry
data:

```
shaders/
├── cursor_trail_common.glsl     # Shared functions
└── cursor_trail_*.glsl          # Generated locally/runtime only (gitignored)
```

## How It Works

`yzc generate ghostty` and Yazelix runtime materialization copy these packaged
shader sources, then call the Rust `yazelix_cursors` shader generators. Every
cursor preset is rendered from cursor registry data; presets describe colors while
shared shader behavior comes from `cursor_trail_common.glsl` and registry settings
such as `glow` and `duration`.

## Making Changes

### To modify shared functions:

1. Edit `cursor_trail_common.glsl`
2. Run `yzc generate ghostty` or the Yazelix runtime materialization path to regenerate outputs

### To modify a preset:

1. Edit its `[[cursor]]` table in `yazelix_ghostty_cursors_default.toml` or your local `~/.config/yazelix_ghostty_cursors/settings.jsonc`
2. Run `yzc generate ghostty` or the Yazelix runtime materialization path to regenerate outputs

### To create a new preset:

1. Add the cursor to `enabled_cursors`
2. Add a matching `[[cursor]]` table with `family = "mono"` or `family = "split"`
3. Run `yzc generate ghostty` or the Yazelix runtime materialization path to regenerate outputs

### Manual build (for testing or local preview):

```bash
yzc generate ghostty
```

By default, that writes generated shaders into the standalone cursor config tree:

```text
~/.config/yazelix_ghostty_cursors/shaders
```

For an isolated preview, pass explicit config and share roots:

```bash
yzc --config-dir /tmp/yazelix_cursor_preview --share-dir <package>/share/yazelix/yazelix_cursors init
yzc --config-dir /tmp/yazelix_cursor_preview --share-dir <package>/share/yazelix/yazelix_cursors generate ghostty
```

## Build Process

The build is Rust-owned:
- Runs from `yzc generate ghostty` and Yazelix terminal materialization
- Combines `cursor_trail_common.glsl` with Rust-rendered mono/split preset code
- Outputs complete shaders ready for Ghostty to use
- Does not require Nushell
- Honors `settings.glow = none | low | medium | high` from `settings.jsonc`
- Honors `settings.duration = 0.25..4.0` from `settings.jsonc` as a multiplier for movement-trail timing

## Important Notes

- **DO NOT directly edit** the generated `cursor_trail_*.glsl` files - your changes will be overwritten
- **ALWAYS edit** either `cursor_trail_common.glsl`, the cursor registry, or the Rust generator
- The generated shaders are **not** git-tracked; the maintained source is the common library, registry, and Rust generator

## Variant Categories

### Mono (6 presets)
- `blaze`, `snow`, `sunset`, `ocean`, `forest`, `cosmic`
- Each preset defines one base color in `yazelix_cursors_default.toml`; Yazelix derives the accent unless `accent_color` overrides it

### Split (5 presets)
- `eclipse`, `dusk`, `orchid`, `reef`, `magma`
- Each preset defines two colors plus `divider = "vertical" | "horizontal"` and `transition = "soft" | "hard"`
