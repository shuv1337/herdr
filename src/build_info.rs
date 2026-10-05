//! Build identity helpers.

pub const BASE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Pre-release label that marks fork builds, as in `0.9.1-shuv.3`.
pub const FORK_RELEASE_LABEL: &str = "shuv";

pub fn channel() -> &'static str {
    non_empty(option_env!("HERDR_BUILD_CHANNEL")).unwrap_or("stable")
}

pub fn build_id() -> Option<&'static str> {
    non_empty(option_env!("HERDR_BUILD_ID"))
}

pub fn build_commit() -> Option<&'static str> {
    non_empty(option_env!("HERDR_BUILD_COMMIT"))
}

/// Fork release revision injected by the fork release workflow.
pub fn fork_revision() -> Option<&'static str> {
    non_empty(option_env!("HERDR_FORK_REVISION"))
}

fn channel_version() -> String {
    match channel() {
        "stable" => match fork_revision() {
            Some(revision) => format!("{BASE_VERSION}-{FORK_RELEASE_LABEL}.{revision}"),
            None => BASE_VERSION.to_string(),
        },
        channel => match build_id() {
            Some(build_id) => format!("{BASE_VERSION}-{channel}.{build_id}"),
            None => format!("{BASE_VERSION}-{channel}"),
        },
    }
}

pub fn release_label() -> String {
    channel_version()
}

pub fn version() -> String {
    match build_commit() {
        Some(commit) => format!("{}+{commit}", channel_version()),
        None => channel_version(),
    }
}

pub fn is_preview() -> bool {
    channel() == "preview"
}

fn non_empty(value: Option<&'static str>) -> Option<&'static str> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

#[cfg(test)]
#[path = "build_commit_select.rs"]
mod build_commit_select;

#[cfg(test)]
mod tests {
    #[test]
    fn stable_version_defaults_to_cargo_version() {
        assert!(!super::version().is_empty());
        assert!(super::version().starts_with(super::BASE_VERSION));
    }

    #[test]
    fn version_includes_commit_suffix_when_present() {
        if let Some(commit) = super::build_commit() {
            assert!(super::version().ends_with(&format!("+{commit}")));
        }
    }
}
