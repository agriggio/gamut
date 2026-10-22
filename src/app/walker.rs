//! The thread that walks every directory named for its images, and then
//! watches each directory it has read.
//!
//! A directory stands for the images directly inside it, or with
//! `--recursive` for every image in its subtree, and either way the walk
//! is done here rather than on the event loop: a tree can be thousands of
//! directories, or on a share where each read is a round trip, and the
//! window has a picture to show meanwhile. What is found is handed back in
//! batches — the first at once, so that the first picture is up while the
//! walk goes on, and the rest no more often than [`BATCH`] — and the
//! application takes each into the list between reads.
//!
//! Every directory read is then watched, by the same `stat` the file on
//! screen is (see [`crate::watch`]). A directory's own timestamp moves when
//! an entry in it comes or goes, and says nothing about its subdirectories,
//! so there is a watch apiece; and since a tree's worth of `stat`s every
//! quarter second would be its own load on a share, they are polled in two
//! tiers. The roots and the folder of the file on screen — where a file
//! exported, saved or rendered beside the picture lands — are polled every
//! [`INTERVAL`]; the rest in rotation, [`COLD_BUDGET`] a tick, so that a
//! change in a folder nobody is looking at is seen within seconds. A
//! directory that changed is read again by itself, and what it gained and
//! lost is reported as a diff, never as a list to rebuild.
//!
//! Every image the walking finds counts against a cap for the session, the
//! configuration's `max_files`: a walk started at the top of a disk by
//! mistake would otherwise read headers and make thumbnails for hours.
//! Reaching it stops the walking, and says so. A directory already watched
//! still reports what arrives in it afterwards — a file just written beside
//! the picture belongs on the list whatever the walk found — but nothing
//! new is walked into.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use super::folder::{self, Glimpse};
use crate::listing;
use crate::ui::filmstrip::Sort;
use crate::watch::{INTERVAL, Watch};

/// The longest a batch of found files waits before it is handed over. The
/// first goes at once; after that, a list taking files in once a quarter
/// second is a list that grows smoothly without being rebuilt for every
/// directory.
pub const BATCH: Duration = INTERVAL;

/// How many of the directories outside the hot tier are polled each tick.
pub const COLD_BUDGET: usize = 25;

/// What the application asks of the thread.
pub enum Ask {
    /// Walk `root` for its images: the directory alone, or with `recursive`
    /// its whole subtree. `sort` is what each image is glimpsed for.
    Walk {
        root: PathBuf,
        recursive: bool,
        sort: Sort,
    },
    /// Watch `dir`, already read elsewhere — the folder beside a single
    /// file — for `images`, without reading it again or reporting them.
    Watch {
        dir: PathBuf,
        images: Vec<PathBuf>,
        sort: Sort,
    },
    /// The folder of the file on screen, absolute, which is polled with
    /// the roots every tick.
    Shown(Option<PathBuf>),
}

/// What the thread says.
#[derive(Debug)]
pub enum Event {
    /// Images a walk found, each glimpsed for the order it was asked under.
    Found {
        images: Vec<PathBuf>,
        glimpses: Vec<(PathBuf, Glimpse)>,
    },
    /// A watched directory changed: the images that arrived in it, and the
    /// ones that have gone, with everything under a subdirectory that went.
    /// A subdirectory that arrived is walked, and comes back as `Found`.
    Changed {
        added: Vec<PathBuf>,
        glimpses: Vec<(PathBuf, Glimpse)>,
        removed: Vec<PathBuf>,
    },
    /// The walk of `root` is over, having found `found` images under it —
    /// all of them, or as many as the cap let through where `capped`.
    Done {
        root: PathBuf,
        found: usize,
        capped: bool,
    },
    /// `root` itself could not be read. The walk of it is over.
    Failed { root: PathBuf, error: anyhow::Error },
    /// The cap was reached, and nothing more will be walked this session.
    /// A `Done` follows for every walk it cut short.
    Capped { cap: usize },
}

/// How many images the walking has found this session, shared with the
/// application for the toast that says a walk is still going.
#[derive(Debug, Default)]
pub struct Progress {
    found: AtomicUsize,
}

impl Progress {
    pub fn found(&self) -> usize {
        self.found.load(Ordering::Relaxed)
    }
}

