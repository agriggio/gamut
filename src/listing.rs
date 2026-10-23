//! Which files there are to show, and keeping that list up to date.
//!
//! A directory named is a place to look rather than a fixed list: it stands
//! for the images inside it, and it is watched while the program runs, so
//! that an image arriving in it or leaving it joins or leaves the list.
//! [`contents`] is the one read of a directory, which the walker thread
//! (`app::walker`) makes of each directory in a tree, one level or all of
//! them, and again of each one that changes.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The images directly inside `dir`, in name order. One level deep: the
/// walk through a tree is [`contents`] asked of each of its directories.
pub fn images_in(dir: &Path) -> Result<Vec<PathBuf>> {
    Ok(contents(dir)?.images)
}

/// What one directory holds that a walk through it wants: the images in
/// it, and the directories in it worth walking into, each in name order.
pub struct Contents {
    pub images: Vec<PathBuf>,
    pub subdirectories: Vec<PathBuf>,
}

/// Reads `dir` once for [`Contents`].
///
/// The extension decides what is an image here, since the alternative is
/// opening every file in the directory to look at its leading bytes. A
/// file named on the command line is still read for what it holds rather
/// than what it is called.
///
/// A subdirectory is walked into unless it is hidden — a dot-directory,
/// where `~/.cache/thumbnails` alone holds thousands of pictures of
/// pictures — or a link, which is how a walk loops, or on a Mac a package,
/// which Finder shows as one file. A link to an image is an image.
///
/// Each entry is `dir` joined to its name, so that the empty path — the
/// folder of a file named without one — lists as bare names, which is how
/// that file itself was named.
pub fn contents(dir: &Path) -> Result<Contents> {
    let extensions = crate::image::decode::supported_extensions();
    let mut images = Vec::new();
    let mut subdirectories = Vec::new();
    let entries = std::fs::read_dir(opened(dir))
        .with_context(|| format!("reading {}", opened(dir).display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", opened(dir).display()))?;
        let candidate = dir.join(entry.file_name());
        let extension = lowercase_extension(&candidate);
        // The extension is checked first because it costs nothing; the
        // directory test that follows it is for the rare directory named
        // like an image.
        if extensions.contains(&extension.as_str()) && !is_directory(&entry, &candidate) {
            images.push(candidate);
        } else if walked_into(&entry, &extension) {
            subdirectories.push(candidate);
        }
    }
    images.sort();
    subdirectories.sort();
    Ok(Contents {
        images,
        subdirectories,
    })
}

fn lowercase_extension(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// The extensions of the directories a Mac shows as single files: an
/// application, and the libraries photo programs keep their own copies
/// and previews in, which a walk would show beside the originals.
const PACKAGES: [&str; 9] = [
    "app",
    "bundle",
    "framework",
    "photoslibrary",
    "aplibrary",
    "lrdata",
    "lrcat",
    "imovielibrary",
    "fcpbundle",
];

/// Whether a walk goes into the entry: a real directory, not hidden and
/// not a package. Asked of the listing's own kind, never followed through
/// a link.
fn walked_into(entry: &std::fs::DirEntry, extension: &str) -> bool {
    let Ok(kind) = entry.file_type() else {
        return false;
    };
    if !kind.is_dir() || entry.file_name().as_encoded_bytes().starts_with(b".") {
        return false;
    }
    !(cfg!(target_os = "macos") && PACKAGES.contains(&extension))
}

/// Whether the entry `candidate` names is a directory, asked of the listing
/// itself where it says: most file systems hand the kind of each entry
/// over with its name, where a `stat` of each would be a round trip apiece
/// on a network share. A link, and an entry whose kind the listing left
/// out, are followed with a `stat`, a link to a directory being a
/// directory here as it is to `is_dir`.
fn is_directory(entry: &std::fs::DirEntry, candidate: &Path) -> bool {
    match entry.file_type() {
        Ok(kind) if !kind.is_symlink() => kind.is_dir(),
        _ => opened(candidate).is_dir(),
    }
}

/// The folder `file` is in, as its name has it: empty for a bare name,
/// which [`images_in`] reads as the current directory.
pub fn folder_of(file: &Path) -> PathBuf {
    file.parent().map(Path::to_path_buf).unwrap_or_default()
}

/// The path a directory is opened and shown by: `.` for the empty path,
/// which would otherwise show as nothing at all.
pub fn opened(dir: &Path) -> &Path {
    if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_images")
    }

    /// A directory stands for the images in it, in name order, and for
    /// nothing else: the color profile, the shell script and the README
    /// lying beside them are not files to show.
    #[test]
    fn a_directory_becomes_the_images_inside_it() {
        let files = images_in(&fixtures()).expect("test_images/ holds images");
        let mut sorted = files.clone();
        sorted.sort();
        assert_eq!(files, sorted, "the list is in name order");
        assert!(files.contains(&fixtures().join("png-rgb8.png")));

        let extensions = crate::image::decode::supported_extensions();
        for file in &files {
            assert!(
                extensions.contains(&lowercase_extension(file).as_str()),
                "{} is not an image and should not be listed",
                file.display()
            );
        }
        assert!(
            !files.contains(&fixtures().join("unsupported.tga")),
            "a format we cannot read is not worth stepping through"
        );
    }

    /// A directory named like an image is not an image, whether it is one
    /// outright or a link to one; a link to an image is.
    #[test]
    fn a_directory_named_like_an_image_is_left_out() {
        let dir = std::env::temp_dir().join(format!("gamut-listing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("album.jpg")).unwrap();
        std::fs::write(dir.join("a.png"), b"").unwrap();
        std::os::unix::fs::symlink(dir.join("album.jpg"), dir.join("linked.jpg")).unwrap();
        std::os::unix::fs::symlink(dir.join("a.png"), dir.join("b.png")).unwrap();
        assert_eq!(
            images_in(&dir).unwrap(),
            [dir.join("a.png"), dir.join("b.png")]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A directory lists the subdirectories a walk goes into beside its
    /// images: not a hidden one, not a link to one — a link back up the
    /// tree being how a walk would loop. One named like an image is a
    /// directory like any other.
    #[test]
    fn a_directory_lists_the_subdirectories_worth_walking_into() {
        let dir = std::env::temp_dir().join(format!("gamut-contents-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["b", "a", ".cache", "album.jpg"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        std::fs::write(dir.join("z.png"), b"").unwrap();
        std::fs::write(dir.join("notes.txt"), b"").unwrap();
        std::os::unix::fs::symlink(&dir, dir.join("up")).unwrap();
        let read = contents(&dir).unwrap();
        assert_eq!(read.images, [dir.join("z.png")]);
        assert_eq!(
            read.subdirectories,
            [dir.join("a"), dir.join("album.jpg"), dir.join("b")]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A package is one file to a Mac, and is not walked into there.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_package_is_not_walked_into_on_a_mac() {
        let dir = std::env::temp_dir().join(format!("gamut-package-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Photos Library.photoslibrary")).unwrap();
        std::fs::create_dir_all(dir.join("trip")).unwrap();
        assert_eq!(contents(&dir).unwrap().subdirectories, [dir.join("trip")]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A directory that will not read is an error, not an empty one.
    #[test]
    fn a_missing_directory_is_an_error() {
        assert!(contents(Path::new("/no/such/directory")).is_err());
    }
}
