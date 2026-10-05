//! Choose the commit id embedded by `build.rs`.
//!
//! Colocated jj points Git HEAD at the parent of `@`. `jj new` leaves an empty
//! child there, so HEAD stays on the bookmarked commit. `jj edit` of that
//! bookmark makes `@` the commit itself, and HEAD slips to the parent.

use std::{env, path::Path, process::Command};

pub fn emit_build_commit(manifest_dir: &Path, jj: &std::ffi::OsStr) {
    if env::var("HERDR_BUILD_COMMIT").map(|v| !v.trim().is_empty()) == Ok(true) {
        return;
    }

    // Resolve paths through Git: `.git` is a file in a Git worktree.
    for name in ["HEAD", "refs/heads", "packed-refs"] {
        if let Some(path) = git_rev_parse(manifest_dir, &["--git-path", name]) {
            let path = manifest_dir.join(path);
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
    let jj_dir = manifest_dir.join(".jj");
    if jj_dir.is_dir() {
        // @ can change without moving Git HEAD (for example, jj describe).
        println!(
            "cargo:rerun-if-changed={}",
            jj_dir.join("working_copy/checkout").display()
        );
        let repo = jj_dir.join("repo");
        let repo = if repo.is_file() {
            // Linked jj workspaces store the shared repository path in this file.
            println!("cargo:rerun-if-changed={}", repo.display());
            std::fs::read_to_string(&repo)
                .ok()
                .map(|path| jj_dir.join(path.trim()))
                .unwrap_or(repo)
        } else {
            repo
        };
        println!(
            "cargo:rerun-if-changed={}",
            repo.join("op_heads/heads").display()
        );
    }

    // Colocated jj sets Git HEAD to the parent of `@`. An empty child leaves
    // HEAD on the bookmarked commit. `jj edit` of that bookmark makes `@` the
    // commit itself, so HEAD is the parent and would be recorded instead.
    let Some(git_head) = git_rev_parse(manifest_dir, &["HEAD"]) else {
        return;
    };
    let working_copy = jj_working_copy(manifest_dir, jj);
    let selected = select_build_commit(&git_head, working_copy.as_ref());
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

fn jj_working_copy(manifest_dir: &Path, jj: &std::ffi::OsStr) -> Option<JjWorkingCopy> {
    if !manifest_dir.join(".jj").is_dir() {
        return None;
    }
    let output = Command::new(jj)
        .args([
            "log",
            "-r",
            "@",
            "--ignore-working-copy",
            "--color=never",
            "--no-graph",
            "-T",
            JJ_WORKING_COPY_TEMPLATE,
        ])
        .current_dir(manifest_dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    parse_jj_working_copy(&stdout)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JjWorkingCopy {
    pub commit: String,
    pub parent: Option<String>,
    pub has_bookmark: bool,
    pub empty: bool,
    pub described: bool,
}

pub const JJ_WORKING_COPY_TEMPLATE: &str = concat!(
    "commit_id ++ \"\\n\" ++ ",
    "parents.map(|c| c.commit_id()).join(\" \") ++ \"\\n\" ++ ",
    "bookmarks.len() ++ \"\\n\" ++ ",
    "empty ++ \"\\n\" ++ ",
    "description.len() ++ \"\\n\""
);

pub fn parse_jj_working_copy(stdout: &str) -> Option<JjWorkingCopy> {
    let mut lines = stdout.lines();
    let commit = normalize_commit(lines.next()?);
    if !is_full_commit(&commit) {
        return None;
    }
    let parent_line = lines.next()?;
    let parent = parent_line
        .split_whitespace()
        .next()
        .map(normalize_commit)
        .filter(|parent| is_full_commit(parent));
    let bookmarks = lines.next()?.trim().parse::<u64>().ok()?;
    let empty = match lines.next()?.trim() {
        "true" => true,
        "false" => false,
        _ => return None,
    };
    let described = lines.next()?.trim().parse::<u64>().ok()? > 0;
    if lines.any(|line| !line.trim().is_empty()) {
        return None;
    }
    Some(JjWorkingCopy {
        commit,
        parent,
        has_bookmark: bookmarks > 0,
        empty,
        described,
    })
}

pub fn select_build_commit<'a>(
    git_head: &'a str,
    working_copy: Option<&'a JjWorkingCopy>,
) -> &'a str {
    let Some(working_copy) = working_copy else {
        return git_head;
    };
    if working_copy.has_bookmark
        && working_copy.commit != git_head
        && working_copy.parent.as_deref() == Some(git_head)
        && (!working_copy.empty || working_copy.described)
    {
        working_copy.commit.as_str()
    } else {
        git_head
    }
}

fn normalize_commit(text: &str) -> String {
    text.trim().to_ascii_lowercase()
}

fn is_full_commit(text: &str) -> bool {
    text.len() == 40 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::{parse_jj_working_copy, select_build_commit, JjWorkingCopy};

    use std::{ffi::OsStr, fs, path::PathBuf, process::Command};

    struct Fixture(PathBuf);

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    impl Fixture {
        fn command(&self, program: &str, args: &[&str]) -> String {
            let output = Command::new(program)
                .args(args)
                .current_dir(&self.0)
                .env("JJ_CONFIG", self.0.join("jj-config.toml"))
                .output()
                .expect("fixture command must start");
            assert!(
                output.status.success(),
                "{program} {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout)
                .expect("UTF-8 output")
                .trim()
                .to_string()
        }

        fn build(&self, override_commit: Option<&str>, jj: &OsStr) -> String {
            let mut command = Command::new(env!("CARGO"));
            command
                .args(["run", "--quiet", "--offline"])
                .current_dir(&self.0)
                .env("CARGO_TARGET_DIR", self.0.join("target"))
                .env("JJ_CONFIG", self.0.join("jj-config.toml"))
                .env("FIXTURE_JJ", jj)
                .env_remove("HERDR_BUILD_COMMIT");
            if let Some(commit) = override_commit {
                command.env("HERDR_BUILD_COMMIT", commit);
            }
            let output = command.output().expect("cargo fixture must start");
            assert!(
                output.status.success(),
                "cargo fixture: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout)
                .expect("UTF-8 build identity")
                .trim()
                .to_string()
        }
    }

    #[test]
    fn real_jj_template_and_cargo_rebuild_identity() {
        if !Command::new("jj")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
        {
            eprintln!("skipping git+jj build fixture: jj is unavailable");
            return;
        }
        let fixture = Fixture(
            std::env::current_dir()
                .expect("test cwd")
                .join("target/build-commit-fixtures")
                .join(format!(
                    "{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("clock")
                        .as_nanos()
                )),
        );
        fs::create_dir_all(fixture.0.join("src")).expect("fixture directory");
        fs::write(
            fixture.0.join("jj-config.toml"),
            "[user]\nname = 'Build Fixture'\nemail = 'fixture@example.invalid'\n",
        )
        .expect("isolated jj config");
        fs::write(fixture.0.join("Cargo.toml"), "[package]\nname = 'build-identity-fixture'\nversion = '0.0.0'\nedition = '2021'\n[workspace]\n").expect("fixture manifest");
        fs::write(
            fixture.0.join("src/main.rs"),
            "fn main() { println!(\"{}\", env!(\"HERDR_BUILD_COMMIT\")); }",
        )
        .expect("fixture executable");
        fs::copy(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/build_commit_select.rs"),
            fixture.0.join("select.rs"),
        )
        .expect("shared production module");
        fs::write(fixture.0.join("build.rs"), r#"
#[path = "select.rs"] mod select;
fn main() {
    println!("cargo:rerun-if-env-changed=HERDR_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=FIXTURE_JJ");
    select::emit_build_commit(&std::env::current_dir().expect("cwd"), &std::env::var_os("FIXTURE_JJ").expect("jj program"));
}
"#).expect("fixture build script");
        fs::write(fixture.0.join(".gitignore"), "/target/\n/Cargo.lock\n")
            .expect("ignore build outputs");
        fixture.command("git", &["init", "-q"]);
        fixture.command("git", &["config", "user.name", "Build Fixture"]);
        fixture.command("git", &["config", "user.email", "fixture@example.invalid"]);
        fixture.command(
            "git",
            &[
                "add",
                "Cargo.toml",
                "src",
                "build.rs",
                "select.rs",
                ".gitignore",
            ],
        );
        fixture.command("git", &["commit", "-qm", "initial"]);
        fixture.command("jj", &["git", "init", "--colocate"]);
        fs::write(fixture.0.join("payload"), "edited working copy\n")
            .expect("nonempty working copy");
        fixture.command("jj", &["describe", "-m", "bookmarked edit"]);
        fixture.command("jj", &["bookmark", "create", "fixture", "-r", "@"]);
        let template = fixture.command(
            "jj",
            &[
                "log",
                "-r",
                "@",
                "--ignore-working-copy",
                "--color=never",
                "--no-graph",
                "-T",
                super::JJ_WORKING_COPY_TEMPLATE,
            ],
        );
        let working_copy = super::parse_jj_working_copy(&template).expect("real template parses");
        let parent = fixture.command("git", &["rev-parse", "HEAD"]);
        super::emit_build_commit(&fixture.0, OsStr::new("jj"));
        assert!(working_copy.has_bookmark && working_copy.described && !working_copy.empty);
        assert_eq!(working_copy.parent.as_deref(), Some(parent.as_str()));
        assert_eq!(
            fixture.build(None, OsStr::new("jj")),
            &working_copy.commit[..12]
        );
        // Rewrite only jj state while Git HEAD remains on the same parent.
        fixture.command("jj", &["describe", "-m", "rewritten bookmark"]);
        assert_eq!(fixture.command("git", &["rev-parse", "HEAD"]), parent);
        let rewritten = fixture.command(
            "jj",
            &[
                "log",
                "-r",
                "@",
                "--ignore-working-copy",
                "--no-graph",
                "-T",
                "commit_id",
            ],
        );
        assert_ne!(rewritten, working_copy.commit);
        assert_eq!(fixture.build(None, OsStr::new("jj")), &rewritten[..12]);
        assert_eq!(
            fixture.build(Some("explicit-identity"), OsStr::new("jj")),
            "explicit-identity"
        );
        assert_eq!(
            fixture.build(None, fixture.0.join("missing-jj").as_os_str()),
            &parent[..12]
        );
        fixture.command("jj", &["new"]);
        let empty_child = fixture.command(
            "jj",
            &[
                "log",
                "-r",
                "@",
                "--ignore-working-copy",
                "--no-graph",
                "-T",
                super::JJ_WORKING_COPY_TEMPLATE,
            ],
        );
        let empty_child = super::parse_jj_working_copy(&empty_child).expect("empty child template");
        assert!(empty_child.empty && !empty_child.has_bookmark && !empty_child.described);
        assert_eq!(empty_child.parent.as_deref(), Some(rewritten.as_str()));
        assert_eq!(fixture.build(None, OsStr::new("jj")), &rewritten[..12]);
        fixture.command("jj", &["edit", "fixture"]);
        assert_eq!(fixture.command("git", &["rev-parse", "HEAD"]), parent);
        assert_eq!(fixture.build(None, OsStr::new("jj")), &rewritten[..12]);
    }

    const BOOKMARK: &str = "cb0d91d61e255a8a2b83a2f4c95a31bd38833d4d";
    const PARENT: &str = "4a1448ca6fd3ebb117de438f34c39dbcb0a89cd6";

    fn working_copy(empty: bool, has_bookmark: bool, described: bool) -> JjWorkingCopy {
        JjWorkingCopy {
            commit: BOOKMARK.to_string(),
            parent: Some(PARENT.to_string()),
            has_bookmark,
            empty,
            described,
        }
    }

    #[test]
    fn git_head_is_used_without_jj() {
        assert_eq!(select_build_commit(BOOKMARK, None), BOOKMARK);
    }

    #[test]
    fn empty_child_keeps_git_head_on_the_bookmark() {
        let working_copy = JjWorkingCopy {
            commit: "8fb4859d22976fca4c3bc890fde49ccad762878c".to_string(),
            parent: Some(BOOKMARK.to_string()),
            has_bookmark: false,
            empty: true,
            described: false,
        };
        assert_eq!(select_build_commit(BOOKMARK, Some(&working_copy)), BOOKMARK);
    }

    #[test]
    fn edited_bookmark_uses_working_copy_when_git_head_is_parent() {
        let working_copy = working_copy(false, true, true);
        assert_eq!(select_build_commit(PARENT, Some(&working_copy)), BOOKMARK);
    }

    #[test]
    fn edited_empty_bookmark_with_description_uses_working_copy() {
        let working_copy = working_copy(true, true, true);
        assert_eq!(select_build_commit(PARENT, Some(&working_copy)), BOOKMARK);
    }

    #[test]
    fn undescribed_empty_bookmark_keeps_git_head() {
        let working_copy = working_copy(true, true, false);
        assert_eq!(select_build_commit(PARENT, Some(&working_copy)), PARENT);
    }

    #[test]
    fn mismatched_parent_keeps_git_head() {
        let working_copy = working_copy(false, true, true);
        assert_eq!(select_build_commit(BOOKMARK, Some(&working_copy)), BOOKMARK);
    }

    #[test]
    fn parses_jj_log_template() {
        let stdout = format!("{BOOKMARK}\n{PARENT}\n1\nfalse\n13\n");
        let parsed = parse_jj_working_copy(&stdout).expect("template output");
        assert_eq!(parsed, working_copy(false, true, true));
        assert_eq!(select_build_commit(PARENT, Some(&parsed)), BOOKMARK);
    }

    #[test]
    fn parses_multiple_parents_using_the_first() {
        let other = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let stdout = format!("{BOOKMARK}\n{PARENT} {other}\n2\nfalse\n13\n");
        let parsed = parse_jj_working_copy(&stdout).expect("merge parents");
        assert_eq!(parsed.parent.as_deref(), Some(PARENT));
    }

    #[test]
    fn rejects_malformed_jj_output() {
        assert!(parse_jj_working_copy("not-a-commit\n\n0\ntrue\n0\n").is_none());
        assert!(
            parse_jj_working_copy(&format!("{BOOKMARK}\n{PARENT}\nnope\nfalse\n0\n")).is_none()
        );
        assert!(
            parse_jj_working_copy(&format!("{BOOKMARK}\n{PARENT}\n1\nfalse\n13\nextra\n"))
                .is_none()
        );
    }
}
