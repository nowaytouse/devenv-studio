# Invoked only through the Studio engine's controlled Homebrew worker.
# Homebrew's installer owns bottle checks and builds. Studio owns the scope,
# reviewed inputs, link restoration and post-install verification.
require "digest"
require "formula_installer"
require "json"
require "open3"
require "tmpdir"

LLVM_REMOTE = "https://github.com/llvm/llvm-project.git".freeze
LLVM_REF = "refs/heads/main".freeze
LLVM_HEAD_FORMULA = "nyamiiko/tools/llvm-head".freeze
LLVM_STABLE_VERSION = /\A23\.1\.\d+(?:_\d+)?\z/
LLVM_SHA = /\A[0-9a-f]{40}\z/

class StudioLlvmInstaller < FormulaInstaller
  def hold_locks
    lock
  end

  # Homebrew normally releases its locks at the end of finish. Link and Rust
  # verification, and any restoration, must happen under the same locks.
  def unlock
  end

  def release_locks
    FormulaInstaller.instance_method(:unlock).bind_call(self)
  end

  def install_dependencies(deps)
    return if deps.empty?

    raise "LLVM would change additional formulae: #{deps.map(&:name).join(", ")}"
  end

  def install_dependency(dep, _dep_formula = nil)
    raise "LLVM would change an additional formula: #{dep.name}"
  end

  # The default optlink also rewrites aliases and former names. This operation
  # owns only the explicit channel path reviewed in Studio.
  def link(keg)
    llvm_replace_link(formula.opt_prefix.to_s, keg.path.to_s)
  end
end

def llvm_capture(*command, env: {})
  stdout, stderr, status = Open3.capture3(env, *command, chdir: "/")
  raise "#{File.basename(command.first)} failed: #{stderr.strip}" unless status.success?

  stdout.strip
end

def llvm_path(path, type)
  stat = File.lstat(path)
  mask = type == :directory? ? 0o002 : 0o022
  unless stat.public_send(type) && File.realpath(path) == path.to_s && stat.uid == Process.uid &&
         (stat.mode & mask).zero?
    raise "Unsafe LLVM input: #{path}"
  end

  path.to_s
end

def llvm_link(path, rack)
  raise "Expected a Homebrew symlink at #{path}" unless File.symlink?(path)

  link = File.readlink(path)
  target = llvm_path(File.realpath(path), :directory?)
  raise "Unexpected LLVM keg target at #{path}" unless File.dirname(target) == rack.to_s
  raise "LLVM link changed while reading #{path}" unless File.readlink(path) == link

  {path: path.to_s, link:, target:}
end

def llvm_receipt(keg, tap, spec)
  path = llvm_path(File.join(keg, "INSTALL_RECEIPT.json"), :file?)
  data = JSON.parse(File.binread(path))
  source = data.fetch("source")
  unless source["tap"] == tap && source["spec"] == spec
    raise "Unexpected installation origin at #{keg}"
  end
  if spec == "stable" && data["poured_from_bottle"] != true
    raise "Expected an official Homebrew LLVM bottle at #{keg}"
  end

  {sha256: Digest::SHA256.file(path).hexdigest, revision: source["scm_revision"]}
end

def llvm_formula_digest(formula)
  Digest::SHA256.file(llvm_path(formula.path, :file?)).hexdigest
end

def llvm_history(formula)
  Dir.children(formula.rack).grep(/\AHEAD-/).sort
end

def llvm_upstream
  value = llvm_capture(
    "/usr/bin/git", "ls-remote", "--exit-code", "--refs", LLVM_REMOTE, LLVM_REF,
    env: {"GIT_CONFIG_NOSYSTEM" => "1", "GIT_CONFIG_GLOBAL" => "/dev/null", "GIT_TERMINAL_PROMPT" => "0"}
  )
  sha, ref = value.split("\t", -1)
  raise "Invalid LLVM GitHub revision" unless ref == LLVM_REF && LLVM_SHA.match?(sha)

  sha
end

