//! Git worktrees for agents: one worker runs every git command, so frames
//! never wait for a repository. It makes a worktree for a branch, watches the
//! worktrees open in tabs for a merged branch, and removes them when asked.
use neptune_model::Worktree;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

type Wake = Arc<dyn Fn() + Send + Sync>;

/// How often the branches of open worktrees are looked at.
const WATCH: Duration = Duration::from_secs(10);
/// Worktrees of one repository that the sheet lists.
const LISTED: usize = 32;
/// Branch names read to tell a new name from an existing one.
const BRANCHES: usize = 4096;

/// What the sheet shows of the repository a terminal is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    /// The checkout worktrees are added to.
    pub root: PathBuf,
    /// The branch new branches start from, or `HEAD` without one.
    pub base: String,
    /// Its local branches, to say whether a name is new.
    pub branches: Vec<String>,
    /// Worktrees Neptune made here earlier, by branch.
    pub worktrees: Vec<Existing>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Existing {
    pub worktree: Worktree,
    /// The branch it was merged into, when it was.
    pub merged: Option<String>,
}
/// What removing a worktree would take with it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Condition {
    /// The branch it was merged into, when it was.
    pub merged: Option<String>,
    /// The branch commits are counted against.
    pub base: String,
    /// Changed and untracked files that were never committed.
    pub uncommitted: usize,
    /// Commits of the branch that `base` does not have.
    pub unmerged: usize,
}
impl Condition {
    /// Nothing would be lost: the offer to clean up is made unasked.
    pub fn finished(&self) -> bool {
        self.merged.is_some() && self.uncommitted == 0
    }
}

pub enum Request {
    Probe {
        cwd: PathBuf,
    },
    Create {
        cwd: PathBuf,
        branch: String,
    },
    /// Fetch a pull request of the repository's `origin` into `branch` and
    /// make that branch's worktree. Answered as `Created` is.
    Checkout {
        cwd: PathBuf,
        branch: String,
        /// `owner/repository`, which `origin` must be.
        repository: String,
        number: u64,
    },
    Inspect(Worktree),
    Remove {
        worktree: Worktree,
        /// Uncommitted changes go with the directory.
        discard: bool,
    },
    /// The worktrees open in tabs, replacing the earlier list.
    Watch(Vec<Worktree>),
}
#[derive(Debug)]
pub enum Event {
    Probed {
        cwd: PathBuf,
        result: Result<Repository, String>,
    },
    /// `cwd` and `branch` as they were asked for: who asked knows its own
    /// request by them.
    Created {
        cwd: PathBuf,
        branch: String,
        result: Result<Worktree, String>,
    },
    Inspected {
        path: PathBuf,
        result: Result<Condition, String>,
    },
    Removed {
        worktree: Worktree,
        result: Result<(), String>,
    },
    /// A watched branch was merged with nothing left uncommitted, or no
    /// longer is.
    Finished {
        path: PathBuf,
        merged: Option<String>,
    },
}

/// The application's handle on the worker, which starts with its first request.
#[derive(Default)]
pub struct Worktrees {
    sender: Option<mpsc::Sender<Request>>,
    events: Arc<Mutex<Vec<Event>>>,
}
impl Worktrees {
    pub fn request(&mut self, request: Request, wake: Wake) -> Result<(), String> {
        if self.sender.is_none() {
            let (sender, receiver) = mpsc::channel();
            let events = self.events.clone();
            std::thread::Builder::new()
                .name("neptune-worktrees".into())
                .spawn(move || work(receiver, events, wake))
                .map_err(|error| format!("Could not start the git worker: {error}"))?;
            self.sender = Some(sender);
        }
        self.sender
            .as_ref()
            .and_then(|sender| sender.send(request).ok())
            .ok_or_else(|| "The git worker stopped".to_owned())
    }
    pub fn drain(&self) -> Vec<Event> {
        self.events
            .lock()
            .map(|mut events| std::mem::take(&mut *events))
            .unwrap_or_default()
    }
}

