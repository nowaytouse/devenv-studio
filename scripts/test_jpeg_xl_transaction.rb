# frozen_string_literal: true

# Run with brew ruby. Every package path and command below is synthetic.
require "tmpdir"
require_relative "../components/topgrade/src/studio/headsync/jpeg_xl"

%i[Keg FormulaLock HOMEBREW_PREFIX HOMEBREW_CELLAR].each { |name| Object.send(:remove_const, name) }

class FormulaLock
  def initialize(_name); end

  def with_lock
    yield
  end
end

class Keg
  def initialize(path)
    @path = path
    name = path.dirname.basename
    @opt = HOMEBREW_PREFIX/"opt"/name
    @link = HOMEBREW_PREFIX/"var/homebrew/linked"/name
  end

  def unlink
    @link.unlink if linked?
  end

  def linked?
    @link.symlink? && @link.readlink == @path
  end

  def optlink
    @opt.unlink if @opt.symlink?
    @opt.make_symlink(@path)
  end

  def link
    raise "Existing synthetic linked record" if @link.symlink?

    optlink
    @link.make_symlink(@path)
  end
end

class Fixture < JpegXlTransaction
  JXL = "example/tap/jpeg-xl"
  CONSUMER = "example/tap/consumer"
  OLD = "HEAD-aaaaaaa"
  NEW = "HEAD-bbbbbbb"

  attr_reader :builds

  def initialize(state, scenario = :success)
    super(state)
    @scenario = scenario
    @builds = []
  end

  def install(full, version, library, options: [], stamp: "initial", dependency: false)
    short = name(full)
    path = rack(short)/version
    FileUtils.mkdir_p(path/"lib")
    source = { "tap" => full.split("/")[0, 2].join("/"), "spec" => version.start_with?("HEAD-") ? "head" : "stable" }
    source["scm_revision"] = version[5] * 40 if source["spec"] == "head"
    tab = { "source" => source, "used_options" => options, "installed_on_request" => !dependency }
    (path/"INSTALL_RECEIPT.json").write(JSON.generate(tab))
    (path/"stamp").write(stamp)
    if short == "jpeg-xl"
      (path/"lib"/library).write("synthetic dylib")
    elsif short == "consumer"
      (path/"links").write((opt("jpeg-xl")/"lib"/library).to_s)
    end
    Keg.new(path).link
    path
  end

  def setup
    abi = repair_fixture? ? "0.14" : "0.13"
    install(JXL, OLD, "libjxl.#{abi}.dylib")
    install(CONSUMER, "1.0", "libjxl.0.13.dylib", options: ["--with-jxl"])
  end

  def repair_fixture?
    %i[repair repair_failed repair_restart repair_still_broken].include?(@scenario)
  end

  def plan(full, users)
    [[full, *users].map do |formula|
      row, path = active_row(formula)
      row.merge("links" => direct_links(path))
    end, { "bison" => "3.8.2" }]
  end

  def dependency_order(consumers)
    consumers
  end

  def preflight(_row)
    {}
  end

  def direct_links(keg)
    path = keg/"links"
    path.file? ? [path.read] : []
  end

  def run_command(*args)
    case args.first
    when "/usr/bin/ditto" then FileUtils.cp_r(args[1], args[2], preserve: true)
    when "/bin/sync" then nil
    else raise "Unexpected fixture command: #{args.inspect}"
    end
  end

  def brew(*args)
    @builds << args.first
    case args.first
    when "upgrade"
      Keg.new(rack("jpeg-xl")/OLD).unlink
      abi = @scenario == :same_abi ? "0.13" : "0.14"
      install(JXL, NEW, "libjxl.#{abi}.dylib", stamp: "candidate")
      install("homebrew/core/bison", "3.8.2", "", dependency: true) if @scenario == :failed
      rack("bison").mkdir if @scenario == :failed_missing_rack
    when "reinstall"
      old = rack("consumer")/"1.0"
      Keg.new(old).unlink
      if @scenario == :failed_missing_rack
        FileUtils.remove_entry_secure(rack("consumer"))
        raise "synthetic interruption before the replacement rack exists"
      end

      staged = rack("consumer")/"1.0.reinstall"
      old.rename(staged)
      abi = @scenario == :repair_still_broken ? "0.13" : "0.14"
      install(CONSUMER, "1.0", "libjxl.#{abi}.dylib", options: ["--with-jxl"], stamp: "replacement")
      raise "synthetic rebuild failure" if %i[failed repair_failed].include?(@scenario)

      FileUtils.remove_entry_secure(staged)
    else raise "Unexpected Brew command: #{args.inspect}"
    end
  end

  def reset_linkage_cache(_rows); end

  def smoke(_jpeg, _consumers)
    verify_linkage if repair_fixture?
  end

  def capture(*)
    return "Missing libraries:\n  test.dylib\n" if @scenario == :zero_status_broken
    if repair_fixture? && direct_links(opt("consumer")).any? { |link| !File.file?(link) }
      return "Broken dependencies:\n  missing JPEG XL library\n"
    end
    if @scenario == :failed_retirement && versions("jpeg-xl") == [NEW]
      raise "synthetic linkage failure after retirement"
    end

    ""
  end
