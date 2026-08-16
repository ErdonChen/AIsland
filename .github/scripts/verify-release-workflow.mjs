import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const workflowPath = resolve(scriptDirectory, "../workflows/release-windows.yml");

test("the Windows release workflow fails closed and keeps verified updater assets private by default", async () => {
  const workflow = await readFile(workflowPath, "utf8");

  assert.match(workflow, /tags:\s*\n\s*- ["']v\*["']/);
  assert.match(workflow, /workflow_dispatch:/);
  assert.match(workflow, /publish:\s*\n(?:\s+.*\n)*?\s+type:\s*boolean(?:\s*\n(?:\s+.*\n)*?\s+default:\s*false)?/);
  assert.match(workflow, /contents:\s*write/);
  assert.match(workflow, /package\.json/);
  assert.match(workflow, /src-tauri[\\/]tauri\.conf\.json/);
  assert.match(workflow, /src-tauri[\\/]Cargo\.toml/);
  assert.match(workflow, /Version mismatch:/);
  assert.match(workflow, /TAURI_SIGNING_PRIVATE_KEY:\s*\$\{\{ secrets\.TAURI_SIGNING_PRIVATE_KEY \}\}/);
  assert.match(workflow, /TAURI_SIGNING_PRIVATE_KEY_PASSWORD:\s*\$\{\{ secrets\.TAURI_SIGNING_PRIVATE_KEY_PASSWORD \}\}/);
  assert.match(workflow, /AUTHENTICODE_RELEASE_ENABLED:\s*\$\{\{ vars\.AUTHENTICODE_RELEASE_ENABLED \}\}/);
  assert.match(workflow, /AUTHENTICODE_RELEASE_ENABLED -ne ["']true["']/);
  assert.match(workflow, /createUpdaterArtifacts/);
  assert.match(workflow, /https:\/\/github\.com\/ErdonChen\/AIsland\/releases\/latest\/download\/latest\.json/);
  assert.match(workflow, /uploadUpdaterJson:\s*true/);
  assert.match(workflow, /uploadUpdaterSignatures:\s*true/);
  assert.match(workflow, /updaterJsonPreferNsis:\s*true/);
  assert.match(workflow, /releaseName:\s*AIsland v__VERSION__/);
  assert.match(workflow, /--bundles nsis/);
  assert.match(workflow, /latest\.json/);
  assert.match(workflow, /\.exe\.sig/);
  assert.match(workflow, /Get-AuthenticodeSignature/);
  assert.match(workflow, /SignatureStatus\]::Valid/);
  assert.match(workflow, /TimeStamperCertificate/);

  const draftIndex = workflow.indexOf("releaseDraft: true");
  const authenticodeIndex = workflow.indexOf("Verify trusted Authenticode signatures");
  const verificationIndex = workflow.indexOf("Verify updater assets before publication");
  const publicationIndex = workflow.indexOf("gh release edit $env:RELEASE_TAG --draft=false --latest");
  assert.ok(draftIndex >= 0, "the release must start as a draft");
  assert.ok(authenticodeIndex > draftIndex, "Authenticode must be verified after the draft build");
  assert.ok(verificationIndex > authenticodeIndex, "updater assets must be checked only after Authenticode passes");
  assert.ok(verificationIndex > draftIndex, "assets must be verified after the draft build");
  assert.ok(publicationIndex > verificationIndex, "only verified assets may be published");
  const guardedPublication = workflow.slice(verificationIndex, publicationIndex);
  assert.match(
    guardedPublication,
    /if:\s*\$\{\{\s*github\.event_name == 'workflow_dispatch' && inputs\.publish == true\s*\}\}/,
    "publication must require an explicit manual publish choice",
  );
  assert.doesNotMatch(workflow, /-----BEGIN (?:OPENSSH |RSA )?PRIVATE KEY-----/);
});
