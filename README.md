# LockIn

A Neovim-style terminal organizer with tasks, a week planner, a calendar, Obsidian notes, habits and a
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
lockin sync       pull deadlines and events from Moodle, and your timetable and tests from Untis
lockin brief      ask Claude for a new plan and print it
lockin plan       plan study sessions for your upcoming tests
lockin remind --preview   show which reminders would be sent right now
lockin today      print open tasks
lockin add "Read chapter 4 @fri 18:00 !! #AM ~45m"
lockin pdf FILE   read a PDF in the terminal
lockin teach FILE "question"   ask a teaching chat about a file
lockin notify     send a test notification
lockin phone      connect your phone, and check that texting works
lockin replies on|off   text LockIn from your phone
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

## Reminders

Between briefings, `lockin-remind.timer` checks every 10 minutes and sends, once each:

- **Deadline warnings** 24, 6 and 2 hours before something is due. If several have passed at once, like for
  a task added late, you only get the closest one. A Moodle deadline is checked against a fresh sync first,
  so work you've handed in doesn't get a warning.
- **Study sessions** from the Week tab, 10 minutes before they start.
- **Tests tomorrow**, from 17:00 the evening before.
- **A nudge to study** at 17:00, if you've done less than 20 minutes in the focus timer today, nothing is
  planned for later, and something is due within a week.

Nothing is sent during quiet hours, 22:00 to 07:00. Deadline warnings wait until they're over. On the phone,
reminders have buttons: **Done ✓** ticks the task off and **What's due** lists open tasks (both need phone
replies on), and tapping a Moodle reminder opens the assignment. All of this is in the Settings tab under
Reminders. It uses no Claude.

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

## Week planner

The Week tab shows your timetable from Untis, with changes and cancellations marked, your tests, your own
events, and deadlines. It needs the `untis` command, logged in, in `~/.local/bin`. Without it, the tab still
shows your own things. Press `[` and `]` to change week.

Press `p`, or run `lockin plan`, to plan study sessions for tests in the next four weeks. Code works out
your free time first, so a session can't land on a lesson or an event: after school until 20:00 by default,
and 10:00 to 18:00 on days without lessons. Claude then picks among those slots and says what to focus on,
and everything it picks is checked. A plain spacing plan fills any gaps, or runs alone without Claude.

A Schularbeit gets 5 sessions, a test gets 3, and a test of 30 minutes or less gets 2. Sessions show up on the
calendar too. Sessions you edit or move are yours, and planning again keeps them. Press `u` to undo a plan.
The daily job plans each new test once, so a session you delete stays deleted. The hours, the session
length and the daily limit are in the Settings tab.

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

### Texting LockIn back

Run `lockin replies on`, or use the Settings tab. The first time, you get a new private topic. Then run
`lockin phone`:

1. Scan the QR code with your phone's camera. It opens the ntfy app straight to the topic, so there's
   nothing to type. Tap Subscribe.
2. In the app, open that subscription (its name ends in the characters `lockin phone` shows) and send `hi`.
3. `lockin phone` waits for it and tells you when your phone is connected. If nothing arrives, it says what
   to check.

If you have older LockIn subscriptions in the app, delete them. A message sent to an old topic only gets a
reply saying LockIn moved. `lockin status` shows when the last text from your phone arrived.

Then write to LockIn in that subscription, or tap the buttons on its notifications:

```
remind me to call mum tomorrow at 5     Claude works out what you mean
what's due this week?                   Claude answers from your tasks and calendar
hi                                      checks the connection, with no Claude
add Read chapter 4 @fri 18:00           adds a task, with no Claude
event Shopping @tom 17:00               adds to your calendar and week, with no Claude
list                                    open tasks, with their numbers
done 12                                 ticks off task 12
sync                                    updates from Moodle
help                                    this list
```

A background service does this, so the app can be closed, but your computer has to be on and you have to
be logged in. Messages sent while it's off are picked up later, for about 12 hours.

**The topic name works like a password.** Anyone who has it can read your replies and send commands, so
keep it private. To limit the damage if it leaks, LockIn can only add tasks and events, tick off tasks and
sync from here. It can't delete anything, submit to Moodle, read your files or run commands, and it answers
at most 20 messages an hour. Messages and replies pass through the ntfy server. If you think the name has
leaked, change the topic in Settings. Turn the feature off with `lockin replies off`.

## Developing

```
cargo test
cargo build --release && install -m755 target/release/lockin ~/.local/bin/lockin
```
