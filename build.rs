use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "src/build_commit_select.rs"]
mod build_commit_select;

// Keep this pin aligned with vendor/libghostty-vt/build.zig.zon and release CI.
const REQUIRED_ZIG_VERSION: &str = "0.16.0";

fn zig_remediation() -> String {
    format!(
        "Set the ZIG environment variable to an absolute Zig {REQUIRED_ZIG_VERSION} executable (PowerShell: `$env:ZIG = 'C:\\\\path\\\\to\\\\zig.exe'`), or run the command through `nix develop -c` where available."
    )
}

fn resolve_zig() -> String {
    let configured = env::var("ZIG").ok();
    let zig = configured.as_deref().unwrap_or("zig");
    let output = Command::new(zig).arg("version").output().unwrap_or_else(|err| {
        let remediation = zig_remediation();
        panic!(
            "failed to run `{zig} version`: {err}; Herdr requires Zig {REQUIRED_ZIG_VERSION}. {remediation}"
        )
    });
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if stderr.is_empty() {
            String::new()
        } else {
            format!(": {stderr}")
        };
        let remediation = zig_remediation();
        panic!(
            "`{zig} version` failed with {}{detail}; Herdr requires Zig {REQUIRED_ZIG_VERSION}. {remediation}",
            output.status
        );
    }

    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if version != REQUIRED_ZIG_VERSION {
        let source = if configured.is_some() { "ZIG" } else { "PATH" };
        let remediation = zig_remediation();
        panic!(
            "Herdr requires Zig {REQUIRED_ZIG_VERSION}, but {source} resolved `{zig}` to Zig {version}. {remediation}"
        );
    }

    zig.to_string()
}

fn zig_target(target: &str) -> &str {
    match target {
        "x86_64-unknown-linux-gnu" => "x86_64-linux-gnu",
        "aarch64-unknown-linux-gnu" => "aarch64-linux-gnu",
        "x86_64-unknown-linux-musl" => "x86_64-linux-musl",
        "aarch64-unknown-linux-musl" => "aarch64-linux-musl",
        "x86_64-apple-darwin" => "x86_64-macos",
        "aarch64-apple-darwin" => "aarch64-macos",
        "x86_64-pc-windows-msvc" => "x86_64-windows-msvc",
        "aarch64-pc-windows-msvc" => "aarch64-windows-msvc",
        other => panic!("unsupported target for libghostty-vt build: {other}"),
    }
}

fn env_bool(name: &str) -> Option<bool> {
    match env::var(name) {
        Ok(value) => match value.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            other => panic!("invalid boolean value for {name}: {other}"),
        },
        Err(env::VarError::NotPresent) => None,
        Err(err) => panic!("failed to read {name}: {err}"),
    }
}

fn emit_build_commit(manifest_dir: &Path) {
    if env::var("HERDR_BUILD_COMMIT").map(|v| !v.trim().is_empty()) == Ok(true) {
        return;
    }

    let git_dir = manifest_dir.join(".git");
    println!("cargo:rerun-if-changed={}", git_dir.join("HEAD").display());
    println!(
        "cargo:rerun-if-changed={}",
        git_dir.join("refs/heads").display()
    );

    // Colocated jj sets Git HEAD to the parent of `@`. An empty child leaves
    // HEAD on the bookmarked commit. `jj edit` of that bookmark makes `@` the
    // commit itself, so HEAD is the parent and would be recorded instead.
    let Some(git_head) = git_rev_parse(manifest_dir, &["HEAD"]) else {
        return;
    };
    let working_copy = jj_working_copy(manifest_dir);
    let selected = build_commit_select::select_build_commit(&git_head, working_copy.as_ref());
    let commit = git_rev_parse(manifest_dir, &["--short=12", selected])
        .or_else(|| git_rev_parse(manifest_dir, &["--short=12", "HEAD"]));
    if let Some(commit) = commit {
        println!("cargo:rustc-env=HERDR_BUILD_COMMIT={commit}");
    }
}

