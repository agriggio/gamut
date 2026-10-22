# Live reload

## What a path stands for

A directory named on the command line — or chosen in the
[file dialog](interface.md#the-empty-window-and-the-file-dialog) — stands for
the images directly inside it, or with `--recursive` for every image in its
subtree. Images are chosen by extension, since the alternative is opening
every file to see what it is. That makes a directory a place to look rather
than a list fixed at startup, which is what the rest of this page is about.
`listing::contents` is the one read of a directory; the order the list stands
in is the [file list's](filmstrip.md).

The walking is `app/walker.rs`'s, on a thread of its own, never on the event
loop: a tree can be thousands of directories, or on a share where each read
is a round trip, and the window has a picture to show meanwhile. So
`cli::parse_args` leaves a directory named as it was, `App::new` hands each to
`App::start_tree`, and the window opens on the first image the walk finds —
waiting up to `files::SLOW_READ` for it, through `App::window_due`, so that it
opens at that image's size rather than at the empty window's. The walk is
depth first, each directory's own images ahead of its subdirectories', each
level in name order, so the first image found is the root's own first. What
is found is handed over in batches: the first at once, and after that no
more often than `walker::BATCH`, since every batch is a `Files::add`, a
`Files::reindex` and an `App::apply_order` over the whole list. A batch asks
for nothing, so the screen never moves under the walk; `app/tree.rs` takes it
in between reads, as every change to the list is.

The walk does not go into a dot-directory — `~/.cache/thumbnails` alone holds
a picture of every picture — nor through a link to a directory, which is how
a walk would loop, nor on a Mac into a package, which Finder shows as one
file; a link to an image is an image. A directory inside the tree that will
not read is passed over in silence, while a root that will not is said. And
every image the walking finds counts against `Config::max_files` for the
session: a walk started at the top of a disk by mistake would otherwise have
the [thumbnail thread](chooser.md) reading headers and making thumbnails for
hours. Reaching it stops the walking and says so.

## The folder beside a single file

One file named alone — nothing else on the command line, no `--paste`, and
`browse_folder` left on — stands for itself until something asks for more:
a step, the chooser or the file list. That is when its folder is read,
not at startup, so that a file opened from a file manager comes up as one
picture with none of the list's panels around it, and the file list, which
is on by default, does not appear beside it a moment later and refit the
picture. `app/folder.rs` owns it: `Folder::Unread` from `App::new`,
`App::read_folder` from `App::step` and the chooser's and the list's
presses, and `App::settle_folder` when the read comes back.

The read runs on a thread of its own, since a folder can be large or on a
network mount, and it reads what the order in force needs of every image
alongside the names — a `stat` for a date or a size, the leading bytes for
a type, the header for the dimensions — as a `Glimpse` that `App::key_of`
falls back on until the thumbnail thread has read the header. The
thumbnail thread is not the one to ask: it reads everything about a file at
once, at low priority, and a step has to wait for the whole folder to land
on the file that really comes next. The wait is said as a file's is, in the
toast that goes up after `files::SLOW_READ`, with a count once there is one.

The folder then takes the file's place in `App::named`, its images join the
list through `Files::add`, and the walker is told to watch it with
`walker::Ask::Watch`, which hands over what was read rather than having it
read again. A bare name's folder is the empty path, which
`listing::images_in` reads as the current directory and lists as bare names,
so the file is found among them under the spelling it was named by; a file
the folder does not list — read for what it holds rather than what its
extension says — stays named beside it. Like any change to the list, it
waits for a read in flight to finish.

## Watching

The file on screen is watched, and a write to it by anything else — a render
finishing, a script rewriting its output, an editor saving — is picked up and
shown within about half a second. Nothing has to be pressed.

A reload keeps you where you were: the same pan and zoom, the same exposure
and tone map, with only an automatic window re-derived from the new pixels.
The point is watching one spot as the numbers under it change. A file that
comes back the same shape at another size is the same picture written larger or
smaller: it keeps its exposure and tone map, and stays where it was at the size it
was on screen. One that comes back another shape is treated as a different picture
and gets a fresh fit. `]` and `[` move the watch along with the view.

It is a `stat` every 250 ms, not `inotify`. That costs nothing measurable, and
it is the version that works over NFS and SSHFS and that survives the way most
editors save — a temporary file renamed over the original, which leaves a
watch on the original inode looking at a file nobody will ever write to again.
A change is read only once the size and timestamp have held still for a whole
interval, so a file caught halfway through being written is waited out rather
than decoded and reported as corrupt.

A directory named is watched the same way, by its own `stat`, and so is every
directory a walk went into: a directory's timestamp moves when an entry in it
comes or goes and says nothing about its subdirectories, so there is a
`Watch` apiece, on the walker thread. A tree's worth of `stat`s every quarter
second would be a load of its own on a share, so they are polled in two
tiers. The roots and the folder of the file on screen — told to the walker
by `App::tell_walker_shown` as each file arrives — are looked at every
250 ms, since that is where a file exported, saved or rendered beside the
picture lands. The rest are looked at in rotation, `walker::COLD_BUDGET` a
tick, so that a change in a folder nobody is looking at is seen within
seconds rather than at once. A directory that changed is read again by
itself, and `Walking::reread` reports what it gained and lost as a diff —
`walker::Event::Changed` — never as a list to rebuild. A subdirectory that
arrived under a recursive walk is walked; one that went takes everything
under it with it. So a script dropping frames into a folder builds the list
as it goes, and `]` reaches a file that did not exist when the window
opened. The images found after the cap was reached still join: the cap is on
walking, not on what a watched folder is seen to gain.

A directory's timestamp moves on the clock's tick, not with each change, and
its size on most file systems moves only with its entry count. A change
inside the tick the directory was read in, which leaves as many entries as
it found, is not seen until the next change — in practice, a file renamed in
the instant the directory was read. The watch is made before the read rather
than after it, so that a change between the two is seen at the next look.

Two things hold still through that. The file on screen is never dropped from
the list, whatever has happened to it on disk: its pixels are up and correct,
and everything the interface says about them — the title, the bars, the
information panel — is read off the path they came from. It keeps its
neighbors too, so `]` from a file deleted under you goes on to whatever has
taken its place rather than back over one already seen. And the list only
changes between reads, since a change moves the file on screen to a new index
and a reply already on its way is aimed at the old one; what the walker says
during a read waits in `Trees::landed`, and is taken in the moment the read is
over, by `App::settle_trees` from the decode's own handler.

