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
  DEPENDENCIES = {
    "bison" => ["3.8.2", []],
    "libpng" => ["2.0", ["--with-fixture"]],
    "webp" => ["1.0", []],
    "libtiff" => ["2.0", []],
  }.freeze
  Target = Struct.new(:name, :installed) do
    def full_name
      name
    end

    def tap
      "homebrew/core"
    end

    def pkg_version
      PkgVersion.parse(DEPENDENCIES.fetch(name).first)
    end

    def active_spec_sym
      :stable
    end

    def pinned?
      false
    end

    def any_version_installed?
      installed
    end

    def keg_only?
      name == "webp"
    end

    def deprecated_options
      []
    end

    def options
      Options.create(DEPENDENCIES.fetch(name).last)
    end
  end
  Dependency = Struct.new(:to_formula) do
    def options
      Options.new
    end

    def build?
      to_formula.name == "bison"
    end
  end

  attr_reader :builds

  def initialize(state, scenario = :success)
    super(state)
    @scenario = scenario
    @builds = []
  end

  def install(full, version, library, options: [], stamp: "initial", dependency: false, linked: true)
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
    keg = Keg.new(path)
    linked ? keg.link : keg.optlink
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

  def dependency_order(consumers)
    consumers
  end

  def dependency_fixture(short)
    Dependency.new(Target.new(short, rack(short).directory?))
  end

  def preflight(row)
    return {} unless row.fetch("name") == "jpeg-xl" && %i[failed failed_missing_rack].include?(@scenario)

    dep = dependency_fixture("bison")
    { "bison" => dependency_write(row, dep, dep.to_formula) }
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

  def brew(action, row)
    @builds << action
    case action
    when "upgrade"
      if %i[failed failed_missing_rack].include?(@scenario)
        dep = dependency_fixture("bison")
        admit_dependency(row, dep, dep.to_formula)
        if @scenario == :failed
          install("homebrew/core/bison", "3.8.2", "", dependency: true)
          verify_dependency(row, "bison")
        else
          rack("bison").mkdir
        end
      end
      mark(row)
      Keg.new(rack("jpeg-xl")/OLD).unlink
      abi = @scenario == :same_abi ? "0.13" : "0.14"
      install(JXL, NEW, "libjxl.#{abi}.dylib", stamp: "candidate")
    when "reinstall"
      mark(row)
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
    else raise "Unexpected Brew command: #{action.inspect}"
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

class DependencyFixture < Fixture
  def setup
    super
    install("homebrew/core/libpng", "1.0", "", options: ["--with-fixture"], dependency: true)
    install("homebrew/core/webp", "1.0", "", dependency: true, linked: false)
    install("homebrew/core/libtiff", "1.0", "", dependency: true) if @scenario == :undeclared_dependency
  end

  def preflight(row)
    names = case row.fetch("name")
    when "jpeg-xl" then %w[libpng webp]
    when "consumer" then opt("libpng").realpath.basename.to_s == "1.0" ? ["libpng"] : []
    else []
    end

    names.to_h do |name|
      dep = dependency_fixture(name)
      [name, dependency_write(row, dep, dep.to_formula)]
    end
  end

  def brew(action, row)
    if action == "upgrade"
      simulate_dependency(row, "libpng")
      simulate_dependency(row, "libtiff") if @scenario == :undeclared_dependency
      (opt("webp")/"stamp").write("independent edit") if @scenario == :dependency_drift
      simulate_dependency(row, "webp")
    elsif preflight(row).key?("libpng")
      simulate_dependency(row, "libpng")
    end
    super
  end

  def simulate_dependency(owner, short)
    dep = dependency_fixture(short)
    target = dep.to_formula
    admit_dependency(owner, dep, target)
    prior = opt(short).realpath
    keg = Keg.new(prior)
    linked = keg.linked?
    keg.unlink
    version, options = DEPENDENCIES.fetch(short)
    if prior.basename.to_s == version
      staged = rack(short)/"#{version}.tmp"
      prior.rename(staged)
    end
    install("homebrew/core/#{short}", version, "", options: options, stamp: "replacement", dependency: true,
            linked: linked)
    raise "synthetic dependency interruption" if @scenario == :dependency_interruption && staged

    FileUtils.remove_entry_secure(staged) if staged
    verify_dependency(owner, short)
  end
end

def with_fixture(scenario, fixture_class: Fixture)
  Dir.mktmpdir("studio-jxl-fixture") do |root|
    prefix = Pathname(root).realpath
    Object.const_set(:HOMEBREW_PREFIX, prefix)
    Object.const_set(:HOMEBREW_CELLAR, prefix/"Cellar")
    [HOMEBREW_CELLAR, prefix/"opt", prefix/"var/homebrew/linked"].each { |path| FileUtils.mkdir_p(path) }
    state = prefix/"state"
    state.mkdir(0o700)
    fixture = fixture_class.new(state, scenario)
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

