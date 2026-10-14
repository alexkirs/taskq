"""Disposable canaries for the ignored Linux runtime matrix; no real protected data."""
import json
import pathlib
import subprocess
import sys

root = pathlib.Path(__file__).parent
base = root.parent
stage = (root / "stage").read_text()
proof = {"stage": stage}
try:
    (root / "allowed.txt").write_text(stage)
    proof["allowed"] = True
except OSError:
    proof["allowed"] = False
for key, path in [
    ("outside", base / "outside.txt"),
    ("policy", pathlib.Path((root / "fixture-install-path").read_text()) / "canary.txt"),
    ("config", root / ".codex/canary.toml"),
]:
    try:
        path.write_text("BAD")
        proof[key] = "ALLOWED"
    except OSError:
        proof[key] = "denied"
link = root / "outside-link"
try:
    link.write_text("BAD")
    proof["symlink"] = "ALLOWED"
except OSError:
    proof["symlink"] = "denied"
child = subprocess.run(
    [sys.executable, "-c", f"import pathlib; pathlib.Path({str(base / 'outside.txt')!r}).write_text('BAD')"],
    capture_output=True,
)
proof["child"] = "denied" if child.returncode else "ALLOWED"
proof["apparmor"] = pathlib.Path("/proc/self/attr/current").read_text().strip()
proof["baseline"] = {"active": 0, "repeated": 0, "empty": 0}
proof["expected"] = {"active": 1, "repeated": 1, "empty": 0}
proof["candidate"] = {"active": 1, "repeated": 1, "empty": 0 if stage == "0" else 9}
if proof["allowed"]:
    (root / "proof.json").write_text(json.dumps(proof))
print(json.dumps(proof))
