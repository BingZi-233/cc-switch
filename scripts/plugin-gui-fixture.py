#!/usr/bin/env python3
"""Create an isolated, enabled plugin for native main-window acceptance."""
import json
import pathlib
import sys

home = pathlib.Path(sys.argv[1]).resolve()
config = home / ".cc-switch"
plugins = config / "ui-plugins"
plugins.mkdir(parents=True, exist_ok=False)
(config / "settings.json").write_text(json.dumps({
    "language": "en",
    "showInTray": True,
    "minimizeToTrayOnClose": True,
    "firstRunNoticeConfirmed": True,
    "newLayoutNoticeConfirmed": True,
    "whatsNewSeenVersion": "4.0.20",
}), encoding="utf-8")
record = {
    "manifest": {
        "apiVersion": 1,
        "id": "test.gui-acceptance",
        "name": "GUI Acceptance",
        "version": "1.0.0",
        "description": "Native main-window acceptance fixture",
        "pages": [{"id": "main", "title": "Acceptance", "entry": "index.html"}],
        "permissions": [],
    },
    "enabled": True,
    "documents": {"main": "<!doctype html><meta charset='utf-8'><h1>GUI Acceptance</h1>"},
    "storage": None,
    "granted_permissions": [],
    "access_revision": "gui-acceptance",
}
(plugins / "plugin-test.gui-acceptance.json").write_text(json.dumps(record), encoding="utf-8")
