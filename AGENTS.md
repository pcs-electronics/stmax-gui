# AGENTS.md

## Repository Intent

This repository is a native Rust desktop configuration tool for PCS Electronics STMAX transmitters.

Primary building blocks:

- `tokio` for async runtime and serial I/O
- `egui` and `eframe` for the native GUI
- `tokio-serial` for cross-platform serial enumeration and transport

## Canonical References

Treat these sources as the protocol truth:

- `../Firmware/Firmware.ino`
- `../stmax-config/stmax-config.py`
- `../stmax-config/README.md`

Use `~/WORK/MINIOCXO/ocxo-setup` as the reference for host-side serial enumeration style and runtime/UI separation.

## Current Shape

- `src/main.rs` starts the native window
- `src/app.rs` owns the UI shell and editable form state
- `src/serial.rs` owns serial discovery, connection lifecycle, async request/response handling, and device logs
- `src/protocol.rs` owns STMAX response parsing, validation, and command generation

## Working Conventions

- Preserve unrelated user changes.
- Do not revert or clean the worktree unless explicitly asked.
- Use `apply_patch` for manual edits.
- Prefer `rg` and `rg --files` for search.
- Keep source ASCII unless a file already requires Unicode.
- Add comments only when they explain non-obvious behavior.

## Protocol Guidance

- The device uses newline-terminated commands at `115200` baud.
- `?` returns `OK` plus the firmware help and the `Current settings:` block.
- `config-save` persists the active in-memory values.
- `config-defaults` restores default values in RAM but does not itself save EEPROM.
- Keep validation ranges in sync with `Firmware.ino`, not older notes.
- Do not shell out to helper scripts to talk to the transmitter. All protocol work stays in-process.

## Architecture Guidance

- Keep egui rendering separate from transport/runtime logic.
- Prefer message passing and snapshot updates over shared mutable state between UI and background tasks.
- Keep serial enumeration and USB summary formatting aligned with the `ocxo-setup` approach unless intentionally changing behavior.
- Preserve the current Windows serial behavior unless deliberately tested otherwise: assert DTR on open, best-effort RTS, and tolerate transient overlapped-read `os error 995`.

## Validation

Use the smallest useful checks for the change:

```bash
cargo fmt
cargo check
cargo test
cargo check --target x86_64-pc-windows-gnu
```

If a command cannot run because of missing toolchains, missing system libraries, dependency resolution, or sandbox limits, note that explicitly in the final handoff.

## Repo Hygiene

- Keep `Cargo.lock` tracked.
- Update `README.md` when setup, behavior, or protocol handling changes materially.
- Keep `AGENTS.md` aligned with the actual module structure.

