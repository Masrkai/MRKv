use std::path::{Path, PathBuf};
use std::collections::HashSet;

pub struct FileTreeState {
    pub root_path: PathBuf,
    pub expanded_dirs: HashSet<PathBuf>,
    pub selected_path: Option<PathBuf>,
}

impl FileTreeState {
    pub fn new(root: PathBuf) -> Self {
        let mut expanded = HashSet::new();
        expanded.insert(root.clone());
        Self {
            root_path: root,
            expanded_dirs: expanded,
            selected_path: None,
        }
    }

    pub fn toggle_expand(&mut self, path: &Path) {
        if self.expanded_dirs.contains(path) {
            self.expanded_dirs.remove(path);
        } else {
            self.expanded_dirs.insert(path.to_path_buf());
        }
    }

    pub fn build_visible_list(&self) -> Vec<(FileNodeInfo, i32)> {
        let mut result = Vec::new();
        if !self.root_path.exists() {
            return result;
        }

        let _root_name = self.root_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Root")
            .to_string();

        self.collect_nodes(&self.root_path, 0, &mut result);
        result
    }

    fn collect_nodes(&self, dir: &Path, indent: i32, result: &mut Vec<(FileNodeInfo, i32)>) {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };

        let mut dirs = Vec::new();
        let mut files = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }

                let is_dir = path.is_dir();
                if is_dir {
                    dirs.push((name.to_string(), path));
                } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown") {
                        files.push((name.to_string(), path));
                    }
                }
            }
        }

        dirs.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
        files.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

        for (name, path) in dirs {
            let is_expanded = self.expanded_dirs.contains(&path);
            let _is_selected = self.selected_path.as_ref() == Some(&path);

            result.push((
                FileNodeInfo {
                    text: name,
                    path: path.to_string_lossy().to_string(),
                    is_dir: true,
                    is_expanded,
                },
                indent,
            ));

            if is_expanded {
                self.collect_nodes(&path, indent + 1, result);
            }
        }

        for (name, path) in files {
            let _is_selected = self.selected_path.as_ref() == Some(&path);
            result.push((
                FileNodeInfo {
                    text: name,
                    path: path.to_string_lossy().to_string(),
                    is_dir: false,
                    is_expanded: false,
                },
                indent,
            ));
        }
    }
}

#[derive(Clone, Debug)]
pub struct FileNodeInfo {
    pub text: String,
    pub path: String,
    pub is_dir: bool,
    pub is_expanded: bool,
}
