//! What a project keeps on disk, in its folder under the data root,
//! `projects/<key>/`: `project.json` (its lead's conversation, its agents and
//! counters) and `chat.jsonl` (the chat, one entry a line, with one earlier
//! file beside it as `chat.1.jsonl`), and `context/`, the files its agents
//! share. One worker reads and writes all of it; frames queue work and take
//! results, and never wait for either.
//!
//! The same worker keeps the clock of the projects' watches: it sleeps until
//! the earliest is due and says so then. It never decides what a watch does.
//!
//! A file this build cannot read is left exactly as it is: its project is
//! shown as far as it could be read and nothing of it is written. That never
//! holds back another project, the saved window or the settings, which have
//! their own writer.
mod context;

pub use self::context::Op as ContextOp;
use super::project_lead::Wake;
use crate::projects::{
    context::Digest,
    registry::{Saved, VERSION},
    transcript::{self, HEADER, MAX_ENTRIES, MAX_LINE, Record},
};
use neptune_model::{AgentKind, ProjectKey};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::{self, BufRead, Read as _, Seek, Write},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError},
    thread,
    time::{Duration, Instant, SystemTime},
};

/// Work that waits for the worker. A frame that finds it full keeps what it
/// has and stops taking more from its lead until there is room.
pub const MAX_JOBS: usize = 512;
/// A project's state is written this long after it first changed, so that a
/// run of changes is one write.
const DEBOUNCE: Duration = Duration::from_millis(200);
/// A `project.json` larger than this was not written by this build.
const MAX_STATE: u64 = 1024 * 1024;
/// A chat this long is set aside as the earlier one when more is added.
const ROTATE: u64 = 4 * 1024 * 1024;
/// How much of a chat's end is read back.
const MAX_READ: u64 = 16 * 1024 * 1024;
/// Older entries read back at once for "Load earlier".
pub const PAGE: usize = 200;
/// Projects without a workspace that are offered again.
const MAX_KEPT: usize = 32;
/// Watches whose time came that no frame has taken yet. A watch that is due
/// while this many wait stays on the clock until there is room.
const MAX_FIRED: usize = 64;
/// Times one project has on the clock: one for each of its watches.
const MAX_TIMES: usize = crate::projects::subscription::MAX;
const STATE: &str = "project.json";
const CHAT: &str = "chat.jsonl";
const ARCHIVE: &str = "chat.1.jsonl";

/// A folder only this user enters: a project's chat is the user's own.
fn private(path: &Path, parents: bool) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(parents);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

/// Makes the folder of a new project under `root` and names it. The key is
/// random and its folder is made new, so a project never takes over what
/// another one left behind.
pub fn create(root: &Path) -> io::Result<ProjectKey> {
    private(root, true)?;
    for _ in 0..4 {
        let random = super::project_lead::uuid()?;
        let key: String = random
            .chars()
            .filter(char::is_ascii_hexdigit)
            .take(ProjectKey::LENGTH)
            .collect();
        let key = ProjectKey::parse(&key).ok_or_else(|| io::Error::other("unusable key"))?;
        match private(&root.join(key.as_str()), false) {
            Ok(()) => return Ok(key),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::other("no free project folder"))
}

/// Makes sure the folder of an existing project is there, such as after its
/// user removed it by hand.
pub fn ensure(folder: &Path) -> io::Result<()> {
    private(folder, true)
}

/// Where the project named `key` keeps its files.
pub fn folder(root: &Path, key: &ProjectKey) -> PathBuf {
    root.join(key.as_str())
}

/// Why nothing of a project is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protection {
    /// Its state names a format this build does not know.
    Newer,
    /// Its state could not be read, is larger than any this build writes,
    /// or is damaged and could not be copied aside first.
    Unreadable,
    /// Its chat does not begin as a chat of this build does.
    Chat,
}
/// A project as it was read back.
#[derive(Debug, Default)]
pub struct Loaded {
    /// Nothing where it has no state yet.
    pub saved: Option<Saved>,
    /// The newest entries of its chat, oldest first.
    pub records: Vec<Record>,
    /// The number of the newest entry ever written to it.
    pub last: u64,
    /// Its chat holds entries older than `records`.
    pub more: bool,
    pub protection: Option<Protection>,
    /// Its state was damaged: a copy was kept and it starts anew.
    pub recovered: bool,
}
/// A project whose folder is there and whose workspace is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kept {
    pub key: ProjectKey,
    pub name: String,
    pub directory: PathBuf,
    pub lead: AgentKind,
}
/// What is written to a chat, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    Entry(Record),
    /// The chat so far is set aside as the earlier one; a new one begins.
    Archive,
}
#[derive(Debug)]
pub enum Reply {
    Created(io::Result<ProjectKey>),
    Loaded {
        key: ProjectKey,
        loaded: Box<Loaded>,
    },
    Earlier {
        key: ProjectKey,
        records: Vec<Record>,
        more: bool,
    },
    /// Something of this project could not be written.
    Unsaved(ProjectKey),
    /// Its folder could not be deleted.
    NotRemoved(ProjectKey),
    Kept(Vec<Kept>),
    /// What became of something asked of a project's context, by the number
    /// it was asked under, and what the context holds now.
    Context {
        key: ProjectKey,
        ticket: u64,
        result: Result<String, String>,
        digest: Digest,
    },
    /// The time watch `id` of this project was set for has come: `due`, in
    /// seconds since the Unix epoch. Said once; the project sets its next.
    Fired {
        key: ProjectKey,
        id: u64,
        due: u64,
    },
}

