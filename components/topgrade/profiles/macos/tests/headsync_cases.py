#!/usr/bin/env python3.15
"""Exercise a HEAD-sync script using only synthetic formulae and fake commands."""

import fcntl
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time

native = len(sys.argv) >= 3 and sys.argv[1] == "--native"
if native:
    command = [str(Path(sys.argv[2]).resolve(strict=True)), *sys.argv[3:]]
elif len(sys.argv) == 2:
    command = ["/bin/zsh", str(Path(sys.argv[1]).resolve(strict=True))]
else:
    raise SystemExit("usage: headsync_cases.py HEADSYNC_SCRIPT | --native EXECUTABLE [ARGS...]")
sha = "0123456789abcdef0123456789abcdef01234567"
ggml = '''class Ggml
  head "https://github.com/ggml-org/ggml.git", branch: "master"
  def install
    system "cmake", "-S", ".", "-B", "build", *args, *std_cmake_args
  end
end
'''
llama = '''class LlamaCpp
  head "https://github.com/ggml-org/llama.cpp.git", branch: "master"
end
'''

brew = r'''#!/usr/bin/python3
import os, pathlib, sys
args=sys.argv[1:]
root=pathlib.Path(os.environ["TEST_ROOT"])
with (root/"events").open("a") as f: f.write("brew " + " ".join(args) + "\n")
if args == ["--repo", "homebrew/core"]:
 print(root/"core")
elif args == ["tap"]:
 print("homebrew/core\nexample/tools")
 if os.environ.get("TEST_CATALOG_REFRESH") == "1":
  print("example/clean\nexample/local")
elif args[:1] == ["--repo"] and args[1] in ("example/tools", "example/clean", "example/local"):
 print(root/{"example/tools":"tap", "example/clean":"clean-tap", "example/local":"local-tap"}[args[1]])
elif args == ["--prefix"]:
 print(root)
elif args == ["--cellar", "llvm"]:
 print(root/"Cellar/llvm")
elif args == ["update"]:
 if os.environ.get("HOMEBREW_NO_AUTO_UPDATE") != "1": print("auto-update guard missing",file=sys.stderr); sys.exit(30)
 if (root/"state/brew-headsync/active").exists(): print("journal was not recovered",file=sys.stderr); sys.exit(38)
 if os.environ.get("TEST_FAIL") == "update": print("injected brew update failure",file=sys.stderr); sys.exit(39)
 if os.environ.get("TEST_SWITCH_LLVM_PIN_AFTER_UPDATE") == "1":
  keg = root/"Cellar/llvm/23.1.4"; keg.mkdir(); pin = root/"var/homebrew/pinned/llvm"; pin.unlink(); pin.symlink_to(keg)
elif args == ["list", "--formula", "--pinned"]:
 if os.environ.get("TEST_FAIL_LIST") == "pinned": print("simulated pinned list error",file=sys.stderr); sys.exit(36)
 print(os.environ.get("TEST_PINNED", "llvm"))
elif args == ["list", "--formula", "--full-name"]:
 if os.environ.get("TEST_FAIL_LIST") == "installed": print("simulated installed list error",file=sys.stderr); sys.exit(37)
 print(os.environ.get("TEST_INSTALLED", "llvm\ntopgrade\nz3\natuin"))
elif args[:1] == ["upgrade"]:
 if os.environ.get("HOMEBREW_NO_INSTALL_CLEANUP") != "1":
  print("missing cleanup protection", file=sys.stderr); sys.exit(31)
 if os.environ.get("HOMEBREW_NO_AUTO_UPDATE") != "1": print("auto-update guard missing",file=sys.stderr); sys.exit(30)
 if "topgrade" in args: print("topgrade must not reach brew upgrade", file=sys.stderr); sys.exit(32)
 target = args[-1] if args[-1] in ("ggml", "llama.cpp") else "bulk"
 if target != "bulk" or "--dry-run" in args or not os.environ.get("TEST_HEADSYNC_NATIVE_CHILD"):
  for f in (root/"core/Formula/g/ggml.rb",root/"core/Formula/l/llama.cpp.rb"):
   if os.environ["TEST_SHA"] not in f.read_text(): print("formula revision mismatch",file=sys.stderr); sys.exit(33)
 if "--dry-run" in args:
  if os.environ.get("TEST_FAIL_PREFLIGHT") == "1":
   print("Pinned LLVM cannot satisfy llama.cpp",file=sys.stderr); sys.exit(44)
  sys.exit(0)
 if os.environ.get("TEST_PAUSE_UPGRADE") == target:
  (root/"upgrade-paused").touch()
  for attempt in range(750):
   if (root/"upgrade-release").exists(): break
   __import__("time").sleep(.02)
 if os.environ.get("TEST_FAIL") == target: print("injected upgrade failure",file=sys.stderr); sys.exit(34)
 if target == "llama.cpp": (root/"pair-installed").touch()
 if target == "bulk" and os.environ.get("TEST_HEADSYNC_NATIVE_CHILD"):
  still_outdated = set(os.environ.get("TEST_BULK_STILL_OUTDATED", "").splitlines())
  upgraded = [arg.rsplit("/", 1)[-1] for arg in args[1:] if not arg.startswith("--")]
  (root/"bulk-upgraded").write_text("\n".join(name for name in upgraded if name not in still_outdated))
elif args[:1] == ["outdated"]:
 eligible={name.rsplit("/",1)[-1]:name for name in args[1:] if not name.startswith("--")}
 outdated=set(os.environ.get("TEST_OUTDATED", "topgrade").splitlines())
 if (root/"core-catalog-refreshed").exists(): outdated.add("postgresql@17")
 if (root/"jpeg-xl-committed").exists():
  outdated.difference_update(os.environ.get("TEST_JXL_REBUILDS", "").splitlines())
  if os.environ.get("TEST_JXL_STILL_OUTDATED") != "1": outdated.discard("jpeg-xl")
 upgraded = root/"bulk-upgraded"
 if upgraded.exists(): outdated.difference_update(upgraded.read_text().splitlines())
 found=[name for short,name in eligible.items() if short in outdated]
 mode=os.environ.get("TEST_OUTDATED_MODE", "normal")
 if mode == "malformed": print("{bad json"); sys.exit(1)
 if mode == "one-empty": print('{"formulae": [], "casks": []}'); sys.exit(1)
 if mode == "operational": print("simulated outdated transport failure",file=sys.stderr); sys.exit(2)
 if mode == "no-json": print("Error: synthetic formula syntax error",file=sys.stderr); sys.exit(1)
 if "--json=v2" in args:
  entries=[{"name":name,"pinned":mode == "newly-pinned"} for name in found]
  if mode == "unselected": entries=[{"name":"not-a-candidate","pinned":False}]
  if mode == "duplicate" and entries: entries.append(dict(entries[0]))
  print(__import__("json").dumps({"formulae":entries,"casks":[]}))
 else:
  print("\n".join(found))
 if found: sys.exit(1)
elif args[:1] == ["uses"]:
 if args[-1].endswith("/jpeg-xl"): print(os.environ.get("TEST_JXL_USERS", ""))
 else: print("topgrade\natuin")
elif len(args) >= 4 and args[0] == "ruby" and args[2] in ("run", "recover", "repair"):
 state = (root/"state/brew-headsync").resolve()
 assert pathlib.Path(args[1]).parent == state and pathlib.Path(args[1]).is_file()
 assert pathlib.Path(args[3]) == state
 assert os.environ["HOMEBREW_NO_INSTALLED_DEPENDENTS_CHECK"] == "1"
 assert os.environ["HOMEBREW_NO_INSTALL_CLEANUP"] == "1"
 if args[2] == "recover":
  assert len(args) == 4
  if os.environ.get("TEST_JXL_LEAVE_PENDING") != "1":
   (state/"jpeg-xl-active.json").unlink(); print("original kegs restored")
 elif args[2] == "repair":
  pass
 elif os.environ.get("TEST_JXL_FAIL") == "1":
  (state/"jpeg-xl-active.json").write_text("{}")
  print("synthetic ABI rebuild failure", file=sys.stderr); sys.exit(45)
 else:
  (root/"jpeg-xl-committed").touch()
elif args == ["linkage", "--test"]:
 if os.environ.get("TEST_BATCH_BROKEN") == "1":
  print("Broken dependencies:\n  synthetic-jxl-library", file=sys.stderr)
elif args == ["linkage", "--test", "rust"]:
 if os.environ.get("TEST_RUST_HEALTH", "broken") == "broken":
  print("Broken dependencies: synthetic LLVM"); sys.exit(1)
elif args == ["--prefix", "rust"]:
 print(root/"rust-prefix")
elif args == ["list", "--versions", "ggml"]:
 print("ggml HEAD-"+os.environ["TEST_SHA"][:7]+(" HEAD-deadbee" if os.environ.get("TEST_EXTRA_HEAD") else ""))
elif args == ["list", "--versions", "llama.cpp"]:
 print("llama.cpp HEAD-"+os.environ["TEST_SHA"][:7]+(" HEAD-deadbee" if os.environ.get("TEST_EXTRA_HEAD") else ""))
elif len(args) == 2 and args[0] == "--prefix" and args[1] in ("ggml", "llama.cpp"):
 if os.environ.get("TEST_HEADSYNC_NATIVE_CHILD") and not (root/"pair-installed").exists():
  print(root/"previous-opt"/args[1])
 else:
  print(root/"opt"/args[1])
elif len(args) == 2 and args[0] == "--cellar" and args[1] in ("ggml", "llama.cpp"):
 print(root/"Cellar"/args[1])
elif args == ["missing"]:
 pass
else:
 print("unexpected brew call: " + repr(args),file=sys.stderr); sys.exit(35)
'''
git = r'''#!/usr/bin/python3
import os,pathlib,sys
args=sys.argv[1:]
root=pathlib.Path(os.environ["TEST_ROOT"])
with (root/"events").open("a") as f: f.write("git " + " ".join(args) + "\n")
if len(args)>=4 and args[0] == "-C" and args[2] == "status":
 if pathlib.Path(args[1]).resolve() == (root/"tap").resolve():
  state = os.environ.get("TEST_TAP_STATUS", "clean")
  if "brew update" in (root/"events").read_text().splitlines():
   state = os.environ.get("TEST_TAP_AFTER", state)
  if state == "dirty": print(" M Formula/example.rb")
  if state == "unmerged": print("UU Formula/example.rb")
  sys.exit(0)
 state=os.environ.get("TEST_GIT_STATUS", "clean")
 if state == "fail": print("simulated git error",file=sys.stderr); sys.exit(41)
 if state == "dirty": print(" M Formula/g/ggml.rb")
elif len(args)>=3 and args[0] == "-C" and os.environ.get("TEST_CATALOG_REFRESH") == "1":
 repo = pathlib.Path(args[1]).name
 assert repo in ("core", "clean-tap", "local-tap"), args
 if args[2:] == ["branch", "--show-current"]:
  print("main")
 elif args[2:] == ["for-each-ref", "--format=%(upstream)%09%(upstream:remotename)%09%(upstream:remoteref)", "refs/heads/main"]:
  print("refs/remotes/origin/main\torigin\trefs/heads/main" if repo != "local-tap" else "\t\t")
 elif args[2:] == ["fetch", "--no-tags", "--no-recurse-submodules", "--no-write-fetch-head", "--", "origin", "refs/heads/main:refs/remotes/origin/main"]:
  assert repo in ("core", "clean-tap"), args
  assert not (root/"state/brew-headsync/active").exists(), "catalog refresh before journal recovery"
  if os.environ.get("TEST_FAIL") == "fetch-" + repo:
   print("injected catalog fetch failure", file=sys.stderr); sys.exit(46)
  (root/f"{repo}-catalog-fetched").touch()
 elif args[2:] == ["merge", "--ff-only", "--no-autostash", "--no-squash", "--no-overwrite-ignore", "--", "refs/remotes/origin/main"]:
  assert repo in ("core", "clean-tap") and (root/f"{repo}-catalog-fetched").exists(), args
  if os.environ.get("TEST_FAIL") == "merge-" + repo:
   print("injected catalog merge failure", file=sys.stderr); sys.exit(46)
  (root/f"{repo}-catalog-refreshed").touch()
 else:
  print("unexpected catalog Git call: " + repr(args), file=sys.stderr); sys.exit(42)
elif args[:1] == ["ls-remote"]:
 with (root/"events").open("a") as f:
  f.write("gitconfig count=" + os.environ.get("GIT_CONFIG_COUNT", "") + " key0=" + os.environ.get("GIT_CONFIG_KEY_0", "") + " value0=" + os.environ.get("GIT_CONFIG_VALUE_0", "") + " key1=" + os.environ.get("GIT_CONFIG_KEY_1", "") + " value1=" + os.environ.get("GIT_CONFIG_VALUE_1", "") + "\n")
 print(os.environ["TEST_SHA"]+"\trefs/heads/master")
else:
 print("unexpected git call: "+repr(args),file=sys.stderr); sys.exit(42)
'''
helper = r'''#!/bin/sh
printf '%s\n' 'unexpected engine update' >> "$TEST_ROOT/events"
exit 43
'''


