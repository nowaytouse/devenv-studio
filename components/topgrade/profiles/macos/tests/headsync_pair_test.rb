# frozen_string_literal: true

# Isolated filesystem regressions for the real pair transaction. Only Homebrew's
# Keg/lock/cache boundary, the builder, and macOS executable probes are doubled.
require "tmpdir"
require "pathname"
require "fileutils"
require "json"
require "find"

%w[keg formula_installer cache_store linkage_cache_store reinstall].each do |name|
  $LOADED_FEATURES << "#{name}.rb"
end

ROOT = Pathname(Dir.mktmpdir("headsync-pair-test-")).realpath
HOMEBREW_PREFIX = ROOT/"prefix"
HOMEBREW_CELLAR = HOMEBREW_PREFIX/"Cellar"
HOMEBREW_BREW_FILE = "/fixture/brew"
OLD = "1111111111111111111111111111111111111111"
TARGET = "2222222222222222222222222222222222222222"

class FormulaLock
  def initialize(_name); end
  def with_lock
    yield
  end
end

class CacheStoreDatabase
  def self.use(_name)
    yield nil
  end
end

class LinkageCacheStore
  def initialize(_path, _db); end
  def delete!; end
end

class Keg
  def initialize(path)
    @path = Pathname(path)
    raise "Not a fixture keg" unless @path.directory? && @path.dirname.dirname == HOMEBREW_CELLAR
  end

  # Homebrew's path accessor is protected; calling keg.path is a real regression.
  attr_reader :path
  protected :path

  def name
    @path.dirname.basename.to_s
  end

  def linked_record
    HOMEBREW_PREFIX/"var/homebrew/linked"/name
  end

  def linked?
    linked_record.symlink? && linked_record.realpath == @path
  end

  def unlink
    (@path/"bin").children.each do |source|
      destination = HOMEBREW_PREFIX/"bin"/source.basename
      destination.unlink if destination.symlink? && destination.realpath == source.realpath
    end
    linked_record.unlink if linked?
  end

  def optlink
    destination = HOMEBREW_PREFIX/"opt"/name
    raise "Refusing non-symlink opt overwrite" if destination.exist? && !destination.symlink?

    destination.unlink if destination.symlink?
    destination.make_symlink(@path)
  end

  def link
    raise "Another keg is already linked" if linked_record.exist?

    optlink
    (@path/"bin").children.each do |source|
      destination = HOMEBREW_PREFIX/"bin"/source.basename
      raise "Conflicting prefix file #{destination}" if destination.exist? || destination.symlink?

      destination.make_symlink(source)
    end
    linked_record.make_symlink(@path)
  end
end

load ARGV.fetch(0)
raise "Pair upgrade transaction is not implemented" unless defined?(GgmlLlamaUpgrade)

class FixturePair < GgmlLlamaUpgrade
  attr_accessor :fail_formula, :fail_recovery, :fail_runtime, :unrelated_during_failure

  def build(short, revision, suffix = "")
    keg = rack(short)/"HEAD-#{revision[0, 7]}#{suffix}"
    (keg/"bin").mkpath
    (keg/"bin"/(short == "ggml" ? "ggml-tool" : "llama-cli")).write(revision)
    (keg/"INSTALL_RECEIPT.json").write(JSON.generate({
      "source" => { "spec" => "head", "tap" => "homebrew/core", "scm_revision" => revision },
      "used_options" => [],
    }))
    keg
  end

  def run_command(*args)
    interrupted!
    case args.first.to_s
    when "/usr/bin/ditto"
      FileUtils.cp_r(args.fetch(1).to_s, args.fetch(2).to_s, preserve: true)
    when "/bin/sync"
      # The fixture checks recovery contents; production still fsyncs the journal.
    when HOMEBREW_BREW_FILE
      raise "Unexpected builder arguments #{args.inspect}" unless args[1, 3] == ["upgrade", "--formula", "--fetch-HEAD"]

      short = args.fetch(4)
      build(short, TARGET, "_2")
      build("ggml", "3333333333333333333333333333333333333333") if unrelated_during_failure && fail_formula == short
      raise "Injected #{short} build failure" if fail_formula == short

      # Model brew's ambiguous HEAD selection: the new keg exists but an older
      # receipt still owns opt and bin. The real transaction must activate exact.
    else
      raise "Unexpected fixture command #{args.inspect}"
    end
    interrupted!
  end

  def verify_runtime(active)
    raise "Injected target runtime failure" if fail_runtime && receipt(active.fetch("llama.cpp")).dig("source", "scm_revision") == TARGET

    active.each do |short, keg|
      file = keg/"bin"/(short == "ggml" ? "ggml-tool" : "llama-cli")
      raise "Fixture binary content mismatch" unless file.read == receipt(keg).dig("source", "scm_revision")
    end
  end

  def restore(row)
    raise "Injected rollback failure" if fail_recovery

    super
  end
