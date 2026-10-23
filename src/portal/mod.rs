//! The desktop's own file dialog: [`choose_on_thread`] puts it up and hands
//! the answer back through the event loop. How it is asked for is the
//! platform's — `freedesktop.rs` on Linux, through the portal, and
//! `macos.rs` on a Mac.
//!
//! A dialog picks files or it picks a folder, never both in one — every
//! desktop's dialog is built that way — so [`Pick`] says which is being asked
//! for and the window offers the two as two buttons. A folder dialog carries
//! a box asking for the folder's subfolders too, which comes back on
//! [`Picked`] beside what was chosen.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;

#[cfg(target_os = "linux")]
mod freedesktop;
#[cfg(target_os = "linux")]
pub use freedesktop::choose_on_thread;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::choose_on_thread;

/// What the dialog is to pick.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pick {
    /// One or more image files, the dialog's list narrowed to the formats
    /// this program reads.
    Files,
    /// A folder, to stand for the images inside it as a directory on the
    /// command line does.
    Folder,
}

impl Pick {
    /// The dialog's title.
    pub(super) fn title(self) -> &'static str {
        match self {
            Pick::Files => "Open images",
            Pick::Folder => "Open a folder of images",
        }
    }
}

/// The words beside the folder dialog's box that asks for the folder's
/// subfolders too.
pub(super) const SUBFOLDERS: &str = "Include subfolders";

/// What came of a dialog: what the user chose, or `None` for a dialog
/// dismissed without choosing.
pub struct Picked {
    pub outcome: Result<Option<Vec<PathBuf>>>,
    /// For a folder dialog, whether its box for subfolders was left ticked
    /// — unticked where the desktop's dialog has no such box to offer.
    /// `None` for a dialog that picked files, which has none.
    pub subfolders: Option<bool>,
}

/// How a dialog's answer reaches the window: what `main` made from the
/// event loop's proxy.
pub type Deliver = Arc<dyn Fn(Picked) + Send + Sync>;