class Fixture:
    def __init__(self, root):
        self.root = root
        self.core = root / "core"
        self.state = root / "state/brew-headsync"
        self.home = root / "home"
        self.bin = root / "bin"
        for directory in (self.core / "Formula/g", self.core / "Formula/l", self.state,
                          self.home / ".local/bin", self.bin, root / "opt", root / "rust-prefix/bin",
                          root / "Cellar/llvm/23.1.5", root / "var/homebrew/pinned",
                          root / "Cellar/ggml" / ("HEAD-" + sha[:7]) / "include",
                          root / "Cellar/llama.cpp" / ("HEAD-" + sha[:7]) / "bin"):
            directory.mkdir(parents=True, exist_ok=True)
        (root / "Cellar/llvm/23.1.5/INSTALL_RECEIPT.json").write_text(json.dumps({"source": {"spec": "stable"}}))
        (root / "opt/llvm").symlink_to(root / "Cellar/llvm/23.1.5")
        (root / "var/homebrew/pinned/llvm").symlink_to(root / "Cellar/llvm/23.1.5")
        self.original = {self.core / "Formula/g/ggml.rb": ggml,
                         self.core / "Formula/l/llama.cpp.rb": llama}
        for path, content in self.original.items():
            path.write_text(content)
        for name in ("ggml", "llama.cpp"):
            (root / "opt" / name).symlink_to(root / "Cellar" / name / ("HEAD-" + sha[:7]))
            (root / "opt" / name / "INSTALL_RECEIPT.json").write_text(
                json.dumps({"source": {"spec": "head", "scm_revision": sha}})
            )
        (root / "opt/ggml/include/ggml-backend.h").write_text("ggml_backend_sched_set_copy_callback\n")
        (root / "opt/llama.cpp/bin/llama-cli").write_text("#!/bin/sh\necho llama-synthetic\n")
        (root / "opt/llama.cpp/bin/llama-cli").chmod(0o755)
        (root / "rust-prefix/bin/rustc").write_text("#!/bin/sh\necho rustc-fixture\n")
        (root / "rust-prefix/bin/rustc").chmod(0o755)
        for path, content in ((self.bin / "brew", brew), (self.bin / "git", git),
                              (self.bin / "nm", "#!/bin/sh\necho ggml_backend_sched_set_copy_callback\n"),
                              (self.bin / "llama-cli", "#!/bin/sh\necho llama-synthetic\n"),
                              (self.home / ".local/bin/topgrade-rustup-headsync", helper)):
            if content.startswith("#!/usr/bin/python3"):
                content = content.replace("#!/usr/bin/python3", f"#!{sys.executable}", 1)
            path.write_text(content)
            path.chmod(0o755)
        (self.bin / "python3").symlink_to(sys.executable)
        (self.bin / "python3.15").symlink_to(sys.executable)
        self.env = {"PATH": f"{self.bin}:/usr/bin:/bin:/usr/sbin:/sbin", "HOME": str(self.home),
                    "XDG_STATE_HOME": str(root / "state"), "TEST_ROOT": str(root), "TEST_SHA": sha,
                    "TMPDIR": str(root), "LC_ALL": "C",
                    "GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "core.pager",
                    "GIT_CONFIG_VALUE_0": "fixture-pager",
                    "TEST_PINNED": "llvm",
                    "TEST_INSTALLED": "llvm\ntopgrade\nz3\natuin"}

    def run(self, **overrides):
        for marker in ("bulk-upgraded", "jpeg-xl-committed"):
            (self.root / marker).unlink(missing_ok=True)
        env = self.env | overrides
        if native:
            env["TEST_HEADSYNC_NATIVE_CHILD"] = "1"
        result = subprocess.run(command, env=env, text=True,
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=30)
        assert "unexpected engine update" not in self.events(), result.stdout
        return result

    def events(self):
        path = self.root / "events"
        return path.read_text().splitlines() if path.exists() else []

    def assert_restored(self):
        for path, content in self.original.items():
            assert path.read_text() == content, path
        assert not (self.state / "active").exists()

    def journal(self, foreign=False, incomplete=False):
        journal = self.state / "active"
        for part in ("original", "patched"):
            (journal / part).mkdir(parents=True)
        for path, content in self.original.items():
            (journal / "original" / path.name).write_text(content)
            (journal / "patched" / path.name).write_text("patched " + path.name + "\n")
        first, second = self.original
        first.write_text("patched " + first.name + "\n")
        if foreign:
            second.write_text("foreign edit\n")
        if incomplete:
            (journal / "patched" / second.name).unlink()


