import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const workflowPath = resolve(scriptDirectory, "../workflows/release-windows-preview.yml");

test("the unsigned preview workflow stays isolated from stable releases", async () => {
  const workflow = await readFile(workflowPath, "utf8");

  assert.match(workflow, /workflow_dispatch:/);
  assert.doesNotMatch(workflow, /\n\s+push:/);
  assert.match(workflow, /publish:\s*\n(?:\s+.*\n)*?\s+type:\s*boolean(?:\s*\n(?:\s+.*\n)*?\s+default:\s*false)?/);
  assert.match(workflow, /contents:\s*write/);
  assert.match(workflow, /runs-on:\s*windows-2022/);
  assert.match(workflow, /PREVIEW_SOURCE_REF:\s*\$\{\{\s*github\.ref\s*\}\}/);
  assert.match(workflow, /refs\/heads\/main/);
  assert.match(workflow, /\^preview-v\(\?<version>/);
  assert.match(workflow, /Version mismatch:/);
  assert.match(workflow, /--bundles nsis/);
  assert.match(workflow, /createUpdaterArtifacts\?\":false|createUpdaterArtifacts\":false/);
  assert.match(workflow, /SignatureStatus\]::NotSigned/);
  assert.match(workflow, /Expected exactly one NSIS installer/);
  assert.match(workflow, /Get-FileHash[^\n]+SHA256/);
  assert.match(workflow, /SHA256SUMS\.txt/);
  assert.match(workflow, /gh release create/);
  assert.match(workflow, /--draft/);
  assert.match(workflow, /--prerelease/);
  assert.match(workflow, /--latest=false/);
  assert.match(workflow, /gh release edit[^\n]+--draft=false --prerelease/);
  assert.match(workflow, /if:\s*\$\{\{\s*inputs\.publish == true\s*\}\}/);
  assert.doesNotMatch(workflow, /AUTHENTICODE_RELEASE_ENABLED/);
  assert.doesNotMatch(workflow, /TAURI_SIGNING_PRIVATE_KEY/);
  assert.doesNotMatch(workflow, /uploadUpdaterJson/);

  const verifyIndex = workflow.indexOf("Verify the unsigned installer and generate its checksum");
  const createIndex = workflow.indexOf("Create the verified draft Pre-release");
  const publishIndex = workflow.indexOf("Publish the verified Pre-release");
  const downloadIndex = workflow.indexOf("Verify release metadata and downloaded assets");
  assert.ok(verifyIndex >= 0, "the unsigned installer must be verified");
  assert.ok(createIndex > verifyIndex, "the draft release must be created after verification");
  assert.ok(publishIndex > createIndex, "publication must happen after the draft is created");
  assert.ok(downloadIndex > publishIndex, "uploaded assets must be verified after publication handling");
  assert.doesNotMatch(workflow, /-----BEGIN (?:OPENSSH |RSA )?PRIVATE KEY-----/);
});
