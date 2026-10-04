//! Choose the commit id embedded by `build.rs`.
//!
//! Colocated jj points Git HEAD at the parent of `@`. `jj new` leaves an empty
//! child there, so HEAD stays on the bookmarked commit. `jj edit` of that
//! bookmark makes `@` the commit itself, and HEAD slips to the parent.

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
