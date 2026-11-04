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
  "\tif (ownerRepo.toLowerCase() === 'builderio/agent-native' && getGitHubHost() === 'github.com' && getToken?.()) " +
  "return fetchTreeWithAvailableAuth(ownerRepo, branches, getToken);\n" +
  "\tif (_rateLimitedThisSession) return fetchTreeWithAvailableAuth(ownerRepo, branches, getToken);");

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
  "\t});",
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
