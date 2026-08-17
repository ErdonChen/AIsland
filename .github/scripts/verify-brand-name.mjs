import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";

const legacyStem = ["ai", "celand"].join("");
const legacyName = new RegExp(`${legacyStem}|aice\\s*land`, "i");
const files = execFileSync(
  "git",
  ["ls-files", "--cached", "--others", "--exclude-standard", "-z"],
  { encoding: "utf8" },
)
  .split("\0")
  .filter((file) => file && existsSync(file));

const pathViolations = files.filter((file) => legacyName.test(file));
const contentViolations = [];

for (const file of files) {
  const buffer = readFileSync(file);
  if (buffer.includes(0)) continue;

  const lines = buffer.toString("utf8").split(/\r?\n/);
  const lineIndex = lines.findIndex((line) => legacyName.test(line));
  if (lineIndex >= 0) contentViolations.push(`${file}:${lineIndex + 1}`);
}

assert.deepEqual(pathViolations, [], `legacy brand spelling remains in paths: ${pathViolations.join(", ")}`);
assert.deepEqual(
  contentViolations,
  [],
  `legacy brand spelling remains in files: ${contentViolations.join(", ")}`,
);

const packageJson = JSON.parse(readFileSync("package.json", "utf8"));
const tauriConfig = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8"));
const cargoToml = readFileSync("src-tauri/Cargo.toml", "utf8");
const rustMain = readFileSync("src-tauri/src/main.rs", "utf8");

assert.equal(packageJson.name, "aisland");
assert.equal(tauriConfig.productName, "AIsland");
assert.equal(tauriConfig.mainBinaryName, "AIsland");
assert.equal(tauriConfig.identifier, "com.aisland.app");
assert.match(cargoToml, /^name = "aisland"$/m);
assert.match(cargoToml, /^name = "aisland_lib"$/m);
assert.match(rustMain, /\baisland_lib::run\(\)/);

for (const hook of [
  "aisland-config-wsl.sh",
  "aisland-profile-event-windows.ps1",
  "aisland-status-windows.ps1",
  "aisland-status-wsl.sh",
]) {
  assert.ok(existsSync(`src-tauri/agent-hooks/${hook}`), `missing renamed hook: ${hook}`);
}

console.log(`Verified AIsland naming across ${files.length} repository files.`);
