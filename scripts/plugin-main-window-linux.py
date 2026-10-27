#!/usr/bin/python3
"""Exercise the actual Linux main window under X11, using OS pointer events."""

import json
import os
import pathlib
import signal
import subprocess
import sys
import time

import pyatspi


binary, home, evidence = map(pathlib.Path, sys.argv[1:4])
evidence.mkdir(parents=True, exist_ok=True)
log = (evidence / "application.log").open("w")
report = {"platform": "linux", "success": False, "steps": []}
app = None
wm = None
plugin_pid = None


def run(*args):
    return subprocess.check_output(args, text=True, stderr=subprocess.STDOUT).strip()


def wait_for(description, fn, timeout=45):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            result = fn()
            if result:
                return result
        except (OSError, RuntimeError, subprocess.CalledProcessError):
            pass
        time.sleep(0.25)
    raise RuntimeError(f"Timed out waiting for {description}")


def nodes():
    queue = [pyatspi.Registry.getDesktop(0)]
    while queue:
        node = queue.pop()
        try:
            yield node
            queue.extend(list(node))
        except Exception:
            continue


def visible_element(name, role=None):
    for node in nodes():
        try:
            if node.name != name or (role and node.getRoleName() != role):
                continue
            if not node.getState().contains(pyatspi.STATE_SHOWING):
                continue
            rect = node.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
            if rect.width > 0 and rect.height > 0:
                return (rect.x + rect.width // 2, rect.y + rect.height // 2)
        except Exception:
            continue
    return None


def click(name):
    x, y = wait_for(name, lambda: visible_element(name, "push button"))
    run("xdotool", "mousemove", "--sync", str(x), str(y), "click", "1")


def screenshot(name):
    subprocess.run(["import", "-window", "root", str(evidence / f"{name}.png")], check=True)


def window_for(pid):
    output = run("wmctrl", "-lp")
    for line in output.splitlines():
        parts = line.split(None, 4)
        if len(parts) >= 3 and parts[2] == str(pid):
            return parts[0]
    return None


def mapped(window):
    return "Map State: IsViewable" in run("xwininfo", "-id", window)


def children(pid):
    result = []
    for entry in pathlib.Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            stat = (entry / "stat").read_text().rsplit(")", 1)[1].split()
            command = (entry / "cmdline").read_bytes().split(b"\0")
            if int(stat[1]) == pid and b"--cc-switch-plugin-runtime" in command:
                result.append(int(entry.name))
        except (OSError, ValueError, IndexError):
            continue
    return result


def plugin_alive(pid):
    return children(app.pid) == [pid] and bool(window_for(pid)) and mapped(window_for(pid))


def fixture_ready():
    path = home / ".cc-switch/ui-plugins/plugin-test.gui-acceptance.json"
    return json.loads(path.read_text()).get("storage") == {"ready": True}


def record(step, main_window):
    if app.poll() is not None or not plugin_alive(plugin_pid):
        raise RuntimeError(f"Plugin window or process was lost at {step}")
    report["steps"].append({"step": step, "mainPid": app.pid, "pluginPid": plugin_pid,
                            "mainMapped": mapped(main_window),
                            "pluginMapped": mapped(window_for(plugin_pid))})
    screenshot(step)


try:
    env = os.environ.copy()
    env.update({"CC_SWITCH_TEST_HOME": str(home), "GDK_BACKEND": "x11", "NO_AT_BRIDGE": "0",
                "GTK_A11Y": "always", "WEBKIT_DISABLE_COMPOSITING_MODE": "1"})
    wm = subprocess.Popen(["openbox"], stdout=log, stderr=log, env=env)
    time.sleep(1)
    app = subprocess.Popen([str(binary)], stdout=log, stderr=log, env=env)
    main_window = wait_for("main window", lambda: window_for(app.pid))
    wait_for("main window visibility", lambda: mapped(main_window))
    run("wmctrl", "-ia", main_window)
    click("Plugins")
    wait_for("fixture plugin", lambda: visible_element("GUI Acceptance"))
    click("Open")
    plugin_pid = wait_for("plugin child", lambda: next(iter(children(app.pid)), None))
    wait_for("plugin native window", lambda: plugin_alive(plugin_pid))
    wait_for("plugin SDK ready", fixture_ready)
    run("wmctrl", "-ia", main_window)
    wait_for("plugin controls", lambda: visible_element("Stop plugin", "push button"))
    record("plugin-open", main_window)

    run("wmctrl", "-ia", main_window)
    click("Skills")
    wait_for("Skills page", lambda: visible_element("More Skills actions", "combo box"))
    record("skills", main_window)
    click("Plugins")
    wait_for("restored plugin controls", lambda: visible_element("Stop plugin", "push button"))
    record("plugin-return", main_window)

    # _NET_CLOSE_WINDOW is handled by the window manager as a normal close request.
    run("wmctrl", "-ic", main_window)
    wait_for("main window hidden to tray", lambda: not mapped(main_window))
    time.sleep(3)
    record("main-hidden-to-tray", main_window)
    report["success"] = True
except Exception as error:
    report["error"] = str(error)
    try:
        screenshot("failure")
        (evidence / "accessibility.txt").write_text("\n".join(
            f"{n.getRoleName()}: {n.name}" for n in nodes() if n.name
        ))
        (evidence / "windows.txt").write_text(run("wmctrl", "-lp"))
    except Exception:
        pass
finally:
    if app and app.poll() is None:
        app.terminate()
        try:
            app.wait(timeout=10)
        except subprocess.TimeoutExpired:
            app.kill()
            app.wait()
    if plugin_pid:
        try:
            os.killpg(plugin_pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    if wm:
        wm.terminate()
        wm.wait(timeout=10)
    log.close()
    (evidence / "report.json").write_text(json.dumps(report, indent=2))

if not report["success"]:
    sys.exit(1)
