import assert from "node:assert/strict";
import {execFileSync} from "node:child_process";
import {copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync} from "node:fs";
import {tmpdir} from "node:os";
import {dirname, join} from "node:path";
import {fileURLToPath} from "node:url";

const scripts = dirname(fileURLToPath(import.meta.url));
for (const [branch, message, pending, expected] of [
  ["main", "docs: refresh documentation", false, "8.4.0"],
  ["main", "feat!: change the contract", false, "9.0.0"],
  ["8.3.x", "fix: repair a regression", false, "8.3.1"],
  ["main", "untyped message", false, null],
  ["main", "untyped message", true, "8.3.1"],
]) {
  const fixture = mkdtempSync(join(tmpdir(), "tilewright-source-version-test-"));
  try {
    mkdirSync(join(fixture, "scripts/release"), {recursive: true});
    mkdirSync(join(fixture, "tools"));
    for (const name of ["source-version.mjs", "release-policy.cjs", "release-rules.cjs"]) {
      copyFileSync(join(scripts, name), join(fixture, "scripts/release", name));
    }
    writeFileSync(join(fixture, ".releaserc.json"), JSON.stringify({plugins: [
      ["@semantic-release/commit-analyzer", {preset: "conventionalcommits", releaseRules: "./scripts/release/release-rules.cjs"}],
    ]}));
    writeFileSync(join(fixture, "tools/crate_prepare.py"), `print('{"pending":${pending}}')\n`);
    const git = (...args) => execFileSync("git", args, {cwd: fixture, stdio: "pipe"});
    git("init", "--quiet");
    git("config", "user.name", "Release fixture");
    git("config", "user.email", "release-fixture@example.invalid");
    git("config", "commit.gpgsign", "false");
    git("add", ".");
    git("commit", "--quiet", "-m", "feat: baseline");
    git("tag", "v8.3.0");
    git("commit", "--quiet", "--allow-empty", "-m", message);
    const result = JSON.parse(execFileSync(process.execPath, ["scripts/release/source-version.mjs"], {
      cwd: fixture, encoding: "utf8", env: {...process.env, TILEWRIGHT_RELEASE_BRANCH: branch},
    }));
    assert.equal(result.version, expected);
    assert.equal(result.source_tag, expected === null ? null : "v" + expected);
    assert.match(result.revision, /^[0-9a-f]{40}$/);
  } finally {
    rmSync(fixture, {recursive: true, force: true});
  }
}
console.log("source version fixtures: mainline, maintenance, breaking and pending crate cases passed");
