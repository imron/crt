use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let git_hash = get_git_hash().unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=CRT_BUILD_HASH={git_hash}");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        build_gui();
    }
}

fn get_git_hash() -> Option<String> {
    let repo = git2::Repository::discover(".").ok()?;
    let head = repo.head().ok()?;
    let commit = head.peel_to_commit().ok()?;
    Some(commit.id().to_string())
}

fn build_gui() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let gui_dir = manifest_dir.join("gui");
    emit_rerun(&gui_dir);

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let swift_build_path = out_dir.join("swift");
    let profile = env::var("PROFILE").unwrap();
    let swift_profile = match profile.as_str() {
        "release" => "release",
        _ => "debug",
    };
    let arch = swift_arch();
    let triple = format!("{arch}-apple-macosx");
    let sdk_path = macos_sdk_path();

    let status = Command::new("xcrun")
        .args([
            "--sdk",
            "macosx",
            "swift",
            "build",
            "--package-path",
            gui_dir.to_str().expect("gui path is not valid UTF-8"),
            "--build-path",
            swift_build_path
                .to_str()
                .expect("OUT_DIR path is not valid UTF-8"),
            "--sdk",
            &sdk_path,
            "--triple",
            &triple,
            "-c",
            swift_profile,
        ])
        .status()
        .expect("failed to run swift build");
    if !status.success() {
        panic!("swift build of gui/ failed with status {status}");
    }

    let lib = find_named_file(&swift_build_path, "libCrtGui.a");
    let lib_dir = lib.parent().unwrap_or(&swift_build_path);
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static:+whole-archive=CrtGui");

    for path in swift_runtime_library_paths(&arch) {
        println!("cargo:rustc-link-search=native={path}");
        println!("cargo:rustc-link-arg=-Wl,-rpath,{path}");
    }

    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=AppKit");
    println!("cargo:rustc-link-lib=framework=SwiftUI");
    println!("cargo:rustc-link-lib=framework=QuartzCore");
    println!("cargo:rustc-link-lib=framework=Combine");
    println!("cargo:rustc-link-lib=c++");
    println!("cargo:rustc-link-lib=objc");

    let plist = gui_dir.join("Info.plist");
    println!("cargo:rerun-if-changed={}", plist.display());
    println!(
        "cargo:rustc-link-arg=-Wl,-sectcreate,__TEXT,__info_plist,{}",
        plist.display()
    );

    generate_app_icon(&manifest_dir, &out_dir);
}

fn generate_app_icon(manifest_dir: &Path, out_dir: &Path) {
    let src = manifest_dir.join("docs/crt.png");
    println!("cargo:rerun-if-changed={}", src.display());
    let square = out_dir.join("crt-icon-square.png");
    let icon = out_dir.join("crt-icon.png");
    let src_s = src.to_str().expect("icon source path is not valid UTF-8");
    let square_s = square
        .to_str()
        .expect("icon square path is not valid UTF-8");
    let icon_s = icon.to_str().expect("icon path is not valid UTF-8");

    let pad = Command::new("sips")
        .args([
            "--padColor",
            "000000",
            "--padToHeightWidth",
            "1448",
            "1448",
            src_s,
            "--out",
            square_s,
        ])
        .status()
        .expect("failed to run sips to pad app icon");
    if !pad.success() {
        panic!("sips pad of docs/crt.png failed with status {pad}");
    }

    let resize = Command::new("sips")
        .args(["-z", "1024", "1024", square_s, "--out", icon_s])
        .status()
        .expect("failed to run sips to resize app icon");
    if !resize.success() {
        panic!("sips resize of app icon failed with status {resize}");
    }
}

fn swift_arch() -> String {
    match env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "aarch64" => "arm64".to_string(),
        other => other.to_string(),
    }
}

fn macos_sdk_path() -> String {
    let output = Command::new("xcrun")
        .args(["--sdk", "macosx", "--show-sdk-path"])
        .output()
        .expect("failed to run xcrun --show-sdk-path");
    if !output.status.success() {
        panic!(
            "xcrun --show-sdk-path failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8(output.stdout)
        .expect("SDK path is not valid UTF-8")
        .trim()
        .to_string()
}

fn swift_runtime_library_paths(arch: &str) -> Vec<String> {
    let target = format!("{arch}-apple-macosx14.0");
    let output = Command::new("xcrun")
        .args(["swift", "-target", &target, "-print-target-info"])
        .output()
        .expect("failed to run swift -print-target-info");
    if !output.status.success() {
        panic!(
            "swift -print-target-info failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let info: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("swift -print-target-info produced invalid JSON");
    info["paths"]["runtimeLibraryPaths"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect()
}

fn find_named_file(root: &Path, file_name: &str) -> PathBuf {
    let mut matches = Vec::new();
    collect_named_files(root, file_name, &mut matches);
    matches
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("could not find {file_name} under {}", root.display()))
}

fn collect_named_files(dir: &Path, file_name: &str, matches: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_named_files(&path, file_name, matches);
        } else if path.file_name().is_some_and(|name| name == file_name) {
            matches.push(path);
        }
    }
}

fn emit_rerun(path: &Path) {
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
    {
        return;
    }
    println!("cargo:rerun-if-changed={}", path.display());
    if path.is_dir() {
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                emit_rerun(&entry.path());
            }
        }
    }
}