/// The thread, as the application holds it. Dropped, it stops the thread
/// and does not wait for it: a read of a directory part way through holds
/// nothing anyone needs.
pub struct Walker {
    asks: Option<Sender<Ask>>,
    canceled: Arc<AtomicBool>,
    progress: Arc<Progress>,
    /// For a walker the tests made: the asks it was sent, and the walking
    /// they drive on the test's own thread — see [`Walker::walk_here`].
    #[cfg(test)]
    here: Option<(Receiver<Ask>, Walking)>,
}

impl Walker {
    /// `cap` is how many images the walking may find this session.
    /// `deliver` is where each event goes, called on the thread; it answers
    /// `false` once nobody is listening, which stops the thread.
    pub fn new(cap: usize, deliver: impl FnMut(Event) -> bool + Send + 'static) -> Self {
        let (asks, incoming) = mpsc::channel();
        let canceled = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Progress::default());
        let flag = Arc::clone(&canceled);
        let counting = Arc::clone(&progress);
        // A thread that cannot be spawned is a session in which a
        // directory named shows nothing, which is said where it is named.
        let spawned = thread::Builder::new()
            .name("gamut walker".into())
            .spawn(move || run(incoming, Walking::new(cap, counting), deliver, &flag));
        if let Err(error) = spawned {
            eprintln!("gamut: no thread to read directories: {error}");
        }
        Self {
            asks: Some(asks),
            canceled,
            progress,
            #[cfg(test)]
            here: None,
        }
    }

    pub fn ask(&self, ask: Ask) {
        // A closed channel is a thread that has gone, which nobody is
        // waiting on.
        if let Some(asks) = &self.asks {
            let _ = asks.send(ask);
        }
    }

    pub fn progress(&self) -> &Progress {
        &self.progress
    }

    /// A walker with no thread, for tests that drive the application with
    /// no threads behind it: what it is asked is done when the test says,
    /// by [`Walker::walk_here`] and [`Walker::poll_here`].
    #[cfg(test)]
    pub fn detached(cap: usize) -> Self {
        let (asks, incoming) = mpsc::channel();
        let progress = Arc::new(Progress::default());
        Self {
            asks: Some(asks),
            canceled: Arc::new(AtomicBool::new(false)),
            here: Some((incoming, Walking::new(cap, Arc::clone(&progress)))),
            progress,
        }
    }

    /// What the thread would have said by now: every ask taken, and every
    /// directory waiting read.
    #[cfg(test)]
    pub(super) fn walk_here(&mut self) -> Vec<Event> {
        let Some((incoming, walking)) = &mut self.here else {
            return Vec::new();
        };
        while let Ok(ask) = incoming.try_recv() {
            walking.take(ask);
        }
        let mut events = Vec::new();
        while !walking.is_idle() {
            events.extend(walking.step());
        }
        events
    }

    /// One tick of the watching, and the walking of anything it found.
    #[cfg(test)]
    pub(super) fn poll_here(&mut self) -> Vec<Event> {
        let mut events = self.walk_here();
        if let Some((_, walking)) = &mut self.here {
            events.extend(walking.poll());
        }
        events.extend(self.walk_here());
        events
    }
}

impl Drop for Walker {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Relaxed);
        self.asks = None;
    }
}

