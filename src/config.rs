use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    /// Obsidian vault with your Markdown notes.
    pub vault: String,
    /// Folder inside the vault for notes created with `:note`.
    pub new_note_dir: String,
    pub editor: String,
    /// File-type icons in the Notes tab. They need a Nerd Font, which Ghostty includes.
    pub nerd_icons: bool,
    /// Model alias passed to `claude --model`.
    pub model: String,
    /// Model alias for the Tutor tab.
    pub tutor_model: String,
    /// Your digi4school login name. The password is never saved.
    pub digi4school_email: String,
    /// `auto` follows the language you write in, or name one like `English`.
    pub tutor_language: String,
    /// A line about you that Claude reads when planning, like your school and year.
    pub about_me: String,
    pub desktop_notifications: bool,
    pub ntfy_enabled: bool,
    pub ntfy_server: String,
    /// Anyone who knows this topic name can read your pushes, so keep it private.
    pub ntfy_topic: String,
    /// Lets messages sent to the topic control LockIn, through a background service.
    pub ntfy_replies: bool,
    /// The topic was replaced with a fresh one when replies were first turned on.
    pub ntfy_reply_topic_ready: bool,
    pub focus_minutes: i64,
    pub break_minutes: i64,
    /// Runs the systemd timer at all.
    pub daily_enabled: bool,
    /// One or more `HH:MM` times for the daily run.
    pub daily_times: Vec<String>,
    pub daily_weekdays_only: bool,
    /// Runs after the next login if the computer was off at the scheduled time.
    pub daily_catch_up: bool,
    /// Without Claude, the daily run sends a plain list of what's due.
    pub daily_use_claude: bool,
    pub sync_on_open: bool,
    /// The daily job plans study sessions for exams it hasn't planned yet.
    pub study_auto_plan: bool,
    pub study_session_minutes: i64,
    pub study_max_per_day: i64,
    /// Latest finish on a school day.
    pub study_until: String,
    /// Study hours on weekends and holidays.
    pub study_free_day_from: String,
    pub study_free_day_until: String,
    /// Minutes between Moodle syncs while the app is open. 0 turns this off.
    pub sync_interval_minutes: i64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            vault: home().join("Documents/Notes").display().to_string(),
            new_note_dir: "LockIn".into(),
            editor: std::env::var("EDITOR").ok().filter(|e| !e.is_empty()).unwrap_or_else(|| "nvim".into()),
            nerd_icons: true,
            model: "sonnet".into(),
            tutor_model: "sonnet".into(),
            tutor_language: "auto".into(),
            digi4school_email: String::new(),
            about_me: String::new(),
            desktop_notifications: true,
            // Off until you choose it, because task titles get sent to a public server.
            ntfy_enabled: false,
            ntfy_server: "https://ntfy.sh".into(),
            ntfy_topic: String::new(),
            ntfy_replies: false,
            ntfy_reply_topic_ready: false,
            focus_minutes: 25,
            break_minutes: 5,
            daily_enabled: true,
            daily_times: vec!["15:00".into()],
            daily_weekdays_only: false,
            daily_catch_up: true,
            daily_use_claude: true,
            sync_on_open: true,
            study_auto_plan: true,
            study_session_minutes: 50,
            study_max_per_day: 2,
            study_until: "20:00".into(),
            study_free_day_from: "10:00".into(),
            study_free_day_until: "18:00".into(),
            sync_interval_minutes: 60,
        }
    }
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

/// `LOCKIN_HOME` keeps everything in one folder, which tests use to stay away from real data.
pub fn data_dir() -> PathBuf {
    match std::env::var_os("LOCKIN_HOME") {
        Some(p) => PathBuf::from(p),
        None => home().join(".local/share/lockin"),
    }
}

pub fn is_sandboxed() -> bool {
    std::env::var_os("LOCKIN_HOME").is_some()
}

pub fn config_path() -> PathBuf {
    match std::env::var_os("LOCKIN_HOME") {
        Some(p) => PathBuf::from(p).join("config.toml"),
        None => home().join(".config/lockin/config.toml"),
    }
}

pub fn db_path() -> PathBuf {
    data_dir().join("lockin.db")
}

pub fn random_topic() -> Result<String> {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("lockin-{hex}"))
}

pub fn save(cfg: &Config) -> Result<()> {
    let path = config_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, toml::to_string_pretty(cfg)?)?;
    Ok(())
}

pub fn load() -> Result<Config> {
    let path = config_path();
    let mut cfg: Config = match std::fs::read_to_string(&path) {
        Ok(s) => toml::from_str(&s).with_context(|| format!("parsing {}", path.display()))?,
        Err(_) => Config::default(),
    };
    if cfg.ntfy_topic.is_empty() || !path.exists() {
        if cfg.ntfy_topic.is_empty() {
            cfg.ntfy_topic = random_topic()?;
        }
        save(&cfg)?;
    }
    std::fs::create_dir_all(data_dir())?;
    Ok(cfg)
}

/// Accepts `15:00`, `7:30, 15:00` or `07:30 15:00`, and returns them tidy, sorted and unique.
pub fn parse_times(input: &str) -> Result<Vec<String>> {
    let mut times = Vec::new();
    for tok in input.split(|c: char| c == ',' || c.is_whitespace()).filter(|t| !t.is_empty()) {
        let Ok(t) = chrono::NaiveTime::parse_from_str(tok, "%H:%M") else {
            bail!("'{tok}' isn't a time. Use 24-hour times like 07:30, 15:00");
        };
        times.push(t.format("%H:%M").to_string());
    }
    if times.is_empty() {
        bail!("Enter at least one time, or turn the daily run off");
    }
    times.sort();
    times.dedup();
    Ok(times)
}

/// Finds a program even when systemd runs us with a minimal PATH.
pub fn find_bin(name: &str) -> PathBuf {
    let on_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).map(|d| d.join(name)).find(|f| f.is_file()))
        .unwrap_or(None);
    on_path.unwrap_or_else(|| home().join(".local/bin").join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_tidied_and_validated() {
        assert_eq!(parse_times("15:00, 7:30  15:00").unwrap(), vec!["07:30", "15:00"]);
        assert!(parse_times("25:00").is_err());
        assert!(parse_times("soon").is_err());
        assert!(parse_times("  ").is_err());
    }

    #[test]
    fn old_config_files_gain_the_new_defaults() {
        let cfg: Config = toml::from_str("vault = \"/v\"\nntfy_topic = \"t\"\n").unwrap();
        assert_eq!(cfg.vault, "/v");
        assert_eq!(cfg.daily_times, vec!["15:00"]);
        assert!(cfg.daily_enabled && cfg.sync_on_open);
    }
}
