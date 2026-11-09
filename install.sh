#!/usr/bin/env bash
# Installs LockIn for the current user. Safe to run again to update: it keeps your settings,
# tasks and notes. Nothing goes outside your home folder except packages you agree to.
#
#   ./install.sh               install or update, asking as it goes
#   ./install.sh --yes         use the default answer for everything
#   ./install.sh --no-timer    don't set up the daily background job
#   ./install.sh --uninstall   remove the program and timer, and keep your data

set -euo pipefail

RUST_MIN="1.89"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="$HOME/.local/bin"
CONFIG="$HOME/.config/lockin/config.toml"
ASSUME_YES=false
WANT_TIMER=true
UNINSTALL=false

for arg in "$@"; do
    case "$arg" in
        -y | --yes) ASSUME_YES=true ;;
        --no-timer) WANT_TIMER=false ;;
        --uninstall) UNINSTALL=true ;;
        -h | --help)
            sed -n '2,8p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "Unknown option: $arg (try --help)" >&2
            exit 2
            ;;
    esac
done
# With no keyboard attached, nobody can answer, so use the defaults.
[ -t 0 ] || ASSUME_YES=true

if [ -t 1 ]; then
    ORANGE=$'\033[38;2;217;119;87m' GREEN=$'\033[38;2;143;168;118m' RED=$'\033[38;2;212;100;90m'
    DIM=$'\033[2m' BOLD=$'\033[1m' RESET=$'\033[0m'
else
    ORANGE="" GREEN="" RED="" DIM="" BOLD="" RESET=""
fi

step() { printf '\n%s✻ %s%s\n' "$ORANGE$BOLD" "$1" "$RESET"; }
ok() { printf '  %s✓%s %s\n' "$GREEN" "$RESET" "$1"; }
note() { printf '  %s\n' "$1"; }
warn() { printf '  %s!%s %s\n' "$RED" "$RESET" "$1"; }
die() {
    printf '\n%sInstall stopped:%s %s\n' "$RED$BOLD" "$RESET" "$1" >&2
    exit 1
}
has() { command -v "$1" >/dev/null 2>&1; }

# ask "question" y|n: returns 0 for yes
ask() {
    local answer hint="[y/N]"
    [ "$2" = y ] && hint="[Y/n]"
    if $ASSUME_YES; then
        [ "$2" = y ]
        return
    fi
    read -r -p "  $1 $hint " answer || answer=""
    case "${answer:-$2}" in
        [yY]*) return 0 ;;
        *) return 1 ;;
    esac
}

# ask_text "question" "default": prints the answer
ask_text() {
    local answer
    if $ASSUME_YES; then
        printf '%s' "$2"
        return
    fi
    read -r -p "  $1 ${2:+[$2] }" answer || answer=""
    printf '%s' "${answer:-$2}"
}

systemd_user_works() {
    has systemctl && systemctl --user show-environment >/dev/null 2>&1
}

if $UNINSTALL; then
    step "Removing LockIn"
    if systemd_user_works; then
        systemctl --user disable --now lockin-daily.timer >/dev/null 2>&1 || true
        systemctl --user disable --now lockin-listen.service >/dev/null 2>&1 || true
        systemctl --user disable --now lockin-remind.timer >/dev/null 2>&1 || true
    fi
    rm -f "$HOME/.config/systemd/user/lockin-daily.timer" "$HOME/.config/systemd/user/lockin-daily.service" "$HOME/.config/systemd/user/lockin-listen.service" "$HOME/.config/systemd/user/lockin-remind.timer" "$HOME/.config/systemd/user/lockin-remind.service" "$BIN_DIR/lockin"
    if systemd_user_works; then
        systemctl --user daemon-reload >/dev/null 2>&1 || true
    fi
    ok "Removed the program and its timers"
    note "Your data is still here. Delete these folders if you don't want it:"
    note "  $HOME/.config/lockin"
    note "  $HOME/.local/share/lockin"
    note "The moodle command and your notes were left alone."
    exit 0
fi

[ "$(uname -s)" = Linux ] || die "LockIn only runs on Linux for now, because its daily job uses systemd."
[ "$(id -u)" -ne 0 ] || die "Run this as your normal user, not with sudo. It asks for sudo only if it needs packages."
[ -f "$HERE/Cargo.toml" ] || die "Run this from inside the LockIn folder. Cargo.toml is missing."

