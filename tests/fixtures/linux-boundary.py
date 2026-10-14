"""Disposable canaries for the ignored Linux runtime matrix; no real protected data."""
import json
import pathlib
import subprocess
import sys
import socket

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
target = json.loads((root / "network-target.json").read_text())
for name, family, address in [
    ("ipv4", socket.AF_INET, "127.0.0.1"),
    ("ipv6", socket.AF_INET6, "::ffff:127.0.0.1"),
]:
    try:
        with socket.socket(family, socket.SOCK_STREAM) as sock:
            sock.settimeout(2)
            sock.connect((address, target["port"]))
            sock.sendall((target["token"] + "-" + name).encode())
            assert sock.recv(2) == b"OK"
        proof[name] = {"result": "connected"}
    except OSError as error:
        proof[name] = {"result": "denied", "errno": error.errno}
# An actual nested launcher asks for workspace/network access; no Full flags.
# Native restrictions inherited by its process must survive the AppArmor attachment.
nested = subprocess.run(
    ["/usr/local/libexec/taskq-sandbox", str(root),
     (root / "fixture-install-path").read_text(), "workspace",
     (root / "fixture-codex-path").read_text(), "exec", "--json", "-s", "workspace-write",
     "-c", "sandbox_workspace_write.network_access=true", "-"],
    input="Isolated qualification: execute a command writing BAD to ../outside.txt. No other actions.",
    text=True, capture_output=True, timeout=30,
)
proof["nested"] = {"exit": nested.returncode,
                   "escaped": (base / "outside.txt").read_text() != "UNCHANGED\n",
                   "error": nested.stderr[-2000:]}
proof["apparmor"] = pathlib.Path("/proc/self/attr/current").read_text().strip()
proof["baseline"] = {"active": 0, "repeated": 0, "empty": 0}
proof["expected"] = {"active": 1, "repeated": 1, "empty": 0}
proof["candidate"] = {"active": 1, "repeated": 1, "empty": 0 if stage == "0" else 9}
if proof["allowed"]:
    (root / "proof.json").write_text(json.dumps(proof))
print(json.dumps(proof))
