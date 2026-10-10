//! Everything one pull request says, for the tab that shows it: its title
//! and description, where it stands, its checks, its reviews, its
//! conversation and the files it changes. It is read through the person's
//! own GitHub CLI when the person opens it, and nothing read is saved.
//!
//! These are the requests and what they reduce to. The tab's worker decides
//! when they are made.
use super::pull_requests::{Checks, State, cli, cli_complaint};
use neptune_model::PullRequest;

/// A description, a conversation and a hundred checks fit well inside this.
const MAX_RESPONSE: u64 = 2 * 1024 * 1024;
/// One page of changed files with their patches.
const MAX_FILES_RESPONSE: u64 = 8 * 1024 * 1024;
/// The most checks, comments, reviews and review conversations asked for.
const MAX_CHECKS: usize = 100;
const MAX_ENTRIES: usize = 50;
const MAX_REPLIES: usize = 30;
const MAX_COMMITS: usize = 50;
/// The longest comment sent: it is handed to the GitHub CLI as an argument.
pub const MAX_COMMENT: usize = 16_000;
/// The most changed files listed: one page of GitHub's.
pub const MAX_FILES: usize = 100;

/// Why a pull request could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The GitHub CLI could not be started.
    NoCli,
    /// The GitHub CLI has no sign-in for the pull request's host.
    SignedOut,
    /// The host has no such pull request, or the account may not read it.
    NotFound,
    /// No network, a host that is not GitHub, or an answer that made no sense.
    Unavailable,
}
impl Failure {
    /// What happened, and what the person can do about it.
    pub fn explain(self, host: &str) -> (&'static str, String) {
        match self {
            Self::NoCli => (
                "GitHub CLI not found",
                "Neptune reads pull requests with the GitHub CLI (gh). Install it, then sign in with gh auth login.".into(),
            ),
            Self::SignedOut => (
                "Not signed in",
                if host == "github.com" {
                    "Sign in to GitHub in a terminal with gh auth login.".into()
                } else {
                    format!("Sign in to {host} in a terminal with gh auth login --hostname {host}.")
                },
            ),
            Self::NotFound => (
                "Pull request not found",
                "It may have been removed, or the account signed in to the GitHub CLI may not read its repository.".into(),
            ),
            Self::Unavailable => (
                "Could not read the pull request",
                format!("Check the network and that {host} is a GitHub host, then try again."),
            ),
        }
    }
}

/// Whether the pull request can be merged as it stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Merge {
    /// GitHub has not worked it out yet.
    Unknown,
    Ready,
    Conflicts,
    /// Its branch lacks commits of the branch it merges into.
    Behind,
    /// A rule of the repository holds it: a review or a check it requires.
    Blocked,
    /// It can be merged, with checks that fail or still run.
    Unstable,
}

/// What reviewers concluded, taken together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Approved,
    ChangesRequested,
    ReviewRequired,
}

/// What one review said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Approved,
    ChangesRequested,
    Commented,
    Dismissed,
    /// Asked to review, and has not yet.
    Awaited,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reviewer {
    pub name: String,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Outcome {
    Failed,
    /// A workflow of a first-time contributor waits to be allowed to run.
    Awaiting,
    Running,
    Passed,
    Cancelled,
    /// Skipped, or finished without a verdict.
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    /// The workflow that runs it, when it has one.
    pub workflow: String,
    pub outcome: Outcome,
    /// How long it ran, once it finished.
    pub seconds: Option<u32>,
    /// Its page, when it has one.
    pub url: String,
}

/// One of the eight reactions the host has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emoji {
    ThumbsUp,
    ThumbsDown,
    Laugh,
    Hooray,
    Confused,
    Heart,
    Rocket,
    Eyes,
}
impl Emoji {
    pub const ALL: [Self; 8] = [
        Self::ThumbsUp,
        Self::ThumbsDown,
        Self::Laugh,
        Self::Hooray,
        Self::Confused,
        Self::Heart,
        Self::Rocket,
        Self::Eyes,
    ];
    /// The host's name for it.
    fn content(self) -> &'static str {
        match self {
            Self::ThumbsUp => "THUMBS_UP",
            Self::ThumbsDown => "THUMBS_DOWN",
            Self::Laugh => "LAUGH",
            Self::Hooray => "HOORAY",
            Self::Confused => "CONFUSED",
            Self::Heart => "HEART",
            Self::Rocket => "ROCKET",
            Self::Eyes => "EYES",
        }
    }
    /// The reaction in a word, for a face that has no picture of it.
    pub fn word(self) -> &'static str {
        match self {
            Self::ThumbsUp => "+1",
            Self::ThumbsDown => "−1",
            Self::Laugh => "Laugh",
            Self::Hooray => "Hooray",
            Self::Confused => "Confused",
            Self::Heart => "Heart",
            Self::Rocket => "Rocket",
            Self::Eyes => "Eyes",
        }
    }
}

/// How many gave one reaction, and whether the person is among them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reaction {
    pub emoji: Emoji,
    pub count: u32,
    pub mine: bool,
}

/// A reply in a review conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub author: String,
    /// Seconds since 1970.
    pub at: i64,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Comment,
    Review(Verdict),
    /// A conversation on a line of a changed file.
    Thread {
        /// The host's name for the conversation, which replies and its
        /// resolving are addressed to.
        id: String,
        /// The person may reply to it, and may resolve or reopen it.
        can_reply: bool,
        can_resolve: bool,
        path: String,
        line: Option<u32>,
        resolved: bool,
        /// The line it is about has changed since.
        outdated: bool,
        replies: Vec<Reply>,
    },
}

/// A commit of the pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// Its name, and the first seven characters of it.
    pub oid: String,
    pub short: String,
    pub headline: String,
    pub author: String,
    pub at: i64,
}

/// How a pull request is merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Merge,
    Squash,
    Rebase,
}
impl Method {
    pub fn label(self) -> &'static str {
        match self {
            Self::Merge => "Merge",
            Self::Squash => "Squash and merge",
            Self::Rebase => "Rebase and merge",
        }
    }
    fn flag(self) -> &'static str {
        match self {
            Self::Merge => "--merge",
            Self::Squash => "--squash",
            Self::Rebase => "--rebase",
        }
    }
}

/// What the account signed in to the GitHub CLI may do with the pull request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Allowed {
    /// Mark it ready or a draft, close it and reopen it.
    pub update: bool,
    pub merge: bool,
    /// Approve it or request changes: anyone but its author.
    pub judge: bool,
    /// The ways the repository lets a pull request be merged.
    pub methods: Vec<Method>,
    /// Have the host merge it by itself once it may be.
    pub auto_merge: bool,
    /// Bring its branch up to date with the one it merges into.
    pub update_branch: bool,
    /// Edit its title and description.
    pub edit: bool,
    pub react: bool,
    pub label: bool,
    pub request: bool,
}

/// A pull request next to this one in a stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Neighbour {
    pub number: u64,
    pub title: String,
    /// This one is stacked on it; otherwise it is stacked on this one.
    pub below: bool,
}

/// One thing said on the pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The host's name for it, which reactions are addressed to.
    pub id: String,
    pub reactions: Vec<Reaction>,
    /// The person may rewrite it.
    pub can_edit: bool,
    pub author: String,
    pub at: i64,
    pub kind: Kind,
    pub body: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detail {
    /// The host's name for the pull request.
    pub id: String,
    /// How the host will merge it by itself, when it was asked to.
    pub auto_merge: Option<Method>,
    /// The changed files the person marked as viewed.
    pub viewed: Vec<String>,
    /// Reactions to its description.
    pub reactions: Vec<Reaction>,
    /// It merges into another pull request's branch, not the main one.
    pub stacked: bool,
    /// The open pull requests it is stacked on and those stacked on it.
    pub stack: Vec<Neighbour>,
    pub title: String,
    pub state: State,
    pub author: String,
    /// The description, in Markdown as written.
    pub body: String,
    /// The branch it merges into, and the one it comes from: `owner:branch`
    /// for one of another repository.
    pub base: String,
    pub head: String,
    pub opened: i64,
    pub updated: i64,
    /// When it was merged or closed.
    pub ended: Option<i64>,
    pub merged_by: String,
    /// How many commits it has, and the newest of them, oldest first.
    pub commits: u32,
    pub history: Vec<Commit>,
    /// The name of its newest commit: its files changed when this does.
    pub head_commit: String,
    pub added: u32,
    pub removed: u32,
    pub files: u32,
    pub merge: Merge,
    pub decision: Option<Decision>,
    pub reviewers: Vec<Reviewer>,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    /// Failing first, then running, then the rest, each by name.
    pub checks: Vec<Check>,
    /// Checks beyond those asked for.
    pub more_checks: usize,
    /// Comments, reviews and review conversations, oldest first.
    pub entries: Vec<Entry>,
    /// Earlier ones that were not asked for.
    pub earlier: usize,
    pub allowed: Allowed,
}
impl Detail {
    /// The checks taken together, as a linked number shows them.
    pub fn checks(&self) -> Checks {
        let any = |outcome| self.checks.iter().any(|check| check.outcome == outcome);
        if any(Outcome::Failed) {
            Checks::Failing
        } else if any(Outcome::Running) {
            Checks::Pending
        } else if any(Outcome::Passed) {
            Checks::Passing
        } else {
            Checks::None
        }
    }
    /// Review conversations that are not resolved.
    pub fn unresolved(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| {
                matches!(
                    entry.kind,
                    Kind::Thread {
                        resolved: false,
                        ..
                    }
                )
            })
            .count()
    }
    pub fn in_review(&self) -> bool {
        matches!(self.state, State::Open | State::Draft)
    }
    /// The workflow runs that wait to be allowed to run, each once.
    pub fn awaiting_runs(&self) -> Vec<u64> {
        let mut runs: Vec<u64> = self
            .checks
            .iter()
            .filter(|check| check.outcome == Outcome::Awaiting)
            .filter_map(|check| {
                let after = check.url.split("/actions/runs/").nth(1)?;
                after.split('/').next()?.parse().ok()
            })
            .collect();
        runs.sort_unstable();
        runs.dedup();
        runs
    }
}