printf '%s✻ LockIn installer%s\n' "$ORANGE$BOLD" "$RESET"
printf '%sA terminal organizer with Moodle sync and a daily Claude plan.%s\n' "$DIM" "$RESET"

# ---- system packages ---------------------------------------------------------

step "Checking system packages"
if has pacman; then
    PM="pacman"
    INSTALL=(sudo pacman -S --needed --noconfirm)
    PKG_PYTHON=python PKG_CURL=curl PKG_CC=gcc PKG_POPPLER=poppler PKG_NOTIFY=libnotify
elif has apt-get; then
    PM="apt"
    INSTALL=(sudo apt-get install -y)
    PKG_PYTHON=python3 PKG_CURL=curl PKG_CC=build-essential PKG_POPPLER=poppler-utils PKG_NOTIFY=libnotify-bin
elif has dnf; then
    PM="dnf"
    INSTALL=(sudo dnf install -y)
    PKG_PYTHON=python3 PKG_CURL=curl PKG_CC=gcc PKG_POPPLER=poppler-utils PKG_NOTIFY=libnotify
elif has zypper; then
    PM="zypper"
    INSTALL=(sudo zypper --non-interactive install)
    PKG_PYTHON=python3 PKG_CURL=curl PKG_CC=gcc PKG_POPPLER=poppler-tools PKG_NOTIFY=libnotify-tools
else
    PM=""
fi

missing_cmds=()
missing_pkgs=()
need() {
    if ! has "$1"; then
        missing_cmds+=("$1")
        missing_pkgs+=("${2:-}")
    fi
}
need python3 "${PKG_PYTHON:-}"
need curl "${PKG_CURL:-}"
need cc "${PKG_CC:-}"
need pdftoppm "${PKG_POPPLER:-}"
need notify-send "${PKG_NOTIFY:-}"

if [ "${#missing_cmds[@]}" -eq 0 ]; then
    ok "Everything needed is already installed"
elif [ -z "$PM" ]; then
    die "Please install these with your package manager, then run this again: ${missing_cmds[*]}"
else
    note "Missing: ${missing_cmds[*]}"
    if ask "Install ${missing_pkgs[*]} with $PM? This needs your sudo password." y; then
        [ "$PM" = apt ] && sudo apt-get update -qq
        "${INSTALL[@]}" "${missing_pkgs[@]}" || die "Installing the packages failed. See the error above."
        ok "Packages installed"
    else
        die "LockIn needs these. Install them yourself, then run this again: ${missing_pkgs[*]}"
    fi
fi

# ---- Rust --------------------------------------------------------------------

step "Checking Rust, used to build LockIn"
export PATH="$HOME/.cargo/bin:$PATH"

rust_is_new_enough() {
    has cargo || return 1
    local version
    version="$(cargo --version 2>/dev/null | awk '{print $2}')"
    [ -n "$version" ] || return 1
    [ "$(printf '%s\n%s\n' "$RUST_MIN" "$version" | sort -V | head -1)" = "$RUST_MIN" ]
}

if rust_is_new_enough; then
    ok "$(cargo --version)"
elif has rustup; then
    note "Your Rust is older than $RUST_MIN, so this updates the stable version through rustup."
    rustup toolchain install stable --profile minimal >/dev/null || die "rustup couldn't install Rust. See the error above."
    export RUSTUP_TOOLCHAIN=stable
    ok "$(cargo --version)"
else
    note "LockIn needs Rust $RUST_MIN or newer to build. It installs into ~/.cargo and ~/.rustup."
    if ask "Install it now from rustup.rs?" y; then
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path >/dev/null 2>&1 ||
            die "Installing Rust failed. See https://rustup.rs and run this again."
        export RUSTUP_TOOLCHAIN=stable
        ok "$(cargo --version)"
    else
        die "LockIn can't be built without Rust. Install it from https://rustup.rs and run this again."
    fi
fi

# ---- build -------------------------------------------------------------------