def case(name, callback):
    with tempfile.TemporaryDirectory(prefix="headsync-regression-") as directory:
        fixture = Fixture(Path(directory).resolve())
        try:
            callback(fixture)
        except Exception as error:
            print(f"FAIL {name}: {error}")
            return False
    print(f"PASS {name}")
    return True


def clean_guard(f, status):
    result = f.run(TEST_GIT_STATUS=status)
    assert result.returncode != 0, result.stdout
    expected = "homebrew/core is dirty" if status == "dirty" else "cannot inspect homebrew/core status"
    assert expected in result.stdout, result.stdout
    assert "brew update" not in f.events(), f.events()
    f.assert_restored()


def recover(f):
    f.journal()
    result = f.run(TEST_FAIL="update")
    assert result.returncode != 0 and "injected brew update failure" in result.stdout, result.stdout
    f.assert_restored()
    assert "brew update" in f.events(), f.events()


def refusal(f, kind):
    f.journal(foreign=kind == "foreign", incomplete=kind == "incomplete")
    before = {p: p.read_text() for p in f.original}
    result = f.run()
    assert result.returncode != 0, result.stdout
    assert (f.state / "active").is_dir()
    assert all(p.read_text() == contents for p, contents in before.items())
    assert "brew update" not in f.events()


def upgrade_failure(f, name):
    kwargs = {"TEST_FAIL": name}
    if name == "bulk":
        kwargs["TEST_OUTDATED"] = "topgrade\nz3"
    result = f.run(**kwargs)
    assert result.returncode != 0 and "injected upgrade failure" in result.stdout, result.stdout
    f.assert_restored()
    result = f.run()
    assert result.returncode == 0, result.stdout
    f.assert_restored()


