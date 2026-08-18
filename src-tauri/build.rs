use std::{env, path::Path, path::PathBuf, process::Command};

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        stage_mediaremote_adapter();
    }
    tauri_build::build()
}

/// Builds the vendored MediaRemote adapter and stages it under `resources/macos/`
/// so `tauri.macos.conf.json` can bundle it into the app.
///
/// macOS 15.4 and later refuse MediaRemote access to unentitled processes, so the
/// now-playing reader runs through the entitled system Perl interpreter and this
/// adapter framework instead of linking MediaRemote directly.
fn stage_mediaremote_adapter() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir is set"));
    let vendor = manifest_dir.join("vendor/mediaremote-adapter");
    let script = vendor.join("bin/mediaremote-adapter.pl");

    if !vendor.join("CMakeLists.txt").exists() || !script.exists() {
        panic!(
            "The vendored MediaRemote adapter is missing from {}.\n\
             Run `git submodule update --init --recursive` and build again.",
            vendor.display()
        );
    }

    println!("cargo:rerun-if-changed={}", vendor.join("src").display());
    println!("cargo:rerun-if-changed={}", script.display());
    println!(
        "cargo:rerun-if-changed={}",
        vendor.join("CMakeLists.txt").display()
    );

    let build_dir = PathBuf::from(env::var("OUT_DIR").expect("out dir is set"))
        .join("mediaremote-adapter-build");
    let staged = manifest_dir.join("resources/macos");

    // The adapter's CMakeLists.txt already requests an x86_64 + arm64 framework,
    // which keeps the staged copy valid for universal builds.
    run(
        Command::new("cmake")
            .arg("-S")
            .arg(&vendor)
            .arg("-B")
            .arg(&build_dir)
            .arg("-DCMAKE_BUILD_TYPE=Release"),
        "configure the MediaRemote adapter",
    );
    run(
        Command::new("cmake")
            .arg("--build")
            .arg(&build_dir)
            .arg("--config")
            .arg("Release"),
        "build the MediaRemote adapter",
    );

    std::fs::create_dir_all(&staged).expect("create the macOS resource directory");
    // `ditto` preserves the framework's symlinks, bundle layout, and ad-hoc
    // signature, all of which a plain recursive file copy would flatten.
    copy(
        &build_dir.join("MediaRemoteAdapter.framework"),
        &staged.join("MediaRemoteAdapter.framework"),
    );
    copy(&script, &staged.join("mediaremote-adapter.pl"));
    // The adapter's `test` command needs this helper to report whether Perl still
    // reaches MediaRemote, which is how the runtime picks its backend.
    copy(
        &build_dir.join("MediaRemoteAdapterTestClient"),
        &staged.join("MediaRemoteAdapterTestClient"),
    );
}

fn copy(source: &Path, destination: &Path) {
    if destination.exists() {
        std::fs::remove_dir_all(destination)
            .or_else(|_| std::fs::remove_file(destination))
            .expect("clear the staged MediaRemote adapter");
    }
    run(
        Command::new("/usr/bin/ditto").arg(source).arg(destination),
        &format!("stage {}", source.display()),
    );
}

fn run(command: &mut Command, description: &str) {
    let status = command.status().unwrap_or_else(|error| {
        panic!(
            "Unable to {description}: {error}.\n\
             Building Music Companion for macOS requires CMake (`brew install cmake`)."
        )
    });
    assert!(status.success(), "Unable to {description}: {status}");
}
