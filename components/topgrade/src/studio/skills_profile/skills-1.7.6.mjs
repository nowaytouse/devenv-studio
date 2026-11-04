// Compatibility patch for vercel-labs/skills v1.7.6 (MIT, Copyright 2026 Vercel, Inc.).
// See https://github.com/vercel-labs/skills/blob/v1.7.6/LICENSE
import { createHash } from "node:crypto";
import { existsSync, readFileSync, realpathSync } from "node:fs";
import { registerHooks } from "node:module";
import { delimiter, dirname, join } from "node:path";
import { pathToFileURL } from "node:url";

const shim = (process.env.PATH ?? "").split(delimiter)
  .map((dir) => join(dir, "skills")).find((file) => existsSync(file));
const entry = shim && realpathSync(shim);
if (!entry?.endsWith("/node_modules/skills/bin/cli.mjs")) {
  throw new Error("The pinned NPX skills executable could not be resolved");
}
const file = join(dirname(entry), "../dist/cli.mjs");
if (realpathSync(file) !== file) throw new Error("Unexpected symbolic link in the pinned skills bundle");
const bundle = readFileSync(file);
if (createHash("sha256").update(bundle).digest("hex") !==
    "d26e7ef21cae131cec2587457d9f1574835329b1532f2dfe50b4b3d5441cad54") {
  throw new Error("The NPX skills bundle does not match the reviewed 1.7.6 artifact");
}
let source = bundle.toString("utf8");
function replaceOnce(text, before, after) {
  const start = text.indexOf(before);
  if (start < 0 || text.indexOf(before, start + before.length) !== -1) {
    throw new Error("The pinned skills compatibility patch no longer matches");
  }
  return text.slice(0, start) + after + text.slice(start + before.length);
}

// Scope the longer timeout to GitHub tree responses, including gh api's fallback.
const treeStart = source.indexOf("async function fetchTreeBranch(");
const treeEnd = source.indexOf("async function fetchTreeWithAvailableAuth(", treeStart);
let tree = source.slice(treeStart, treeEnd);
tree = replaceOnce(tree, "AbortSignal.timeout(FETCH_TIMEOUT)", "AbortSignal.timeout(45_000)");
tree = replaceOnce(tree, "timeout: FETCH_TIMEOUT,", "timeout: 45_000,");
tree = replaceOnce(tree, "if (typeof data.sha !==", "if (data.truncated || typeof data.sha !==");
tree = replaceOnce(tree, "const data = await response.json();", "const data = await response.json();\n" +
  'if (data.truncated || typeof data.sha !== "string" || !Array.isArray(data.tree)) throw new Error("Incomplete GitHub tree");');
source = source.slice(0, treeStart) + tree + source.slice(treeEnd);
source = replaceOnce(source,
  "\tif (_rateLimitedThisSession) return fetchTreeWithAvailableAuth(ownerRepo, branches, getToken);",
  "\tif (getGitHubHost() === 'github.com' && getToken?.()) " +
  "return fetchTreeWithAvailableAuth(ownerRepo, branches, getToken);\n" +
  "\tif (_rateLimitedThisSession) return fetchTreeWithAvailableAuth(ownerRepo, branches, getToken);");