end

def with_fixture(scenario)
  Dir.mktmpdir("studio-jxl-fixture") do |root|
    prefix = Pathname(root).realpath
    Object.const_set(:HOMEBREW_PREFIX, prefix)
    Object.const_set(:HOMEBREW_CELLAR, prefix/"Cellar")
    [HOMEBREW_CELLAR, prefix/"opt", prefix/"var/homebrew/linked"].each { |path| FileUtils.mkdir_p(path) }
    state = prefix/"state"
    state.mkdir(0o700)
    fixture = Fixture.new(state, scenario)
    fixture.setup
    yield fixture, state
  ensure
    %i[HOMEBREW_PREFIX HOMEBREW_CELLAR].each { |name| Object.send(:remove_const, name) }
  end
end

def check(condition, message)
  raise message unless condition
end

[:success, :same_abi].each do |scenario|
  with_fixture(scenario) do |fixture, state|
    fixture.run(Fixture::JXL, [Fixture::CONSUMER])
    check(fixture.versions("jpeg-xl") == [Fixture::NEW], "old JPEG XL was not retired")
    expected = scenario == :same_abi ? ["upgrade"] : %w[upgrade reinstall]
    check(fixture.builds == expected, "incorrect ABI rebuild selection")
    check(!fixture.present?(state/"jpeg-xl-active.json"), "successful transaction left a journal")
    check(state.glob("jpeg-xl-*/backup").empty?, "duplicate successful snapshots were kept")
  end
end

[:failed, :failed_retirement, :failed_missing_rack].each do |scenario|
  with_fixture(scenario) do |fixture, state|
    fixture.rack("consumer").chmod(0o755)
    begin
      fixture.run(Fixture::JXL, [Fixture::CONSUMER])
      raise "synthetic failure incorrectly succeeded"
    rescue RuntimeError => error
      check(error.message.start_with?("synthetic") && !error.message.include?("incomplete"), error.message)
    end
    check(fixture.versions("jpeg-xl") == [Fixture::OLD], "JPEG XL rollback failed")
    check(fixture.versions("consumer") == ["1.0"], "partial consumer keg remains")
    check((fixture.rack("consumer").stat.mode & 0o777) == 0o755, "restored rack permissions changed")
    check((fixture.opt("consumer")/"stamp").read == "initial", "consumer backup was not restored")
    check(!fixture.present?(fixture.rack("bison")), "build dependency was not removed on rollback")
    check(!fixture.present?(state/"jpeg-xl-active.json"), "verified rollback left a journal")
  end
end

with_fixture(:success) do |fixture, state|
  rows, tools = fixture.plan(Fixture::JXL, [Fixture::CONSUMER])
  fixture.prepare(rows, tools)
  fixture.mark(rows.first)
  fixture.brew("upgrade")
  Fixture.new(state).recover
  check(fixture.versions("jpeg-xl") == [Fixture::OLD], "restart recovery did not restore JPEG XL")
  check(!fixture.present?(state/"jpeg-xl-active.json"), "restart recovery left a journal")
end

with_fixture(:success) do |fixture, state|
  rows, tools = fixture.plan(Fixture::JXL, [Fixture::CONSUMER])
  fixture.prepare(rows, tools)
  fixture.mark(rows.first)
  fixture.brew("upgrade")
  journal = state/"jpeg-xl-active.json"
  record = JSON.parse(journal.read)
  record.fetch("rows").first["manifest"] = {}
  journal.write(JSON.generate(record))
  begin
    Fixture.new(state).recover
    raise "malformed rollback manifest was accepted"
  rescue RuntimeError => error
    check(error.message == "Invalid JPEG XL journal row", error.message)
  end
  check(fixture.versions("jpeg-xl") == [Fixture::OLD, Fixture::NEW], "malformed journal changed kegs")
  check(journal.file?, "malformed journal was deleted")
end

with_fixture(:zero_status_broken) do |fixture, _state|
  begin
    fixture.verify_linkage
    raise "zero-status linkage diagnostic was ignored"
  rescue RuntimeError => error
    check(error.message == "Homebrew reported broken linkage", error.message)
  end
end

with_fixture(:repair) do |fixture, state|
  provider = fixture.manifest(fixture.opt("jpeg-xl").realpath)
  rows, = fixture.repair_plan(Fixture::JXL, [Fixture::CONSUMER])
  check(rows.map { |row| row.fetch("name") } == %w[jpeg-xl consumer], "repair selected wrong consumer")
  check(fixture.builds.empty? && state.children.empty?, "repair preflight wrote state")

  2.times { fixture.repair(Fixture::JXL, [Fixture::CONSUMER]) }
  check(fixture.builds == ["reinstall"], "repair rebuilt a current consumer or provider")
  check(fixture.manifest(fixture.opt("jpeg-xl").realpath) == provider, "repair changed provider")
  check(fixture.versions("jpeg-xl") == [Fixture::OLD], "repair changed provider rack")
  check((fixture.opt("consumer")/"stamp").read == "replacement", "repair did not commit the consumer")
  check(!fixture.present?(state/"jpeg-xl-active.json"), "successful repair left a journal")