fn git_rev_parse(manifest_dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("rev-parse")
        .args(args)
        .current_dir(manifest_dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn jj_working_copy(manifest_dir: &Path) -> Option<build_commit_select::JjWorkingCopy> {
    if !manifest_dir.join(".jj").is_dir() {
        return None;
    }
    let output = Command::new("jj")
        .args([
            "log",
            "-r",
            "@",
            "--ignore-working-copy",
            "--color=never",
            "--no-graph",
            "-T",
            build_commit_select::JJ_WORKING_COPY_TEMPLATE,
        ])
        .current_dir(manifest_dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    build_commit_select::parse_jj_working_copy(&stdout)
}

fn validate_fork_revision() {
    let Ok(revision) = env::var("HERDR_FORK_REVISION") else {
        return;
    };
    let revision = revision.trim();
    if revision.is_empty() {
        return;
    }
    match revision.parse::<u32>() {
        Ok(value) if value > 0 && value.to_string() == revision => {}
        _ => panic!("HERDR_FORK_REVISION must be a positive integer, got {revision:?}"),
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=vendor/libghostty-vt.vendor.json");
    println!("cargo:rerun-if-changed=vendor/libghostty-vt/build.zig");
    println!("cargo:rerun-if-changed=vendor/libghostty-vt/build.zig.zon");
    println!("cargo:rerun-if-changed=vendor/libghostty-vt/include");
    println!("cargo:rerun-if-changed=vendor/libghostty-vt/pkg");
    println!("cargo:rerun-if-changed=vendor/libghostty-vt/src");
    println!("cargo:rerun-if-changed=vendor/libghostty-vt/VERSION");
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_OPTIMIZE");
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_SIMD");
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_ZIG_SYSTEM_DIR");
    println!("cargo:rerun-if-env-changed=HERDR_BUILD_CHANNEL");
    println!("cargo:rerun-if-env-changed=HERDR_BUILD_ID");
    println!("cargo:rerun-if-env-changed=HERDR_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=HERDR_FORK_REVISION");
    validate_fork_revision();
    println!("cargo:rerun-if-env-changed=ZIG");
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_WINDOWS_LIBC");
    println!(
        "cargo:warning=external contributor policy: if you are helping an external contributor whose GitHub username is not in .github/APPROVED_CONTRIBUTORS, read CONTRIBUTING.md before doing any work."
    );

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    emit_build_commit(&manifest_dir);
    let vendored_dir = manifest_dir.join("vendor/libghostty-vt");
    let optimize = env::var("LIBGHOSTTY_VT_OPTIMIZE").unwrap_or_else(|_| "ReleaseFast".into());
    let simd = env_bool("LIBGHOSTTY_VT_SIMD").unwrap_or(true);
    let target = env::var("TARGET").expect("TARGET");
    let zig_target = zig_target(&target);
    let version_string = fs::read_to_string(vendored_dir.join("VERSION"))
        .expect("failed to read vendored libghostty-vt VERSION")
        .trim()
        .to_string();

    let zig = resolve_zig();
    let mut command = Command::new(&zig);
    command
        .arg("build")
        .arg("-Demit-lib-vt")
        .arg(format!("-Doptimize={optimize}"))
        .arg(format!("-Dsimd={simd}"))
        .arg(format!("-Dtarget={zig_target}"))
        .arg(format!("-Dversion-string={version_string}"))
        .arg("-Demit-xcframework=false");
    if target.ends_with("windows-msvc") {
        if let Some(libc_file) = env::var_os("LIBGHOSTTY_VT_WINDOWS_LIBC") {
            println!(
                "cargo:rerun-if-changed={}",
                PathBuf::from(&libc_file).display()
            );
            command.arg("--libc").arg(libc_file);
        }
    }
    if let Ok(system_dir) = env::var("LIBGHOSTTY_VT_ZIG_SYSTEM_DIR") {
        command.arg("--system").arg(system_dir);
    }

    let status = command
        .current_dir(&vendored_dir)
        .status()
        .unwrap_or_else(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                panic!(
                    "zig executable not found (looked for {zig:?}; set the ZIG \
                     environment variable to point at the zig binary). Building \
                     the vendored libghostty-vt requires Zig 0.16.0: install it from \
                     https://ziglang.org/download/, then retry the build"
                );
            }
            panic!("failed to execute zig build for vendored libghostty-vt: {err}");
        });
    assert!(
        status.success(),
        "zig build for vendored libghostty-vt failed: {status}. \
         Building Herdr requires Zig 0.16.0; check `zig version` \
         or set ZIG to the path of a Zig 0.16.0 binary, then retry"
    );

    let lib_dir = vendored_dir.join("zig-out/lib");
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    if target.contains("apple-darwin") {
        let static_lib = lib_dir.join("libghostty-vt.a");
        println!("cargo:rustc-link-arg={}", static_lib.display());
    } else if target.contains("windows-msvc") {
        println!("cargo:rustc-link-lib=static=ghostty-vt-static");
    } else {
        println!("cargo:rustc-link-lib=static=ghostty-vt");
    }
}
