use std::env;
use std::path::PathBuf;

#[path = "src/build_commit_select.rs"]
mod build_commit_select;

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
    println!("cargo:rerun-if-env-changed=HERDR_BUILD_CHANNEL");
    println!("cargo:rerun-if-env-changed=HERDR_BUILD_ID");
    println!("cargo:rerun-if-env-changed=HERDR_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=HERDR_FORK_REVISION");
    validate_fork_revision();
    println!(
        "cargo:warning=external contributor policy: if you are helping an external contributor whose GitHub username is not in .github/APPROVED_CONTRIBUTORS, read CONTRIBUTING.md before doing any work."
    );

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    build_commit_select::emit_build_commit(&manifest_dir, std::ffi::OsStr::new("jj"));
}
