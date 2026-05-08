import { spawn } from "child_process";
import { resolve } from "path";
import { fileURLToPath } from "url";

import { ensureRipgrep } from "./ensure-ripgrep.mjs";

const scriptDir = fileURLToPath(new URL(".", import.meta.url));
const uiRoot = resolve(scriptDir, "..");
const tauriCli = resolve(uiRoot, "node_modules", "@tauri-apps", "cli", "tauri.js");

const ripgrep = await ensureRipgrep();
console.log(`${ripgrep.reused ? "Using cached" : "Downloaded"} ripgrep: ${ripgrep.path}`);

const child = spawn(process.execPath, [tauriCli, ...process.argv.slice(2)], {
  cwd: uiRoot,
  env: {
    ...process.env,
    MCP_GATEWAY_BUNDLED_RG_PATH: ripgrep.path,
  },
  stdio: "inherit",
});

child.on("exit", (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code ?? 1);
});

child.on("error", (error) => {
  console.error(error);
  process.exit(1);
});