/// What was last seen of a watched worktree.
#[derive(Default)]
struct Seen {
    /// The branch and its targets, as git names their commits.
    refs: String,
    merged: Option<String>,
    /// What the application was last told.
    told: Option<String>,
}

fn work(receiver: mpsc::Receiver<Request>, events: Arc<Mutex<Vec<Event>>>, wake: Wake) {
    let mut watched: Vec<Worktree> = Vec::new();
    let mut seen: HashMap<PathBuf, Seen> = HashMap::new();
    // The base branch of each watched repository, found once per list.
    let mut bases: HashMap<PathBuf, String> = HashMap::new();
    let report = |event| {
        if let Ok(mut events) = events.lock() {
            events.push(event);
        }
        wake();
    };
    loop {
        // Nothing open in a worktree: nothing to look at until asked.
        let request = if watched.is_empty() {
            match receiver.recv() {
                Ok(request) => Some(request),
                Err(_) => return,
            }
        } else {
            match receiver.recv_timeout(WATCH) {
                Ok(request) => Some(request),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        };
        match request {
            Some(Request::Probe { cwd }) => {
                let result = probe(&cwd);
                report(Event::Probed { cwd, result });
            }
            Some(Request::Create { cwd, branch }) => {
                let result = create(&cwd, &branch);
                report(Event::Created {
                    cwd,
                    branch,
                    result,
                });
            }
            Some(Request::Checkout {
                cwd,
                branch,
                repository,
                number,
            }) => {
                let result = fetch_pull_request(&cwd, &branch, &repository, number)
                    .and_then(|()| create(&cwd, &branch));
                report(Event::Created {
                    cwd,
                    branch,
                    result,
                });
            }
            Some(Request::Inspect(worktree)) => report(Event::Inspected {
                result: condition(&worktree),
                path: worktree.path,
            }),
            Some(Request::Remove { worktree, discard }) => {
                let result = remove(&worktree, discard);
                report(Event::Removed { worktree, result });
            }
            Some(Request::Watch(list)) => {
                seen.retain(|path, _| list.iter().any(|worktree| &worktree.path == path));
                bases.clear();
                watched = list;
            }
            None => {}
        }
        for worktree in &watched {
            let entry = seen.entry(worktree.path.clone()).or_default();
            let base = bases
                .entry(worktree.repository.clone())
                .or_insert_with(|| base(&worktree.repository));
            let refs = refs(worktree, base);
            if refs != entry.refs {
                entry.merged = merged(worktree, base);
                entry.refs = refs;
            }
            // Uncommitted work can come and go without any branch moving.
            let finished = entry
                .merged
                .clone()
                .filter(|_| uncommitted(&worktree.path) == Some(0));
            if finished != entry.told {
                entry.told.clone_from(&finished);
                report(Event::Finished {
                    path: worktree.path.clone(),
                    merged: finished,
                });
            }
        }
    }
}

/// Runs git without a terminal to ask on and without taking the locks an
/// agent's own git commands need. `Err` carries what git said.
fn git(directory: &Path, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => "git is not installed or not on PATH".to_owned(),
            _ => format!("Could not run git: {error}"),
        })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_owned())
    } else {
        let said = String::from_utf8_lossy(&output.stderr);
        let said = said
            .lines()
            .map(|line| {
                line.trim_start_matches("fatal: ")
                    .trim_start_matches("error: ")
            })
            .find(|line| !line.trim().is_empty())
            .unwrap_or("git failed");
        Err(said.to_owned())
    }
}

