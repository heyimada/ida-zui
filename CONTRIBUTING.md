# Contributing

Thanks for helping. Read [AGENTS.md](AGENTS.md) first: it explains the layout, how to add a host capability end to end, and the testing rules.

## Before you open a pull request

- `cargo fmt --all` and `cargo clippy -p ida-proto -p zui-editor -p ida-zui --all-targets -- -D warnings` are clean.
- `cargo test --workspace` passes. If you have IDA and the SDK, `.\scripts\e2e.ps1` must pass too. CI cannot build `ida-host` because the SDK is licensed, so it runs only the SDK-free crates.
- New behaviour is covered by the protocol E2E (`crates/ida-proto/examples/e2e.rs`) or a UI capture. Unit tests are for pure logic only; no tautological or change-detector tests.
- UI changes: attach a screenshot taken with `scripts/ui-capture.ps1` on a public sample such as `find.exe`.

## Never commit

The IDA SDK or anything from it, IDA binaries, `.i64` / `.id0` and other database files, or captures that show personal paths or files. `third_party/`, `testdata/` and `artifacts/` are git-ignored for this reason.

## Style

Match the surrounding code: naming, comment density and idiom. Use the design-system pieces in `crates/app/src/ui.rs` and theme tokens, never literal colours. Keep the app crate free of `unsafe`.