with_fixture(:success, fixture_class: DependencyFixture) do |fixture, state|
  rows, tools = fixture.plan(Fixture::JXL, [Fixture::CONSUMER])
  check(rows.map { |row| row.fetch("name") } == %w[jpeg-xl consumer libpng webp], "dependency snapshots are missing")
  check(rows.first.fetch("dependencies").keys == %w[libpng webp], "provider dependency plan is incomplete")
  check(rows[1].fetch("dependencies") == rows.first.fetch("dependencies").slice("libpng"),
        "provider and consumer should share a single dependency snapshot")
  check(tools.empty? && state.children.empty?, "installed dependency planning wrote state")

  fixture.run(Fixture::JXL, [Fixture::CONSUMER])
  check(fixture.versions("jpeg-xl") == [Fixture::NEW], "provider upgrade did not commit")
  check(fixture.versions("libpng") == %w[1.0 2.0], "dependency upgrade discarded its previous keg")
  check(fixture.opt("libpng").realpath == fixture.rack("libpng")/"2.0", "new dependency keg was not activated")
  check(fixture.linked("libpng").realpath == fixture.opt("libpng").realpath, "dependency lost its linked status")
  check(fixture.versions("webp") == ["1.0"], "same-version rebuild left a temporary keg")
  check((fixture.opt("webp")/"stamp").read == "replacement", "same-version dependency was not rebuilt")
  check(!fixture.present?(fixture.linked("webp")), "previously unlinked dependency was linked")
  check(fixture.builds == %w[upgrade reinstall], "dependency upgrades bypassed the ABI consumer rebuild")
  check(!fixture.present?(state/"jpeg-xl-active.json"), "committed dependency upgrade left a journal")
end

[:dependency_interruption, :undeclared_dependency, :dependency_drift].each do |scenario|
  with_fixture(scenario, fixture_class: DependencyFixture) do |fixture, state|
    png = fixture.opt("libpng").realpath
    webp = fixture.opt("webp").realpath
    before_png = fixture.manifest(png)
    before_webp = fixture.manifest(webp)
    if scenario == :undeclared_dependency
      tiff = fixture.opt("libtiff").realpath
      before_tiff = fixture.manifest(tiff)
    end
    error = nil
    begin
      fixture.run(Fixture::JXL, [Fixture::CONSUMER])
    rescue RuntimeError => problem
      error = problem.message
    end
    expected = {
      dependency_interruption: "synthetic dependency interruption",
      undeclared_dependency:    "libtiff",
      dependency_drift:        "webp",
    }.fetch(scenario)
    check(error&.include?(expected), error || "unexpected dependency operation succeeded")
    check(fixture.versions("jpeg-xl") == [Fixture::OLD], "dependency failure changed the provider")
    check(fixture.versions("libpng") == ["1.0"] && fixture.manifest(png) == before_png,
          "previous dependency upgrade was not rolled back")
    check(fixture.opt("libpng").realpath == png && fixture.linked("libpng").realpath == png,
          "dependency rollback changed the original link targets")
    check(fixture.versions("webp") == ["1.0"], "dependency rollback left a temporary keg")
    check(fixture.opt("webp").realpath == webp && !fixture.present?(fixture.linked("webp")),
          "dependency rollback changed unlinked installation")
    if scenario == :dependency_drift
      check((webp/"stamp").read == "independent edit", "dependency recovery overwrote independent change")
    else
      check(fixture.manifest(webp) == before_webp, "failed dependency change did not restore the original keg")
    end
    if scenario == :undeclared_dependency
      check(fixture.versions("libtiff") == ["1.0"] && fixture.manifest(tiff) == before_tiff,
            "unplanned dependency reached the package mutation")
    end
    check(!fixture.present?(state/"jpeg-xl-active.json"), "verified dependency rollback left a journal")
  end
end

with_fixture(:success) do |fixture, state|
  rows, tools = fixture.plan(Fixture::JXL, [Fixture::CONSUMER])
  fixture.prepare(rows, tools)
  fixture.brew("upgrade", rows.first)
  Fixture.new(state).recover
  check(fixture.versions("jpeg-xl") == [Fixture::OLD], "restart recovery did not restore JPEG XL")
  check(!fixture.present?(state/"jpeg-xl-active.json"), "restart recovery left a journal")
end

with_fixture(:success) do |fixture, state|
  rows, tools = fixture.plan(Fixture::JXL, [Fixture::CONSUMER])
  fixture.prepare(rows, tools)
  fixture.brew("upgrade", rows.first)
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

with_fixture(:success) do |fixture, state|
  rows, tools = fixture.plan(Fixture::JXL, [Fixture::CONSUMER])
  fixture.prepare(rows, tools)
  fixture.brew("upgrade", rows.first)
  prior = fixture.original(rows.first)
  prior.rename(state/"interrupted-original")
  snapshot = fixture.saved(rows.first)/"stamp"
  original = snapshot.read
  snapshot.write("damaged backup")
  journal = state/"jpeg-xl-active.json"
  journal_before = journal.read

  begin
    Fixture.new(state).recover
    raise "Damaged rollback backup was accepted"
  rescue RuntimeError => error
    check(error.message.include?("rollback backup drifted"), error.message)
  end
  check(journal.read == journal_before, "failed rollback rewrote or discarded its journal")
  check(fixture.opt("jpeg-xl").realpath == fixture.rack("jpeg-xl")/Fixture::NEW, "failed rollback changed active keg")
  check(snapshot.read == "damaged backup", "failed rollback discarded the backup evidence")

  snapshot.write(original)
  Fixture.new(state).recover
  check(fixture.versions("jpeg-xl") == [Fixture::OLD], "recovery retry did not restore JPEG XL")
  check(fixture.manifest(prior) == fixture.manifest(state/"interrupted-original"), "retry did not restore backup bytes")
  check(!fixture.present?(journal), "verified retry left a journal")
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
      fixture.brew("reinstall", rows.last)
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
  fixture.brew("reinstall", rows.last)
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

  def brew(action, row)
    raise "Unexpected rebuild request" unless action == "reinstall" && row.fetch("full") == "whisper.cpp"

    mark(row)
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
