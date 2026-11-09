# Read-only bridge to the `moodle` CLI. It loads the script as a module and reuses
# its login, API client and HTML cleanup, then prints JSON. It only calls read
# functions: nothing here submits work or marks anything as read.
import contextlib
import importlib.machinery
import importlib.util
import json
import sys
import time

LOOKBACK_DAYS = 120
INTRO_LIMIT = 2000


def load(path):
    loader = importlib.machinery.SourceFileLoader("moodle_cli", path)
    spec = importlib.util.spec_from_loader("moodle_cli", loader)
    module = importlib.util.module_from_spec(spec)
    sys.modules["moodle_cli"] = module
    loader.exec_module(module)
    return module


def collect(path):
    m = load(path)
    sh = m.Shell()
    api = sh.need()
    text = lambda s: m.html2text(s or "", links=False).strip()

    now = int(time.time())
    window_from = now - LOOKBACK_DAYS * 86400
    courses = sh.courses()
    by_id = {c["id"]: c for c in courses}
    group = lambda c: sh.course_group(c)  # 0 current, 1 hidden, 2 past

    intros = {}
    try:
        for a in sh.load_assigns():
            # Calendar events point at an assignment by its course-module id, not its own id.
            intros[a.get("cmid")] = text(a.get("intro"))[:INTRO_LIMIT]
    except m.MoodleError:
        pass  # descriptions are a nice-to-have, deadlines still sync without them

    res = api.call("core_calendar_get_action_events_by_timesort",
                   timesortfrom=window_from, limitnum=50)
    tasks, action_ids = [], set()
    for e in res.get("events", []):
        action_ids.add(e.get("id"))
        course = e.get("course") or {}
        known = by_id.get(course.get("id"))
        if known is not None and group(known) == 2:
            continue  # leftovers from finished courses are not actionable
        action = e.get("action") or {}
        tasks.append({
            "ext_id": f"event:{e.get('id')}",
            "title": text(e.get("activityname") or e.get("name")),
            "course": course.get("shortname", ""),
            "due": e.get("timesort") or e.get("timestart"),
            "url": action.get("url") or e.get("url") or "",
            "action": action.get("name", "") + ("" if action.get("actionable", True) else " (closed on Moodle)"),
            "notes": intros.get(e.get("instance"), "") if e.get("modulename") == "assign" else text(e.get("description"))[:INTRO_LIMIT],
        })

    events = []
    for e in api.call("core_calendar_get_calendar_upcoming_view").get("events", []):
        if e.get("id") in action_ids or e.get("eventtype") == "due":
            continue  # deadlines live in the task list; finished ones can stay hidden
        start = e.get("timestart") or 0
        events.append({
            "ext_id": f"event:{e.get('id')}",
            "title": text(e.get("name")),
            "course": (e.get("course") or {}).get("shortname", ""),
            "start": start,
            "end": start + (e.get("timeduration") or 0),
            "location": text(e.get("location")),
            "notes": text(e.get("description"))[:INTRO_LIMIT],
        })

    grades = []
    for g in api.call("gradereport_overview_get_course_grades").get("grades", []):
        c = by_id.get(g.get("courseid"))
        if not c:
            continue
        current = group(c) == 0
        items = []
        if current:
            try:
                r = api.call("gradereport_user_get_grade_items", courseid=c["id"], userid=sh.uid)
                for it in (r.get("usergrades") or [{}])[0].get("gradeitems") or []:
                    name = text(it.get("itemname")) or ("Course total" if it.get("itemtype") == "course" else "Category total")
                    items.append({
                        "name": name,
                        "grade": text(it.get("gradeformatted")) or "-",
                        "range": text(it.get("rangeformatted")),
                        "pct": text(it.get("percentageformatted")),
                        "feedback": text(it.get("feedback"))[:500],
                        "total": it.get("itemtype") in ("course", "category"),
                    })
            except m.MoodleError:
                pass
        grades.append({
            "course_id": c["id"],
            "course": text(c.get("fullname")),
            "grade": text(g.get("grade")) or "-",
            "current": current,
            "items": items,
        })

    inbox = []
    for n in api.call("message_popup_get_popup_notifications", useridto=sh.uid, limit=30, newestfirst=1).get("notifications", []):
        subject = text(n.get("subject"))
        # Moodle's automatic deadline reminders only repeat the task list, even for submitted work.
        if not n.get("read") and not subject.lower().startswith(("due on ", "fällig am ")):
            inbox.append({
                "subject": subject,
                "message": text(n.get("smallmessage") or n.get("fullmessage"))[:300],
                "time": n.get("timecreated") or 0,
            })

    return {"window_from": window_from, "tasks": tasks, "events": events, "grades": grades, "inbox": inbox}


def main():
    out = sys.stdout
    try:
        with contextlib.redirect_stdout(sys.stderr):
            data = collect(sys.argv[1])
        json.dump({"ok": True, **data}, out)
    except BaseException as e:  # includes SystemExit from the CLI when logged out
        json.dump({"ok": False, "error": f"{type(e).__name__}: {e}"}, out)
        sys.exit(1)


main()
