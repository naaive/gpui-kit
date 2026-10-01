//! Finding the SQL files in an attached folder.

use std::path::{Path, PathBuf};

/// How deep a folder is searched; deeper files are rarely scripts.
const MAX_DEPTH: usize = 8;

/// Folders that hold build output or dependencies rather than scripts.
const SKIPPED: &[&str] = &["node_modules", "target"];

/// A folder or SQL file under an attached folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    path: PathBuf,
    /// A folder's entries, folders first; `None` for a file.
    children: Option<Vec<FileEntry>>,
}

impl FileEntry {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    pub fn children(&self) -> Option<&[FileEntry]> {
        self.children.as_deref()
    }
}

/// The SQL files under `folder`, and the folders that lead to them. A folder
/// with no SQL file anywhere in it is left out; the attached folder itself
/// is always there.
pub fn scan(folder: &Path) -> FileEntry {
    FileEntry {
        path: folder.to_path_buf(),
        children: Some(scan_children(folder, 0)),
    }
}

fn scan_children(folder: &Path, depth: usize) -> Vec<FileEntry> {
    let Ok(read) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut folders = Vec::new();
    let mut files = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if name.starts_with('.') || SKIPPED.contains(&name.as_str()) || depth >= MAX_DEPTH {
                continue;
            }
            let children = scan_children(&path, depth + 1);
            if !children.is_empty() {
                folders.push(FileEntry {
                    path,
                    children: Some(children),
                });
            }
        } else if is_sql(&path) {
            files.push(FileEntry {
                path,
                children: None,
            });
        }
    }
    let by_name = |a: &FileEntry, b: &FileEntry| {
        a.name()
            .to_lowercase()
            .cmp(&b.name().to_lowercase())
            .then_with(|| a.name().cmp(&b.name()))
    };
    folders.sort_by(by_name);
    files.sort_by(by_name);
    folders.extend(files);
    folders
}

fn is_sql(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("sql"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_sql_files_and_the_folders_leading_to_them_are_listed() {
        let root = std::env::temp_dir().join(format!("datakit-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for path in [
            "b.sql",
            "A.SQL",
            "notes.txt",
            "reports/monthly.sql",
            "empty/readme.md",
            ".git/hooks.sql",
            "node_modules/x.sql",
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "select 1").unwrap();
        }

        let entry = scan(&root);
        let names: Vec<String> = entry
            .children()
            .unwrap()
            .iter()
            .map(FileEntry::name)
            .collect();
        assert_eq!(names, ["reports", "A.SQL", "b.sql"]);
        let reports = &entry.children().unwrap()[0];
        assert_eq!(reports.children().unwrap()[0].name(), "monthly.sql");
        assert!(reports.children().unwrap()[0].children().is_none());

        std::fs::remove_dir_all(&root).unwrap();
    }
}