end

def assert(value, message)
  raise message unless value
end

def fixture
  FileUtils.rm_rf(HOMEBREW_PREFIX)
  FileUtils.rm_rf(ROOT/"state")
  [HOMEBREW_CELLAR, HOMEBREW_PREFIX/"bin", HOMEBREW_PREFIX/"opt", HOMEBREW_PREFIX/"var/homebrew/linked"].each(&:mkpath)
  (ROOT/"state").mkdir(0o700)
  pair = FixturePair.new(ROOT/"state")
  pair.providers.each { |short| Keg.new(pair.build(short, OLD)).link }
  pair
end

def expect_failure(pattern)
  yield
rescue StandardError => error
  raise "Wrong failure: #{error.message}" unless pattern.match?(error.message)
else
  raise "Expected failure matching #{pattern.inspect}"
end

def assert_active(pair, revision)
  pair.providers.each do |short|
    active = pair.opt(short).realpath
    assert(pair.receipt(active).dig("source", "scm_revision") == revision, "Wrong full receipt for #{short}")
    assert(pair.linked(short).realpath == active, "Wrong linked record for #{short}")
    binary = short == "ggml" ? "ggml-tool" : "llama-cli"
    assert((HOMEBREW_PREFIX/"bin"/binary).realpath == active/"bin"/binary, "Wrong prefix binary for #{short}")
  end
end