/// How a file differs in the pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Modified,
    Added,
    Deleted,
    Renamed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    /// The path a renamed file had.
    pub from: Option<String>,
    pub change: Change,
    pub added: u32,
    pub removed: u32,
    /// Its unified diff, which GitHub leaves out of a file that is not text
    /// or that changed too much.
    pub patch: Option<String>,
}

/// Seconds since 1970 of a time as GitHub writes it, `2026-10-07T03:14:21Z`.
pub fn timestamp(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    // Days from the civil date, counting years from March.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year.rem_euclid(400);
    let of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let days = era * 146_097 + of_era * 365 + of_era / 4 - of_era / 100 + of_year - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second)
}

/// The reactions to something, as the query asks for them.
const REACTIONS: &str = "reactionGroups{content viewerHasReacted reactors{totalCount}}";

/// The reactions someone gave, of those the host counts.
fn reactions(of: &serde_json::Value) -> Vec<Reaction> {
    of["reactionGroups"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|group| {
            let emoji = Emoji::ALL
                .into_iter()
                .find(|emoji| group["content"] == emoji.content())?;
            let count = group["reactors"]["totalCount"]
                .as_u64()?
                .min(u32::MAX as u64) as u32;
            (count > 0).then_some(Reaction {
                emoji,
                count,
                mine: group["viewerHasReacted"] == true,
            })
        })
        .collect()
}

fn method(name: Option<&str>) -> Option<Method> {
    Some(match name? {
        "MERGE" => Method::Merge,
        "SQUASH" => Method::Squash,
        "REBASE" => Method::Rebase,
        _ => return None,
    })
}

/// `PullRequest::parse` admits only letters, digits and `-_.` in the names
/// written into the query.
fn query(link: &PullRequest) -> String {
    let (_, owner, repository) = link.location();
    format!(
        "query{{repository(owner:\"{owner}\",name:\"{repository}\"){{\
         defaultBranchRef{{name}} viewerPermission mergeCommitAllowed squashMergeAllowed rebaseMergeAllowed autoMergeAllowed \
         pullRequest(number:{number}){{\
         id viewerCanUpdateBranch viewerCanReact autoMergeRequest{{mergeMethod}} {REACTIONS} \
         files(first:{MAX_FILES}){{nodes{{path viewerViewedState}}}} \
         title state isDraft body createdAt updatedAt mergedAt closedAt headRefOid \
         viewerCanUpdate viewerDidAuthor \
         author{{login}} mergedBy{{login}} \
         baseRefName headRefName isCrossRepository headRepositoryOwner{{login}} \
         additions deletions changedFiles mergeable mergeStateStatus reviewDecision \
         labels(first:20){{nodes{{name}}}} \
         assignees(first:10){{nodes{{login}}}} \
         reviewRequests(first:20){{nodes{{requestedReviewer{{__typename ...on User{{login}} ...on Team{{name}}}}}}}} \
         latestOpinionatedReviews(first:20){{nodes{{author{{login}} state}}}} \
         history:commits(last:{MAX_COMMITS}){{nodes{{commit{{oid abbreviatedOid messageHeadline committedDate \
         author{{name user{{login}}}}}}}}}} \
         commits(last:1){{totalCount nodes{{commit{{statusCheckRollup{{contexts(first:{MAX_CHECKS}){{totalCount nodes{{__typename \
         ...on CheckRun{{name status conclusion detailsUrl startedAt completedAt checkSuite{{workflowRun{{workflow{{name}}}}}}}} \
         ...on StatusContext{{context state targetUrl}}}}}}}}}}}}}} \
         comments(last:{MAX_ENTRIES}){{totalCount nodes{{id viewerCanUpdate {REACTIONS} author{{login}} body createdAt url isMinimized}}}} \
         reviews(last:{MAX_ENTRIES}){{totalCount nodes{{id viewerCanUpdate {REACTIONS} author{{login}} state body submittedAt url}}}} \
         reviewThreads(last:{MAX_ENTRIES}){{totalCount nodes{{id viewerCanResolve viewerCanUnresolve viewerCanReply isResolved isOutdated path line originalLine \
         comments(first:{MAX_REPLIES}){{nodes{{id viewerCanUpdate {REACTIONS} author{{login}} body createdAt url}}}}}}}}}}}}}}",
        number = link.number()
    )
}

/// What someone wrote, without what a page would not show of it: the
/// comments a template leaves for its author.
fn written(of: &serde_json::Value) -> String {
    let mut text = of.as_str().unwrap_or_default().replace("\r\n", "\n");
    while let Some(start) = text.find("<!--") {
        let end = text[start..]
            .find("-->")
            .map_or(text.len(), |end| start + end + 3);
        text.replace_range(start..end, "");
    }
    text.trim().to_owned()
}

/// The account that wrote something; one that was deleted has no name.
fn login(of: &serde_json::Value) -> String {
    of["login"].as_str().unwrap_or("ghost").to_owned()
}

fn verdict(state: &str) -> Option<Verdict> {
    Some(match state {
        "APPROVED" => Verdict::Approved,
        "CHANGES_REQUESTED" => Verdict::ChangesRequested,
        "COMMENTED" => Verdict::Commented,
        "DISMISSED" => Verdict::Dismissed,
        _ => return None,
    })
}

