//! Resume references only; prompts, commands and process memory are never saved.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Claude,
    Codex,
    Opencode,
    Gemini,
    Pi,
    Omp,
}
impl AgentKind {
    pub const ALL: [Self; 6] = [
        Self::Claude,
        Self::Codex,
        Self::Opencode,
        Self::Gemini,
        Self::Pi,
        Self::Omp,
    ];

    pub fn executable(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::Gemini => "gemini",
            Self::Pi => "pi",
            Self::Omp => "omp",
        }
    }

    /// Whether `id` is a session ID as this CLI writes one. Never an option,
    /// a name or a command.
    fn names_session(self, id: &str) -> bool {
        match self {
            // `ses_`, twelve hex digits of time, fourteen random letters and digits.
            Self::Opencode => id.strip_prefix("ses_").is_some_and(|rest| {
                rest.len() == 26
                    && rest.bytes().enumerate().all(|(i, b)| {
                        if i < 12 {
                            b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
                        } else {
                            b.is_ascii_alphanumeric()
                        }
                    })
            }),
            // The others use UUIDs.
            _ => {
                id.len() == 36
                    && id.bytes().enumerate().all(|(i, b)| {
                        if matches!(i, 8 | 13 | 18 | 23) {
                            b == b'-'
                        } else {
                            b.is_ascii_hexdigit()
                        }
                    })
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSession {
    pub kind: AgentKind,
    /// None means the CLI was open but did not report a resumable session.
    pub session_id: Option<String>,
    pub cwd: PathBuf,
}
impl AgentSession {
    /// The most agents one agent can have started and still have open.
    pub const MAX_SPAWNED: usize = 8;
    /// An agent a person started can start agents, and those can start
    /// agents; these last cannot.
    pub const MAX_SPAWN_DEPTH: usize = 2;

    pub fn is_valid(&self) -> bool {
        self.cwd.is_absolute()
            && self.cwd.as_os_str().len() <= 32768
            && self
                .session_id
                .as_ref()
                .is_none_or(|id| self.kind.names_session(id))
    }
}

/// A pull request an agent linked to its terminal: `https://host/owner/repo/pull/N`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    url: String,
    number: u64,
}
impl PullRequest {
    /// A terminal keeps its most recent links.
    pub const MAX_PER_PANE: usize = 8;

    /// Accepts the address of a pull request or of a page under it. The saved
    /// form names only the pull request: no credentials, query or fragment.
    pub fn parse(url: &str) -> Option<Self> {
        let url = url.trim();
        if !url.get(..8)?.eq_ignore_ascii_case("https://") {
            return None;
        }
        let rest = &url[8..];
        let mut parts = rest.split(['?', '#']).next()?.split('/');
        let (host, owner, repository) = (parts.next()?, parts.next()?, parts.next()?);
        let named = |part: &str, extra: &[u8]| {
            !part.is_empty()
                && part.len() <= 100
                && !part.starts_with(['.', '-'])
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || extra.contains(&b))
        };
        if !named(host, b".-:") || !named(owner, b"-_.") || !named(repository, b"-_.") {
            return None;
        }
        if parts.next() != Some("pull") {
            return None;
        }
        let number = parts.next()?;
        if number.len() > 12 || !number.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let number = number.parse().ok().filter(|number| *number > 0)?;
        Some(Self {
            url: format!(
                "https://{}/{owner}/{repository}/pull/{number}",
                host.to_ascii_lowercase()
            ),
            number,
        })
    }
    pub fn url(&self) -> &str {
        &self.url
    }
    pub fn number(&self) -> u64 {
        self.number
    }
    /// Its host, owner and repository.
    pub fn location(&self) -> (&str, &str, &str) {
        let mut parts = self.url[8..].split('/');
        let mut part = || parts.next().unwrap_or_default();
        (part(), part(), part())
    }
    /// `owner/repo#N`.
    pub fn label(&self) -> String {
        let (_, owner, repository) = self.location();
        format!("{owner}/{repository}#{}", self.number)
    }
    /// Hosts treat owner and repository names without regard to case.
    pub fn same(&self, other: &Self) -> bool {
        self.url.eq_ignore_ascii_case(&other.url)
    }
}

/// A file an agent attached to its terminal for the user to look at: a
/// screenshot, a report, a recording. Only where the file is and what the
/// agent called it; never its contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    path: PathBuf,
    title: Option<String>,
}
impl Attachment {
    /// A terminal keeps its most recent attachments.
    pub const MAX_PER_PANE: usize = 24;
    /// Characters of a title; a longer one is cut here.
    pub const MAX_TITLE: usize = 120;
    const MAX_PATH: usize = 4096;