/// Every worktree of the repository `directory` is in, the repository's own
/// checkout first: its path and the branch it has checked out.
fn list(directory: &Path) -> Result<Vec<(PathBuf, Option<String>)>, String> {
    let listed = git(directory, &["worktree", "list", "--porcelain"]).map_err(|error| {
        if error.contains("not a git repository") {
            "This terminal is not in a git repository".to_owned()
        } else {
            error
        }
    })?;
    let mut entries: Vec<(PathBuf, Option<String>)> = Vec::new();
    for line in listed.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            // git writes `/` on every system; the rest of Neptune compares
            // paths as the system writes them.
            entries.push((Path::new(path).components().collect(), None));
        } else if let (Some(branch), Some(entry)) =
            (line.strip_prefix("branch refs/heads/"), entries.last_mut())
        {
            entry.1 = Some(branch.to_owned());
        }
    }
    if entries.is_empty() {
        return Err("This terminal is not in a git repository".into());
    }
    Ok(entries)
}

/// The branch finished work is merged into: the remote's default branch as
/// this checkout has it, else `main` or `master`. `HEAD` without any.
fn base(repository: &Path) -> String {
    let local = |name: &str| {
        git(
            repository,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{name}"),
            ],
        )
        .is_ok()
    };
    if let Ok(head) = git(
        repository,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    ) && let Some(name) = head.strip_prefix("origin/")
    {
        return if local(name) { name.to_owned() } else { head };
    }
    ["main", "master"]
        .into_iter()
        .find(|name| local(name))
        .unwrap_or("HEAD")
        .to_owned()
}

/// Where the worktrees of a repository are kept: beside it, so that neither
/// its status nor its ignore rules see them.
fn home(root: &Path) -> Option<PathBuf> {
    let mut name = root.file_name()?.to_owned();
    name.push(".worktrees");
    Some(root.parent()?.join(name))
}

/// The directory name of a branch: `feat/login` is `feat-login`.
fn slug(branch: &str) -> String {
    branch
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches(['-', '.'])
        .to_owned()
}

/// What a typed name becomes as a branch: spaces turn into dashes, so a few
/// words are a name. `Err` says what git would not take.
pub fn branch_name(typed: &str) -> Result<String, &'static str> {
    let name = typed.split_whitespace().collect::<Vec<_>>().join("-");
    if name.is_empty() {
        return Err("Name the branch");
    }
    let refused = name.len() > 200
        || name.starts_with(['-', '/', '.'])
        || name.ends_with(['/', '.'])
        || name.ends_with(".lock")
        || name == "HEAD"
        || name == "@"
        || name.contains("..")
        || name.contains("//")
        || name.contains("/.")
        || name.contains("@{")
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\'));
    if refused || slug(&name).is_empty() {
        return Err("Git does not take this as a branch name");
    }
    Ok(name)
}

fn probe(cwd: &Path) -> Result<Repository, String> {
    let entries = list(cwd)?;
    let root = entries[0].0.clone();
    let base = base(&root);
    let home = home(&root);
    let worktrees = entries
        .iter()
        .skip(1)
        .filter(|(path, _)| home.as_deref().is_some_and(|home| path.starts_with(home)))
        .filter_map(|(path, branch)| {
            let branch = branch.clone()?;
            let start = start(&root, &branch, &base).ok()?;
            let worktree = Worktree {
                repository: root.clone(),
                path: path.clone(),
                branch,
                start,
            };
            worktree.is_valid().then(|| Existing {
                merged: merged(&worktree, &base),
                worktree,
            })
        })
        .take(LISTED)
        .collect();
    let branches = git(
        &root,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )
    .unwrap_or_default()
    .lines()
    .take(BRANCHES)
    .map(str::to_owned)
    .collect();
    Ok(Repository {
        root,
        base,
        branches,
        worktrees,
    })
}

/// The commit a branch began at: the first one git's log of the branch
/// remembers, which for a new branch is where it stands. Without a log, where
/// it left the base.
fn start(root: &Path, branch: &str, base: &str) -> Result<String, String> {
    let reference = format!("refs/heads/{branch}");
    git(root, &["reflog", "show", "--format=%H", &reference])
        .ok()
        .and_then(|log| log.lines().last().map(str::to_owned))
        .filter(|commit| !commit.is_empty())
        .map_or_else(
            || {
                git(root, &["merge-base", &reference, base])
                    .or_else(|_| git(root, &["rev-parse", &reference]))
            },
            Ok,
        )
}