step "Building LockIn"
note "The first build takes a few minutes."
log="$(mktemp)"
if ! (cd "$HERE" && cargo build --release --locked) >"$log" 2>&1; then
    tail -25 "$log" >&2
    rm -f "$log"
    die "The build failed. The end of its output is above."
fi
rm -f "$log"
mkdir -p "$BIN_DIR"
install -m 755 "$HERE/target/release/lockin" "$BIN_DIR/lockin"
ok "Installed $BIN_DIR/lockin"

# ---- moodle CLI --------------------------------------------------------------

step "Setting up the moodle command"
if [ -x "$BIN_DIR/moodle" ] || has moodle; then
    ok "Already installed, so it was left as it is"
elif [ -f "$HERE/vendor/moodle" ]; then
    install -m 755 "$HERE/vendor/moodle" "$BIN_DIR/moodle"
    ok "Installed $BIN_DIR/moodle"
else
    warn "The moodle command isn't installed, and it doesn't come with this download."
    note "LockIn works without it, but it can't sync. To add it later, put it in $BIN_DIR/moodle."
fi
MOODLE="$(command -v moodle 2>/dev/null || echo "$BIN_DIR/moodle")"

# ---- PATH --------------------------------------------------------------------

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *)
        step "Adding ~/.local/bin to your PATH"
        case "$(basename "${SHELL:-bash}")" in
            zsh) rc="$HOME/.zshrc" ;;
            bash) rc="$HOME/.bashrc" ;;
            *) rc="" ;;
        esac
        line='export PATH="$HOME/.local/bin:$PATH"'
        if [ -z "$rc" ]; then
            warn "Add ~/.local/bin to your PATH in your shell's config, so you can type lockin."
        elif grep -qsF "$line" "$rc"; then
            ok "$rc already has it. Open a new terminal to use it."
        elif ask "Add it to $rc, so you can just type lockin?" y; then
            printf '\n# Added by the LockIn installer\n%s\n' "$line" >>"$rc"
            ok "Added. Open a new terminal to use it."
        else
            note "Skipped. Run $BIN_DIR/lockin directly, or add it yourself."
        fi
        ;;
esac
export PATH="$BIN_DIR:$PATH"

# ---- Claude ------------------------------------------------------------------

step "Checking Claude Code, for the daily plan and the Claude tab"
HAS_CLAUDE=false
if has claude; then
    HAS_CLAUDE=true
    ok "Installed"
else
    note "This is optional, and needs your own Claude account. Without it, LockIn still syncs"
    note "and reminds you, but sends a plain list of what's due instead of a plan."
    if ask "Install Claude Code now from claude.ai?" n; then
        if curl -fsSL https://claude.ai/install.sh | bash; then
            hash -r
            if has claude; then
                HAS_CLAUDE=true
                ok "Installed. Run claude once to log in, or the daily plan won't work."
            fi
        else
            warn "That didn't work. You can install it later and turn it on in the Settings tab."
        fi
    fi
fi

# ---- settings ----------------------------------------------------------------

step "Choosing your settings"
if [ -f "$CONFIG" ]; then
    ok "Keeping your existing settings"
else
    default_notes="$HOME/Documents/Notes"
    # If they use Obsidian, its open vault is the natural place for notes.
    obsidian_vault="$(python3 -I -c '
import json, os, sys
try:
    data = json.load(open(os.path.expanduser("~/.config/obsidian/obsidian.json")))
    vaults = data.get("vaults", {}).values()
    print(next((v["path"] for v in vaults if v.get("open")), next((v["path"] for v in vaults), "")))
except Exception:
    pass
