//! An in-memory index of the files under the user's folders, and matching a
//! query against their names.

use std::{
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

/// The index stops growing here; a home folder with more files than this is
/// mostly caches and dependencies nobody searches by name.
pub const MAX_FILES: usize = 300_000;
/// Folders deeper than this are not walked.
const MAX_DEPTH: usize = 8;

/// Folders that hold generated or third-party files: walking them costs
/// much and finds nothing a person named.
const SKIPPED: &[&str] = &[
    "node_modules",
    "target",
    "build",
    "dist",
    "out",
    "__pycache__",
    "venv",
    "site-packages",
    "vendor",
    "Pods",
    "DerivedData",
    "AppData",
    "Library",
    "go",
    "Caches",
    "cache",
];

#[derive(Clone, Debug, PartialEq)]
pub struct FileEntry {
    pub path: PathBuf,
    /// The file name, lowercased, which is what queries match.
    name: String,
    pub is_dir: bool,
    pub size: u64,
    /// Unix seconds.
    pub modified: u64,
}

impl FileEntry {
    pub fn new(path: PathBuf, is_dir: bool, size: u64, modified: u64) -> Self {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        Self {
            path,
            name,
            is_dir,
            size,
            modified,
        }
    }

    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

/// The folders searched: the home folder, whose usual subfolders (Desktop,
/// Documents, Downloads…) are found by walking it.
pub fn default_roots() -> Vec<PathBuf> {
    dirs::home_dir().into_iter().collect()
}

/// Walks `roots`, skipping hidden and generated folders. Blocking.
pub fn walk(roots: &[PathBuf]) -> Vec<FileEntry> {
    let mut files = Vec::new();
    let mut pending: Vec<(PathBuf, usize)> = roots.iter().map(|root| (root.clone(), 0)).collect();
    while let Some((directory, depth)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            if files.len() >= MAX_FILES {
                return files;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name.starts_with('$') {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            // Links can loop; the file they point at is indexed where it is.
            if file_type.is_symlink() {
                continue;
            }
            let metadata = entry.metadata().ok();
            let modified = metadata
                .as_ref()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |elapsed| elapsed.as_secs());
            let path = entry.path();
            if file_type.is_dir() {
                if SKIPPED
                    .iter()
                    .any(|skipped| name.eq_ignore_ascii_case(skipped))
                {
                    continue;
                }
                if depth < MAX_DEPTH {
                    pending.push((path.clone(), depth + 1));
                }
                files.push(FileEntry::new(path, true, 0, modified));
            } else {
                let size = metadata.map_or(0, |metadata| metadata.len());
                files.push(FileEntry::new(path, false, size, modified));
            }
        }
    }
    files
}

/// The best matches for `query`, best first, at most `limit`.
///
/// Every word of the query must appear in the file name. Among matches, a
/// name equal to the query beats one starting with it, which beats a word in
/// it starting with it; then shorter names and newer files come first.
pub fn search<'a>(
    files: &'a [FileEntry],
    query: &str,
    limit: usize,
    keep: impl Fn(&FileEntry) -> bool,
) -> Vec<&'a FileEntry> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if terms.is_empty() {
        return Vec::new();
    }
    let whole = terms.join(" ");
    let mut matches: Vec<(u32, &FileEntry)> = files
        .iter()
        .filter(|file| terms.iter().all(|term| file.name.contains(term.as_str())))
        .filter(|file| keep(file))
        .map(|file| (rank(&file.name, &whole, &terms[0]), file))
        .collect();
    matches.sort_by(|(a, left), (b, right)| {
        b.cmp(a)
            .then(left.name.len().cmp(&right.name.len()))
            .then(right.modified.cmp(&left.modified))
    });
    matches.truncate(limit);
    matches.into_iter().map(|(_, file)| file).collect()
}

fn rank(name: &str, whole: &str, first: &str) -> u32 {
    let stem = Path::new(name)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name == whole || stem == whole {
        4
    } else if name.starts_with(whole) {
        3
    } else if name
        .match_indices(first)
        .any(|(ix, _)| ix == 0 || !name.as_bytes()[ix - 1].is_ascii_alphanumeric())
    {
        2
    } else {
        1
    }
}

/// The most recently modified files (not folders), newest first.
pub fn recent(
    files: &[FileEntry],
    limit: usize,
    keep: impl Fn(&FileEntry) -> bool,
) -> Vec<&FileEntry> {
    let mut recent: Vec<&FileEntry> = files
        .iter()
        .filter(|file| !file.is_dir && keep(file))
        .collect();
    recent.sort_by_key(|file| std::cmp::Reverse(file.modified));
    recent.truncate(limit);
    recent
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, modified: u64) -> FileEntry {
        FileEntry::new(PathBuf::from(path), false, 1, modified)
    }

    #[test]
    fn test_search_ranks_exact_then_prefix_then_word_start() {
        let files = [
            file("/a/my-report-final.pdf", 5),
            file("/a/report.pdf", 1),
            file("/a/reports 2024.xlsx", 3),
            file("/a/unreported.txt", 9),
            file("/a/notes.md", 9),
        ];
        let names: Vec<String> = search(&files, "report", 10, |_| true)
            .into_iter()
            .map(FileEntry::name)
            .collect();
        assert_eq!(
            names,
            [
                "report.pdf",
                "reports 2024.xlsx",
                "my-report-final.pdf",
                "unreported.txt"
            ]
        );
        assert_eq!(search(&files, "REPORT 2024", 10, |_| true).len(), 1);
        assert!(search(&files, "  ", 10, |_| true).is_empty());
        assert_eq!(
            search(&files, "report", 10, |file| file.name().ends_with(".pdf")).len(),
            2
        );
    }

    #[test]
    fn test_walk_skips_hidden_and_generated_folders() {
        let root = tempfile::tempdir().unwrap();
        for path in [
            "Documents/plan.md",
            ".git/config",
            "code/node_modules/x/index.js",
            "code/src/main.rs",
        ] {
            let path = root.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "x").unwrap();
        }
        let mut names: Vec<String> = walk(&[root.path().to_path_buf()])
            .iter()
            .map(FileEntry::name)
            .collect();
        names.sort();
        assert_eq!(names, ["Documents", "code", "main.rs", "plan.md", "src"]);
        assert_eq!(
            recent(&walk(&[root.path().to_path_buf()]), 10, |_| true).len(),
            2
        );
    }
}