/// Whether `origin`, as git names its address, is `repository`
/// (`owner/name`): the last two parts of the address, whatever leads them.
fn is_origin(address: &str, repository: &str) -> bool {
    let address = address.trim().trim_end_matches('/');
    let address = address.strip_suffix(".git").unwrap_or(address);
    let mut parts = address.rsplit(['/', ':']);
    let (name, owner) = (parts.next(), parts.next());
    matches!((owner, name), (Some(owner), Some(name))
        if format!("{owner}/{name}").eq_ignore_ascii_case(repository))
}

/// Brings pull request `number` of `origin` into `branch`, where `origin`
/// is `repository`. A branch that is already there is brought up to date
/// when it only has to move forward, and left as it is otherwise.
fn fetch_pull_request(
    cwd: &Path,
    branch: &str,
    repository: &str,
    number: u64,
) -> Result<(), String> {
    let root = list(cwd)?[0].0.clone();
    let origin = git(&root, &["remote", "get-url", "origin"])
        .map_err(|_| "This repository has no remote named origin".to_owned())?;
    if !is_origin(&origin, repository) {
        return Err(format!(
            "The repository of this terminal is not {repository}. Focus a terminal in it"
        ));
    }
    let known = git(
        &root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok();
    let fetched = git(
        &root,
        &["fetch", "origin", &format!("pull/{number}/head:{branch}")],
    );
    match fetched {
        Ok(_) => Ok(()),
        // One that is checked out, or has work of its own, stays.
        Err(_) if known => Ok(()),
        Err(why) => Err(why),
    }
}

/// Makes the worktree of `branch`, or finds the one it already has. A new
/// branch starts from the base branch; an existing one is checked out as is.
fn create(cwd: &Path, branch: &str) -> Result<Worktree, String> {
    let entries = list(cwd)?;
    let root = entries[0].0.clone();
    git(&root, &["check-ref-format", "--branch", branch])
        .map_err(|_| "Git does not take this as a branch name".to_owned())?;
    let base = base(&root);
    let exists = git(
        &root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok();
    let path = match entries
        .iter()
        .position(|(_, checked_out)| checked_out.as_deref() == Some(branch))
    {
        Some(0) => {
            return Err(format!(
                "{branch} is checked out in the repository itself. Choose another branch"
            ));
        }
        Some(index) => entries[index].0.clone(),
        None => {
            let home = home(&root).ok_or("This repository has no parent directory")?;
            let path = home.join(slug(branch));
            if path.exists() {
                return Err(format!(
                    "{} already exists and is not the worktree of {branch}",
                    path.display()
                ));
            }
            let directory = path.to_string_lossy();
            if exists {
                git(&root, &["worktree", "add", &directory, branch])?;
            } else {
                git(
                    &root,
                    &[
                        "worktree",
                        "add",
                        "--no-track",
                        "-b",
                        branch,
                        &directory,
                        &base,
                    ],
                )?;
            }
            path
        }
    };
    let start = start(&root, branch, &base)?;
    let worktree = Worktree {
        repository: root,
        path,
        branch: branch.to_owned(),
        start,
    };
    if !worktree.is_valid() {
        return Err("Git reported a worktree Neptune cannot keep".into());
    }
    Ok(worktree)
}

/// The base branch here and as the remote has it; either may have the merge.
fn targets(base: &str) -> Vec<String> {
    if base == "HEAD" {
        Vec::new()
    } else if base.starts_with("origin/") {
        vec![base.to_owned()]
    } else {
        vec![base.to_owned(), format!("origin/{base}")]
    }
}

/// One cheap look at the commits the branch and its targets stand at.
fn refs(worktree: &Worktree, base: &str) -> String {
    let mut arguments = vec![
        "for-each-ref".to_owned(),
        "--format=%(refname) %(objectname)".to_owned(),
        format!("refs/heads/{}", worktree.branch),
    ];
    for target in targets(base) {
        arguments.push(match target.strip_prefix("origin/") {
            Some(name) => format!("refs/remotes/origin/{name}"),
            None => format!("refs/heads/{target}"),
        });
    }
    let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
    git(&worktree.repository, &arguments).unwrap_or_default()
}

/// The branch `worktree`'s branch was merged into. Its commits are in that
/// branch, or merging it there would change nothing, as after a squash or a
/// rebase. A branch still at its start has nothing to have merged.
fn merged(worktree: &Worktree, base: &str) -> Option<String> {
    let repository = &worktree.repository;
    let branch = format!("refs/heads/{}", worktree.branch);
    let tip = git(repository, &["rev-parse", "--verify", "--quiet", &branch]).ok()?;
    if tip == worktree.start {
        return None;
    }
    targets(base).into_iter().find(|target| {
        if git(repository, &["merge-base", "--is-ancestor", &tip, target]).is_ok() {
            return true;
        }
        // Older gits lack this form of merge-tree and report nothing merged.
        let tree = format!("{target}^{{tree}}");
        match (
            git(repository, &["merge-tree", "--write-tree", target, &tip]),
            git(repository, &["rev-parse", "--verify", "--quiet", &tree]),
        ) {
            (Ok(result), Ok(tree)) => result.lines().next() == Some(tree.as_str()),
            _ => false,
        }
    })
}

/// Changed and untracked files of a worktree; `None` when it cannot be read.
fn uncommitted(path: &Path) -> Option<usize> {
    git(path, &["status", "--porcelain"])
        .ok()
        .map(|status| status.lines().filter(|line| !line.is_empty()).count())
}

fn condition(worktree: &Worktree) -> Result<Condition, String> {
    let repository = &worktree.repository;
    let base = base(repository);
    let merged = merged(worktree, &base);
    let branch = format!("refs/heads/{}", worktree.branch);
    // A branch deleted by hand has nothing left to compare or to lose.
    let exists = git(repository, &["rev-parse", "--verify", "--quiet", &branch]).is_ok();
    let unmerged = if merged.is_some() || !exists {
        0
    } else {
        git(
            repository,
            &["rev-list", "--count", &format!("{base}..{branch}")],
        )
        .ok()
        .and_then(|count| count.trim().parse().ok())
        .ok_or_else(|| format!("Could not compare {} with {base}", worktree.branch))?
    };
    Ok(Condition {
        merged,
        base,
        // Already deleted by hand: only git's record of it is left.
        uncommitted: if worktree.path.is_dir() {
            uncommitted(&worktree.path).ok_or("Could not read the worktree's changes")?
        } else {
            0
        },
        unmerged,
    })
}

/// Removes the directory, and the branch when the base has all its commits.
/// A branch with commits of its own is kept, whether Neptune made it or
/// found it, so removing never loses committed work.
fn remove(worktree: &Worktree, discard: bool) -> Result<(), String> {
    let repository = &worktree.repository;
    let state = condition(worktree)?;
    if state.uncommitted > 0 && !discard {
        return Err("The worktree has uncommitted changes".into());
    }
    if worktree.path.is_dir() {
        let directory = worktree.path.to_string_lossy();
        let mut arguments = vec!["worktree", "remove"];
        if discard {
            arguments.push("--force");
        }
        arguments.push(&directory);
        git(repository, &arguments)?;
    } else {
        git(repository, &["worktree", "prune"])?;
    }
    if let Some(home) = worktree.path.parent() {
        // Only an empty directory goes.
        let _ = std::fs::remove_dir(home);
    }
    let branch = format!("refs/heads/{}", worktree.branch);
    let exists = git(repository, &["rev-parse", "--verify", "--quiet", &branch]).is_ok();
    if exists && (state.merged.is_some() || state.unmerged == 0) {
        git(repository, &["branch", "-D", &worktree.branch])
            .map_err(|error| format!("The worktree is removed, but its branch is kept: {error}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(directory: &Path, arguments: &[&str]) -> String {
        git(directory, arguments).unwrap_or_else(|error| panic!("git {arguments:?}: {error}"))
    }
    /// A repository with one commit on `main`, in a directory of its own.
    #[test]
    fn a_pull_request_is_fetched_into_a_branch_of_its_own_from_its_own_repository() {
        for (address, is) in [
            ("https://github.com/zevem/neptune.git", true),
            ("git@github.com:Zevem/Neptune", true),
            ("https://github.com/zevem/neptune/", true),
            ("https://github.com/other/neptune.git", false),
            ("neptune", false),
        ] {
            assert_eq!(is_origin(address, "zevem/neptune"), is, "{address}");
        }
        // A repository that stands in for the host, with a pull request's
        // head where a host keeps it.
        let (_host_directory, host) = repository();
        run(&host, &["checkout", "--quiet", "-b", "feature"]);
        commit(&host, "feature.txt", "work");
        run(&host, &["update-ref", "refs/pull/7/head", "HEAD"]);
        run(&host, &["checkout", "--quiet", "main"]);
        let (_directory, root) = repository();
        // Named as the address of the repository it is asked for.
        let named = host.parent().unwrap().join("zevem").join("neptune");
        std::fs::create_dir_all(named.parent().unwrap()).unwrap();
        std::fs::rename(&host, &named).unwrap();
        run(&root, &["remote", "add", "origin", named.to_str().unwrap()]);
        assert!(
            fetch_pull_request(&root, "pr-7", "other/neptune", 7)
                .unwrap_err()
                .contains("is not other/neptune")
        );
        fetch_pull_request(&root, "pr-7", "zevem/neptune", 7).unwrap();
        let made = create(&root, "pr-7").unwrap();
        assert!(made.path.join("feature.txt").is_file());
        assert_eq!(made.branch, "pr-7");
        // Asked for again, the branch that is checked out stays as it is.
        fetch_pull_request(&root, "pr-7", "zevem/neptune", 7).unwrap();
        assert_eq!(create(&root, "pr-7").unwrap().path, made.path);
        assert!(fetch_pull_request(&root, "pr-8", "zevem/neptune", 8).is_err());
    }

    fn repository() -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap().join("project");
        std::fs::create_dir(&root).unwrap();
        run(&root, &["init", "--quiet", "--initial-branch=main"]);
        for (key, value) in [
            ("user.name", "Neptune Test"),
            ("user.email", "test@neptune.invalid"),
            ("commit.gpgsign", "false"),
        ] {
            run(&root, &["config", key, value]);
        }
        commit(&root, "README.md", "one");
        // As git names it, which is how every worktree's path is reported.
        let root = list(&root).unwrap()[0].0.clone();
        (directory, root)
    }
    fn commit(directory: &Path, file: &str, contents: &str) {
        std::fs::write(directory.join(file), contents).unwrap();
        run(directory, &["add", "--all"]);
        run(directory, &["commit", "--quiet", "-m", file]);
    }

    #[test]
    fn typed_words_become_a_branch_name_and_what_git_refuses_is_refused() {
        assert_eq!(
            branch_name("  fix login  flow "),
            Ok("fix-login-flow".into())
        );
        assert_eq!(
            branch_name("feat/agent-worktree"),
            Ok("feat/agent-worktree".into())
        );
        for refused in [
            "", "  ", "-D", "a..b", "x.lock", "a:b", "feat/", "HEAD", "/x", "a~1",
        ] {
            assert!(branch_name(refused).is_err(), "{refused:?}");
        }
        assert_eq!(slug("feat/Agent worktree"), "feat-Agent-worktree");
    }

    #[test]
    fn a_new_branch_gets_a_worktree_beside_the_repository_and_is_found_again() {
        let (_directory, root) = repository();
        commit(&root, "later.txt", "two");
        let nested = root.join("src");
        std::fs::create_dir(&nested).unwrap();

        let repository = probe(&nested).unwrap();
        assert_eq!(
            (repository.root.as_path(), repository.base.as_str()),
            (root.as_path(), "main")
        );
        assert!(repository.worktrees.is_empty());
        assert_eq!(repository.branches, ["main"]);

        let made = create(&nested, "feat/login").unwrap();
        assert_eq!(made.repository, root);
        assert_eq!(
            made.path,
            root.parent().unwrap().join("project.worktrees/feat-login")
        );
        assert_eq!(made.start, run(&root, &["rev-parse", "main"]));
        assert_eq!(run(&made.path, &["branch", "--show-current"]), "feat/login");
        // The repository's own checkout stays as it was.
        assert_eq!(run(&root, &["status", "--porcelain"]), "");
        assert_eq!(run(&root, &["branch", "--show-current"]), "main");

        // Asked again, from inside the worktree: the same one, nothing new.
        assert_eq!(create(&made.path, "feat/login").unwrap(), made);
        let listed = probe(&made.path).unwrap();
        assert_eq!(listed.root, root);
        assert_eq!(listed.worktrees.len(), 1);
        assert_eq!(listed.worktrees[0].worktree, made);
        assert_eq!(listed.worktrees[0].merged, None);

        // An existing branch is checked out as it is, never reset.
        run(&root, &["branch", "old", "HEAD~1"]);
        let old = create(&root, "old").unwrap();
        assert_eq!(
            run(&old.path, &["rev-parse", "HEAD"]),
            run(&root, &["rev-parse", "main~1"])
        );

        assert!(
            create(&root, "main")
                .unwrap_err()
                .contains("checked out in the repository")
        );
        assert!(create(&root, "a..b").is_err());
        let outside = tempfile::tempdir().unwrap();
        assert_eq!(
            probe(outside.path()).unwrap_err(),
            "This terminal is not in a git repository"
        );
    }

    #[test]
    fn a_branch_is_merged_once_its_work_is_in_the_base_however_it_got_there() {
        let (_directory, root) = repository();
        let fresh = create(&root, "fresh").unwrap();
        // Nothing done yet, and the base moving on does not change that.
        commit(&root, "base.txt", "base");
        assert_eq!(merged(&fresh, "main"), None);
        assert_eq!(
            condition(&fresh).unwrap(),
            Condition {
                base: "main".into(),
                ..Default::default()
            }
        );

        let merge = create(&root, "by-merge").unwrap();
        commit(&merge.path, "merge.txt", "work");
        let state = condition(&merge).unwrap();
        assert_eq!(
            (state.merged, state.unmerged, state.uncommitted),
            (None, 1, 0)
        );
        run(
            &root,
            &["merge", "--quiet", "--no-ff", "--no-edit", "by-merge"],
        );
        assert_eq!(merged(&merge, "main"), Some("main".into()));

        // A squash leaves no commit of the branch in the base.
        let squash = create(&root, "by-squash").unwrap();
        commit(&squash.path, "squash-1.txt", "work");
        commit(&squash.path, "squash-2.txt", "more");
        assert_eq!(merged(&squash, "main"), None);
        run(&root, &["merge", "--quiet", "--squash", "by-squash"]);
        run(&root, &["commit", "--quiet", "-m", "squashed"]);
        commit(&root, "after.txt", "the base goes on");
        assert_eq!(merged(&squash, "main"), Some("main".into()));

        // The sheet's list says the same of worktrees it finds again.
        let listed = probe(&root).unwrap().worktrees;
        let found = |branch: &str| {
            listed
                .iter()
                .find(|existing| existing.worktree.branch == branch)
                .unwrap()
        };
        assert_eq!(found("by-merge").worktree, merge);
        assert_eq!(found("by-merge").merged.as_deref(), Some("main"));
        assert_eq!(found("by-squash").merged.as_deref(), Some("main"));
        assert_eq!(found("fresh").merged, None);

        // Uncommitted work keeps the offer back.
        std::fs::write(squash.path.join("note.txt"), "not committed").unwrap();
        let state = condition(&squash).unwrap();
        assert_eq!(state.uncommitted, 1);
        assert!(!state.finished());
        assert!(condition(&merge).unwrap().finished());
    }

    #[test]
    fn removing_takes_the_directory_and_only_a_branch_with_nothing_left_to_lose() {
        let (_directory, root) = repository();
        let branches = |root: &Path| run(root, &["branch", "--format=%(refname:short)"]);

        // Merged: the directory and the branch go, and so does the empty
        // directory that held the worktrees.
        let done = create(&root, "done").unwrap();
        commit(&done.path, "done.txt", "work");
        run(&root, &["merge", "--quiet", "--no-edit", "done"]);
        remove(&done, false).unwrap();
        assert!(!done.path.exists() && !done.path.parent().unwrap().exists());
        assert_eq!(branches(&root), "main");

        // Commits nobody merged: the branch keeps them.
        let open = create(&root, "open").unwrap();
        commit(&open.path, "open.txt", "work");
        std::fs::write(open.path.join("draft.txt"), "not committed").unwrap();
        assert_eq!(
            remove(&open, false).unwrap_err(),
            "The worktree has uncommitted changes"
        );
        assert!(open.path.join("draft.txt").exists());
        remove(&open, true).unwrap();
        assert!(!open.path.exists());
        assert_eq!(branches(&root), "main\nopen");

        // Never used: nothing to keep.
        let unused = create(&root, "unused").unwrap();
        remove(&unused, false).unwrap();
        assert_eq!(branches(&root), "main\nopen");

        // A branch found with commits of its own keeps them, though nothing
        // was added to it here.
        run(&root, &["branch", "found", "open"]);
        let found = create(&root, "found").unwrap();
        assert_eq!(found.start, run(&root, &["rev-parse", "found"]));
        remove(&found, false).unwrap();
        assert_eq!(branches(&root), "found\nmain\nopen");

        // Deleted by hand: git's record of it is cleared.
        let gone = create(&root, "gone").unwrap();
        std::fs::remove_dir_all(&gone.path).unwrap();
        remove(&gone, false).unwrap();
        assert_eq!(list(&root).unwrap().len(), 1);
        assert_eq!(branches(&root), "found\nmain\nopen");
    }

    #[test]
    fn the_worker_reports_a_merge_once_and_takes_it_back_when_work_appears() {
        let (_directory, root) = repository();
        let tree = create(&root, "watched").unwrap();
        let mut worktrees = Worktrees::default();
        let (woken, wakes) = mpsc::channel();
        let wake: Wake = Arc::new(move || {
            let _ = woken.send(());
        });
        let next = |worktrees: &Worktrees| {
            wakes.recv_timeout(Duration::from_secs(20)).unwrap();
            worktrees.drain()
        };
        commit(&tree.path, "work.txt", "work");
        run(&root, &["merge", "--quiet", "--no-edit", "watched"]);
        worktrees
            .request(Request::Watch(vec![tree.clone()]), wake.clone())
            .unwrap();
        let events = next(&worktrees);
        assert!(matches!(
            &events[..],
            [Event::Finished { path, merged: Some(into) }] if path == &tree.path && into == "main"
        ));
        std::fs::write(tree.path.join("more.txt"), "not committed").unwrap();
        worktrees
            .request(Request::Watch(vec![tree.clone()]), wake)
            .unwrap();
        let events = next(&worktrees);
        assert!(matches!(
            &events[..],
            [Event::Finished { merged: None, .. }]
        ));
    }
}
