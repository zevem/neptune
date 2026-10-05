//! A git worktree Neptune made for an agent: where it is, the branch its tab
//! is named after, and what tells that the branch has work to merge.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Worktree {
    /// The checkout the worktree was added to.
    pub repository: PathBuf,
    pub path: PathBuf,
    pub branch: String,
    /// The commit the branch stood at when the worktree was opened. A branch
    /// still there has nothing that could have been merged.
    pub start: String,
}
impl Worktree {
    pub fn is_valid(&self) -> bool {
        let path = |path: &PathBuf| {
            path.is_absolute()
                && path.as_os_str().len() <= 32768
                && !path.as_os_str().as_encoded_bytes().contains(&0)
        };
        path(&self.repository)
            && path(&self.path)
            && self.path != self.repository
            // A name git takes as a branch and never as an option.
            && !self.branch.is_empty()
            && self.branch.len() <= 255
            && !self.branch.starts_with('-')
            && !self
                .branch
                .chars()
                .any(|c| c.is_control() || c.is_whitespace())
            && matches!(self.start.len(), 40 | 64)
            && self.start.bytes().all(|b| b.is_ascii_hexdigit())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_absolute_paths_a_branch_name_and_a_commit_are_accepted() {
        let valid = Worktree {
            repository: std::env::temp_dir().join("repo"),
            path: std::env::temp_dir().join("repo.worktrees").join("feat-x"),
            branch: "feat/x".into(),
            start: "a".repeat(40),
        };
        assert!(valid.is_valid());
        for broken in [
            Worktree {
                repository: "repo".into(),
                ..valid.clone()
            },
            Worktree {
                path: valid.repository.clone(),
                ..valid.clone()
            },
            Worktree {
                branch: "--force".into(),
                ..valid.clone()
            },
            Worktree {
                branch: "two words".into(),
                ..valid.clone()
            },
            Worktree {
                branch: String::new(),
                ..valid.clone()
            },
            Worktree {
                start: "HEAD".into(),
                ..valid.clone()
            },
        ] {
            assert!(!broken.is_valid(), "{broken:?}");
        }
    }
}