fn run(
    incoming: Receiver<Ask>,
    mut walking: Walking,
    mut deliver: impl FnMut(Event) -> bool,
    canceled: &AtomicBool,
) {
    let mut next_poll = Instant::now() + INTERVAL;
    let mut batch = Batch::default();
    loop {
        if canceled.load(Ordering::Relaxed) {
            return;
        }
        // Block for an ask when there is nothing to walk, waking for the
        // next poll; take everything already waiting either way.
        if walking.is_idle() {
            if !batch.flush(&mut deliver) {
                return;
            }
            let wait = next_poll.saturating_duration_since(Instant::now());
            match incoming.recv_timeout(wait) {
                Ok(ask) => walking.take(ask),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        loop {
            match incoming.try_recv() {
                Ok(ask) => walking.take(ask),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        if !walking.is_idle() {
            for event in walking.step() {
                if !batch.take(event, &mut deliver) {
                    return;
                }
            }
            if batch.due() && !batch.flush(&mut deliver) {
                return;
            }
        }
        if Instant::now() >= next_poll {
            next_poll = Instant::now() + INTERVAL;
            for event in walking.poll() {
                if !batch.take(event, &mut deliver) {
                    return;
                }
            }
        }
    }
}

/// The images found since the last batch was handed over.
#[derive(Default)]
struct Batch {
    images: Vec<PathBuf>,
    glimpses: Vec<(PathBuf, Glimpse)>,
    /// When the last batch went: `None` before the first, which goes the
    /// moment there is anything in it.
    flushed: Option<Instant>,
}

impl Batch {
    /// Gathers a `Found` into the batch, and hands anything else on after
    /// whatever the batch holds, so that events keep their order. `false`
    /// once nobody is listening.
    fn take(&mut self, event: Event, deliver: &mut impl FnMut(Event) -> bool) -> bool {
        match event {
            Event::Found { images, glimpses } => {
                self.images.extend(images);
                self.glimpses.extend(glimpses);
                true
            }
            other => self.flush(deliver) && deliver(other),
        }
    }

    /// Whether the batch should go now: there is something in it, and a
    /// batch has not gone for [`BATCH`].
    fn due(&self) -> bool {
        !self.images.is_empty() && self.flushed.is_none_or(|at| at.elapsed() >= BATCH)
    }

    fn flush(&mut self, deliver: &mut impl FnMut(Event) -> bool) -> bool {
        if self.images.is_empty() {
            return true;
        }
        self.flushed = Some(Instant::now());
        deliver(Event::Found {
            images: std::mem::take(&mut self.images),
            glimpses: std::mem::take(&mut self.glimpses),
        })
    }
}

/// A directory read and watched.
struct Listed {
    /// Every image in it when it was last read, including any a cap kept
    /// off the list, so that those never come back as having arrived.
    images: HashSet<PathBuf>,
    /// The subdirectories walked into, for a recursive walk; empty for one
    /// that is not.
    subdirectories: HashSet<PathBuf>,
    watch: Watch,
    recursive: bool,
    sort: Sort,
    /// The directory absolute, for matching the folder of the file on
    /// screen, which the application says absolute.
    absolute: PathBuf,
}

/// A directory waiting to be read: under which walk, if any is still
/// counting, whether to go further down, and what to glimpse for.
struct Queued {
    dir: PathBuf,
    root: Option<PathBuf>,
    recursive: bool,
    sort: Sort,
}

/// A walk still going: how many of its directories are still to be read,
/// and how many images it has found.
#[derive(Default)]
struct Walk {
    queued: usize,
    found: usize,
}

/// What the thread knows, driven by [`run`] and by the tests directly.
pub(super) struct Walking {
    queue: VecDeque<Queued>,
    listed: HashMap<PathBuf, Listed>,
    walks: HashMap<PathBuf, Walk>,
    /// The roots, polled every tick.
    roots: HashSet<PathBuf>,
    /// Every directory listed, in the order the cold tier polls them.
    cold: VecDeque<PathBuf>,
    shown: Option<PathBuf>,
    found: usize,
    cap: usize,
    capped: bool,
    progress: Arc<Progress>,
}

impl Walking {
    pub(super) fn new(cap: usize, progress: Arc<Progress>) -> Self {
        Self {
            queue: VecDeque::new(),
            listed: HashMap::new(),
            walks: HashMap::new(),
            roots: HashSet::new(),
            cold: VecDeque::new(),
            shown: None,
            found: 0,
            cap,
            capped: false,
            progress,
        }
    }

    /// Whether there is nothing waiting to be read.
    pub(super) fn is_idle(&self) -> bool {
        self.queue.is_empty()
    }

    pub(super) fn take(&mut self, ask: Ask) {
        match ask {
            Ask::Walk {
                root,
                recursive,
                sort,
            } => {
                self.roots.insert(root.clone());
                self.walks.entry(root.clone()).or_default().queued += 1;
                self.queue.push_back(Queued {
                    dir: root.clone(),
                    root: Some(root),
                    recursive,
                    sort,
                });
            }
            Ask::Watch { dir, images, sort } => {
                if self.listed.contains_key(&dir) {
                    return;
                }
                let watch = Watch::new(listing::opened(&dir));
                self.roots.insert(dir.clone());
                self.list(
                    dir,
                    images.into_iter().collect(),
                    HashSet::new(),
                    watch,
                    false,
                    sort,
                );
            }
            Ask::Shown(dir) => self.shown = dir,
        }
    }

    /// Reads the next directory waiting, and says what came of it.
    pub(super) fn step(&mut self) -> Vec<Event> {
        let Some(Queued {
            dir,
            root,
            recursive,
            sort,
        }) = self.queue.pop_front()
        else {
            return Vec::new();
        };
        let mut events = Vec::new();
        if let Some(already) = self.listed.get(&dir) {
            // Read already, under another name for the walk: `-r a a/b`.
            // What it holds is on the list; this walk found it all the same.
            let count = already.images.len();
            self.walked(root, count, &mut events);
            return events;
        }
        // Watched from before the read, so that a change between the two
        // is seen at the next look rather than lost.
        let watch = Watch::new(listing::opened(&dir));
        let contents = match listing::contents(&dir) {
            Ok(contents) => contents,
            Err(error) => {
                // A root that will not read is said; a directory inside one
                // that will not — a permission, a link gone stale — is one
                // place fewer to look, and nothing to say.
                if let Some(root) = root.as_ref().filter(|root| **root == dir) {
                    let root = root.clone();
                    self.walks.remove(&root);
                    events.push(Event::Failed { root, error });
                } else {
                    self.walked(root, 0, &mut events);
                }
                return events;
            }
        };
        let subdirectories: HashSet<PathBuf> = match recursive {
            true => contents.subdirectories.iter().cloned().collect(),
            false => HashSet::new(),
        };
        if recursive {
            // At the front, in name order: depth first, so that a folder's
            // own images come before its subfolders', and the list grows
            // a folder at a time.
            for sub in contents.subdirectories.into_iter().rev() {
                if let Some(root) = &root {
                    self.walks.entry(root.clone()).or_default().queued += 1;
                }
                self.queue.push_front(Queued {
                    dir: sub,
                    root: root.clone(),
                    recursive,
                    sort,
                });
            }
        }
        let every: HashSet<PathBuf> = contents.images.iter().cloned().collect();
        let images = self.within_cap(contents.images);
        let count = images.len();
        self.list(dir, every, subdirectories, watch, recursive, sort);
        if !images.is_empty() {
            let glimpses = glimpse(&images, sort);
            events.push(Event::Found { images, glimpses });
        }
        self.walked(root, count, &mut events);
        if self.capped && !self.walks.is_empty() {
            // Every walk the cap cut short is over.
            events.push(Event::Capped { cap: self.cap });
            let roots: Vec<PathBuf> = self.walks.keys().cloned().collect();
            for root in roots {
                let found = self.walks.remove(&root).map_or(0, |walk| walk.found);
                events.push(Event::Done {
                    root,
                    found,
                    capped: true,
                });
            }
        }
        events
    }

    /// `images` as far as the cap lets them through, counted. Finding more
    /// than the cap leaves room for empties the queue: nothing more is
    /// walked. A walk that ends exactly at the cap was not cut short.
    fn within_cap(&mut self, mut images: Vec<PathBuf>) -> Vec<PathBuf> {
        let room = self.cap.saturating_sub(self.found);
        if images.len() > room {
            images.truncate(room);
            self.capped = true;
            self.queue.clear();
        }
        self.found += images.len();
        self.progress.found.store(self.found, Ordering::Relaxed);
        images
    }

    /// One more directory of `root`'s walk read, with `count` images in
    /// it; the walk is over once none of its directories is still queued.
    fn walked(&mut self, root: Option<PathBuf>, count: usize, events: &mut Vec<Event>) {
        let Some(root) = root else {
            return;
        };
        let Some(walk) = self.walks.get_mut(&root) else {
            return;
        };
        walk.found += count;
        walk.queued = walk.queued.saturating_sub(1);
        if walk.queued == 0 && !self.capped {
            let found = walk.found;
            self.walks.remove(&root);
            events.push(Event::Done {
                root,
                found,
                capped: false,
            });
        }
    }

    fn list(
        &mut self,
        dir: PathBuf,
        images: HashSet<PathBuf>,
        subdirectories: HashSet<PathBuf>,
        watch: Watch,
        recursive: bool,
        sort: Sort,
    ) {
        let absolute = std::path::absolute(listing::opened(&dir)).unwrap_or_else(|_| dir.clone());
        self.cold.push_back(dir.clone());
        self.listed.insert(
            dir,
            Listed {
                images,
                subdirectories,
                watch,
                recursive,
                sort,
                absolute,
            },
        );
    }

    /// Looks at the hot tier and the next [`COLD_BUDGET`] of the rest, and
    /// says what changed in each that has.
    pub(super) fn poll(&mut self) -> Vec<Event> {
        let mut due: Vec<PathBuf> = self
            .listed
            .iter()
            .filter(|(dir, listed)| {
                self.roots.contains(*dir) || self.shown.as_ref() == Some(&listed.absolute)
            })
            .map(|(dir, _)| dir.clone())
            .collect();
        for _ in 0..COLD_BUDGET.min(self.cold.len()) {
            let Some(dir) = self.cold.pop_front() else {
                break;
            };
            if self.listed.contains_key(&dir) {
                self.cold.push_back(dir.clone());
                if !due.contains(&dir) {
                    due.push(dir);
                }
            }
        }
        let mut events = Vec::new();
        for dir in due {
            let fired = self
                .listed
                .get_mut(&dir)
                .is_some_and(|listed| listed.watch.poll());
            if fired && let Some(event) = self.reread(&dir) {
                events.push(event);
            }
        }
        events
    }

    /// Reads `dir` again after its watch fired, and says what it gained
    /// and lost. A directory that will not read now is taken to be empty,
    /// and keeps its watch, so that one that comes back is seen to.
    fn reread(&mut self, dir: &Path) -> Option<Event> {
        let listed = self.listed.get(dir)?;
        let (recursive, sort) = (listed.recursive, listed.sort);
        let contents = listing::contents(dir).unwrap_or(listing::Contents {
            images: Vec::new(),
            subdirectories: Vec::new(),
        });
        let now: HashSet<PathBuf> = contents.images.iter().cloned().collect();
        let subdirectories: HashSet<PathBuf> = match recursive {
            true => contents.subdirectories.into_iter().collect(),
            false => HashSet::new(),
        };
        let listed = self.listed.get_mut(dir)?;
        let mut added: Vec<PathBuf> = now.difference(&listed.images).cloned().collect();
        let mut removed: Vec<PathBuf> = listed.images.difference(&now).cloned().collect();
        let arrived: Vec<PathBuf> = subdirectories
            .difference(&listed.subdirectories)
            .cloned()
            .collect();
        let gone: Vec<PathBuf> = listed
            .subdirectories
            .difference(&subdirectories)
            .cloned()
            .collect();
        listed.images = now;
        listed.subdirectories = subdirectories;
        for sub in gone {
            removed.extend(self.forget(&sub));
        }
        if !self.capped {
            let mut arrived = arrived;
            arrived.sort();
            for sub in arrived {
                self.queue.push_back(Queued {
                    dir: sub,
                    root: None,
                    recursive,
                    sort,
                });
            }
        }
        if added.is_empty() && removed.is_empty() {
            return None;
        }
        added.sort();
        removed.sort();
        let glimpses = glimpse(&added, sort);
        Some(Event::Changed {
            added,
            glimpses,
            removed,
        })
    }

    /// Stops watching `gone` and everything under it, and hands back every
    /// image they held.
    fn forget(&mut self, gone: &Path) -> Vec<PathBuf> {
        let under: Vec<PathBuf> = self
            .listed
            .keys()
            .filter(|dir| dir.starts_with(gone))
            .cloned()
            .collect();
        let mut images = Vec::new();
        for dir in under {
            if let Some(listed) = self.listed.remove(&dir) {
                images.extend(listed.images);
            }
        }
        self.cold.retain(|dir| self.listed.contains_key(dir));
        images
    }
}

/// What `sort` needs of each of `images`, as the folder beside a file
/// reads it.
fn glimpse(images: &[PathBuf], sort: Sort) -> Vec<(PathBuf, Glimpse)> {
    folder::glimpse_all(images, sort, &folder::Progress::default())
}

#[cfg(test)]
mod tests;
