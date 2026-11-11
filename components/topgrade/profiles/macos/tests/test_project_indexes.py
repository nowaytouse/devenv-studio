"""Exercise private project indexes with committed synthetic fixtures."""

import json
from pathlib import Path
import runpy
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "bin/topgrade-project-indexes.py"


class ProjectIndexTests(unittest.TestCase):
    def test_stale_receipts_and_invalid_metadata_leave_the_active_directory(self):
        api = runpy.run_path(str(SCRIPT))
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            current = {
                "schema": 3, "commit": "1" * 40, "checkout_id": "a" * 64,
                "tools": {"repomix": "fixture", "lean_ctx": "fixture"},
            }
            stale = {
                **current, "commit": "2" * 40, "generated_at": "2026-01-01T00:00:00+00:00",
                "source_files": 1, "excluded_by_security": 0,
                "token_tree": "────\n└── sample.rs (1 token)",
                "token_heatmap": [{"path": "sample.rs", "token_count": 1}],
            }
            data = json.dumps(stale).encode()
            invalid = b'{"schema":3,"schema":3}'
            unrelated = root / "unrelated.json"
            for name, content in (
                ("latest.json", data), (api["dated_name"](stale), data),
                ("20260101T000000000000Z-333333333333.json", invalid),
                (unrelated.name, b"untouched"),
            ):
                api["publish"](root / name, content)
            self.assertIsNone(api["tidy_receipts"](root, current))
            self.assertFalse((root / "latest.json").exists())
            self.assertEqual([item.read_bytes() for item in (root / "history").glob("*/*.json")], [data, data])
            self.assertEqual([item.read_bytes() for item in (root / "quarantine").glob("*/*.json")], [invalid])
            self.assertEqual(unrelated.read_bytes(), b"untouched")
            self.assertIsNone(api["decode_receipt"](json.dumps({
                **stale, "token_heatmap": [{"path": "../sample.rs", "token_count": 1}],
            }).encode()))

    def test_serena_inventory_never_loads_pickles_and_only_moves_empty_owned_cache_on_request(self):
        api = runpy.run_path(str(SCRIPT))
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            repo, index = root / "fixture", root / "index"
            cache = repo / ".serena/cache/rust"
            cache.mkdir(parents=True)
            index.mkdir()
            metadata = repo / ".serena/project.yml"
            metadata.write_text("synthetic metadata")
            native = cache / "document_symbols.pkl"
            native.write_bytes(b"not valid pickle, never opened")
            empty = cache / "raw_document_symbols.pkl"
            empty.touch()
            env = {"HOME": folder, "PATH": "/usr/bin:/bin"}
            api["serena_inventory"](repo, index, env)
            receipt = json.loads((index / "serena.json").read_text())
            self.assertEqual([row["status"] for row in receipt["files"]], ["unverified", "invalid"])
            self.assertTrue(empty.is_file())
            api["serena_inventory"](repo, index, env, quarantine_empty=True)
            receipt = json.loads((index / "serena.json").read_text())
            self.assertEqual([row["status"] for row in receipt["files"]], ["unverified", "quarantined-empty"])
            self.assertEqual([file.read_bytes() for file in (index / "serena-quarantine").glob("*/*.pkl")], [b""])
            self.assertFalse(empty.exists())
            self.assertEqual(native.read_bytes(), b"not valid pickle, never opened")
            self.assertEqual(metadata.read_text(), "synthetic metadata")
            empty.symlink_to(native)
            api["serena_inventory"](repo, index, env, quarantine_empty=True)
            self.assertTrue(empty.is_symlink())

    @unittest.skipUnless(
        Path("/opt/homebrew/bin/repomix").is_file()
        and Path("/opt/homebrew/bin/lean-ctx").is_file(),
        "local indexing tools are absent",
    )
    def test_only_scanned_committed_sources_reach_private_receipt(self):
        refresh = runpy.run_path(str(SCRIPT))["refresh"]
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            repo = root / "fixture"
            repo.mkdir()
            env = {"HOME": folder, "PATH": "/usr/bin:/bin", "GIT_CONFIG_NOSYSTEM": "1"}

            def git(*args):
                return subprocess.check_output(["/usr/bin/git", *args], cwd=repo, env=env).decode().strip()

            git("init", "--template=", "--quiet")
            (repo / "public.rs").write_text("pub fn synthetic() -> u8 { 42 }\n")
            (repo / "private").mkdir()
            (repo / "private" / "customer.rs").write_text("synthetic private fixture")
            (root / "outside.rs").write_text("synthetic outside fixture")
            (repo / "linked.rs").symlink_to(root / "outside.rs")
            (repo / ".env").write_text("EXAMPLE_TOKEN=synthetic-fixture\n")
            git("add", "--", ".")
            git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.test",
                "commit", "--quiet", "-m", "synthetic fixture")
            (repo / "untracked.rs").write_text("synthetic untracked fixture")
            (repo / "public.rs").write_text("synthetic dirty replacement")
            destination = root / "index"
            refresh(repo, destination)
            latest = destination / "latest.json"
            data = latest.read_bytes()
            receipt = json.loads(data)
            self.assertEqual(receipt["schema"], 3)
            self.assertEqual(receipt["commit"], git("rev-parse", "HEAD"))
            self.assertEqual(receipt["source_files"], 1)
            self.assertEqual(receipt["excluded_by_security"], 0)
            self.assertIn(b"public.rs", data)
            for fragment in (b"customer.rs", b"linked.rs", b"untracked.rs", b".env", b"synthetic"):
                self.assertNotIn(fragment, data)
            refresh(repo, destination)
            self.assertEqual(latest.read_bytes(), data)


if __name__ == "__main__":
    unittest.main()