fn check(node: &serde_json::Value) -> Option<Check> {
    let text = |field: &str| node[field].as_str().unwrap_or_default().to_owned();
    if node["__typename"] == "StatusContext" {
        return Some(Check {
            name: text("context"),
            workflow: String::new(),
            outcome: match node["state"].as_str()? {
                "SUCCESS" => Outcome::Passed,
                "PENDING" | "EXPECTED" => Outcome::Running,
                _ => Outcome::Failed,
            },
            seconds: None,
            url: text("targetUrl"),
        });
    }
    let outcome = match (node["status"].as_str()?, node["conclusion"].as_str()) {
        ("COMPLETED", Some("SUCCESS")) => Outcome::Passed,
        ("COMPLETED", Some("ACTION_REQUIRED")) => Outcome::Awaiting,
        ("COMPLETED", Some("SKIPPED" | "NEUTRAL" | "STALE")) => Outcome::Skipped,
        ("COMPLETED", Some("CANCELLED")) => Outcome::Cancelled,
        ("COMPLETED", _) => Outcome::Failed,
        _ => Outcome::Running,
    };
    let time = |field: &str| node[field].as_str().and_then(timestamp);
    Some(Check {
        name: text("name"),
        workflow: node["checkSuite"]["workflowRun"]["workflow"]["name"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        outcome,
        seconds: time("startedAt")
            .zip(time("completedAt"))
            .and_then(|(started, completed)| u32::try_from(completed - started).ok()),
        url: text("detailsUrl"),
    })
}

/// What the response says of the pull request. A response without one says
/// why: it is not there, or nothing in it could be read.
fn parse(response: &[u8]) -> Result<Detail, Failure> {
    let response: serde_json::Value =
        serde_json::from_slice(response).map_err(|_| Failure::Unavailable)?;
    let repository = &response["data"]["repository"];
    let pull = &repository["pullRequest"];
    if !pull.is_object() {
        let missing = response["errors"]
            .as_array()
            .is_some_and(|errors| errors.iter().any(|error| error["type"] == "NOT_FOUND"));
        return Err(if missing {
            Failure::NotFound
        } else {
            Failure::Unavailable
        });
    }
    let text = |field: &str| pull[field].as_str().unwrap_or_default().to_owned();
    let count = |of: &serde_json::Value| of.as_u64().unwrap_or(0).min(u32::MAX as u64) as u32;
    let nodes = |field: &str| pull[field]["nodes"].as_array().into_iter().flatten();
    let state = match (pull["state"].as_str(), pull["isDraft"].as_bool()) {
        (Some("OPEN"), Some(false)) => State::Open,
        (Some("OPEN"), Some(true)) => State::Draft,
        (Some("MERGED"), _) => State::Merged,
        (Some("CLOSED"), _) => State::Closed,
        _ => return Err(Failure::Unavailable),
    };
    let merge = match (
        pull["mergeable"].as_str().unwrap_or_default(),
        pull["mergeStateStatus"].as_str().unwrap_or_default(),
    ) {
        ("CONFLICTING", _) | (_, "DIRTY") => Merge::Conflicts,
        (_, "BEHIND") => Merge::Behind,
        (_, "BLOCKED") => Merge::Blocked,
        (_, "UNSTABLE") => Merge::Unstable,
        (_, "CLEAN" | "HAS_HOOKS") => Merge::Ready,
        _ => Merge::Unknown,
    };
    let head = if pull["isCrossRepository"] == true {
        format!(
            "{}:{}",
            login(&pull["headRepositoryOwner"]),
            text("headRefName")
        )
    } else {
        text("headRefName")
    };
    // Whoever reviewed, then whoever is still asked to.
    let mut reviewers: Vec<Reviewer> = nodes("latestOpinionatedReviews")
        .filter_map(|review| {
            Some(Reviewer {
                name: login(&review["author"]),
                verdict: verdict(review["state"].as_str()?)?,
            })
        })
        .collect();
    for request in nodes("reviewRequests") {
        let asked = &request["requestedReviewer"];
        let name = asked["login"].as_str().or(asked["name"].as_str());
        if let Some(name) = name
            && !reviewers.iter().any(|reviewer| reviewer.name == name)
        {
            reviewers.push(Reviewer {
                name: name.to_owned(),
                verdict: Verdict::Awaited,
            });
        }
    }
    let commits = &pull["commits"];
    let contexts = &commits["nodes"][0]["commit"]["statusCheckRollup"]["contexts"];
    let mut checks: Vec<Check> = contexts["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(check)
        .collect();
    checks
        .sort_by(|a, b| (a.outcome, &a.workflow, &a.name).cmp(&(b.outcome, &b.workflow, &b.name)));
    let more_checks = (count(&contexts["totalCount"]) as usize).saturating_sub(checks.len());

    let mut entries = Vec::new();
    let mut earlier = 0;
    let mut listed = |field: &str, shown: usize| {
        earlier += (count(&pull[field]["totalCount"]) as usize).saturating_sub(shown);
    };
    let comments: Vec<_> = nodes("comments").collect();
    listed("comments", comments.len());
    for comment in comments {
        if comment["isMinimized"] == true {
            continue;
        }
        entries.push(Entry {
            id: comment["id"].as_str().unwrap_or_default().to_owned(),
            reactions: reactions(comment),
            can_edit: comment["viewerCanUpdate"] == true,
            author: login(&comment["author"]),
            at: comment["createdAt"]
                .as_str()
                .and_then(timestamp)
                .unwrap_or(0),
            kind: Kind::Comment,
            body: written(&comment["body"]),
            url: comment["url"].as_str().unwrap_or_default().to_owned(),
        });
    }
    let reviews: Vec<_> = nodes("reviews").collect();
    listed("reviews", reviews.len());
    for review in reviews {
        let Some(verdict) = review["state"].as_str().and_then(verdict) else {
            // One that is still being written is its author's alone.
            continue;
        };
        let body = written(&review["body"]);
        // A review that only carries comments on lines is its conversations.
        if verdict == Verdict::Commented && body.is_empty() {
            continue;
        }
        entries.push(Entry {
            id: review["id"].as_str().unwrap_or_default().to_owned(),
            reactions: reactions(review),
            can_edit: review["viewerCanUpdate"] == true,
            author: login(&review["author"]),
            at: review["submittedAt"]
                .as_str()
                .and_then(timestamp)
                .unwrap_or(0),
            kind: Kind::Review(verdict),
            body,
            url: review["url"].as_str().unwrap_or_default().to_owned(),
        });
    }
    let threads: Vec<_> = nodes("reviewThreads").collect();
    listed("reviewThreads", threads.len());
    for thread in threads {
        let mut said = thread["comments"]["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|comment| {
                (
                    Reply {
                        author: login(&comment["author"]),
                        at: comment["createdAt"]
                            .as_str()
                            .and_then(timestamp)
                            .unwrap_or(0),
                        body: written(&comment["body"]),
                    },
                    comment["url"].as_str().unwrap_or_default().to_owned(),
                )
            });
        let Some((first, url)) = said.next() else {
            continue;
        };
        let opening = &thread["comments"]["nodes"][0];
        entries.push(Entry {
            id: opening["id"].as_str().unwrap_or_default().to_owned(),
            reactions: reactions(opening),
            can_edit: opening["viewerCanUpdate"] == true,
            author: first.author,
            at: first.at,
            kind: Kind::Thread {
                id: thread["id"].as_str().unwrap_or_default().to_owned(),
                can_reply: thread["viewerCanReply"] == true,
                can_resolve: thread["viewerCanResolve"] == true
                    || thread["viewerCanUnresolve"] == true,
                path: thread["path"].as_str().unwrap_or_default().to_owned(),
                line: thread["line"]
                    .as_u64()
                    .or(thread["originalLine"].as_u64())
                    .map(|line| line.min(u32::MAX as u64) as u32),
                resolved: thread["isResolved"] == true,
                outdated: thread["isOutdated"] == true,
                replies: said.map(|(reply, _)| reply).collect(),
            },
            body: first.body,
            url,
        });
    }
    entries.sort_by_key(|entry| entry.at);

    let history = nodes("history")
        .map(|node| {
            let commit = &node["commit"];
            let text = |field: &str| commit[field].as_str().unwrap_or_default().to_owned();
            Commit {
                oid: text("oid"),
                short: text("abbreviatedOid"),
                headline: text("messageHeadline"),
                author: commit["author"]["user"]["login"]
                    .as_str()
                    .or(commit["author"]["name"].as_str())
                    .unwrap_or_default()
                    .to_owned(),
                at: commit["committedDate"]
                    .as_str()
                    .and_then(timestamp)
                    .unwrap_or(0),
            }
        })
        .collect();
    let writes = matches!(
        repository["viewerPermission"].as_str(),
        Some("ADMIN" | "MAINTAIN" | "WRITE")
    );
    let allowed = Allowed {
        update: pull["viewerCanUpdate"] == true,
        merge: writes,
        judge: pull["viewerDidAuthor"] == false,
        methods: [
            ("mergeCommitAllowed", Method::Merge),
            ("squashMergeAllowed", Method::Squash),
            ("rebaseMergeAllowed", Method::Rebase),
        ]
        .into_iter()
        .filter(|(field, _)| repository[*field] == true)
        .map(|(_, method)| method)
        .collect(),
        auto_merge: writes && repository["autoMergeAllowed"] == true,
        update_branch: pull["viewerCanUpdateBranch"] == true,
        edit: pull["viewerCanUpdate"] == true,
        react: pull["viewerCanReact"] == true,
        label: matches!(
            repository["viewerPermission"].as_str(),
            Some("ADMIN" | "MAINTAIN" | "WRITE" | "TRIAGE")
        ),
        request: writes,
    };

    Ok(Detail {
        id: text("id"),
        auto_merge: method(pull["autoMergeRequest"]["mergeMethod"].as_str()),
        viewed: nodes("files")
            .filter(|file| file["viewerViewedState"] == "VIEWED")
            .filter_map(|file| file["path"].as_str().map(str::to_owned))
            .collect(),
        reactions: reactions(pull),
        stacked: repository["defaultBranchRef"]["name"]
            .as_str()
            .is_some_and(|main| main != pull["baseRefName"]),
        stack: Vec::new(),
        title: text("title"),
        state,
        author: login(&pull["author"]),
        body: written(&pull["body"]),
        base: text("baseRefName"),
        head,
        opened: pull["createdAt"].as_str().and_then(timestamp).unwrap_or(0),
        updated: pull["updatedAt"].as_str().and_then(timestamp).unwrap_or(0),
        ended: pull["mergedAt"]
            .as_str()
            .or(pull["closedAt"].as_str())
            .and_then(timestamp),
        merged_by: pull["mergedBy"]["login"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        commits: count(&commits["totalCount"]),
        history,
        head_commit: text("headRefOid"),
        added: count(&pull["additions"]),
        removed: count(&pull["deletions"]),
        files: count(&pull["changedFiles"]),
        merge,
        decision: match pull["reviewDecision"].as_str() {
            Some("APPROVED") => Some(Decision::Approved),
            Some("CHANGES_REQUESTED") => Some(Decision::ChangesRequested),
            Some("REVIEW_REQUIRED") => Some(Decision::ReviewRequired),
            _ => None,
        },
        reviewers,
        labels: nodes("labels")
            .filter_map(|label| label["name"].as_str().map(str::to_owned))
            .collect(),
        assignees: nodes("assignees").map(login).collect(),
        checks,
        more_checks,
        entries,
        earlier,
        allowed,
    })
}

fn parse_files(response: &[u8]) -> Result<Vec<ChangedFile>, Failure> {
    let response: serde_json::Value =
        serde_json::from_slice(response).map_err(|_| Failure::Unavailable)?;
    let files = response.as_array().ok_or(Failure::Unavailable)?;
    Ok(files
        .iter()
        .filter_map(|file| {
            let count = |field: &str| file[field].as_u64().unwrap_or(0).min(u32::MAX as u64) as u32;
            Some(ChangedFile {
                path: file["filename"].as_str()?.to_owned(),
                from: file["previous_filename"].as_str().map(str::to_owned),
                change: match file["status"].as_str()? {
                    "added" | "copied" => Change::Added,
                    "removed" => Change::Deleted,
                    "renamed" => Change::Renamed,
                    _ => Change::Modified,
                },
                added: count("additions"),
                removed: count("deletions"),
                patch: file["patch"].as_str().map(str::to_owned),
            })
        })
        .collect())
}

/// Why the GitHub CLI answered nothing usable: it has no sign-in for the
/// host, or something else is wrong.
fn unanswered(host: &str) -> Failure {
    match cli(&["auth", "status", "--hostname", host], 1024) {
        Err(_) => Failure::NoCli,
        Ok(status) if !status.ok => Failure::SignedOut,
        Ok(_) => Failure::Unavailable,
    }
}

/// Reads the pull request. One bounded request, made by the GitHub CLI as the
/// account signed in to it.
pub fn read(link: &PullRequest) -> Result<Detail, Failure> {
    let (host, ..) = link.location();
    let query = format!("query={}", query(link));
    let printed = cli(
        &["api", "graphql", "--hostname", host, "-f", &query],
        MAX_RESPONSE,
    )
    .map_err(|_| Failure::NoCli)?;
    match parse(&printed.bytes) {
        // The CLI prints what GitHub answered even where it reports a failure.
        Err(Failure::Unavailable) if !printed.ok => Err(unanswered(host)),
        Ok(mut detail) => {
            detail.stack = read_stack(link, &detail);
            Ok(detail)
        }
        read => read,
    }
}

/// The most pull requests followed in each direction of a stack.
const MAX_STACK: usize = 5;

/// The open pull requests of the stack this one is in, from the one nearest
/// the main branch to the last stacked on top, without itself: those it is
/// stacked on, when it does not merge into the main branch, and those
/// stacked on it. Nothing where they could not be read. Branch names are
/// passed as values of their own, never as part of the request's text.
fn read_stack(link: &PullRequest, detail: &Detail) -> Vec<Neighbour> {
    if !detail.in_review() {
        return Vec::new();
    }
    let (host, owner, repository) = link.location();
    let query = format!(
        "query=query($base:String!,$head:String!){{repository(owner:\"{owner}\",name:\"{repository}\"){{\
         defaultBranchRef{{name}} \
         below:pullRequests(headRefName:$base,states:OPEN,first:1){{nodes{{number title baseRefName headRefName}}}} \
         above:pullRequests(baseRefName:$head,states:OPEN,first:5){{nodes{{number title baseRefName headRefName}}}}}}}}"
    );
    let around = |base: &str, head: &str| -> Option<serde_json::Value> {
        let printed = cli(
            &[
                "api",
                "graphql",
                "--hostname",
                host,
                "-f",
                &query,
                "-f",
                &format!("base={base}"),
                "-f",
                &format!("head={head}"),
            ],
            MAX_RESPONSE,
        )
        .ok()?;
        serde_json::from_slice(&printed.bytes).ok()
    };
    let own = link.number();
    let head = detail
        .head
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .to_owned();
    let Some(first) = around(&detail.base, &head) else {
        return Vec::new();
    };
    let mut stack = parse_stack(&first.to_string().into_bytes(), detail.stacked, own);
    let branches = |response: &serde_json::Value, field: &str| -> Option<(String, String)> {
        let node = &response["data"]["repository"][field]["nodes"][0];
        Some((
            node["baseRefName"].as_str()?.to_owned(),
            node["headRefName"].as_str()?.to_owned(),
        ))
    };
    let main = first["data"]["repository"]["defaultBranchRef"]["name"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    // Further down, towards the main branch.
    let mut below = branches(&first, "below").filter(|_| detail.stacked);
    for _ in 1..MAX_STACK {
        let Some((base, _)) = below.take().filter(|(base, _)| *base != main) else {
            break;
        };
        let Some(next) = around(&base, "") else { break };
        let found = parse_stack(&next.to_string().into_bytes(), true, own);
        let Some(near) = found.into_iter().find(|near| near.below) else {
            break;
        };
        if stack.iter().any(|known| known.number == near.number) {
            break;
        }
        stack.insert(0, near);
        below = branches(&next, "below");
    }
    // Further up, along the first pull request stacked on each.
    let mut above = branches(&first, "above");
    for _ in 1..MAX_STACK {
        let Some((_, head)) = above.take() else { break };
        let Some(next) = around("", &head) else { break };
        let found = parse_stack(&next.to_string().into_bytes(), false, own);
        let Some(near) = found.into_iter().find(|near| !near.below) else {
            break;
        };
        if stack.iter().any(|known| known.number == near.number) {
            break;
        }
        stack.push(near);
        above = branches(&next, "above");
    }
    stack
}

fn parse_stack(response: &[u8], stacked: bool, own: u64) -> Vec<Neighbour> {
    let response: serde_json::Value = serde_json::from_slice(response).unwrap_or_default();
    let repository = &response["data"]["repository"];
    let neighbours = |field: &str, below: bool| {
        repository[field]["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(move |node| {
                Some(Neighbour {
                    number: node["number"].as_u64().filter(|number| *number != own)?,
                    title: node["title"].as_str()?.to_owned(),
                    below,
                })
            })
            .collect::<Vec<_>>()
    };
    let mut stack = if stacked {
        neighbours("below", true)
    } else {
        Vec::new()
    };
    stack.extend(neighbours("above", false));
    stack
}

/// Reads the files the pull request changes, with their diffs: the first
/// `MAX_FILES` of them. With a `commit`, the files that one commit changes.
pub fn read_files(link: &PullRequest, commit: Option<&str>) -> Result<Vec<ChangedFile>, Failure> {
    let (host, owner, repository) = link.location();
    // A commit is named by the host: only its hexadecimal name is passed on.
    let commit = commit.filter(|oid| oid.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let path = match commit {
        Some(oid) => format!("repos/{owner}/{repository}/commits/{oid}?per_page={MAX_FILES}"),
        None => format!(
            "repos/{owner}/{repository}/pulls/{}/files?per_page={MAX_FILES}",
            link.number()
        ),
    };
    let printed =
        cli(&["api", "--hostname", host, &path], MAX_FILES_RESPONSE).map_err(|_| Failure::NoCli)?;
    if !printed.ok {
        return Err(unanswered(host));
    }
    if commit.is_none() {
        return parse_files(&printed.bytes);
    }
    // A commit lists its files inside what it says of itself.
    let response: serde_json::Value =
        serde_json::from_slice(&printed.bytes).map_err(|_| Failure::Unavailable)?;
    parse_files(response["files"].to_string().as_bytes())
}

/// What a pull request of the repository can be given: its labels and the
/// people who can be asked to review.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Choices {
    pub labels: Vec<String>,
    pub people: Vec<String>,
}

/// Reads the labels and the people of the pull request's repository, the
/// first hundred of each.
pub fn read_choices(link: &PullRequest) -> Result<Choices, Failure> {
    let (host, owner, repository) = link.location();
    let named = |what: &str, field: &str| -> Result<Vec<String>, Failure> {
        let path = format!("repos/{owner}/{repository}/{what}?per_page=100");
        let printed =
            cli(&["api", "--hostname", host, &path], MAX_RESPONSE).map_err(|_| Failure::NoCli)?;
        if !printed.ok {
            return Err(unanswered(host));
        }
        let response: serde_json::Value =
            serde_json::from_slice(&printed.bytes).map_err(|_| Failure::Unavailable)?;
        Ok(response
            .as_array()
            .ok_or(Failure::Unavailable)?
            .iter()
            .filter_map(|item| item[field].as_str().map(str::to_owned))
            .collect())
    };
    Ok(Choices {
        labels: named("labels", "name")?,
        people: named("assignees", "login")?,
    })
}

/// A comment on a line of a changed file, sent with a review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineComment {
    pub path: String,
    /// The line as the pull request leaves it, or as it was for one removed.
    pub line: u32,
    pub removed: bool,
    pub body: String,
}

/// Something the person asks of the pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    Merge(Method),
    /// Have the host merge it by itself once it may be, or no longer.
    AutoMerge(Option<Method>),
    /// Bring its branch up to date with the one it merges into.
    UpdateBranch,
    /// Open a pull request that takes back what this one merged.
    Revert,
    Ready,
    Draft,
    Close,
    Reopen,
    Title(String),
    Description(String),
    Comment(String),
    /// A review: approval, a request for changes or a comment, with what it
    /// says and what it says of single lines.
    Review(Verdict, String, Vec<LineComment>),
    /// An answer in a review conversation.
    Reply {
        thread: String,
        body: String,
    },
    Resolve {
        thread: String,
        resolved: bool,
    },
    React {
        subject: String,
        emoji: Emoji,
        on: bool,
    },
    Label {
        name: String,
        on: bool,
    },
    /// Ask someone to review, or no longer.
    Request {
        name: String,
        on: bool,
    },
    /// Mark a changed file as viewed, or no longer.
    Viewed {
        path: String,
        on: bool,
    },
    /// Rewrite something the person said: a comment, a review, or a
    /// comment on a line.
    Edit {
        subject: String,
        said: Said,
        body: String,
    },
    /// Let the workflow runs that wait for it run.
    ApproveRuns(Vec<u64>),
}

