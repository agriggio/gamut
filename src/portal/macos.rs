//! The desktop's own file dialog on macOS: `NSOpenPanel`, the dialog every
//! Mac program opens files with, its sidebar and recent places included.
//!
//! AppKit puts a panel up on the main thread only, which is the thread the
//! window asks from, so the panel is run there, modally, and its answer
//! handed to [`Deliver`] before this returns — the same way back into the
//! event loop the Linux dialog's answer takes from its thread.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use objc2::MainThreadMarker;
use objc2::rc::autoreleasepool;
use objc2_app_kit::{
    NSButton, NSControlStateValueOff, NSControlStateValueOn, NSModalResponseOK, NSOpenPanel,
};
use objc2_foundation::{NSArray, NSString, NSURL};
use objc2_uniform_type_identifiers::UTType;

use super::{Deliver, Pick, Picked};

/// Puts up the dialog and hands the answer to `deliver`: what the user
/// chose, or `None` for a dialog dismissed without choosing.
///
/// `folder` is where the dialog starts, where there is somewhere to start,
/// and `subfolders` how a folder dialog's box for subfolders is put up.
pub fn choose_on_thread(pick: Pick, folder: Option<PathBuf>, subfolders: bool, deliver: Deliver) {
    let (outcome, ticked) = match choose(pick, folder.as_deref(), subfolders) {
        Ok((chosen, ticked)) => (Ok(chosen), ticked),
        Err(error) => (Err(error), None),
    };
    deliver(Picked {
        outcome,
        subfolders: ticked,
    });
}

/// The panel, run; and for a folder, how its box for subfolders was left,
/// which hangs below the panel's own controls as its accessory view, shown
/// rather than folded away behind an Options button.
fn choose(
    pick: Pick,
    folder: Option<&Path>,
    subfolders: bool,
) -> Result<(Option<Vec<PathBuf>>, Option<bool>)> {
    let main = MainThreadMarker::new().context("the file dialog is put up from the main thread")?;
    autoreleasepool(|_| {
        let panel = NSOpenPanel::openPanel(main);
        let files = pick == Pick::Files;
        panel.setCanChooseFiles(files);
        panel.setCanChooseDirectories(!files);
        panel.setAllowsMultipleSelection(files);
        panel.setMessage(Some(&NSString::from_str(pick.title())));
        panel.setPrompt(Some(&NSString::from_str("Open")));
        if files {
            // Only the formats this program reads, by the types their
            // extensions stand for; an extension the system has no type for
            // is left out of the filter rather than failing it.
            let types: Vec<_> = crate::image::decode::supported_extensions()
                .iter()
                .filter_map(|extension| {
                    UTType::typeWithFilenameExtension(&NSString::from_str(extension))
                })
                .collect();
            panel.setAllowedContentTypes(&NSArray::from_retained_slice(&types));
        }
        if let Some(folder) = folder.and_then(NSURL::from_directory_path) {
            panel.setDirectoryURL(Some(&folder));
        }
        let check = (!files).then(|| {
            // SAFETY: no target and no action, so nothing is sent to an
            // object that might not answer it; the state is read back once
            // the panel is down.
            let check = unsafe {
                NSButton::checkboxWithTitle_target_action(
                    &NSString::from_str(super::SUBFOLDERS),
                    None,
                    None,
                    main,
                )
            };
            check.setState(match subfolders {
                true => NSControlStateValueOn,
                false => NSControlStateValueOff,
            });
            panel.setAccessoryView(Some(&check));
            panel.setAccessoryViewDisclosed(true);
            check
        });
        // How the box was left, said only for a folder chosen: a dialog
        // dismissed says nothing of it, and leaves it as it was.
        if panel.runModal() != NSModalResponseOK {
            return Ok((None, None));
        }
        let ticked = check.map(|check| check.state() == NSControlStateValueOn);
        let paths: Vec<PathBuf> = panel
            .URLs()
            .iter()
            .filter_map(|url| url.to_file_path())
            .collect();
        Ok(((!paths.is_empty()).then_some(paths), ticked))
    })
}