// A known large repository is usually checked for only one installed path.
async function fetchBuilderLockedTree(ref, token, paths) {
  if (!token || paths.length !== 1 || typeof paths[0] !== "string") return null;
  const parts = paths[0].split("/");
  if (parts.length > 5 || parts.some((part) => !part || part === "." || part === "..")) return null;
  if (parts.at(-1) !== "SKILL.md") return null;
  const isSha = (value) => typeof value === "string" && /^[0-9a-f]{40}$/.test(value);
  const signal = AbortSignal.timeout(12_000);
  const headers = { Accept: "application/vnd.github+json", Authorization: "Bearer " + token, "User-Agent": "skills-cli" };
  const fetchTree = async (id) => {
    const response = await fetch(
      "https://api.github.com/repos/BuilderIO/agent-native/git/trees/" + encodeURIComponent(id),
      { headers, signal, redirect: "error" },
    );
    if (!response.ok) throw new Error("GitHub tree unavailable");
    const data = await response.json();
    if (data.truncated || !isSha(data.sha) || !Array.isArray(data.tree)) throw new Error("Incomplete GitHub tree");
    return data;
  };
  try {
    const root = await fetchTree(ref || "HEAD");
    let current = root;
    const tree = [];
    for (const [index, part] of parts.entries()) {
      const type = index + 1 === parts.length ? "blob" : "tree";
      const child = current.tree.find((item) => item.path === part && item.type === type);
      if (!child || !isSha(child.sha)) return null;
      tree.push({ ...child, path: parts.slice(0, index + 1).join("/") });
      if (type === "tree") {
        current = await fetchTree(child.sha);
        if (current.sha !== child.sha) return null;
      }
    }
    return { sha: root.sha, branch: ref || "HEAD", tree };
  } catch {
    // A missing or relocated path still uses the upstream full-tree/clone resolution.
    return null;
  }
}
source = replaceOnce(source, "async function fetchGlobalSource(entry, lockedSkillPaths) {",
  fetchBuilderLockedTree.toString() + "\nasync function fetchGlobalSource(entry, lockedSkillPaths) {");
source = replaceOnce(source, "\t\t\tconst tree = await fetchRepoTree(entry.source, entry.ref, getGitHubToken);",
  "\t\t\tconst sparse = entry.source.toLowerCase() === 'builderio/agent-native' && getGitHubHost() === 'github.com' " +
  "? await fetchBuilderLockedTree(entry.ref, getGitHubToken(), lockedSkillPaths) : null;\n" +
  "\t\t\tconst tree = sparse ?? await fetchRepoTree(entry.source, entry.ref, getGitHubToken);");

// This caller consumes only the names; agent-location enumeration is unnecessary.
source = replaceOnce(source, "\t\t\tconst skillKey = `${scopeKey}:${skill.name}`;",
  "\t\t\tconst skillKey = `${scopeKey}:${skill.name}`;\n" +
  "\t\t\tif (options.namesOnly) { skillsMap.set(skillKey, { name: skill.name }); continue; }");
source = replaceOnce(source,
  "(await listInstalledSkills({ global })).map((skill) => skill.name)",
  "(await listInstalledSkills({ global, namesOnly: global })).map((skill) => skill.name)");