    /// Accepts an absolute path of ordinary length. A title is kept as one
    /// line without control characters, and left out when nothing remains.
    pub fn new(path: PathBuf, title: Option<&str>) -> Option<Self> {
        let text = path.as_os_str();
        if !path.is_absolute()
            || text.len() > Self::MAX_PATH
            || text.as_encoded_bytes().contains(&0)
            || path.file_name().is_none()
        {
            return None;
        }
        let title = title
            .map(|title| {
                title
                    .split(|c: char| c.is_whitespace() || c.is_control())
                    .filter(|word| !word.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(Self::MAX_TITLE)
                    .collect::<String>()
            })
            .filter(|title| !title.is_empty());
        Some(Self { path, title })
    }
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
    /// What the agent called the file, if it said.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }
    /// The file's own name.
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// What an agent linked and attached in a conversation that is on no tab
/// now, kept for when the conversation is resumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    pub kind: AgentKind,
    pub session_id: String,
    pub pull_requests: Vec<PullRequest>,
    pub attachments: Vec<Attachment>,
}
impl Conversation {
    /// The most recent conversations set aside are kept.
    pub const MAX: usize = 32;

    pub fn is_valid(&self) -> bool {
        self.kind.names_session(&self.session_id)
            && self.pull_requests.len() <= PullRequest::MAX_PER_PANE
            && self.attachments.len() <= Attachment::MAX_PER_PANE
            && !(self.pull_requests.is_empty() && self.attachments.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Controller, Model, Remote, WorkspaceId};
    fn agent() -> AgentSession {
        AgentSession {
            kind: AgentKind::Codex,
            session_id: Some("019a1234-5678-7000-8000-123456789abc".into()),
            cwd: std::env::temp_dir(),
        }
    }
    #[test]
    fn resume_references_reject_options_commands_and_relative_paths() {
        let mut value = agent();
        assert!(value.is_valid());
        for id in [
            "--last",
            "abc; touch x",
            "latest",
            "",
            "019a1234-5678-7000-8000-123456789abz",
        ] {
            value.session_id = Some(id.into());
            assert!(!value.is_valid());
        }
        // Each CLI's own form of ID, and no other's.
        value.kind = AgentKind::Opencode;
        value.session_id = Some("ses_ef579273dffe5vpZTGF29Kt3yQ".into());
        assert!(value.is_valid());
        for id in [
            "019a1234-5678-7000-8000-123456789abc",
            "ses_ef579273dffe5vpZTGF29Kt3y",
            "ses_EF579273dffe5vpZTGF29Kt3yQ",
            "ses_ef579273dffe5vpZTGF29Kt3-Q",
            "-s_ef579273dffe5vpZTGF29Kt3yQx",
        ] {
            value.session_id = Some(id.into());
            assert!(!value.is_valid(), "{id}");
        }
        for kind in [AgentKind::Gemini, AgentKind::Pi, AgentKind::Omp] {
            value.kind = kind;
            value.session_id = Some("01a10a88-4d43-71a3-8741-4822706a387c".into());
            assert!(value.is_valid());
            value.session_id = Some("ses_ef579273dffe5vpZTGF29Kt3yQ".into());
            assert!(!value.is_valid());
        }
        value.session_id = None;
        value.cwd = "relative".into();
        assert!(!value.is_valid());
    }
    #[test]
    fn pull_request_addresses_are_reduced_to_the_pull_request_or_rejected() {
        let link = PullRequest::parse(" https://GitHub.com/zevem/neptune/pull/83/files?w=1#diff ")
            .unwrap();
        assert_eq!(link.url(), "https://github.com/zevem/neptune/pull/83");
        assert_eq!(
            (link.number(), link.label()),
            (83, "zevem/neptune#83".into())
        );
        assert_eq!(link.location(), ("github.com", "zevem", "neptune"));
        assert!(
            link.same(&PullRequest::parse("https://github.com/Zevem/Neptune/pull/83").unwrap())
        );
        for url in [
            "http://github.com/zevem/neptune/pull/83",
            "https://github.com/zevem/neptune/issues/83",
            "https://github.com/zevem/neptune/pull/0",
            "https://github.com/zevem/neptune/pull/8x",
            "https://github.com/zevem/neptune/pull/",
            "https://user@github.com/zevem/neptune/pull/83",
            "https://github.com/zevem/../pull/83",
            "https://github.com/zevem/-rf/pull/83",
            "https://github.com/pull/83",
            "file:///zevem/neptune/pull/83",
        ] {
            assert_eq!(PullRequest::parse(url), None, "{url}");
        }
    }
    #[test]
    fn attachments_are_absolute_paths_with_one_line_titles() {
        let root = std::env::temp_dir();
        let file = root.join("shots").join("after.png");
        let kept = Attachment::new(file.clone(), Some("  After:\n the\tnew \u{7}dialog ")).unwrap();
        assert_eq!(kept.path(), file);
        assert_eq!(kept.name(), "after.png");
        assert_eq!(kept.title(), Some("After: the new dialog"));
        assert_eq!(
            Attachment::new(file.clone(), Some(" \n ")).unwrap().title(),
            None
        );
        let long = Attachment::new(file.clone(), Some(&"é".repeat(500))).unwrap();
        assert_eq!(long.title().unwrap().chars().count(), Attachment::MAX_TITLE);
        assert_eq!(Attachment::new("shots/after.png".into(), None), None);
        assert_eq!(Attachment::new(root.join("x".repeat(5000)), None), None);
        assert_eq!(Attachment::new(root.join("a\0b"), None), None);
    }
    #[test]
    fn attachments_follow_the_agent_are_bounded_and_can_be_removed() {
        let root = std::env::temp_dir();
        let mut controller = Controller::new(Model::default());
        controller
            .dispatch(Command::AddWorkspace {
                cwd: root.clone(),
                name: "a".into(),
                remote: None,
                group: None,
            })
            .unwrap();
        let pane = controller.model().active_pane().unwrap();
        let generation = controller.model().pane(pane).unwrap().generation();
        let file = |name: &str| root.join(name);
        let attach = |controller: &mut Controller, generation, name: &str, title: Option<&str>| {
            controller
                .dispatch(Command::PaneFileAttached {
                    pane,
                    generation,
                    attachment: Attachment::new(file(name), title).unwrap(),
                })
                .unwrap();
        };
        let names = |controller: &Controller| -> Vec<String> {
            controller
                .model()
                .pane(pane)
                .unwrap()
                .attachments()
                .iter()
                .map(Attachment::name)
                .collect()
        };
        // Without an agent there is nobody to attach anything.
        attach(&mut controller, generation, "a.png", None);
        assert!(names(&controller).is_empty());
        controller
            .dispatch(Command::PaneAgentChanged {
                pane,
                generation,
                agent: Some(agent()),
            })
            .unwrap();
        attach(&mut controller, generation + 1, "a.png", None);
        assert!(names(&controller).is_empty());
        attach(&mut controller, generation, "a.png", None);
        attach(&mut controller, generation, "b.txt", None);
        // The same file again is the newest, under its new title.
        attach(&mut controller, generation, "a.png", Some("After"));
        assert_eq!(names(&controller), ["b.txt", "a.png"]);
        let pane_ref = controller.model().pane(pane).unwrap();
        assert_eq!(pane_ref.attachments()[1].title(), Some("After"));
        assert!(controller.is_dirty());
        // The user removes one, then all of them.
        controller
            .dispatch(Command::RemoveAttachment {
                pane,
                path: file("b.txt"),
            })
            .unwrap();
        assert_eq!(names(&controller), ["a.png"]);
        for number in 0..Attachment::MAX_PER_PANE + 3 {
            attach(&mut controller, generation, &format!("{number}.png"), None);
        }
        let kept = names(&controller);
        assert_eq!(kept.len(), Attachment::MAX_PER_PANE);
        assert_eq!(kept[0], "3.png");
        let restored = Model::restore(
            controller.model().specs(),
            Some(WorkspaceId::new(1)),
            true,
            Default::default(),
        )
        .unwrap();
        assert_eq!(
            restored.pane(pane).unwrap().attachments().len(),
            Attachment::MAX_PER_PANE
        );
        controller
            .dispatch(Command::ClearAttachments { pane })
            .unwrap();
        assert!(names(&controller).is_empty());
        // They leave with the agent, like its other links.
        attach(&mut controller, generation, "a.png", None);
        controller
            .dispatch(Command::PaneAgentChanged {
                pane,
                generation,
                agent: None,
            })
            .unwrap();
        assert!(names(&controller).is_empty());
    }
    #[test]
    fn linked_pull_requests_follow_the_agent_and_are_bounded_and_durable() {
        let mut controller = Controller::new(Model::default());
        controller
            .dispatch(Command::AddWorkspace {
                cwd: std::env::temp_dir(),
                name: "a".into(),
                remote: None,
                group: None,
            })
            .unwrap();
        let pane = controller.model().active_pane().unwrap();
        let generation = controller.model().pane(pane).unwrap().generation();
        let link = |controller: &mut Controller, generation, number: u64| {
            controller
                .dispatch(Command::PanePullRequestLinked {
                    pane,
                    generation,
                    pull_request: PullRequest::parse(&format!(
                        "https://github.com/o/r/pull/{number}"
                    ))
                    .unwrap(),
                })
                .unwrap();
        };
        let numbers = |controller: &Controller| -> Vec<u64> {
            let pane = controller.model().pane(pane).unwrap();
            pane.pull_requests()
                .iter()
                .map(PullRequest::number)
                .collect()
        };
        // Only a terminal that runs an agent takes a link.
        link(&mut controller, generation, 1);
        assert!(numbers(&controller).is_empty());
        let open = |controller: &mut Controller, generation, agent| {
            controller
                .dispatch(Command::PaneAgentChanged {
                    pane,
                    generation,
                    agent,
                })
                .unwrap();
        };
        open(&mut controller, generation, Some(agent()));
        link(&mut controller, generation, 1);
        link(&mut controller, generation + 1, 2);
        link(&mut controller, generation, 1);
        assert_eq!(numbers(&controller), [1]);
        assert!(controller.is_dirty());
        for number in 2..=PullRequest::MAX_PER_PANE as u64 + 1 {
            link(&mut controller, generation, number);
        }
        assert_eq!(numbers(&controller), [2, 3, 4, 5, 6, 7, 8, 9]);
        // The same conversation reported again keeps them; restore does too.
        open(&mut controller, generation, Some(agent()));
        assert_eq!(numbers(&controller).len(), 8);
        let restored = Model::restore(
            controller.model().specs(),
            Some(WorkspaceId::new(1)),
            true,
            Default::default(),
        )
        .unwrap();
        assert_eq!(restored.pane(pane).unwrap().pull_requests().len(), 8);
        open(&mut controller, generation, None);
        assert!(numbers(&controller).is_empty());
        // An agent's first ID names the conversation its links were made in,
        // and brings back what that conversation had when the agent exited.
        let mut unnamed = agent();
        unnamed.session_id = None;
        open(&mut controller, generation, Some(unnamed));
        link(&mut controller, generation, 1);
        open(&mut controller, generation, Some(agent()));
        let first = vec![3, 4, 5, 6, 7, 8, 9, 1];
        assert_eq!(numbers(&controller), first);
        // Another conversation in the same run starts without them, and the
        // one before has them again when the agent returns to it.
        let attach = |controller: &mut Controller, name: &str| {
            controller
                .dispatch(Command::PaneFileAttached {
                    pane,
                    generation,
                    attachment: Attachment::new(std::env::temp_dir().join(name), None).unwrap(),
                })
                .unwrap();
        };
        let shown = |controller: &Controller| {
            let item = controller.model().pane(pane).unwrap();
            let files: Vec<_> = item.attachments().iter().map(Attachment::name).collect();
            (numbers(controller), files)
        };
        let nothing = (Vec::new(), Vec::new());
        attach(&mut controller, "a.png");
        let mut next = agent();
        next.session_id = Some("019a1234-5678-7000-8000-123456789def".into());
        open(&mut controller, generation, Some(next.clone()));
        assert_eq!(shown(&controller), nothing);
        assert_eq!(controller.model().pane(pane).unwrap().agent(), Some(&next));
        link(&mut controller, generation, 2);
        open(&mut controller, generation, Some(agent()));
        assert_eq!(shown(&controller), (first.clone(), vec!["a.png".to_owned()]));
        assert_eq!(controller.model().conversations().len(), 1);
        // A conversation the CLI has not named yet is another one as well;
        // what it links joins what its name had set aside.
        let mut unnamed = agent();
        unnamed.session_id = None;
        open(&mut controller, generation, Some(unnamed));
        assert_eq!(shown(&controller), nothing);
        link(&mut controller, generation, 3);
        attach(&mut controller, "b.png");
        open(&mut controller, generation, Some(next.clone()));
        assert_eq!(shown(&controller), (vec![2, 3], vec!["b.png".to_owned()]));
        // They wait through an exit, a restart and a close and reopen.
        open(&mut controller, generation, None);
        assert_eq!(shown(&controller), nothing);
        let restored = Model::restore(
            controller.model().specs(),
            Some(WorkspaceId::new(1)),
            true,
            Default::default(),
        )
        .unwrap()
        .with_conversations(controller.model().conversations().to_vec());
        assert_eq!(restored.conversations(), controller.model().conversations());
        open(&mut controller, generation, Some(agent()));
        assert_eq!(shown(&controller), (first.clone(), vec!["a.png".to_owned()]));
        controller.dispatch(Command::RestartPane(pane)).unwrap();
        let generation = controller.model().pane(pane).unwrap().generation();
        let open = |controller: &mut Controller, agent| {
            controller
                .dispatch(Command::PaneAgentChanged {
                    pane,
                    generation,
                    agent,
                })
                .unwrap();
        };
        open(&mut controller, Some(next.clone()));
        assert_eq!(numbers(&controller), [2, 3]);
        // Another CLI is another conversation whatever its ID.
        next.kind = AgentKind::Claude;
        open(&mut controller, Some(next));
        assert!(numbers(&controller).is_empty());
        // Only the newest conversations are kept, and only well-formed ones.
        let kept = |id: usize| crate::Conversation {
            kind: AgentKind::Codex,
            session_id: format!("019a1234-5678-7000-8000-{id:012}"),
            pull_requests: vec![PullRequest::parse("https://github.com/o/r/pull/1").unwrap()],
            attachments: Vec::new(),
        };
        let mut many: Vec<_> = (0..crate::Conversation::MAX + 2).map(kept).collect();
        many.push(crate::Conversation {
            pull_requests: Vec::new(),
            ..kept(99)
        });
        many.push(crate::Conversation {
            session_id: "--last".into(),
            ..kept(99)
        });
        let model = Model::default().with_conversations(many);
        assert_eq!(model.conversations().len(), crate::Conversation::MAX);
        assert_eq!(model.conversations()[0], kept(2));
    }
    #[test]
    fn links_leave_with_a_restart() {
        let mut controller = Controller::new(Model::default());
        controller
            .dispatch(Command::AddWorkspace {
                cwd: std::env::temp_dir(),
                name: "a".into(),
                remote: None,
                group: None,
            })
            .unwrap();
        let pane = controller.model().active_pane().unwrap();
        let generation = controller.model().pane(pane).unwrap().generation();
        controller
            .dispatch(Command::PaneAgentChanged {
                pane,
                generation,
                agent: Some(agent()),
            })
            .unwrap();
        let link = |controller: &mut Controller, generation, number: u64| {
            controller
                .dispatch(Command::PanePullRequestLinked {
                    pane,
                    generation,
                    pull_request: PullRequest::parse(&format!(
                        "https://github.com/o/r/pull/{number}"
                    ))
                    .unwrap(),
                })
                .unwrap();
        };
        let numbers = |controller: &Controller| -> Vec<u64> {
            let pane = controller.model().pane(pane).unwrap();
            pane.pull_requests()
                .iter()
                .map(PullRequest::number)
                .collect()
        };
        link(&mut controller, generation, 1);
        controller.dispatch(Command::RestartPane(pane)).unwrap();
        assert!(numbers(&controller).is_empty());
    }
    #[test]
    fn spawned_agents_open_out_of_view_and_leave_with_either_agent() {
        let mut controller = Controller::new(Model::default());
        controller
            .dispatch(Command::AddWorkspace {
                cwd: std::env::temp_dir(),
                name: "a".into(),
                remote: None,
                group: None,
            })
            .unwrap();
        let parent = controller.model().active_pane().unwrap();
        let generation = controller.model().pane(parent).unwrap().generation();
        let spawn = |controller: &mut Controller, parent, generation| {
            controller
                .dispatch(Command::SpawnAgent {
                    parent,
                    generation,
                    cwd: std::env::temp_dir(),
                })
                .map(|effects| {
                    effects
                        .iter()
                        .find_map(|effect| match effect {
                            crate::Effect::StartSession { pane, .. } => Some(*pane),
                            _ => None,
                        })
                        .unwrap()
                })
        };
        let open = |controller: &mut Controller, pane, agent| {
            let generation = controller.model().pane(pane).unwrap().generation();
            controller
                .dispatch(Command::PaneAgentChanged {
                    pane,
                    generation,
                    agent,
                })
                .unwrap();
        };
        let spawned = |controller: &Controller, parent| -> Vec<crate::PaneId> {
            controller
                .model()
                .spawned(parent)
                .map(|pane| pane.id())
                .collect()
        };
        // A terminal without an agent, or a stale one, starts nothing.
        assert!(spawn(&mut controller, parent, generation).is_err());
        open(&mut controller, parent, Some(agent()));
        assert!(spawn(&mut controller, parent, generation + 1).is_err());
        assert_eq!(controller.model().pane_count(), 1);

        let child = spawn(&mut controller, parent, generation).unwrap();
        let workspace = controller.model().workspace(WorkspaceId::new(1)).unwrap();
        assert_eq!(workspace.active(), parent);
        assert_eq!(workspace.layout().panes(), [parent]);
        assert!(workspace.is_background(child));
        assert_eq!(spawned(&controller, parent), [child]);
        // The link survives a restore only once the started agent can be resumed.
        let restore = |controller: &Controller| {
            Model::restore(
                controller.model().specs(),
                Some(WorkspaceId::new(1)),
                true,
                Default::default(),
            )
        };
        assert!(restore(&controller).is_err());
        open(&mut controller, child, Some(agent()));
        let restored = restore(&controller).unwrap();
        assert_eq!(restored.pane(child).unwrap().spawned_by(), Some(parent));
        assert!(restored.workspaces()[0].is_background(child));

        // Its agents may start agents; theirs may not.
        let grandchild = spawn(&mut controller, child, 1).unwrap();
        open(&mut controller, grandchild, Some(agent()));
        assert_eq!(
            spawn(&mut controller, grandchild, 1),
            Err(crate::Error::SpawnDepth)
        );
        // The started agent leaving ends its link, and its own agents' links.
        open(&mut controller, child, None);
        assert!(spawned(&controller, parent).is_empty());
        assert!(spawned(&controller, child).is_empty());

        // One that never opened leaves when its launch reports failure.
        let failed = spawn(&mut controller, parent, generation).unwrap();
        open(&mut controller, failed, None);
        assert!(spawned(&controller, parent).is_empty());

        for _ in 0..AgentSession::MAX_SPAWNED {
            spawn(&mut controller, parent, generation).unwrap();
        }
        assert_eq!(
            spawn(&mut controller, parent, generation),
            Err(crate::Error::SpawnLimit)
        );
        let all = spawned(&controller, parent);
        controller.dispatch(Command::RestartPane(all[0])).unwrap();
        controller.dispatch(Command::ClosePane(all[1])).unwrap();
        assert_eq!(
            spawned(&controller, parent).len(),
            AgentSession::MAX_SPAWNED - 2
        );
        controller
            .dispatch(Command::SessionFailed {
                pane: all[2],
                generation: 1,
                error: "no shell".into(),
            })
            .unwrap();
        assert_eq!(
            spawned(&controller, parent).len(),
            AgentSession::MAX_SPAWNED - 3
        );
        // The agent that started them leaving ends every link at once.
        open(&mut controller, parent, None);
        assert!(spawned(&controller, parent).is_empty());
        assert!(controller.is_dirty());
        // A loop or a link to a terminal without an agent is not restored.
        let mut specs = controller.model().specs();
        specs[0].panes[0].spawned_by = Some(specs[0].panes[0].id);
        specs[0].panes[0].agent = Some(agent());
        assert!(Model::restore(specs, None, true, Default::default()).is_err());
    }
    #[test]
    fn a_background_terminal_gets_a_tab_when_opened_or_left_alone() {
        let mut controller = Controller::new(Model::default());
        controller
            .dispatch(Command::AddWorkspace {
                cwd: std::env::temp_dir(),
                name: "a".into(),
                remote: None,
                group: None,
            })
            .unwrap();
        let workspace = WorkspaceId::new(1);
        let parent = controller.model().active_pane().unwrap();
        let open = |controller: &mut Controller, pane, agent| {
            controller
                .dispatch(Command::PaneAgentChanged {
                    pane,
                    generation: 1,
                    agent,
                })
                .unwrap();
        };
        let spawn = |controller: &mut Controller, parent| {
            let before = controller.model().pane_count();
            controller
                .dispatch(Command::SpawnAgent {
                    parent,
                    generation: 1,
                    cwd: std::env::temp_dir(),
                })
                .unwrap();
            let pane = controller.model().workspaces()[0].panes()[before].id();
            open(controller, pane, Some(agent()));
            pane
        };
        let tabs = |controller: &Controller| {
            let layout = controller.model().workspace(workspace).unwrap().layout();
            (layout.panes(), layout.shown())
        };
        open(&mut controller, parent, Some(agent()));
        let first = spawn(&mut controller, parent);
        let second = spawn(&mut controller, parent);
        assert_eq!(tabs(&controller), (vec![parent], vec![parent]));

        // Opening one gives it the tab after its starter and the focus.
        controller
            .dispatch(Command::FocusPane {
                workspace,
                pane: first,
            })
            .unwrap();
        assert_eq!(tabs(&controller), (vec![parent, first], vec![first]));
        assert_eq!(controller.model().active_pane(), Some(first));
        assert_eq!(controller.model().spawned(parent).count(), 2);
        // Nothing is placed against a terminal that has no place.
        assert!(
            controller
                .dispatch(Command::MovePane {
                    pane: first,
                    destination: crate::Destination::Tab {
                        pane: second,
                        index: 0,
                    },
                })
                .is_err()
        );

        // One that never had a tab closes with its agent: nobody saw its shell.
        let unseen = spawn(&mut controller, parent);
        let effects = controller
            .dispatch(Command::PaneAgentChanged {
                pane: unseen,
                generation: 1,
                agent: None,
            })
            .unwrap();
        assert!(effects.contains(&crate::Effect::StopSession {
            pane: unseen,
            generation: 1
        }));
        assert!(controller.model().pane(unseen).is_none());
        assert_eq!(tabs(&controller), (vec![parent, first], vec![first]));
        // Its starter's agent leaving gives the other a tab, out of view.
        open(&mut controller, parent, None);
        assert_eq!(
            tabs(&controller),
            (vec![parent, first, second], vec![first])
        );
        assert_eq!(controller.model().active_pane(), Some(first));

        // The last terminal in view closing leaves its place to the rest.
        open(&mut controller, first, Some(agent()));
        let third = spawn(&mut controller, first);
        controller.dispatch(Command::ClosePane(parent)).unwrap();
        controller.dispatch(Command::ClosePane(second)).unwrap();
        assert_eq!(tabs(&controller), (vec![first], vec![first]));
        controller.dispatch(Command::ClosePane(first)).unwrap();
        assert_eq!(tabs(&controller), (vec![third], vec![third]));
        assert_eq!(controller.model().active_pane(), Some(third));
        assert!(
            Model::restore(
                controller.model().specs(),
                Some(workspace),
                true,
                Default::default()
            )
            .is_ok()
        );

        // A terminal no agent answers for is never restored without a tab.
        let mut specs = controller.model().specs();
        specs[0].panes.push(crate::PaneSpec {
            id: crate::PaneId::new(90),
            cwd: std::env::temp_dir(),
            remote_cwd: None,
            agent: None,
            pull_requests: Vec::new(),
            attachments: Vec::new(),
            spawned_by: None,
            worktree: None,
        });
        assert!(Model::restore(specs, None, true, Default::default()).is_err());
    }
    #[test]
    fn agent_metadata_is_targeted_durable_and_cleared_by_reverse_actions() {
        let mut controller = Controller::new(Model::default());
        controller
            .dispatch(Command::AddWorkspace {
                cwd: std::env::temp_dir(),
                name: "a".into(),
                remote: None,
                group: None,
            })
            .unwrap();
        let pane = controller.model().active_pane().unwrap();
        let generation = controller.model().pane(pane).unwrap().generation();
        controller
            .dispatch(Command::PaneAgentChanged {
                pane,
                generation,
                agent: Some(agent()),
            })
            .unwrap();
        let saved = controller.model().specs();
        let restored =
            Model::restore(saved, Some(WorkspaceId::new(1)), true, Default::default()).unwrap();
        assert_eq!(restored.pane(pane).unwrap().agent(), Some(&agent()));
        controller.dispatch(Command::RestartPane(pane)).unwrap();
        assert!(controller.model().pane(pane).unwrap().agent().is_none());
        controller
            .dispatch(Command::PaneAgentChanged {
                pane,
                generation,
                agent: Some(agent()),
            })
            .unwrap();
        assert!(controller.model().pane(pane).unwrap().agent().is_none());
        let generation = generation + 1;
        controller
            .dispatch(Command::PaneAgentChanged {
                pane,
                generation,
                agent: Some(agent()),
            })
            .unwrap();
        controller
            .dispatch(Command::SetWorkspaceRemote {
                workspace: WorkspaceId::new(1),
                remote: Some(Remote::parse("host").unwrap().destination().into()),
            })
            .unwrap();
        assert!(controller.model().pane(pane).unwrap().agent().is_none());
        controller
            .dispatch(Command::PaneAgentChanged {
                pane,
                generation: generation + 1,
                agent: Some(agent()),
            })
            .unwrap();
        assert!(controller.model().pane(pane).unwrap().agent().is_none());
        controller.dispatch(Command::ClosePane(pane)).unwrap();
        controller
            .dispatch(Command::PaneAgentChanged {
                pane,
                generation,
                agent: Some(agent()),
            })
            .unwrap();
        assert!(controller.model().pane(pane).is_none());
    }
}
