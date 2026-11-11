//! Image diffs — load the "before" and "after" bytes of an image file so a
//! front-end can render them side by side.

use anyhow::{Context, Result};
use git2::Repository;

use super::types::{DiffInfo, FileStatus};

/// Largest image side we load (either version), to keep memory bounded.
pub const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;

/// How a front-end should render an image file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    /// Bitmap formats: PNG, JPEG, GIF, WebP, BMP, ICO.
    Raster,
    /// Vector SVG — rendered, but also has a meaningful text diff.
    Svg,
}

/// Classify a path by its extension. `None` means "not an image we render".
pub fn image_kind(path: &str) -> Option<ImageKind> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" => Some(ImageKind::Raster),
        "svg" => Some(ImageKind::Svg),
        _ => None,
    }
}

/// Both sides of an image diff. A side is `None` when the file did not exist
/// there (added / deleted files).
#[derive(Debug, Clone)]
pub struct ImageDiff {
    pub kind: ImageKind,
    pub old: Option<Vec<u8>>,
    pub new: Option<Vec<u8>>,
}

/// Load the old and new bytes of an image file described by `diff`.
///
/// The old side always comes from the object database. The new side comes
/// from the object database when its blob is there (commits, the index) and
/// otherwise from the working directory (unstaged and untracked files).
pub fn load_image_diff(repo: &Repository, diff: &DiffInfo) -> Result<ImageDiff> {
    let kind = image_kind(diff.display_path())
        .with_context(|| format!("'{}' is not an image", diff.display_path()))?;

    let old = match diff.status {
        FileStatus::New | FileStatus::Untracked => None,
        _ => diff
            .old_id
            .as_deref()
            .map(|id| read_blob(repo, id))
            .transpose()?,
    };

    let new = match diff.status {
        FileStatus::Deleted => None,
        _ => {
            let from_odb = diff
                .new_id
                .as_deref()
                .and_then(|id| read_blob(repo, id).ok());
            match from_odb {
                Some(bytes) => Some(bytes),
                None => Some(read_workdir_file(repo, &diff.new_file)?),
            }
        }
    };

    Ok(ImageDiff { kind, old, new })
}

fn read_blob(repo: &Repository, id: &str) -> Result<Vec<u8>> {
    let oid = git2::Oid::from_str(id).with_context(|| format!("invalid blob id {id}"))?;
    let blob = repo
        .find_blob(oid)
        .with_context(|| format!("blob {id} not found"))?;
    anyhow::ensure!(
        blob.size() <= MAX_IMAGE_BYTES,
        "image is too large to preview ({} bytes)",
        blob.size()
    );
    Ok(blob.content().to_vec())
}

fn read_workdir_file(repo: &Repository, rel_path: &str) -> Result<Vec<u8>> {
    let workdir = repo.workdir().context("bare repository")?;
    let path = workdir.join(rel_path);
    let len = std::fs::metadata(&path)
        .with_context(|| format!("cannot read '{rel_path}'"))?
        .len();
    anyhow::ensure!(
        len as usize <= MAX_IMAGE_BYTES,
        "image is too large to preview ({len} bytes)"
    );
    std::fs::read(&path).with_context(|| format!("cannot read '{rel_path}'"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::diff::ops::{
        get_single_file_diff, get_staged_single_file_diff, get_working_dir_single_file_diff,
    };
    use std::fs;
    use std::path::Path;

    const OLD: &[u8] = b"<svg xmlns='http://www.w3.org/2000/svg'><rect width='1'/></svg>";
    const NEW: &[u8] = b"<svg xmlns='http://www.w3.org/2000/svg'><rect width='2'/></svg>";

    fn commit_file(repo: &Repository, name: &str, bytes: &[u8]) -> String {
        fs::write(repo.workdir().unwrap().join(name), bytes).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(name)).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();
        let parents: Vec<git2::Commit<'_>> = repo
            .head()
            .ok()
            .and_then(|h| h.peel_to_commit().ok())
            .into_iter()
            .collect();
        let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, "msg", &tree, &parent_refs)
            .unwrap()
            .to_string()
    }

    #[test]
    fn image_kind_by_extension() {
        assert_eq!(image_kind("a/b/logo.PNG"), Some(ImageKind::Raster));
        assert_eq!(image_kind("icon.svg"), Some(ImageKind::Svg));
        assert_eq!(image_kind("photo.jpeg"), Some(ImageKind::Raster));
        assert_eq!(image_kind("main.rs"), None);
        assert_eq!(image_kind("Makefile"), None);
    }

    #[test]
    fn commit_diff_loads_both_sides() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        commit_file(&repo, "logo.svg", OLD);
        let oid = commit_file(&repo, "logo.svg", NEW);

        let diff = get_single_file_diff(&repo, &oid, "logo.svg").unwrap();
        let img = load_image_diff(&repo, &diff).unwrap();
        assert_eq!(img.kind, ImageKind::Svg);
        assert_eq!(img.old.as_deref(), Some(OLD));
        assert_eq!(img.new.as_deref(), Some(NEW));
    }

    #[test]
    fn added_file_has_no_old_side() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        let oid = commit_file(&repo, "logo.svg", OLD);

        let diff = get_single_file_diff(&repo, &oid, "logo.svg").unwrap();
        let img = load_image_diff(&repo, &diff).unwrap();
        assert!(img.old.is_none());
        assert_eq!(img.new.as_deref(), Some(OLD));
    }

    #[test]
    fn unstaged_change_reads_new_side_from_workdir() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        commit_file(&repo, "logo.svg", OLD);
        fs::write(tmp.path().join("logo.svg"), NEW).unwrap();

        let diff = get_working_dir_single_file_diff(&repo, "logo.svg").unwrap();
        let img = load_image_diff(&repo, &diff).unwrap();
        assert_eq!(img.old.as_deref(), Some(OLD));
        assert_eq!(img.new.as_deref(), Some(NEW));
    }

    #[test]
    fn untracked_file_reads_from_workdir() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        commit_file(&repo, "other.svg", OLD);
        fs::write(tmp.path().join("new.svg"), NEW).unwrap();

        let diff = get_working_dir_single_file_diff(&repo, "new.svg").unwrap();
        let img = load_image_diff(&repo, &diff).unwrap();
        assert!(img.old.is_none());
        assert_eq!(img.new.as_deref(), Some(NEW));
    }

    #[test]
    fn staged_change_reads_new_side_from_index() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        commit_file(&repo, "logo.svg", OLD);
        fs::write(tmp.path().join("logo.svg"), NEW).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("logo.svg")).unwrap();
        index.write().unwrap();
        // Change the working copy again: the staged view must still show NEW.
        fs::write(tmp.path().join("logo.svg"), b"<svg/>").unwrap();

        let diff = get_staged_single_file_diff(&repo, "logo.svg").unwrap();
        let img = load_image_diff(&repo, &diff).unwrap();
        assert_eq!(img.old.as_deref(), Some(OLD));
        assert_eq!(img.new.as_deref(), Some(NEW));
    }

    #[test]
    fn deleted_file_has_no_new_side() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        commit_file(&repo, "logo.svg", OLD);
        fs::remove_file(tmp.path().join("logo.svg")).unwrap();

        let diff = get_working_dir_single_file_diff(&repo, "logo.svg").unwrap();
        let img = load_image_diff(&repo, &diff).unwrap();
        assert_eq!(img.old.as_deref(), Some(OLD));
        assert!(img.new.is_none());
    }
}
