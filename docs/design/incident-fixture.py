"""Isolated M-A15 fixtures; never starts a service or accesses a board.

python docs/design/incident-fixture.py NEW_DIRECTORY
For each state: TASKQ_INSTALL=NEW_DIRECTORY/STATE/install taskq tray --snapshot NEW_DIRECTORY/STATE/shots
"""
import json
import os
from pathlib import Path
import sys
import time


def fixtures(destination):
    destination.mkdir(parents=True, exist_ok=False)
    now = int(time.time())
    for state in ("sent", "local", "unsupported-auth", "disabled", "none", "notice", "stale", "missing"):
        directory = destination / state
        root = directory / "project"
        dot = root / ".taskq"
        dot.mkdir(parents=True)
        install = directory / "install"
        install.mkdir()
        (install / "projects").write_text(str(root.resolve()) + "\n", encoding="utf-8")
        config = {"board": "github", "repo": "fixture/example", "limits": {"claude": 0, "codex": 0}}
        if state != "notice":
            config["incident_reports"] = state != "disabled"
        (root / "taskq.json").write_text(json.dumps(config), encoding="utf-8")
        incident = None if state in ("none", "notice") else {
            "state": "local" if state == "stale" else state,
            "fault": "dispatch.panic", "at": now - 300,
            "path": ".taskq/incidents/0123456789abcdef.json",
            "url": "https://github.com/taskqorg/taskq/issues/1" if state == "sent" else None,
            "reason": "no github.com login" if state == "unsupported-auth" else "offline" if state in ("local", "stale") else None,
        }
        status = {"project": "alpha", "host": "fixture", "version": "4.0.0-dev",
                  "focus": ["all"], "tasks": [], "error": None, "incident": incident,
                  "incident_notice": state == "notice", "capacity": {
                      "limits": {"claude": 0, "codex": 0}, "used": {"claude": 0, "codex": 0},
                      "load_gate": {"enabled": False, "cpu_max": 85, "memory_min": 15},
                      "gate": "off", "reasons": [], "sample_age_seconds": 0}}
        if incident:
            (dot / "incident.json").write_text(json.dumps(incident), encoding="utf-8")
        if state != "missing":
            (dot / "status.json").write_text(json.dumps(status), encoding="utf-8")
        if state == "stale":
            os.utime(dot / "status.json", (now - 600, now - 600))
        (dot / "incidents").mkdir()
        report = {"schema": "taskq-incident/1", "fault": "dispatch.panic", "fingerprint": "0123456789abcdef",
                  "report_id": "11111111-1111-4111-8111-111111111111", "release": "4.0.0-dev",
                  "revision": "0" * 40, "contract": "0" * 12, "board": "github", "runtime": "none",
                  "os": "windows-x86_64", "site": "src/main.rs:1", "evidence": ["exit:101"],
                  "observed": ["exit:101"], "inferred": ["defect"]}
        (dot / "incidents/0123456789abcdef.json").write_text(json.dumps(report), encoding="utf-8")
    assert len(list(destination.glob("*/install/projects"))) == 8
    print(destination.resolve())


if __name__ == "__main__":
    fixtures(Path(sys.argv[1]).resolve())
