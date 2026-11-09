//! digi4school books, through the digi_download_core library. Downloads run in a separate helper
//! process, because the library panics on unexpected responses and that mustn't take the app down.
//! The password reaches the helper over stdin, never as a command argument. It's only saved if you
//! choose that: in your desktop keyring when there is one, otherwise in a file only you can read.
//! Books stay on this computer.

use crate::config::{data_dir, find_bin};
use anyhow::{anyhow, bail, Context, Result};
use digi_download_core::digi4school::session::Session;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RemoteBook {
    pub title: String,
    pub year: u16,
}

#[derive(Deserialize)]
struct Login {
    email: String,
    password: String,
}

const KEYRING_SERVICE: &str = "lockin-digi4school";

fn password_file() -> PathBuf {
    match std::env::var_os("LOCKIN_HOME") {
        Some(p) => PathBuf::from(p).join("digi4school-password"),
        None => crate::config::home().join(".config/lockin/digi4school-password"),
    }
}

/// The keyring is skipped in a sandbox, so tests can't touch the real one.
fn keyring_available() -> bool {
    !crate::config::is_sandboxed() && find_bin("secret-tool").is_file()
}

pub fn load_password(email: &str) -> Option<String> {
    if keyring_available() {
        let out = Command::new(find_bin("secret-tool")).args(["lookup", "service", KEYRING_SERVICE, "account", email]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok();
        if let Some(out) = out.filter(|o| o.status.success() && !o.stdout.is_empty()) {
            return String::from_utf8(out.stdout).ok();
        }
    }
    std::fs::read_to_string(password_file()).ok().filter(|p| !p.is_empty())
}

/// Saves the password, and returns where it went so you can be told.
pub fn save_password(email: &str, password: &str) -> Result<String> {
    if keyring_available() {
        let child = Command::new(find_bin("secret-tool"))
            .args(["store", "--label=LockIn digi4school password", "service", KEYRING_SERVICE, "account", email])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(mut child) = child {
            let wrote = child.stdin.take().is_some_and(|mut s| s.write_all(password.as_bytes()).is_ok());
            if wrote && child.wait().is_ok_and(|st| st.success()) {
                std::fs::remove_file(password_file()).ok();
                return Ok("your desktop keyring".to_string());
            }
        }
    }
    use std::os::unix::fs::OpenOptionsExt;
    let path = password_file();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::remove_file(&path).ok();
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
    file.write_all(password.as_bytes())?;
    Ok(format!("{}, unencrypted but readable only by you", path.display()))
}

pub fn forget_password(email: &str) {
    if keyring_available() {
        let _ = Command::new(find_bin("secret-tool")).args(["clear", "service", KEYRING_SERVICE, "account", email]).stdin(Stdio::null()).stderr(Stdio::null()).status();
    }
    std::fs::remove_file(password_file()).ok();
}

/// `lockin books-debug`: reports the site's structure, with no personal data, using the saved login.
pub fn debug(email: &str) -> Result<()> {
    if email.is_empty() {
        bail!("Log in once from the Books screen first");
    }
    let password = load_password(email).context("No saved password. Log in from the Books screen and choose to save it")?;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async {
        let session = Session::new(email.to_string(), password).await.map_err(|e| anyhow!("Login failed: {e}"))?;
        print!("{}", session.debug_report().await);
        Ok(())
    })
}

pub fn books_dir() -> PathBuf {
    data_dir().join("books")
}

/// Book titles come from a website, so they become a single safe file name here.
pub fn safe_name(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| if c.is_alphanumeric() || " -_.,()".contains(c) { c } else { '_' })
        .take(100)
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.').trim();
    if trimmed.is_empty() { "book".into() } else { trimmed.to_string() }
}

/// Downloaded books, newest first. Unfinished downloads are in a hidden folder and don't count.
pub fn local_books() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if crate::pdf::is_pdf(&path) {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&books_dir(), &mut out);
    let modified = |p: &PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    out.sort_by_key(|p| std::cmp::Reverse(modified(p)));
    out
}

/// Only you can read the folder. Books are licensed to your account.
fn private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

