#!/opt/homebrew/bin/python3.15
"""Refresh private token indexes from committed project sources."""

import argparse
from datetime import UTC, datetime
import fcntl
import io
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import tempfile


GIT = "/usr/bin/git"
REPOMIX = "/opt/homebrew/bin/repomix"
LEAN_CTX = "/opt/homebrew/bin/lean-ctx"
SOURCE_SUFFIXES = {
    ".rs", ".swift", ".py", ".rb", ".c", ".h", ".cpp", ".hpp",
    ".go", ".kt", ".java", ".js", ".ts", ".tsx", ".ps1", ".sh", ".cs",
}
SOURCE_NAMES = {"AGENTS.md", "README.md", "Cargo.toml", "Package.swift", "pyproject.toml"}
PRIVATE_PARTS = {
    "artifacts", "assets", "backups", "cache", "data", "debug", "dist", "fixtures",
    "generated", "logs", "node_modules", "private", "receipts", "rulesets", "secrets",
    "snapshots", "target", "testdata", "vendor",
}
SAFE_PATH = re.compile(r"[A-Za-z0-9_./+-]+")
SHA = re.compile(r"[0-9a-f]{40}")
SCHEMA = 2


def run(args, cwd, env, *, input_bytes=None):
    result = subprocess.run(
        args, cwd=cwd, env=env, input=input_bytes, capture_output=True, timeout=180,
    )
    if result.returncode != 0:
        raise RuntimeError(f"{Path(args[0]).name} exited {result.returncode}")
    return result.stdout


def source_path(name):
    if not SAFE_PATH.fullmatch(name):
        return False
    parts = name.split("/")
    if any(not part or part.startswith(".") or part.casefold() in PRIVATE_PARTS for part in parts):
        return False
    return Path(name).suffix in SOURCE_SUFFIXES or parts[-1] in SOURCE_NAMES


def snapshot(repo, revision, stage, env):
    git = [GIT, "--no-replace-objects", "-C", str(repo)]
    listing = run(git + ["ls-tree", "-r", "-l", "-z", revision], stage, env)
    rows = []
    names = set()
    for entry in listing.rstrip(b"\0").split(b"\0"):
        if not entry:
            continue
        header, raw_name = entry.split(b"\t", 1)
        mode, kind, oid, size = header.split()
        name = raw_name.decode("utf-8", errors="replace")
        if mode not in {b"100644", b"100755"} or kind != b"blob" or not source_path(name):
            continue
        size = int(size)
        if size > 256 * 1024:
            continue
        if name.casefold() in names:
            raise RuntimeError("Source paths collide on a case-insensitive filesystem")
        names.add(name.casefold())
        rows.append((oid, size, name))
    if not rows or len(rows) > 6000 or sum(size for _, size, _ in rows) > 32 * 1024 * 1024:
        raise RuntimeError("Committed source selection is empty or exceeds its bound")

    objects = io.BytesIO(run(
        git + ["cat-file", "--batch"], stage, env,
        input_bytes=b"".join(oid + b"\n" for oid, _, _ in rows),
    ))
    selected = set()
    for oid, size, name in rows:
        if objects.readline().split() != [oid, b"blob", str(size).encode()]:
            raise RuntimeError("Unexpected Git object in source snapshot")
        content = objects.read(size)
        if len(content) != size or objects.read(1) != b"\n":
            raise RuntimeError("Truncated Git object in source snapshot")
        if b"\0" in content:
            continue
        try:
            content.decode("utf-8")
        except UnicodeDecodeError:
            continue
        path = stage / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
        selected.add(name)
    if objects.read(1) or not selected:
        raise RuntimeError("Invalid or empty source snapshot")
    return selected


def private_directory(path):
    path.mkdir(parents=True, mode=0o700, exist_ok=True)
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise RuntimeError("Index directory must be private and owned by the current user")


def publish(path, data):
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as output:
        temporary = Path(output.name)
        try:
            output.write(data)
            output.flush()
            os.fsync(output.fileno())
            output.close()
            os.replace(temporary, path)
        finally:
            temporary.unlink(missing_ok=True)


def token_tree(stage, work, files, env):
    config = work / "repomix.json"
    config.write_text(json.dumps({"security": {"enableSecurityCheck": True}}))
    output = work / "pack.json"
    excluded = 0
    for _ in range(2):
        report = run([
            REPOMIX, "--config", str(config), "--style", "json", "--no-files",
            "--no-default-patterns", "--no-gitignore", "--no-dot-ignore",
            "--token-count-tree", "--output", str(output),
        ], stage, env).decode()
        report = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", report)
        match = re.search(r"Security:\s+(\d+) suspicious file\(s\) detected and excluded", report)
        if not match:
            break
        section = report.split("Security Check:", 1)[-1].split("Pack Summary:", 1)[0]
        blocked = set(re.findall(r"^\s*\d+\. ([A-Za-z0-9_./+-]+)$", section, re.M))
        if excluded or len(blocked) != int(match[1]) or not blocked < files:
            raise RuntimeError("Repomix exclusions could not be verified")
        for name in blocked:
            (stage / name).unlink()
        files.difference_update(blocked)
        excluded = len(blocked)
    count = re.search(r"Total Files:\s+([\d,]+) files", report)
    if ("No suspicious files detected" not in report or "Token Count Tree:" not in report
            or not count or int(count[1].replace(",", "")) != len(files)):
        raise RuntimeError("Repomix did not confirm a clean security scan and token tree")
    if set(json.loads(output.read_text())) != {"fileSummary", "directoryStructure"}:
        raise RuntimeError("Repomix metadata output unexpectedly contains other sections")
    tree = report.split("Token Count Tree:", 1)[1].strip().split("\n\n", 1)[0]
    return tree, excluded