def idempotent(f):
    for _ in range(2):
        result = f.run(TEST_OUTDATED="topgrade\nz3")
        assert result.returncode == 0, result.stdout
        assert "HEAD sync transaction: OK" in result.stdout
        f.assert_restored()
    upgrades = [line for line in f.events() if line.startswith("brew upgrade") and "--dry-run" not in line]
    assert len(upgrades) == (4 if native else 6) and all("topgrade" not in line for line in upgrades), upgrades


def dirty_tap_preserves_checkout(f, failed_refresh=""):
    (f.root / "pair-installed").touch()
    dirty_tap = f.root / "tap"
    dirty_tap.mkdir()
    draft = dirty_tap / "local-formula.rb"
    draft.write_text("# local draft\n")
    for name in ("clean-tap", "local-tap"):
        (f.root / name).mkdir()
    result = f.run(TEST_TAP_STATUS="dirty", TEST_CATALOG_REFRESH="1",
                   TEST_INSTALLED="llvm\npostgresql@17", TEST_FAIL=failed_refresh)
    events = f.events()
    refreshes = [line for line in events if line.startswith("git -C ") and
                 (" fetch " in line or " merge " in line)]
    expected = [("core", "fetch")]
    if failed_refresh != "fetch-core":
        expected.extend([("core", "merge"), ("clean-tap", "fetch"), ("clean-tap", "merge")])
    assert len(refreshes) == len(expected) and all(
        event.startswith(f"git -C {f.root / repo} {operation} ")
        for event, (repo, operation) in zip(refreshes, expected, strict=True)
    ), events
    assert "brew update" not in events, events
    dirty_events = [line for line in events if line.startswith(f"git -C {dirty_tap} ")]
    assert dirty_events and all(line.startswith(f"git -C {dirty_tap} status ") for line in dirty_events), events
    assert draft.read_text() == "# local draft\n"
    if failed_refresh:
        diagnostic = f"injected catalog {failed_refresh.split('-', 1)[0]} failure"
        assert result.returncode != 0 and diagnostic in result.stdout, result.stdout
    else:
        assert result.returncode != 0, result.stdout
        deferred = next((line for line in result.stdout.splitlines()
                         if "Homebrew catalog refresh deferred for:" in line), "")
        assert "example/tools" in deferred and "example/local" in deferred, deferred
        assert "example/clean" not in deferred, deferred
    upgrades = [line for line in events if line.startswith("brew upgrade ")]
    if failed_refresh == "fetch-core":
        assert not upgrades and not (f.root / "core-catalog-refreshed").exists(), events
    else:
        assert "already installed at the full target revision" in result.stdout, result.stdout
        assert upgrades == ["brew upgrade --formula --fetch-HEAD postgresql@17"], events
        assert (f.root / "core-catalog-refreshed").exists()
        assert (f.root / "clean-tap-catalog-refreshed").exists() == (not failed_refresh)
        [initial, verified] = [i for i, line in enumerate(events)
                               if line == "brew outdated --formula --fetch-HEAD --json=v2 postgresql@17"]
        assert events.index(refreshes[-1]) < initial < events.index(upgrades[0]) < verified, events
        for repo, start, end in (("core", 0, 1), ("clean-tap", 2, 3)):
            rechecks = events[events.index(refreshes[start]) + 1:events.index(refreshes[end])]
            assert f"git -C {f.root / repo} branch --show-current" in rechecks, rechecks
            assert f"git -C {f.root / repo} status --porcelain=v1 --untracked-files=all" in rechecks, rechecks
    f.assert_restored()


def tap_conflict_stops_before_pair(f, after_update):
    result = f.run(**{"TEST_TAP_AFTER" if after_update else "TEST_TAP_STATUS": "unmerged"})
    assert result.returncode != 0 and "example/tools has unresolved Git conflicts" in result.stdout, result.stdout
    assert ("brew update" in f.events()) == after_update, f.events()
    assert not any(line.startswith("brew upgrade ") for line in f.events()), f.events()
    f.assert_restored()


def outdated_exit_one_without_json_explains_error(f):
    result = f.run(TEST_OUTDATED_MODE="no-json")
    assert result.returncode != 0 and "no formula inventory" in result.stdout, result.stdout
    assert "synthetic formula syntax error" in result.stdout, result.stdout
    f.assert_restored()


