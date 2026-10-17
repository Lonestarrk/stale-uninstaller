# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

- Build: `cargo build`
- Run: `cargo run -p stale-gui` (binary is `stale`; needs elevation at runtime)
- Test all: `cargo test --workspace`
- Single test: `cargo test -p stale-core <name>`
- Lint: `cargo clippy --workspace --all-targets`

## Architecture

Stale Uninstaller is a Windows 10/11 x64 uninstaller (toolchain `stable-x86_64-pc-windows-msvc`) built as a Cargo workspace of two crates:

- `crates/stale-core` (lib): all program discovery and uninstall logic. It must never depend on egui/eframe.
- `crates/stale-gui` (bin `stale`): the egui/eframe front end over `stale-core`. All user-visible strings live in `crates/stale-gui/src/strings.rs`.

Add dependencies with `cargo add -p <crate> <dep>`. Design spec and plans are in `docs/`.
