use super::*;

/// A directory of its own under the temporary directory, removed when the
/// test is done with it.
struct Tree(PathBuf);

impl Tree {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("gamut-walker-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    /// Writes an empty file at each of `paths`, relative to the tree, with
    /// the directories they need.
    fn files(&self, paths: &[&str]) -> &Self {
        for path in paths {
            let path = self.0.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"").unwrap();
        }
        self
    }

    fn at(&self, path: &str) -> PathBuf {
        self.0.join(path)
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn walking(cap: usize) -> Walking {
    Walking::new(cap, Arc::new(Progress::default()))
}

fn walk(walking: &mut Walking, root: &Path, recursive: bool) {
    walking.take(Ask::Walk {
        root: root.to_path_buf(),
        recursive,
        sort: Sort::Name,
    });
}

/// Every event until there is nothing left to read.
fn walk_out(walking: &mut Walking) -> Vec<Event> {
    let mut events = Vec::new();
    while !walking.is_idle() {
        events.extend(walking.step());
    }
    events
}

/// The images of every `Found`, in the order they came.
fn found(events: &[Event]) -> Vec<PathBuf> {
    events
        .iter()
        .flat_map(|event| match event {
            Event::Found { images, .. } => images.clone(),
            _ => Vec::new(),
        })
        .collect()
}

fn done(events: &[Event]) -> Vec<(PathBuf, usize, bool)> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Done {
                root,
                found,
                capped,
            } => Some((root.clone(), *found, *capped)),
            _ => None,
        })
        .collect()
}

/// Polls until the watch has had the two looks a change needs to settle,
/// and hands back what was said.
fn settle(walking: &mut Walking, ticks: usize) -> Vec<Event> {
    (0..ticks).flat_map(|_| walking.poll()).collect()
}

/// A directory alone is its own images, and nothing below it.
#[test]
fn a_directory_alone_is_its_own_images() {
    let tree = Tree::new("alone");
    tree.files(&["b.png", "a.png", "sub/c.png"]);
    let mut walking = walking(100);
    walk(&mut walking, &tree.0, false);
    let events = walk_out(&mut walking);
    assert_eq!(found(&events), [tree.at("a.png"), tree.at("b.png")]);
    assert_eq!(done(&events), [(tree.0.clone(), 2, false)]);
}

/// A recursive walk takes a folder's own images before its subfolders',
/// each level in name order, and ends once every one has been read.
#[test]
fn a_recursive_walk_goes_depth_first_in_name_order() {
    let tree = Tree::new("depth");
    tree.files(&["z.png", "b/2.png", "a/1.png", "a/deep/0.png"]);
    let mut walking = walking(100);
    walk(&mut walking, &tree.0, true);
    let events = walk_out(&mut walking);
    assert_eq!(
        found(&events),
        [
            tree.at("z.png"),
            tree.at("a/1.png"),
            tree.at("a/deep/0.png"),
            tree.at("b/2.png"),
        ]
    );
    assert_eq!(done(&events), [(tree.0.clone(), 4, false)]);
}

/// A hidden directory is not walked into, nor a link to one — and a link
/// back up the tree, the loop a walk would otherwise never leave.
#[test]
fn a_walk_goes_round_no_loop_and_into_nothing_hidden() {
    let tree = Tree::new("loop");
    tree.files(&["a.png", ".cache/thumb.png", "sub/b.png"]);
    std::os::unix::fs::symlink(&tree.0, tree.at("sub/up")).unwrap();
    std::os::unix::fs::symlink(tree.at("sub"), tree.at("again")).unwrap();
    let mut walking = walking(100);
    walk(&mut walking, &tree.0, true);
    let events = walk_out(&mut walking);
    assert_eq!(found(&events), [tree.at("a.png"), tree.at("sub/b.png")]);
}

