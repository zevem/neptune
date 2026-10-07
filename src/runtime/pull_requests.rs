//! The live state of linked pull requests, read through the person's own
//! GitHub CLI. Neptune holds no credentials and saves none of what it reads.
//!
//! One worker owns the schedule and runs one bounded lookup at a time. It
//! sleeps between refreshes and wakes the application only when a state
//! changed, so linked pull requests cost no frames while nothing happens.
use eframe::egui;
use neptune_model::PullRequest;
use std::{
    collections::HashMap,
    io::Read,
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

/// The most pull requests looked up in one round.
pub const MAX_WATCHED: usize = 32;
const MAX_RESPONSE: u64 = 256 * 1024;
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(20);
/// A state no longer shown is kept this long, so returning to its workspace
/// shows the last one while it is read again.
const REMEMBERED: Duration = Duration::from_secs(60 * 60);
/// The most review conversations counted; the chip says "99+" beyond 99.
const MAX_THREADS: u8 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Open,
    Draft,
    Merged,
    Closed,
}

/// The checks of the pull request's last commit, taken together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Checks {
    /// The commit has no checks.
    None,
    Passing,
    Pending,
    Failing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    pub checks: Checks,
    /// Review conversations that are not resolved.
    pub unresolved: u8,
}
impl Status {
    /// Checks and review conversations matter until a pull request is merged
    /// or closed.
    pub fn in_review(&self) -> bool {
        matches!(self.state, State::Open | State::Draft)
    }
    pub fn checks(&self) -> Checks {
        if self.in_review() {
            self.checks
        } else {
            Checks::None
        }
    }
    pub fn unresolved(&self) -> u8 {
        if self.in_review() { self.unresolved } else { 0 }
    }
    /// The state in words: "Open · Checks failing · 2 unresolved comments".
    pub fn describe(&self) -> String {
        let mut words = vec![
            match self.state {
                State::Open => "Open",
                State::Draft => "Draft",
                State::Merged => "Merged",
                State::Closed => "Closed",
            }
            .to_owned(),
        ];
        match self.checks() {
            Checks::None => {}
            Checks::Passing => words.push("Checks passing".into()),
            Checks::Pending => words.push("Checks running".into()),
            Checks::Failing => words.push("Checks failing".into()),
        }
        match self.unresolved() {
            0 => {}
            1 => words.push("1 unresolved comment".into()),
            count => words.push(format!("{} unresolved comments", unresolved_label(count))),
        }
        words.join(" · ")
    }
    fn refresh_after(&self) -> Duration {
        Duration::from_secs(match (self.in_review(), self.checks) {
            (true, Checks::Pending) => 15,
            (true, _) => 60,
            // A merged or closed pull request rarely changes again.
            (false, _) => 10 * 60,
        })
    }
}

/// A count of unresolved conversations as a chip shows it.
pub fn unresolved_label(count: u8) -> String {
    if count >= MAX_THREADS {
        "99+".into()
    } else {
        count.to_string()
    }
}

/// What is known of one linked pull request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Lookup {
    /// Not read yet.
    #[default]
    Checking,
    /// The last lookup failed: no signed-in GitHub CLI, no network, or a host
    /// or pull request it cannot read.
    Unavailable,
    Known(Status),
}
impl Lookup {
    pub fn status(self) -> Option<Status> {
        match self {
            Self::Known(status) => Some(status),
            _ => None,
        }
    }
    fn refresh_after(self) -> Duration {
        self.status()
            .map_or(Duration::from_secs(60), |status| status.refresh_after())
    }
}

/// Reads the pull requests of one host; `None` for each it could not read.
pub(crate) type Fetch = Box<dyn Fn(&str, &[PullRequest]) -> Vec<Option<Status>> + Send>;

