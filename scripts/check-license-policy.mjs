import { spawnSync } from "node:child_process";
import { readdir, readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const allowedTokens = new Set([
  "0BSD",
  "Apache-2.0",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "BSL-1.0",
  "CC0-1.0",
  "CC-BY-4.0",
  "ISC",
  "LLVM-exception",
  "MIT",
  "MIT-0",
  "MPL-2.0",
  "BlueOak-1.0.0",
  "Unicode-3.0",
  "Unicode-DFS-2016",
  "Unlicense",
  "Zlib",
]);
const expressionOperators = new Set(["AND", "OR", "WITH"]);
const deniedPattern = /(?:AGPL|SSPL|BUSL|Commons-Clause|CC-BY-NC|CC-BY-ND|LicenseRef|\bGPL(?:-|\b)|noncommercial)/i;

function validateLicense(name, version, expression, source) {
  if (typeof expression !== "string" || expression.trim() === "") {
    return `${source}: ${name}@${version} has no license metadata`;
  }
  if (deniedPattern.test(expression)) {
    return `${source}: ${name}@${version} uses denied license expression ${expression}`;
  }

  const tokens = expression.match(/[A-Za-z0-9.+-]+/g) ?? [];
  const unknown = tokens.filter(
    (token) => !expressionOperators.has(token) && !allowedTokens.has(token),
  );
  if (unknown.length > 0) {
    return `${source}: ${name}@${version} has unreviewed license token(s) ${[...new Set(unknown)].join(", ")} in ${expression}`;
  }
  return null;
}

function cargoPackages() {
  const cargo = process.env.CARGO || (process.platform === "win32" ? "cargo.exe" : "cargo");
  const result = spawnSync(
    cargo,
    [
      "metadata",
      "--manifest-path",
      join(repositoryRoot, "src-tauri", "Cargo.toml"),
      "--locked",
      "--format-version",
      "1",
      "--filter-platform",
      "x86_64-pc-windows-msvc",
    ],
    { cwd: repositoryRoot, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 },
  );
  if (result.error) {
    throw new Error(`could not start cargo metadata: ${result.error.message}`);
  }
  if (result.status !== 0) {
    throw new Error(`cargo metadata failed: ${(result.stderr ?? "no stderr output").trim()}`);
  }
  return JSON.parse(result.stdout).packages.map((pkg) => ({
    name: pkg.name,
    version: pkg.version,
    license: pkg.license,
  }));
}

async function findPackageManifests(root) {
  const found = [];
  async function walk(directory) {
    const entries = await readdir(directory, { withFileTypes: true });
    for (const entry of entries) {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) {
        await walk(path);
      } else if (entry.isFile() && entry.name === "package.json") {
        found.push(path);
      }
    }
  }
  await walk(root);
  return found;
}

async function npmPackages() {
  const storeRoot = join(repositoryRoot, "node_modules", ".pnpm");
  const manifests = await findPackageManifests(storeRoot);
  const unique = new Map();
  for (const manifestPath of manifests) {
    const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
    if (typeof manifest.name !== "string" || typeof manifest.version !== "string") {
      continue;
    }
    const license = typeof manifest.license === "object" ? manifest.license?.type : manifest.license;
    unique.set(`${manifest.name}@${manifest.version}`, {
      name: manifest.name,
      version: manifest.version,
      license,
    });
  }
  return [...unique.values()];
}

const cargo = cargoPackages();
const npm = await npmPackages();
const failures = [
  ...cargo.map((pkg) => validateLicense(pkg.name, pkg.version, pkg.license, "cargo")),
  ...npm.map((pkg) => validateLicense(pkg.name, pkg.version, pkg.license, "npm")),
].filter(Boolean);

if (failures.length > 0) {
  console.error("Dependency license policy failed:");
  for (const failure of failures) console.error(`- ${failure}`);
  process.exitCode = 1;
} else {
  console.log(`Dependency license policy passed: ${cargo.length} Rust packages, ${npm.length} npm packages.`);
}