enum Job {
    Create,
    Load(ProjectKey),
    Write(ProjectKey, Out),
    Earlier(ProjectKey, u64),
    Remove(ProjectKey),
    /// Lists the folders of projects other than these.
    Kept(Vec<ProjectKey>),
    Context(ProjectKey, u64, ContextOp),
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    /// Each project's newest state and when it is due to be written.
    saves: BTreeMap<ProjectKey, (Saved, Instant)>,
    replies: Vec<Reply>,
    /// When each project's watches are next due, as `(id, due)` in seconds
    /// since the Unix epoch.
    clock: BTreeMap<ProjectKey, Vec<(u64, u64)>>,
    /// The worker runs.
    started: bool,
    /// The worker is writing what it took.
    busy: bool,
    /// Everything is written now, not when it is due.
    hurry: bool,
    stop: bool,
}
struct Shared {
    root: PathBuf,
    /// A run that saves nothing, such as a capture.
    ephemeral: bool,
    queue: Mutex<Queue>,
    changed: Condvar,
    idle: Condvar,
    wake: Mutex<Option<Wake>>,
    disks: Mutex<BTreeMap<ProjectKey, Disk>>,
}
impl Shared {
    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The projects' folders, as frames use them.
pub struct Store {
    shared: Arc<Shared>,
    /// Work is done when results are taken, on the caller's thread.
    inline: bool,
}
impl Store {
    /// The worker starts with the first thing asked of it.
    pub fn new(root: PathBuf, ephemeral: bool) -> Self {
        Self {
            shared: Arc::new(Shared {
                root,
                ephemeral,
                queue: Mutex::default(),
                changed: Condvar::new(),
                idle: Condvar::new(),
                wake: Mutex::new(None),
                disks: Mutex::default(),
            }),
            inline: false,
        }
    }
    /// A store without a worker: `poll` does what was asked before it
    /// answers, so a test sees each result on the frame after its cause.
    #[cfg(test)]
    pub fn inline(root: PathBuf) -> Self {
        let mut store = Self::new(root, false);
        store.inline = true;
        store
    }
    pub fn root(&self) -> &Path {
        &self.shared.root
    }
    /// Who is woken when a result is ready. The first caller's stays.
    pub fn woken_by(&self, wake: impl FnOnce() -> Wake) {
        let mut slot = self
            .shared
            .wake
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if slot.is_none() {
            *slot = Some(wake());
        }
    }
    fn start(&self, queue: &mut Queue) -> bool {
        if self.inline || queue.started {
            return true;
        }
        let shared = self.shared.clone();
        queue.started = thread::Builder::new()
            .name("neptune-projects".into())
            .spawn(move || run(&shared))
            .is_ok();
        queue.started
    }
    fn push(&self, job: Job) -> bool {
        let mut queue = self.shared.lock();
        if queue.jobs.len() >= MAX_JOBS || !self.start(&mut queue) {
            return false;
        }
        queue.jobs.push_back(job);
        self.shared.changed.notify_all();
        true
    }
    /// Asks for a new project's folder. False when the worker is too far
    /// behind to take it.
    pub fn create(&self) -> bool {
        self.push(Job::Create)
    }
    pub fn load(&self, key: &ProjectKey) -> bool {
        self.push(Job::Load(key.clone()))
    }
    /// Hands over what waits to be written to a chat, oldest first, as far
    /// as the worker has room. What is left waits for the caller's next try.
    pub fn write(&self, key: &ProjectKey, out: &mut VecDeque<Out>) {
        if out.is_empty() {
            return;
        }
        let mut queue = self.shared.lock();
        if !self.start(&mut queue) {
            return;
        }
        while queue.jobs.len() < MAX_JOBS
            && let Some(next) = out.pop_front()
        {
            queue.jobs.push_back(Job::Write(key.clone(), next));
        }
        self.shared.changed.notify_all();
    }
    /// The state of a project as it is now. A newer one replaces what has
    /// not been written yet.
    pub fn save(&self, key: &ProjectKey, saved: Saved) {
        let mut queue = self.shared.lock();
        if !self.start(&mut queue) {
            return;
        }
        let due = queue
            .saves
            .get(key)
            .map_or_else(|| Instant::now() + DEBOUNCE, |(_, due)| *due);
        queue.saves.insert(key.clone(), (saved, due));
        self.shared.changed.notify_all();
    }
    /// Sets when the watches of a project are next due, as `(id, due)` in
    /// seconds since the Unix epoch, in place of what was set for it before.
    /// Each is said once, with `Reply::Fired`, and then forgotten.
    pub fn schedule(&self, key: &ProjectKey, mut times: Vec<(u64, u64)>) {
        let mut queue = self.shared.lock();
        times.truncate(MAX_TIMES);
        if times.is_empty() {
            if queue.clock.remove(key).is_none() {
                return;
            }
        } else {
            if !self.start(&mut queue) {
                return;
            }
            queue.clock.insert(key.clone(), times);
        }
        self.shared.changed.notify_all();
    }
    /// Asks for entries older than number `before`.
    pub fn earlier(&self, key: &ProjectKey, before: u64) -> bool {
        self.push(Job::Earlier(key.clone(), before))
    }
    /// Deletes a project's folder with everything in it.
    pub fn remove(&self, key: &ProjectKey) -> bool {
        // Its state is not written into the folder that goes, and its
        // watches are over.
        {
            let mut queue = self.shared.lock();
            queue.saves.remove(key);
            queue.clock.remove(key);
        }
        self.push(Job::Remove(key.clone()))
    }
    /// Asks which folders belong to projects other than `open`.
    pub fn kept(&self, open: Vec<ProjectKey>) -> bool {
        self.push(Job::Kept(open))
    }
    /// Asks something of a project's context: to read it, or to write what
    /// the rules already allowed. Every one is answered with what the
    /// context holds afterwards, under `ticket`.
    pub fn context(&self, key: &ProjectKey, ticket: u64, op: ContextOp) -> bool {
        self.push(Job::Context(key.clone(), ticket, op))
    }
    /// What the worker has ready. Never waits.
    pub fn poll(&self) -> Vec<Reply> {
        let mut queue = self.shared.lock();
        if self.inline {
            strike(&mut queue, SystemTime::now());
        }
        if self.inline && (!queue.jobs.is_empty() || !queue.saves.is_empty()) {
            let jobs = queue.jobs.drain(..).collect();
            let saves = std::mem::take(&mut queue.saves)
                .into_iter()
                .map(|(key, (saved, _))| (key, saved))
                .collect();
            drop(queue);
            let replies = work(&self.shared, jobs, saves);
            queue = self.shared.lock();
            queue.replies.extend(replies);
        }
        std::mem::take(&mut queue.replies)
    }
    /// Writes everything that waits, for at most `within`. Called as the
    /// application closes; never from a frame.
    pub fn flush(&self, within: Duration) {
        if self.inline {
            let replies = self.poll();
            self.shared.lock().replies.extend(replies);
            return;
        }
        let deadline = Instant::now() + within;
        let mut queue = self.shared.lock();
        if !queue.started {
            return;
        }
        queue.hurry = true;
        self.shared.changed.notify_all();
        while queue.busy || !queue.jobs.is_empty() || !queue.saves.is_empty() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            queue = self
                .shared
                .idle
                .wait_timeout(queue, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}
impl Drop for Store {
    fn drop(&mut self) {
        // The worker writes what it was handed and then ends by itself.
        self.shared.lock().stop = true;
        self.shared.changed.notify_all();
    }
}

/// Moves the watches whose time has come by `now` from the clock to what
/// frames take. Returns whether any did, and how long until the next.
fn strike(queue: &mut Queue, now: SystemTime) -> (bool, Option<Duration>) {
    let now = crate::projects::schedule::seconds(now);
    let waiting = |queue: &Queue| {
        queue
            .replies
            .iter()
            .filter(|reply| matches!(reply, Reply::Fired { .. }))
            .count()
    };
    let mut room = MAX_FIRED.saturating_sub(waiting(queue));
    let Queue { clock, replies, .. } = queue;
    let (mut struck, mut next) = (false, None::<u64>);
    clock.retain(|key, times| {
        times.retain(|(id, due)| {
            if *due > now {
                next = Some(next.map_or(*due, |next| next.min(*due)));
                return true;
            }
            if room == 0 {
                // No frame has taken the earlier ones: looked at again soon.
                next = Some(now + 1);
                return true;
            }
            room -= 1;
            struck = true;
            replies.push(Reply::Fired {
                key: key.clone(),
                id: *id,
                due: *due,
            });
            false
        });
        !times.is_empty()
    });
    (
        struck,
        next.map(|next| Duration::from_secs(next.saturating_sub(now).max(1))),
    )
}

fn run(shared: &Shared) {
    let mut queue = shared.lock();
    loop {
        // A watch whose time has come is said at once: its project decides
        // what follows. Never before its time, by the system's clock.
        let (struck, until) = strike(&mut queue, SystemTime::now());
        if struck {
            let wake = shared
                .wake
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            if let Some(wake) = wake {
                drop(queue);
                wake();
                queue = shared.lock();
                // The clock may have been set anew meanwhile, with nobody
                // waiting to be told: it is read again before sleeping.
                continue;
            }
        }
        let now = Instant::now();
        let all = queue.stop || queue.hurry;
        let due: Vec<ProjectKey> = queue
            .saves
            .iter()
            .filter(|(_, (_, at))| all || *at <= now)
            .map(|(key, _)| key.clone())
            .collect();
        if queue.jobs.is_empty() && due.is_empty() {
            queue.busy = false;
            queue.hurry = false;
            shared.idle.notify_all();
            if queue.stop {
                return;
            }
            let next = queue
                .saves
                .values()
                .map(|(_, at)| at.saturating_duration_since(now))
                .chain(until)
                .min();
            queue = match next {
                Some(wait) => {
                    shared
                        .changed
                        .wait_timeout(queue, wait)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
                None => shared
                    .changed
                    .wait(queue)
                    .unwrap_or_else(PoisonError::into_inner),
            };
            continue;
        }
        queue.busy = true;
        let jobs: Vec<Job> = queue.jobs.drain(..).collect();
        let saves = due
            .into_iter()
            .filter_map(|key| {
                let (saved, _) = queue.saves.remove(&key)?;
                Some((key, saved))
            })
            .collect();
        drop(queue);
        let replies = work(shared, jobs, saves);
        let wake = (!replies.is_empty())
            .then(|| {
                shared
                    .wake
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone()
            })
            .flatten();
        queue = shared.lock();
        queue.replies.extend(replies);
        if let Some(wake) = wake {
            drop(queue);
            wake();
            queue = shared.lock();
        }
    }
}

/// Does what was asked, in the order it was asked, then writes the states
/// that are due. Runs on the worker only.
fn work(shared: &Shared, jobs: Vec<Job>, saves: Vec<(ProjectKey, Saved)>) -> Vec<Reply> {
    let root = &shared.root;
    let mut disks = shared.disks.lock().unwrap_or_else(PoisonError::into_inner);
    let mut replies = Vec::new();
    let mut unsaved = BTreeSet::new();
    let mut jobs = jobs.into_iter().peekable();
    while let Some(job) = jobs.next() {
        match job {
            Job::Create => {
                let made = if shared.ephemeral {
                    Err(io::Error::from(io::ErrorKind::Unsupported))
                } else {
                    create(root)
                };
                if let Ok(key) = &made {
                    disks.insert(
                        key.clone(),
                        Disk {
                            writable: true,
                            chat: ChatFile::Absent,
                        },
                    );
                }
                replies.push(Reply::Created(made));
            }
            Job::Load(key) => {
                let (disk, loaded) = load(root, &key, shared.ephemeral);
                disks.insert(key.clone(), disk);
                replies.push(Reply::Loaded {
                    key,
                    loaded: Box::new(loaded),
                });
            }
            Job::Write(key, first) => {
                // What follows for the same chat is written with it.
                let mut batch = vec![first];
                while let Some(Job::Write(next, _)) = jobs.peek()
                    && *next == key
                {
                    if let Some(Job::Write(_, out)) = jobs.next() {
                        batch.push(out);
                    }
                }
                let folder = folder(root, &key);
                let disk = disks
                    .entry(key.clone())
                    .or_insert_with(|| load(root, &key, shared.ephemeral).0);
                if disk.write(&folder, &batch).is_err() {
                    unsaved.insert(key);
                }
            }
            Job::Earlier(key, before) => {
                let read = read_chat(&folder(root, &key).join(CHAT), before, PAGE);
                let (records, more) = match read {
                    Ok(Some(chat)) if chat.ours => (chat.records.into(), chat.more),
                    _ => (Vec::new(), false),
                };
                replies.push(Reply::Earlier { key, records, more });
            }
            Job::Remove(key) => {
                disks.remove(&key);
                if !shared.ephemeral && remove(&folder(root, &key)).is_err() {
                    replies.push(Reply::NotRemoved(key));
                }
            }
            Job::Kept(open) => replies.push(Reply::Kept(kept(root, &open))),
            Job::Context(key, ticket, op) => {
                let folder = folder(root, &key);
                let disk = disks
                    .entry(key.clone())
                    .or_insert_with(|| load(root, &key, shared.ephemeral).0);
                let (result, digest) = context::carry(&folder, disk.writable, op);
                replies.push(Reply::Context {
                    key,
                    ticket,
                    result,
                    digest,
                });
            }
        }
    }
    for (key, saved) in saves {
        let folder = folder(root, &key);
        let disk = disks
            .entry(key.clone())
            .or_insert_with(|| load(root, &key, shared.ephemeral).0);
        if disk.save(&folder, &saved).is_err() {
            unsaved.insert(key);
        }
    }
    replies.extend(unsaved.into_iter().map(Reply::Unsaved));
    replies
}

/// A chat's file as the worker last left it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatFile {
    /// None, or an empty one: what is added begins with the header.
    Absent,
    Open {
        size: u64,
        /// Something follows its header.
        entries: bool,
        /// Its last line has no end, as after a write that was cut short.
        ragged: bool,
    },
}
/// One project's folder as the worker knows it.
struct Disk {
    /// False for a protected project and in a run that saves nothing.
    writable: bool,
    chat: ChatFile,
}
impl Disk {
    fn write(&mut self, folder: &Path, batch: &[Out]) -> io::Result<()> {
        if !self.writable {
            return Ok(());
        }
        let mut lines = String::new();
        for out in batch {
            match out {
                Out::Entry(record) => {
                    lines.push_str(&transcript::line(record));
                    lines.push('\n');
                }
                Out::Archive => {
                    self.append(folder, &std::mem::take(&mut lines))?;
                    self.rotate(folder)?;
                }
            }
        }
        self.append(folder, &lines)
    }
    /// Sets the chat aside as the earlier one, in place of the one before.
    /// Nothing of either is rewritten.
    fn rotate(&mut self, folder: &Path) -> io::Result<()> {
        if let ChatFile::Open { entries: true, .. } = self.chat {
            std::fs::rename(folder.join(CHAT), folder.join(ARCHIVE))?;
            self.chat = ChatFile::Absent;
        }
        Ok(())
    }
    fn append(&mut self, folder: &Path, lines: &str) -> io::Result<()> {
        if lines.is_empty() {
            return Ok(());
        }
        if let ChatFile::Open { size, .. } = self.chat
            && size + lines.len() as u64 > ROTATE
        {
            self.rotate(folder)?;
        }
        let mut text = String::new();
        let before = match self.chat {
            ChatFile::Absent => {
                text.push_str(HEADER);
                text.push('\n');
                0
            }
            ChatFile::Open { size, ragged, .. } => {
                if ragged {
                    text.push('\n');
                }
                size
            }
        };
        text.push_str(lines);
        ensure(folder)?;
        let mut options = std::fs::OpenOptions::new();
        options.append(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let written = options.open(folder.join(CHAT)).and_then(|mut file| {
            file.write_all(text.as_bytes())?;
            file.sync_data()
        });
        self.chat = match &written {
            Ok(()) => ChatFile::Open {
                size: before + text.len() as u64,
                entries: true,
                ragged: false,
            },
            // Nothing got through, or part of a line did. The file says
            // which: a chat that still has nothing begins with its header
            // when it is next added to, and after a line that was cut short
            // the next one must not be joined to it.
            Err(_) => {
                let path = folder.join(CHAT);
                if before == 0 {
                    // It held nothing before this, so nothing of a chat is
                    // lost with the part of its first lines that got in.
                    let _ = std::fs::OpenOptions::new()
                        .write(true)
                        .open(&path)
                        .and_then(|file| file.set_len(0));
                }
                match std::fs::metadata(&path) {
                    Ok(found) if found.is_file() && found.len() > 0 => ChatFile::Open {
                        size: found.len(),
                        entries: true,
                        ragged: true,
                    },
                    _ => ChatFile::Absent,
                }
            }
        };
        written
    }
    fn save(&self, folder: &Path, saved: &Saved) -> io::Result<()> {
        if !self.writable {
            return Ok(());
        }
        let bytes = serde_json::to_vec(saved)?;
        if bytes.len() as u64 > MAX_STATE {
            return Err(io::Error::other("project state too large"));
        }
        ensure(folder)?;
        crate::config::atomic_write(&folder.join(STATE), &bytes)
            .map_err(|error| io::Error::other(error.to_string()))
    }
}

enum State {
    Absent,
    Read(Box<Saved>),
    Newer,
    Unreadable,
    /// Not what this build writes although it says so, or not JSON at all.
    Damaged(Vec<u8>),
}
fn read_state(path: &Path) -> State {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return State::Absent,
        Err(_) => return State::Unreadable,
    };
    let mut bytes = Vec::new();
    match file.take(MAX_STATE + 1).read_to_end(&mut bytes) {
        Ok(read) if read as u64 <= MAX_STATE => {}
        _ => return State::Unreadable,
    }
    #[derive(serde::Deserialize)]
    struct Versioned {
        version: u64,
    }
    match serde_json::from_slice::<Versioned>(&bytes) {
        Ok(versioned) if versioned.version != u64::from(VERSION) => State::Newer,
        Ok(_) => match serde_json::from_slice::<Saved>(&bytes) {
            Ok(saved) if saved.is_valid() => State::Read(Box::new(saved)),
            _ => State::Damaged(bytes),
        },
        Err(_) => State::Damaged(bytes),
    }
}
/// Keeps the bytes of a damaged state beside it before it is replaced.
fn preserve(folder: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut copy = tempfile::Builder::new()
        .prefix("project-recovery-")
        .suffix(".json")
        .tempfile_in(folder)?;
    copy.write_all(bytes)?;
    copy.as_file().sync_all()?;
    copy.keep().map_err(|error| error.error)?;
    Ok(())
}

/// What was read of a chat's file.
struct Chat {
    /// It begins as a chat of this build does.
    ours: bool,
    file: ChatFile,
    /// The newest entries below the number asked for, oldest first.
    records: VecDeque<Record>,
    /// The highest number of any entry in it.
    last: u64,
    /// Entries older than `records` were passed over.
    more: bool,
}
/// Reads one line of at most `MAX_LINE` bytes into `line`, without its end.
/// A longer one is passed over and comes back empty. Returns whether a line
/// was there and whether it had its end.
fn read_line(reader: &mut impl BufRead, line: &mut Vec<u8>) -> io::Result<Option<bool>> {
    line.clear();
    let limit = MAX_LINE as u64 + 1;
    if reader.by_ref().take(limit).read_until(b'\n', line)? == 0 {
        return Ok(None);
    }
    let mut ended = line.last() == Some(&b'\n');
    if !ended && line.len() as u64 == limit {
        // Too long to be an entry: the rest of it is not kept either.
        line.clear();
        let mut rest = Vec::new();
        loop {
            rest.clear();
            let read = reader.by_ref().take(limit).read_until(b'\n', &mut rest)?;
            ended = rest.last() == Some(&b'\n');
            if read == 0 || ended {
                break;
            }
        }
        return Ok(Some(ended));
    }
    if ended {
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
    }
    Ok(Some(ended))
}
/// Reads the chat at `path`: at most the `keep` newest entries numbered
/// below `before`. Lines that are no entries are passed over. `None` where
/// there is no file.
fn read_chat(path: &Path, before: u64, keep: usize) -> io::Result<Option<Chat>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let size = file.metadata()?.len();
    let mut chat = Chat {
        ours: true,
        file: ChatFile::Absent,
        records: VecDeque::new(),
        last: 0,
        more: false,
    };
    if size == 0 {
        return Ok(Some(chat));
    }
    let mut reader = io::BufReader::new(file);
    let mut line = Vec::new();
    let mut ragged = read_line(&mut reader, &mut line)? == Some(false);
    if line != HEADER.as_bytes() {
        chat.ours = false;
        return Ok(Some(chat));
    }
    if size > MAX_READ {
        // Only its end is read; the line the cut falls in is no entry.
        reader.seek(io::SeekFrom::Start(size - MAX_READ))?;
        read_line(&mut reader, &mut line)?;
        chat.more = true;
    }
    let mut entries = false;
    while let Some(ended) = read_line(&mut reader, &mut line)? {
        ragged = !ended;
        entries = true;
        let Some(record) = std::str::from_utf8(&line).ok().and_then(transcript::parse) else {
            continue;
        };
        chat.last = chat.last.max(record.seq);
        if record.seq >= before {
            continue;
        }
        if chat.records.len() == keep {
            chat.records.pop_front();
            chat.more = true;
        }
        chat.records.push_back(record);
    }
    chat.file = ChatFile::Open {
        size,
        entries,
        ragged,
    };
    Ok(Some(chat))
}

/// Reads a project back and decides whether it may be written.
fn load(root: &Path, key: &ProjectKey, ephemeral: bool) -> (Disk, Loaded) {
    let folder = folder(root, key);
    let mut loaded = Loaded::default();
    let mut damaged = None;
    match read_state(&folder.join(STATE)) {
        State::Absent => {}
        State::Read(saved) => loaded.saved = Some(*saved),
        State::Newer => loaded.protection = Some(Protection::Newer),
        State::Unreadable => loaded.protection = Some(Protection::Unreadable),
        State::Damaged(bytes) => damaged = Some(bytes),
    }
    let mut file = ChatFile::Absent;
    match read_chat(&folder.join(CHAT), u64::MAX, MAX_ENTRIES) {
        Ok(None) => {}
        Ok(Some(chat)) if chat.ours => {
            file = chat.file;
            loaded.records = chat.records.into();
            loaded.last = chat.last;
            loaded.more = chat.more;
        }
        Ok(Some(_)) => {
            loaded.protection.get_or_insert(Protection::Chat);
        }
        Err(_) => {
            loaded.protection.get_or_insert(Protection::Unreadable);
        }
    }
    if loaded.last == 0 {
        // A chat that was just set aside: its numbers go on.
        if let Ok(Some(earlier)) = read_chat(&folder.join(ARCHIVE), u64::MAX, 0)
            && earlier.ours
        {
            loaded.last = earlier.last;
        }
    }
    // A damaged state is replaced only once its bytes are safe beside it.
    // Where nothing is written anyway it stays as it is, and no copy is
    // made each time the project is read.
    if let Some(bytes) = damaged
        && !ephemeral
        && loaded.protection.is_none()
    {
        if preserve(&folder, &bytes).is_ok() {
            loaded.recovered = true;
        } else {
            loaded.protection = Some(Protection::Unreadable);
        }
    }
    let disk = Disk {
        writable: !ephemeral && loaded.protection.is_none(),
        chat: file,
    };
    (disk, loaded)
}

/// Deletes a project's folder. A link in its place is removed, not followed.
fn remove(folder: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(folder) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
        Ok(found) if found.is_dir() => std::fs::remove_dir_all(folder),
        Ok(_) => std::fs::remove_file(folder),
    }
}

/// The projects under `root` that this build reads and that are not `open`.
fn kept(root: &Path, open: &[ProjectKey]) -> Vec<Kept> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten().take(4 * MAX_KEPT) {
        let Some(key) = entry.file_name().to_str().and_then(ProjectKey::parse) else {
            continue;
        };
        if open.contains(&key) || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        if let State::Read(saved) = read_state(&entry.path().join(STATE))
            && !saved.name.trim().is_empty()
            && saved.directory.is_absolute()
            && let Some(lead) = &saved.lead
        {
            found.push(Kept {
                key,
                name: saved.name,
                directory: saved.directory,
                lead: lead.kind,
            });
        }
    }
    found.sort_by(|a, b| a.key.as_str().cmp(b.key.as_str()));
    found.truncate(MAX_KEPT);
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projects::{
        registry::{LeadRef, Task, TaskState},
        transcript::{Entry, Origin, Source},
    };
    use std::sync::mpsc;