cases = {
  "upgrades activate exact new receipts and preserve old kegs" => lambda do
    pair = fixture
    pair.upgrade_pair(TARGET)
    assert_active(pair, TARGET)
    assert(pair.providers.all? { |short| (pair.rack(short)/"HEAD-1111111").directory? }, "Prior kegs were deleted")
    assert(!(ROOT/"state/ggml-llama-upgrade-active.json").exist?, "Committed journal remains")
  end,
  "llama failure restores the previously matched pair" => lambda do
    pair = fixture
    pair.fail_formula = "llama.cpp"
    expect_failure(/Injected llama.cpp build failure/) { pair.upgrade_pair(TARGET) }
    assert_active(pair, OLD)
    assert(!(ROOT/"state/ggml-llama-upgrade-active.json").exist?, "Recovered journal remains")
    assert(Dir.glob((ROOT/"state/ggml-llama-upgrade-*/archive/ggml/*").to_s).any?, "New ggml keg was discarded instead of archived")
  end,
  "runtime failure also restores the pair" => lambda do
    pair = fixture
    pair.fail_runtime = true
    expect_failure(/Injected target runtime failure/) { pair.upgrade_pair(TARGET) }
    assert_active(pair, OLD)
  end,
  "the unified launcher is probed and its failure is propagated" => lambda do
    pair = fixture
    active = pair.providers.to_h { |short| [short, pair.opt(short).realpath] }
    include_dir = active.fetch("ggml")/"include"
    include_dir.mkdir
    (include_dir/"ggml-backend.h").write("void ggml_backend_sched_set_copy_callback(void);")
    cli = active.fetch("llama.cpp")/"bin/llama-cli"
    unified = active.fetch("llama.cpp")/"bin/llama"
    cli.write("#!/bin/sh\nexit 0\n")
    unified.write("#!/bin/sh\nexit 7\n")
    [cli, unified].each { |path| path.chmod(0o700) }
    pair.define_singleton_method(:capture) { |*_args| "00000001 T _ggml_backend_sched_set_copy_callback\n" }
    pair.define_singleton_method(:run_command) do |*args|
      raise "Synthetic executable failed" unless system(*args.map(&:to_s))
    end
    probe = -> { GgmlLlamaUpgrade.instance_method(:verify_runtime).bind_call(pair, active) }
    expect_failure(/Synthetic executable failed/) { probe.call }
    unified.unlink
    probe.call
  end,
  "rollback failure keeps its journal and is recoverable later" => lambda do
    pair = fixture
    pair.fail_formula = "llama.cpp"
    pair.fail_recovery = true
    expect_failure(/recovery incomplete.*Injected rollback failure/) { pair.upgrade_pair(TARGET) }
    assert((ROOT/"state/ggml-llama-upgrade-active.json").file?, "Rollback failure lost its journal")
    FixturePair.new(ROOT/"state").recover
    assert_active(pair, OLD)
    assert(!(ROOT/"state/ggml-llama-upgrade-active.json").exist?, "Resumed recovery left its journal")
  end,
  "a concurrent unrelated provider keg is preserved with the pending journal" => lambda do
    pair = fixture
    pair.fail_formula = "llama.cpp"
    pair.unrelated_during_failure = true
    expect_failure(/Unexpected provider keg during recovery.*preserved in place/) { pair.upgrade_pair(TARGET) }
    unexpected = pair.rack("ggml")/"HEAD-3333333"
    assert(pair.receipt(unexpected).dig("source", "scm_revision") == "3333333333333333333333333333333333333333", "Concurrent keg was changed")
    assert((unexpected/"bin/ggml-tool").read == "3333333333333333333333333333333333333333", "Concurrent keg bytes changed")
    assert((ROOT/"state/ggml-llama-upgrade-active.json").file?, "Concurrent state conflict discarded its journal")
  end,
  "different full revisions with the same prefix cannot be substituted" => lambda do
    pair = fixture
    wrong = "2222222333333333333333333333333333333333"
    pair.build("ggml", wrong)
    expect_failure(/full revision|collision/) { pair.upgrade_pair(TARGET) }
    assert_active(pair, OLD)
    assert(pair.receipt(pair.rack("ggml")/"HEAD-2222222").dig("source", "scm_revision") == wrong, "Existing colliding keg changed")
  end,
  "newest rebuild is selected only among full receipt matches" => lambda do
    pair = fixture
    pair.providers.each do |short|
      pair.build(short, "2222222333333333333333333333333333333333", "_9")
      pair.build(short, TARGET, "_2")
      pair.build(short, TARGET, "_3")
    end
    pair.upgrade_pair(TARGET)
    assert_active(pair, TARGET)
    assert(pair.providers.all? { |short| pair.opt(short).realpath.basename.to_s == "HEAD-2222222_3" }, "Wrong exact rebuild selected")
  end,
  "unmatched starting pair is rejected before changing kegs" => lambda do
    pair = fixture
    different = pair.build("llama.cpp", "3333333333333333333333333333333333333333")
    Keg.new(pair.opt("llama.cpp").realpath).unlink
    Keg.new(different).link
    before = pair.providers.map { |short| pair.manifest(pair.rack(short)) }
    expect_failure(/matched|same full revision/) { pair.upgrade_pair(TARGET) }
    assert(pair.providers.map { |short| pair.manifest(pair.rack(short)) } == before, "Rejected plan changed kegs")
    assert(!(ROOT/"state/ggml-llama-upgrade-active.json").exist?, "Rejected plan opened a transaction")
  end,
  "installed exact target repairs missing opt linked record and prefix binary" => lambda do
    pair = fixture
    pair.providers.each do |short|
      pair.build(short, TARGET, "_2")
      Keg.new(pair.opt(short).realpath).unlink
    end
    pair.opt("llama.cpp").unlink
    pair.fail_formula = "ggml"
    pair.upgrade_pair(TARGET)
    assert_active(pair, TARGET)
    assert(pair.providers.all? { |short| (pair.rack(short)/"HEAD-1111111").directory? }, "Link repair deleted prior kegs")
  end,
  "link repair preserves an unrelated prefix file and reports incomplete work" => lambda do
    pair = fixture
    pair.providers.each { |short| pair.build(short, TARGET, "_2") }
    Keg.new(pair.opt("llama.cpp").realpath).unlink
    file = HOMEBREW_PREFIX/"bin/llama-cli"
    file.write("user-owned executable")
    expect_failure(/link repair is incomplete.*Conflicting prefix file/) { pair.upgrade_pair(TARGET) }
    assert(file.read == "user-owned executable", "Unrelated file was overwritten")
    assert(pair.providers.all? { |short| pair.versions(short) == ["HEAD-1111111", "HEAD-2222222_2"] }, "Failed link repair changed kegs")
  end,
}

failures = 0
begin
  cases.each do |name, run|
    begin
      run.call
      puts "PASS #{name}"
    rescue StandardError => error
      failures += 1
      warn "FAIL #{name}: #{error.full_message}"
    end
  end
ensure
  FileUtils.remove_entry_secure(ROOT)
end
exit(failures.zero? ? 0 : 1)