def jpeg_xl_abi_transaction(f, users, fail=False):
    target = "nyamiiko/headfixes/jpeg-xl"
    consumer = "homebrew-ffmpeg/ffmpeg/ffmpeg"
    result = f.run(TEST_INSTALLED=f"llvm\n{target}\n{consumer}\nz3", TEST_OUTDATED="jpeg-xl\nffmpeg\nz3",
                   TEST_JXL_USERS=users, TEST_JXL_FAIL=str(int(fail)),
                   TEST_JXL_REBUILDS="ffmpeg" if consumer in users.splitlines() else "")
    events = f.events()
    [transaction] = [line for line in events if line.startswith("brew ruby ") and " run " in line]
    assert transaction.split(" run ", 1)[1].split()[1:] == [target, *users.splitlines()], events
    upgrades = [line for line in events if line.startswith("brew upgrade ")]
    assert all(target not in line for line in upgrades), events
    bulk = [line for line in upgrades if line.endswith(" z3")]
    if fail:
        assert result.returncode != 0 and "original kegs restored" in result.stdout, result.stdout
        assert not bulk, events
        assert not (f.state / "jpeg-xl-active.json").exists()
        [recovery] = [line for line in events if line.startswith("brew ruby ") and " recover " in line]
        assert events.index(transaction) < events.index(recovery), events
    else:
        assert result.returncode == 0 and len(bulk) == 1, (result.stdout, events)
        assert events.index(transaction) < events.index(bulk[0]), events
        assert (consumer not in bulk[0]) == (consumer in users.splitlines()), events
        [before, after] = [i for i, line in enumerate(events)
                           if line.startswith("brew ruby ") and " repair " in line]
        assert before < events.index(transaction) < events.index(bulk[0]) < after, events
    f.assert_restored()


def jpeg_xl_repairs_current_provider(f):
    (f.root / "pair-installed").touch()
    target = "nyamiiko/headfixes/jpeg-xl"
    result = f.run(TEST_INSTALLED=f"llvm\n{target}\nvips", TEST_JXL_USERS="vips")
    assert result.returncode == 0 and "No remaining outdated formulae" in result.stdout, result.stdout
    events = f.events()
    [repair] = [line for line in events if line.startswith("brew ruby ") and " repair " in line]
    assert repair.split(" repair ", 1)[1].split()[1:] == [target, "vips"], events
    assert not any(line.startswith("brew upgrade ") or " run " in line for line in events), events
    f.assert_restored()


def jpeg_xl_recovers_before_brew_refresh(f, leave_pending=False):
    f.state.mkdir(parents=True, exist_ok=True)
    pending = f.state / "jpeg-xl-active.json"
    pending.write_text("{}")
    result = f.run(TEST_JXL_LEAVE_PENDING=str(int(leave_pending)))
    assert f.events()[0].startswith("brew ruby ") and " recover " in f.events()[0], f.events()
    if leave_pending:
        assert result.returncode != 0 and "left a pending journal" in result.stdout, result.stdout
        assert pending.exists() and "brew update" not in f.events(), f.events()
    else:
        assert result.returncode == 0 and not pending.exists(), result.stdout
    f.assert_restored()


def broken_post_upgrade_linkage_fails(f):
    result = f.run(TEST_OUTDATED="z3", TEST_BATCH_BROKEN="1")
    assert result.returncode != 0 and "Linkage check after Homebrew formula upgrade" in result.stdout, result.stdout
    assert "synthetic-jxl-library" in result.stdout, result.stdout
    f.assert_restored()


def rust_guard(f):
    result = f.run(TEST_OUTDATED="topgrade\natuin")
    assert result.returncode != 0 and "depends on Homebrew Rust" in result.stdout, (result.stdout, f.events())
    f.assert_restored()
    assert not any(line.startswith("brew upgrade") and "atuin" in line for line in f.events())


def rust_healthy(f):
    result = f.run(TEST_OUTDATED="topgrade\natuin", TEST_RUST_HEALTH="healthy")
    assert result.returncode == 0, result.stdout
    assert any(line.startswith("brew upgrade") and line.endswith("atuin") for line in f.events())
    f.assert_restored()


def stale_head_is_not_active(f):
    result = f.run(TEST_EXTRA_HEAD="1")
    assert result.returncode == 0, result.stdout
    f.assert_restored()


def wrong_active_head(f):
    old = f.root / "Cellar/llama.cpp/HEAD-deadbee"
    old.mkdir()
    link = f.root / "opt/llama.cpp"
    link.unlink()
    link.symlink_to(old)
    result = f.run()
    assert result.returncode != 0 and "llama.cpp did not land" in result.stdout, result.stdout
    f.assert_restored()


def dangling_pending(f):
    pending = f.core / "Formula/g/ggml.rb.headsync-pending"
    pending.symlink_to(f.root / "outside-missing")
    result = f.run()
    assert result.returncode != 0 and "unexpected pending file" in result.stdout, result.stdout
    assert pending.is_symlink() and not (f.root / "outside-missing").exists()
    f.assert_restored()


def different_full_revision(f):
    receipt = f.root / "opt/llama.cpp/INSTALL_RECEIPT.json"
    receipt.write_text(json.dumps({"source": {"spec": "head", "scm_revision": sha[:7] + "f" * 33}}))
    result = f.run()
    assert result.returncode != 0 and "full revision" in result.stdout, result.stdout
    f.assert_restored()


def mismatched_pair_stops_remaining(f):
    different_full_revision(f)
    events = f.events()
    assert "brew upgrade --formula --fetch-HEAD llama.cpp" in events, events
    assert not any(line.startswith("brew outdated ") for line in events), events


def blocked_pair_does_not_upgrade(f):
    result = f.run(TEST_FAIL_PREFLIGHT="1")
    assert result.returncode != 0 and "preflight failed before either keg was upgraded" in result.stdout, result.stdout
    upgrades = [line for line in f.events() if line.startswith("brew upgrade ")]
    assert upgrades == ["brew upgrade --dry-run --formula --fetch-HEAD ggml llama.cpp"], upgrades
    f.assert_restored()


def pin_scan_exclusions(f):
    versioned = "\nllvm@22" if native else ""
    result = f.run(TEST_INSTALLED=f"llvm\nnyamiiko/tools/llvm-head\ntopgrade\nz3\natuin{versioned}",
                   TEST_OUTDATED="topgrade\nz3")
    assert result.returncode == 0, result.stdout
    scan = next(line for line in f.events() if line.startswith("brew outdated "))
    assert "llvm" not in scan and "topgrade" not in scan, scan
    assert "z3" in scan, scan
    f.assert_restored()