end

[:repair_failed, :repair_still_broken, :repair_restart].each do |scenario|
  with_fixture(scenario) do |fixture, state|
    provider = fixture.manifest(fixture.opt("jpeg-xl").realpath)
    original = fixture.manifest(fixture.opt("consumer").realpath)
    if scenario == :repair_restart
      rows, tools = fixture.repair_plan(Fixture::JXL, [Fixture::CONSUMER])
      fixture.prepare(rows, tools, repair: true)
      fixture.mark(rows.last)
      fixture.brew("reinstall")
      Fixture.new(state, :repair_restart).recover
    else
      begin
        fixture.repair(Fixture::JXL, [Fixture::CONSUMER])
        raise "synthetic failure incorrectly succeeded"
      rescue RuntimeError => error
        expected = scenario == :repair_failed ? "synthetic rebuild failure" : "does not link the active JPEG XL keg"
        check(error.message.include?(expected) && !error.message.include?("incomplete"), error.message)
      end
    end
    check(fixture.manifest(fixture.opt("jpeg-xl").realpath) == provider, "failed repair changed provider")
    check(fixture.manifest(fixture.opt("consumer").realpath) == original, "repair did not restore the pre-run consumer")
    check(fixture.versions("consumer") == ["1.0"], "repair left a partial consumer")
    check(!fixture.present?(state/"jpeg-xl-active.json"), "verified repair rollback left a journal")
  end
end

with_fixture(:repair_restart) do |fixture, state|
  rows, tools = fixture.repair_plan(Fixture::JXL, [Fixture::CONSUMER])
  fixture.prepare(rows, tools, repair: true)
  fixture.mark(rows.last)
  fixture.brew("reinstall")
  (fixture.opt("jpeg-xl")/"stamp").write("independent provider change")
  begin
    Fixture.new(state, :repair_restart).recover
    raise "provider drift was incorrectly accepted"
  rescue RuntimeError => error
    check(error.message == "Original keg changed before updating jpeg-xl", error.message)
  end
  check((fixture.opt("jpeg-xl")/"stamp").read == "independent provider change", "repair overwrote provider drift")
  check((state/"jpeg-xl-active.json").file?, "provider drift discarded recovery journal")
end

puts "JPEG XL transaction fixtures passed"

class PairFixture < GgmlLlamaRepair
  def preflight(_row)
    {}
  end

  def dependency_order(consumers)
    consumers
  end

  def direct_links(keg)
    path = keg/"links"
    path.file? ? path.readlines.map(&:chomp) : []
  end

  def run_command(*args)
    case args.first
    when "/usr/bin/ditto" then FileUtils.cp_r(args[1], args[2], preserve: true)
    when "/bin/sync" then nil
    else raise "Unexpected fixture command: #{args.inspect}"
    end
  end

  def reset_linkage_cache(_rows); end

  def install(short, links: [])
    path = rack(short)/"1.0"
    FileUtils.mkdir_p(path/"lib")
    (path/"INSTALL_RECEIPT.json").write(JSON.generate(
      "source" => { "tap" => "homebrew/core", "spec" => "stable" },
      "used_options" => [],
    ))
    (path/"links").write(links.join("\n"))
    Keg.new(path).link unless linked(short).symlink?
    path
  end

  def brew(*args)
    raise "Unexpected rebuild request" unless args == %w[reinstall --no-ask --build-from-source --formula whisper.cpp]

    install("whisper.cpp", links: [opt("ggml")/"lib/libggml.1.dylib", opt("llama.cpp")/"lib/libllama.1.dylib"])
    raise "Synthetic pair failure" if @fail
  end

  def fail_rebuild!
    @fail = true
  end
end

with_fixture(:success) do |_, state|
  pair = PairFixture.new(state)
  ggml = pair.install("ggml")
  llama = pair.install("llama.cpp")
  (ggml/"lib/libggml.1.dylib").write("synthetic ggml")
  (llama/"lib/libllama.1.dylib").write("synthetic llama")
  whisper = pair.install("whisper.cpp", links: [pair.opt("ggml")/"lib/libggml.0.dylib",
                                               pair.opt("llama.cpp")/"lib/libllama.0.dylib"])
  prior = pair.manifest(whisper)
  pair.fail_rebuild!
  begin
    pair.repair_pair(%w[ggml llama.cpp], ["whisper.cpp"])
    raise "Synthetic rebuild should fail"
  rescue RuntimeError => error
    check(error.message == "Synthetic pair failure", error.message)
  end
  check(pair.manifest(whisper) == prior, "Pair repair failed to restore the original consumer")
  check(!pair.present?(state/"ggml-llama-active.json"), "Pair repair left a pending journal")
  repaired = PairFixture.new(state)
  2.times { repaired.repair_pair(%w[ggml llama.cpp], ["whisper.cpp"]) }
  check(repaired.direct_links(whisper).all? { |link| File.file?(link) }, "Pair repair left a broken link")
  check(!repaired.present?(state/"ggml-llama-active.json"), "Successful pair repair left a pending journal")
end
puts "ggml/llama.cpp consumer repair fixtures passed"