#[derive(Default)]
struct Watched {
    wanted: Vec<PullRequest>,
    known: HashMap<String, (Lookup, Instant)>,
    closed: bool,
}
#[derive(Default)]
struct Shared {
    watched: Mutex<Watched>,
    changed: Condvar,
}
impl Shared {
    fn lock(&self) -> MutexGuard<'_, Watched> {
        self.watched.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn key(link: &PullRequest) -> String {
    link.url().to_ascii_lowercase()
}

pub struct Watcher {
    shared: Arc<Shared>,
    /// Taken by the worker, which starts with the first link.
    fetch: Option<Fetch>,
}
impl Default for Watcher {
    fn default() -> Self {
        // Tests of the application do not run the person's GitHub CLI.
        if cfg!(test) {
            Self::with(Box::new(|_, links| vec![None; links.len()]))
        } else {
            Self::with(Box::new(fetch))
        }
    }
}
impl Watcher {
    /// A watcher that reads through `fetch` in place of the GitHub CLI.
    pub(crate) fn with(fetch: Fetch) -> Self {
        Self {
            shared: Arc::default(),
            fetch: Some(fetch),
        }
    }

    /// Name the pull requests in view. Their states are read at once when
    /// new and again as they age; `wake` is asked for a frame when one changes.
    pub fn watch(&mut self, links: impl Iterator<Item = PullRequest>, wake: &egui::Context) {
        let mut wanted: Vec<PullRequest> = Vec::new();
        for link in links {
            if wanted.len() < MAX_WATCHED && !wanted.iter().any(|known| known.same(&link)) {
                wanted.push(link);
            }
        }
        let mut watched = self.shared.lock();
        if watched.wanted == wanted {
            return;
        }
        watched.wanted = wanted;
        let idle = watched.wanted.is_empty();
        drop(watched);
        self.shared.changed.notify_all();
        if !idle && let Some(fetch) = self.fetch.take() {
            let (shared, wake) = (self.shared.clone(), wake.clone());
            let started = std::thread::Builder::new()
                .name("neptune-pull-requests".into())
                .spawn(move || run(&shared, &fetch, &wake));
            if let Err(error) = started {
                eprintln!("Could not watch pull requests: {error}");
            }
        }
    }

