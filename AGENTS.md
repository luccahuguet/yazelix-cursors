# Agent Guidelines

Shared Yazelix agent workflow and release policy live in the main repo:

- https://github.com/luccahuguet/yazelix/blob/main/AGENTS.md
- In sibling local checkouts, read `../yazelix/AGENTS.md` first

Only Yazelix Cursors-specific guidance belongs here.

## Local Scope

- This repo owns the `yzc` CLI, cursor registry, generated shader assets, and terminal target contracts.
- Main Yazelix owns per-window runtime integration, settings wiring, and launch-scoped cursor facts.
- Keep Ghostty-compatible shader generation deterministic for standalone and main-repo consumers.

## Local Commands

- `cargo fmt --all -- --check`
- `cargo test`
- `nix build .#yazelix_cursors --no-link`
- `nix run .#yzc -- --help`

## Integration Notes

Main Yazelix consumes the package and Rust crate through pinned child revisions. Publish child changes before updating main locks for coupled runtime work.