' 2>/dev/null || true)"
    [ -n "$obsidian_vault" ] && [ -d "$obsidian_vault" ] && default_notes="$obsidian_vault"

    notes="$(ask_text "Notes folder (it's created if needed):" "$default_notes")"
    case "$notes" in
        "~/"*) notes="$HOME/${notes#\~/}" ;;
        /*) ;;
        *) notes="$HOME/$notes" ;;
    esac
    mkdir -p "$notes"

    while true; do
        time="$(ask_text "When should the daily reminder run? Use 24-hour time:" "15:00")"
        if printf '%s' "$time" | grep -Eq '^([01]?[0-9]|2[0-3]):[0-5][0-9]$'; then
            break
        fi
        warn "That isn't a time. Try something like 15:00."
    done

    about=""
    if $HAS_CLAUDE; then
        about="$(ask_text "Optional line for Claude about you, like your school and year:" "")"
    fi

    phone=false
    note "Phone reminders use the ntfy app. Your task titles go through the public ntfy.sh"
    note "server, under a private random name that only you get."
    ask "Turn on phone reminders?" n && phone=true

    mkdir -p "$(dirname "$CONFIG")"
    # Python does the quoting, so any path or text is written correctly.
    LOCKIN_NOTES="$notes" LOCKIN_TIME="$time" LOCKIN_ABOUT="$about" LOCKIN_PHONE="$phone" LOCKIN_CLAUDE="$HAS_CLAUDE" \
        python3 -I -c '
import json, os, sys
e = os.environ
q = json.dumps
h, m = e["LOCKIN_TIME"].split(":")
lines = [
    f"vault = {q(e['"'"'LOCKIN_NOTES'"'"'])}",
    f"about_me = {q(e['"'"'LOCKIN_ABOUT'"'"'], ensure_ascii=False)}",
    f"ntfy_enabled = {e['"'"'LOCKIN_PHONE'"'"']}",
    f"daily_use_claude = {e['"'"'LOCKIN_CLAUDE'"'"']}",
    f"daily_times = [{q(f'"'"'{int(h):02d}:{m}'"'"')}]",
]
open(sys.argv[1], "w").write("\n".join(lines) + "\n")
' "$CONFIG"
    "$BIN_DIR/lockin" status >/dev/null || die "LockIn couldn't read the new settings in $CONFIG."
    ok "Saved. You can change all of this later in the Settings tab."
fi

# ---- Moodle login ------------------------------------------------------------

step "Connecting to Moodle"
if [ ! -x "$MOODLE" ]; then
    note "Skipped, because the moodle command isn't installed."
elif grep -qs '"token"' "$HOME/.config/moodle-cli/config.json"; then
    ok "Already logged in"
elif $ASSUME_YES; then
    note "Not logged in yet. Run this when you're ready: moodle login"
elif ask "Log in to Moodle now? You'll need your school's Moodle address." y; then
    note "If your school signs in through Microsoft or Google, press Ctrl-C and run: moodle login --sso"
    "$MOODLE" login || warn "Login didn't finish. Try again any time with: moodle login"
    if grep -qs '"token"' "$HOME/.config/moodle-cli/config.json"; then
        "$BIN_DIR/lockin" sync && ok "Moodle synced" || warn "The first sync failed. Press gs in the app to try again."
    fi
else
    note "Skipped. Run this when you're ready: moodle login"
fi

# ---- daily timer -------------------------------------------------------------

step "Setting up the daily job"
if ! $WANT_TIMER; then
    note "Skipped, as you asked."
elif systemd_user_works; then
    "$BIN_DIR/lockin" install || warn "The timer couldn't be set up. Try later with: lockin install"
else
    warn "systemd user services aren't available here, so reminders won't run on their own."
    note "Everything else works, and you can run lockin daily yourself."
fi

# ---- notifications -----------------------------------------------------------

step "Checking desktop notifications"
if has busctl && busctl --user list 2>/dev/null | grep -q 'org.freedesktop.Notifications'; then
    ok "A notification service is running"
elif [ -n "${WAYLAND_DISPLAY:-}${DISPLAY:-}" ]; then
    warn "No notification service found, so desktop reminders won't appear."
    note "GNOME and KDE include one. On Hyprland or Sway, install mako and start it at login."
else
    note "No desktop session here, so this was skipped."
fi

printf '\n%s✻ All done.%s Open a new terminal and type %slockin%s. Press ? in the app for help.\n' "$ORANGE$BOLD" "$RESET" "$BOLD" "$RESET"
if [ -f "$CONFIG" ] && grep -q '^ntfy_enabled = true' "$CONFIG"; then
    note "For phone reminders, install the ntfy app and subscribe to the topic from: lockin status"
fi
case "${TERM:-}${TERM_PROGRAM:-}" in
    *ghostty* | *kitty* | *WezTerm* | *wezterm*) ;;
    *) note "Tip: Ghostty, kitty and WezTerm show PDFs as real pages. Other terminals show their text." ;;
esac
