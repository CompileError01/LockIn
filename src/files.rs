//! The file logic behind the Yazi-style Notes tab. It never leaves the vault.

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Sort {
    Name,
    Modified,
    Size,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub modified: i64,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Yank {
    pub path: PathBuf,
    pub cut: bool,
}

pub enum Preview {
    None,
    Dir(Vec<Entry>),
    Markdown(String),
    Text(String),
    Pdf { pages: usize, text: String },
    Binary,
}

const PREVIEW_BYTES: usize = 256 * 1024;

pub fn list_dir(dir: &Path, show_hidden: bool, sort: Sort) -> Vec<Entry> {
    let Ok(read) = std::fs::read_dir(dir) else { return vec![] };
    let mut entries: Vec<Entry> = read
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !show_hidden && name.starts_with('.') {
                return None;
            }
            let meta = std::fs::metadata(e.path()).or_else(|_| e.metadata()).ok()?;
            let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64);
            Some(Entry { name, path: e.path(), is_dir: meta.is_dir(), size: meta.len(), modified })
        })
        .collect();
    entries.sort_by(|a, b| {
        b.is_dir.cmp(&a.is_dir).then_with(|| match sort {
            Sort::Name => natural_cmp(&a.name, &b.name),
            Sort::Modified => b.modified.cmp(&a.modified),
            Sort::Size => b.size.cmp(&a.size),
        })
    });
    entries
}

/// Sorts `note 2` before `note 10`, ignoring case.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (a, b) = (a.to_lowercase(), b.to_lowercase());
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(p), Some(q)) if p.is_ascii_digit() && q.is_ascii_digit() => {
                let number = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut n = 0u128;
                    while let Some(d) = it.peek().and_then(|c| c.to_digit(10)) {
                        n = n.saturating_mul(10).saturating_add(d as u128);
                        it.next();
                    }
                    n
                };
                let (l, r) = (number(&mut x), number(&mut y));
                if l != r {
                    return l.cmp(&r);
                }
            }
            (Some(p), Some(q)) => {
                if p != q {
                    return p.cmp(&q);
                }
                x.next();
                y.next();
            }
        }
    }
}

fn read_head(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.take(PREVIEW_BYTES as u64).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

fn looks_textual(bytes: &[u8]) -> bool {
    !bytes.iter().take(4096).any(|b| *b == 0)
}

pub fn is_text_file(path: &Path) -> bool {
    read_head(path).is_some_and(|b| looks_textual(&b))
}

pub fn preview(entry: Option<&Entry>, show_hidden: bool, sort: Sort) -> Preview {
    let Some(e) = entry else { return Preview::None };
    if e.is_dir {
        return Preview::Dir(list_dir(&e.path, show_hidden, sort));
    }
    if crate::pdf::is_pdf(&e.path) {
        return match crate::pdf::summary(&e.path) {
            Some((pages, text)) => Preview::Pdf { pages, text },
            None => Preview::Binary,
        };
    }
    let Some(bytes) = read_head(&e.path) else { return Preview::Binary };
    if !looks_textual(&bytes) {
        return Preview::Binary;
    }
    let text = String::from_utf8_lossy(&bytes).to_string();
    if e.path.extension().is_some_and(|x| x.eq_ignore_ascii_case("md")) {
        Preview::Markdown(text)
    } else {
        Preview::Text(text)
    }
}

pub fn human_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n}B")
    } else if value < 10.0 {
        format!("{value:.1}{}", UNITS[unit])
    } else {
        format!("{value:.0}{}", UNITS[unit])
    }
}

/// Rejects anything that could climb out of the current folder.
fn safe_relative(name: &str) -> Result<PathBuf> {
    let path = PathBuf::from(name);
    let ok = !name.is_empty() && path.components().all(|c| matches!(c, Component::Normal(_)));
    if !ok {
        bail!("Use a plain name or sub/folder/name");
    }
    Ok(path)
}