def llvm_snapshot(step)
  unless HOMEBREW_PREFIX.to_s == "/opt/homebrew" && HOMEBREW_CELLAR.to_s == "/opt/homebrew/Cellar" &&
         Hardware::CPU.arm?
    raise "Studio LLVM updates require Apple Silicon Homebrew in /opt/homebrew"
  end
  stable = Formulary.factory("homebrew/core/llvm", :stable)
  head = Formulary.factory(LLVM_HEAD_FORMULA, :head)
  unless stable.tap&.core_tap? && stable.keg_only? && !stable.fetch_defined? && head.full_name == LLVM_HEAD_FORMULA &&
         head.keg_only? && head.head? && head.stable.nil? && head.head.url == LLVM_REMOTE &&
         head.head.specs == {branch: "main"} && !head.fetch_defined? && !head.network_access_allowed?(:build)
    raise "Unexpected LLVM formula origin or build policy"
  end
  [stable.rack, head.rack, stable.opt_prefix.dirname, FormulaPin.new(stable).path.dirname].each do |path|
    llvm_path(path, :directory?)
  end
  opt = llvm_link(stable.opt_prefix, stable.rack)
  pin = llvm_link(FormulaPin.new(stable).path, stable.rack)
  installed = File.basename(opt.fetch(:target))
  unless opt[:target] == pin[:target] && LLVM_STABLE_VERSION.match?(installed)
    raise "Active Homebrew LLVM and its pin must select the same LLVM 23.1 keg"
  end
  head_opt = llvm_link(head.opt_prefix, head.rack)
  head_receipt = llvm_receipt(head_opt.fetch(:target), "nyamiiko/tools", "head")
  unless LLVM_SHA.match?(head_receipt[:revision].to_s)
    raise "Installed LLVM HEAD lacks a full Git revision"
  end

  current = {
    schema_version: 1, step:, opt:, pin:, head_opt:, head_receipt:,
    stable_receipt: llvm_receipt(opt.fetch(:target), "homebrew/core", "stable"),
    old_head_kegs: llvm_history(stable), head_kegs: llvm_history(head),
  }
  if step == "llvm_stable"
    version = stable.pkg_version.to_s
    unless LLVM_STABLE_VERSION.match?(version) && PkgVersion.parse(version) >= PkgVersion.parse(installed)
      raise "Official LLVM #{version} is outside the installed LLVM 23.1 upgrade range"
    end
    rust_opt = llvm_link(HOMEBREW_PREFIX/"opt/rust", HOMEBREW_CELLAR/"rust")
    bottle = FormulaInstaller.new(stable, force_bottle: true).send(:selected_bottle)
    unless bottle && stable.bottle_tag? && stable.pour_bottle? &&
           bottle.url.match?(%r{\Ahttps://ghcr\.io/v2/homebrew/core/llvm/blobs/sha256:[0-9a-f]{64}\z})
      raise "No compatible official LLVM #{version} bottle"
    end
    current.merge!(formula_sha256: llvm_formula_digest(stable), version:, rust_opt:, bottle: bottle.url, tag: bottle.tag.to_s)
    detail = "Official Homebrew LLVM #{installed} → #{version} (#{bottle.tag} bottle). " \
             "Verify Homebrew Rust and retain the previous LLVM keg."
    [stable, current, detail]
  else
    raise "The separate LLVM HEAD formula is pinned" if FormulaPin.new(head).pinned?

    revision = llvm_upstream
    current.merge!(formula_sha256: llvm_formula_digest(head), revision:, formula_revision: head.revision)
    detail = "#{LLVM_HEAD_FORMULA} #{head_receipt.fetch(:revision)} → #{revision}. " \
             "Build the official LLVM main revision in the separate keg."
    [head, current, detail]
  end
end

def llvm_unchanged(link)
  File.symlink?(link[:path]) && File.readlink(link[:path]) == link[:link] &&
    File.realpath(link[:path]) == link[:target]
end

def llvm_replace_link(path, target)
  raise "Refusing to replace a non-symlink at #{path}" if File.exist?(path) && !File.symlink?(path)

  parent = File.dirname(path)
  Dir.mktmpdir(".studio-llvm-", parent) do |directory|
    temporary = File.join(directory, "link")
    File.symlink(target, temporary)
    File.rename(temporary, path)
  end
  File.open(parent, &:fsync)
end

def llvm_restore(link, candidate)
  return if llvm_unchanged(link)

  unless File.symlink?(link[:path]) && File.readlink(link[:path]) == candidate
    raise "Unrecognized change at #{link[:path]}; preserved for manual review"
  end
  llvm_path(link[:target], :directory?)
  llvm_replace_link(link[:path], link[:link])
  raise "Could not verify restored link at #{link[:path]}" unless llvm_unchanged(link)
end

def llvm_verify_stable(keg, version, rust)
  installed = llvm_capture(File.join(keg, "bin/llvm-config"), "--version")
  raise "Unexpected installed LLVM version #{installed}" unless installed == version.split("_", 2).first

  raise "Homebrew Rust changed during the LLVM update" unless llvm_unchanged(rust)

  llvm_capture(File.join(rust[:target], "bin/rustc"), "--version")
  llvm_capture(File.join(rust[:target], "bin/cargo"), "--version")
end

def llvm_write_journal(path, snapshot)
  File.open(path, File::WRONLY | File::CREAT | File::EXCL, 0o600) do |file|
    file.write(JSON.pretty_generate(snapshot))
    file.flush
    file.fsync
  end
  File.open(File.dirname(path), &:fsync)
end

def llvm_run(formula, snapshot, journal)
  stable = snapshot[:step] == "llvm_stable"
  active = stable ? snapshot[:opt] : snapshot[:head_opt]
  version = stable ? snapshot[:version] : snapshot[:revision]
  if stable ? File.basename(active[:target]) == version : snapshot[:head_receipt][:revision] == version
    llvm_verify_stable(active[:target], version, snapshot.fetch(:rust_opt)) if stable
    puts "LLVM is already current at #{version}."
    return
  end

  installer = StudioLlvmInstaller.new(formula, force_bottle: stable, link_keg: false,
                                               installed_on_request: true, show_header: true, verbose: true)
  installer.hold_locks
  journal_written = false
  candidate_path = nil
  begin
    protected_links = snapshot.values_at(:opt, :pin, :head_opt, :rust_opt).compact
    unless protected_links.all? { |link| llvm_unchanged(link) } && llvm_formula_digest(formula) == snapshot[:formula_sha256]
      raise "LLVM formula or active keg changed while acquiring Homebrew locks"
    end
    if stable && !installer.pour_bottle?
      raise "LLVM stable requires an official bottle"
    end
    if stable
      bottle = installer.send(:selected_bottle)
      unless bottle&.url == snapshot[:bottle] && bottle.compatible_locations?
        raise "Cannot verify a compatible official LLVM bottle for the reviewed formula"
      end
    end
    installer.prelude
    missing = installer.compute_dependencies(use_cache: false)
    raise "LLVM requires other Homebrew changes: #{missing.map(&:name).join(", ")}" unless missing.empty?

    if stable && File.exist?(formula.prefix)
      raise "Candidate LLVM keg already exists at #{formula.prefix}; retaining it"
    end
    installer.fetch
    unless stable
      cache = formula.head.downloader.cached_location
      fetched = llvm_capture("/usr/bin/git", "-C", cache.to_s, "rev-parse", "--verify", "HEAD")
      raise "LLVM main changed during fetch. Review the new revision first." unless fetched == version

      formula.update_head_version
    end
    raise "LLVM candidate already exists at #{formula.prefix}; retaining it" if File.exist?(formula.prefix)
    candidate_path = formula.prefix.to_s
    raise "LLVM formula changed during fetch" unless llvm_formula_digest(formula) == snapshot[:formula_sha256]
    llvm_write_journal(journal, snapshot)
    journal_written = true
    installer.install
    installer.finish
    candidate = llvm_link(formula.opt_prefix, formula.rack)
    unless candidate[:target] == formula.prefix.to_s && candidate[:target] != active[:target]
      raise "Homebrew did not activate the expected LLVM keg"
    end
    if stable
      llvm_receipt(candidate[:target], "homebrew/core", "stable")
      llvm_verify_stable(candidate[:target], version, snapshot.fetch(:rust_opt))
      raise "Separate LLVM HEAD changed during the stable update" unless llvm_unchanged(snapshot[:head_opt])
      raise "LLVM pin changed during the stable update" unless llvm_unchanged(snapshot[:pin])

      llvm_replace_link(snapshot[:pin][:path], candidate[:target])
      raise "New LLVM pin readback failed" unless File.realpath(snapshot[:pin][:path]) == candidate[:target]
    else
      receipt = llvm_receipt(candidate[:target], "nyamiiko/tools", "head")
      raise "Installed LLVM HEAD differs from the reviewed commit" unless receipt[:revision] == version
      unless llvm_unchanged(snapshot[:opt]) && llvm_unchanged(snapshot[:pin])
        raise "Stable LLVM changed during the HEAD update"
      end
      llvm_capture(File.join(candidate[:target], "bin/llvm-config"), "--version")
    end
    raise "Previous LLVM keg disappeared" unless File.directory?(active[:target])
    snapshot.fetch(stable ? :old_head_kegs : :head_kegs).each do |name|
      raise "An earlier LLVM HEAD keg disappeared: #{name}" unless File.directory?(formula.rack/name)
    end
    llvm_write_journal(journal + ".done", {verified: candidate_path})
    puts "Verified LLVM #{version} at #{candidate[:target]}."
  rescue Exception => error
    if journal_written
      links = stable ? [snapshot[:opt], snapshot[:pin]] : [snapshot[:head_opt]]
      failures = links.filter_map do |link|
        llvm_restore(link, candidate_path)
        nil
      rescue StandardError => restore_error
        restore_error.message
      end
      unless failures.empty?
        raise "#{error.message}\nLLVM restoration incomplete. Previous links saved at #{journal}. #{failures.join("; ")}"
      end
      llvm_write_journal(journal + ".done", {restored: true})
      warn "Restored the previous LLVM links. See #{journal}."
    end
    raise
  ensure
    installer.release_locks
  end
end

$stdout.sync = true
$stderr.sync = true
action, step, approved_digest, journal = ARGV
unless %w[llvm_stable llvm_head].include?(step)
  abort "Invalid LLVM update channel"
end
begin
  formula, snapshot, detail = llvm_snapshot(step)
  digest = Digest::SHA256.hexdigest(JSON.generate(snapshot))
  case action
  when "plan"
    puts JSON.generate(digest:, detail:)
  when "run"
    abort "LLVM review is stale. Review the current version and source again." unless digest == approved_digest
    llvm_run(formula, snapshot, journal)
  else
    abort "Invalid LLVM operation"
  end
rescue StandardError => error
  abort "LLVM #{step.delete_prefix("llvm_")}: #{error.message}"
end
