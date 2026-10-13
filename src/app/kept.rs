//! What each file was left in, so that coming back to it puts it back.
//!
//! Flipping between two pictures is how they are compared, and a comparison
//! only holds if each of them comes back as it was left: the same pan and
//! zoom, the same turn, the same window and exposure, the same tone curve
//! and false color.
//! So whatever the file leaving the screen was set to is put away here, and
//! the file arriving takes back whatever it left — including the file the
//! walk has stepped past and come round to again.
//!
//! One thing outranks what a file left: a file arriving beside a picture of
//! its own size takes that picture's pan and zoom rather than its own. Images
//! of a size are a set being compared, and the comparison is made wherever
//! the eye already is. Everything else it left — its window and exposure, its
//! tone curve and false color — still comes back with it.
//!
//! By path rather than by index: the list is rebuilt while the window is open
//! — a directory named on the command line is a place to look rather than a
//! fixed list — and a file that has moved in it is still the same file.
//!
//! Nothing is ever forgotten. An entry is a handful of numbers beside a path,
//! so a walk through a directory of thousands costs less than one of the
//! pictures in it, and a viewer that dropped the oldest would drop exactly
//! the picture a long comparison keeps returning to.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::image::decode::{Reading, Rendering};
use crate::image::display::Display;
use crate::image::orient::Turn;
use crate::view::View;

/// How one file was left.
#[derive(Clone)]
pub(super) struct Settings {
    /// Where the view was going rather than where it was drawn: a move still
    /// under way when the file was left was on its way to this, and this is
    /// what the file should come back to.
    pub(super) view: View,
    pub(super) display: Display,
    /// Where in the file it was left, for one that holds more than one
    /// picture.
    pub(super) left: Option<Left>,
    /// How far it was turned on screen.
    pub(super) turn: Turn,
    /// Which of a raw's pictures it was left showing. What it was left in is
    /// only put back onto the same one.
    pub(super) rendering: Rendering,
}

/// Where a file of several pictures was left.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Left {
    /// An animation: the frame that was up, and whether it was stopped
    /// there. One left playing comes back playing.
    Frame { frame: usize, paused: bool },
    /// A paged file: the page that was up.
    Page(usize),
}

/// Every file seen so far, and what each was left in.
#[derive(Default)]
pub(super) struct Kept {
    settings: HashMap<PathBuf, Settings>,
    /// The files whose reading was chosen by hand — as a photo, or as
    /// data — and which. Apart from the settings, since it is wanted before
    /// the file is read rather than once it has arrived, and is set while
    /// the file is on screen rather than as it is left.
    readings: HashMap<PathBuf, Reading>,
}

impl Kept {
    /// Puts away how the file at `path` is being left, replacing whatever it
    /// had left before.
    pub(super) fn keep(&mut self, path: &Path, settings: Settings) {
        self.settings.insert(path.to_path_buf(), settings);
    }

    /// What that file left behind, or `None` for one that has not been on
    /// screen yet.
    pub(super) fn left(&self, path: &Path) -> Option<&Settings> {
        self.settings.get(path)
    }

    /// Has the file at `path` read as `reading` from now on.
    pub(super) fn read_as(&mut self, path: &Path, reading: Reading) {
        self.readings.insert(path.to_path_buf(), reading);
    }

    /// How the file at `path` was asked to be read, or `None` for one left
    /// to the decoder's own reading.
    pub(super) fn reading(&self, path: &Path) -> Option<Reading> {
        self.readings.get(path).copied()
    }

    /// The file at `from` is called `to` now: what it left comes with it,
    /// since a renamed file is still the same file.
    pub(super) fn rename(&mut self, from: &Path, to: &Path) {
        if let Some(settings) = self.settings.remove(from) {
            self.settings.insert(to.to_path_buf(), settings);
        }
        if let Some(reading) = self.readings.remove(from) {
            self.readings.insert(to.to_path_buf(), reading);
        }
    }
}