/// A root that will not read is said; a directory inside one that will
/// not is one place fewer to look.
#[test]
fn only_a_root_that_will_not_read_is_reported() {
    let mut walking = walking(100);
    let missing = PathBuf::from("/no/such/directory");
    walk(&mut walking, &missing, true);
    let events = walk_out(&mut walking);
    assert!(
        matches!(&events[..], [Event::Failed { root, .. }] if *root == missing),
        "{events:?}"
    );

    // Unreadable to anyone but root, who reads it anyway.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let tree = Tree::new("locked");
    tree.files(&["a.png", "locked/b.png"]);
    std::fs::set_permissions(tree.at("locked"), std::fs::Permissions::from_mode(0o000)).unwrap();
    walk(&mut walking, &tree.0, true);
    let events = walk_out(&mut walking);
    std::fs::set_permissions(tree.at("locked"), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(found(&events), [tree.at("a.png")]);
    assert_eq!(done(&events), [(tree.0.clone(), 1, false)]);
}

/// The cap stops the walking: the batch that crosses it is cut to fit,
/// nothing more is read, and the walk ends saying it was cut short.
#[test]
fn the_cap_stops_the_walking() {
    let tree = Tree::new("cap");
    tree.files(&["a/1.png", "a/2.png", "a/3.png", "b/4.png"]);
    let mut walking = walking(2);
    walk(&mut walking, &tree.0, true);
    let events = walk_out(&mut walking);
    assert_eq!(found(&events), [tree.at("a/1.png"), tree.at("a/2.png")]);
    assert!(events.iter().any(|e| matches!(e, Event::Capped { cap: 2 })));
    assert_eq!(done(&events), [(tree.0.clone(), 2, true)]);
    assert_eq!(walking.progress.found(), 2);
}

/// A walk that ends exactly at the cap was not cut short.
#[test]
fn a_walk_that_ends_at_the_cap_is_not_cut_short() {
    let tree = Tree::new("exact");
    tree.files(&["a.png", "b.png"]);
    let mut walking = walking(2);
    walk(&mut walking, &tree.0, true);
    let events = walk_out(&mut walking);
    assert!(!events.iter().any(|e| matches!(e, Event::Capped { .. })));
    assert_eq!(done(&events), [(tree.0.clone(), 2, false)]);
}

/// A watched directory says what arrived in it and what left, once the
/// change has held still for a look.
#[test]
fn a_change_in_a_watched_directory_is_a_diff() {
    let tree = Tree::new("diff");
    tree.files(&["a.png", "b.png"]);
    let mut walking = walking(100);
    walk(&mut walking, &tree.0, false);
    walk_out(&mut walking);
    assert!(settle(&mut walking, 2).is_empty(), "nothing has changed");

    std::fs::remove_file(tree.at("a.png")).unwrap();
    tree.files(&["c.png", "notes.txt"]);
    let events = settle(&mut walking, 2);
    match &events[..] {
        [Event::Changed { added, removed, .. }] => {
            assert_eq!(added, &[tree.at("c.png")]);
            assert_eq!(removed, &[tree.at("a.png")]);
        }
        other => panic!("{other:?}"),
    }
    assert!(settle(&mut walking, 2).is_empty(), "said once");
}

/// A file that arrived and is then written again — a renderer filling the
/// file it opened — is said modified once the writing has held still for a
/// look, and once only.
#[test]
fn a_file_written_after_it_arrived_is_modified() {
    let tree = Tree::new("growing");
    tree.files(&["a.png"]);
    let mut walking = walking(100);
    walk(&mut walking, &tree.0, false);
    walk_out(&mut walking);

    tree.files(&["c.png"]);
    match &settle(&mut walking, 2)[..] {
        [Event::Changed { added, .. }] => assert_eq!(added, &[tree.at("c.png")]),
        other => panic!("{other:?}"),
    }
    std::fs::write(tree.at("c.png"), b"the rest of it").unwrap();
    match &settle(&mut walking, 2)[..] {
        [
            Event::Changed {
                added,
                removed,
                modified,
                ..
            },
        ] => {
            assert!(added.is_empty() && removed.is_empty());
            assert_eq!(modified, &[tree.at("c.png")]);
        }
        other => panic!("{other:?}"),
    }
    assert!(settle(&mut walking, 2).is_empty(), "said once");
}

/// A file there when the directory was read is not watched by itself, and
/// one that arrived is watched only until it has held still for `FRESH`.
#[test]
fn only_a_newcomer_is_watched_and_not_for_long() {
    let tree = Tree::new("fresh");
    tree.files(&["a.png"]);
    let mut walking = walking(100);
    walk(&mut walking, &tree.0, false);
    walk_out(&mut walking);
    std::fs::write(tree.at("a.png"), b"rewritten").unwrap();
    assert!(
        settle(&mut walking, 2).is_empty(),
        "the walk's own files are not"
    );

    tree.files(&["c.png"]);
    settle(&mut walking, 2);
    assert!(walking.fresh.contains_key(&tree.at("c.png")));
    assert!(
        walking
            .poll_fresh(Instant::now() + FRESH + Duration::from_secs(1))
            .is_none()
    );
    assert!(walking.fresh.is_empty(), "held still for long enough");
}

/// Under a recursive walk a folder that arrives is walked into, and one
/// that goes takes everything under it off the list with it.
#[test]
fn a_folder_arriving_is_walked_and_one_leaving_takes_its_tree() {
    let tree = Tree::new("folders");
    tree.files(&["a.png", "old/b.png", "old/deeper/c.png"]);
    let mut walking = walking(100);
    walk(&mut walking, &tree.0, true);
    walk_out(&mut walking);

    // One entry gone and one come leaves the directory the size it was,
    // and a timestamp only moves on the clock's tick, which is coarser
    // than this test: changed inside the tick it was read in, it would
    // look as it did.
    std::thread::sleep(Duration::from_millis(20));
    std::fs::remove_dir_all(tree.at("old")).unwrap();
    tree.files(&["new/d.png"]);
    let events = settle(&mut walking, 2);
    match &events[..] {
        [Event::Changed { added, removed, .. }] => {
            assert!(added.is_empty());
            assert_eq!(
                removed,
                &[tree.at("old/b.png"), tree.at("old/deeper/c.png")]
            );
        }
        other => panic!("{other:?}"),
    }
    let events = walk_out(&mut walking);
    assert_eq!(found(&events), [tree.at("new/d.png")]);
    assert!(done(&events).is_empty(), "the walk of the root was over");
}

/// The folder of the file on screen is looked at every tick, however many
/// folders there are; the rest are reached in turn.
#[test]
fn the_folder_on_screen_is_hot_and_the_rest_take_turns() {
    let tree = Tree::new("tiers");
    let names: Vec<String> = (0..60).map(|i| format!("d{i:02}/x.png")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    tree.files(&names);
    let mut walking = walking(1000);
    walk(&mut walking, &tree.0, true);
    walk_out(&mut walking);
    let shown = std::path::absolute(tree.at("d59")).unwrap();
    walking.take(Ask::Shown(Some(shown)));

    // Rotated so that d59 would be the last of the cold tier to come up.
    tree.files(&["d59/y.png"]);
    assert_eq!(settle(&mut walking, 2).len(), 1, "seen within two ticks");

    // A cold folder is reached within its turn: 61 folders, 25 a tick.
    tree.files(&["d30/y.png"]);
    let ticks = 2 * 61_usize.div_ceil(COLD_BUDGET) + 1;
    assert_eq!(settle(&mut walking, ticks).len(), 1);
}

/// The folder beside a single file is watched without being read again.
#[test]
fn a_folder_read_elsewhere_is_watched_without_being_read() {
    let tree = Tree::new("beside");
    tree.files(&["a.png", "b.png"]);
    let mut walking = walking(100);
    walking.take(Ask::Watch {
        dir: tree.0.clone(),
        images: vec![tree.at("a.png"), tree.at("b.png")],
        sort: Sort::Name,
    });
    assert!(walking.is_idle(), "nothing to read");
    tree.files(&["c.png"]);
    match &settle(&mut walking, 2)[..] {
        [Event::Changed { added, .. }] => assert_eq!(added, &[tree.at("c.png")]),
        other => panic!("{other:?}"),
    }
}

/// The first batch goes the moment there is anything in it, and the next
/// waits for its turn.
#[test]
fn the_first_batch_goes_at_once() {
    let mut batch = Batch::default();
    let mut said = Vec::new();
    let mut deliver = |event| {
        said.push(event);
        true
    };
    assert!(batch.take(
        Event::Found {
            images: vec![PathBuf::from("a.png")],
            glimpses: Vec::new(),
        },
        &mut deliver,
    ));
    assert!(batch.due());
    assert!(batch.flush(&mut deliver));
    batch.take(
        Event::Found {
            images: vec![PathBuf::from("b.png")],
            glimpses: Vec::new(),
        },
        &mut deliver,
    );
    assert!(!batch.due(), "one has only just gone");
    // Anything else goes after what the batch holds.
    batch.take(Event::Capped { cap: 1 }, &mut deliver);
    assert!(matches!(
        &said[..],
        [
            Event::Found { .. },
            Event::Found { .. },
            Event::Capped { .. }
        ]
    ));
}