/// A name that isn't taken yet: `Note.md`, then `Note copy.md`, then `Note copy 2.md`.
pub fn free_name(dir: &Path, name: &str, suffix: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let as_path = Path::new(name);
    let stem = as_path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = as_path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let mut n = 1;
    loop {
        let label = if n == 1 { format!("{stem} {suffix}{ext}") } else { format!("{stem} {suffix} {n}{ext}") };
        let candidate = dir.join(label);
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

/// `a/b/` makes folders. Any other name makes a file, and a name with no extension becomes a note.
/// Returns the path and whether it should open in the editor.
pub fn create(dir: &Path, name: &str) -> Result<(PathBuf, bool)> {
    let wants_dir = name.ends_with('/');
    let rel = safe_relative(name.trim_end_matches('/'))?;
    if wants_dir {
        let path = dir.join(rel);
        std::fs::create_dir_all(&path)?;
        return Ok((path, false));
    }
    let with_ext = if rel.extension().is_none() { rel.with_extension("md") } else { rel };
    let path = dir.join(&with_ext);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if !path.exists() {
        let is_md = path.extension().is_some_and(|e| e == "md");
        let title = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        std::fs::write(&path, if is_md { format!("# {title}\n\n") } else { String::new() })?;
    }
    Ok((path, true))
}

pub fn rename(path: &Path, new_name: &str) -> Result<PathBuf> {
    if new_name.contains('/') {
        bail!("A new name can't contain /. Use x and p to move things");
    }
    let rel = safe_relative(new_name)?;
    let dest = path.parent().context("no parent folder")?.join(rel);
    if dest.exists() {
        bail!("{new_name} already exists");
    }
    std::fs::rename(path, &dest)?;
    Ok(dest)
}

/// Moves to the vault's `.trash`, the same place Obsidian uses, so nothing is destroyed.
pub fn trash(vault: &Path, path: &Path) -> Result<PathBuf> {
    let bin = vault.join(".trash");
    std::fs::create_dir_all(&bin)?;
    let name = path.file_name().context("no file name")?.to_string_lossy().to_string();
    let dest = free_name(&bin, &name, "old");
    std::fs::rename(path, &dest)?;
    Ok(dest)
}

fn copy_recursive(from: &Path, to: &Path) -> Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)?.flatten() {
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
    } else {
        std::fs::copy(from, to)?;
    }
    Ok(())
}

/// Pastes into `dir`. Returns where it landed.
pub fn paste(yank: &Yank, dir: &Path) -> Result<PathBuf> {
    if !yank.path.exists() {
        bail!("{} no longer exists", yank.path.display());
    }
    if yank.path.is_dir() && dir.starts_with(&yank.path) {
        bail!("Can't put a folder inside itself");
    }
    let name = yank.path.file_name().context("no file name")?.to_string_lossy().to_string();
    if yank.cut && yank.path.parent() == Some(dir) {
        return Ok(yank.path.clone()); // cut and pasted in place: nothing to do
    }
    let dest = free_name(dir, &name, "copy");
    if yank.cut {
        std::fs::rename(&yank.path, &dest)?;
    } else {
        copy_recursive(&yank.path, &dest)?;
    }
    Ok(dest)
}

/// Where you are in the vault, and which entry you last had selected in each folder.
pub struct Browser {
    pub root: PathBuf,
    pub cwd: PathBuf,
    pub entries: Vec<Entry>,
    pub parent: Vec<Entry>,
    pub show_hidden: bool,
    pub sort: Sort,
    pub yank: Option<Yank>,
    remembered: HashMap<PathBuf, String>,
}

impl Browser {
    pub fn new(root: PathBuf) -> Browser {
        let mut b = Browser {
            cwd: root.clone(),
            root,
            entries: vec![],
            parent: vec![],
            show_hidden: false,
            sort: Sort::Name,
            yank: None,
            remembered: HashMap::new(),
        };
        b.refresh();
        b
    }

    pub fn refresh(&mut self) {
        if !self.cwd.is_dir() || !self.cwd.starts_with(&self.root) {
            self.cwd = self.root.clone();
        }
        self.entries = list_dir(&self.cwd, self.show_hidden, self.sort);
        self.parent = match self.parent_dir() {
            Some(p) => list_dir(&p, self.show_hidden, self.sort),
            None => vec![],
        };
    }

    /// `None` at the vault root, so you can't browse out of the vault.
    pub fn parent_dir(&self) -> Option<PathBuf> {
        if self.cwd == self.root {
            return None;
        }
        self.cwd.parent().map(Path::to_path_buf)
    }

    pub fn remember(&mut self, name: &str) {
        self.remembered.insert(self.cwd.clone(), name.to_string());
    }

    pub fn index_of(&self, name: &str, visible: &[&Entry]) -> usize {
        let _ = self;
        visible.iter().position(|e| e.name == name).unwrap_or(0)
    }

    pub fn remembered_name(&self) -> Option<String> {
        self.remembered.get(&self.cwd).cloned()
    }

    /// Goes to a folder inside the vault. Anything else is ignored.
    pub fn enter(&mut self, dir: &Path) {
        if dir.is_dir() && dir.starts_with(&self.root) {
            self.cwd = dir.to_path_buf();
            self.refresh();
        }
    }

    /// Returns the folder you came from, so it can stay selected in the parent.
    pub fn leave(&mut self) -> Option<String> {
        let parent = self.parent_dir()?;
        let came_from = self.cwd.file_name().map(|n| n.to_string_lossy().to_string());
        self.cwd = parent;
        self.refresh();
        came_from
    }

