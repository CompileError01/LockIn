# LockIn

A Neovim-style terminal organizer with tasks, a calendar, Obsidian notes, habits, grades and a
focus timer. It syncs with Moodle, and Claude writes a plan for you every day at the times you choose.

## Install

Works on Linux. In this folder, run:

```
./install.sh
```

It checks what's needed, asks before installing anything, builds LockIn, and walks you through the
setup. Run it again to update, which keeps your settings and data. `./install.sh --uninstall` removes
the program and keeps your data. To send LockIn to someone, run `./package.sh` and send them
`lockin.tar.gz`.

You'll need a Moodle account and the `moodle` terminal client, which LockIn uses to reach Moodle. It
isn't part of this repo, so put it at `~/.local/bin/moodle`. A Claude account is optional: without one, LockIn sends a plain list of
what's due instead of a plan.

## Commands

```
lockin            open the app (press ? for all keys, Space for the shortcut menu)
lockin daily      sync Moodle, ask Claude, send notifications (what the timer runs)
lockin sync       pull deadlines, events and grades from Moodle
lockin brief      ask Claude for a new plan and print it
lockin today      print open tasks
lockin add "Read chapter 4 @fri 18:00 !! #AM ~45m"
lockin pdf FILE   read a PDF in the terminal
lockin teach FILE "question"   ask a teaching chat about a file
lockin notify     send a test notification
lockin status     show paths, phone topic and last run times
lockin install    apply your settings to the daily systemd timer
```

## How the daily run works

`lockin-daily.timer` starts `lockin daily` at the times in the Settings tab (15:00 by default). If the
computer was off, it runs after your next login. The job:

1. Reads Moodle through `src/bridge.py`, which only reads and reuses the login from the `moodle` CLI.
2. Merges the results. New deadlines become tasks, and anything gone from Moodle's timeline closes
   itself because it was submitted. Deadlines you ticked off yourself stay ticked off.
3. Runs `claude -p` to rank the work, estimate effort and write the plan.
4. Sends the summary to your desktop through mako and to your phone through ntfy.

If Moodle or Claude fails, you still get a notification from the latest data, with a warning.

## What Claude can see and do

Claude runs with all tools turned off, so it can only return text. Assignment text written by other
people can't make it submit, delete or run anything. It sees your tasks, calendar, habits, focus time
and unread Moodle notifications. **It never sees your vault notes**, unless you add a file from your notes to
a chat, and then it sees only that file.

Priorities you set yourself with `+` or `-` are locked, and Claude won't change them.

## Files

| What | Where |
| --- | --- |
| Settings | The Settings tab, stored in `~/.config/lockin/config.toml` |
| Tasks, calendar, habits | `~/.local/share/lockin/lockin.db` |
| Notes | The folder you chose. `:note` puts new notes in `LockIn/`, and daily notes follow Obsidian's settings if it's a vault |
| Timer | `~/.config/systemd/user/lockin-daily.{timer,service}` |
| Timer logs | `journalctl --user -u lockin-daily` |

To change the schedule, use the Settings tab, which updates the timer for you.

## Assignments

Press Enter on a Moodle task to open it on the moodle CLI's own page. There, `u` uploads files, `t`
submits online text, `d` downloads attachments, Esc goes to its assignment list and `q` returns to
LockIn, which then syncs so submitted work closes itself. `gx` opens the task in your browser, and
`:moodle` opens the full CLI.

## PDFs

Press Enter on a PDF in the Notes tab, use `:pdf file`, or run `lockin pdf file`. Pages are drawn as
real images in Ghostty, and also in kitty and WezTerm. Other terminals get the PDF's text. Files you
open from the Moodle CLI through LockIn (Enter on a task, or `:moodle`) use this viewer too. To get the
same when you run `moodle` on its own, add this to your shell config:

```
export MOODLE_OPENER="lockin open"
```

Needs `poppler`.

## Chats

The Chats tab holds your conversations with Claude. Each chat has its own files, which can be any PDFs or
text files from your books, Moodle downloads or notes. Press `o` for a new chat, then `a` to add files.
Claude sees only the files you add. It reads short files right away. In long books it searches for the
right page and looks at the image of it, then tells you which page a number came from.

**Teach mode.** Press `t` in any chat. Claude then uses your teacher's methods, works step by step, and
on graded assignments guides you instead of writing answers you could hand in. With teach mode off, it
answers directly.

**Learning from a course.** Press `c` to see your courses, `s` to download a course's files, and Enter on
an exercise or a PDF. That opens a chat in teach mode. You can also press `L` on a Moodle task, or on a
file in the Notes tab.

**Claude's notes.** Claude keeps its own notes per subject in `~/.local/share/lockin/tutor-notes`, one
Markdown file per method, and reads them in later chats on that subject. Press `n` to read, edit or
delete them.

Chats are saved, each chat uses your Claude usage, and you can choose the model and language in Settings.

## Books from digi4school

Press `B` in the Chats tab, then `L` to log in with your digi4school account and see your books. Press
Enter on one to download it, and add it to a chat like any other file.

- Downloading goes through a patched copy of [digiDownload](https://github.com/digiDownload/digiDownload)
  (MIT licence), in `crates/digi_download_core`, updated for digi4school's current website.
- Your email is saved in your settings. After a login that works, LockIn asks whether to save your
  password: in your desktop keyring if you have one, otherwise in a file only you can read,
  `~/.config/lockin/digi4school-password`, which isn't encrypted. If you say no, it stays in memory until
  you quit. You can remove it in the Settings tab.
- If the site changes again, `lockin books-debug` describes its responses with no personal data.
- An interrupted download carries on from the last saved page.
- Books go in `~/.local/share/lockin/books`, which only you can read. They're licensed to your account,
  so they stay out of this repo. Only download books you have access to, and don't share them. Pages you
  ask about in a chat are sent to Claude.
- Check that downloading fits digi4school's terms of use for your account.

## Phone notifications

Install the ntfy app and subscribe to the topic that `lockin status` prints. The topic name is the
only thing protecting those messages on the public ntfy.sh server, so keep it private. Phone pushes
start off, and you can turn them on in the Settings tab.

## Developing

```
cargo test
cargo build --release && install -m755 target/release/lockin ~/.local/bin/lockin
```
