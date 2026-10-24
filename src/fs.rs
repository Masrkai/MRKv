use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

struct Entry {
    name: String,
    path: PathBuf,
    is_dir: bool,
}

pub struct FileTreeState {
    pub root_path: PathBuf,
    pub expanded_dirs: HashSet<PathBuf>,
    pub selected_path: Option<PathBuf>,
    cache: RefCell<HashMap<PathBuf, Rc<Vec<Entry>>>>,
}

impl FileTreeState {
    pub fn new(root: PathBuf) -> Self {
        let mut expanded = HashSet::new();
        expanded.insert(root.clone());
        Self {
            root_path: root,
            expanded_dirs: expanded,
            selected_path: None,
            cache: RefCell::new(HashMap::new()),
        }
    }

    /// Forget all cached directory listings (call when changing root).
    pub fn refresh(&self) {
        self.cache.borrow_mut().clear();
    }

    pub fn toggle_expand(&mut self, path: &Path) {
        if self.expanded_dirs.remove(path) {
            return;
        }
        self.expanded_dirs.insert(path.to_path_buf());
        // Re-read a directory every time it is opened so it is never stale.
        self.cache.borrow_mut().remove(path);
    }

    pub fn build_visible_list(&self) -> Vec<(FileNodeInfo, i32)> {
        let mut result = Vec::new();
        if self.root_path.exists() {
            self.collect_nodes(&self.root_path, 0, &mut result);
        }
        result
    }

    fn listing(&self, dir: &Path) -> Rc<Vec<Entry>> {
        if let Some(hit) = self.cache.borrow().get(dir) {
            return Rc::clone(hit);
        }

        let mut dirs = Vec::new();
        let mut files = Vec::new();

        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if name.starts_with('.') {
                    continue;
                }
                // file_type() avoids an extra stat; only symlinks need one.
                let is_dir = match entry.file_type() {
                    Ok(t) if t.is_symlink() => path.is_dir(),
                    Ok(t) => t.is_dir(),
                    Err(_) => false,
                };
                let name = name.to_string();
                if is_dir {
                    dirs.push(Entry {
                        name,
                        path,
                        is_dir: true,
                    });
                } else if path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map_or(false, |e| {
                        e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown")
                    })
                {
                    files.push(Entry {
                        name,
                        path,
                        is_dir: false,
                    });
                }
            }
        }

        dirs.sort_by_cached_key(|e| e.name.to_lowercase());
        files.sort_by_cached_key(|e| e.name.to_lowercase());
        dirs.extend(files);

        let list = Rc::new(dirs);
        self.cache
            .borrow_mut()
            .insert(dir.to_path_buf(), Rc::clone(&list));
        list
    }

    fn collect_nodes(&self, dir: &Path, indent: i32, result: &mut Vec<(FileNodeInfo, i32)>) {
        let list = self.listing(dir);
        for entry in list.iter() {
            let is_expanded = entry.is_dir && self.expanded_dirs.contains(&entry.path);
            result.push((
                FileNodeInfo {
                    text: entry.name.clone(),
                    path: entry.path.to_string_lossy().to_string(),
                    is_dir: entry.is_dir,
                    is_expanded,
                },
                indent,
            ));
            if is_expanded {
                self.collect_nodes(&entry.path, indent + 1, result);
            }
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
