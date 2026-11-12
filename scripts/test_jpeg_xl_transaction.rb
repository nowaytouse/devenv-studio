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
    install(JXL, OLD, "libjxl.0.13.dylib")
    install(CONSUMER, "1.0", "libjxl.0.13.dylib", options: ["--with-jxl"])
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
    when "reinstall"
      old = rack("consumer")/"1.0"
      Keg.new(old).unlink
      staged = rack("consumer")/"1.0.reinstall"
      old.rename(staged)
      install(CONSUMER, "1.0", "libjxl.0.14.dylib", options: ["--with-jxl"], stamp: "replacement")
      raise "synthetic rebuild failure" if @scenario == :failed

      FileUtils.remove_entry_secure(staged)
    else raise "Unexpected Brew command: #{args.inspect}"
    end
  end

  def reset_linkage_cache(_rows); end

  def smoke(_jpeg, _consumers); end

  def capture(*)
    return "Missing libraries:\n  test.dylib\n" if @scenario == :zero_status_broken
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

[:failed, :failed_retirement].each do |scenario|
  with_fixture(scenario) do |fixture, state|
    begin
      fixture.run(Fixture::JXL, [Fixture::CONSUMER])
      raise "synthetic failure incorrectly succeeded"
    rescue RuntimeError => error
      check(error.message.start_with?("synthetic") && !error.message.include?("incomplete"), error.message)
    end
    check(fixture.versions("jpeg-xl") == [Fixture::OLD], "JPEG XL rollback failed")
    check(fixture.versions("consumer") == ["1.0"], "partial consumer keg remains")
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

with_fixture(:zero_status_broken) do |fixture, _state|
  begin
    fixture.verify_linkage
    raise "zero-status linkage diagnostic was ignored"
  rescue RuntimeError => error
    check(error.message == "Homebrew reported broken linkage", error.message)
  end
end

puts "JPEG XL transaction fixtures passed"