/// Joins saved pages into one PDF. Poppler handles a long book far faster than merging one by one.
pub fn merge_parts(parts_dir: &Path, output: &Path) -> Result<()> {
    let mut parts: Vec<PathBuf> = std::fs::read_dir(parts_dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "pdf"))
        .collect();
    parts.sort();
    if parts.is_empty() {
        bail!("No pages were downloaded");
    }
    if parts.len() == 1 {
        std::fs::copy(&parts[0], output)?;
        return Ok(());
    }
    let out = Command::new(find_bin("pdfunite")).args(&parts).arg(output).stdin(Stdio::null()).output().context("pdfunite isn't available. Install poppler")?;
    if !out.status.success() {
        bail!("Couldn't join the pages: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

fn emit(value: serde_json::Value) {
    println!("{value}");
    let _ = std::io::stdout().flush();
}

/// The hidden `lockin books-helper` command. It reads the login from stdin and reports as JSON lines.
pub fn helper(mode: &str, title: Option<&str>) -> Result<()> {
    std::panic::set_hook(Box::new(|info| {
        // The library panics when the website's pages change, so this is shown as a normal error.
        let message: String = info.to_string().chars().take(300).collect();
        println!("{}", json!({"event": "error", "message": format!("digi4school sent something unexpected: {message}")}));
    }));
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let login: Login = serde_json::from_str(&line).context("no login received")?;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let result = runtime.block_on(run(mode, title, login));
    if let Err(e) = &result {
        emit(json!({"event": "error", "message": e.to_string()}));
    }
    result
}

async fn run(mode: &str, title: Option<&str>, login: Login) -> Result<()> {
    let session = Session::new(login.email, login.password).await.map_err(|e| anyhow!("Login failed: {e}"))?;
    let books = session.get_books().await.map_err(|e| anyhow!("Couldn't list your books: {e}"))?;
    if mode == "list" {
        let list: Vec<RemoteBook> = books.iter().map(|b| RemoteBook { title: b.title().to_string(), year: b.year() }).collect();
        emit(json!({"event": "books", "books": list}));
        return Ok(());
    }
    let wanted = title.context("no book given")?;
    let book = books.iter().find(|b| b.title() == wanted).with_context(|| format!("{wanted} isn't on your account"))?;
    let volumes = book.get_volumes().await.map_err(|e| anyhow!("Couldn't open the book: {e}"))?;

    let root = books_dir();
    private_dir(&root)?;
    let book_dir = root.join(safe_name(wanted));
    private_dir(&book_dir)?;
    let mut saved = Vec::new();
    for volume in &volumes {
        let name = safe_name(volume.name());
        let output = book_dir.join(format!("{name}.pdf"));
        if output.exists() {
            saved.push(output);
            continue;
        }
        // Each page is saved as it arrives, so an interrupted download picks up where it stopped.
        let parts = root.join(".parts").join(safe_name(wanted)).join(&name);
        private_dir(&parts)?;
        let scraper = volume.get_scraper().await.map_err(|e| anyhow!("Couldn't open {name}: {e}"))?;
        let total = scraper.fetch_page_count().await.map_err(|e| anyhow!("Couldn't count the pages: {e}"))?;
        for page in 1..=total {
            let part = parts.join(format!("{page:05}.pdf"));
            if !part.exists() {
                let bytes = scraper.fetch_page_raw_pdf(page).await.map_err(|e| anyhow!("Page {page} failed: {e}"))?;
                let tmp = part.with_extension("tmp");
                std::fs::write(&tmp, bytes)?;
                std::fs::rename(&tmp, &part)?;
            }
            emit(json!({"event": "progress", "volume": name, "page": page, "total": total}));
        }
        emit(json!({"event": "merging", "volume": name}));
        merge_parts(&parts, &output)?;
        std::fs::remove_dir_all(&parts).ok();
        saved.push(output);
    }
    std::fs::remove_dir_all(root.join(".parts").join(safe_name(wanted))).ok();
    emit(json!({"event": "done", "title": wanted, "count": saved.len()}));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_become_one_safe_file_name() {
        assert_eq!(safe_name("Mathematik 2 HTL"), "Mathematik 2 HTL");
        assert_eq!(safe_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(safe_name("..."), "book");
        assert_eq!(safe_name(""), "book");
        assert_eq!(safe_name("Größe: a/b"), "Größe_ a_b");
        assert!(!safe_name(".hidden").starts_with('.'));
        assert!(safe_name(&"x".repeat(500)).chars().count() <= 100);
    }

    #[test]
    fn saved_pages_are_joined_in_page_order() {
        // A smallest-possible PDF, so the test needs nothing but poppler.
        let page = |width: u32| {
            let body = format!("%PDF-1.4\n1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 {width} 200]>>endobj\ntrailer<</Root 1 0 R>>\n");
            body.into_bytes()
        };
        let dir = std::env::temp_dir().join(format!("lockin-books-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        // Written out of order: the zero-padded names must decide the order.
        std::fs::write(dir.join("00002.pdf"), page(222)).unwrap();
        std::fs::write(dir.join("00001.pdf"), page(111)).unwrap();
        std::fs::write(dir.join("00010.pdf"), page(333)).unwrap();
        let out = dir.join("out.result");
        merge_parts(&dir, &out).unwrap();
        let info = Command::new(find_bin("pdfinfo")).args(["-f", "1", "-l", "3"]).arg(&out).output().unwrap();
        let sizes = crate::pdf::parse_page_sizes(&String::from_utf8_lossy(&info.stdout));
        assert_eq!(sizes.iter().map(|s| s.0 as u32).collect::<Vec<_>>(), vec![111, 222, 333]);
        std::fs::remove_dir_all(&dir).ok();
        assert!(merge_parts(Path::new("/nonexistent-lockin-dir"), &out).is_err());
    }
}