/// What kind of thing was said, which the host rewrites each its own way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Said {
    Comment,
    Review,
    Line,
}

/// A name as part of an address.
fn encoded(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            byte => format!("%{byte:02X}"),
        })
        .collect()
}

impl Act {
    /// What to say when it was done, and when it could not be.
    pub fn outcome(&self) -> (&'static str, &'static str) {
        match self {
            Self::Merge(_) => ("Pull request merged", "Could not merge this pull request"),
            Self::AutoMerge(Some(_)) => ("Auto-merge turned on", "Could not turn on auto-merge"),
            Self::AutoMerge(None) => ("Auto-merge turned off", "Could not turn off auto-merge"),
            Self::UpdateBranch => ("Branch updated", "Could not update the branch"),
            Self::Revert => (
                "Revert pull request opened",
                "Could not open a pull request that reverts this one",
            ),
            Self::Ready => (
                "Marked ready for review",
                "Could not mark this ready for review",
            ),
            Self::Draft => ("Converted to draft", "Could not convert this to a draft"),
            Self::Close => ("Pull request closed", "Could not close this pull request"),
            Self::Reopen => (
                "Pull request reopened",
                "Could not reopen this pull request",
            ),
            Self::Title(_) => ("Title saved", "Could not save the title"),
            Self::Description(_) => ("Description saved", "Could not save the description"),
            Self::Comment(_) => ("Comment posted", "Could not post the comment"),
            Self::Review(Verdict::Approved, ..) => {
                ("Pull request approved", "Could not submit the review")
            }
            Self::Review(Verdict::ChangesRequested, ..) => {
                ("Changes requested", "Could not submit the review")
            }
            Self::Review(..) => ("Review submitted", "Could not submit the review"),
            Self::Reply { .. } => ("Reply posted", "Could not post the reply"),
            Self::Resolve { resolved: true, .. } => (
                "Conversation resolved",
                "Could not resolve the conversation",
            ),
            Self::Resolve { .. } => ("Conversation reopened", "Could not reopen the conversation"),
            Self::React { .. } => ("Reaction saved", "Could not save the reaction"),
            Self::Label { .. } => ("Labels changed", "Could not change the labels"),
            Self::Request { .. } => ("Reviewers changed", "Could not change the reviewers"),
            Self::Viewed { .. } => ("File marked", "Could not mark the file"),
            Self::Edit { .. } => ("Comment saved", "Could not save the comment"),
            Self::ApproveRuns(_) => ("Workflows approved", "Could not approve the workflows"),
        }
    }

