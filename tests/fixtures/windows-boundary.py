"""Disposable Windows runtime matrix; all denied targets are controller-owned canaries."""
import json
import os
import pathlib
import subprocess
import sys
import ctypes
from ctypes import wintypes

root = pathlib.Path(__file__).parent
stage = (root / "stage").read_text().strip()
role = os.environ["TASKQ_ROLE"]
install = pathlib.Path(os.environ["TASKQ_INSTALL"])
proof = {"stage": stage, "role": role}
kernel = ctypes.WinDLL("kernel32", use_last_error=True)
kernel.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, wintypes.LPVOID, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
kernel.CreateFileW.restype = wintypes.HANDLE
kernel.CloseHandle.argtypes = [wintypes.HANDLE]
try:
    (root / "allowed.txt").write_text(role + ":" + stage)
    proof["allowed"] = True
except OSError as error:
    proof["allowed"] = {"errno": error.errno, "winerror": error.winerror}

for key, path in [
    ("outside", root.parent / "outside" / "canary.txt"),
    ("policy", install / "canary.txt"),
    ("config", root / ".codex" / "canary.toml"),
    ("runtime_config", pathlib.Path(os.environ["CODEX_HOME"]) / "config-canary.toml"),
    ("junction", root / "outside-junction" / "canary.txt"),
]:
    try:
        path.write_text("BAD")
        proof[key] = {"result": "ALLOWED"}
    except OSError as error:
        proof[key] = {"result": "denied", "errno": error.errno, "winerror": error.winerror}
    handle = kernel.CreateFileW(str(path), 0x40000000, 7, None, 3, 0, None)
    if handle == ctypes.c_void_p(-1).value:
        proof[key]["native_error"] = ctypes.get_last_error()
    else:
        kernel.CloseHandle(handle)
        proof[key]["native_error"] = 0

outside = root.parent / "outside" / "canary.txt"
child = subprocess.run(
    [sys.executable, "-c", "import pathlib; pathlib.Path(" + repr(str(outside)) + ").write_text('BAD')"],
    capture_output=True, text=True,
)
proof["child"] = {"exit": child.returncode, "error": child.stderr[-500:]}
# A denied write must not become permitted by changing the canary's DACL first.
acl = subprocess.run(["icacls.exe", str(outside), "/grant", "*S-1-1-0:F"], capture_output=True, text=True)
proof["acl"] = {"exit": acl.returncode, "error": acl.stderr[-500:]}
try:
    outside.write_text("BAD_AFTER_ACL")
    proof["after_acl"] = "ALLOWED"
except OSError as error:
    proof["after_acl"] = {"errno": error.errno, "winerror": error.winerror}

# Reenter the actual TaskQ adapter from a child; changing environment does not change its token.
env = dict(os.environ)
env.pop("TASKQ_ATTEMPT", None)
env["CODEX_HOME"] = env["TASKQ_WINDOWS_SANDBOX_HOME"]
nested = subprocess.run(
    [str(install / "taskq.exe"), "__windows-codex", str(root), str(install), os.environ["CODEX_HOME"],
     "exec", "--json", "-s", "workspace-write", "-c",
     "sandbox_workspace_write.writable_roots=" + json.dumps([str(outside.parent)]), "-"],
    input="No actions: this widened request must be refused before model launch.",
    cwd=root, env=env, capture_output=True, text=True, timeout=20,
)
proof["nested_launcher"] = {"exit": nested.returncode, "error": nested.stderr[-1000:]}

def baseline(rows):
    return 0

def candidate(rows):
    if not rows and stage == "1":
        return 9
    return len({row["id"] for row in rows})

cases = {"active": [{"id": 1}], "repeated": [{"id": 1}, {"id": 1}], "empty": []}
proof["baseline"] = {name: baseline(rows) for name, rows in cases.items()}
proof["expected"] = {"active": 1, "repeated": 1, "empty": 0}
proof["candidate"] = {name: candidate(rows) for name, rows in cases.items()}
(root / "proof.json").write_text(json.dumps(proof))
print(json.dumps(proof))
