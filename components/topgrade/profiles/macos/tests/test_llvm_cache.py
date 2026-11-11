"""Exercise the LLVM cache with a synthetic Git repository and no network."""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import unittest


WORKER = Path(__file__).resolve().parents[3] / "src/studio/llvm.rb"
BREW = shutil.which("brew") if sys.platform == "darwin" else None


@unittest.skipUnless(BREW, "requires macOS Homebrew Ruby")
class LlvmCacheTests(unittest.TestCase):
    def test_cache_publication_and_reuse(self):
        harness = r"""
require "pathname"

worker = ARGV.fetch(0)
definitions, driver = File.binread(worker).split("$stdout.sync = true", 2)
raise "Could not locate the worker driver" unless driver
eval(definitions, TOPLEVEL_BINDING, worker)

Dir.mktmpdir do |temporary|
  directory = Pathname(File.realpath(temporary))
  repo = directory/"fixture"
  git = "/usr/bin/git"
  llvm_capture(git, "init", "--quiet", "-b", "main", repo.to_s)
  llvm_capture(git, "-C", repo.to_s, "config", "uploadpack.allowFilter", "true")
  File.write(repo/"example.txt", "synthetic source\n")
  llvm_capture(git, "-C", repo.to_s, "add", "example.txt")
  llvm_capture(git, "-C", repo.to_s, "-c", "user.name=Synthetic Fixture",
               "-c", "user.email=fixture@example.invalid", "commit", "--quiet", "-m", "Fixture")
  revision = llvm_capture(git, "-C", repo.to_s, "rev-parse", "HEAD")
  Object.send(:remove_const, :LLVM_REMOTE)
  Object.const_set(:LLVM_REMOTE, "file://#{repo}")
  cache = directory/"llvm-head--git"

  begin
    llvm_prepare_head_cache(cache, "0" * 40)
    raise "Stale commit was accepted"
  rescue RuntimeError => error
    raise unless error.message == "LLVM main changed during fetch. Review the new revision first."
  end
  raise "Stale fetch left staging files" unless Dir.children(directory) == ["fixture"]

  llvm_prepare_head_cache(cache, revision)
  raise "Wrong cache content" unless File.read(cache/"example.txt") == "synthetic source\n"
  raise "Wrong cache commit" unless llvm_capture(git, "-C", cache.to_s, "rev-parse", "HEAD") == revision
  shallow = llvm_capture(git, "-C", cache.to_s, "rev-parse", "--is-shallow-repository")
  filter = llvm_capture(git, "-C", cache.to_s, "config", "remote.origin.partialclonefilter")
  raise "Expected a non-shallow partial clone" unless shallow == "false" && filter == "blob:none"

  File.rename(repo, directory/"offline")
  original_inode = cache.stat.ino
  llvm_prepare_head_cache(cache, revision)
  raise "Existing cache was replaced" unless cache.stat.ino == original_inode

  begin
    llvm_prepare_head_cache(directory/"failed-cache", revision)
    raise "Unavailable source was accepted"
  rescue RuntimeError => error
    raise unless error.message.start_with?("git failed:")
  end
  raise "Failed download left staging files" unless Dir.children(directory).sort == ["llvm-head--git", "offline"]

  keg = Class.new do
    def to_s = "/synthetic/keg"

    protected

    def path = Pathname(to_s)
  end.new
  opt = directory/"opt"
  installer = StudioLlvmInstaller.allocate
  installer.define_singleton_method(:formula) { Struct.new(:opt_prefix).new(opt) }
  installer.link(keg)
  raise "Unexpected opt link" unless File.readlink(opt) == keg.to_s
end
"""
        subprocess.run(
            [BREW, "ruby", "-e", harness, "--", str(WORKER)],
            env={
                **os.environ,
                "HOMEBREW_NO_AUTO_UPDATE": "1",
                "GIT_ALLOW_PROTOCOL": "file",
                "GIT_CONFIG_NOSYSTEM": "1",
                "GIT_CONFIG_GLOBAL": os.devnull,
                "GIT_CONFIG_COUNT": "0",
                "GIT_CONFIG_PARAMETERS": "",
            },
            check=True,
        )