    fn key(n: u64) -> ProjectKey {
        ProjectKey::parse(&format!("{n:016x}")).unwrap()
    }
    fn notice(seq: u64, text: &str) -> Out {
        Out::Entry(Record {
            seq,
            at: 10 + seq,
            entry: Entry::Notice { text: text.into() },
        })
    }
    fn write(store: &Store, key: &ProjectKey, out: impl IntoIterator<Item = Out>) {
        let mut out: VecDeque<Out> = out.into_iter().collect();
        // More than the worker takes at once goes in as it makes room.
        for _ in 0..8 {
            store.write(key, &mut out);
            if out.is_empty() {
                return;
            }
            assert!(store.poll().is_empty());
        }
        panic!("the worker never had room");
    }
    /// Loads a project and returns it with every other reply that came.
    fn loaded(store: &Store, key: &ProjectKey) -> (Loaded, Vec<Reply>) {
        assert!(store.load(key));
        let mut others = Vec::new();
        let mut found = None;
        for reply in store.poll() {
            match reply {
                Reply::Loaded { key: of, loaded } if of == *key => found = Some(*loaded),
                other => others.push(other),
            }
        }
        (found.expect("the project was not loaded"), others)
    }
    fn texts(records: &[Record]) -> Vec<String> {
        records
            .iter()
            .map(|record| match &record.entry {
                Entry::Notice { text } => text.clone(),
                other => format!("{other:?}"),
            })
            .collect()
    }
    /// Every file under `root`, by its path from there.
    fn files(root: &Path) -> Vec<String> {
        fn walk(at: &Path, root: &Path, found: &mut Vec<String>) {
            for entry in std::fs::read_dir(at).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, root, found);
                } else {
                    let below = path.strip_prefix(root).unwrap().display().to_string();
                    found.push(below.replace('\\', "/"));
                }
            }
        }
        let mut found = Vec::new();
        walk(root, root, &mut found);
        found.sort();
        found
    }
    fn saved(name: &str) -> Saved {
        Saved {
            name: name.into(),
            directory: "/code/shop".into(),
            lead: Some(LeadRef {
                kind: AgentKind::Claude,
                session: Some("session-1".into()),
                opened: true,
            }),
            tasks: vec![Task {
                n: 12,
                title: "auth".into(),
                kind: AgentKind::Codex,
                session: Some("agent-session".into()),
                cwd: "/code/shop".into(),
                worktree: None,
                pull_requests: vec!["https://example.test/o/r/pull/214".into()],
                state: TaskState::Review,
                last_report: Some("PR #214 is up.".into()),
                started: 100,
                ended: None,
                merged: false,
                model: None,
                effort: None,
                ultracode: false,
            }],
            ..Saved::default()
        }
    }

    #[test]
    fn a_new_project_gets_a_fresh_private_folder_named_by_its_key() {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("projects");
        let first = create(&root).unwrap();
        let second = create(&root).unwrap();
        assert_ne!(first, second);
        for key in [&first, &second] {
            assert!(folder(&root, key).is_dir());
            assert_eq!(ProjectKey::parse(key.as_str()).as_ref(), Some(key));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [root.clone(), folder(&root, &first)] {
                let mode = std::fs::metadata(path).unwrap().permissions().mode();
                assert_eq!(mode & 0o777, 0o700);
            }
        }
        // A folder that is there is left as it is; one that went is remade.
        std::fs::write(folder(&root, &first).join("kept"), "x").unwrap();
        ensure(&folder(&root, &first)).unwrap();
        assert!(folder(&root, &first).join("kept").exists());
        std::fs::remove_dir_all(folder(&root, &second)).unwrap();
        ensure(&folder(&root, &second)).unwrap();
        assert!(folder(&root, &second).is_dir());

        // Through the worker, the same: the result arrives with a wake.
        let store = Store::new(root.clone(), false);
        let (woke, woken) = mpsc::channel();
        store.woken_by(|| {
            Arc::new(move || {
                let _ = woke.send(());
            })
        });
        assert!(store.poll().is_empty());
        assert!(store.create());
        woken.recv_timeout(Duration::from_secs(10)).unwrap();
        let replies = store.poll();
        let [Reply::Created(Ok(third))] = &replies[..] else {
            panic!("{replies:?}");
        };
        assert!(folder(&root, third).is_dir());
        assert_eq!(files(&folder(&root, third)), Vec::<String>::new());
        // A root that cannot be a folder is an error, not a panic.
        std::fs::write(data.path().join("file"), "x").unwrap();
        assert!(create(&data.path().join("file")).is_err());
        let broken = Store::inline(data.path().join("file"));
        assert!(broken.create());
        assert!(matches!(broken.poll()[..], [Reply::Created(Err(_))]));
    }

    #[test]
    fn a_projects_state_and_chat_come_back_as_they_were_written() {
        let data = tempfile::tempdir().unwrap();
        let store = Store::inline(data.path().into());
        let key = key(1);
        // Nothing is there, and reading makes nothing.
        let (empty, _) = loaded(&store, &key);
        assert!(empty.saved.is_none() && empty.records.is_empty() && empty.protection.is_none());
        assert_eq!((empty.last, empty.more, empty.recovered), (0, false, false));
        assert!(files(data.path()).is_empty());

        let entries = [
            Entry::User {
                text: "go\nnow".into(),
                attachments: Vec::new(),
            },
            Entry::Turn {
                id: "turn-1".into(),
                origin: Origin::User,
            },
            Entry::Lead {
                text: "A plan — with “quotes”.".into(),
            },
            Entry::Tool {
                name: "spawn_agent".into(),
                summary: "Started agent 12 · Codex · auth".into(),
                ok: true,
            },
            Entry::Event {
                source: Source::Agent,
                agent: Some(12),
                what: "“auth” finished its turn".into(),
                text: "PR #214 is up.".into(),
            },
        ];
        let records: Vec<Record> = entries
            .into_iter()
            .enumerate()
            .map(|(n, entry)| Record {
                seq: n as u64 + 1,
                at: 50,
                entry,
            })
            .collect();
        write(&store, &key, records.iter().cloned().map(Out::Entry));
        store.save(&key, saved("shop"));
        assert!(store.poll().is_empty());
        assert_eq!(
            files(data.path()),
            [
                format!("{}/chat.jsonl", key.as_str()),
                format!("{}/project.json", key.as_str())
            ]
        );
        let chat = std::fs::read_to_string(folder(data.path(), &key).join(CHAT)).unwrap();
        let mut lines = chat.lines();
        assert_eq!(lines.next(), Some(HEADER));
        assert_eq!(
            lines.next(),
            Some(r#"{"seq":1,"at":50,"kind":"user","text":"go\nnow"}"#)
        );
        assert_eq!(chat.lines().count(), 6);
        assert!(chat.ends_with('\n'));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for name in [CHAT, STATE] {
                let mode = std::fs::metadata(folder(data.path(), &key).join(name))
                    .unwrap()
                    .permissions()
                    .mode();
                assert_eq!(mode & 0o777, 0o600, "{name}");
            }
        }

        // Another run reads the same back.
        let again = Store::inline(data.path().into());
        let (back, _) = loaded(&again, &key);
        assert_eq!(back.saved, Some(saved("shop")));
        assert_eq!(back.records, records);
        assert_eq!((back.last, back.more), (5, false));
        assert!(back.protection.is_none() && !back.recovered);
        // More is added to the end, and the newest state wins.
        write(&again, &key, [notice(6, "later")]);
        again.save(&key, saved("first"));
        again.save(&key, saved("checkout"));
        assert!(again.poll().is_empty());
        let (back, _) = loaded(&Store::inline(data.path().into()), &key);
        assert_eq!(back.saved.unwrap().name, "checkout");
        assert_eq!(back.records.len(), 6);
        assert_eq!(back.records[..5], records[..]);
        assert_eq!(
            std::fs::read_to_string(folder(data.path(), &key).join(CHAT))
                .unwrap()
                .matches(HEADER)
                .count(),
            1
        );
        // Nothing but the two files was ever made beside them.
        assert_eq!(files(data.path()).len(), 2);
    }

    #[test]
    fn memory_gets_the_newest_entries_and_older_ones_are_read_when_asked_for() {
        let data = tempfile::tempdir().unwrap();
        let store = Store::inline(data.path().into());
        let key = key(2);
        let total = (MAX_ENTRIES + PAGE + 50) as u64;
        write(
            &store,
            &key,
            (1..=total).map(|seq| notice(seq, &seq.to_string())),
        );
        let (back, _) = loaded(&store, &key);
        assert_eq!(back.records.len(), MAX_ENTRIES);
        assert_eq!(back.records[0].seq, total - MAX_ENTRIES as u64 + 1);
        assert_eq!((back.last, back.more), (total, true));
        // A page of older ones, then the rest, then nothing.
        assert!(store.earlier(&key, back.records[0].seq));
        let replies = store.poll();
        let [Reply::Earlier { records, more, .. }] = &replies[..] else {
            panic!("{replies:?}");
        };
        assert_eq!(records.len(), PAGE);
        assert_eq!(
            (records[0].seq, records[PAGE - 1].seq, *more),
            (51, 250, true)
        );
        assert!(store.earlier(&key, 51));
        let replies = store.poll();
        let [Reply::Earlier { records, more, .. }] = &replies[..] else {
            panic!("{replies:?}");
        };
        assert_eq!((records.len(), records[0].seq, *more), (50, 1, false));
        assert!(store.earlier(&key, 1));
        assert!(matches!(
            &store.poll()[..],
            [Reply::Earlier { records, more: false, .. }] if records.is_empty()
        ));
    }

    #[test]
    fn a_long_chat_is_set_aside_once_and_a_new_chat_sets_it_aside_at_once() {
        let data = tempfile::tempdir().unwrap();
        let store = Store::inline(data.path().into());
        let key = key(3);
        let folder = folder(data.path(), &key);
        // An entry is saved within one line's limit however long it is.
        let long = "é\"\n".repeat(MAX_LINE);
        write(&store, &key, [notice(1, &long)]);
        store.poll();
        let chat = std::fs::read_to_string(folder.join(CHAT)).unwrap();
        let line = chat.lines().nth(1).unwrap();
        assert!(line.len() <= MAX_LINE && line.len() > MAX_LINE / 2);
        assert!(transcript::parse(line).is_some());
        // Filled to its limit, the chat becomes the earlier one and the
        // next entry begins a new file. Nothing in either is rewritten.
        let filler = "x".repeat(60 * 1024);
        let mut seq = 1;
        while std::fs::metadata(folder.join(CHAT)).unwrap().len() + 62 * 1024 <= ROTATE {
            seq += 1;
            write(&store, &key, [notice(seq, &filler)]);
            store.poll();
        }
        assert!(!folder.join(ARCHIVE).exists());
        let before = std::fs::read(folder.join(CHAT)).unwrap();
        write(
            &store,
            &key,
            [
                notice(seq + 1, &filler),
                notice(seq + 2, &filler),
                notice(seq + 3, "after"),
            ],
        );
        store.poll();
        assert_eq!(std::fs::read(folder.join(ARCHIVE)).unwrap(), before);
        let chat = std::fs::read_to_string(folder.join(CHAT)).unwrap();
        assert_eq!(chat.lines().count(), 4);
        assert!(chat.starts_with(HEADER) && chat.len() as u64 <= ROTATE);
        // What is read back is the chat, and its numbers go on.
        let (back, _) = loaded(&Store::inline(data.path().into()), &key);
        assert_eq!(back.records.len(), 3);
        assert_eq!((back.last, back.more), (seq + 3, false));

        // A new chat: the one so far is the earlier one, in place of the
        // one before, and nothing is in the way of the next.
        write(
            &store,
            &key,
            [notice(seq + 4, "last of the old"), Out::Archive],
        );
        store.poll();
        assert!(!folder.join(CHAT).exists());
        let archive = std::fs::read_to_string(folder.join(ARCHIVE)).unwrap();
        assert_eq!(archive.lines().count(), 5);
        assert!(archive.contains("last of the old"));
        let (back, _) = loaded(&Store::inline(data.path().into()), &key);
        assert!(back.records.is_empty());
        assert_eq!(back.last, seq + 4, "numbers go on from the earlier chat");
        write(
            &store,
            &key,
            [
                notice(seq + 5, "first of the new"),
                Out::Archive,
                Out::Archive,
            ],
        );
        store.poll();
        assert!(
            std::fs::read_to_string(folder.join(ARCHIVE))
                .unwrap()
                .contains("first of the new")
        );
        // An empty chat is not set aside over one that has something.
        write(&store, &key, [Out::Archive]);
        store.poll();
        assert!(
            std::fs::read_to_string(folder.join(ARCHIVE))
                .unwrap()
                .contains("first of the new")
        );
    }

    /// The bytes of every file under `root`.
    fn snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
        files(root)
            .into_iter()
            .map(|name| {
                let bytes = std::fs::read(root.join(&name)).unwrap();
                (name, bytes)
            })
            .collect()
    }
    /// Asks for everything that writes and returns what the worker said.
    fn try_to_write(store: &Store, key: &ProjectKey) -> Vec<Reply> {
        write(
            store,
            key,
            [notice(900, "must not be written"), Out::Archive],
        );
        write(store, key, [notice(901, "nor this")]);
        store.save(key, saved("must not be written"));
        store.poll()
    }

    #[test]
    fn a_project_this_build_cannot_read_is_shown_and_never_written() {
        let data = tempfile::tempdir().unwrap();
        let chat = format!(
            "{HEADER}\n{}\n",
            r#"{"seq":1,"at":1,"kind":"notice","text":"kept"}"#
        );
        let cases: [(&str, Vec<u8>, &str, Protection); 4] = [
            (
                "a later format",
                br#"{"version":2,"lead":null,"plans":[]}"#.to_vec(),
                &chat,
                Protection::Newer,
            ),
            (
                "an earlier one that never was",
                br#"{"version":0}"#.to_vec(),
                &chat,
                Protection::Newer,
            ),
            (
                "too large",
                {
                    let mut bytes = br#"{"version":1,"name":""#.to_vec();
                    bytes.resize(MAX_STATE as usize + 1, b'x');
                    bytes
                },
                &chat,
                Protection::Unreadable,
            ),
            (
                "a chat of another format",
                serde_json::to_vec(&saved("shop")).unwrap(),
                "{\"neptune_chat\":2}\n{\"seq\":1,\"at\":1,\"kind\":\"notice\",\"text\":\"x\"}\n",
                Protection::Chat,
            ),
        ];
        for (n, (what, state, chat, protection)) in cases.into_iter().enumerate() {
            let key = key(10 + n as u64);
            let folder = folder(data.path(), &key);
            ensure(&folder).unwrap();
            std::fs::write(folder.join(STATE), &state).unwrap();
            std::fs::write(folder.join(CHAT), chat).unwrap();
            std::fs::write(folder.join(ARCHIVE), "earlier").unwrap();
            let before = snapshot(&folder);
            let store = Store::inline(data.path().into());
            let (back, _) = loaded(&store, &key);
            assert_eq!(back.protection, Some(protection), "{what}");
            assert!(!back.recovered, "{what}");
            // What could be read is shown.
            if protection != Protection::Chat {
                assert_eq!(texts(&back.records), ["kept"], "{what}");
            } else {
                assert!(back.records.is_empty() && back.saved.is_some());
            }
            let replies = try_to_write(&store, &key);
            assert!(replies.is_empty(), "{what}: {replies:?}");
            assert_eq!(snapshot(&folder), before, "{what}");
            // Also by a store that was never asked to load it.
            let unasked = Store::inline(data.path().into());
            assert!(try_to_write(&unasked, &key).is_empty());
            assert_eq!(snapshot(&folder), before, "{what}");
        }
        // A state that cannot be opened at all, the same.
        let key = key(20);
        let folder = folder(data.path(), &key);
        ensure(&folder.join(STATE)).unwrap();
        let store = Store::inline(data.path().into());
        let (back, _) = loaded(&store, &key);
        assert_eq!(back.protection, Some(Protection::Unreadable));
        assert!(try_to_write(&store, &key).is_empty());
        assert!(files(&folder).is_empty() && folder.join(STATE).is_dir());
        // One protected project does not hold back another.
        let other = self::key(21);
        write(&store, &other, [notice(1, "fine")]);
        store.save(&other, saved("other"));
        assert!(store.poll().is_empty());
        let (back, _) = loaded(&Store::inline(data.path().into()), &other);
        assert_eq!(texts(&back.records), ["fine"]);
        assert_eq!(back.saved.unwrap().name, "other");
    }

    #[test]
    fn a_damaged_state_is_copied_aside_before_it_is_replaced() {
        let data = tempfile::tempdir().unwrap();
        for (n, damaged) in [
            &b"{\"version\":1,\"tasks\":"[..],
            b"not json at all",
            br#"{"version":1,"chat":["a field this build does not know"]}"#,
            br#"{"version":1,"tasks":[{"n":0,"title":"x","kind":"claude","cwd":"/","state":"ended","started":1}]}"#,
            br#"{"version":"one"}"#,
        ]
        .into_iter()
        .enumerate()
        {
            let key = key(30 + n as u64);
            let folder = folder(data.path(), &key);
            ensure(&folder).unwrap();
            std::fs::write(folder.join(STATE), damaged).unwrap();
            let store = Store::inline(data.path().into());
            let (back, _) = loaded(&store, &key);
            assert!(back.recovered && back.protection.is_none() && back.saved.is_none());
            let copies: Vec<String> = files(&folder)
                .into_iter()
                .filter(|name| name.starts_with("project-recovery-") && name.ends_with(".json"))
                .collect();
            assert_eq!(copies.len(), 1, "{n}");
            assert_eq!(std::fs::read(folder.join(&copies[0])).unwrap(), damaged);
            // Not replaced by reading; replaced by the next state.
            assert_eq!(std::fs::read(folder.join(STATE)).unwrap(), damaged);
            store.save(&key, saved("again"));
            assert!(store.poll().is_empty());
            assert_eq!(
                serde_json::from_slice::<Saved>(&std::fs::read(folder.join(STATE)).unwrap())
                    .unwrap(),
                saved("again")
            );
            assert_eq!(std::fs::read(folder.join(&copies[0])).unwrap(), damaged);
        }
        // A project that is not written for another reason keeps its damaged
        // state as it is: no copy is put beside it, however often it is read.
        {
            let key = key(39);
            let folder = folder(data.path(), &key);
            ensure(&folder).unwrap();
            std::fs::write(folder.join(STATE), "broken").unwrap();
            std::fs::write(folder.join(CHAT), "{\"neptune_chat\":2}\n").unwrap();
            let before = snapshot(&folder);
            let store = Store::inline(data.path().into());
            for _ in 0..3 {
                let (back, _) = loaded(&store, &key);
                assert_eq!(back.protection, Some(Protection::Chat));
                assert!(!back.recovered && back.saved.is_none());
                assert!(try_to_write(&store, &key).is_empty());
            }
            assert_eq!(snapshot(&folder), before);
        }
        // Where the copy cannot be made, the original stays and nothing is
        // written.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let key = key(40);
            let folder = folder(data.path(), &key);
            ensure(&folder).unwrap();
            std::fs::write(folder.join(STATE), "broken").unwrap();
            std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o500)).unwrap();
            // A user who may write anywhere cannot be refused a copy.
            if std::fs::write(folder.join("probe"), "x").is_err() {
                let store = Store::inline(data.path().into());
                let (back, _) = loaded(&store, &key);
                assert_eq!(back.protection, Some(Protection::Unreadable));
                assert!(!back.recovered);
                assert!(try_to_write(&store, &key).is_empty());
                assert_eq!(files(&folder), [STATE]);
                assert_eq!(std::fs::read(folder.join(STATE)).unwrap(), b"broken");
            }
            std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    #[test]
    fn lines_that_are_no_entries_are_passed_over_and_stay_where_they_are() {
        let data = tempfile::tempdir().unwrap();
        let key = key(50);
        let folder = folder(data.path(), &key);
        ensure(&folder).unwrap();
        let entry = |seq: u64, text: &str| {
            format!(r#"{{"seq":{seq},"at":1,"kind":"notice","text":"{text}"}}"#)
        };
        // Garbage, an entry of a kind this build does not know, a line far
        // too long, and a last line that was cut short as it was written.
        let original = format!(
            "{HEADER}\r\n{}\nnot json\n{}\n{}\n\n{}\n{}",
            entry(1, "one"),
            r#"{"seq":2,"at":1,"kind":"hologram","text":"later"}"#,
            "y".repeat(3 * MAX_LINE),
            entry(4, "four"),
            r#"{"seq":5,"at":1,"kind":"notice","te"#,
        );
        std::fs::write(folder.join(CHAT), &original).unwrap();
        let store = Store::inline(data.path().into());
        let (back, _) = loaded(&store, &key);
        assert_eq!(back.protection, None);
        assert_eq!(texts(&back.records), ["one", "four"]);
        assert_eq!(back.last, 4);
        // What is added follows on a line of its own; not a byte before it
        // changed.
        write(&store, &key, [notice(6, "six")]);
        assert!(store.poll().is_empty());
        let now = std::fs::read_to_string(folder.join(CHAT)).unwrap();
        assert_eq!(
            now,
            format!(
                "{original}\n{}\n",
                r#"{"seq":6,"at":16,"kind":"notice","text":"six"}"#
            )
        );
        let (back, _) = loaded(&Store::inline(data.path().into()), &key);
        assert_eq!(texts(&back.records), ["one", "four", "six"]);
        // An empty file is a chat that has not begun.
        let empty = self::key(51);
        ensure(&self::folder(data.path(), &empty)).unwrap();
        std::fs::write(self::folder(data.path(), &empty).join(CHAT), "").unwrap();
        let (back, _) = loaded(&store, &empty);
        assert!(back.protection.is_none() && back.records.is_empty());
        write(&store, &empty, [notice(1, "first")]);
        store.poll();
        assert!(
            std::fs::read_to_string(self::folder(data.path(), &empty).join(CHAT))
                .unwrap()
                .starts_with(&format!("{HEADER}\n{{\"seq\":1,"))
        );
    }

    #[test]
    fn a_write_that_fails_is_said_and_the_chat_still_begins_as_one() {
        let data = tempfile::tempdir().unwrap();
        let store = Store::inline(data.path().into());
        let key = key(55);
        let folder = folder(data.path(), &key);
        // Something is in the way of a chat that has not begun.
        ensure(&folder.join(CHAT)).unwrap();
        let mut disk = Disk {
            writable: true,
            chat: ChatFile::Absent,
        };
        assert!(disk.write(&folder, &[notice(1, "lost")]).is_err());
        assert_eq!(disk.chat, ChatFile::Absent);
        // Out of the way again, what is added next begins a chat this
        // build reads, not a file without its first line.
        std::fs::remove_dir(folder.join(CHAT)).unwrap();
        disk.write(&folder, &[notice(2, "kept")]).unwrap();
        let chat = std::fs::read_to_string(folder.join(CHAT)).unwrap();
        assert!(
            chat.starts_with(&format!("{HEADER}\n{{\"seq\":2,")),
            "{chat}"
        );
        let (back, _) = loaded(&store, &key);
        assert_eq!(back.protection, None);
        assert_eq!(texts(&back.records), ["kept"]);
        // The worker says so, once for the project, and goes on.
        let third = self::key(57);
        write(&store, &third, [notice(1, "one")]);
        assert!(store.poll().is_empty());
        std::fs::remove_file(self::folder(data.path(), &third).join(CHAT)).unwrap();
        ensure(&self::folder(data.path(), &third).join(CHAT)).unwrap();
        write(&store, &third, [notice(2, "two"), notice(3, "three")]);
        let replies = store.poll();
        assert!(
            matches!(&replies[..], [Reply::Unsaved(of)] if *of == third),
            "{replies:?}"
        );
    }

    #[test]
    fn a_run_that_saves_nothing_writes_nothing() {
        let data = tempfile::tempdir().unwrap();
        let key = key(60);
        // With nothing there, nothing is made: not even the folder.
        let store = Store::new(data.path().join("projects"), true);
        let (woke, woken) = mpsc::channel();
        store.woken_by(|| {
            Arc::new(move || {
                let _ = woke.send(());
            })
        });
        assert!(store.create());
        assert!(store.load(&key));
        write(
            &store,
            &key,
            [notice(1, "said"), Out::Archive, notice(2, "more")],
        );
        store.save(&key, saved("shop"));
        assert!(store.kept(Vec::new()));
        assert!(store.remove(&key));
        store.flush(Duration::from_secs(10));
        woken.recv_timeout(Duration::from_secs(10)).unwrap();
        let replies = store.poll();
        assert!(matches!(replies[0], Reply::Created(Err(_))), "{replies:?}");
        assert!(
            replies
                .iter()
                .all(|reply| !matches!(reply, Reply::Unsaved(_) | Reply::NotRemoved(_)))
        );
        assert!(!data.path().join("projects").exists());
        // With a project there, damaged at that, not a byte of it changes
        // and nothing is put beside it.
        let root = data.path().join("kept");
        let folder = folder(&root, &key);
        ensure(&folder).unwrap();
        std::fs::write(folder.join(STATE), "damaged").unwrap();
        std::fs::write(folder.join(CHAT), format!("{HEADER}\n")).unwrap();
        let before = snapshot(&root);
        let store = Store::new(root.clone(), true);
        assert!(store.load(&key));
        write(&store, &key, [notice(1, "said"), Out::Archive]);
        store.save(&key, saved("shop"));
        assert!(store.remove(&key));
        store.flush(Duration::from_secs(10));
        assert_eq!(snapshot(&root), before);
    }

    #[test]
    fn the_worker_writes_a_run_of_changes_once_and_everything_before_it_ends() {
        let data = tempfile::tempdir().unwrap();
        let key = key(70);
        let state = folder(data.path(), &key).join(STATE);
        let store = Store::new(data.path().into(), false);
        // A run of changes is one write, a moment after the first.
        store.save(&key, saved("one"));
        store.save(&key, saved("two"));
        assert!(!state.exists(), "written before it was due");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !state.exists() {
            assert!(Instant::now() < deadline, "never written");
            thread::sleep(Duration::from_millis(5));
        }
        let name = |path: &Path| {
            serde_json::from_slice::<Saved>(&std::fs::read(path).unwrap())
                .unwrap()
                .name
        };
        assert_eq!(name(&state), "two");
        // Closing does not wait for what is due later.
        store.save(&key, saved("three"));
        write(&store, &key, (1..=300).map(|seq| notice(seq, "x")));
        store.flush(Duration::from_secs(10));
        assert_eq!(name(&state), "three");
        let chat = std::fs::read_to_string(folder(data.path(), &key).join(CHAT)).unwrap();
        assert_eq!(chat.lines().count(), 301);
        // A store that is let go still writes what it was handed.
        store.save(&key, saved("four"));
        write(&store, &key, [notice(301, "last")]);
        drop(store);
        let deadline = Instant::now() + Duration::from_secs(10);
        while name(&state) != "four" {
            assert!(Instant::now() < deadline, "the last state was lost");
            thread::sleep(Duration::from_millis(5));
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while !std::fs::read_to_string(folder(data.path(), &key).join(CHAT))
            .unwrap()
            .contains("last")
        {
            assert!(Instant::now() < deadline, "the last entry was lost");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn the_clock_says_once_when_a_watch_is_due_and_never_before() {
        let dir = tempfile::tempdir().unwrap();
        let now = || crate::projects::schedule::seconds(SystemTime::now());
        let fired = |replies: Vec<Reply>| -> Vec<(ProjectKey, u64, u64)> {
            replies
                .into_iter()
                .filter_map(|reply| match reply {
                    Reply::Fired { key, id, due } => Some((key, id, due)),
                    _ => None,
                })
                .collect()
        };
        // Sums over times that are handed in: what is due is said and let
        // go of, what is not stays, and the next look is when it is due.
        let mut queue = Queue::default();
        queue.clock.insert(key(1), vec![(3, 100), (4, 160)]);
        queue.clock.insert(key(2), vec![(1, 100)]);
        let at = |seconds| std::time::UNIX_EPOCH + Duration::from_secs(seconds);
        assert_eq!(
            strike(&mut queue, at(99)),
            (false, Some(Duration::from_secs(1)))
        );
        assert!(queue.replies.is_empty());
        assert_eq!(
            strike(&mut queue, at(100)),
            (true, Some(Duration::from_secs(60)))
        );
        assert_eq!(
            fired(std::mem::take(&mut queue.replies)),
            [(key(1), 3, 100), (key(2), 1, 100)]
        );
        assert_eq!(queue.clock.get(&key(1)), Some(&vec![(4, 160)]));
        assert!(!queue.clock.contains_key(&key(2)));
        // Said once: asked again at the same time, nothing more is due.
        assert!(!strike(&mut queue, at(100)).0);
        // Seen late, it is said late, with the time it was set for.
        assert_eq!(strike(&mut queue, at(5000)), (true, None));
        assert_eq!(
            fired(std::mem::take(&mut queue.replies)),
            [(key(1), 4, 160)]
        );
        // While no frame takes what was said, no more piles up: the rest
        // stays on the clock and is looked at again soon.
        for n in 0..(MAX_FIRED as u64 + 5) {
            queue
                .clock
                .entry(key(1 + n / 16))
                .or_default()
                .push((n, 10));
        }
        assert_eq!(
            strike(&mut queue, at(20)),
            (true, Some(Duration::from_secs(1)))
        );
        assert_eq!(queue.replies.len(), MAX_FIRED);
        assert_eq!(queue.clock.values().map(Vec::len).sum::<usize>(), 5);
        queue.replies.clear();
        assert_eq!(strike(&mut queue, at(20)), (true, None));
        assert_eq!(queue.replies.len(), 5);

        // The worker: one that is due is said at once, one for later is
        // not, and a project's times replace the ones it had.
        let store = Store::new(dir.path().into(), false);
        let (woken, wakes) = mpsc::channel();
        store.woken_by(|| {
            Arc::new(move || {
                let _ = woken.send(());
            })
        });
        let soon = now() + 3600;
        store.schedule(&key(1), vec![(7, now() + 7200)]);
        store.schedule(&key(1), vec![(7, now().saturating_sub(5)), (8, soon)]);
        wakes
            .recv_timeout(Duration::from_secs(10))
            .expect("the clock never struck");
        let said = fired(store.poll());
        assert_eq!(said.len(), 1);
        assert_eq!((&said[0].0, said[0].1), (&key(1), 7));
        assert!(wakes.recv_timeout(Duration::from_millis(150)).is_err());
        assert!(store.poll().is_empty());
        // Set anew, the one that waits strikes; a project that is removed
        // or has no times left is off the clock.
        store.schedule(&key(1), vec![(8, now())]);
        wakes
            .recv_timeout(Duration::from_secs(10))
            .expect("the clock never struck again");
        assert_eq!(fired(store.poll()).len(), 1);
        store.schedule(&key(2), vec![(1, soon)]);
        store.schedule(&key(3), vec![(1, soon)]);
        store.schedule(&key(2), Vec::new());
        assert!(store.remove(&key(3)));
        store.flush(Duration::from_secs(5));
        assert!(store.shared.lock().clock.is_empty());
        // More times than a project has watches are not kept.
        store.schedule(&key(4), (0..40).map(|id| (id, soon)).collect());
        assert_eq!(store.shared.lock().clock[&key(4)].len(), MAX_TIMES);

        // A store without a worker strikes as its results are taken.
        let inline = Store::inline(dir.path().join("inline"));
        inline.schedule(&key(1), vec![(1, now()), (2, soon)]);
        assert_eq!(fired(inline.poll()).len(), 1);
        assert!(fired(inline.poll()).is_empty());
    }

    #[test]
    fn a_projects_watches_come_back_as_they_were_kept() {
        use crate::projects::subscription::{self, New, Trigger};
        let dir = tempfile::tempdir().unwrap();
        let store = Store::inline(dir.path().into());
        let mut state = saved("shop");
        let at = std::time::UNIX_EPOCH + Duration::from_secs(1_790_000_000);
        for (title, trigger, allowed) in [
            ("Nightly", Trigger::Interval { minutes: 60 }, true),
            ("Proposed", Trigger::Interval { minutes: 15 }, false),
            (
                "Review",
                Trigger::PullRequest {
                    url: "https://github.com/zevem/neptune/pull/83".into(),
                    auto: true,
                },
                true,
            ),
        ] {
            subscription::add(
                &mut state.subscriptions,
                &mut state.counters.watches,
                New {
                    title: title.into(),
                    trigger,
                    instruction: "Check the build.".into(),
                    allowed,
                },
                at,
            )
            .unwrap();
        }
        state.subscriptions[0].fired(1_790_003_600, 1_790_003_601, true);
        state.subscriptions[2].seen = Some(subscription::Seen {
            state: subscription::Review::Open,
            checks: subscription::Checks::Failing,
            unresolved: 2,
        });
        store.save(&key(1), state.clone());
        assert!(store.poll().is_empty());
        let (read, _) = loaded(&store, &key(1));
        assert_eq!(read.saved, Some(state.clone()));
        assert_eq!(read.protection, None);
        // A watch this build would not have written makes the file damaged:
        // it is copied aside, and the project goes on without it.
        let path = folder(dir.path(), &key(1)).join(STATE);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(r#""minutes":60"#));
        std::fs::write(&path, text.replace(r#""minutes":60"#, r#""minutes":1"#)).unwrap();
        let (read, _) = loaded(&store, &key(1));
        assert!(read.saved.is_none() && read.recovered);
    }

    #[test]
    fn a_full_queue_takes_no_more_and_loses_nothing() {
        let data = tempfile::tempdir().unwrap();
        let key = key(80);
        // Nothing takes work from a store whose results nobody asks for.
        let store = Store::inline(data.path().into());
        let mut out: VecDeque<Out> = (1..=MAX_JOBS as u64 + 40)
            .map(|seq| notice(seq, "x"))
            .collect();
        store.write(&key, &mut out);
        assert_eq!(out.len(), 40, "what did not fit stays with its owner");
        assert!(!store.load(&key) && !store.earlier(&key, 5) && !store.create());
        assert!(!store.kept(Vec::new()) && !store.remove(&key));
        // The state of a project is one slot, however full the queue is.
        store.save(&key, saved("shop"));
        assert!(store.poll().is_empty());
        store.write(&key, &mut out);
        assert!(out.is_empty());
        store.poll();
        let (back, _) = loaded(&store, &key);
        assert_eq!(back.last, MAX_JOBS as u64 + 40);
        assert_eq!(back.records.len(), MAX_ENTRIES);
        let chat = std::fs::read_to_string(folder(data.path(), &key).join(CHAT)).unwrap();
        // In order, each once.
        let numbers: Vec<u64> = chat
            .lines()
            .skip(1)
            .map(|line| transcript::parse(line).unwrap().seq)
            .collect();
        assert_eq!(numbers, (1..=MAX_JOBS as u64 + 40).collect::<Vec<_>>());
    }

    #[test]
    #[cfg_attr(windows, ignore = "its directories are written as on Unix")]
    fn a_removed_project_leaves_nothing_and_a_kept_one_is_offered_again() {
        let data = tempfile::tempdir().unwrap();
        let store = Store::inline(data.path().into());
        let (open, closed, removed, newer) = (key(90), key(91), key(92), key(93));
        for (key, name) in [(&open, "open"), (&closed, "shop"), (&removed, "old")] {
            write(&store, key, [notice(1, "said")]);
            store.save(key, saved(name));
        }
        store.poll();
        // What this build cannot read is not offered, and neither is what
        // is no project's folder.
        ensure(&folder(data.path(), &newer)).unwrap();
        std::fs::write(folder(data.path(), &newer).join(STATE), r#"{"version":9}"#).unwrap();
        ensure(&data.path().join("not-a-project")).unwrap();
        std::fs::write(data.path().join("00000000000000ff"), "a file").unwrap();
        let context = folder(data.path(), &removed).join("context");
        ensure(&context).unwrap();
        std::fs::write(context.join("STATUS.md"), "notes").unwrap();

        // Its state is not written into a folder that goes.
        store.save(&removed, saved("old, renamed"));
        assert!(store.remove(&removed));
        assert!(store.kept(vec![open.clone()]));
        let replies = store.poll();
        let [Reply::Kept(kept)] = &replies[..] else {
            panic!("{replies:?}");
        };
        assert_eq!(
            kept[..],
            [Kept {
                key: closed.clone(),
                name: "shop".into(),
                directory: "/code/shop".into(),
                lead: AgentKind::Claude,
            }]
        );
        assert!(!folder(data.path(), &removed).exists());
        assert!(folder(data.path(), &open).is_dir() && folder(data.path(), &closed).is_dir());
        // Removing what is gone already is no failure.
        assert!(store.remove(&removed));
        assert!(store.poll().is_empty());
        // A link in a project's place is removed; where it leads is not.
        #[cfg(unix)]
        {
            let elsewhere = data.path().join("elsewhere");
            ensure(&elsewhere).unwrap();
            std::fs::write(elsewhere.join("precious"), "x").unwrap();
            std::os::unix::fs::symlink(&elsewhere, folder(data.path(), &removed)).unwrap();
            assert!(store.remove(&removed));
            assert!(store.poll().is_empty());
            assert!(!folder(data.path(), &removed).exists());
            assert!(elsewhere.join("precious").exists());
        }
    }
    /// Asks something of a project's context and returns what came of it
    /// and what the folder holds afterwards.
    fn asked(store: &Store, key: &ProjectKey, op: ContextOp) -> (Result<String, String>, Digest) {
        assert!(store.context(key, 9, op));
        store
            .poll()
            .into_iter()
            .find_map(|reply| match reply {
                Reply::Context {
                    key: of,
                    ticket: 9,
                    result,
                    digest,
                } if of == *key => Some((result, digest)),
                _ => None,
            })
            .expect("the context was never answered for")
    }
    fn rules_entry(n: u64, size: usize) -> String {
        rules::decision(
            AT + n,
            &rules::Writer::Lead,
            &format!("Decision {n}. {}", "d".repeat(size)),
            None,
        )
    }
    use crate::projects::context::{self as rules, Place};
    /// When the entries of these tests were written: 2023, which every
    /// clock is past.
    const AT: u64 = 1_700_000_000;

    #[test]
    fn what_a_project_keeps_is_only_added_to_and_stays_within_its_bounds() {
        let data = tempfile::tempdir().unwrap();
        let store = Store::inline(data.path().into());
        let key = key(1);
        let kept = folder(data.path(), &key).join("context");
        let text = |name: &str| std::fs::read_to_string(kept.join(name)).unwrap_or_default();

        // Looking at a project that keeps nothing makes nothing.
        let (result, digest) = asked(&store, &key, ContextOp::Look);
        assert_eq!((result, digest), (Ok(String::new()), Digest::default()));
        assert!(files(data.path()).is_empty());

        // Decisions are added one after the other; nothing there is
        // touched by the next.
        let mut whole = String::new();
        for n in 0..3 {
            let entry = rules_entry(n, 20);
            let (result, digest) = asked(&store, &key, ContextOp::Record { entry });
            assert_eq!(result, Ok("Recorded in DECISIONS.md.".into()));
            assert_eq!(digest.decisions.len() as u64, n + 1);
            let now = text("DECISIONS.md");
            assert!(now.starts_with(&whole), "what was there was rewritten");
            whole = now;
        }
        assert!(whole.starts_with(rules::DECISIONS_HEAD));
        assert!(!kept.join(rules::ARCHIVE).exists());
        // Past what the file holds, its oldest entries move to the archive,
        // whole and in order; none is lost and none is in both.
        let mut recorded = 3;
        while !kept.join(rules::ARCHIVE).exists() {
            let entry = rules_entry(recorded, 900);
            let (result, _) = asked(&store, &key, ContextOp::Record { entry });
            assert_eq!(result, Ok("Recorded in DECISIONS.md.".into()));
            recorded += 1;
            assert!(recorded < 200, "the file never filled");
        }
        let (now, archive) = (text("DECISIONS.md"), text(rules::ARCHIVE));
        assert!(
            now.len() <= rules::MAX_DECISIONS / 2 + 1024,
            "{}",
            now.len()
        );
        assert!(now.starts_with(rules::DECISIONS_HEAD));
        assert!(archive.starts_with("# Decisions archive\n"));
        let times: Vec<Option<u64>> = rules::decisions(&archive)
            .iter()
            .chain(&rules::decisions(&now))
            .map(|entry| entry.at)
            .collect();
        assert_eq!(
            times,
            (0..recorded).map(|n| Some(AT + n)).collect::<Vec<_>>()
        );
        let (_, digest) = asked(&store, &key, ContextOp::Look);
        assert_eq!(digest.newest(), AT + recorded - 1);
        assert!(digest.decisions.len() <= rules::KEPT_DECISIONS);
        // An archive that is full is set aside whole and a new one begun.
        let filler = "old\n".repeat(rules::MAX_ARCHIVE / 4);
        std::fs::write(kept.join(rules::ARCHIVE), &filler).unwrap();
        let before = recorded;
        while !kept.join(rules::ARCHIVE_BEFORE).exists() {
            let entry = rules_entry(recorded, 900);
            asked(&store, &key, ContextOp::Record { entry }).0.unwrap();
            recorded += 1;
            assert!(recorded < before + 200, "the archive was never set aside");
        }
        assert_eq!(text(rules::ARCHIVE_BEFORE), filler);
        assert!(text(rules::ARCHIVE).starts_with("# Decisions archive\n"));

        // Notes on a topic are added to until their file is full.
        let note = |n: usize| {
            rules::note(
                AT,
                &rules::Writer::Member {
                    agent: 12,
                    title: "auth".into(),
                },
                &format!("{n} {}", "n".repeat(rules::MAX_NOTE - 200)),
            )
        };
        let mut added = 0;
        let full = loop {
            let op = ContextOp::Note {
                topic: "auth".into(),
                entry: note(added),
            };
            let before = text("notes/auth.md");
            match asked(&store, &key, op).0 {
                Ok(said) => {
                    assert_eq!(said, "Added to notes/auth.md.");
                    assert!(text("notes/auth.md").starts_with(&before));
                    added += 1;
                }
                Err(reason) => {
                    assert_eq!(text("notes/auth.md"), before, "a refused note left a trace");
                    break reason;
                }
            }
            assert!(added < 20);
        };
        assert_eq!(added, rules::MAX_NOTES / rules::MAX_NOTE);
        assert!(full.contains("notes/auth.md is full (64 KB)"), "{full}");
        assert!(text("notes/auth.md").len() <= rules::MAX_NOTES);
        assert!(text("notes/auth.md").starts_with("# auth\n\n0 n"));

        // The status and the instructions hold what they may and no more.
        let status = |content: &str, append| ContextOp::Status {
            content: content.into(),
            append,
        };
        let long = "s".repeat(rules::MAX_STATUS);
        assert!(asked(&store, &key, status(&long, false)).0.is_err());
        assert!(!kept.join("STATUS.md").exists());
        let most = "s".repeat(rules::MAX_STATUS - 100);
        assert_eq!(
            asked(&store, &key, status(&most, false)).0,
            Ok("Wrote STATUS.md.".into())
        );
        let over = asked(&store, &key, status(&"a".repeat(200), true)).0;
        assert!(over.is_err_and(|reason| reason.contains("would pass 8 KB")));
        assert_eq!(text("STATUS.md"), format!("{most}\n"));
        let instructions = |content: &str| ContextOp::Instructions {
            content: content.into(),
        };
        let long = "i".repeat(rules::MAX_INSTRUCTIONS);
        assert!(asked(&store, &key, instructions(&long)).0.is_err());
        assert!(!kept.join("INSTRUCTIONS.md").exists());
        let (result, digest) = asked(&store, &key, instructions(" Small commits.\n"));
        assert_eq!(result, Ok("Saved.".into()));
        assert_eq!(digest.instructions, "Small commits.");
        assert_eq!(text("INSTRUCTIONS.md"), "Small commits.\n");

        // The index is Neptune's: written from what is there, and again
        // when it is changed or deleted.
        let index = text("INDEX.md");
        assert_eq!(index, rules::index(&digest.files));
        for name in [
            "DECISIONS.md",
            "INSTRUCTIONS.md",
            "STATUS.md",
            "decisions-archive.md",
            "decisions-archive.1.md",
            "notes/auth.md",
        ] {
            assert!(index.contains(&format!("\n- {name} ")), "{name} in {index}");
        }
        assert!(
            index.contains("notes/auth.md — auth · ") && index.contains("· agent 12 \"auth\" ·")
        );
        assert!(!index.contains("INDEX.md ·"));
        std::fs::write(kept.join("INDEX.md"), "mine now").unwrap();
        asked(&store, &key, ContextOp::Look).0.unwrap();
        assert_eq!(text("INDEX.md"), index);
        let delete = |place| ContextOp::Delete { place };
        assert_eq!(
            asked(&store, &key, delete(Place::Index)).0,
            Ok("Deleted.".into())
        );
        assert_eq!(text("INDEX.md"), index);

        // A long file is read in parts that join to all of it.
        let whole = text("notes/auth.md");
        let (mut offset, mut read) = (0, String::new());
        for _ in 0..10 {
            let place = Place::Note("auth".into());
            let part = asked(&store, &key, ContextOp::Read { place, offset })
                .0
                .unwrap();
            match part.rsplit_once("\n\n[") {
                Some((text, trailer)) if trailer.ends_with(']') => {
                    read.push_str(text);
                    offset = read.len() as u64;
                    assert!(trailer.ends_with(&format!("read on with offset {offset}]")));
                    assert!(text.len() <= rules::MAX_READ);
                }
                _ => {
                    read.push_str(&part);
                    break;
                }
            }
        }
        assert_eq!(read, whole);
        let index_read = asked(
            &store,
            &key,
            ContextOp::Read {
                place: Place::Index,
                offset: 0,
            },
        );
        assert_eq!(index_read.0, Ok(index.clone()));
        let none = asked(
            &store,
            &key,
            ContextOp::Read {
                place: Place::Other("none.md".into()),
                offset: 0,
            },
        );
        assert!(
            none.0
                .is_err_and(|reason| reason.contains("There is no none.md yet"))
        );

        // The person deletes a file; what is gone is gone from the list.
        let (result, digest) = asked(&store, &key, delete(Place::Status));
        assert_eq!(result, Ok("Deleted.".into()));
        assert!(!kept.join("STATUS.md").exists());
        assert!(digest.files.iter().all(|file| file.name != "STATUS.md"));
        assert!(digest.status.is_empty());
        assert_eq!(
            asked(&store, &key, delete(Place::Status)).0,
            Ok("Deleted.".into())
        );
        // An entry whose time has not come, as a hand or an agent may have
        // put it there, is not the newest record: what is recorded after
        // it still counts as new.
        let newest = asked(&store, &key, ContextOp::Look).1.newest();
        let forged = "\n## 9999-12-31 23:59:59 UTC · lead\n\nForged.\n";
        let mut by_hand = std::fs::OpenOptions::new()
            .append(true)
            .open(kept.join("DECISIONS.md"))
            .unwrap();
        std::io::Write::write_all(&mut by_hand, forged.as_bytes()).unwrap();
        drop(by_hand);
        let (_, digest) = asked(&store, &key, ContextOp::Look);
        assert_eq!(digest.newest(), newest);
        let last = digest.decisions.last().unwrap();
        assert!(last.at.is_none() && last.text.ends_with("Forged."));
        // Decisions pasted in by hand, more than an archive holds, still
        // make room: with no archive to set aside, one is begun.
        for name in [rules::ARCHIVE, rules::ARCHIVE_BEFORE] {
            std::fs::remove_file(kept.join(name)).unwrap();
        }
        let pasted = format!("## by hand\n\n{}\n", "p".repeat(100)).repeat(12_000);
        assert!(pasted.len() > rules::MAX_ARCHIVE);
        std::fs::write(kept.join("DECISIONS.md"), &pasted).unwrap();
        let entry = rules_entry(900, 20);
        let (result, digest) = asked(&store, &key, ContextOp::Record { entry });
        assert_eq!(result, Ok("Recorded in DECISIONS.md.".into()));
        assert_eq!(digest.newest(), AT + 900);
        assert!(text("DECISIONS.md").len() <= rules::MAX_DECISIONS);
        assert_eq!(
            text(rules::ARCHIVE).matches("## by hand").count()
                + text("DECISIONS.md").matches("## by hand").count(),
            12_000
        );
        // Nothing beside the context was touched by any of it.
        assert!(
            files(data.path())
                .iter()
                .all(|file| file.contains("/context/")),
            "{:?}",
            files(data.path())
        );
    }

    #[test]
    fn a_context_holds_only_so_much_and_reads_nothing_through_a_link() {
        let data = tempfile::tempdir().unwrap();
        let store = Store::inline(data.path().into());
        let key = key(2);
        let kept = folder(data.path(), &key).join("context");
        let note = |topic: String| ContextOp::Note {
            topic,
            entry: rules::note(AT, &rules::Writer::Lead, "Seen."),
        };
        // No more files than the folder may hold, whoever asks for one.
        let mut made = 0;
        let full = loop {
            match asked(&store, &key, note(format!("topic-{made}"))).0 {
                Ok(_) => made += 1,
                Err(reason) => break reason,
            }
            assert!(made <= rules::MAX_FILES);
        };
        assert!(full.contains("The project's context is full"), "{full}");
        // The index is one of them.
        assert_eq!(made, rules::MAX_FILES - 1);
        let (_, digest) = asked(&store, &key, ContextOp::Look);
        assert_eq!(digest.files.len(), rules::MAX_FILES);
        // A topic that exists is still added to.
        assert!(asked(&store, &key, note("topic-0".into())).0.is_ok());
        // Nor more bytes: a file put there by hand counts.
        for n in 0..8 {
            std::fs::remove_file(kept.join(format!("notes/topic-{n}.md"))).unwrap();
        }
        std::fs::write(
            kept.join("big.md"),
            vec![b'x'; rules::MAX_FOLDER as usize - 1024],
        )
        .unwrap();
        let full = asked(&store, &key, note("fresh".into())).0;
        assert!(full.is_err_and(|reason| reason.contains("context is full")));
        assert!(!kept.join("notes/fresh.md").exists());
        std::fs::remove_file(kept.join("big.md")).unwrap();

        #[cfg(unix)]
        {
            // A link in the folder is not a file of it: not listed, not
            // read, not written through; deleted, only the link goes.
            let outside = data.path().join("secret.md");
            std::fs::write(&outside, "# Secret\n").unwrap();
            for name in ["leak.md", "notes/leak.md", "STATUS.md", "DECISIONS.md"] {
                std::os::unix::fs::symlink(&outside, kept.join(name)).unwrap();
            }
            let (_, digest) = asked(&store, &key, ContextOp::Look);
            assert!(digest.files.iter().all(|file| !file.name.contains("leak")));
            assert!(digest.status.is_empty() && digest.decisions.is_empty());
            for place in [
                Place::Other("leak.md".into()),
                Place::Note("leak".into()),
                Place::Status,
            ] {
                let read = asked(&store, &key, ContextOp::Read { place, offset: 0 }).0;
                assert!(read.is_err(), "{read:?}");
            }
            let deleted = asked(
                &store,
                &key,
                ContextOp::Delete {
                    place: Place::Note("leak".into()),
                },
            );
            assert_eq!(deleted.0, Ok("Deleted.".into()));
            assert!(!kept.join("notes/leak.md").exists());
            // Nothing is added to what a link points at.
            std::os::unix::fs::symlink(&outside, kept.join("notes/leak.md")).unwrap();
            let entry = rules_entry(0, 20);
            assert!(asked(&store, &key, ContextOp::Record { entry }).0.is_err());
            assert!(asked(&store, &key, note("leak".into())).0.is_err());
            let add = ContextOp::Status {
                content: "Goal".into(),
                append: true,
            };
            assert!(asked(&store, &key, add).0.is_err());
            assert_eq!(std::fs::read_to_string(&outside).unwrap(), "# Secret\n");
            // The status is replaced where the link was, not through it.
            let replace = ContextOp::Status {
                content: "Goal".into(),
                append: false,
            };
            assert_eq!(
                asked(&store, &key, replace).0,
                Ok("Wrote STATUS.md.".into())
            );
            assert!(
                std::fs::symlink_metadata(kept.join("STATUS.md"))
                    .unwrap()
                    .is_file()
            );
            assert_eq!(std::fs::read_to_string(&outside).unwrap(), "# Secret\n");

            // Nor is a link where the notes should be a folder of the
            // project: nothing behind it is listed, read, added to or deleted.
            let elsewhere = data.path().join("elsewhere");
            std::fs::create_dir(&elsewhere).unwrap();
            std::fs::write(elsewhere.join("theirs.md"), "# Theirs\n").unwrap();
            std::fs::remove_dir_all(kept.join("notes")).unwrap();
            std::os::unix::fs::symlink(&elsewhere, kept.join("notes")).unwrap();
            let (result, digest) = asked(&store, &key, ContextOp::Look);
            assert_eq!(result, Ok(String::new()));
            assert!(
                digest
                    .files
                    .iter()
                    .all(|file| !file.name.contains("notes/"))
            );
            let theirs = || Place::Note("theirs".into());
            for op in [
                note("fresh".into()),
                note("theirs".into()),
                ContextOp::Read {
                    place: theirs(),
                    offset: 0,
                },
                ContextOp::Delete { place: theirs() },
            ] {
                let refused = asked(&store, &key, op.clone()).0;
                assert!(
                    refused.is_err_and(|reason| reason.contains("is a link")),
                    "{op:?}"
                );
            }
            assert_eq!(
                std::fs::read_dir(&elsewhere).unwrap().count(),
                1,
                "something was made behind the link"
            );
            assert_eq!(
                std::fs::read_to_string(elsewhere.join("theirs.md")).unwrap(),
                "# Theirs\n"
            );
            // What is the project's own beside it is still written.
            let entry = rules_entry(1, 20);
            std::fs::remove_file(kept.join("DECISIONS.md")).unwrap();
            assert!(asked(&store, &key, ContextOp::Record { entry }).0.is_ok());

            // The same holds for the whole folder.
            let moved = data.path().join("moved");
            std::fs::rename(&kept, &moved).unwrap();
            std::os::unix::fs::symlink(&elsewhere, &kept).unwrap();
            let before = std::fs::read_dir(&elsewhere).unwrap().count();
            let (result, digest) = asked(&store, &key, ContextOp::Look);
            assert_eq!((result, digest), (Ok(String::new()), Digest::default()));
            for op in [
                ContextOp::Record {
                    entry: rules_entry(2, 20),
                },
                ContextOp::Instructions {
                    content: "Mine".into(),
                },
                ContextOp::Read {
                    place: Place::Other("theirs.md".into()),
                    offset: 0,
                },
                ContextOp::Read {
                    place: Place::Index,
                    offset: 0,
                },
            ] {
                let (refused, digest) = asked(&store, &key, op.clone());
                assert!(
                    refused.is_err_and(|reason| reason.contains("is a link")),
                    "{op:?}"
                );
                assert_eq!(digest, Digest::default());
            }
            assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), before);
        }
    }

    #[test]
    fn the_context_of_a_project_that_is_not_written_is_read_and_never_changed() {
        let data = tempfile::tempdir().unwrap();
        let key = key(3);
        let kept = folder(data.path(), &key).join("context");
        {
            let store = Store::inline(data.path().into());
            let entry = rules_entry(0, 20);
            asked(&store, &key, ContextOp::Record { entry }).0.unwrap();
        }
        // Saved by a later build: shown, and left exactly as it is.
        std::fs::write(
            folder(data.path(), &key).join("project.json"),
            r#"{"version":2}"#,
        )
        .unwrap();
        std::fs::write(kept.join("mine.md"), "# Mine\n").unwrap();
        let before = snapshot(data.path());
        let store = Store::inline(data.path().into());
        let writes = [
            ContextOp::Record {
                entry: rules_entry(1, 20),
            },
            ContextOp::Note {
                topic: "auth".into(),
                entry: "x\n".into(),
            },
            ContextOp::Status {
                content: "Goal".into(),
                append: false,
            },
            ContextOp::Instructions {
                content: "Mine".into(),
            },
            ContextOp::Delete {
                place: Place::Decisions,
            },
        ];
        for op in writes {
            let (result, digest) = asked(&store, &key, op.clone());
            assert!(
                result.is_err_and(|reason| reason.contains("read-only")),
                "{op:?}"
            );
            // What is there is still read, the file added by hand as well.
            assert_eq!(digest.decisions.len(), 1);
            assert!(digest.files.iter().any(|file| file.name == "mine.md"));
        }
        let read = asked(
            &store,
            &key,
            ContextOp::Read {
                place: Place::Decisions,
                offset: 0,
            },
        );
        assert!(read.0.is_ok_and(|text| text.contains("Decision 0.")));
        // Not even the index is brought up to date.
        assert_eq!(snapshot(data.path()), before);

        // A run that saves nothing makes no folder to keep anything in.
        let elsewhere = tempfile::tempdir().unwrap();
        let mut store = Store::new(elsewhere.path().into(), true);
        store.inline = true;
        let (result, _) = asked(
            &store,
            &key,
            ContextOp::Instructions {
                content: "Mine".into(),
            },
        );
        assert!(result.is_err_and(|reason| reason.contains("read-only")));
        assert!(files(elsewhere.path()).is_empty());
    }
}
