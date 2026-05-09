import { readFileSync, writeFileSync } from "fs";
import { fileURLToPath } from "url";
import { resolve } from "path";

const scriptDir = fileURLToPath(new URL(".", import.meta.url));
const rootDir = resolve(scriptDir, "..");
const packageJsonPath = resolve(rootDir, "package.json");
const packageLockPath = resolve(rootDir, "package-lock.json");
const cargoTomlPath = resolve(rootDir, "src-tauri", "Cargo.toml");
const tauriConfigPath = resolve(rootDir, "src-tauri", "tauri.conf.json");

function resolveVersion() {
  const envVersion = process.env.VITE_APP_VERSION
    || process.env.APP_VERSION
    || process.env.GITHUB_REF_NAME;
  if (envVersion?.trim()) {
    return normalizeVersion(envVersion);
  }

  const pkg = JSON.parse(readFileSync(packageJsonPath, "utf8"));
  return normalizeVersion(pkg.version);
}

function normalizeVersion(input) {
  return String(input).trim().replace(/^v/, "");
}

function updatePackageJson(version) {
  const pkg = JSON.parse(readFileSync(packageJsonPath, "utf8"));
  if (pkg.version === version) {
    return;
  }
  pkg.version = version;
  writeFileSync(packageJsonPath, `${JSON.stringify(pkg, null, 2)}\n`);
}

function updatePackageLock(version) {
  try {
    const lock = JSON.parse(readFileSync(packageLockPath, "utf8"));
    let changed = false;

    if (lock.version !== version) {
      lock.version = version;
      changed = true;
    }

    if (lock.packages?.[""]?.version !== version) {
      lock.packages[""].version = version;
      changed = true;
    }

    if (changed) {
      writeFileSync(packageLockPath, `${JSON.stringify(lock, null, 2)}\n`);
    }
  } catch {
    // Ignore missing lockfile or invalid JSON.
  }
}

function updateCargoToml(version) {
  const content = readFileSync(cargoTomlPath, "utf8");
  const next = content.replace(
    /^version = ".*"$/m,
    `version = "${version}"`,
  );
  if (next !== content) {
    writeFileSync(cargoTomlPath, next);
  }
}

function updateTauriConfig(version) {
  const config = JSON.parse(readFileSync(tauriConfigPath, "utf8"));
  if (config.version === version) {
    return;
  }
  config.version = version;
  writeFileSync(tauriConfigPath, `${JSON.stringify(config, null, 2)}\n`);
}

const version = resolveVersion();
updatePackageJson(version);
updatePackageLock(version);
updateCargoToml(version);
updateTauriConfig(version);
console.log(`Synced app version to ${version}`);
