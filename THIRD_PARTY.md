# Third-party notices

ida-zui itself is MIT licensed (see `LICENSE`). It includes or depends on the following.

## Included in this repository

- **IBM Plex Sans** (`crates/app/assets/fonts/`): Copyright IBM Corp., licensed under the SIL Open Font License 1.1. The license text is next to the fonts in `crates/app/assets/fonts/OFL-IBMPlex.txt`.
- **Text input** (`crates/app/src/input.rs`): adapted from GPUI's `input.rs` example, Copyright Zed Industries, Inc., licensed under the Apache License 2.0 (<https://www.apache.org/licenses/LICENSE-2.0>). The file header keeps the attribution.
- **Icons** (`crates/app/assets/icons/`): simple 16 px stroke icons written for this project.

## Dependencies (not included)

- **zui / gpui**: the UI framework, taken as a git dependency on the `zeronsh/zui` fork of Zed's gpui (<https://github.com/zeronsh/zui>) at the revision pinned in `Cargo.toml`. gpui is licensed under the Apache License 2.0 by Zed Industries.
- Other Rust crates are pulled from crates.io; see `Cargo.lock`, and each crate's own license.

## Not included, required at build and run time

- **IDA Pro, idalib and the IDA SDK** are proprietary products of Hex-Rays SA and are never distributed with ida-zui. You need your own licence. IDA is a trademark of Hex-Rays SA; ida-zui is not affiliated with Hex-Rays.
