#!/opt/homebrew/bin/python3.15
"""Refresh private token indexes from committed project sources."""

import argparse
from datetime import UTC, datetime
import fcntl
from hashlib import sha256
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
DATED = re.compile(r"[0-9]{8}T[0-9]{12}Z-[0-9a-f]{12}\.json")
TREE_ROW = re.compile(r"((?:│   |    )*)[├└]── ([A-Za-z0-9_.+-]+)(/?) \(([0-9,]+) tokens?\)")
SERENA_FILES = ("document_symbols.pkl", "raw_document_symbols.pkl")
SCHEMA = 3
MAX_RECEIPT_BYTES = 2 * 1024 * 1024


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


def read_receipt(path):
    with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK), "rb") as handle:
        info = os.fstat(handle.fileno())
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                or info.st_nlink != 1 or info.st_mode & 0o077):
            raise RuntimeError("Index receipt must be a private, owned regular file")
        if info.st_size > MAX_RECEIPT_BYTES:
            return None, None
        data = handle.read(MAX_RECEIPT_BYTES + 1)
    if len(data) > MAX_RECEIPT_BYTES:
        return None, None
    return data, decode_receipt(data)


def decode_receipt(data):
    def unique_fields(pairs):
        result = dict(pairs)
        if len(pairs) != len(result):
            raise ValueError("Duplicate receipt fields")
        return result

    try:
        receipt = json.loads(data, object_pairs_hook=unique_fields)
        if (not isinstance(receipt, dict) or type(receipt.get("schema")) is not int
                or receipt["schema"] not in (2, SCHEMA)):
            return None
        fields = {"schema", "commit", "generated_at", "tools", "source_files", "excluded_by_security",
                  "token_tree", "token_heatmap"}
        if receipt["schema"] == SCHEMA:
            fields.add("checkout_id")
            if not re.fullmatch(r"[0-9a-f]{64}", receipt.get("checkout_id", "")):
                return None
        if set(receipt) != fields or not SHA.fullmatch(receipt["commit"]):
            return None
        timestamp = datetime.fromisoformat(receipt["generated_at"])
        if timestamp.tzinfo is None or timestamp > datetime.now(UTC):
            return None
        tools = receipt["tools"]
        if (not isinstance(tools, dict) or set(tools) != {"repomix", "lean_ctx"}
                or any(not isinstance(v, str) or not 0 < len(v) <= 256 or not v.isprintable()
                       for v in tools.values())):
            return None
        count, excluded = receipt["source_files"], receipt["excluded_by_security"]
        if type(count) is not int or type(excluded) is not int or count <= 0 or excluded < 0 or count + excluded > 6000:
            return None
        lines = receipt["token_tree"].splitlines()
        if not lines or not re.fullmatch(r"─+", lines[0]):
            return None
        files, parents = set(), []
        for line in lines[1:]:
            match = TREE_ROW.fullmatch(line)
            if not match:
                return None
            indent, name, directory, _ = match.groups()
            depth = len(indent) // 4
            del parents[depth:]
            if depth != len(parents) or name.startswith(".") or name.casefold() in PRIVATE_PARTS:
                return None
            if directory:
                parents.append(name)
            else:
                relative = "/".join([*parents, name])
                if relative in files or not source_path(relative):
                    return None
                files.add(relative)
        heatmap = receipt["token_heatmap"]
        if (len(files) != count or not isinstance(heatmap, list) or len(heatmap) > min(count, 100)
                or any(not isinstance(row, dict) or set(row) != {"path", "token_count"}
                       or row["path"] not in files or type(row["token_count"]) is not int
                       or row["token_count"] < 0 for row in heatmap)
                or len({row["path"] for row in heatmap}) != len(heatmap)):
            return None
    except (AttributeError, OverflowError, RecursionError, TypeError, ValueError):
        return None
    return receipt


def dated_name(receipt):
    timestamp = datetime.fromisoformat(receipt["generated_at"]).astimezone(UTC)
    return f"{timestamp:%Y%m%dT%H%M%S%fZ}-{receipt['commit'][:12]}.json"


def retain_receipt(path, valid):
    folder = path.parent / ("history" if valid else "quarantine")
    private_directory(folder)
    target = Path(tempfile.mkdtemp(prefix="receipt-", dir=folder)) / path.name
    path.rename(target)
    print(f"{path.parent.name}: moved {path.name} to {folder.name}/")


