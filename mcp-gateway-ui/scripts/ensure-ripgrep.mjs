import { createHash } from "crypto";
import { spawnSync } from "child_process";
import { chmod, mkdir, readFile, readdir, rm, writeFile } from "fs/promises";
import { existsSync } from "fs";
import https from "https";
import { basename, dirname, join, resolve } from "path";
import { fileURLToPath } from "url";

const scriptDir = fileURLToPath(new URL(".", import.meta.url));
const uiRoot = resolve(scriptDir, "..");
const repoRoot = resolve(uiRoot, "..");
const bundleRoot = resolve(repoRoot, "mcp-gateway", ".bundle-tools", "ripgrep");
const downloadsRoot = resolve(repoRoot, "mcp-gateway", ".bundle-tools", "downloads");
const apiUrl = "https://api.github.com/repos/BurntSushi/ripgrep/releases/latest";

const targets = {
  "win32:x64": {
    target: "x86_64-pc-windows-msvc",
    assetSuffix: "x86_64-pc-windows-msvc.zip",
    binName: "rg.exe",
    archiveKind: "zip",
  },
  "win32:arm64": {
    target: "aarch64-pc-windows-msvc",
    assetSuffix: "aarch64-pc-windows-msvc.zip",
    binName: "rg.exe",
    archiveKind: "zip",
  },
  "darwin:x64": {
    target: "x86_64-apple-darwin",
    assetSuffix: "x86_64-apple-darwin.tar.gz",
    binName: "rg",
    archiveKind: "tar.gz",
  },
  "darwin:arm64": {
    target: "aarch64-apple-darwin",
    assetSuffix: "aarch64-apple-darwin.tar.gz",
    binName: "rg",
    archiveKind: "tar.gz",
  },
  "linux:x64": {
    target: "x86_64-unknown-linux-musl",
    assetSuffix: "x86_64-unknown-linux-musl.tar.gz",
    binName: "rg",
    archiveKind: "tar.gz",
  },
  "linux:arm64": {
    target: "aarch64-unknown-linux-gnu",
    assetSuffix: "aarch64-unknown-linux-gnu.tar.gz",
    binName: "rg",
    archiveKind: "tar.gz",
  },
};

export async function ensureRipgrep() {
  const target = resolveTarget();
  const dest = resolve(bundleRoot, target.target, target.binName);

  if (existsSync(dest)) {
    return { path: dest, target: target.target, reused: true };
  }

  const release = await requestJson(apiUrl);
  const asset = release.assets?.find((item) =>
    item.name.startsWith("ripgrep-") && item.name.endsWith(`-${target.assetSuffix}`)
  );
  if (!asset) {
    throw new Error(`Could not find ripgrep release asset for ${target.assetSuffix}`);
  }

  const checksumAsset = release.assets?.find((item) => item.name === `${asset.name}.sha256`);
  if (!checksumAsset) {
    throw new Error(`Could not find checksum for ${asset.name}`);
  }

  await mkdir(downloadsRoot, { recursive: true });
  const archive = resolve(downloadsRoot, asset.name);
  const checksumFile = `${archive}.sha256`;
  await download(asset.browser_download_url, archive);
  await download(checksumAsset.browser_download_url, checksumFile);
  await verifySha256(archive, checksumFile);

  const extractDir = resolve(downloadsRoot, `${basename(asset.name)}.extract`);
  await rm(extractDir, { recursive: true, force: true });
  await mkdir(extractDir, { recursive: true });
  extractArchive(target.archiveKind, archive, extractDir);

  const rgPath = await findFile(extractDir, target.binName);
  if (!rgPath) {
    throw new Error(`Downloaded ripgrep archive did not contain ${target.binName}`);
  }

  await mkdir(dirname(dest), { recursive: true });
  await writeFile(dest, await readFile(rgPath));
  if (process.platform !== "win32") {
    await chmod(dest, 0o755);
  }
  await rm(extractDir, { recursive: true, force: true });

  return { path: dest, target: target.target, reused: false };
}

function resolveTarget() {
  const key = `${process.platform}:${process.arch}`;
  const target = targets[key];
  if (!target) {
    throw new Error(`Unsupported platform for bundled ripgrep: ${key}`);
  }
  return target;
}

async function requestJson(url) {
  const file = await requestBuffer(url, {
    Accept: "application/vnd.github+json",
  });
  return JSON.parse(file.toString("utf8"));
}

async function download(url, filePath) {
  await mkdir(dirname(filePath), { recursive: true });
  const buffer = await requestBuffer(url);
  await writeFile(filePath, buffer);
}

function requestBuffer(url, headers = {}) {
  return new Promise((resolvePromise, reject) => {
    const request = https.get(
      url,
      {
        headers: {
          "User-Agent": "mcp-gateway-dev",
          ...headers,
        },
      },
      (response) => {
        if (
          response.statusCode >= 300 &&
          response.statusCode < 400 &&
          response.headers.location
        ) {
          response.resume();
          requestBuffer(response.headers.location, headers).then(resolvePromise, reject);
          return;
        }

        if (response.statusCode !== 200) {
          response.resume();
          reject(new Error(`HTTP ${response.statusCode} while downloading ${url}`));
          return;
        }

        const chunks = [];
        response.on("data", (chunk) => chunks.push(chunk));
        response.on("end", () => resolvePromise(Buffer.concat(chunks)));
      },
    );
    request.on("error", reject);
  });
}

async function verifySha256(filePath, checksumFile) {
  const checksumText = await readFile(checksumFile, "utf8");
  const expected = checksumText.match(/\b[a-fA-F0-9]{64}\b/)?.[0]?.toLowerCase();
  if (!expected) {
    throw new Error(`Could not parse checksum for ${basename(filePath)}`);
  }
  const actual = createHash("sha256")
    .update(await readFile(filePath))
    .digest("hex")
    .toLowerCase();
  if (actual !== expected) {
    throw new Error(`Checksum mismatch for ${basename(filePath)}`);
  }
}

function extractArchive(kind, archive, destDir) {
  if (kind === "zip") {
    const result = spawnSync(
      "powershell",
      [
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        `Expand-Archive -LiteralPath ${quotePowerShell(archive)} -DestinationPath ${quotePowerShell(destDir)} -Force`,
      ],
      { stdio: "inherit" },
    );
    if (result.status !== 0) {
      throw new Error("Failed to extract ripgrep zip archive");
    }
    return;
  }

  const result = spawnSync("tar", ["-xzf", archive, "-C", destDir], { stdio: "inherit" });
  if (result.status !== 0) {
    throw new Error("Failed to extract ripgrep tar archive");
  }
}

function quotePowerShell(value) {
  return `'${String(value).replace(/'/g, "''")}'`;
}

async function findFile(root, fileName) {
  for (const entry of await readdir(root, { withFileTypes: true })) {
    const fullPath = join(root, entry.name);
    if (entry.isFile() && entry.name === fileName) {
      return fullPath;
    }
    if (entry.isDirectory()) {
      const nested = await findFile(fullPath, fileName);
      if (nested) {
        return nested;
      }
    }
  }
  return null;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const result = await ensureRipgrep();
  console.log(`${result.reused ? "Using cached" : "Downloaded"} ripgrep: ${result.path}`);
}
