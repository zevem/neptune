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
    /// `owner/repo#N`.
    pub fn label(&self) -> String {
        let mut parts = self.url[8..].split('/').skip(1);
        format!(
            "{}/{}#{}",
            parts.next().unwrap_or_default(),
            parts.next().unwrap_or_default(),
            self.number
        )
    }
    /// Hosts treat owner and repository names without regard to case.
    pub fn same(&self, other: &Self) -> bool {
        self.url.eq_ignore_ascii_case(&other.url)
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
        // A new conversation in the same run keeps them; restore does too.
        let mut next = agent();
        next.session_id = Some("019a1234-5678-7000-8000-123456789def".into());
        open(&mut controller, generation, Some(next));
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
        open(&mut controller, generation, Some(agent()));
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
            spawned_by: None,
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