def tidy_receipts(destination, expected):
    latest = destination / "latest.json"
    active_data = active_name = None
    if os.path.lexists(latest):
        data, receipt = read_receipt(latest)
        if receipt and receipt.get("checkout_id", expected["checkout_id"]) != expected["checkout_id"]:
            raise RuntimeError("Index directory belongs to another checkout with the same name")
        if receipt and all(receipt.get(k) == v for k, v in expected.items()):
            active_data, active_name = data, dated_name(receipt)
        else:
            retain_receipt(latest, valid=receipt is not None)
    for path in sorted(destination.iterdir()):
        if not DATED.fullmatch(path.name):
            continue
        data, receipt = read_receipt(path)
        valid = bool(receipt and dated_name(receipt) == path.name)
        if path.name == active_name and valid and data == active_data:
            continue
        retain_receipt(path, valid)
    return active_data


def serena_inventory(repo, destination, env, quarantine_empty=False):
    paths = (Path.home() / ".local/bin/serena", Path("/opt/homebrew/bin/serena"))
    executable = next((path for path in paths if path.is_file() and os.access(path, os.X_OK)), None)
    version = None
    if executable:
        try:
            reported = run([str(executable), "--version"], destination, env).decode().strip()
            if re.fullmatch(r"Serena [0-9][A-Za-z0-9.+_-]{0,63}", reported):
                version = reported
        except (OSError, RuntimeError, UnicodeError, subprocess.SubprocessError):
            pass

    cache = repo / ".serena/cache"
    entries = []
    status = "absent"
    for directory in (cache.parent, cache):
        try:
            if not stat.S_ISDIR(directory.lstat().st_mode):
                status = "unsafe"
                break
        except FileNotFoundError:
            break
    else:
        status = "present-unverified"
        for server in sorted(cache.iterdir()):
            if not re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}", server.name):
                continue
            if not stat.S_ISDIR(server.lstat().st_mode):
                status = "unsafe"
                continue
            for name in SERENA_FILES:
                file = server / name
                try:
                    info = file.lstat()
                except FileNotFoundError:
                    continue
                regular = stat.S_ISREG(info.st_mode)
                state = "unverified" if regular and info.st_size else "invalid"
                if (quarantine_empty and regular and info.st_size == 0 and info.st_uid == os.getuid()
                        and info.st_nlink == 1 and file.lstat() == info):
                    folder = destination / "serena-quarantine"
                    private_directory(folder)
                    saved = Path(tempfile.mkdtemp(prefix=f"{server.name}-", dir=folder)) / name
                    file.rename(saved)
                    state = "quarantined-empty"
                entries.append({"path": f"{server.name}/{name}", "bytes": info.st_size if regular else None,
                                "modified_at": datetime.fromtimestamp(info.st_mtime, UTC).isoformat(),
                                "status": state})
    data = {"version": version, "cache_status": status, "files": entries}
    publish(destination / "serena.json", (json.dumps(data, indent=2) + "\n").encode())


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


def refresh(repo, destination, *, quarantine_empty_serena=False):
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
        expected = {"schema": SCHEMA, "commit": revision, "tools": versions,
                    "checkout_id": sha256(os.fsencode(repo.resolve(strict=True))).hexdigest()}
        active = tidy_receipts(destination, expected)
        serena_inventory(repo, destination, env, quarantine_empty_serena)
        latest = destination / "latest.json"
        if active:
            dated = destination / dated_name(json.loads(active))
            if not dated.exists():
                publish(dated, active)
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
            **expected, "generated_at": now.isoformat(),
            "source_files": len(files), "token_tree": tree,
            "excluded_by_security": excluded,
            "token_heatmap": [{"path": row["path"], "token_count": row["token_count"]} for row in heatmap],
        }, ensure_ascii=False, indent=2) + "\n").encode()
        if len(data) > MAX_RECEIPT_BYTES or not decode_receipt(data):
            raise RuntimeError("Generated source index failed receipt verification")
        publish(destination / f"{now:%Y%m%dT%H%M%S%fZ}-{revision[:12]}.json", data)
        publish(latest, data)
        print(f"{repo.name}: indexed {revision[:12]} ({len(files)} files, {excluded} excluded by security scan)")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("repositories", type=Path, nargs="*")
    parser.add_argument("--quarantine-empty-serena-cache", action="store_true",
                        help="Move empty Serena symbol caches for explicit checkouts after closing their Serena sessions")
    args = parser.parse_args()
    if args.quarantine_empty_serena_cache and not args.repositories:
        parser.error("--quarantine-empty-serena-cache requires explicit checkouts")
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
            refresh(repo.resolve(strict=True), state / repo.name,
                    quarantine_empty_serena=args.quarantine_empty_serena_cache)
        except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as exc:
            print(f"{repo.name}: index failed ({type(exc).__name__})" +
                  (f": {exc}" if isinstance(exc, RuntimeError) else ""), flush=True)
            failures += 1
    return int(bool(failures))


if __name__ == "__main__":
    raise SystemExit(main())