def refresh(repo, destination):
    private_directory(destination)
    lock = os.open(destination / ".lock", os.O_CREAT | os.O_NOFOLLOW | os.O_RDWR, 0o600)
    with os.fdopen(lock, "rb") as guard, tempfile.TemporaryDirectory(dir=destination) as work:
        fcntl.flock(guard, fcntl.LOCK_EX)
        work = Path(work)
        stage = work / "source"
        stage.mkdir()
        home = work / "home"
        home.mkdir()
        env = {
            "HOME": str(home), "PATH": "/opt/homebrew/bin:/usr/bin:/bin",
            "LANG": "en_US.UTF-8", "NO_COLOR": "1",
            "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_NO_LAZY_FETCH": "1", "GIT_TERMINAL_PROMPT": "0",
            "LEAN_CTX_PROJECT_ROOT": str(stage),
        }
        revision = run([GIT, "-C", str(repo), "rev-parse", "--verify", "HEAD^{commit}"], stage, env).decode().strip()
        if not SHA.fullmatch(revision):
            raise RuntimeError("Invalid committed source revision")
        versions = {
            "repomix": run([REPOMIX, "--version"], stage, env).decode().strip(),
            "lean_ctx": run([LEAN_CTX, "--version"], stage, env).decode().strip(),
        }
        latest = destination / "latest.json"
        if latest.is_symlink():
            raise RuntimeError("Unexpected symlink at latest.json")
        if latest.exists():
            try:
                prior = json.loads(latest.read_text())
            except (UnicodeError, ValueError):
                prior = {}
            if isinstance(prior, dict) and all(prior.get(k) == v for k, v in {
                "schema": SCHEMA, "commit": revision, "tools": versions,
            }.items()) and isinstance(prior.get("token_heatmap"), list) and prior.get("token_tree"):
                print(f"{repo.name}: current {revision[:12]}")
                return

        files = snapshot(repo, revision, stage, env)
        tree, excluded = token_tree(stage, work, files, env)
        run([GIT, "init", "--template=", "-q"], stage, env)
        run([GIT, "add", "--", "."], stage, env)
        heatmap = json.loads(run([LEAN_CTX, "heatmap", "--by=tokens", "--top=100", "--json"], stage, env))
        if not isinstance(heatmap, list) or any(
            not isinstance(row, dict) or row.get("path") not in files
            or type(row.get("token_count")) is not int or row["token_count"] < 0
            for row in heatmap
        ):
            raise RuntimeError("Invalid LeanCTX token heatmap")
        if run([GIT, "-C", str(repo), "rev-parse", "--verify", "HEAD^{commit}"], stage, env).decode().strip() != revision:
            raise RuntimeError("Project HEAD changed during indexing")
        now = datetime.now(UTC)
        data = (json.dumps({
            "schema": SCHEMA, "commit": revision, "generated_at": now.isoformat(),
            "tools": versions, "source_files": len(files), "token_tree": tree,
            "excluded_by_security": excluded,
            "token_heatmap": [{"path": row["path"], "token_count": row["token_count"]} for row in heatmap],
        }, ensure_ascii=False, indent=2) + "\n").encode()
        publish(destination / f"{now:%Y%m%dT%H%M%S%fZ}-{revision[:12]}.json", data)
        publish(latest, data)
        print(f"{repo.name}: indexed {revision[:12]} ({len(files)} files, {excluded} excluded by security scan)")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("repositories", type=Path, nargs="*")
    args = parser.parse_args()
    os.umask(0o077)
    state = Path(os.environ.get("XDG_STATE_HOME") or Path.home() / ".local/state") / "project-indexes"
    private_directory(state)
    root = Path.home() / "Downloads/GitHub"
    repositories = args.repositories or sorted(path for path in root.iterdir() if (path / ".git").exists())
    failures = 0
    for repo in repositories:
        try:
            if repo.is_symlink() or not re.fullmatch(r"[A-Za-z0-9_.-]+", repo.name) or repo.name.startswith("."):
                raise RuntimeError("Invalid checkout name")
            refresh(repo.resolve(strict=True), state / repo.name)
        except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as exc:
            print(f"{repo.name}: index failed ({type(exc).__name__})" +
                  (f": {exc}" if isinstance(exc, RuntimeError) else ""), flush=True)
            failures += 1
    return int(bool(failures))


if __name__ == "__main__":
    raise SystemExit(main())
