# The only part of LockIn that changes anything on Moodle. It runs in two modes, through the moodle CLI's
# login, and prints JSON:
#
#   info   COURSE CMID              downloads the assignment's attachments and describes what it accepts
#   submit COURSE CMID DIR [--accept-statement]
#                                   uploads a draft you approved in LockIn, then submits it for grading
#
# `submit` only runs after you press S on a draft in the app and confirm. DIR holds `text.md` for online
# text and a `files/` folder for uploads.
import contextlib
import importlib.machinery
import importlib.util
import json
import sys
from pathlib import Path


def load(path):
    loader = importlib.machinery.SourceFileLoader("moodle_cli", path)
    spec = importlib.util.spec_from_loader("moodle_cli", loader)
    module = importlib.util.module_from_spec(spec)
    sys.modules["moodle_cli"] = module
    loader.exec_module(module)
    return module


def find(m, sh, shortname, cmid):
    course = next((c for c in sh.courses() if c.get("shortname") == shortname), None)
    if course is not None:
        try:
            return sh.find_assign(cmid=cmid, course=course)
        except m.MoodleError:
            pass
    return sh.find_assign(cmid=cmid)


def config(x, plugin, name):
    for c in x.get("configs") or []:
        if c.get("plugin") == plugin and c.get("subtype") == "assignsubmission" and c.get("name") == name:
            return c.get("value")
    return None


def state(m, sh, x):
    st = sh.submission_status(x)
    last = st.get("lastattempt") or {}
    sub = last.get("submission") or last.get("teamsubmission") or {}
    enabled = sorted({p.get("type") for p in sub.get("plugins") or []} & {"onlinetext", "file"})
    files = [f.get("filename") for p in sub.get("plugins") or [] if p.get("type") == "file"
             for fa in p.get("fileareas") or [] for f in fa.get("files") or []]
    return {
        "status": sub.get("status", "new"),
        "accepts": enabled,
        "locked": bool(last.get("locked")) or not last.get("caneditowner", True),
        "current_files": files,
    }


def info(m, sh, x):
    got, base = sh.get_assignment_files(x, log=lambda *a, **k: None, show=False)
    statement = ""
    if x.get("requiresubmissionstatement"):
        statement = m.html2text(x.get("submissionstatement") or "", links=False).strip()
    return {
        "name": m.html2text(x.get("name") or "", links=False).strip(),
        "description": m.html2text(x.get("intro") or "", links=False).strip(),
        "folder": str(base),
        "attachments": [str(base / m.safe_name(f["filename"], 180)) for f in x.get("introattachments") or []],
        "max_files": int(config(x, "file", "maxfilesubmissions") or 0),
        "file_types": config(x, "file", "filetypeslist") or "",
        "word_limit": int(config(x, "onlinetext", "wordlimit") or 0),
        "statement": statement,
        "needs_final_submit": bool(x.get("submissiondrafts")),
        **state(m, sh, x),
    }


def submit(m, sh, x, folder, accept_statement):
    before = state(m, sh, x)
    if before["locked"]:
        raise m.MoodleError("submissions are locked or closed for you")
    folder = Path(folder)
    text_file = folder / "text.md"
    text = text_file.read_text() if text_file.is_file() else None
    paths = sorted(p for p in (folder / "files").glob("*") if p.is_file()) if (folder / "files").is_dir() else []
    if text is not None and "onlinetext" not in before["accepts"]:
        raise m.MoodleError("this assignment doesn't take online text")
    if paths and "file" not in before["accepts"]:
        raise m.MoodleError("this assignment doesn't take file uploads")
    if text is None and not paths:
        raise m.MoodleError("the draft is empty")
    statement = x.get("requiresubmissionstatement") and (x.get("submissionstatement") or "").strip()
    if x.get("submissiondrafts") and statement and not accept_statement:
        raise m.MoodleError("the submission statement wasn't accepted")

    api = sh.need()
    plugindata = {}
    if paths:
        plugindata["files_filemanager"] = api.upload(paths)
    if text is not None:
        try:
            item = api.call("core_files_get_unused_draft_itemid").get("itemid", 0)
        except m.MoodleError:
            item = 0
        plugindata["onlinetext_editor"] = {"text": m.text_to_html(text), "format": 1, "itemid": item}
    warnings = api.call("mod_assign_save_submission", assignmentid=x["id"], plugindata=plugindata)
    if warnings:
        raise m.MoodleError("; ".join(str(w.get("message") or w) for w in warnings))
    if x.get("submissiondrafts"):
        warnings = api.call("mod_assign_submit_for_grading", assignmentid=x["id"],
                            acceptsubmissionstatement=1 if statement else 0)
        if warnings:
            raise m.MoodleError("saved as a draft, but submitting failed: "
                                + "; ".join(str(w.get("message") or w) for w in warnings))
    # Moodle's own answer is what counts, not ours.
    after = state(m, sh, x)
    return {"status": after["status"], "files": after["current_files"]}


def main():
    out = sys.stdout
    try:
        with contextlib.redirect_stdout(sys.stderr):
            moodle, mode, shortname, cmid = sys.argv[1:5]
            m = load(moodle)
            sh = m.Shell()
            sh.need()
            x = find(m, sh, shortname, int(cmid))
            if mode == "info":
                data = info(m, sh, x)
            elif mode == "submit":
                data = submit(m, sh, x, sys.argv[5], "--accept-statement" in sys.argv[6:])
            else:
                raise ValueError(f"unknown mode {mode}")
        json.dump({"ok": True, **data}, out)
    except BaseException as e:
        json.dump({"ok": False, "error": f"{type(e).__name__}: {e}"}, out)
        sys.exit(1)


main()
