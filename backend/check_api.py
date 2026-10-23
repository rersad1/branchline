"""Real backend process/HTTP/restart check; only isolated temporary Linux fixtures."""
import contextlib
import json
import os
from pathlib import Path
import secrets
import signal
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request


def git(directory, *args):
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    subprocess.run(["git", "-C", str(directory), *args], env=env, check=True,
                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)


def request(port, method, path, token=None, body=None, origin=None):
    headers = {}
    if token:
        headers["Authorization"] = "Bearer " + token
    if origin:
        headers["Origin"] = origin
    payload = None
    if body is not None:
        payload = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(f"http://127.0.0.1:{port}{path}", data=payload,
                                 headers=headers, method=method)
    try:
        response = urllib.request.urlopen(req, timeout=3)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        data = response.read()
        return response.status, json.loads(data) if data else None


@contextlib.contextmanager
def server(binary, config, token):
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    env = dict(os.environ, AWM_PORT=str(port), AWM_CONFIG=str(config), AWM_TOKEN=token,
               GIT_DIR="/nonexistent/inherited-context", GIT_WORK_TREE="/nonexistent/tree")
    with tempfile.TemporaryFile() as log:
        process = subprocess.Popen([str(binary)], env=env, stdout=log, stderr=log)
        try:
            for _ in range(100):
                if process.poll() is not None:
                    raise AssertionError("backend exited before health became available")
                try:
                    if request(port, "GET", "/api/v1/health")[0] == 200:
                        break
                except (OSError, urllib.error.URLError):
                    time.sleep(0.05)
            else:
                raise AssertionError("backend did not become healthy")
            listeners = subprocess.check_output(["ss", "-ltnH", "sport", "=", f":{port}"], text=True)
            assert f"127.0.0.1:{port}" in listeners and f"0.0.0.0:{port}" not in listeners
            yield port, env
        finally:
            if process.poll() is None:
                process.send_signal(signal.SIGINT)
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
            assert process.returncode == 0, "backend did not shut down cleanly"


def main():
    binary = Path(os.environ.get("CARGO_TARGET_DIR", Path(__file__).resolve().parent / "target")) / "debug/agent-workspace-backend"
    checks = []
    with tempfile.TemporaryDirectory(prefix="awm-phase1-http-") as fixture:
        root = Path(fixture)
        repo = root / "Проект с пробелом"
        linked = root / "linked дерево"
        repo.mkdir()
        git(repo, "init", "-b", "main")
        git(repo, "-c", "user.name=Test", "-c", "user.email=test@example.invalid",
            "commit", "--allow-empty", "-m", "fixture")
        git(repo, "worktree", "add", "-b", "linked", str(linked))
        (repo / "dirty.txt").write_text("must survive unregister", encoding="utf-8")
        git_before = {str(path.relative_to(repo)): path.read_bytes()
                      for path in (repo / ".git").rglob("*") if path.is_file()}
        config = root / "config.json"
        token = secrets.token_hex(32)
        with server(binary, config, token) as (port, env):
            assert request(port, "GET", "/api/v1/health")[1]["healthy"] is True
            assert request(port, "POST", "/api/v1/projects", body={"root_path": str(repo)})[0] == 401
            assert request(port, "POST", "/api/v1/projects", token, {"root_path": str(repo)}, "https://hostile.example")[0] == 403
            status, project = request(port, "POST", "/api/v1/projects", token,
                                      {"root_path": str(repo), "name": "Проект"}, f"http://localhost:{port}")
            assert status == 201
            uri = "/api/v1/projects/" + project["id"]
            assert request(port, "GET", "/api/v1/projects", token) == (200, [project])
            assert request(port, "GET", uri, token) == (200, project)
            assert request(port, "POST", "/api/v1/projects", token, {"root_path": str(linked)})[0] == 409
            assert request(port, "POST", "/api/v1/projects", token, {"root_path": str(root / "missing")})[0] == 400
            assert token not in config.read_text()
            assert config.stat().st_mode & 0o777 == 0o600
            # A second OS process must not race the same JSON registry.
            duplicate = subprocess.run([str(binary)], env=dict(env, AWM_PORT="0"),
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=5)
            assert duplicate.returncode != 0
            checks.extend(["health_and_loopback", "authorization_and_origin", "add_list_get",
                           "linked_duplicate_409", "missing_path_400", "inherited_git_context_ignored",
                           "private_config_without_token", "second_process_lock"])
        with server(binary, config, token) as (port, _):
            assert request(port, "GET", uri, token) == (200, project)
            assert request(port, "DELETE", uri, token) == (204, None)
            assert request(port, "GET", uri, token)[0] == 404
            assert request(port, "GET", "/api/v1/projects", token) == (200, [])
            git_after = {str(path.relative_to(repo)): path.read_bytes()
                         for path in (repo / ".git").rglob("*") if path.is_file()}
            assert git_after == git_before
            assert (repo / "dirty.txt").read_text() == "must survive unregister"
            assert (linked / ".git").is_file()
            checks.extend(["real_process_restart_stable_project", "remove_preserves_all_git_metadata_and_files"])
        with server(binary, config, token) as (port, _):
            assert request(port, "GET", "/api/v1/projects", token) == (200, [])
            checks.append("removal_persists_after_restart")
        corrupt = b'{"version":99,"projects":[]}'
        config.write_bytes(corrupt)
        failed = subprocess.run([str(binary)], env=dict(os.environ, AWM_PORT="0", AWM_CONFIG=str(config), AWM_TOKEN=token),
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=5)
        assert failed.returncode != 0 and config.read_bytes() == corrupt
        checks.append("unsupported_config_fails_startup_without_overwrite")
    print(json.dumps({"passed": len(checks), "checks": checks}, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