    pub fn location(&self) -> String {
        let vault = self.root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "vault".into());
        match self.cwd.strip_prefix(&self.root) {
            Ok(rel) if rel.as_os_str().is_empty() => vault,
            Ok(rel) => format!("{vault}/{}", rel.display()),
            Err(_) => vault,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lockin-files-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn folders_come_first_and_numbers_sort_naturally() {
        let d = sandbox("sort");
        for f in ["note 10.md", "note 2.md", "Alpha.md", ".hidden"] {
            std::fs::write(d.join(f), "x").unwrap();
        }
        std::fs::create_dir(d.join("zeta")).unwrap();
        let names: Vec<String> = list_dir(&d, false, Sort::Name).into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["zeta", "Alpha.md", "note 2.md", "note 10.md"]);
        assert_eq!(list_dir(&d, true, Sort::Name).len(), 5);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn browser_stays_inside_the_vault() {
        let d = sandbox("bounds");
        let outside = sandbox("outside");
        std::fs::create_dir_all(d.join("a/b")).unwrap();
        let mut b = Browser::new(d.clone());
        assert_eq!(b.leave(), None, "can't go above the root");
        b.enter(&outside);
        assert_eq!(b.cwd, d, "can't enter a folder outside the vault");
        b.enter(&d.join("a/b"));
        assert_eq!(b.leave().as_deref(), Some("b"));
        assert_eq!(b.location(), format!("{}/a", d.file_name().unwrap().to_string_lossy()));
        std::fs::remove_dir_all(&d).ok();
        std::fs::remove_dir_all(&outside).ok();
    }

    #[test]
    fn create_refuses_escapes_and_defaults_to_notes() {
        let d = sandbox("create");
        assert!(create(&d, "../evil").is_err());
        assert!(create(&d, "/etc/evil").is_err());
        assert!(create(&d, "a/../../evil").is_err());
        assert!(!d.parent().unwrap().join("evil.md").exists());

        let (note, open) = create(&d, "sub/Plan").unwrap();
        assert!(open && note.ends_with("sub/Plan.md"));
        assert_eq!(std::fs::read_to_string(&note).unwrap(), "# Plan\n\n");
        let (folder, open) = create(&d, "new/folder/").unwrap();
        assert!(!open && folder.is_dir());

        // Opening an existing name must not wipe it.
        std::fs::write(&note, "my work").unwrap();
        create(&d, "sub/Plan").unwrap();
        assert_eq!(std::fs::read_to_string(&note).unwrap(), "my work");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn rename_never_overwrites() {
        let d = sandbox("rename");
        std::fs::write(d.join("a.md"), "a").unwrap();
        std::fs::write(d.join("b.md"), "b").unwrap();
        assert!(rename(&d.join("a.md"), "b.md").is_err());
        assert!(rename(&d.join("a.md"), "../c.md").is_err());
        assert_eq!(std::fs::read_to_string(d.join("b.md")).unwrap(), "b");
        assert!(rename(&d.join("a.md"), "c.md").unwrap().exists());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn trash_keeps_files_with_the_same_name() {
        let d = sandbox("trash");
        std::fs::write(d.join("a.md"), "first").unwrap();
        let one = trash(&d, &d.join("a.md")).unwrap();
        std::fs::write(d.join("a.md"), "second").unwrap();
        let two = trash(&d, &d.join("a.md")).unwrap();
        assert_ne!(one, two);
        assert_eq!(std::fs::read_to_string(one).unwrap(), "first");
        assert_eq!(std::fs::read_to_string(two).unwrap(), "second");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn paste_copies_moves_and_guards_folders() {
        let d = sandbox("paste");
        std::fs::create_dir_all(d.join("dir/inner")).unwrap();
        std::fs::write(d.join("dir/inner/f.md"), "x").unwrap();
        std::fs::create_dir(d.join("to")).unwrap();

        let copy = Yank { path: d.join("dir"), cut: false };
        let dest = paste(&copy, &d.join("to")).unwrap();
        assert!(dest.join("inner/f.md").exists() && d.join("dir/inner/f.md").exists());
        let again = paste(&copy, &d.join("to")).unwrap();
        assert!(again.ends_with("dir copy"), "second paste gets a new name");

        assert!(paste(&Yank { path: d.join("dir"), cut: true }, &d.join("dir/inner")).is_err());

        let cut = Yank { path: d.join("dir/inner/f.md"), cut: true };
        let moved = paste(&cut, &d).unwrap();
        assert!(moved.exists() && !d.join("dir/inner/f.md").exists());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn preview_tells_text_from_binary() {
        let d = sandbox("preview");
        std::fs::write(d.join("a.md"), "# hi").unwrap();
        std::fs::write(d.join("b.dll"), [0u8, 1, 2]).unwrap();
        let list = list_dir(&d, false, Sort::Name);
        assert!(matches!(preview(list.first(), false, Sort::Name), Preview::Markdown(_)));
        assert!(matches!(preview(list.get(1), false, Sort::Name), Preview::Binary));
        assert!(matches!(preview(None, false, Sort::Name), Preview::None));
        assert_eq!(human_size(512), "512B");
        assert_eq!(human_size(2048), "2.0K");
        assert_eq!(human_size(50 * 1024 * 1024), "50M");
        std::fs::remove_dir_all(&d).ok();
    }
}
