// Builds the Tauri updater manifest (latest.json) from the .sig files
// already attached to a release. Run exactly once, after every platform
// build has finished, so there is no concurrent read-modify-write of the
// manifest (which is what tauri-action does per matrix job and what
// races with "Not Found").
//
// Usage: TAG=v1.2.3 REPO=owner/name node scripts/make-latest-json.mjs <sigs-dir> <out-file>
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const { TAG, REPO } = process.env;
const [sigsDir, outFile] = process.argv.slice(2);
if (!TAG || !REPO || !sigsDir || !outFile) {
  console.error("Usage: TAG=... REPO=... node make-latest-json.mjs <sigs-dir> <out-file>");
  process.exit(1);
}

// Asset-name pattern -> updater platform keys. The Tauri updater tries
// "<os>-<arch>-<installer>" first, then falls back to "<os>-<arch>".
const RULES = [
  { re: /_x64-setup\.exe$/, keys: ["windows-x86_64", "windows-x86_64-nsis"] },
  { re: /_x64_en-US\.msi$/, keys: ["windows-x86_64-msi"] },
  { re: /_aarch64\.app\.tar\.gz$/, keys: ["darwin-aarch64", "darwin-aarch64-app"] },
  { re: /_(x64|x86_64)\.app\.tar\.gz$/, keys: ["darwin-x86_64", "darwin-x86_64-app"] },
  { re: /_amd64\.AppImage$/, keys: ["linux-x86_64", "linux-x86_64-appimage"] },
];

const platforms = {};
for (const sigFile of readdirSync(sigsDir).filter((f) => f.endsWith(".sig"))) {
  const asset = sigFile.slice(0, -".sig".length);
  const rule = RULES.find((r) => r.re.test(asset));
  if (!rule) {
    console.log(`skip (no platform rule): ${asset}`);
    continue;
  }
  const entry = {
    signature: readFileSync(join(sigsDir, sigFile), "utf8").trim(),
    url: `https://github.com/${REPO}/releases/download/${TAG}/${encodeURIComponent(asset)}`,
  };
  for (const key of rule.keys) {
    // Never silently overwrite: two assets claiming one key is a bug.
    if (platforms[key]) throw new Error(`Duplicate platform key ${key} (${asset})`);
    platforms[key] = entry;
  }
}

// Every platform we ship must be present. A missing one means a desktop
// build dropped its signature; refuse to write a manifest that would
// silently leave those users without updates (the release stays a draft).
const REQUIRED = ["windows-x86_64", "linux-x86_64", "darwin-aarch64", "darwin-x86_64"];
const missing = REQUIRED.filter((k) => !platforms[k]);
if (missing.length) {
  throw new Error(`Missing updater platforms: ${missing.join(", ")}; refusing to write latest.json`);
}

writeFileSync(
  outFile,
  JSON.stringify(
    {
      version: TAG.replace(/^v/, ""),
      notes: "See the release notes on GitHub.",
      pub_date: new Date().toISOString(),
      platforms,
    },
    null,
    2,
  ) + "\n",
);
console.log(`Wrote ${outFile} with platforms: ${Object.keys(platforms).join(", ")}`);
