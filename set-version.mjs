#!/usr/bin/env node

import { readFileSync, writeFileSync } from "node:fs";
import { relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = fileURLToPath(new URL(".", import.meta.url));
const rootDir = resolve(scriptDir);

const version = normalizeVersion(process.argv[2]);
if (!version) {
  fail("Usage: node set-version.mjs <version>");
}

if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(version)) {
  fail(`Invalid version "${version}". Expected semver like 0.1.2 or v0.1.2.`);
}

const paths = {
  gatewayCargoToml: resolve(rootDir, "mcp-gateway", "Cargo.toml"),
  gatewayCargoLock: resolve(rootDir, "mcp-gateway", "Cargo.lock"),
  uiPackageJson: resolve(rootDir, "mcp-gateway-ui", "package.json"),
  uiPackageLock: resolve(rootDir, "mcp-gateway-ui", "package-lock.json"),
  tauriConfig: resolve(rootDir, "mcp-gateway-ui", "src-tauri", "tauri.conf.json"),
  tauriCargoToml: resolve(rootDir, "mcp-gateway-ui", "src-tauri", "Cargo.toml"),
  tauriCargoLock: resolve(rootDir, "mcp-gateway-ui", "src-tauri", "Cargo.lock"),
};

const changed = [];

updateTextFile(paths.gatewayCargoToml, (content) =>
  replaceRequired(
    content,
    /^version = ".*"$/m,
    `version = "${version}"`,
    "mcp-gateway workspace version",
  ),
);

updateCargoLockPackages(paths.gatewayCargoLock, [
  "gateway-cli",
  "gateway-core",
  "gateway-http",
  "gateway-integration-tests",
]);

updateJsonFile(paths.uiPackageJson, (json) => {
  json.version = version;
  return json;
});

updateJsonFile(paths.uiPackageLock, (json) => {
  json.version = version;
  if (json.packages?.[""]) {
    json.packages[""].version = version;
  }
  return json;
});

updateJsonFile(paths.tauriConfig, (json) => {
  json.version = version;
  return json;
});

updateTextFile(paths.tauriCargoToml, (content) =>
  replaceRequired(
    content,
    /^version = ".*"$/m,
    `version = "${version}"`,
    "Tauri Cargo package version",
  ),
);

updateCargoLockPackages(paths.tauriCargoLock, [
  "gateway-core",
  "gateway-http",
  "mcp-gateway-ui",
]);

if (changed.length === 0) {
  console.log(`Version is already ${version}.`);
} else {
  console.log(`Updated version to ${version}:`);
  for (const filePath of changed) {
    console.log(`- ${relativePath(filePath)}`);
  }
}

function normalizeVersion(input) {
  const value = String(input ?? "").trim();
  return value.startsWith("v") ? value.slice(1) : value;
}

function updateJsonFile(filePath, mutate) {
  updateTextFile(filePath, (content) => {
    const json = JSON.parse(content);
    const next = `${JSON.stringify(mutate(json), null, 2)}\n`;
    return next;
  });
}

function updateCargoLockPackages(filePath, packageNames) {
  const targets = new Set(packageNames);
  updateTextFile(filePath, (content) => {
    const eol = content.includes("\r\n") ? "\r\n" : "\n";
    const lines = content.split(/\r?\n/);
    let inPackage = false;
    let isTargetPackage = false;
    let lockChanged = false;

    for (let index = 0; index < lines.length; index += 1) {
      const line = lines[index];

      if (line === "[[package]]") {
        inPackage = true;
        isTargetPackage = false;
        continue;
      }

      if (!inPackage) {
        continue;
      }

      const nameMatch = line.match(/^name = "([^"]+)"$/);
      if (nameMatch) {
        isTargetPackage = targets.has(nameMatch[1]);
        continue;
      }

      if (isTargetPackage && /^version = "[^"]+"$/.test(line)) {
        const nextLine = `version = "${version}"`;
        if (line !== nextLine) {
          lines[index] = nextLine;
          lockChanged = true;
        }
        isTargetPackage = false;
      }
    }

    return lockChanged ? lines.join(eol) : content;
  });
}

function updateTextFile(filePath, transform) {
  const current = readFileSync(filePath, "utf8");
  const next = transform(current);
  if (next !== current) {
    writeFileSync(filePath, next);
    changed.push(filePath);
  }
}

function replaceRequired(content, pattern, replacement, label) {
  if (!pattern.test(content)) {
    fail(`Could not find ${label}.`);
  }
  return content.replace(pattern, replacement);
}

function relativePath(filePath) {
  return relative(rootDir, filePath).replaceAll("\\", "/");
}

function fail(message) {
  console.error(message);
  process.exit(1);
}
