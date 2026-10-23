"""Phase 2 live HTTP/process checks using only temporary Ubuntu Git fixtures."""
import json
import os
from pathlib import Path
import secrets
import tempfile
import time
from check_api import git, request, server


def commit(path, message):
    git(path, "-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "-m", message)


def git_bytes(path):
    return {str(file.relative_to(path)): file.read_bytes()
            for file in path.rglob("*") if file.is_file()}


def main():
    binary = Path(os.environ.get("CARGO_TARGET_DIR", Path(__file__).resolve().parent / "target")) / "debug/agent-workspace-backend"
    checks = []
    with tempfile.TemporaryDirectory(prefix="awm-phase2-http-") as fixture:
        root = Path(fixture)
        repo = root / "Проект ' $()\n"
        linked = root / "linked дерево\n"
        repo.mkdir()
        git(repo, "init", "-b", "main")
        for file in ["both\n", "old name", "delete", "binary"]:
            (repo / file).write_bytes(b"initial\0content")
        git(repo, "add", "--all")
        commit(repo, "initial")
        git(repo, "worktree", "add", "-b", "linked", str(linked))
        (repo / "both\n").write_text("staged")
        git(repo, "add", "--", "both\n")
        (repo / "both\n").write_text("unstaged")
        (repo / "old name").rename(repo / "new имя\n")
        git(repo, "add", "-A", "--", "old name", "new имя\n")
        (repo / "delete").unlink()
        (repo / "binary").write_bytes(b"changed\0binary")
        (repo / "untracked имя").write_text("fixture")
        (linked / "dirty").write_text("fixture")
        other = root / "other"
        other.mkdir()
        git(other, "init")
        bare = root / "bare.git"
        bare.mkdir()
        git(bare, "init", "--bare")
        config = root / "config.json"
        token = secrets.token_hex(32)
        with server(binary, config, token) as (port, _):
            def add(path):
                status, project = request(port, "POST", "/api/v1/projects", token, {"root_path": str(path)})
                assert status == 201
                return project
            project = add(repo)
            other_project = add(other)
            bare_project = add(bare)
            uri = f'/api/v1/projects/{project["id"]}/worktrees'
            assert request(port, "POST", uri + "/refresh")[0] == 401
            assert request(port, "POST", uri + "/refresh", token, origin="https://hostile.example")[0] == 403
            before = git_bytes(repo / ".git")
            status, snapshot = request(port, "POST", uri + "/refresh", token)
            assert status == 200 and snapshot["error"] is None
            assert len(snapshot["worktrees"]) == 2
            main_tree = next(tree for tree in snapshot["worktrees"] if tree["is_main"])
            linked_tree = next(tree for tree in snapshot["worktrees"] if tree["path"] == str(linked))
            status_uri = f'/api/v1/worktrees/{main_tree["id"]}/status'
            status, value = request(port, "GET", status_uri, token)
            assert status == 200 and value["error"] is None
            assert (value["staged"], value["unstaged"], value["untracked"], value["conflicts"]) == (2, 3, 1, 0)
            rename = next(file for file in value["files"] if file["kind"] == "renamed")
            assert rename["original_path"] == "old name" and rename["path"] == "new имя\n"
            assert git_bytes(repo / ".git") == before
            checks.extend(["worktree_auth_origin", "main_linked_snapshot", "status_counts_and_rename", "read_only_git_metadata"])
            git(repo, "worktree", "lock", "--reason", "reason\nпричина", str(linked))
            snapshot = request(port, "POST", uri + "/refresh", token)[1]
            locked = next(tree for tree in snapshot["worktrees"] if tree["id"] == linked_tree["id"])
            assert locked["is_locked"] and locked["lock_reason"] == "reason\nпричина"
            checks.append("locked_reason_and_stable_id")
            external = root / "external"
            git(repo, "worktree", "add", "--detach", str(external))
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                snapshot = request(port, "GET", uri, token)[1]
                if len(snapshot["worktrees"]) == 3:
                    break
                time.sleep(0.1)
            else:
                raise AssertionError("periodic refresh did not discover external add")
            external_tree = next(tree for tree in snapshot["worktrees"] if tree["path"] == str(external))
            assert external_tree["is_detached"]
            checks.append("real_periodic_external_add_detached")
            git(repo, "worktree", "remove", str(external))
            assert len(request(port, "POST", uri + "/refresh", token)[1]["worktrees"]) == 2
            assert request(port, "GET", f'/api/v1/worktrees/{external_tree["id"]}', token)[0] == 404
            checks.append("external_remove_old_id_404")
            bare_snapshot = request(port, "GET", f'/api/v1/projects/{bare_project["id"]}/worktrees', token)[1]
            bare_tree = bare_snapshot["worktrees"][0]
            assert bare_tree["is_bare"] and bare_tree["is_available"]
            assert bare_tree["git_status"]["staged"] is None
            assert bare_tree["git_status"]["error"]["code"] == "bare_repository"
            checks.append("bare_has_no_fake_clean_status")
        with server(binary, config, token) as (port, _):
            snapshot = request(port, "POST", uri + "/refresh", token)[1]
            assert {tree["id"] for tree in snapshot["worktrees"]} == {main_tree["id"], linked_tree["id"]}
            assert request(port, "GET", status_uri, token)[1]["staged"] == 2
            checks.append("real_restart_stable_worktree_ids")
            git(repo, "worktree", "unlock", str(linked))
            linked.rename(root / "moved-without-git")
            stale = request(port, "GET", f'/api/v1/worktrees/{linked_tree["id"]}/status', token)[1]
            assert stale["error"] is not None and stale["untracked"] == 1
            missing = request(port, "GET", f'/api/v1/worktrees/{linked_tree["id"]}', token)[1]
            assert not missing["is_available"] and missing["is_prunable"]
            checks.append("missing_prunable_status_marked_stale")
            other.rename(root / "other-moved")
            bad = request(port, "POST", f'/api/v1/projects/{other_project["id"]}/worktrees/refresh', token)[1]
            assert bad["error"] is not None
            assert request(port, "GET", status_uri, token)[1]["error"] is None
            assert request(port, "GET", "/api/v1/health")[1]["healthy"]
            checks.append("bad_project_does_not_break_healthy_repo_or_health")
            assert request(port, "GET", "/api/v1/worktrees/00000000-0000-0000-0000-000000000001/status", token)[0] == 404
            checks.append("unknown_worktree_404")
    print(json.dumps({"passed": len(checks), "checks": checks}, indent=2))


if __name__ == "__main__":
    main()
