# Opens one assignment on the moodle CLI's own full-screen page, where `u` uploads files and
# `t` submits online text. Esc goes back to its assignment list and q returns to LockIn.
import importlib.machinery
import importlib.util
import sys


def load(path):
    loader = importlib.machinery.SourceFileLoader("moodle_cli", path)
    spec = importlib.util.spec_from_loader("moodle_cli", loader)
    module = importlib.util.module_from_spec(spec)
    sys.modules["moodle_cli"] = module
    loader.exec_module(module)
    return module


def find(m, sh, cmid, course_name):
    """Asking one course takes a fraction of a second, while asking them all takes several."""
    course = next((c for c in sh.courses() if c.get("shortname") == course_name), None)
    if course is not None:
        try:
            return sh.find_assign(cmid=cmid, course=course)
        except m.MoodleError:
            pass
    return sh.find_assign(cmid=cmid)


def main():
    m = load(sys.argv[1])
    sh = m.Shell()
    assignment = find(m, sh, int(sys.argv[2]), sys.argv[3])
    tab = next(i for i, (name, _) in enumerate(m.TUI.TABS) if name == "Assignments")

    class Opened(m.TUI):
        started = False

        def switch(self, i):
            if self.started:
                return super().switch(i)
            self.started = True
            self.tab = tab
            self.stacks[tab] = [m.AssignmentsView(self)]  # loaded only if you go back to it
            self.push(m.AssignmentView(self, assignment))

        def pop(self):
            super().pop()
            if not self.view.loaded:
                self.load(self.view)

    Opened(sh).run()


try:
    main()
except Exception as e:
    print(f"Could not open this assignment: {e}")
    input("[Enter] back to LockIn ")
    sys.exit(1)
