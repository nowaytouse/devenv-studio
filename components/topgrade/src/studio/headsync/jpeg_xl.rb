# frozen_string_literal: true

require "digest"
require "fileutils"
require "json"
require "keg"
require "formula_installer"
require "cache_store"
require "linkage_cache_store"
require "open3"
require "reinstall"
require "pathname"
require "securerandom"

class JpegXlTransaction
  NAME = /\A[a-zA-Z0-9][a-zA-Z0-9_.+@-]*\z/
  LIBRARY = /\Alibjxl(?:_[a-z0-9_]+)?(?:\.[0-9]+)*\.dylib\z/
  OPTIONS = /\A--(?:with|without)-[a-zA-Z0-9_.+@-]+\z/
  BUILD_TOOLS = %w[bison util-macros xtrans lzip make].freeze
  MAX_JOURNAL_BYTES = 32 * 1024 * 1024
  BREW_ENV = {
    "HOMEBREW_NO_AUTO_UPDATE" => "1",
    "HOMEBREW_NO_INSTALL_FROM_API" => "1",
    "HOMEBREW_NO_INSTALL_CLEANUP" => "1",
    "HOMEBREW_NO_INSTALLED_DEPENDENTS_CHECK" => "1",
    "HOMEBREW_NO_AUTOREMOVE" => "1",
    "HOMEBREW_NO_ASK" => "1",
    "LANG" => "C",
    "LC_ALL" => "C",
  }.freeze

  def initialize(state)
    @state = Pathname(state)
    private_directory(@state)
    @journal = @state/"#{transaction_name}-active.json"
  end

  def transaction_name
    "jpeg-xl"
  end

  def providers
    ["jpeg-xl"]
  end

  def provider_upgrades?
    providers == ["jpeg-xl"]
  end

  def protected?(short)
    %w[llvm llvm-head rust topgrade ggml llama.cpp].include?(short) || short.start_with?("llvm@")
  end

  def private_directory(path)
    info = path.lstat
    raise "Unsafe transaction directory: #{path}" unless path.absolute? && path.realpath == path &&
      info.directory? && info.uid == Process.uid && (info.mode & 0o077).zero?
  end

  def sync_directory(path)
    path.open(&:fsync)
  end

  def interrupted!
    raise "#{linkage_group} upgrade interrupted by #{@signal}" if @signal && !@recovering
  end

  def run_command(*args)
    interrupted!
    status = system(BREW_ENV, *args.map(&:to_s))
    interrupted!
    raise "Failed: #{args.join(' ')}" unless status
  end

  def brew(action, owner)
    run_command(HOMEBREW_BREW_FILE, "ruby", __FILE__, "#{transaction_name}-install", @state, owner.fetch("name"), action)
    load_record
  end

  def run_install(owner_name, action)
    require "install"
    require "upgrade"

    load_record
    owner = @record.fetch("rows").find { |row| row.fetch("name") == owner_name }
    raise "Invalid Homebrew transaction installer" unless @record.key?("touched_tools") && owner&.key?("dependencies") &&
      !@record.fetch("touched").include?(owner_name) && !@record.fetch("committed") &&
      action == (providers.include?(owner_name) ? "upgrade" : "reinstall") &&
      (!providers.include?(owner_name) || !@record.fetch("repair"))

    Homebrew.running_command = action
    verify_original(owner)
    context = install_context(owner)
    installer = context.formula_installer
    formula = context.formula
    if action == "upgrade"
      raise "Homebrew would unlink a keg outside this transaction" unless
        (Homebrew::Upgrade.outdated_kegs(formula).map(&:to_path) - [original(owner).to_s]).empty?
      raise "No current HEAD update for #{owner.fetch('full')}" unless formula.outdated?(fetch_head: true)
    end

    verify_dependency_plan(owner, dependency_writes(owner, installer))
    transaction = self
    # Homebrew recomputes dependencies and can install them from nested installers.
    FormulaInstaller.prepend(Module.new do
      define_method(:install_dependency) do |dep, target = dep.to_formula|
        transaction.admit_dependency(owner, dep, target)
        super(dep, target)
        transaction.verify_dependency(owner, target.name)
      end
    end)

    Homebrew::Unlink.singleton_class.prepend(Module.new do
      def unlink(keg, **)
        raise "Unjournaled Homebrew unlink required for #{keg.name}"
      end
    end)

    Homebrew::Install.perform_preinstall_checks_once
    valid = Homebrew::Install.fetch_formulae([installer])
    raise "Homebrew could not prepare #{owner.fetch('full')}" unless valid.include?(installer) && !Homebrew.failed?

    mark(owner)
    if action == "upgrade"
      Homebrew::Install.install_formula(installer, upgrade: true)
    else
      Homebrew::Reinstall.reinstall_formula(context)
    end
    raise "Homebrew installation failed for #{owner.fetch('full')}" if Homebrew.failed?
  end

  def capture(*args, quiet: false)
    interrupted!
    output, status = Open3.capture2e(BREW_ENV, *args.map(&:to_s))
    interrupted!
    puts output unless output.empty? || (quiet && status.success?)
    raise "Failed: #{args.join(' ')}" unless status.success?

    output
  end

  def verify_linkage
    output = capture(HOMEBREW_BREW_FILE, "linkage", "--test")
    raise "Homebrew reported broken linkage" if output.match?(/^\s*(?:Missing libraries|Broken dependencies):/)
  end

  def name(full)
    parts = full.split("/", -1)
    raise "Unexpected formula name: #{full}" unless [1, 3].include?(parts.length) &&
      parts.all? { |part| NAME.match?(part) }

    parts.last
  end

  def rack(name)
    HOMEBREW_CELLAR/name
  end

  def opt(name)
    HOMEBREW_PREFIX/"opt"/name
  end

  def linked(name)
    HOMEBREW_PREFIX/"var/homebrew/linked"/name
  end

  def present?(path)
    path.exist? || path.symlink?
  end

  def record_target(path, name)
    return unless present?(path)

    raise "Expected a Homebrew symlink at #{path}" unless path.symlink?

    target = path.readlink.expand_path(path.dirname)
    raise "Unexpected keg path: #{target}" unless target.dirname == rack(name) && NAME.match?(target.basename.to_s)

    target
  end

  def keg_directory(path)
    raise "Unsafe keg directory: #{path}" unless path&.directory? && path.realpath == path

    path
  end

  def versions(name)
    path = rack(name)
    return [] unless present?(path)

    keg_directory(path).children.map do |child|
      raise "Unexpected keg entry: #{child}" unless NAME.match?(child.basename.to_s)

      keg_directory(child).basename.to_s
    end.sort
  end

  def receipt(keg)
    file = keg/"INSTALL_RECEIPT.json"
    raise "Invalid keg receipt: #{file}" unless file.file? && !file.symlink? && file.size <= 4 * 1024 * 1024

    JSON.parse(file.read)
  end

  def source_info(tab)
    source = tab.fetch("source")
    options = tab.fetch("used_options").sort
    raise "Unsupported keg source or build options" unless %w[stable head].include?(source["spec"]) &&
      options.all? { |option| OPTIONS.match?(option) }

    [source.fetch("tap"), source.fetch("spec"), options]
  end

  def resolve(row)
    formula = Formulary.resolve(row.fetch("full")).latest_formula
    tap, spec, = row.fetch("source")
    raise "Homebrew source changed for #{row.fetch('full')}" unless formula.name == row.fetch("name") &&
      formula.tap.to_s == tap && formula.active_spec_sym.to_s == spec && !formula.pinned?

    formula
  end

  def active_row(full)
    short = name(full)
    active = keg_directory(record_target(opt(short), short))
    linked_target = record_target(linked(short), short)
    raise "Inconsistent linked keg for #{full}" if linked_target && linked_target != active

    installed = versions(short)
    raise "An unfinished Homebrew reinstall exists for #{full}" if installed.any? { |v| v.end_with?(".reinstall") }

    tab = receipt(active)
    row = {
      "full" => full, "name" => short, "version" => active.basename.to_s, "versions" => installed,
      "linked" => !linked_target.nil?, "source" => source_info(tab),
      "rack_mode" => rack(short).stat.mode & 0o777,
    }
    [row, active]
  end

  def dylib_links(keg)
    Keg.new(keg).binary_executable_or_library_files.flat_map do |file|
      next [] unless file.arch_compatible?(Hardware::CPU.arch)

      file.dynamically_linked_libraries(except: :DYLIB_USE_WEAK_LINK)
    end.uniq
  end

  def direct_links(keg)
    dylib_links(keg).grep(%r{\A(?:#{Regexp.escape(opt("jpeg-xl").to_s)}/|#{Regexp.escape(rack("jpeg-xl").to_s)}/|@rpath/|@loader_path/|@executable_path/)}).select do |link|
      LIBRARY.match?(File.basename(link))
    end
  end

  def link_satisfied?(link, jpeg)
    return false if link.start_with?("@")

    file = Pathname(link)
    file.file? && file.realpath.to_s.start_with?("#{jpeg}/")
  end

  def install_context(row)
    formula = resolve(row)
    options = row.fetch("source").last
    context = Homebrew::Reinstall.build_install_context(
      formula, flags: options, build_from_source_formulae: [formula.full_name],
    )
    raise "Homebrew would drop build options for #{row.fetch('full')}" unless context.options.as_flags.sort == options

    context
  end

  def dependency_write(owner, dep, target)
    short = target.name
    full = target.full_name
    version = target.pkg_version.to_s
    tap = target.tap.to_s
    error = "Unjournaled Homebrew dependency write required for #{owner.fetch('full')}: #{full}"
    raise error unless name(full) == short && NAME.match?(version) && !tap.empty? &&
      target.active_spec_sym == :stable && !target.pinned? && !protected?(short)

    options = Tab.remap_deprecated_options(target.deprecated_options, dep.options)
    if target.any_version_installed?
      row, = active_row(full)
      previous = row.fetch("version")
      installed = row.fetch("versions")
      saved_options = row.fetch("source").last
      raise error unless row.fetch("source").take(2) == [tap, "stable"] &&
        target.pkg_version >= PkgVersion.parse(previous) &&
        (version == previous || !installed.include?(version)) &&
        !installed.include?("#{version}.tmp") && (row.fetch("linked") || target.keg_only?)

      options |= Options.create(saved_options) if row.fetch("linked") || version == previous
      raise error unless (options & target.options).as_flags.sort == saved_options
    else
      raise error unless dep.build? && tap == "homebrew/core" && BUILD_TOOLS.include?(short) &&
        !present?(rack(short)) && !present?(opt(short)) && !present?(linked(short)) &&
        (options & target.options).empty?

      saved_options = []
    end
    { "full" => full, "source" => [tap, "stable", saved_options], "version" => version }
  end

  def preflight(row)
    dependency_writes(row, install_context(row).formula_installer)
  end

  def dependency_writes(row, installer)
    writes = {}
    pending = installer.compute_dependencies.dup
    until pending.empty?
      dep = pending.shift
      target = dep.to_formula
      short = target.name
      write = dependency_write(row, dep, target)
      if writes.key?(short)
        raise "Conflicting Homebrew dependency targets for #{short}" unless writes.fetch(short) == write

        next
      end

      writes[short] = write
      # A dependency's bottle receipt can add minimum version requirements of its own.
      dependency_installer = FormulaInstaller.new(
        target, options: Options.create(write.fetch("source").last),
        build_from_source_formulae: [installer.formula.full_name],
      )
      pending.concat(dependency_installer.compute_dependencies)
    end
    writes
  end

  def plan_dependencies(rows, owners)
    writes = {}
    owners.each do |owner|
      owner["dependencies"] = preflight(owner)
      owner.fetch("dependencies").each do |short, write|
        if (writes.key?(short) && writes.fetch(short) != write) ||
            rows.any? { |row| row.fetch("name") == short }
          raise "Conflicting Homebrew dependency write for #{short}"
        end

        writes[short] = write
      end
    end

    tools = {}
    writes.each do |short, write|
      if present?(rack(short))
        row, = active_row(write.fetch("full"))
        raise "Homebrew dependency source changed for #{short}" unless row.fetch("source") == write.fetch("source")

        rows << row.merge("replacement" => write.fetch("version"))
      else
        raise "Homebrew dependency disappeared while planning: #{short}" unless
          BUILD_TOOLS.include?(short) && write.fetch("source") == ["homebrew/core", "stable", []] &&
          !present?(opt(short)) && !present?(linked(short))

        tools[short] = write.fetch("version")
      end
    end
    puts "Planned Homebrew dependency writes: #{writes.keys.sort.join(', ')}" unless writes.empty?
    tools
  end

  def dependency_group(full, users)
    jpeg, = active_row(full)
    raise "Expected the JPEG XL formula" unless jpeg.fetch("name") == "jpeg-xl"

    names = users.map { |user| name(user) }
    raise "Ambiguous JPEG XL dependency group" if names.uniq != names || names.include?("jpeg-xl")

    consumers = users.filter_map do |user|
      row, keg = active_row(user)
      links = direct_links(keg)
      if links.any? && protected?(row.fetch("name"))
        raise "Protected formula links JPEG XL: #{user}"
      end
      raise "Ambiguous JPEG XL load path in #{user}" if links.any? { |link| link.start_with?("@") }

      row.merge("links" => links) unless links.empty?
    end
    [jpeg, *consumers]
  end

  def plan(full, users)
    rows = dependency_group(full, users)
    jpeg, *consumers = rows
    puts "JPEG XL transaction: #{jpeg.fetch('full')}; direct consumers: #{consumers.map { |r| r.fetch('full') }.join(', ')}"
    [rows, plan_dependencies(rows, rows.dup)]
  end

  def repair_plan(full, users)
    jpeg, *consumers = dependency_group(full, users)
    current = original(jpeg)
    broken = consumers.reject { |row| row.fetch("links").all? { |link| link_satisfied?(link, current) } }
    puts "JPEG XL linkage repair: #{broken.map { |r| r.fetch('full') }.join(', ')}" unless broken.empty?

    rows = [jpeg, *broken]
    [rows, plan_dependencies(rows, broken)]
  end

  def manifest(root)
    keg_directory(root).find.to_h do |path|
      info = path.lstat
      relative = path.relative_path_from(root).to_s
      detail = if info.symlink?
        ["symlink", path.readlink.to_s]
      elsif info.directory?
        ["directory", info.mode & 0o7777]
      elsif info.file?
        ["file", info.mode & 0o7777, Digest::SHA256.file(path).hexdigest]
      else
        raise "Unsupported keg entry: #{path}"
      end
      [relative, detail]
    end
  end

  def saved(row)
    @stage/"backup"/row.fetch("name")
  end

  def original(row)
    rack(row.fetch("name"))/row.fetch("version")
  end

  def archive(path)
    folder = @stage/"archive"/path.dirname.basename
    FileUtils.mkdir_p(folder)
    destination = folder/"#{path.basename}-#{SecureRandom.hex(6)}"
    path.rename(destination)
    sync_directory(path.dirname)
    sync_directory(folder)
  end

  def checkpoint
    data = JSON.generate(@record)
    raise "JPEG XL journal is too large" if data.bytesize > MAX_JOURNAL_BYTES

    temp = @state/".#{transaction_name}-journal-#{SecureRandom.hex(12)}"
    temp.open("wx", 0o600) do |file|
      file.write(data)
      file.fsync
    end
    temp.rename(@journal)
    sync_directory(@state)
  ensure
    temp.unlink if temp&.exist?
  end

  def prepare(rows, tools, repair: false)
    raise "An unfinished JPEG XL transaction exists" if present?(@journal)
    raise "JPEG XL transaction directory must share the Cellar volume" unless @state.stat.dev == HOMEBREW_CELLAR.stat.dev

    @stage = @state/"#{transaction_name}-#{SecureRandom.hex(12)}"
    @stage.mkdir(0o700)
    (@stage/"backup").mkdir(0o700)
    rows.each do |row|
      path = original(row)
      row["manifest"] = manifest(path)
      run_command("/usr/bin/ditto", path, saved(row))
      raise "JPEG XL backup verification failed: #{path}" unless manifest(saved(row)) == row.fetch("manifest")
    end
    run_command("/bin/sync")
    @record = {
      "stage" => @stage.basename.to_s, "rows" => rows, "tools" => tools, "touched" => [],
      "committed" => false, "repair" => repair, "touched_tools" => [],
    }
    checkpoint
  rescue StandardError
    FileUtils.remove_entry_secure(@stage) if @stage&.directory? && !present?(@journal)
    raise
  end

  def verify_original(row)
    short = row.fetch("name")
    prior = original(row)
    raise "Original keg changed before updating #{short}" unless record_target(opt(short), short) == prior &&
      record_target(linked(short), short) == (row.fetch("linked") ? prior : nil) &&
      (prior.dirname.stat.mode & 0o777) == row.fetch("rack_mode") &&
      versions(short) == row.fetch("versions") && manifest(prior) == row.fetch("manifest")
  end

  def mark(row)
    verify_original(row)
    @record.fetch("touched") << row.fetch("name")
    checkpoint
  end

  def verify_dependency_plan(owner, writes)
    changed = writes.keys.find { |short| owner.fetch("dependencies").fetch(short, nil) != writes.fetch(short) }
    raise "Homebrew dependency plan changed during rebuild for #{owner.fetch('full')}: #{changed}" if changed
  end

  def admit_dependency(owner, dep, target)
    short = target.name
    planned = owner.fetch("dependencies")[short]
    raise "Unjournaled Homebrew dependency write required for #{owner.fetch('full')}: #{short}" unless planned

    verify_dependency_plan(owner, short => dependency_write(owner, dep, target))
    row = @record.fetch("rows").find { |entry| entry.fetch("name") == short }
    touched = @record.fetch(row ? "touched" : "touched_tools")
    raise "Repeated Homebrew dependency write for #{short}" if touched.include?(short)

    if row
      mark(row)
    else
      raise "Unjournaled Homebrew dependency write required for #{short}" unless
        @record.fetch("tools")[short] == planned.fetch("version") &&
        [rack(short), opt(short), linked(short)].none? { |path| present?(path) }

      touched << short
      checkpoint
    end
  end

  def verify_dependency(owner, short)
    expected = owner.fetch("dependencies").fetch(short)
    row = @record.fetch("rows").find { |entry| entry.fetch("name") == short }
    actual = row ? verify_replacement(row) : keg_directory(record_target(opt(short), short))
    raise "Homebrew selected an unexpected dependency version for #{short}" unless
      actual.basename.to_s == expected.fetch("version") && source_info(receipt(actual)) == expected.fetch("source")
  end

  def verify_replacement(row)
    short = row.fetch("name")
    path = keg_directory(record_target(opt(short), short))
    linked_target = record_target(linked(short), short)
    tab = receipt(path)
    raise "Source or build options changed for #{short}" unless source_info(tab) == row.fetch("source")
    raise "Homebrew linking mode changed for #{short}" unless linked_target == (row.fetch("linked") ? path : nil)
    if row.fetch("source")[1] == "head"
      revision = tab.fetch("source")["scm_revision"]
      raise "Homebrew did not record an exact HEAD for #{short}" unless revision&.match?(/\A[0-9a-f]{40}\z/) &&
        path.basename.to_s.start_with?("HEAD-#{revision[0, 7]}")
    end
    path
  end

  def dependency_order(consumers)
    pending = consumers.to_h { |row| [row.fetch("name"), row] }
    dependencies = pending.transform_values do |row|
      resolve(row).recursive_dependencies.map(&:name)
    end
    Array.new(pending.size) do
      name = pending.keys.find { |short| (dependencies.fetch(short) & pending.keys).empty? }
      raise "Circular JPEG XL consumer dependencies" unless name

      pending.delete(name)
    end
  end

  def remove_current_links(short)
    [opt(short), linked(short)].filter_map { |path| record_target(path, short) }.uniq.each do |target|
      Keg.new(keg_directory(target)).unlink if target.directory?
    end
    [opt(short), linked(short)].each do |path|
      path.unlink if record_target(path, short)
    end
  end

  def restore(row)
    short = row.fetch("name")
    prior = original(row)
    backup = saved(row)
    expected = row.fetch("manifest")
    FormulaLock.new(short).with_lock do
      installed = versions(short)
      validate_recovery_versions(row, installed)
      missing = row.fetch("versions") - installed - [row.fetch("version")]
      raise "Inactive Homebrew kegs disappeared during JPEG XL recovery: #{short}" unless missing.empty?

      if !prior.directory? || manifest(prior) != expected
        raise "JPEG XL rollback backup drifted: #{backup}" unless manifest(backup) == expected
      end
      remove_current_links(short)
      (installed - row.fetch("versions")).each { |version| archive(rack(short)/version) }
      if !prior.directory? || manifest(prior) != expected
        archive(prior) if present?(prior)
        unless present?(prior.dirname)
          prior.dirname.mkdir
          prior.dirname.chmod(row.fetch("rack_mode"))
          sync_directory(prior.dirname.dirname)
        end
        backup.rename(prior)
        sync_directory(prior.dirname)
        sync_directory(backup.dirname)
      end
      keg = Keg.new(prior)
      row.fetch("linked") ? keg.link : keg.optlink
      raise "JPEG XL rollback verification failed for #{short}" unless opt(short).realpath == prior &&
        keg.linked? == row.fetch("linked") && manifest(prior) == expected && versions(short) == row.fetch("versions")
    end
  end

  def validate_recovery_versions(row, installed)
    return unless (replacement = row["replacement"])

    unexpected = installed - row.fetch("versions") - [replacement, "#{replacement}.tmp"]
    raise "Unexpected dependency kegs during recovery for #{row.fetch('name')}: #{unexpected.join(', ')}" unless unexpected.empty?
  end

  def restore_tool(short, version)
    FormulaLock.new(short).with_lock do
      installed = versions(short)
      if installed.empty? && !present?(opt(short)) && !present?(linked(short))
        if rack(short).directory?
          rack(short).rmdir
          sync_directory(HOMEBREW_CELLAR)
        end
        return
      end

      keg = rack(short)/version
      raise "Unexpected build-tool state during recovery: #{short}" unless installed == [version] &&
        [opt(short), linked(short)].all? { |path| [nil, keg].include?(record_target(path, short)) }

      if present?(keg/"INSTALL_RECEIPT.json")
        tab = receipt(keg)
        raise "Build-tool provenance drifted: #{short}" unless source_info(tab) == ["homebrew/core", "stable", []] &&
          tab["installed_on_request"] == false
      elsif present?(opt(short)) || present?(linked(short))
        raise "Linked build tool has no receipt: #{short}"
      end
      remove_current_links(short)
      archive(keg)
      rack(short).rmdir if rack(short).directory?
    end
  end

  def reset_linkage_cache(rows)
    CacheStoreDatabase.use(:linkage) do |db|
      rows.each do |row|
        short = row.fetch("name")
        path = record_target(opt(short), short)
        LinkageCacheStore.new(path.to_s, db).delete! if path
      end
    end
  end

  def load_record
    info = @journal.lstat
    raise "Unsafe JPEG XL journal" unless info.file? && info.uid == Process.uid &&
      info.nlink == 1 && (info.mode & 0o077).zero? && info.size <= MAX_JOURNAL_BYTES

    data = @journal.binread(MAX_JOURNAL_BYTES + 1)
    raise "JPEG XL journal is too large" if data.bytesize > MAX_JOURNAL_BYTES

    @record = JSON.parse(data)
    stage = @record.fetch("stage")
    raise "Invalid JPEG XL journal directory" unless stage.is_a?(String) && stage.match?(/\A#{Regexp.escape(transaction_name)}-[a-f0-9]{24}\z/)

    @stage = @state/stage
    private_directory(@stage)
    rows = @record.fetch("rows")
    tools = @record.fetch("tools")
    touched = @record.fetch("touched")
    repair = @record.fetch("repair", false)
    names = rows.map do |row|
      short = name(row.fetch("full"))
      manifest = row.fetch("manifest")
      mode = row.fetch("rack_mode")
      raise "Invalid JPEG XL journal row" unless row.fetch("name") == short &&
        NAME.match?(row.fetch("version")) && row.fetch("versions").all? { |version| NAME.match?(version) } &&
        row.fetch("versions").include?(row.fetch("version")) && [true, false].include?(row.fetch("linked")) &&
        mode.is_a?(Integer) && mode.between?(0o700, 0o777) &&
        manifest.is_a?(Hash) && manifest["."]&.first == "directory"

      short
    end
    raise "Invalid #{transaction_name} journal package group" unless names.take(providers.size) == providers &&
      names.uniq == names && names.drop(providers.size).none? { |short| protected?(short) } &&
      tools.is_a?(Hash) && (tools.keys - BUILD_TOOLS).empty? && tools.values.all? { |version| NAME.match?(version) } &&
      (tools.keys & names).empty? && (touched - names).empty? && touched.uniq == touched &&
      [true, false].include?(@record.fetch("committed")) &&
      [true, false].include?(repair) && (provider_upgrades? || repair) &&
      (!repair || (touched & providers).empty?)

    return unless @record.key?("touched_tools")

    touched_tools = @record.fetch("touched_tools")
    raise "Invalid #{transaction_name} journal tools" unless touched_tools.is_a?(Array) &&
      touched_tools.uniq == touched_tools && (touched_tools - tools.keys).empty?

    indexed = rows.to_h { |row| [row.fetch("name"), row] }
    rows.each do |owner|
      next unless owner.key?("dependencies")

      owner.fetch("dependencies").each do |short, write|
        dependency = indexed[short]
        valid = if dependency
          !dependency.key?("dependencies") && !providers.include?(short) &&
            dependency.fetch("replacement") == write.fetch("version") &&
            dependency.fetch("full") == write.fetch("full") && dependency.fetch("source") == write.fetch("source")
        else
          [short, "homebrew/core/#{short}"].include?(write.fetch("full")) &&
            write.fetch("source") == ["homebrew/core", "stable", []] &&
            tools.fetch(short, nil) == write.fetch("version")
        end
        raise "Invalid Homebrew journal dependency: #{short}" unless valid &&
          NAME.match?(write.fetch("version"))
      end
    end
  end

  def recover
    return unless present?(@journal)

    load_record
    unless @record.fetch("committed")
      @recovering = true
      touched = @record.fetch("touched")
      rows = @record.fetch("rows").to_h { |row| [row.fetch("name"), row] }
      touched.reverse_each { |short| restore(rows.fetch(short)) }
      tools = @record.fetch("tools")
      # Older journals marked only owners, so any of their planned tools may have been installed.
      touched_tools = @record.fetch("touched_tools") { touched.empty? ? [] : tools.keys }
      touched_tools.reverse_each { |short| restore_tool(short, tools.fetch(short)) }
      reset_linkage_cache(rows.values_at(*touched))
      # Repair starts from broken linkage. Check the untouched provider and exact restored kegs.
      if @record.fetch("repair", false)
        providers.each { |provider| verify_original(rows.fetch(provider)) }
      else
        verify_linkage
      end
      puts "#{transaction_name} transaction rolled back"
    end
    @journal.unlink
    sync_directory(@state)
    remove_committed_backup if @record.fetch("committed")
  ensure
    @recovering = false
  end

  def smoke(jpeg, consumers)
    fixture = @stage/"fixture.ppm"
    encoded = @stage/"fixture.jxl"
    fixture.binwrite("P6\n16 16\n255\n".b + [32, 112, 224].pack("C*") * 256)
    run_command(jpeg/"bin/cjxl", fixture, encoded, "-d", "0")
    decoded = @stage/"decoded.ppm"
    run_command(jpeg/"bin/djxl", encoded, decoded)
    raise "JPEG XL lossless round trip changed the fixture" unless decoded.binread == fixture.binread

    consumers.each do |row|
      bin = opt(row.fetch("name"))/"bin"
      case row.fetch("name")
      when "ffmpeg"
        ffmpeg_encoded = @stage/"ffmpeg.jxl"
        run_command(bin/"ffmpeg", "-v", "error", "-i", fixture, "-c:v", "libjxl", ffmpeg_encoded)
        run_command(bin/"ffmpeg", "-v", "error", "-i", ffmpeg_encoded, "-f", "null", "-")
        info = capture(bin/"ffprobe", "-v", "error", "-show_entries", "stream=width,height", "-of", "json", ffmpeg_encoded)
        raise "FFmpeg JPEG XL smoke dimensions changed" unless JSON.parse(info).fetch("streams") ==
          [{ "width" => 16, "height" => 16 }]
      when "imagemagick-full"
        info = capture(bin/"magick", "identify", "-format", "%wx%h", encoded)
        raise "ImageMagick JPEG XL smoke dimensions changed" unless info.strip == "16x16"
      when "vips"
        png = @stage/"vips.png"
        run_command(bin/"vips", "copy", encoded, png)
        header = png.binread(24)
        raise "vips JPEG XL smoke dimensions changed" unless header.start_with?("\x89PNG\r\n\x1a\n".b) &&
          header.byteslice(16, 8).unpack("NN") == [16, 16]
      end
    end
    verify_linkage
  end

  def remove_committed_backup
    FileUtils.remove_entry_secure(@stage/"backup") if (@stage/"backup").directory?
  rescue SystemCallError => error
    warn "#{linkage_group} upgrade succeeded; could not remove duplicate backup in #{@stage}: #{error}"
  end

  def linkage_group
    "JPEG XL"
  end

  def rebuild_consumers(consumers, candidate)
    consumers.each do |row|
      next if row.fetch("links").all? { |link| link_satisfied?(link, candidate) }

      Formula.cache.clear
      Formulary.clear_cache
      verify_dependency_plan(row, preflight(row))

      brew("reinstall", row)
      verify_replacement(row)
    end
    consumers.each do |row|
      active = keg_directory(record_target(opt(row.fetch("name")), row.fetch("name")))
      links = direct_links(active)
      raise "#{row.fetch('full')} does not link the active #{linkage_group} keg" unless links.any? &&
        links.all? { |link| link_satisfied?(link, candidate) }
    end
  end

  def transact(rows, tools, repair: false)
    prepare(rows, tools, repair: repair)
    begin
      yield
      @record["committed"] = true
      checkpoint
      @journal.unlink
      sync_directory(@state)
    rescue StandardError => error
      begin
        recover
      rescue StandardError => recovery
        raise "#{error.message}; #{linkage_group} recovery incomplete: #{recovery.message}; journal: #{@journal}"
      end
      raise error
    end
    remove_committed_backup
  end

  def run(full, users)
    %w[INT TERM HUP].each { |signal| Signal.trap(signal) { @signal ||= signal } }
    rows, tools = plan(full, users)
    jpeg = rows.first
    consumers = rows.select { |row| row.key?("links") }
    ordered = dependency_order(consumers)
    verify_linkage
    transact(rows, tools) do
      brew("upgrade", jpeg)
      candidate = verify_replacement(jpeg)
      raise "Homebrew did not select a new JPEG XL keg" if candidate == original(jpeg)

      rebuild_consumers(ordered, candidate)
      reset_linkage_cache(rows)
      smoke(candidate, consumers)
      FormulaLock.new("jpeg-xl").with_lock do
        prior = original(jpeg)
        raise "Prior JPEG XL keg changed before retirement" unless manifest(prior) == jpeg.fetch("manifest")

        archive(prior)
      end
      verify_linkage
    end
    puts "JPEG XL and dependent linkage verified; retired keg stored in #{@stage}"
  end

  def repair(full, users)
    %w[INT TERM HUP].each { |signal| Signal.trap(signal) { @signal ||= signal } }
    rows, tools = repair_plan(full, users)
    jpeg = rows.first
    consumers = rows.select { |row| row.key?("links") }
    return if consumers.empty?

    ordered = dependency_order(consumers)
    transact(rows, tools, repair: true) do
      verify_original(jpeg)
      current = original(jpeg)
      rebuild_consumers(ordered, current)
      reset_linkage_cache(rows)
      smoke(current, consumers)
      verify_original(jpeg)
    end
    puts "JPEG XL consumer linkage repaired and verified"
  end
end

# Reuse the keg journal for consumers of the synchronized pair. The provider
# kegs have already been selected by HEAD-sync and are verified read-only here.
class GgmlLlamaRepair < JpegXlTransaction
  PAIR_LIBRARY = /\Alib(?:ggml|llama|mtmd)(?:[-_][a-z0-9_]+)*(?:\.[0-9]+)*\.dylib\z/

  def transaction_name
    "ggml-llama"
  end

  def linkage_group
    "ggml/llama.cpp"
  end

  def providers
    %w[ggml llama.cpp]
  end

  def direct_links(keg)
    dylib_links(keg).select do |link|
      providers.any? { |provider| link.start_with?("#{opt(provider)}/", "#{rack(provider)}/") } ||
        (link.start_with?("@") && PAIR_LIBRARY.match?(File.basename(link)))
    end
  end

  def link_satisfied?(link, active)
    provider = providers.find { |short| link.start_with?("#{opt(short)}/", "#{rack(short)}/") }
    return false unless provider

    file = Pathname(link)
    file.file? && file.realpath.to_s.start_with?("#{active.fetch(provider)}/")
  end

  def pair_group(full, users)
    raise "Unexpected synchronized provider group" unless full.all? && full.map { |formula| name(formula) } == providers

    provider_rows = full.map { |formula| active_row(formula).first }
    active = provider_rows.to_h { |row| [row.fetch("name"), original(row)] }
    names = users.map { |user| name(user) }
    raise "Ambiguous synchronized dependency group" unless names.uniq == names && (names & providers).empty?

    broken = users.filter_map do |user|
      row, keg = active_row(user)
      links = direct_links(keg)
      next if links.all? { |link| link_satisfied?(link, active) }

      raise "Protected formula has stale #{linkage_group} linkage: #{user}" if protected?(row.fetch("name"))
      raise "Ambiguous #{linkage_group} load path in #{user}" if links.any? { |link| link.start_with?("@") }

      row.merge("links" => links)
    end
    [provider_rows, active, broken]
  end

  def plan_pair(full, users)
    _, active, broken = pair_group(full, users)
    return "current" if broken.empty?

    whisper = broken.find do |row|
      row.fetch("name") == "whisper.cpp" && row.fetch("source").take(2) == ["homebrew/core", "stable"]
    end
    if whisper && resolve(whisper).version.to_s == "1.9.5" &&
        (active.fetch("llama.cpp")/"lib/cmake/llama/llama-config-version.cmake").read.match?(
          /^set\(PACKAGE_VERSION "[1-9][0-9]*\./,
        )
      "whisper-compat"
    else
      "repair"
    end
  end

  def repair_pair(full, users)
    %w[INT TERM HUP].each { |signal| Signal.trap(signal) { @signal ||= signal } }
    provider_rows, active, broken = pair_group(full, users)
    return if broken.empty?

    puts "#{linkage_group} linkage repair: #{broken.map { |row| row.fetch('full') }.join(', ')}"
    rows = provider_rows + broken
    tools = plan_dependencies(rows, broken)
    transact(rows, tools, repair: true) do
      provider_rows.each { |row| verify_original(row) }
      rebuild_consumers(dependency_order(broken), active)
      reset_linkage_cache(rows)
      provider_rows.each { |row| verify_original(row) }
    end
    puts "#{linkage_group} consumer linkage repaired and verified"
  end
end

# The formula journal pins both builds. This journal separately protects the
# installed provider kegs, which restoring formula source cannot roll back.
class GgmlLlamaUpgrade < GgmlLlamaRepair
  REVISION = /\A[0-9a-f]{40}\z/
  BACKEND_SYMBOL = "ggml_backend_sched_set_copy_callback"

  def transaction_name
    "ggml-llama-upgrade"
  end

  def provider_upgrades?
    true
  end

  def head_revision(keg)
    tab = receipt(keg)
    revision = tab.fetch("source")["scm_revision"]
    raise "Expected a full homebrew/core HEAD revision in #{keg}" unless
      source_info(tab).take(2) == ["homebrew/core", "head"] && revision.is_a?(String) && REVISION.match?(revision) &&
      keg.basename.to_s.match?(/\AHEAD-#{Regexp.escape(revision[0, 7])}(?:_[0-9]+)?\z/)

    revision
  end

  def target_keg(short, revision)
    matches = []
    collisions = []
    versions(short).each do |version|
      next unless version.match?(/\AHEAD-#{Regexp.escape(revision[0, 7])}(?:_[0-9]+)?\z/)

      path = keg_directory(rack(short)/version)
      if head_revision(path) == revision
        matches << path
      else
        collisions << path
      end
    end
    if matches.empty? && collisions.any?
      raise "Short HEAD collision: #{collisions.join(', ')} has a different full revision; existing kegs were preserved"
    end

    # Homebrew can retain multiple HEADs and multiple formula revisions. The
    # receipt, never their enumeration order or seven-character name, selects SHA.
    matches.max_by { |path| path.basename.to_s[/_([0-9]+)\z/, 1].to_i }
  end

  def verify_links(short, keg)
    raise "The exact #{short} keg is not linked: #{keg}" unless
      record_target(opt(short), short) == keg && record_target(linked(short), short) == keg

    bin = keg/"bin"
    return unless bin.directory?

    bin.children.reject(&:directory?).each do |source|
      destination = HOMEBREW_PREFIX/"bin"/source.basename
      raise "#{destination} does not resolve to #{source}" unless destination.symlink? &&
        destination.exist? && destination.realpath == source.realpath
    end
  end

  def matched_pair(revision = nil)
    rows = providers.map do |short|
      row, keg = active_row(short)
      full_revision = head_revision(keg)
      raise "#{short} full revision does not match #{revision}" if revision && full_revision != revision

      verify_links(short, keg)
      row.merge("revision" => full_revision)
    end
    raise "ggml and llama.cpp must have the same full revision before upgrading" unless
      rows.map { |row| row.fetch("revision") }.uniq.one?

    [rows, rows.to_h { |row| [row.fetch("name"), original(row)] }]
  end

  def verify_runtime(active)
    ggml = active.fetch("ggml")
    raise "Missing ggml backend declaration #{BACKEND_SYMBOL}" unless (ggml/"include/ggml-backend.h").read.include?(BACKEND_SYMBOL)

    symbols = capture("nm", "-gU", ggml/"lib/libggml-base.dylib", quiet: true)
    found = symbols.lines.any? do |line|
      line.split.last&.delete_prefix("_") == BACKEND_SYMBOL
    end
    raise "Missing ggml backend symbol #{BACKEND_SYMBOL}" unless found

    run_command(active.fetch("llama.cpp")/"bin/llama-cli", "--version")
    unified = active.fetch("llama.cpp")/"bin/llama"
    run_command(unified, "--version") if present?(unified)
  end

  def activate(short, candidate, revision)
    FormulaLock.new(short).with_lock do
      raise "Selected #{short} receipt changed before linking" unless head_revision(keg_directory(candidate)) == revision

      # Validate record destinations before unlinking. Keg#unlink removes only
      # links owned by its Pathname; no --overwrite may remove unrelated files.
      [opt(short), linked(short)].each { |record| record_target(record, short) }
      versions(short).each { |version| Keg.new(rack(short)/version).unlink }
      remove_current_links(short)
      Keg.new(candidate).link
      verify_links(short, candidate)
    end
  end

  def load_record
    super
    rows = @record.fetch("rows")
    valid_rows = rows.all? do |row|
      revision = row["revision"]
      target = row["target_revision"]
      revision.is_a?(String) && REVISION.match?(revision) && target.is_a?(String) && REVISION.match?(target) && row.fetch("linked") &&
        row.fetch("source").take(2) == ["homebrew/core", "head"] &&
        row.fetch("version").match?(/\AHEAD-#{Regexp.escape(revision[0, 7])}(?:_[0-9]+)?\z/)
    end
    raise "Invalid synchronized provider upgrade journal" unless valid_rows && rows.size == providers.size &&
      @record.fetch("tools").empty? && !@record.fetch("repair")
    raise "Provider rollback journal does not contain a matched pair" unless
      rows.map { |row| row.fetch("revision") }.uniq.one? && rows.map { |row| row.fetch("target_revision") }.uniq.one?
  end

  def validate_recovery_versions(row, installed)
    super
    short = row.fetch("name")
    target = row.fetch("target_revision")
    (installed - row.fetch("versions")).each do |version|
      error = "Unexpected provider keg during recovery: #{rack(short)/version}; preserved in place. " \
              "Resolve the concurrent Homebrew change before retrying; journal retained at #{@journal}"
      raise error unless version.match?(/\AHEAD-#{Regexp.escape(target[0, 7])}(?:_[0-9]+)?(?:\.tmp)?\z/)

      path = rack(short)/version
      next unless present?(path/"INSTALL_RECEIPT.json")

      tab = receipt(path)
      raise "#{error}; full revision does not match #{target}" unless
        source_info(tab).take(2) == ["homebrew/core", "head"] && tab.fetch("source")["scm_revision"] == target
    end
  end

  def verify_linkage
    # Called by the inherited recovery after both exact originals are restored.
    rows, active = matched_pair(@record.fetch("rows").first.fetch("revision"))
    raise "Recovery selected different provider kegs" unless rows.map { |row| original(row) } ==
      @record.fetch("rows").map { |row| original(row) }

    verify_runtime(active)
  end

  def upgrade_pair(revision)
    raise "Expected one full target HEAD revision" unless revision.is_a?(String) && REVISION.match?(revision)
    raise "An unfinished provider upgrade exists; recover it first" if present?(@journal)

    %w[INT TERM HUP].each { |signal| Signal.trap(signal) { @signal ||= signal } }
    ready = providers.to_h { |short| [short, target_keg(short, revision)] }
    begin
      rows, active = matched_pair
      verify_runtime(active)
    rescue StandardError => error
      unless ready.values.all?
        raise "No verified matched ggml/llama.cpp pair is available for rollback: #{error.message}. " \
              "No provider builds were started; restore a matched installed HEAD pair before retrying"
      end

      # A previous failed run may already have built both exact kegs but left
      # opt/bin unlinked. Repair their links without building or removing kegs.
      warn "Repairing installed target links; no verified prior pair is available for rollback: #{error.message}"
      begin
        ready.each { |short, keg| activate(short, keg, revision) }
        repaired_rows, active = matched_pair(revision)
        verify_runtime(active)
        reset_linkage_cache(repaired_rows)
      rescue StandardError => repair
        raise "Installed target #{revision} link repair is incomplete: #{repair.message}. " \
              "All kegs were retained (#{ready.values.join(', ')}); resolve the reported error and rerun HEAD-sync"
      end
      puts "Exact installed ggml/llama.cpp target links and runtime verified: #{revision}"
      return
    end

    # The target is present in the very first durable checkpoint, including when
    # the first builder is interrupted before writing an installation receipt.
    rows = rows.map { |row| row.merge("target_revision" => revision) }
    transact(rows, {}) do
      # Mark both before the first builder; neither provider may become an
      # unjournaled side effect if Homebrew's dependency plan changes.
      rows.each { |row| mark(row) }
      rows.each do |row|
        short = row.fetch("name")
        verify_original(row)
        candidate = ready.fetch(short)
        unless candidate
          puts "== Upgrade #{short} at #{revision} =="
          run_command(HOMEBREW_BREW_FILE, "upgrade", "--formula", "--fetch-HEAD", short)
          candidate = target_keg(short, revision)
        end
        raise "Homebrew did not install #{short} at full revision #{revision}" unless candidate
        raise "Homebrew changed the source or build options for #{short}" unless source_info(receipt(candidate)) == row.fetch("source")

        activate(short, candidate, revision)
      end
      _, active = matched_pair(revision)
      verify_runtime(active)
      reset_linkage_cache(rows)
    end
    puts "ggml and llama.cpp upgraded, linked, and verified at full revision #{revision}"
  end
end

if $PROGRAM_NAME == __FILE__
  File.umask(0o077)
  mode, state, full, *users = ARGV
  if %w[pair-upgrade pair-upgrade-recover].include?(mode)
    raise "Unexpected provider upgrade arguments" unless users.empty? && (mode == "pair-upgrade" || full.nil?)

    pair = GgmlLlamaUpgrade.new(state)
    mode == "pair-upgrade" ? pair.upgrade_pair(full) : pair.recover
    exit
  end
  if %w[jpeg-xl-install ggml-llama-install].include?(mode)
    klass = mode == "jpeg-xl-install" ? JpegXlTransaction : GgmlLlamaRepair
    klass.new(state).run_install(full, users.fetch(0))
    exit
  elsif %w[pair-plan pair-repair].include?(mode)
    llama, *consumers = users
    pair = GgmlLlamaRepair.new(state)
    if mode == "pair-plan"
      puts pair.plan_pair([full, llama], consumers)
    else
      pair.repair_pair([full, llama], consumers)
    end
    exit
  elsif mode == "pair-recover"
    GgmlLlamaRepair.new(state).recover
    exit
  end
  transaction = JpegXlTransaction.new(state)
  case mode
  when "recover" then transaction.recover
  when "plan" then transaction.plan(full, users)
  when "repair-plan" then transaction.repair_plan(full, users)
  when "repair" then transaction.repair(full, users)
  when "run" then transaction.run(full, users)
  else raise "Expected JPEG XL transaction mode: plan, repair-plan, run, repair or recover"
  end
end
