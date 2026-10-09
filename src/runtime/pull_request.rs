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
    /// The first seven characters of its name.
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
}

/// One thing said on the pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub author: String,
    pub at: i64,
    pub kind: Kind,
    pub body: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detail {
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

/// `PullRequest::parse` admits only letters, digits and `-_.` in the names
/// written into the query.
fn query(link: &PullRequest) -> String {
    let (_, owner, repository) = link.location();
    format!(
        "query{{repository(owner:\"{owner}\",name:\"{repository}\"){{\
         viewerPermission mergeCommitAllowed squashMergeAllowed rebaseMergeAllowed \
         pullRequest(number:{number}){{\
         title state isDraft body createdAt updatedAt mergedAt closedAt headRefOid \
         viewerCanUpdate viewerDidAuthor \
         author{{login}} mergedBy{{login}} \
         baseRefName headRefName isCrossRepository headRepositoryOwner{{login}} \
         additions deletions changedFiles mergeable mergeStateStatus reviewDecision \
         labels(first:20){{nodes{{name}}}} \
         assignees(first:10){{nodes{{login}}}} \
         reviewRequests(first:20){{nodes{{requestedReviewer{{__typename ...on User{{login}} ...on Team{{name}}}}}}}} \
         latestOpinionatedReviews(first:20){{nodes{{author{{login}} state}}}} \
         history:commits(last:{MAX_COMMITS}){{nodes{{commit{{abbreviatedOid messageHeadline committedDate \
         author{{name user{{login}}}}}}}}}} \
         commits(last:1){{totalCount nodes{{commit{{statusCheckRollup{{contexts(first:{MAX_CHECKS}){{totalCount nodes{{__typename \
         ...on CheckRun{{name status conclusion detailsUrl startedAt completedAt checkSuite{{workflowRun{{workflow{{name}}}}}}}} \
         ...on StatusContext{{context state targetUrl}}}}}}}}}}}}}} \
         comments(last:{MAX_ENTRIES}){{totalCount nodes{{author{{login}} body createdAt url isMinimized}}}} \
         reviews(last:{MAX_ENTRIES}){{totalCount nodes{{author{{login}} state body submittedAt url}}}} \
         reviewThreads(last:{MAX_ENTRIES}){{totalCount nodes{{isResolved isOutdated path line originalLine \
         comments(first:{MAX_REPLIES}){{nodes{{author{{login}} body createdAt url}}}}}}}}}}}}}}",
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
        entries.push(Entry {
            author: first.author,
            at: first.at,
            kind: Kind::Thread {
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
    };

    Ok(Detail {
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
        read => read,
    }
}

/// Reads the files the pull request changes, with their diffs: the first
/// `MAX_FILES` of them.
pub fn read_files(link: &PullRequest) -> Result<Vec<ChangedFile>, Failure> {
    let (host, owner, repository) = link.location();
    let path = format!(
        "repos/{owner}/{repository}/pulls/{}/files?per_page={MAX_FILES}",
        link.number()
    );
    let printed =
        cli(&["api", "--hostname", host, &path], MAX_FILES_RESPONSE).map_err(|_| Failure::NoCli)?;
    if !printed.ok {
        return Err(unanswered(host));
    }
    parse_files(&printed.bytes)
}

/// Something the person asks of the pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    Merge(Method),
    Ready,
    Draft,
    Close,
    Reopen,
    Comment(String),
    /// A review: approval, a request for changes or a comment, with what it
    /// says.
    Review(Verdict, String),
}
impl Act {
    /// What to say when it was done, and when it could not be.
    pub fn outcome(&self) -> (&'static str, &'static str) {
        match self {
            Self::Merge(_) => ("Pull request merged", "Could not merge this pull request"),
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
            Self::Comment(_) => ("Comment posted", "Could not post the comment"),
            Self::Review(Verdict::Approved, _) => {
                ("Pull request approved", "Could not submit the review")
            }
            Self::Review(Verdict::ChangesRequested, _) => {
                ("Changes requested", "Could not submit the review")
            }
            Self::Review(..) => ("Review submitted", "Could not submit the review"),
        }
    }

    /// The GitHub CLI command that does it.
    fn command<'a>(&'a self, url: &'a str) -> Vec<&'a str> {
        match self {
            Self::Merge(method) => vec!["pr", "merge", url, method.flag()],
            Self::Ready => vec!["pr", "ready", url],
            Self::Draft => vec!["pr", "ready", url, "--undo"],
            Self::Close => vec!["pr", "close", url],
            Self::Reopen => vec!["pr", "reopen", url],
            Self::Comment(body) => vec!["pr", "comment", url, "--body", body],
            Self::Review(verdict, body) => {
                let mut command = vec![
                    "pr",
                    "review",
                    url,
                    match verdict {
                        Verdict::Approved => "--approve",
                        Verdict::ChangesRequested => "--request-changes",
                        _ => "--comment",
                    },
                ];
                if !body.is_empty() {
                    command.extend(["--body", body]);
                }
                command
            }
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
    let line = line.strip_prefix("GraphQL: ").unwrap_or(line);
    let mut short: String = line.chars().take(280).collect();
    if short.len() < line.len() {
        short.push('…');
    }
    short
}

/// Does what was asked, as the account signed in to the GitHub CLI. On
/// failure, what the host said of it; nothing when it said nothing.
pub fn act(link: &PullRequest, act: &Act) -> Result<(), String> {
    match cli_complaint(&act.command(link.url())) {
        Ok(done) if done.ok => Ok(()),
        Ok(done) => Err(complaint(&done.bytes)),
        Err(_) => Err("The GitHub CLI (gh) could not be started.".into()),
    }
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
                "query{repository(owner:\"zevem\",name:\"re.po_2\"){viewerPermission "
            )
        );
        assert!(query.contains("pullRequest(number:83){title "));
        assert!(query.contains("comments(last:50)") && query.contains("contexts(first:100)"));
        assert_eq!(query.matches('{').count(), query.matches('}').count());
    }

    const RESPONSE: &str = r#"{"data":{"repository":{
        "viewerPermission":"WRITE","mergeCommitAllowed":false,"squashMergeAllowed":true,"rebaseMergeAllowed":true,
        "pullRequest":{
        "title":"feat(ui): a tab for pull requests","state":"OPEN","isDraft":false,
        "body":"<!-- Say what it does. -->\r\nWhat it does.\r\n","createdAt":"2026-10-07T03:14:21Z","updatedAt":"2026-10-07T06:00:00Z",
        "mergedAt":null,"closedAt":null,"headRefOid":"8b45e821b3937e7a512707df6e40d0e1b7e9e7ba",
        "viewerCanUpdate":true,"viewerDidAuthor":false,
        "history":{"nodes":[
            {"commit":{"abbreviatedOid":"1a2b3c4","messageHeadline":"Add the tab","committedDate":"2026-10-07T03:00:00Z","author":{"name":"Ada L","user":{"login":"ada"}}}},
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
            {"isResolved":false,"isOutdated":true,"path":"src/ui/panel.rs","line":null,"originalLine":42,
             "comments":{"nodes":[
                {"author":{"login":"linus"},"body":"Why five?","createdAt":"2026-10-07T04:00:00Z","url":"https://github.com/t/1"},
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
                ("Lint", Outcome::Running, None),
                ("deploy/preview", Outcome::Passed, None),
                ("Test", Outcome::Passed, Some(90)),
                ("Docs", Outcome::Skipped, None),
            ]
        );
        assert_eq!(detail.checks[3].workflow, "CI");
        assert_eq!(detail.more_checks, 99);
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
            }
        );
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
        let url = "https://github.com/zevem/neptune/pull/83";
        assert_eq!(
            Act::Merge(Method::Squash).command(url),
            ["pr", "merge", url, "--squash"]
        );
        assert_eq!(Act::Draft.command(url), ["pr", "ready", url, "--undo"]);
        assert_eq!(
            Act::Comment("Thanks".into()).command(url),
            ["pr", "comment", url, "--body", "Thanks"]
        );
        // An approval needs no words; what is written goes with it.
        assert_eq!(
            Act::Review(Verdict::Approved, String::new()).command(url),
            ["pr", "review", url, "--approve"]
        );
        assert_eq!(
            Act::Review(Verdict::ChangesRequested, "Not yet".into()).command(url),
            [
                "pr",
                "review",
                url,
                "--request-changes",
                "--body",
                "Not yet"
            ]
        );
        assert_eq!(
            complaint(b"\nGraphQL: Pull request is not mergeable (mergePullRequest)\nmore\n"),
            "Pull request is not mergeable (mergePullRequest)"
        );
        assert_eq!(complaint(b""), "");
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
