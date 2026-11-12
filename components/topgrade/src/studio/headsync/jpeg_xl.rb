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
  STAGE = /\Ajpeg-xl-[a-f0-9]{24}\z/
  LIBRARY = /\Alibjxl(?:_[a-z0-9_]+)?(?:\.[0-9]+)*\.dylib\z/
  OPTIONS = /\A--(?:with|without)-[a-zA-Z0-9_.+@-]+\z/
  BUILD_TOOLS = %w[bison util-macros xtrans lzip make].freeze
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
    @journal = @state/"jpeg-xl-active.json"
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
    raise "JPEG XL upgrade interrupted by #{@signal}" if @signal && !@recovering
  end

  def run_command(*args)
    interrupted!
    status = system(BREW_ENV, *args.map(&:to_s))
    interrupted!
    raise "Failed: #{args.join(' ')}" unless status
  end

  def brew(*args)
    run_command(HOMEBREW_BREW_FILE, *args)
  end

  def capture(*args)
    interrupted!
    output, status = Open3.capture2e(BREW_ENV, *args.map(&:to_s))
    interrupted!
    puts output unless output.empty?
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
    }
    [row, active]
  end

  def direct_links(keg)
    Keg.new(keg).binary_executable_or_library_files.flat_map do |file|
      next [] unless file.arch_compatible?(Hardware::CPU.arch)

      file.dynamically_linked_libraries(except: :DYLIB_USE_WEAK_LINK)
    end.uniq.grep(%r{\A(?:#{Regexp.escape(opt("jpeg-xl").to_s)}/|#{Regexp.escape(rack("jpeg-xl").to_s)}/|@rpath/|@loader_path/|@executable_path/)}).select do |link|
      LIBRARY.match?(File.basename(link))
    end
  end

  def link_satisfied?(link, jpeg)
    return false if link.start_with?("@")

    file = Pathname(link)
    file.file? && file.realpath.to_s.start_with?("#{jpeg}/")
  end

  def preflight(row)
    formula = resolve(row)
    options = row.fetch("source").last
    context = Homebrew::Reinstall.build_install_context(
      formula, flags: options, build_from_source_formulae: [formula.full_name],
    )
    raise "Homebrew would drop build options for #{row.fetch('full')}" unless context.options.as_flags.sort == options

    context.formula_installer.compute_dependencies.to_h do |dep|
      target = dep.to_formula
      unless dep.build? && target.tap.to_s == "homebrew/core" && BUILD_TOOLS.include?(target.name) &&
          !target.pinned? && !target.any_version_installed? && !present?(rack(target.name)) &&
          !record_target(opt(target.name), target.name) && !record_target(linked(target.name), target.name)
        raise "Unjournaled Homebrew dependency write required for #{row.fetch('full')}: #{target.full_name}"
      end
      version = target.pkg_version.to_s
      raise "Unsupported build-tool version: #{version}" unless NAME.match?(version)

      [target.name, version]
    end
  end

  def plan(full, users)
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
    rows = [jpeg, *consumers]
    tools = rows.flat_map { |row| preflight(row).to_a }.to_h
    puts "JPEG XL transaction: #{jpeg.fetch('full')}; direct consumers: #{consumers.map { |r| r.fetch('full') }.join(', ')}"
    puts "Planned optional build tools: #{tools.keys.sort.join(', ')}" unless tools.empty?
    [rows, tools]
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
    temp = @state/".jpeg-xl-journal-#{SecureRandom.hex(12)}"
    temp.open("wx", 0o600) do |file|
      file.write(JSON.generate(@record))
      file.fsync
    end
    temp.rename(@journal)
    sync_directory(@state)
  ensure
    temp.unlink if temp&.exist?
  end

  def prepare(rows, tools)
    raise "An unfinished JPEG XL transaction exists" if present?(@journal)
    raise "JPEG XL transaction directory must share the Cellar volume" unless @state.stat.dev == HOMEBREW_CELLAR.stat.dev

    @stage = @state/"jpeg-xl-#{SecureRandom.hex(12)}"
    @stage.mkdir(0o700)
    (@stage/"backup").mkdir(0o700)
    rows.each do |row|
      path = original(row)
      row["manifest"] = manifest(path)
      run_command("/usr/bin/ditto", path, saved(row))
      raise "JPEG XL backup verification failed: #{path}" unless manifest(saved(row)) == row.fetch("manifest")
    end
    run_command("/bin/sync")
    @record = { "stage" => @stage.basename.to_s, "rows" => rows, "tools" => tools, "touched" => [], "committed" => false }
    checkpoint
  rescue StandardError
    FileUtils.remove_entry_secure(@stage) if @stage&.directory? && !present?(@journal)
    raise
  end

  def mark(row)
    short = row.fetch("name")
    prior = original(row)
    raise "Original keg changed before updating #{short}" unless record_target(opt(short), short) == prior &&
      record_target(linked(short), short) == (row.fetch("linked") ? prior : nil) &&
      versions(short) == row.fetch("versions") && manifest(prior) == row.fetch("manifest")

    @record.fetch("touched") << row.fetch("name")
    checkpoint
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
      if !prior.directory? || manifest(prior) != expected
        raise "JPEG XL rollback backup drifted: #{backup}" unless manifest(backup) == expected
      end
      remove_current_links(short)
      (versions(short) - row.fetch("versions")).each { |version| archive(rack(short)/version) }
      if !prior.directory? || manifest(prior) != expected
        archive(prior) if present?(prior)
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

  def restore_tool(short, version)
    FormulaLock.new(short).with_lock do
      installed = versions(short)
      return if installed.empty? && !present?(rack(short)) && !present?(opt(short)) && !present?(linked(short))

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
      info.nlink == 1 && (info.mode & 0o077).zero?

    @record = JSON.parse(@journal.read)
    stage = @record.fetch("stage")
    raise "Invalid JPEG XL journal directory" unless STAGE.match?(stage)

    @stage = @state/stage
    private_directory(@stage)
    rows = @record.fetch("rows")
    tools = @record.fetch("tools")
    touched = @record.fetch("touched")
    names = rows.map do |row|
      short = name(row.fetch("full"))
      raise "Invalid JPEG XL journal row" unless row.fetch("name") == short &&
        NAME.match?(row.fetch("version")) && row.fetch("versions").all? { |version| NAME.match?(version) } &&
        row.fetch("versions").include?(row.fetch("version"))

      short
    end
    raise "Invalid JPEG XL journal package group" unless names.first == "jpeg-xl" && names.uniq == names &&
      names.none? { |short| protected?(short) } &&
      tools.is_a?(Hash) && (tools.keys - BUILD_TOOLS).empty? && tools.values.all? { |version| NAME.match?(version) } &&
      (tools.keys & names).empty? && (touched - names).empty? && touched.uniq == touched &&
      [true, false].include?(@record.fetch("committed"))
  end

  def recover
    return unless present?(@journal)

    load_record
    unless @record.fetch("committed")
      @recovering = true
      touched = @record.fetch("touched")
      rows = @record.fetch("rows").to_h { |row| [row.fetch("name"), row] }
      touched.reverse_each { |short| restore(rows.fetch(short)) }
      @record.fetch("tools").to_a.reverse_each { |short, version| restore_tool(short, version) } unless touched.empty?
      reset_linkage_cache(rows.values_at(*touched))
      verify_linkage
      puts "JPEG XL transaction rolled back"
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
    warn "JPEG XL upgrade succeeded; could not remove duplicate backup in #{@stage}: #{error}"
  end

  def run(full, users)
    %w[INT TERM HUP].each { |signal| Signal.trap(signal) { @signal ||= signal } }
    rows, tools = plan(full, users)
    jpeg, *consumers = rows
    ordered = dependency_order(consumers)
    verify_linkage
    prepare(rows, tools)
    begin
      mark(jpeg)
      brew("upgrade", "--no-ask", "--build-from-source", "--formula", "--fetch-HEAD", full,
           *jpeg.fetch("source").last)
      candidate = verify_replacement(jpeg)
      raise "Homebrew did not select a new JPEG XL keg" if candidate == original(jpeg)

      ordered.each do |row|
        next if row.fetch("links").all? { |link| link_satisfied?(link, candidate) }
        Formula.cache.clear
        Formulary.clear_cache
        raise "JPEG XL dependency plan changed during upgrade" unless (preflight(row).to_a - tools.to_a).empty?

        mark(row)
        brew("reinstall", "--no-ask", "--build-from-source", "--formula", row.fetch("full"),
             *row.fetch("source").last)
        verify_replacement(row)
      end
      consumers.each do |row|
        active = keg_directory(record_target(opt(row.fetch("name")), row.fetch("name")))
        links = direct_links(active)
        raise "#{row.fetch('full')} does not link the new JPEG XL keg" unless links.any? &&
          links.all? { |link| link_satisfied?(link, candidate) }
      end
      reset_linkage_cache(rows)
      smoke(candidate, consumers)
      FormulaLock.new("jpeg-xl").with_lock do
        prior = original(jpeg)
        raise "Prior JPEG XL keg changed before retirement" unless manifest(prior) == jpeg.fetch("manifest")

        archive(prior)
      end
      verify_linkage
      @record["committed"] = true
      checkpoint
      @journal.unlink
      sync_directory(@state)
    rescue StandardError => error
      begin
        recover
      rescue StandardError => recovery
        raise "#{error.message}; JPEG XL recovery incomplete: #{recovery.message}; journal: #{@journal}"
      end
      raise error
    end
    remove_committed_backup
    puts "JPEG XL and dependent linkage verified; retired keg stored in #{@stage}"
  end
end

if $PROGRAM_NAME == __FILE__
  File.umask(0o077)
  mode, state, full, *users = ARGV
  transaction = JpegXlTransaction.new(state)
  case mode
  when "recover" then transaction.recover
  when "plan" then transaction.plan(full, users)
  when "run" then transaction.run(full, users)
  else raise "Expected JPEG XL transaction mode: plan, run or recover"
  end
end
