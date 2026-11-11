"""Exercise private project indexes with committed synthetic fixtures."""

import json
from pathlib import Path
import runpy
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "bin/topgrade-project-indexes.py"


class ProjectIndexTests(unittest.TestCase):
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
