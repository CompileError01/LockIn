# Downloads course files or one assignment's attachments through the moodle CLI, then prints JSON.
# It only downloads: it never submits work or changes anything on Moodle.
import contextlib
import importlib.machinery
import importlib.util
import json
import sys


def load(path):
    loader = importlib.machinery.SourceFileLoader("moodle_cli", path)
    spec = importlib.util.spec_from_loader("moodle_cli", loader)
    module = importlib.util.module_from_spec(spec)
    sys.modules["moodle_cli"] = module
    loader.exec_module(module)
    return module


def quiet(*_args, **_kwargs):
    pass


def collect(path, mode, shortname, cmid):
    m = load(path)
    sh = m.Shell()
    sh.need()
    course = next((c for c in sh.courses() if c.get("shortname") == shortname), None)
    if mode == "course":
        if course is None:
            raise m.MoodleError(f"course not found: {shortname}")
        got, skipped = sh.sync_course(course, log=quiet, show=False)
        return {"got": got, "skipped": skipped}
    try:
        assignment = sh.find_assign(cmid=int(cmid), course=course) if course else sh.find_assign(cmid=int(cmid))
    except m.MoodleError:
        assignment = sh.find_assign(cmid=int(cmid))
    got, base = sh.get_assignment_files(assignment, log=quiet, show=False)
    return {"got": got, "folder": str(base), "description": m.html2text(assignment.get("intro") or "", links=False).strip()}


def main():
    out = sys.stdout
    try:
        with contextlib.redirect_stdout(sys.stderr):
            data = collect(sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4] if len(sys.argv) > 4 else "0")
        json.dump({"ok": True, **data}, out)
    except BaseException as e:
        json.dump({"ok": False, "error": f"{type(e).__name__}: {e}"}, out)
        sys.exit(1)


main()
