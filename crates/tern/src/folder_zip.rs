//! Zipping a folder so it can be attached (email has no folders; Apple Mail does the same).
//!
//! Hidden entries (names starting with `.`) and symlinks are left out: the first are usually
//! clutter (`.git`, `.DS_Store`), the second could pull in files from outside the folder.

use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// Folders bigger than this aren't zipped: the archive is built in memory, and no mail
/// server would take it anyway.
pub const MAX_FOLDER_BYTES: u64 = 200 * 1024 * 1024;

/// Extensions whose contents are already compressed; storing them is as small and faster.
const COMPRESSED: &[&str] = &[
    "zip", "gz", "tgz", "bz2", "xz", "zst", "7z", "rar", "jpg", "jpeg", "png", "gif", "webp", "heic",
    "avif", "mp3", "m4a", "aac", "ogg", "opus", "flac", "mp4", "m4v", "mov", "mkv", "webm", "pdf",
    "docx", "xlsx", "pptx", "odt", "ods", "odp", "epub", "jar", "apk",
];

struct Entry {
    /// Path inside the archive, `/`-separated, starting with the folder's own name.
    name: String,
    path: PathBuf,
    dir: bool,
}

/// Zip `dir` into `"<folder name>.zip"`. Fails if the files add up to more than `max_bytes`.
pub fn zip_folder(dir: &Path, max_bytes: u64) -> Result<(String, Vec<u8>), String> {
    let root = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Folder".into());
    let mut entries = vec![Entry { name: format!("{root}/"), path: dir.to_path_buf(), dir: true }];
    let mut total = 0u64;
    walk(dir, &root, &mut entries, &mut total, max_bytes)?;

    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for e in &entries {
        let meta = fs::metadata(&e.path).map_err(|err| format!("{}: {err}", e.name))?;
        let mut options = SimpleFileOptions::default().unix_permissions(permissions(&meta, e.dir));
        if let Some(t) = modified(&meta) {
            options = options.last_modified_time(t);
        }
        if e.dir {
            zip.add_directory(e.name.as_str(), options).map_err(|err| err.to_string())?;
            continue;
        }
        let method = if is_compressed(&e.name) { CompressionMethod::Stored } else { CompressionMethod::Deflated };
        zip.start_file(e.name.as_str(), options.compression_method(method).large_file(meta.len() >= u32::MAX as u64))
            .map_err(|err| err.to_string())?;
        let data = fs::read(&e.path).map_err(|err| format!("{}: {err}", e.name))?;
        zip.write_all(&data).map_err(|err| err.to_string())?;
    }
    let bytes = zip.finish().map_err(|err| err.to_string())?.into_inner();
    Ok((format!("{root}.zip"), bytes))
}

fn walk(dir: &Path, prefix: &str, out: &mut Vec<Entry>, total: &mut u64, max: u64) -> Result<(), String> {
    let mut children: Vec<_> = fs::read_dir(dir)
        .map_err(|e| format!("{prefix}: {e}"))?
        .filter_map(Result::ok)
        .collect();
    children.sort_by_key(|c| c.file_name());
    for child in children {
        let file_name = child.file_name().to_string_lossy().into_owned();
        if file_name.starts_with('.') {
            continue;
        }
        // symlink_metadata: don't follow links.
        let Ok(meta) = fs::symlink_metadata(child.path()) else { continue };
        let name = format!("{prefix}/{file_name}");
        if meta.is_dir() {
            out.push(Entry { name: format!("{name}/"), path: child.path(), dir: true });
            walk(&child.path(), &name, out, total, max)?;
        } else if meta.is_file() {
            *total += meta.len();
            if *total > max {
                return Err(format!("the folder is over {}", crate::format::human_size(max)));
            }
            out.push(Entry { name, path: child.path(), dir: false });
        }
        // Symlinks, sockets, devices: skipped.
    }
    Ok(())
}

fn is_compressed(name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    COMPRESSED.contains(&ext.as_str())
}

fn permissions(meta: &fs::Metadata, dir: bool) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    let mode = meta.permissions().mode() & 0o777;
    if mode == 0 { if dir { 0o755 } else { 0o644 } } else { mode }
}

/// File time as zip's local DOS time; `None` if it's outside what zip can store (1980–2107).
fn modified(meta: &fs::Metadata) -> Option<zip::DateTime> {
    let t: chrono::DateTime<chrono::Local> = meta.modified().ok()?.into();
    zip::DateTime::try_from(t.naive_local()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tern-zip-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn names(bytes: &[u8]) -> Vec<String> {
        let z = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut v: Vec<String> = z.file_names().map(|n| n.unwrap().into_owned()).collect();
        v.sort();
        v
    }

    #[test]
    fn zips_folder_tree_without_hidden_files_or_links() {
        let base = scratch("tree");
        let dir = base.join("Photos");
        fs::create_dir_all(dir.join("trip/day 1")).unwrap();
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::write(dir.join("notes.txt"), "hello hello hello hello").unwrap();
        fs::write(dir.join("trip/day 1/beach.jpg"), [0xFFu8, 0xD8, 1, 2, 3]).unwrap();
        fs::write(dir.join(".DS_Store"), "junk").unwrap();
        fs::write(dir.join(".git/config"), "secret").unwrap();
        fs::write(base.join("outside.txt"), "not yours").unwrap();
        std::os::unix::fs::symlink(base.join("outside.txt"), dir.join("link.txt")).unwrap();

        let (name, bytes) = zip_folder(&dir, MAX_FOLDER_BYTES).unwrap();
        assert_eq!(name, "Photos.zip");
        assert_eq!(
            names(&bytes),
            ["Photos/", "Photos/notes.txt", "Photos/trip/", "Photos/trip/day 1/", "Photos/trip/day 1/beach.jpg"]
        );

        let mut z = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
        let mut text = String::new();
        z.by_name("Photos/notes.txt").unwrap().read_to_string(&mut text).unwrap();
        assert_eq!(text, "hello hello hello hello");
        assert_eq!(z.by_name("Photos/notes.txt").unwrap().compression(), CompressionMethod::Deflated);
        assert_eq!(z.by_name("Photos/trip/day 1/beach.jpg").unwrap().compression(), CompressionMethod::Stored);
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn empty_folder_is_still_an_archive() {
        let base = scratch("empty");
        let dir = base.join("Empty");
        fs::create_dir_all(&dir).unwrap();
        let (_, bytes) = zip_folder(&dir, MAX_FOLDER_BYTES).unwrap();
        assert_eq!(names(&bytes), ["Empty/"]);
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn refuses_folders_over_the_limit() {
        let base = scratch("big");
        let dir = base.join("Big");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.bin"), vec![0u8; 600]).unwrap();
        fs::write(dir.join("b.bin"), vec![0u8; 600]).unwrap();
        let err = zip_folder(&dir, 1000).unwrap_err();
        assert!(err.contains("over"), "{err}");
        fs::remove_dir_all(&base).unwrap();
    }
}