    /// Whether the field it was written in is put away once it is done.
    pub fn writes(&self) -> bool {
        matches!(
            self,
            Self::Comment(_) | Self::Review(..) | Self::Reply { .. } | Self::Edit { .. }
        )
    }

    /// The GitHub CLI command that does it. `id` is the host's name for the
    /// pull request. What the person wrote is passed as a value of its own,
    /// never as part of a request's text.
    fn command(&self, link: &PullRequest, id: &str) -> Vec<String> {
        let (host, owner, repository) = link.location();
        let url = link.url();
        let words = |words: &[&str]| words.iter().map(|word| (*word).to_owned()).collect();
        let rest = |verb: &str, path: String, fields: &[(&str, &str)]| {
            let mut command: Vec<String> = words(&["api", "--hostname", host, "-X", verb]);
            command.push(format!("repos/{owner}/{repository}/{path}"));
            for (flag, field) in fields {
                command.extend([(*flag).to_owned(), (*field).to_owned()]);
            }
            command
        };
        let mutation = |text: &str, values: &[(&str, &str)]| {
            let mut command: Vec<String> = words(&["api", "graphql", "--hostname", host, "-f"]);
            command.push(format!("query=mutation{text}"));
            for (name, value) in values {
                command.extend(["-f".to_owned(), format!("{name}={value}")]);
            }
            command
        };
        let number = link.number();
        match self {
            Self::Merge(method) => words(&["pr", "merge", url, method.flag()]),
            Self::AutoMerge(Some(method)) => words(&["pr", "merge", url, "--auto", method.flag()]),
            Self::AutoMerge(None) => words(&["pr", "merge", url, "--disable-auto"]),
            Self::UpdateBranch => rest("PUT", format!("pulls/{number}/update-branch"), &[]),
            Self::Revert => mutation(
                "($id:ID!){revertPullRequest(input:{pullRequestId:$id}){clientMutationId}}",
                &[("id", id)],
            ),
            Self::Ready => words(&["pr", "ready", url]),
            Self::Draft => words(&["pr", "ready", url, "--undo"]),
            Self::Close => words(&["pr", "close", url]),
            Self::Reopen => words(&["pr", "reopen", url]),
            Self::Title(title) => rest(
                "PATCH",
                format!("pulls/{number}"),
                &[("-f", &format!("title={title}"))],
            ),
            Self::Description(body) => rest(
                "PATCH",
                format!("pulls/{number}"),
                &[("-f", &format!("body={body}"))],
            ),
            Self::Comment(body) => words(&["pr", "comment", url, "--body", body]),
            Self::Review(verdict, body, lines) if lines.is_empty() => {
                let mut command: Vec<String> = words(&[
                    "pr",
                    "review",
                    url,
                    match verdict {
                        Verdict::Approved => "--approve",
                        Verdict::ChangesRequested => "--request-changes",
                        _ => "--comment",
                    },
                ]);
                if !body.is_empty() {
                    command.extend(["--body".to_owned(), body.clone()]);
                }
                command
            }
            // One request carries the review and what it says of each line.
            Self::Review(verdict, body, lines) => {
                let event = match verdict {
                    Verdict::Approved => "APPROVE",
                    Verdict::ChangesRequested => "REQUEST_CHANGES",
                    _ => "COMMENT",
                };
                let mut command = rest(
                    "POST",
                    format!("pulls/{number}/reviews"),
                    &[("-f", &format!("event={event}"))],
                );
                if !body.is_empty() {
                    command.extend(["-f".to_owned(), format!("body={body}")]);
                }
                for line in lines {
                    let side = if line.removed { "LEFT" } else { "RIGHT" };
                    command.extend([
                        "-f".to_owned(),
                        format!("comments[][path]={}", line.path),
                        "-F".to_owned(),
                        format!("comments[][line]={}", line.line),
                        "-f".to_owned(),
                        format!("comments[][side]={side}"),
                        "-f".to_owned(),
                        format!("comments[][body]={}", line.body),
                    ]);
                }
                command
            }
            Self::Reply { thread, body } => mutation(
                "($id:ID!,$body:String!){addPullRequestReviewThreadReply(input:{pullRequestReviewThreadId:$id,body:$body}){clientMutationId}}",
                &[("id", thread), ("body", body)],
            ),
            Self::Resolve { thread, resolved } => mutation(
                if *resolved {
                    "($id:ID!){resolveReviewThread(input:{threadId:$id}){clientMutationId}}"
                } else {
                    "($id:ID!){unresolveReviewThread(input:{threadId:$id}){clientMutationId}}"
                },
                &[("id", thread)],
            ),
            Self::React { subject, emoji, on } => mutation(
                &format!(
                    "($id:ID!){{{}(input:{{subjectId:$id,content:{}}}){{clientMutationId}}}}",
                    if *on { "addReaction" } else { "removeReaction" },
                    emoji.content()
                ),
                &[("id", subject)],
            ),
            Self::Label { name, on: true } => rest(
                "POST",
                format!("issues/{number}/labels"),
                &[("-f", &format!("labels[]={name}"))],
            ),
            Self::Label { name, on: false } => rest(
                "DELETE",
                format!("issues/{number}/labels/{}", encoded(name)),
                &[],
            ),
            Self::Request { name, on } => rest(
                if *on { "POST" } else { "DELETE" },
                format!("pulls/{number}/requested_reviewers"),
                &[("-f", &format!("reviewers[]={name}"))],
            ),
            Self::Viewed { path, on } => mutation(
                if *on {
                    "($id:ID!,$path:String!){markFileAsViewed(input:{pullRequestId:$id,path:$path}){clientMutationId}}"
                } else {
                    "($id:ID!,$path:String!){unmarkFileAsViewed(input:{pullRequestId:$id,path:$path}){clientMutationId}}"
                },
                &[("id", id), ("path", path)],
            ),
            Self::Edit {
                subject,
                said,
                body,
            } => mutation(
                match said {
                    Said::Comment => {
                        "($id:ID!,$body:String!){updateIssueComment(input:{id:$id,body:$body}){clientMutationId}}"
                    }
                    Said::Review => {
                        "($id:ID!,$body:String!){updatePullRequestReview(input:{pullRequestReviewId:$id,body:$body}){clientMutationId}}"
                    }
                    Said::Line => {
                        "($id:ID!,$body:String!){updatePullRequestReviewComment(input:{pullRequestReviewCommentId:$id,body:$body}){clientMutationId}}"
                    }
                },
                &[("id", subject), ("body", body)],
            ),
            // One request for each run: see `commands`.
            Self::ApproveRuns(runs) => rest(
                "POST",
                format!(
                    "actions/runs/{}/approve",
                    runs.first().copied().unwrap_or_default()
                ),
                &[],
            ),
        }
    }

    /// Every command it takes, in order.
    fn commands(&self, link: &PullRequest, id: &str) -> Vec<Vec<String>> {
        match self {
            Self::ApproveRuns(runs) => runs
                .iter()
                .map(|run| Self::ApproveRuns(vec![*run]).command(link, id))
                .collect(),
            act => vec![act.command(link, id)],
        }
    }
}