const globalStart = source.indexOf("async function updateGlobalSkills(");
const globalEnd = source.indexOf("async function updateProjectSkills(", globalStart);
if (globalStart < 0 || globalEnd < 0) throw new Error("Missing pinned global updater");
let global = source.slice(globalStart, globalEnd);
global = replaceOnce(global, "\tconst onDisk = await installedSkillNames(true);", [
  "\tconsole.log('Reading installed global skills…');",
  "\tconst inventoryStart = Date.now();",
  "\tconst inventoryProgress = setInterval(() => {",
  "\t\tconsole.log('Reading installed global skills (' + Math.round((Date.now() - inventoryStart) / 1000) + 's elapsed)…');",
  "\t}, 15_000).unref();",
  "\tconst onDisk = await installedSkillNames(true).finally(() => clearInterval(inventoryProgress));",
].join("\n"));
const fetchStart = global.indexOf("\tconst checkouts = await mapConcurrent(groups,");
const loopStart = global.indexOf("\tfor (const [index, itemsForSource] of groups.entries())", fetchStart);
const cloneStart = global.indexOf("\t\tconst { tempDir, note } = checkout;", loopStart);
if (fetchStart < 0 || loopStart < 0 || cloneStart < 0) throw new Error("Missing pinned source checks");
global = global.slice(0, fetchStart) + [
  "\tlet completed = 0;",
  "\tconst checkouts = await mapConcurrent(groups, async (itemsForSource) => {",
  "\t\tconst first = itemsForSource[0].entry;",
  "\t\tconst source = sanitizeMetadata(first.source);",
  "\t\tconst checkout = await fetchGlobalSource(first, lockedNamesFor(first).map((name) => lock.skills[name]?.skillPath));",
  "\t\tconst progress = '[' + (++completed) + '/' + groups.length + '] ';",
  "\t\tif ('error' in checkout) {",
  "\t\t\tconsole.log(progress + '✗ Failed to check skills from ' + source);",
  "\t\t\treturn { error: true };",
  "\t\t}",
  "\t\tif ('tree' in checkout) {",
  "\t\t\tconst pending = [];",
  "\t\t\tfor (const { name, entry } of itemsForSource) {",
  "\t\t\t\tconst hash = getSkillFolderHashFromTree(checkout.tree, entry.skillPath);",
  "\t\t\t\tif (!hash) {",
  "\t\t\t\t\tconsole.log(progress + '✗ Failed to check skills from ' + source + ' (missing skill tree)');",
  "\t\t\t\t\treturn { error: true };",
  "\t\t\t\t}",
  "\t\t\t\tif (hash !== entry.skillFolderHash || !onDisk.has(name)) pending.push({ name, entry, skillFolderHash: hash });",
  "\t\t\t}",
  "\t\t\tconsole.log(progress + '✓ ' + source + (pending.length ? ' — ' + pending.length + ' pending' : ' — current'));",
  "\t\t\treturn { pending };",
  "\t\t}",
  "\t\tconsole.log(progress + source + ' — Git checkout ready; verifying paths next');",
  "\t\treturn checkout;",
  "\t}, 8);",
  "\tfor (const [index, itemsForSource] of groups.entries()) {",
  "\t\tconst firstEntry = itemsForSource[0].entry;",
  "\t\tconst source = firstEntry.source;",
  "\t\tconst isGitHubSource = firstEntry.sourceType === 'github';",
  "\t\tconst checkout = checkouts[index];",
  "\t\tif ('error' in checkout) { failCount++; continue; }",
  "\t\tif ('pending' in checkout) { updates.push(...checkout.pending); continue; }",
  "",
].join("\n") + global.slice(cloneStart);
global = replaceOnce(global, "\t\t} catch (error) {\n\t\t\tconsole.log(", "\t\t} catch (error) {\n\t\t\tfailCount++;\n\t\t\tconsole.log(");
global = replaceOnce(global, "\t\tconst { tempDir, note } = checkout;",
  "\t\tconst { tempDir, note } = checkout;\n\t\tconst beforeSource = updates.length;");
global = replaceOnce(global, "\t\t} catch (error) {\n\t\t\tfailCount++;",
  "\t\t\tconsole.log('✓ ' + sanitizeMetadata(source) + ' — Git paths verified; ' + " +
  "(updates.length - beforeSource) + ' pending');\n\t\t} catch (error) {\n\t\t\tfailCount++;");
global = replaceOnce(global,
  "\tif (checkable.length === 0 && skipped.length === 0 && wellKnownCount === 0) {",
  "\tif (failCount > 0) {\n\t\tprocess.exitCode = 1;\n\t\treturn { successCount, failCount, checkedCount };\n\t}\n" +
  "\tif (checkable.length === 0 && skipped.length === 0 && wellKnownCount === 0) {");
source = source.slice(0, globalStart) + global + source.slice(globalEnd);

// Keep original module URLs for dependency resolution and leave the NPX cache untouched.
const url = pathToFileURL(file).href;
registerHooks({
  load(specifier, context, nextLoad) {
    if (specifier === url) return { format: "module", source, shortCircuit: true };
    return nextLoad(specifier, context);
  },
});
process.argv[1] = entry;
await import(url);
