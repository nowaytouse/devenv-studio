# NPM integration

The trusted deployment catalog exposes six optional CLI packages: TypeScript, TypeScript Language Server, Prettier, npm-check-updates, http-server and serve. Every item is unchecked initially; importing a legacy or Mac list only proposes exact matching IDs. Web recipes select dependencies explicitly. No server is started by installation.

The Windows adapter resolves the official Node.js installation and bundled npm-cli.js per task, allowing a selected Node.js WinGet prerequisite to run first. It never invokes a shell command wrapper. Inspection, installation and verification share the current user's fixed npm prefix and official registry. The task ignores user npm configuration, disables lifecycle scripts, leaves PATH and project dependencies unchanged, and preserves already installed packages.

Installed state means a complete successful npm JSON list contains the exact package identity, version and fixed installation path. It does not establish a compatible runtime or a working CLI. A package requiring build/postinstall scripts may be present yet unusable; those script-dependent tools are not included in the six-item catalog. Project-local versions remain each project's responsibility. Cancellation and timeout do not undo completed package changes, and execution history does not uninstall packages.

Primary references reviewed on 2026-10-23:

- [npm install](https://docs.npmjs.com/cli/v11/commands/npm-install)
- [npm folders and global prefix](https://docs.npmjs.com/cli/v11/configuring-npm/folders)
- [npm list](https://docs.npmjs.com/cli/v11/commands/npm-ls)
- [TypeScript download](https://www.typescriptlang.org/download/)
- [TypeScript Language Server](https://github.com/typescript-language-server/typescript-language-server)
- [Prettier installation](https://prettier.io/docs/install)
- [npm-check-updates](https://github.com/raineorshine/npm-check-updates)
- [http-server](https://github.com/http-party/http-server)
- [serve](https://github.com/vercel/serve)

Catalog identity is review evidence. Package versions and transitive dependency behavior follow the registry at execution time; Windows/LTSC device acceptance remains separate from source review and target compilation.