    pub fn lookup(&self, link: &PullRequest) -> Lookup {
        self.shared
            .lock()
            .known
            .get(&key(link))
            .map_or(Lookup::Checking, |known| known.0)
    }
}
impl Drop for Watcher {
    fn drop(&mut self) {
        self.shared.lock().closed = true;
        self.shared.changed.notify_all();
    }
}

fn run(shared: &Shared, fetch: &Fetch, wake: &egui::Context) {
    let mut watched = shared.lock();
    while !watched.closed {
        let now = Instant::now();
        let Watched { wanted, known, .. } = &mut *watched;
        known.retain(|url, (_, read)| {
            now.duration_since(*read) < REMEMBERED || wanted.iter().any(|link| key(link) == *url)
        });
        let mut due = Vec::new();
        let mut next: Option<Duration> = None;
        for link in wanted.iter() {
            let wait = known
                .get(&key(link))
                .map_or(Duration::ZERO, |(lookup, read)| {
                    lookup
                        .refresh_after()
                        .saturating_sub(now.duration_since(*read))
                });
            if wait.is_zero() {
                due.push(link.clone());
            } else {
                next = Some(next.map_or(wait, |next| next.min(wait)));
            }
        }
        if due.is_empty() {
            watched = match next {
                Some(wait) => {
                    shared
                        .changed
                        .wait_timeout(watched, wait)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
                None => shared
                    .changed
                    .wait(watched)
                    .unwrap_or_else(PoisonError::into_inner),
            };
            continue;
        }
        drop(watched);
        let read = read_all(fetch, &due);
        watched = shared.lock();
        let now = Instant::now();
        let mut changed = false;
        for (link, lookup) in due.iter().zip(read) {
            let before = watched.known.insert(key(link), (lookup, now));
            changed |= before.map(|before| before.0) != Some(lookup);
        }
        if changed {
            wake.request_repaint();
        }
    }
}

/// One lookup for each host, in turn.
fn read_all(fetch: &Fetch, links: &[PullRequest]) -> Vec<Lookup> {
    let mut read = vec![Lookup::Unavailable; links.len()];
    let mut hosts: Vec<&str> = links.iter().map(|link| link.location().0).collect();
    hosts.sort_unstable();
    hosts.dedup();
    for host in hosts {
        let (places, of_host): (Vec<usize>, Vec<PullRequest>) = links
            .iter()
            .enumerate()
            .filter(|(_, link)| link.location().0 == host)
            .map(|(place, link)| (place, link.clone()))
            .unzip();
        for (place, status) in places.into_iter().zip(fetch(host, &of_host)) {
            read[place] = status.map_or(Lookup::Unavailable, Lookup::Known);
        }
    }
    read
}

fn fetch(host: &str, links: &[PullRequest]) -> Vec<Option<Status>> {
    match gh(host, &query(links)) {
        Some(response) => parse(&response, links.len()),
        None => vec![None; links.len()],
    }
}

/// One aliased field for each pull request. `PullRequest::parse` admits only
/// letters, digits and `-_.` in the names written into the query.
fn query(links: &[PullRequest]) -> String {
    let mut query = String::from("query{");
    for (place, link) in links.iter().enumerate() {
        let (_, owner, repository) = link.location();
        query.push_str(&format!(
            "p{place}:repository(owner:\"{owner}\",name:\"{repository}\"){{\
             pullRequest(number:{}){{state isDraft \
             commits(last:1){{nodes{{commit{{statusCheckRollup{{state}}}}}}}} \
             reviewThreads(first:{MAX_THREADS}){{nodes{{isResolved}}}}}}}}",
            link.number()
        ));
    }
    query.push('}');
    query
}

/// A pull request that is missing, or in a repository that cannot be read, is
/// `None`; the others of the same response are still read.
fn parse(response: &[u8], count: usize) -> Vec<Option<Status>> {
    let response: serde_json::Value = serde_json::from_slice(response).unwrap_or_default();
    (0..count)
        .map(|place| {
            let pull = &response["data"][format!("p{place}")]["pullRequest"];
            let state = match (pull["state"].as_str()?, pull["isDraft"].as_bool()?) {
                ("OPEN", false) => State::Open,
                ("OPEN", true) => State::Draft,
                ("MERGED", _) => State::Merged,
                ("CLOSED", _) => State::Closed,
                _ => return None,
            };
            let rollup = &pull["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["state"];
            let checks = match rollup.as_str() {
                None => Checks::None,
                Some("SUCCESS") => Checks::Passing,
                Some("PENDING" | "EXPECTED") => Checks::Pending,
                Some("FAILURE" | "ERROR") => Checks::Failing,
                Some(_) => return None,
            };
            let unresolved = pull["reviewThreads"]["nodes"]
                .as_array()?
                .iter()
                .filter(|thread| thread["isResolved"] == false)
                .count()
                .min(MAX_THREADS as usize) as u8;
            Some(Status {
                state,
                checks,
                unresolved,
            })
        })
        .collect()
}

/// A desktop launch may not have the person's shell `PATH`.
fn program() -> std::path::PathBuf {
    let on_path = std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|folder| folder.join("gh").is_file()));
    if !on_path {
        for installed in [
            "/opt/homebrew/bin/gh",
            "/usr/local/bin/gh",
            "/home/linuxbrew/.linuxbrew/bin/gh",
        ] {
            if std::path::Path::new(installed).is_file() {
                return installed.into();
            }
        }
    }
    "gh".into()
}

/// The response of one GraphQL request made by the GitHub CLI, which prints
/// what it could read even when part of the request failed.
fn gh(host: &str, query: &str) -> Option<Vec<u8>> {
    use std::process::{Command, Stdio};
    let mut command = Command::new(program());
    command
        .args(["api", "graphql", "--hostname", host, "-f"])
        .arg(format!("query={query}"))
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn().ok()?;
    let mut output = child.stdout.take()?;
    std::thread::scope(|scope| {
        let reader = scope.spawn(move || {
            let mut response = Vec::new();
            (&mut output)
                .take(MAX_RESPONSE)
                .read_to_end(&mut response)
                .ok()
                .map(|_| response)
        });
        let deadline = Instant::now() + LOOKUP_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline && !reader.is_finished() => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
            }
        }
        reader.join().ok().flatten()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(url: &str) -> PullRequest {
        PullRequest::parse(url).unwrap()
    }
    fn status(state: State, checks: Checks, unresolved: u8) -> Status {
        Status {
            state,
            checks,
            unresolved,
        }
    }

    #[test]
    fn one_request_names_every_pull_request_of_a_host() {
        let links = [
            link("https://github.com/zevem/neptune/pull/83"),
            link("https://github.com/other-owner/re.po_2/pull/7"),
        ];
        let query = query(&links);
        assert!(query.starts_with(
            "query{p0:repository(owner:\"zevem\",name:\"neptune\"){pullRequest(number:83){state isDraft "
        ));
        assert!(query.contains(
            "p1:repository(owner:\"other-owner\",name:\"re.po_2\"){pullRequest(number:7){"
        ));
        assert_eq!(query.matches('{').count(), query.matches('}').count());
    }

    #[test]
    fn responses_reduce_to_state_checks_and_unresolved_conversations() {
        let pull = |state: &str, draft: bool, rollup: &str, threads: &str| {
            format!(
                r#"{{"pullRequest":{{"state":"{state}","isDraft":{draft},
                "commits":{{"nodes":[{{"commit":{{"statusCheckRollup":{rollup}}}}}]}},
                "reviewThreads":{{"nodes":[{threads}]}}}}}}"#
            )
        };
        let (open, resolved) = (r#"{"isResolved":false}"#, r#"{"isResolved":true}"#);
        let response = format!(
            r#"{{"data":{{"p0":{},"p1":{},"p2":{},"p3":{},"p4":null,"p5":{},"p6":{}}},
            "errors":[{{"type":"NOT_FOUND","path":["p4"]}}]}}"#,
            pull(
                "OPEN",
                false,
                r#"{"state":"FAILURE"}"#,
                &format!("{open},{resolved},{open}")
            ),
            pull("OPEN", true, r#"{"state":"PENDING"}"#, ""),
            pull("MERGED", false, r#"{"state":"SUCCESS"}"#, open),
            pull("CLOSED", false, "null", ""),
            pull(
                "OPEN",
                false,
                r#"{"state":"EXPECTED"}"#,
                &[open; 120].join(",")
            ),
            pull("OPEN", false, r#"{"state":"SURPRISE"}"#, ""),
        );
        assert_eq!(
            parse(response.as_bytes(), 8),
            [
                Some(status(State::Open, Checks::Failing, 2)),
                Some(status(State::Draft, Checks::Pending, 0)),
                Some(status(State::Merged, Checks::Passing, 1)),
                Some(status(State::Closed, Checks::None, 0)),
                None,
                Some(status(State::Open, Checks::Pending, 100)),
                None,
                None,
            ]
        );
        assert_eq!(parse(b"gh: not signed in", 2), [None, None]);
    }

    #[test]
    fn states_are_said_in_words_and_review_marks_end_with_the_review() {
        let open = status(State::Open, Checks::Failing, 2);
        assert_eq!(
            open.describe(),
            "Open · Checks failing · 2 unresolved comments"
        );
        assert_eq!(
            status(State::Draft, Checks::Pending, 1).describe(),
            "Draft · Checks running · 1 unresolved comment"
        );
        assert_eq!(
            status(State::Open, Checks::Passing, 100).describe(),
            "Open · Checks passing · 99+ unresolved comments"
        );
        assert_eq!(status(State::Open, Checks::None, 0).describe(), "Open");
        let merged = status(State::Merged, Checks::Failing, 3);
        assert_eq!(merged.describe(), "Merged");
        assert_eq!((merged.checks(), merged.unresolved()), (Checks::None, 0));
        // Running checks are read again soonest; a finished pull request rarely.
        assert!(
            status(State::Open, Checks::Pending, 0).refresh_after() < open.refresh_after()
                && open.refresh_after() < merged.refresh_after()
        );
    }

    #[test]
    fn links_are_read_once_per_host_and_a_frame_is_asked_only_for_a_change() {
        use std::sync::mpsc;
        let (asked, asks) = mpsc::channel::<(String, Vec<u64>)>();
        let (answer, answers) = mpsc::channel::<Vec<Option<Status>>>();
        let mut watcher = Watcher::with(Box::new(move |host, links| {
            let numbers = links.iter().map(PullRequest::number).collect();
            asked.send((host.to_owned(), numbers)).unwrap();
            answers.recv().unwrap()
        }));
        let ctx = egui::Context::default();
        let frames = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = frames.clone();
        ctx.set_request_repaint_callback(move |_| {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        let frames = || frames.load(std::sync::atomic::Ordering::SeqCst);
        let wait = |until: &dyn Fn() -> bool| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !until() {
                assert!(Instant::now() < deadline, "timed out");
                std::thread::sleep(Duration::from_millis(2));
            }
        };

        let first = link("https://github.com/zevem/neptune/pull/83");
        let other = link("https://git.example.com/team/tool/pull/4");
        // No link, no worker.
        watcher.watch(std::iter::empty(), &ctx);
        assert!(watcher.fetch.is_some());
        assert_eq!(watcher.lookup(&first), Lookup::Checking);

        // The same pull request linked by two terminals is read once.
        let same = link("https://github.com/Zevem/Neptune/pull/83");
        watcher.watch(
            [first.clone(), other.clone(), same.clone()].into_iter(),
            &ctx,
        );
        let timeout = Duration::from_secs(5);
        assert_eq!(
            asks.recv_timeout(timeout).unwrap(),
            ("git.example.com".into(), vec![4])
        );
        answer.send(vec![None]).unwrap();
        assert_eq!(
            asks.recv_timeout(timeout).unwrap(),
            ("github.com".into(), vec![83])
        );
        let open = status(State::Open, Checks::Pending, 0);
        answer.send(vec![Some(open)]).unwrap();
        wait(&|| watcher.lookup(&same) == Lookup::Known(open));
        assert_eq!(watcher.lookup(&other), Lookup::Unavailable);
        wait(&|| frames() == 1);

        // Naming the same links again asks for nothing.
        watcher.watch([first.clone(), other.clone()].into_iter(), &ctx);
        assert!(asks.recv_timeout(Duration::from_millis(50)).is_err());
        assert_eq!(frames(), 1);

        // A state that left view is remembered, and its worker stops with it.
        watcher.watch(std::iter::empty(), &ctx);
        assert_eq!(watcher.lookup(&first), Lookup::Known(open));
        drop(watcher);
        assert!(asks.recv_timeout(timeout).is_err());
    }

    #[test]
    fn a_state_is_read_again_when_it_is_due() {
        let shared = Arc::new(Shared::default());
        let first = link("https://github.com/zevem/neptune/pull/83");
        let merged = status(State::Merged, Checks::Passing, 0);
        let long_ago = Instant::now() - Duration::from_secs(11 * 60);
        {
            let mut watched = shared.lock();
            watched.wanted = vec![first.clone()];
            watched
                .known
                .insert(key(&first), (Lookup::Known(merged), long_ago));
            // Out of view and read more than an hour ago: forgotten.
            watched.known.insert(
                "https://github.com/zevem/neptune/pull/1".into(),
                (Lookup::Unavailable, Instant::now() - REMEMBERED),
            );
        }
        let open = status(State::Open, Checks::Passing, 0);
        let closing = shared.clone();
        let fetch: Fetch = Box::new(move |_, links| {
            assert_eq!(links.len(), 1);
            // Closing with the answer ends the worker once it is stored.
            closing.lock().closed = true;
            vec![Some(open)]
        });
        run(&shared, &fetch, &egui::Context::default());
        let watched = shared.lock();
        assert_eq!(watched.known.len(), 1);
        assert_eq!(watched.known[&key(&first)].0, Lookup::Known(open));
    }
}
