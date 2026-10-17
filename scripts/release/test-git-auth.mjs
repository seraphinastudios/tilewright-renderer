import assert from "node:assert/strict";
import {mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync} from "node:fs";
import {createRequire} from "node:module";
import {tmpdir} from "node:os";
import {dirname, join} from "node:path";
import {pathToFileURL} from "node:url";

// Exercise the installed pinned implementation against an inert local Git
// transport. No real credential, GitHub connection, push or repository is used.
const workflow = readFileSync(new URL("../../.github/workflows/release.yml", import.meta.url), "utf8");
const executionMarker = "      - name: Analyze commits, verify prepared evidence, and publish\n";
const execution = workflow.slice(workflow.indexOf(executionMarker));
assert.ok(workflow.includes(executionMarker), "final publication execution step exists");
for (const key of ["GH_TOKEN", "GITHUB_TOKEN", "GH_READ_TOKEN"]) {
  assert.ok(execution.includes("          " + key + ": ${{ github.token }}"), `${key} aliases the Actions token only for execution`);
}
assert.equal(workflow.split("          GITHUB_TOKEN: ${{ github.token }}").length, 2,
  "the installation token alias occurs exactly once");
assert.ok(!workflow.slice(0, workflow.indexOf(executionMarker)).includes("          GITHUB_TOKEN:"),
  "the installation token alias is absent from package and dependency installation steps");
const require = createRequire(import.meta.url);
const releaseRoot = dirname(require.resolve("semantic-release"));
assert.equal(JSON.parse(readFileSync(join(releaseRoot, "package.json"))).version, "25.0.9");
const {default: getGitAuthUrl} = await import(pathToFileURL(join(releaseRoot, "lib/get-git-auth-url.js")));
const {verifyAuth} = await import(pathToFileURL(join(releaseRoot, "lib/git.js")));
const fixture = mkdtempSync(join(tmpdir(), "tilewright-release-git-auth-"));
try {
  const bin = join(fixture, "bin");
  mkdirSync(bin);
  writeFileSync(join(bin, "git"), `#!/usr/bin/env node
const args = process.argv.slice(2);
if (args.slice(0, 4).join(' ') !== 'push --dry-run --no-verify --' || args[5] !== 'HEAD:main') process.exit(2);
const url = new URL(args[4]);
const accepted = url.hostname === 'github.com' && url.username === 'x-access-token' && url.password === 'fixture-installation-token';
process.exit(accepted ? 0 : 1);
`, {mode: 0o700});
  const env = {PATH: bin + ":" + process.env.PATH, HOME: fixture, GIT_CONFIG_NOSYSTEM: "1",
    GITHUB_ACTION: "release_fixture", GH_TOKEN: "fixture-installation-token"};
  const context = {cwd: fixture, env, branch: {name: "main"},
    options: {repositoryUrl: "https://github.com/seraphinastudios/tilewright-renderer"}};
  const broken = await getGitAuthUrl(context);
  // GH_TOKEN alone formats an installation token as a username, which GitHub
  // rejects even though the same token works with the REST API.
  assert.notEqual(new URL(broken).username, "x-access-token");
  await assert.rejects(verifyAuth(broken, "main", context));
  const repaired = {...context, env: {...env, GITHUB_TOKEN: env.GH_TOKEN}};
  const authenticated = await getGitAuthUrl(repaired);
  assert.equal(new URL(authenticated).username, "x-access-token");
  assert.equal(new URL(authenticated).password, env.GH_TOKEN);
  await verifyAuth(authenticated, "main", repaired);
  console.log("pinned semantic-release Git authentication: GH_TOKEN regression rejected; Actions GITHUB_TOKEN accepted");
} finally {
  rmSync(fixture, {recursive: true, force: true});
}
