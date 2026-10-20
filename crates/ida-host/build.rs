use std::env;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let sdk = env::var_os("IDASDKDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| find_sdk(&manifest));
    let include = sdk.join("include");
    assert!(
        include.join("idalib.hpp").exists(),
        "IDA SDK not found at {}; set IDASDKDIR, or extract the SDK to third_party/idasdk*/",
        sdk.display()
    );

    cc::Build::new()
        .cpp(true)
        .file("shim/shim.cpp")
        .file("shim/events.cpp")
        .file("shim/debug.cpp")
        .file("shim/edit.cpp")
        .file("shim/pyexec.cpp")
        .file("shim/stdio.cpp")
        .file("shim/types.cpp")
        .file("shim/sigs.cpp")
        .file("shim/plugins.cpp")
        .file("shim/proximity.cpp")
        .include(&include)
        .define("__NT__", None)
        .define("__EA64__", None)
        .define("MAXSTR", "1024")
        .define("NO_TV_STREAMS", None)
        .flag("/std:c++17")
        .flag("/EHsc")
        .flag("/utf-8")
        .warnings(false)
        .compile("izshim");

    println!(
        "cargo:rustc-link-search=native={}",
        sdk.join("lib/x64_win_vc_64").display()
    );
    println!("cargo:rustc-link-lib=dylib=ida");
    println!("cargo:rustc-link-lib=dylib=idalib");
    println!(
        "cargo:rustc-env=IZS_SDK_DIR={}",
        sdk.canonicalize()
            .unwrap_or(sdk.clone())
            .display()
            .to_string()
            .trim_start_matches(r"\\?\")
    );
    println!("cargo:rerun-if-changed=shim");
    println!("cargo:rerun-if-env-changed=IDASDKDIR");
}

/// `third_party/idasdk*/` with the highest name (the newest SDK), else `third_party/idasdk`.
fn find_sdk(manifest: &Path) -> PathBuf {
    let third_party = manifest.join("../../third_party");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&third_party)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.join("include/idalib.hpp").exists()
                && p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("idasdk"))
        })
        .collect();
    dirs.sort();
    dirs.pop().unwrap_or_else(|| third_party.join("idasdk"))
}