/// What the host said of a command that failed, as one short line.
fn complaint(said: &[u8]) -> String {
    let said = String::from_utf8_lossy(said);
    let line = said
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    // The CLI leads a failure with a mark of its own.
    let line = ["X ", "✗ ", "GraphQL: ", "gh: "]
        .into_iter()
        .fold(line, |line, mark| line.strip_prefix(mark).unwrap_or(line));
    let mut short: String = line.chars().take(280).collect();
    if short.len() < line.len() {
        short.push('…');
    }
    short
}

/// Does what was asked, as the account signed in to the GitHub CLI. On
/// failure, what the host said of it; nothing when it said nothing.
/// `id` is the host's name for the pull request, as it was read.
pub fn act(link: &PullRequest, id: &str, act: &Act) -> Result<(), String> {
    for command in act.commands(link, id) {
        let command: Vec<&str> = command.iter().map(String::as_str).collect();
        match cli_complaint(&command) {
            Ok(done) if done.ok => {}
            Ok(done) => return Err(complaint(&done.bytes)),
            Err(_) => return Err("The GitHub CLI (gh) could not be started.".into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_read_as_github_writes_them() {
        assert_eq!(timestamp("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(timestamp("2026-10-07T03:14:21Z"), Some(1_791_342_861));
        assert_eq!(timestamp("2024-02-29T23:59:59Z"), Some(1_709_251_199));
        assert_eq!(timestamp("2026-13-07T03:14:21Z"), None);
        assert_eq!(timestamp("yesterday"), None);
    }

    #[test]
    fn the_request_names_one_pull_request_and_bounds_what_it_asks_for() {
        let link = PullRequest::parse("https://github.com/zevem/re.po_2/pull/83").unwrap();
        let query = query(&link);
        assert!(
            query.starts_with(
                "query{repository(owner:\"zevem\",name:\"re.po_2\"){defaultBranchRef{name} viewerPermission "
            )
        );
        assert!(query.contains("pullRequest(number:83){id "));
        assert!(query.contains("comments(last:50)") && query.contains("contexts(first:100)"));
        assert_eq!(query.matches('{').count(), query.matches('}').count());
    }

    const RESPONSE: &str = r#"{"data":{"repository":{
        "defaultBranchRef":{"name":"main"},"viewerPermission":"WRITE","mergeCommitAllowed":false,"squashMergeAllowed":true,"rebaseMergeAllowed":true,
        "autoMergeAllowed":true,
        "pullRequest":{"id":"PR_1","viewerCanUpdateBranch":true,"viewerCanReact":true,
        "autoMergeRequest":{"mergeMethod":"SQUASH"},
        "reactionGroups":[{"content":"ROCKET","viewerHasReacted":true,"reactors":{"totalCount":2}},
            {"content":"EYES","viewerHasReacted":false,"reactors":{"totalCount":0}}],
        "files":{"nodes":[{"path":"src/a.rs","viewerViewedState":"VIEWED"},{"path":"src/b.rs","viewerViewedState":"UNVIEWED"}]},
        "title":"feat(ui): a tab for pull requests","state":"OPEN","isDraft":false,
        "body":"<!-- Say what it does. -->\r\nWhat it does.\r\n","createdAt":"2026-10-07T03:14:21Z","updatedAt":"2026-10-07T06:00:00Z",
        "mergedAt":null,"closedAt":null,"headRefOid":"8b45e821b3937e7a512707df6e40d0e1b7e9e7ba",
        "viewerCanUpdate":true,"viewerDidAuthor":false,
        "history":{"nodes":[
            {"commit":{"oid":"1a2b3c4d5e","abbreviatedOid":"1a2b3c4","messageHeadline":"Add the tab","committedDate":"2026-10-07T03:00:00Z","author":{"name":"Ada L","user":{"login":"ada"}}}},
            {"commit":{"abbreviatedOid":"8b45e82","messageHeadline":"Fit five tabs","committedDate":"2026-10-07T03:10:00Z","author":{"name":"Robot","user":null}}}]},
        "author":{"login":"ada"},"mergedBy":null,
        "baseRefName":"main","headRefName":"feat/tab","isCrossRepository":true,
        "headRepositoryOwner":{"login":"fork"},
        "additions":120,"deletions":7,"changedFiles":4,
        "mergeable":"MERGEABLE","mergeStateStatus":"BLOCKED","reviewDecision":"CHANGES_REQUESTED",
        "labels":{"nodes":[{"name":"ui"}]},
        "assignees":{"nodes":[{"login":"ada"}]},
        "reviewRequests":{"nodes":[
            {"requestedReviewer":{"__typename":"User","login":"grace"}},
            {"requestedReviewer":{"__typename":"Team","name":"desktop"}},
            {"requestedReviewer":{"__typename":"User","login":"linus"}}]},
        "latestOpinionatedReviews":{"nodes":[{"author":{"login":"linus"},"state":"CHANGES_REQUESTED"}]},
        "commits":{"totalCount":3,"nodes":[{"commit":{"statusCheckRollup":{"contexts":{"totalCount":104,"nodes":[
            {"__typename":"CheckRun","name":"Test","status":"COMPLETED","conclusion":"SUCCESS",
             "detailsUrl":"https://github.com/zevem/neptune/actions/runs/1",
             "startedAt":"2026-10-07T03:19:15Z","completedAt":"2026-10-07T03:20:45Z",
             "checkSuite":{"workflowRun":{"workflow":{"name":"CI"}}}},
            {"__typename":"CheckRun","name":"Lint","status":"IN_PROGRESS","conclusion":null,
             "detailsUrl":"","startedAt":"2026-10-07T03:19:15Z","completedAt":null,"checkSuite":null},
            {"__typename":"CheckRun","name":"Build","status":"COMPLETED","conclusion":"FAILURE",
             "detailsUrl":"","startedAt":null,"completedAt":null,"checkSuite":null},
            {"__typename":"CheckRun","name":"Fork","status":"COMPLETED","conclusion":"ACTION_REQUIRED",
             "detailsUrl":"https://github.com/zevem/neptune/actions/runs/77/job/5","startedAt":null,"completedAt":null,"checkSuite":null},
            {"__typename":"CheckRun","name":"Docs","status":"COMPLETED","conclusion":"SKIPPED",
             "detailsUrl":"","startedAt":null,"completedAt":null,"checkSuite":null},
            {"__typename":"StatusContext","context":"deploy/preview","state":"SUCCESS","targetUrl":"https://preview.example"}
        ]}}}}]},
        "comments":{"totalCount":52,"nodes":[
            {"author":{"login":"grace"},"body":"Looks close.","createdAt":"2026-10-07T05:00:00Z","url":"https://github.com/c/1","isMinimized":false},
            {"author":null,"body":"spam","createdAt":"2026-10-07T05:30:00Z","url":"","isMinimized":true}]},
        "reviews":{"totalCount":3,"nodes":[
            {"author":{"login":"linus"},"state":"CHANGES_REQUESTED","body":"Not yet.","submittedAt":"2026-10-07T06:00:00Z","url":"https://github.com/r/1"},
            {"author":{"login":"linus"},"state":"COMMENTED","body":"","submittedAt":"2026-10-07T04:00:00Z","url":""},
            {"author":{"login":"grace"},"state":"PENDING","body":"draft","submittedAt":null,"url":""}]},
        "reviewThreads":{"totalCount":1,"nodes":[
            {"id":"T_1","viewerCanResolve":true,"viewerCanUnresolve":false,"viewerCanReply":true,
             "isResolved":false,"isOutdated":true,"path":"src/ui/panel.rs","line":null,"originalLine":42,
             "comments":{"nodes":[
                {"id":"C_1","reactionGroups":[{"content":"THUMBS_UP","viewerHasReacted":false,"reactors":{"totalCount":1}}],
                 "author":{"login":"linus"},"body":"Why five?","createdAt":"2026-10-07T04:00:00Z","url":"https://github.com/t/1"},
                {"author":{"login":"ada"},"body":"One more tab.","createdAt":"2026-10-07T04:10:00Z","url":"https://github.com/t/2"}]}}]}
    }}}}"#;

    #[test]
    fn a_response_reduces_to_what_the_tab_shows() {
        let detail = parse(RESPONSE.as_bytes()).unwrap();
        assert_eq!(detail.title, "feat(ui): a tab for pull requests");
        assert_eq!(
            detail.body, "What it does.",
            "a template's comment is not shown"
        );
        assert_eq!((detail.state, detail.author.as_str()), (State::Open, "ada"));
        assert_eq!(
            (detail.base.as_str(), detail.head.as_str()),
            ("main", "fork:feat/tab")
        );
        assert_eq!(
            (detail.commits, detail.added, detail.removed, detail.files),
            (3, 120, 7, 4)
        );
        assert_eq!(detail.merge, Merge::Blocked);
        assert_eq!(detail.decision, Some(Decision::ChangesRequested));
        // A reviewer asked again is listed once, by what they last said.
        let reviewers: Vec<_> = detail
            .reviewers
            .iter()
            .map(|reviewer| (reviewer.name.as_str(), reviewer.verdict))
            .collect();
        assert_eq!(
            reviewers,
            [
                ("linus", Verdict::ChangesRequested),
                ("grace", Verdict::Awaited),
                ("desktop", Verdict::Awaited),
            ]
        );
        assert_eq!(
            (&detail.labels[..], &detail.assignees[..]),
            (&["ui".to_owned()][..], &["ada".to_owned()][..])
        );
        // What needs a person leads the checks.
        let checks: Vec<_> = detail
            .checks
            .iter()
            .map(|check| (check.name.as_str(), check.outcome, check.seconds))
            .collect();
        assert_eq!(
            checks,
            [
                ("Build", Outcome::Failed, None),
                ("Fork", Outcome::Awaiting, None),
                ("Lint", Outcome::Running, None),
                ("deploy/preview", Outcome::Passed, None),
                ("Test", Outcome::Passed, Some(90)),
                ("Docs", Outcome::Skipped, None),
            ]
        );
        assert_eq!(detail.checks[4].workflow, "CI");
        assert_eq!(detail.more_checks, 98);
        assert_eq!(detail.awaiting_runs(), [77]);
        assert!(!detail.stacked, "it merges into the main branch");
        assert_eq!(detail.checks(), Checks::Failing);
        // Oldest first; a hidden comment, a review still being written and one
        // that only carries its line comments are left out.
        let said: Vec<_> = detail
            .entries
            .iter()
            .map(|entry| (entry.author.as_str(), entry.body.as_str()))
            .collect();
        assert_eq!(
            said,
            [
                ("linus", "Why five?"),
                ("grace", "Looks close."),
                ("linus", "Not yet.")
            ]
        );
        assert_eq!(
            detail.entries[0].kind,
            Kind::Thread {
                id: "T_1".into(),
                can_reply: true,
                can_resolve: true,
                path: "src/ui/panel.rs".into(),
                line: Some(42),
                resolved: false,
                outdated: true,
                replies: vec![Reply {
                    author: "ada".into(),
                    at: 1_791_346_200,
                    body: "One more tab.".into(),
                }],
            }
        );
        assert_eq!(
            detail.entries[2].kind,
            Kind::Review(Verdict::ChangesRequested)
        );
        assert_eq!(detail.unresolved(), 1);
        assert_eq!(detail.earlier, 50);
        let history: Vec<_> = detail
            .history
            .iter()
            .map(|commit| (commit.short.as_str(), commit.author.as_str()))
            .collect();
        assert_eq!(history, [("1a2b3c4", "ada"), ("8b45e82", "Robot")]);
        assert!(detail.head_commit.starts_with("8b45e82"));
        assert_eq!(
            detail.allowed,
            Allowed {
                update: true,
                merge: true,
                judge: true,
                methods: vec![Method::Squash, Method::Rebase],
                auto_merge: true,
                update_branch: true,
                edit: true,
                react: true,
                label: true,
                request: true,
            }
        );
        // What the host names is kept for what is asked of it later.
        assert_eq!(
            (detail.id.as_str(), detail.auto_merge),
            ("PR_1", Some(Method::Squash))
        );
        assert_eq!(detail.viewed, ["src/a.rs"]);
        assert_eq!(
            detail.reactions,
            [Reaction {
                emoji: Emoji::Rocket,
                count: 2,
                mine: true
            }]
        );
        assert_eq!(detail.entries[0].id, "C_1");
        assert_eq!(detail.entries[0].reactions[0].emoji, Emoji::ThumbsUp);
        assert_eq!(detail.history[0].oid, "1a2b3c4d5e");
    }

    #[test]
    fn a_response_without_a_pull_request_says_why() {
        let missing = br#"{"data":{"repository":{"pullRequest":null}},
            "errors":[{"type":"NOT_FOUND","path":["repository","pullRequest"]}]}"#;
        assert_eq!(parse(missing), Err(Failure::NotFound));
        assert_eq!(parse(b"gh: not signed in"), Err(Failure::Unavailable));
        assert_eq!(parse(br#"{"data":null}"#), Err(Failure::Unavailable));
        let merged = RESPONSE
            .replace(r#""state":"OPEN""#, r#""state":"MERGED""#)
            .replace(r#""mergedAt":null"#, r#""mergedAt":"2026-10-08T00:00:00Z""#)
            .replace(r#""mergedBy":null"#, r#""mergedBy":{"login":"grace"}"#);
        let merged = parse(merged.as_bytes()).unwrap();
        assert_eq!(
            (merged.state, merged.merged_by.as_str(), merged.in_review()),
            (State::Merged, "grace", false)
        );
        assert_eq!(merged.ended, timestamp("2026-10-08T00:00:00Z"));
    }

    #[test]
    fn what_is_asked_becomes_one_command_and_a_failure_one_line() {
        let link = PullRequest::parse("https://github.com/zevem/neptune/pull/83").unwrap();
        let url = link.url();
        let command = |act: Act| act.command(&link, "PR_1");
        assert_eq!(
            command(Act::Merge(Method::Squash)),
            ["pr", "merge", url, "--squash"]
        );
        assert_eq!(
            command(Act::AutoMerge(Some(Method::Rebase))),
            ["pr", "merge", url, "--auto", "--rebase"]
        );
        assert_eq!(command(Act::Draft), ["pr", "ready", url, "--undo"]);
        assert_eq!(
            command(Act::Comment("Thanks".into())),
            ["pr", "comment", url, "--body", "Thanks"]
        );
        // An approval needs no words; what is written goes with it.
        assert_eq!(
            command(Act::Review(Verdict::Approved, String::new(), Vec::new())),
            ["pr", "review", url, "--approve"]
        );
        assert_eq!(
            command(Act::Review(
                Verdict::ChangesRequested,
                "Not yet".into(),
                Vec::new()
            )),
            [
                "pr",
                "review",
                url,
                "--request-changes",
                "--body",
                "Not yet"
            ]
        );
        // What is said of single lines goes in the same request.
        let lines = vec![LineComment {
            path: "src/a.rs".into(),
            line: 7,
            removed: true,
            body: "Why?".into(),
        }];
        assert_eq!(
            command(Act::Review(Verdict::Commented, String::new(), lines))[2..],
            [
                "github.com",
                "-X",
                "POST",
                "repos/zevem/neptune/pulls/83/reviews",
                "-f",
                "event=COMMENT",
                "-f",
                "comments[][path]=src/a.rs",
                "-F",
                "comments[][line]=7",
                "-f",
                "comments[][side]=LEFT",
                "-f",
                "comments[][body]=Why?",
            ]
        );
        // What the person wrote is a value of its own, whatever it holds.
        let reply = command(Act::Reply {
            thread: "T_1".into(),
            body: "\"){evil}".into(),
        });
        assert!(
            reply[5].starts_with(
                "query=mutation($id:ID!,$body:String!){addPullRequestReviewThreadReply("
            )
        );
        assert_eq!(reply[6..], ["-f", "id=T_1", "-f", "body=\"){evil}"]);
        let react = command(Act::React {
            subject: "C_1".into(),
            emoji: Emoji::Rocket,
            on: false,
        });
        assert!(react[5].contains("removeReaction(input:{subjectId:$id,content:ROCKET})"));
        assert_eq!(
            command(Act::Label {
                name: "needs review/ui".into(),
                on: false
            })[3..],
            [
                "-X",
                "DELETE",
                "repos/zevem/neptune/issues/83/labels/needs%20review%2Fui"
            ]
        );
        assert_eq!(
            command(Act::Request {
                name: "grace".into(),
                on: true
            })[3..],
            [
                "-X",
                "POST",
                "repos/zevem/neptune/pulls/83/requested_reviewers",
                "-f",
                "reviewers[]=grace"
            ]
        );
        assert_eq!(
            command(Act::Title("A = b".into()))[3..],
            [
                "-X",
                "PATCH",
                "repos/zevem/neptune/pulls/83",
                "-f",
                "title=A = b"
            ]
        );
        let viewed = command(Act::Viewed {
            path: "src/a.rs".into(),
            on: true,
        });
        assert!(viewed[5].contains("markFileAsViewed"));
        assert_eq!(viewed[6..], ["-f", "id=PR_1", "-f", "path=src/a.rs"]);
        assert_eq!(
            complaint(b"\nGraphQL: Pull request is not mergeable (mergePullRequest)\nmore\n"),
            "Pull request is not mergeable (mergePullRequest)"
        );
        let edit = command(Act::Edit {
            subject: "C_1".into(),
            said: Said::Line,
            body: "Better".into(),
        });
        assert!(edit[5].contains("updatePullRequestReviewComment"));
        assert_eq!(edit[6..], ["-f", "id=C_1", "-f", "body=Better"]);
        // Each waiting run is allowed by a request of its own.
        let runs = Act::ApproveRuns(vec![77, 78]).commands(&link, "PR_1");
        assert_eq!(runs.len(), 2);
        assert_eq!(
            runs[1][3..],
            ["-X", "POST", "repos/zevem/neptune/actions/runs/78/approve"]
        );
        let stack = br#"{"data":{"repository":{
            "below":{"nodes":[{"number":82,"title":"Base work"}]},
            "above":{"nodes":[{"number":83,"title":"Itself"},{"number":84,"title":"Next"}]}}}}"#;
        let numbers = |stacked| -> Vec<(u64, bool)> {
            parse_stack(stack, stacked, 83)
                .iter()
                .map(|near| (near.number, near.below))
                .collect()
        };
        assert_eq!(numbers(true), [(82, true), (84, false)]);
        // One that merges into the main branch is stacked on nothing.
        assert_eq!(numbers(false), [(84, false)]);
        assert_eq!(
            complaint(b"X Pull request zevem/neptune#117 is closed.\n"),
            "Pull request zevem/neptune#117 is closed."
        );
        assert_eq!(complaint(b""), "");
    }

    /// The real GitHub CLI against a real pull request named by
    /// `NEPTUNE_PR_LIVE`: it is read, one of its files is marked as viewed
    /// and unmarked again, which only the signed-in account sees, and a
    /// change the host must refuse is refused in the host's words.
    #[test]
    #[ignore = "Uses the signed-in GitHub CLI; needs NEPTUNE_PR_LIVE"]
    fn the_github_cli_reads_marks_and_reports_a_refusal() {
        let url = std::env::var("NEPTUNE_PR_LIVE").expect("Name a merged pull request");
        let link = PullRequest::parse(&url).unwrap();
        let detail = read(&link).unwrap();
        assert!(!detail.title.is_empty() && !detail.id.is_empty());
        let files = read_files(&link, None).unwrap();
        let path = files[0].path.clone();
        let was = detail.viewed.contains(&path);
        let mark = |on| {
            act(
                &link,
                &detail.id,
                &Act::Viewed {
                    path: path.clone(),
                    on,
                },
            )
        };
        assert_eq!(mark(!was), Ok(()));
        assert_eq!(read(&link).unwrap().viewed.contains(&path), !was);
        assert_eq!(mark(was), Ok(()));
        assert_eq!(read(&link).unwrap().viewed.contains(&path), was);
        // One commit's files and the repository's lists are read as well.
        let commit = &detail.history.last().unwrap().oid;
        assert!(!read_files(&link, Some(commit)).unwrap().is_empty());
        let choices = read_choices(&link).unwrap();
        assert!(!choices.people.is_empty());
        // A merged pull request cannot be made a draft: the host says so.
        if detail.state == State::Merged {
            let refused = act(&link, &detail.id, &Act::Draft).unwrap_err();
            println!("refused: {refused}");
            assert!(!refused.is_empty());
        }
    }

    /// Everything the tab can ask of a pull request, done for real through
    /// the signed-in GitHub CLI and read back. `NEPTUNE_PR_SANDBOX` names a
    /// throwaway open pull request that changes `main.rs` and may be merged,
    /// and `NEPTUNE_PR_SANDBOX_OTHER` another one that is closed and
    /// reopened; their repository has a label `sandbox-label`. What a host
    /// does not offer a repository (drafts, auto-merge) is reported, not
    /// required.
    #[test]
    #[ignore = "Changes real pull requests; needs NEPTUNE_PR_SANDBOX and NEPTUNE_PR_SANDBOX_OTHER"]
    fn every_change_the_tab_asks_for_reaches_the_host() {
        let link = PullRequest::parse(&std::env::var("NEPTUNE_PR_SANDBOX").unwrap()).unwrap();
        let other =
            PullRequest::parse(&std::env::var("NEPTUNE_PR_SANDBOX_OTHER").unwrap()).unwrap();
        let id = read(&link).unwrap().id;
        let ask = |what: Act| {
            let done = act(&link, &id, &what);
            println!("{:<28} {done:?}", what.outcome().0);
            done
        };
        let now = || read(&link).unwrap();

        ask(Act::Comment("From the tab".into())).unwrap();
        let said = now()
            .entries
            .into_iter()
            .find(|entry| entry.body == "From the tab");
        let said = said.expect("the comment is on the pull request");
        assert!(said.can_edit);
        let react = |on| Act::React {
            subject: said.id.clone(),
            emoji: Emoji::Rocket,
            on,
        };
        ask(react(true)).unwrap();
        let mine = |detail: &Detail| {
            detail
                .entries
                .iter()
                .find(|entry| entry.id == said.id)
                .is_some_and(|entry| entry.reactions.iter().any(|given| given.mine))
        };
        assert!(mine(&now()));
        ask(react(false)).unwrap();
        assert!(!mine(&now()));
        ask(Act::Edit {
            subject: said.id.clone(),
            said: Said::Comment,
            body: "Rewritten from the tab".into(),
        })
        .unwrap();
        assert!(
            now()
                .entries
                .iter()
                .any(|entry| entry.body == "Rewritten from the tab")
        );

        let label = |on| Act::Label {
            name: "sandbox-label".into(),
            on,
        };
        ask(label(true)).unwrap();
        assert_eq!(now().labels, ["sandbox-label"]);
        ask(label(false)).unwrap();
        assert!(now().labels.is_empty());

        ask(Act::Title("Say more, as the tab put it".into())).unwrap();
        ask(Act::Description("A body the tab wrote.".into())).unwrap();
        let edited = now();
        assert_eq!(
            (edited.title.as_str(), edited.body.as_str()),
            ("Say more, as the tab put it", "A body the tab wrote.")
        );

        // A review that says something of one line opens a conversation.
        let line = LineComment {
            path: "main.rs".into(),
            line: 2,
            removed: false,
            body: "Why two?".into(),
        };
        ask(Act::Review(
            Verdict::Commented,
            "A review from the tab".into(),
            vec![line],
        ))
        .unwrap();
        let thread = |detail: &Detail| {
            detail.entries.iter().find_map(|entry| match &entry.kind {
                Kind::Thread {
                    id,
                    resolved,
                    replies,
                    path,
                    ..
                } if path == "main.rs" => {
                    Some((id.clone(), *resolved, replies.len(), entry.id.clone()))
                }
                _ => None,
            })
        };
        let (conversation, _, _, opening) = thread(&now()).expect("a conversation on main.rs");
        ask(Act::Reply {
            thread: conversation.clone(),
            body: "Because.".into(),
        })
        .unwrap();
        let resolve = |resolved| Act::Resolve {
            thread: conversation.clone(),
            resolved,
        };
        ask(resolve(true)).unwrap();
        assert_eq!(
            thread(&now()).map(|found| (found.1, found.2)),
            Some((true, 1))
        );
        ask(resolve(false)).unwrap();
        assert_eq!(thread(&now()).map(|found| found.1), Some(false));
        ask(Act::Edit {
            subject: opening,
            said: Said::Line,
            body: "Why two, though?".into(),
        })
        .unwrap();
        assert!(
            now()
                .entries
                .iter()
                .any(|entry| entry.body == "Why two, though?")
        );

        let viewed = |on| Act::Viewed {
            path: "main.rs".into(),
            on,
        };
        ask(viewed(true)).unwrap();
        assert_eq!(now().viewed, ["main.rs"]);
        ask(viewed(false)).unwrap();

        // What this repository may not have is said in the host's words.
        if ask(Act::Draft).is_ok() {
            assert_eq!(now().state, State::Draft);
            ask(Act::Ready).unwrap();
        }
        if ask(Act::AutoMerge(Some(Method::Squash))).is_ok() {
            let _ = ask(Act::AutoMerge(None));
        }
        let _ = ask(Act::Request {
            name: now().author,
            on: true,
        });

        // The other one is closed and reopened, then this one merged, which
        // leaves the other behind and gives something to revert.
        let other_id = read(&other).unwrap().id;
        act(&other, &other_id, &Act::Close).unwrap();
        assert_eq!(read(&other).unwrap().state, State::Closed);
        act(&other, &other_id, &Act::Reopen).unwrap();
        assert_eq!(read(&other).unwrap().state, State::Open);
        ask(Act::Merge(Method::Squash)).unwrap();
        assert_eq!(now().state, State::Merged);
        println!(
            "{:<28} {:?}",
            "Branch updated",
            act(&other, &other_id, &Act::UpdateBranch)
        );
        ask(Act::Revert).unwrap();
    }

    #[test]
    fn changed_files_carry_their_counts_and_the_diff_github_gives() {
        let response = br#"[
            {"filename":"src/a.rs","status":"modified","additions":3,"deletions":1,"patch":"@@ -1 +1,3 @@\n-a\n+b"},
            {"filename":"new.rs","previous_filename":"old.rs","status":"renamed","additions":0,"deletions":0},
            {"filename":"logo.png","status":"added","additions":0,"deletions":0},
            {"filename":"gone.rs","status":"removed","additions":0,"deletions":9,"patch":"@@ -1,9 +0,0 @@"}
        ]"#;
        let files = parse_files(response).unwrap();
        let listed: Vec<_> = files
            .iter()
            .map(|file| (file.path.as_str(), file.change, file.patch.is_some()))
            .collect();
        assert_eq!(
            listed,
            [
                ("src/a.rs", Change::Modified, true),
                ("new.rs", Change::Renamed, false),
                ("logo.png", Change::Added, false),
                ("gone.rs", Change::Deleted, true),
            ]
        );
        assert_eq!(files[1].from.as_deref(), Some("old.rs"));
        assert_eq!((files[0].added, files[0].removed), (3, 1));
        assert_eq!(
            parse_files(br#"{"message":"Not Found"}"#),
            Err(Failure::Unavailable)
        );
    }
}