def unrelated_pin_excluded(f):
    result = f.run(TEST_OUTDATED="z3\natuin",
                   TEST_PINNED="llvm\nz3",
                   TEST_RUST_HEALTH="healthy")
    assert result.returncode == 0, result.stdout
    scan = next(line for line in f.events() if line.startswith("brew outdated "))
    assert "z3" not in scan and "atuin" in scan, scan
    bulk = [line for line in f.events() if line == "brew upgrade --formula --fetch-HEAD atuin"]
    assert bulk, f.events()
    f.assert_restored()


def outdated_exit_one_nonempty_is_parsed(f):
    result = f.run(TEST_OUTDATED="atuin", TEST_RUST_HEALTH="healthy")
    assert result.returncode == 0, result.stdout
    scan = next(line for line in f.events() if line.startswith("brew outdated "))
    assert "--json=v2" in scan, scan
    assert "brew upgrade --formula --fetch-HEAD atuin" in f.events(), f.events()
    f.assert_restored()


def successful_bulk_upgrade_leaves_outdated_formula(f):
    result = f.run(TEST_OUTDATED="z3\natuin", TEST_RUST_HEALTH="healthy",
                   TEST_BULK_STILL_OUTDATED="z3")
    assert result.returncode != 0, result.stdout
    assert "Homebrew still reports outdated formulae: z3" in result.stdout, result.stdout
    events = f.events()
    [initial, verified] = [i for i, line in enumerate(events)
                           if line == "brew outdated --formula --fetch-HEAD --json=v2 z3 atuin"]
    assert initial < events.index("brew upgrade --formula --fetch-HEAD z3 atuin") < verified, events
    f.assert_restored()


def successful_jpeg_xl_upgrade_leaves_outdated_formula(f):
    target = "nyamiiko/headfixes/jpeg-xl"
    result = f.run(TEST_INSTALLED=f"llvm\n{target}", TEST_OUTDATED="jpeg-xl",
                   TEST_JXL_STILL_OUTDATED="1")
    assert result.returncode != 0, result.stdout
    assert f"Homebrew still reports outdated formulae: {target}" in result.stdout, result.stdout
    events = f.events()
    [initial, verified] = [i for i, line in enumerate(events)
                           if line == f"brew outdated --formula --fetch-HEAD --json=v2 {target}"]
    [transaction] = [i for i, line in enumerate(events) if line.startswith("brew ruby ") and " run " in line]
    assert initial < transaction < verified, events
    assert not any(line.startswith("brew upgrade ") and target in line for line in events), events
    f.assert_restored()


def outdated_failure_propagates(f, mode):
    result = f.run(TEST_OUTDATED="atuin", TEST_OUTDATED_MODE=mode,
                   TEST_RUST_HEALTH="healthy")
    assert result.returncode != 0, result.stdout
    assert "brew uses --installed --include-build --formula rust" not in f.events(), f.events()
    assert "brew upgrade --formula --fetch-HEAD atuin" not in f.events(), f.events()
    f.assert_restored()


def no_candidates_skip_scan(f):
    result = f.run(TEST_INSTALLED="llvm\ntopgrade")
    assert result.returncode == 0, result.stdout
    assert "No eligible installed formulae; skipping outdated scan" in result.stdout
    assert not any(line.startswith("brew outdated ") for line in f.events()), f.events()
    assert not any(line.startswith("brew upgrade ") and line.endswith("bulk") for line in f.events()), f.events()
    f.assert_restored()


def inherited_git_config_preserved(f):
    result = f.run(TEST_OUTDATED="", TEST_RUST_HEALTH="healthy")
    assert result.returncode == 0, result.stdout
    expected = ("gitconfig count=2 key0=core.pager value0=fixture-pager "
                "key1=url.https://github.com/exiftool/exiftool.git.insteadOf "
                "value1=https://git.code.sf.net/p/exiftool/code.git")
    assert expected in f.events(), f.events()
    f.assert_restored()


def malformed_git_config_count_recovers_first(f):
    f.journal()
    result = f.run(GIT_CONFIG_COUNT="not-a-number")
    assert result.returncode != 0 and "malformed inherited GIT_CONFIG_COUNT" in result.stdout, result.stdout
    f.assert_restored()
    assert "brew update" not in f.events(), f.events()


def missing_llvm_pin_stops_before_updates(f):
    result = f.run(TEST_PINNED="")
    assert result.returncode != 0 and "Homebrew llvm must remain pinned" in result.stdout, result.stdout
    assert "brew update" not in f.events(), f.events()
    f.assert_restored()


def list_failure_propagates(f, which):
    result = f.run(TEST_FAIL_LIST=which)
    expected = "unable to enumerate pinned" if which == "pinned" else "unable to enumerate installed"
    assert result.returncode != 0 and expected in result.stdout, result.stdout
    assert "brew update" not in f.events(), f.events()
    f.assert_restored()


def updated_list_failure(f):
    list_failure_propagates(f, "installed")


def pinned_list_failure(f):
    list_failure_propagates(f, "pinned")


def exact_tap_name(f):
    target = "example/tools/libsndfile"
    result = f.run(TEST_INSTALLED=f"llvm\n{target}", TEST_OUTDATED="libsndfile")
    assert result.returncode == 0, result.stdout
    assert f"brew upgrade --formula --fetch-HEAD {target}" in f.events(), f.events()
    f.assert_restored()


def preserve_formula_metadata(f):
    for path in f.original:
        path.chmod(0o640)
        os.utime(path, ns=(1_570_000_000_000_000_000, 1_570_000_000_123_456_789))
    before = [(p.stat().st_mode, p.stat().st_mtime_ns) for p in f.original]
    result = f.run(TEST_FAIL="llama.cpp")
    assert result.returncode != 0 and "injected upgrade failure" in result.stdout, result.stdout
    f.assert_restored()
    assert [(p.stat().st_mode, p.stat().st_mtime_ns) for p in f.original] == before


def refuses_posix_lock_contention(f):
    with (f.state / "lock").open("a") as lock:
        fcntl.lockf(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        result = f.run()
    assert result.returncode != 0 and "HEAD-sync lock" in result.stdout, result.stdout
    assert not f.events(), f.events()
    f.assert_restored()


def refuses_symlinked_journal(f):
    f.journal()
    journal = f.state / "active"
    relocated = f.root / "outside-journal"
    journal.rename(relocated)
    journal.symlink_to(relocated)
    before = [p.read_bytes() for p in f.original]
    result = f.run()
    assert result.returncode != 0 and "unsafe transaction journal" in result.stdout, result.stdout
    assert journal.is_symlink() and [p.read_bytes() for p in f.original] == before
    assert "brew update" not in f.events(), f.events()


def llvm_link_rejected(f, mode):
    pin = f.root / "var/homebrew/pinned/llvm"
    opt = f.root / "opt/llvm"
    if mode == "mismatch":
        keg = f.root / "Cellar/llvm/23.1.4"
        keg.mkdir()
        pin.unlink()
        pin.symlink_to(keg)
    elif mode == "head":
        keg = f.root / "Cellar/llvm/HEAD-deadbee"
        keg.mkdir()
        (keg / "INSTALL_RECEIPT.json").write_text(json.dumps({"source": {"spec": "head"}}))
        pin.unlink()
        opt.unlink()
        pin.symlink_to(keg)
        opt.symlink_to(keg)
    elif mode == "receipt":
        (f.root / "Cellar/llvm/23.1.5/INSTALL_RECEIPT.json").write_text(json.dumps({"source": {"spec": "head"}}))
    elif mode == "dangling":
        pin.unlink()
        pin.symlink_to(f.root / "Cellar/llvm/missing")
    elif mode == "outside":
        keg = f.root / "outside/23.1.5"
        keg.mkdir(parents=True)
        pin.unlink()
        opt.unlink()
        pin.symlink_to(keg)
        opt.symlink_to(keg)
    elif mode == "missing_receipt":
        (f.root / "Cellar/llvm/23.1.5/INSTALL_RECEIPT.json").unlink()
    elif mode == "not_link":
        pin.unlink()
        pin.mkdir()
    elif mode == "receipt_symlink":
        receipt = f.root / "Cellar/llvm/23.1.5/INSTALL_RECEIPT.json"
        target = f.root / "other-receipt.json"
        target.write_text(json.dumps({"source": {"spec": "stable"}}))
        receipt.unlink()
        receipt.symlink_to(target)
    elif mode == "receipt_oversize":
        (f.root / "Cellar/llvm/23.1.5/INSTALL_RECEIPT.json").write_bytes(b" " * (256 * 1024 + 1))
    result = f.run()
    assert result.returncode != 0 and "LLVM" in result.stdout, result.stdout
    assert "brew update" not in f.events(), f.events()
    assert not any(line.startswith("brew upgrade ") for line in f.events()), f.events()
    f.assert_restored()


def llvm_mismatch_still_recovers_existing_journal(f):
    f.journal()
    keg = f.root / "Cellar/llvm/23.1.4"
    keg.mkdir()
    pin = f.root / "var/homebrew/pinned/llvm"
    pin.unlink()
    pin.symlink_to(keg)
    result = f.run()
    assert result.returncode != 0 and "same LLVM Cellar keg" in result.stdout, result.stdout
    assert "brew update" not in f.events(), f.events()
    f.assert_restored()


def llvm_pin_changed_during_update(f):
    result = f.run(TEST_SWITCH_LLVM_PIN_AFTER_UPDATE="1")
    assert result.returncode != 0 and "same LLVM Cellar keg" in result.stdout, result.stdout
    assert "brew update" in f.events(), f.events()
    assert not any(line.startswith("brew upgrade ") for line in f.events()), f.events()
    f.assert_restored()


def refuses_pinned_pair(f):
    result = f.run(TEST_PINNED="llvm\nllama.cpp")
    assert result.returncode != 0 and "cannot be upgraded while either formula is pinned" in result.stdout, result.stdout
    assert "brew update" not in f.events(), f.events()
    f.assert_restored()


def paused_upgrade(f):
    env = f.env | {"TEST_HEADSYNC_NATIVE_CHILD": "1", "TEST_PAUSE_UPGRADE": "ggml"}
    return subprocess.Popen(command, env=env, text=True, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, start_new_session=True)


def wait_for_paused_upgrade(f, child):
    deadline = time.monotonic() + 10
    while child.poll() is None and not (f.root / "upgrade-paused").exists() and time.monotonic() < deadline:
        time.sleep(0.02)
    assert (f.root / "upgrade-paused").exists(), "mock upgrade never reached the paired transaction"


def terminate_group(child):
    try:
        os.killpg(child.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    child.communicate(timeout=5)


def hard_interruption_recovers(f):
    with paused_upgrade(f) as child:
        try:
            wait_for_paused_upgrade(f, child)
        finally:
            terminate_group(child)
    journal = f.state / "active"
    assert journal.is_dir() and all(sha in p.read_text() for p in f.original)
    result = f.run(TEST_FAIL="update")
    assert result.returncode != 0 and "HEAD-sync transaction recovered" in result.stdout, result.stdout
    f.assert_restored()


def deferred_parent_signal(f, sig):
    with paused_upgrade(f) as child:
        try:
            wait_for_paused_upgrade(f, child)
            os.kill(child.pid, sig)
            competing = f.run()
            assert competing.returncode != 0 and "HEAD-sync lock" in competing.stdout, competing.stdout
            assert child.poll() is None and all(sha in p.read_text() for p in f.original)
            (f.root / "upgrade-release").touch()
            output, _ = child.communicate(timeout=5)
            assert child.returncode == 128 + sig and "HEAD-sync transaction recovered" in output, output
            assert "brew upgrade --formula --fetch-HEAD llama.cpp" not in f.events()
        except Exception:
            terminate_group(child)
            raise
    f.assert_restored()


cases = [
    ("dirty core stops before brew update", lambda f: clean_guard(f, "dirty")),
    ("failed git status stops before brew update", lambda f: clean_guard(f, "fail")),
    ("recover partially applied journal", recover),
    ("preserve unexpected external edit", lambda f: refusal(f, "foreign")),
    ("preserve incomplete journal", lambda f: refusal(f, "incomplete")),
    ("ggml failure recovers and retries", lambda f: upgrade_failure(f, "ggml")),
    ("llama failure recovers and retries", lambda f: upgrade_failure(f, "llama.cpp")),
    ("bulk failure recovers and retries", lambda f: upgrade_failure(f, "bulk")),
    ("repeat successful update", idempotent),
    ("other Homebrew Rust dependent stops", rust_guard),
    ("healthy Homebrew Rust permits its dependent", rust_healthy),
    ("old HEAD keg does not change active result", stale_head_is_not_active),
    ("mismatched active HEAD is rejected", wrong_active_head),
    ("dangling pending file is preserved before journal", dangling_pending),
    ("different full revision with same short SHA is rejected", different_full_revision),
    ("LLVM and Topgrade excluded from HEAD scan", pin_scan_exclusions),
    ("unrelated pinned formula excluded exactly", unrelated_pin_excluded),
    ("outdated exit 1 with nonempty JSON succeeds", outdated_exit_one_nonempty_is_parsed),
    ("malformed outdated JSON fails closed", lambda f: outdated_failure_propagates(f, "malformed")),
    ("outdated exit 1 with empty JSON fails closed", lambda f: outdated_failure_propagates(f, "one-empty")),
    ("outdated operational error propagates", lambda f: outdated_failure_propagates(f, "operational")),
    ("newly pinned outdated formula fails closed", lambda f: outdated_failure_propagates(f, "newly-pinned")),
    ("unselected outdated formula fails closed", lambda f: outdated_failure_propagates(f, "unselected")),
    ("duplicate outdated formula fails closed", lambda f: outdated_failure_propagates(f, "duplicate")),
    ("empty candidate set skips outdated and bulk upgrade", no_candidates_skip_scan),
    ("inherited Git config retained with exact ExifTool rewrite", inherited_git_config_preserved),
    ("malformed Git config fails after journal recovery", malformed_git_config_count_recovers_first),
    ("missing LLVM pin stops before updates", missing_llvm_pin_stops_before_updates),
    ("installed formula enumeration error propagates", updated_list_failure),
    ("pinned formula enumeration error propagates", pinned_list_failure),
]
if native:
    cases += [
        ("JPEG XL and installed dependents use one coordinated upgrade", lambda f: jpeg_xl_abi_transaction(f, "vips\nhomebrew-ffmpeg/ffmpeg/ffmpeg")),
        ("JPEG XL with no declared dependents still uses the transaction", lambda f: jpeg_xl_abi_transaction(f, "")),
        ("JPEG XL rollback stops the rest of the batch", lambda f: jpeg_xl_abi_transaction(f, "vips", True)),
        ("JPEG XL repair runs even with no outdated formulas", jpeg_xl_repairs_current_provider),
        ("Interrupted JPEG XL recovery runs before Homebrew refresh", jpeg_xl_recovers_before_brew_refresh),
        ("Pending JPEG XL recovery prevents Homebrew refresh", lambda f: jpeg_xl_recovers_before_brew_refresh(f, True)),
        ("broken linkage after a successful Homebrew command is a failure", broken_post_upgrade_linkage_fails),
        ("bulk upgrade exit 0 with a remaining outdated formula is incomplete", successful_bulk_upgrade_leaves_outdated_formula),
        ("JPEG XL transaction exit 0 with an outdated target is incomplete", successful_jpeg_xl_upgrade_leaves_outdated_formula),
        ("dirty tap is preserved while clean catalogs reveal a stable upgrade", dirty_tap_preserves_checkout),
        ("core catalog fetch failure prevents formula upgrades", lambda f: dirty_tap_preserves_checkout(f, "fetch-core")),
        ("clean tap merge failure still upgrades from the refreshed core catalog", lambda f: dirty_tap_preserves_checkout(f, "merge-clean-tap")),
        ("existing tap conflict stops before either mutation", lambda f: tap_conflict_stops_before_pair(f, False)),
        ("tap conflict after refresh stops before either keg upgrade", lambda f: tap_conflict_stops_before_pair(f, True)),
        ("exit 1 without JSON retains the Homebrew diagnostic", outdated_exit_one_without_json_explains_error),
        ("LLVM mismatched pin stops before mutation", lambda f: llvm_link_rejected(f, "mismatch")),
        ("LLVM mismatch still recovers existing journal", llvm_mismatch_still_recovers_existing_journal),
        ("LLVM pin change during update stops before formula patch", llvm_pin_changed_during_update),
        ("LLVM HEAD pin stops before mutation", lambda f: llvm_link_rejected(f, "head")),
        ("LLVM non-stable receipt stops before mutation", lambda f: llvm_link_rejected(f, "receipt")),
        ("LLVM dangling pin stops before mutation", lambda f: llvm_link_rejected(f, "dangling")),
        ("LLVM keg outside cellar stops before mutation", lambda f: llvm_link_rejected(f, "outside")),
        ("LLVM missing receipt stops before mutation", lambda f: llvm_link_rejected(f, "missing_receipt")),
        ("LLVM non-symlink pin stops before mutation", lambda f: llvm_link_rejected(f, "not_link")),
        ("LLVM symlinked receipt stops before mutation", lambda f: llvm_link_rejected(f, "receipt_symlink")),
        ("LLVM oversized receipt stops before mutation", lambda f: llvm_link_rejected(f, "receipt_oversize")),
        ("pinned dependency failure stops both keg upgrades", blocked_pair_does_not_upgrade),
        ("mismatched active pair stops before remaining enumeration", mismatched_pair_stops_remaining),
        ("tap-qualified formula name survives the exact scan and upgrade", exact_tap_name),
        ("restored formulae retain original mode and nanosecond mtime", preserve_formula_metadata),
        ("legacy POSIX lock excludes the Rust updater", refuses_posix_lock_contention),
        ("symlinked journal is preserved without following it", refuses_symlinked_journal),
        ("pinned synchronized pair stops before updates", refuses_pinned_pair),
        ("SIGKILL after both formulae were patched replays the journal", hard_interruption_recovers),
        ("parent-only SIGTERM waits for brew and restores both formulae", lambda f: deferred_parent_signal(f, signal.SIGTERM)),
        ("parent-only SIGHUP waits for brew and restores both formulae", lambda f: deferred_parent_signal(f, signal.SIGHUP)),
    ]
results = [case(name, callback) for name, callback in cases]
raise SystemExit(0 if all(results) else 1)
