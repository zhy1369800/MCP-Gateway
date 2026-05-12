#!/usr/bin/env node
// cdp - lightweight Chrome DevTools Protocol CLI
// Uses raw CDP over WebSocket, no Puppeteer dependency.
// Requires Node 22+ (built-in WebSocket).
//
// Per-tab persistent daemon: page commands go through a daemon that holds
// the CDP session open. Chrome's "Allow debugging" modal fires once per
// daemon (= once per tab). Daemons auto-exit after 20min idle.

import { cpSync, mkdirSync, readFileSync, rmSync, unlinkSync, writeFileSync, existsSync } from 'fs';
import { homedir } from 'os';
import { resolve } from 'path';
import { spawn } from 'child_process';
import net from 'net';

const TIMEOUT = 15000;
const NAVIGATION_TIMEOUT = 30000;
const IDLE_TIMEOUT = 20 * 60 * 1000;
const DAEMON_CONNECT_RETRIES = 20;
const DAEMON_CONNECT_DELAY = 300;
const MIN_TARGET_PREFIX_LEN = 8;
const IS_WINDOWS = process.platform === 'win32';
if (!IS_WINDOWS) process.umask(0o077);
const RUNTIME_DIR = process.env.CDP_RUNTIME_DIR ? resolve(process.env.CDP_RUNTIME_DIR) : IS_WINDOWS
  ? resolve(process.env.LOCALAPPDATA || resolve(homedir(), 'AppData', 'Local'), 'cdp')
  : process.env.XDG_RUNTIME_DIR
    ? resolve(process.env.XDG_RUNTIME_DIR, 'cdp')
    : resolve(homedir(), '.cache', 'cdp');
try { mkdirSync(RUNTIME_DIR, { recursive: true, mode: 0o700 }); } catch {}
const PAGES_CACHE = resolve(RUNTIME_DIR, 'pages.json');
const ACTIVE_TARGET = resolve(RUNTIME_DIR, 'active-target.json');
const MANAGED_STATE = resolve(RUNTIME_DIR, 'managed-browser.json');
const MANAGED_PROFILES_DIR = resolve(RUNTIME_DIR, 'profiles');
const MANAGED_START_TIMEOUT = 20000;
const DEFAULT_BROWSER_MODE = (process.env.CDP_BROWSER_MODE || 'launch').toLowerCase();
const DEFAULT_PROFILE_MODE = (process.env.CDP_PROFILE_MODE || 'clone').toLowerCase();

function sockPath(targetId) {
  return IS_WINDOWS
    ? `\\\\.\\pipe\\cdp-${targetId}`
    : resolve(RUNTIME_DIR, `cdp-${targetId}.sock`);
}

function browserMode() {
  return DEFAULT_BROWSER_MODE === 'attach' ? 'attach' : 'launch';
}

function profileMode() {
  if (DEFAULT_PROFILE_MODE === 'empty') return 'empty';
  if (DEFAULT_PROFILE_MODE === 'persistent' || DEFAULT_PROFILE_MODE === 'reuse') return 'persistent';
  return 'clone';
}

function readJson(path) {
  try {
    return JSON.parse(readFileSync(path, 'utf8'));
  } catch {
    return null;
  }
}

function writeJson(path, value) {
  writeFileSync(path, JSON.stringify(value, null, 2), { mode: 0o600 });
}

function setActiveTarget(targetId) {
  if (!targetId) return;
  writeJson(ACTIVE_TARGET, { targetId, updatedAt: new Date().toISOString() });
}

function readWsUrlFromPortFile(portFile) {
  const lines = readFileSync(portFile, 'utf8').trim().split('\n');
  if (lines.length < 2 || !lines[0] || !lines[1]) throw new Error(`Invalid DevToolsActivePort file: ${portFile}`);
  const host = process.env.CDP_HOST || '127.0.0.1';
  return `ws://${host}:${lines[0]}${lines[1]}`;
}

function getAttachPortCandidates() {
  const home = homedir();
  // macOS: ~/Library/Application Support/<name>/DevToolsActivePort
  const macBrowsers = [
    'Google/Chrome', 'Google/Chrome Beta', 'Google/Chrome for Testing',
    'Chromium', 'BraveSoftware/Brave-Browser', 'Microsoft Edge',
  ];
  // Linux: ~/.config/<name>/DevToolsActivePort
  const linuxBrowsers = [
    'google-chrome', 'google-chrome-beta', 'chromium',
    'vivaldi', 'vivaldi-snapshot',
    'BraveSoftware/Brave-Browser', 'microsoft-edge',
  ];
  const candidates = [
    process.env.CDP_PORT_FILE,
    ...macBrowsers.flatMap(b => [
      resolve(home, 'Library/Application Support', b, 'DevToolsActivePort'),
      resolve(home, 'Library/Application Support', b, 'Default/DevToolsActivePort'),
    ]),
    ...linuxBrowsers.flatMap(b => [
      resolve(home, '.config', b, 'DevToolsActivePort'),
      resolve(home, '.config', b, 'Default/DevToolsActivePort'),
    ]),
    // Windows: %LOCALAPPDATA%/<name>/User Data/DevToolsActivePort
    ...(IS_WINDOWS ? ['Google/Chrome', 'BraveSoftware/Brave-Browser', 'Microsoft/Edge'].flatMap(b => {
      const base = process.env.LOCALAPPDATA || resolve(home, 'AppData/Local');
      return [
        resolve(base, b, 'User Data/DevToolsActivePort'),
        resolve(base, b, 'User Data/Default/DevToolsActivePort'),
      ];
    }) : []),
  ];
  return candidates;
}

function getAttachWsUrl() {
  const candidates = getAttachPortCandidates().filter(Boolean);
  const portFile = candidates.find(p => existsSync(p));
  if (!portFile) throw new Error('No DevToolsActivePort found. Enable remote debugging at chrome://inspect/#remote-debugging');
  return readWsUrlFromPortFile(portFile);
}

function getBrowserInstallations() {
  const home = homedir();
  const localAppData = process.env.LOCALAPPDATA || resolve(home, 'AppData', 'Local');
  const programFiles = process.env.ProgramFiles || 'C:\\Program Files';
  const programFilesX86 = process.env['ProgramFiles(x86)'] || 'C:\\Program Files (x86)';

  return [
    {
      id: 'chrome',
      name: 'Google Chrome',
      userDataDir: IS_WINDOWS ? resolve(localAppData, 'Google', 'Chrome', 'User Data')
        : process.platform === 'darwin' ? resolve(home, 'Library', 'Application Support', 'Google', 'Chrome')
        : resolve(home, '.config', 'google-chrome'),
      executables: IS_WINDOWS ? [
        resolve(localAppData, 'Google', 'Chrome', 'Application', 'chrome.exe'),
        resolve(programFiles, 'Google', 'Chrome', 'Application', 'chrome.exe'),
        resolve(programFilesX86, 'Google', 'Chrome', 'Application', 'chrome.exe'),
      ] : process.platform === 'darwin' ? [
        '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
      ] : [
        '/usr/bin/google-chrome',
        '/usr/bin/google-chrome-stable',
      ],
    },
    {
      id: 'edge',
      name: 'Microsoft Edge',
      userDataDir: IS_WINDOWS ? resolve(localAppData, 'Microsoft', 'Edge', 'User Data')
        : process.platform === 'darwin' ? resolve(home, 'Library', 'Application Support', 'Microsoft Edge')
        : resolve(home, '.config', 'microsoft-edge'),
      executables: IS_WINDOWS ? [
        resolve(localAppData, 'Microsoft', 'Edge', 'Application', 'msedge.exe'),
        resolve(programFiles, 'Microsoft', 'Edge', 'Application', 'msedge.exe'),
        resolve(programFilesX86, 'Microsoft', 'Edge', 'Application', 'msedge.exe'),
      ] : process.platform === 'darwin' ? [
        '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge',
      ] : [
        '/usr/bin/microsoft-edge',
        '/usr/bin/microsoft-edge-stable',
      ],
    },
    {
      id: 'brave',
      name: 'Brave',
      userDataDir: IS_WINDOWS ? resolve(localAppData, 'BraveSoftware', 'Brave-Browser', 'User Data')
        : process.platform === 'darwin' ? resolve(home, 'Library', 'Application Support', 'BraveSoftware', 'Brave-Browser')
        : resolve(home, '.config', 'BraveSoftware', 'Brave-Browser'),
      executables: IS_WINDOWS ? [
        resolve(localAppData, 'BraveSoftware', 'Brave-Browser', 'Application', 'brave.exe'),
        resolve(programFiles, 'BraveSoftware', 'Brave-Browser', 'Application', 'brave.exe'),
        resolve(programFilesX86, 'BraveSoftware', 'Brave-Browser', 'Application', 'brave.exe'),
      ] : process.platform === 'darwin' ? [
        '/Applications/Brave Browser.app/Contents/MacOS/Brave Browser',
      ] : [
        '/usr/bin/brave-browser',
      ],
    },
    {
      id: 'chromium',
      name: 'Chromium',
      userDataDir: IS_WINDOWS ? resolve(localAppData, 'Chromium', 'User Data')
        : process.platform === 'darwin' ? resolve(home, 'Library', 'Application Support', 'Chromium')
        : resolve(home, '.config', 'chromium'),
      executables: IS_WINDOWS ? [
        resolve(localAppData, 'Chromium', 'Application', 'chrome.exe'),
      ] : process.platform === 'darwin' ? [
        '/Applications/Chromium.app/Contents/MacOS/Chromium',
      ] : [
        '/usr/bin/chromium',
        '/usr/bin/chromium-browser',
      ],
    },
  ];
}

function pickBrowserInstallation() {
  const preferred = (process.env.CDP_BROWSER || '').toLowerCase();
  const installations = getBrowserInstallations();
  const ordered = preferred
    ? [
        ...installations.filter(browser => browser.id === preferred),
        ...installations.filter(browser => browser.id !== preferred),
      ]
    : installations;

  for (const browser of ordered) {
    const executable = browser.executables.find(candidate => existsSync(candidate));
    if (executable) return { ...browser, executable };
  }

  throw new Error(
    'No supported Chromium browser executable found. Set CDP_BROWSER to one of: chrome, edge, brave, chromium.'
  );
}

async function waitForManagedPortFile(portFile) {
  const deadline = Date.now() + MANAGED_START_TIMEOUT;
  while (Date.now() < deadline) {
    if (existsSync(portFile)) {
      try {
        return readWsUrlFromPortFile(portFile);
      } catch {}
    }
    await sleep(250);
  }
  throw new Error(`Timed out waiting for managed browser to start: ${portFile}`);
}

function getSourceUserDataDir(browser) {
  return process.env.CDP_PROFILE_SOURCE_DIR || browser.userDataDir;
}

function cloneUserDataDir(browser, targetUserDataDir) {
  const sourceUserDataDir = getSourceUserDataDir(browser);
  if (!sourceUserDataDir) {
    throw new Error(`No source user data dir configured for ${browser.name}. Set CDP_PROFILE_SOURCE_DIR.`);
  }
  if (!existsSync(sourceUserDataDir)) {
    throw new Error(`Source browser profile not found: ${sourceUserDataDir}`);
  }

  try {
    rmSync(targetUserDataDir, { recursive: true, force: true });
    mkdirSync(targetUserDataDir, { recursive: true, mode: 0o700 });
    cpSync(sourceUserDataDir, targetUserDataDir, {
      recursive: true,
      force: true,
      verbatimSymlinks: true,
    });
  } catch (error) {
    throw new Error(
      `Failed to fully copy ${browser.name} profile from "${sourceUserDataDir}" to "${targetUserDataDir}". ` +
      `Close all ${browser.name} windows first and try again. Original error: ${error.message}`
    );
  }
}

async function launchManagedBrowser(initialUrl = 'about:blank') {
  const browser = pickBrowserInstallation();
  const userDataDir = process.env.CDP_USER_DATA_DIR || resolve(MANAGED_PROFILES_DIR, browser.id);
  const portFile = resolve(userDataDir, 'DevToolsActivePort');

  if (profileMode() === 'clone') {
    cloneUserDataDir(browser, userDataDir);
  } else if (profileMode() === 'empty') {
    try { rmSync(userDataDir, { recursive: true, force: true }); } catch {}
    try { mkdirSync(userDataDir, { recursive: true, mode: 0o700 }); } catch {}
  } else {
    try { mkdirSync(userDataDir, { recursive: true, mode: 0o700 }); } catch {}
  }
  try { unlinkSync(portFile); } catch {}

  const args = [
    '--remote-debugging-port=0',
    `--user-data-dir=${userDataDir}`,
    '--no-first-run',
    '--no-default-browser-check',
    initialUrl,
  ];

  const child = spawn(browser.executable, args, {
    detached: true,
    stdio: 'ignore',
  });
  child.unref();

  const state = {
    mode: 'launch',
    profileMode: profileMode(),
    browserId: browser.id,
    browserName: browser.name,
    executable: browser.executable,
    userDataDir,
    sourceUserDataDir: profileMode() === 'clone' ? getSourceUserDataDir(browser) : null,
    portFile,
    pid: child.pid,
    startedAt: new Date().toISOString(),
  };
  writeJson(MANAGED_STATE, state);

  await waitForManagedPortFile(portFile);
  return state;
}

function clearRuntimeState() {
  try { unlinkSync(MANAGED_STATE); } catch {}
  try { unlinkSync(PAGES_CACHE); } catch {}
  try { unlinkSync(ACTIVE_TARGET); } catch {}
}

function killManagedProcessFromState(state) {
  if (!state?.pid) return null;
  try {
    process.kill(state.pid);
    return `Killed managed browser process ${state.pid}.`;
  } catch (error) {
    return `Process kill failed for ${state.pid}: ${error.message}`;
  }
}

async function stopManagedBrowser() {
  const state = readJson(MANAGED_STATE);
  if (!state) {
    clearRuntimeState();
    return 'No managed browser state found.';
  }

  let closed = false;
  const messages = [];

  if (state.portFile && existsSync(state.portFile)) {
    try {
      const cdp = new CDP();
      await cdp.connect(readWsUrlFromPortFile(state.portFile));
      await cdp.send('Browser.close');
      cdp.close();
      closed = true;
      messages.push('Sent Browser.close to managed browser.');
    } catch (error) {
      messages.push(`Browser.close failed: ${error.message}`);
    }
  }

  if (!closed && state.pid) {
    const message = killManagedProcessFromState(state);
    if (message) {
      closed = message.startsWith('Killed ');
      messages.push(message);
    }
  }

  clearRuntimeState();
  return messages.join('\n') || 'Cleared managed browser state.';
}

async function connectToBrowserSession(session, retryInitialUrl = 'about:blank', retried = false) {
  const cdp = new CDP();
  try {
    await cdp.connect(session.wsUrl);
    return { session, cdp };
  } catch (error) {
    if (session.mode === 'launch' && !retried) {
      killManagedProcessFromState(session.state);
      clearRuntimeState();
      const freshSession = await ensureBrowser(retryInitialUrl);
      return connectToBrowserSession(freshSession, retryInitialUrl, true);
    }
    throw error;
  }
}

async function openBrowserConnection(initialUrl = 'about:blank') {
  const session = await ensureBrowser(initialUrl);
  return connectToBrowserSession(session, initialUrl);
}

async function ensureBrowser(initialUrl = 'about:blank') {
  if (process.env.CDP_PORT_FILE) {
    return {
      mode: 'port-file',
      launched: false,
      wsUrl: readWsUrlFromPortFile(process.env.CDP_PORT_FILE),
    };
  }

  if (browserMode() === 'attach') {
    return {
      mode: 'attach',
      launched: false,
      wsUrl: getAttachWsUrl(),
    };
  }

  const browser = pickBrowserInstallation();
  const desiredUserDataDir = process.env.CDP_USER_DATA_DIR || resolve(MANAGED_PROFILES_DIR, browser.id);
  const desiredSourceUserDataDir = profileMode() === 'clone' ? getSourceUserDataDir(browser) : null;
  const state = readJson(MANAGED_STATE);
  const stateMatches =
    state?.mode === 'launch' &&
    state?.browserId === browser.id &&
    state?.userDataDir === desiredUserDataDir &&
    (state?.profileMode || 'empty') === profileMode() &&
    (state?.sourceUserDataDir || null) === desiredSourceUserDataDir;
  if (stateMatches && state?.portFile && existsSync(state.portFile)) {
    try {
      return {
        mode: 'launch',
        launched: false,
        wsUrl: readWsUrlFromPortFile(state.portFile),
        state,
      };
    } catch {}
  }
  if (stateMatches) {
    killManagedProcessFromState(state);
    clearRuntimeState();
  }

  const launchedState = await launchManagedBrowser(initialUrl);
  return {
    mode: 'launch',
    launched: true,
    wsUrl: readWsUrlFromPortFile(launchedState.portFile),
    state: launchedState,
  };
}

async function cacheOpenPages(cdp) {
  const pages = await getPages(cdp);
  writeFileSync(PAGES_CACHE, JSON.stringify(pages), { mode: 0o600 });
  return pages;
}

function resolveTargetFromPages(targetPrefix, pages) {
  const active = readJson(ACTIVE_TARGET)?.targetId;
  const pageIds = pages.map(p => p.targetId);

  if (!targetPrefix || targetPrefix === 'current' || targetPrefix === 'active') {
    if (active && pageIds.includes(active)) return active;
    if (pages.length === 1) return pages[0].targetId;
    throw new Error('No active target is set. Run "open <url>" or pass a target prefix from "list".');
  }

  return resolvePrefix(targetPrefix, pageIds, 'target', 'Run "cdp list".');
}

const sleep = (ms) => new Promise(r => setTimeout(r, ms));


function resolvePrefix(prefix, candidates, noun = 'target', missingHint = '') {
  const upper = prefix.toUpperCase();
  const matches = candidates.filter(candidate => candidate.toUpperCase().startsWith(upper));
  if (matches.length === 0) {
    const hint = missingHint ? ` ${missingHint}` : '';
    throw new Error(`No ${noun} matching prefix "${prefix}".${hint}`);
  }
  if (matches.length > 1) {
    throw new Error(`Ambiguous prefix "${prefix}" — matches ${matches.length} ${noun}s. Use more characters.`);
  }
  return matches[0];
}

function getDisplayPrefixLength(targetIds) {
  if (targetIds.length === 0) return MIN_TARGET_PREFIX_LEN;
  const maxLen = Math.max(...targetIds.map(id => id.length));
  for (let len = MIN_TARGET_PREFIX_LEN; len <= maxLen; len++) {
    const prefixes = new Set(targetIds.map(id => id.slice(0, len).toUpperCase()));
    if (prefixes.size === targetIds.length) return len;
  }
  return maxLen;
}

// ---------------------------------------------------------------------------
// CDP WebSocket client
// ---------------------------------------------------------------------------

class CDP {
  #ws; #id = 0; #pending = new Map(); #eventHandlers = new Map(); #closeHandlers = [];

  async connect(wsUrl) {
    return new Promise((res, rej) => {
      this.#ws = new WebSocket(wsUrl);
      this.#ws.onopen = () => res();
      this.#ws.onerror = (e) => rej(new Error('WebSocket error: ' + (e.message || e.type)));
      this.#ws.onclose = () => this.#closeHandlers.forEach(h => h());
      this.#ws.onmessage = (ev) => {
        const msg = JSON.parse(ev.data);
        if (msg.id && this.#pending.has(msg.id)) {
          const { resolve, reject } = this.#pending.get(msg.id);
          this.#pending.delete(msg.id);
          if (msg.error) reject(new Error(msg.error.message));
          else resolve(msg.result);
        } else if (msg.method && this.#eventHandlers.has(msg.method)) {
          for (const handler of [...this.#eventHandlers.get(msg.method)]) {
            handler(msg.params || {}, msg);
          }
        }
      };
    });
  }

  send(method, params = {}, sessionId) {
    const id = ++this.#id;
    return new Promise((resolve, reject) => {
      let timer;
      this.#pending.set(id, { resolve, reject });
      const msg = { id, method, params };
      if (sessionId) msg.sessionId = sessionId;
      this.#ws.send(JSON.stringify(msg));
      timer = setTimeout(() => {
        if (this.#pending.has(id)) {
          this.#pending.delete(id);
          reject(new Error(`Timeout: ${method}`));
        }
      }, TIMEOUT);
      this.#pending.set(id, {
        resolve: (value) => {
          clearTimeout(timer);
          resolve(value);
        },
        reject: (error) => {
          clearTimeout(timer);
          reject(error);
        },
      });
    });
  }

  onEvent(method, handler) {
    if (!this.#eventHandlers.has(method)) this.#eventHandlers.set(method, new Set());
    const handlers = this.#eventHandlers.get(method);
    handlers.add(handler);
    return () => {
      handlers.delete(handler);
      if (handlers.size === 0) this.#eventHandlers.delete(method);
    };
  }

  waitForEvent(method, timeout = TIMEOUT) {
    let settled = false;
    let off;
    let timer;
    const promise = new Promise((resolve, reject) => {
      off = this.onEvent(method, (params) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        off();
        resolve(params);
      });
      timer = setTimeout(() => {
        if (settled) return;
        settled = true;
        off();
        reject(new Error(`Timeout waiting for event: ${method}`));
      }, timeout);
    });
    return {
      promise,
      cancel() {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        off?.();
      },
    };
  }

  onClose(handler) { this.#closeHandlers.push(handler); }
  close() { this.#ws.close(); }
}

// ---------------------------------------------------------------------------
// Command implementations — return strings, take (cdp, sessionId)
// ---------------------------------------------------------------------------

async function getPages(cdp) {
  const { targetInfos } = await cdp.send('Target.getTargets');
  return targetInfos.filter(t => t.type === 'page' && !t.url.startsWith('chrome://'));
}

function formatPageList(pages) {
  const prefixLen = getDisplayPrefixLength(pages.map(p => p.targetId));
  return pages.map(p => {
    const id = p.targetId.slice(0, prefixLen).padEnd(prefixLen);
    const title = p.title.substring(0, 54).padEnd(54);
    return `${id}  ${title}  ${p.url}`;
  }).join('\n');
}

function shouldShowAxNode(node, compact = false) {
  const role = node.role?.value || '';
  const name = node.name?.value ?? '';
  const value = node.value?.value;
  if (compact && role === 'InlineTextBox') return false;
  return role !== 'none' && role !== 'generic' && !(name === '' && (value === '' || value == null));
}

function formatAxNode(node, depth) {
  const role = node.role?.value || '';
  const name = node.name?.value ?? '';
  const value = node.value?.value;
  const indent = '  '.repeat(Math.min(depth, 10));
  let line = `${indent}[${role}]`;
  if (name !== '') line += ` ${name}`;
  if (!(value === '' || value == null)) line += ` = ${JSON.stringify(value)}`;
  return line;
}

function orderedAxChildren(node, nodesById, childrenByParent) {
  const children = [];
  const seen = new Set();
  for (const childId of node.childIds || []) {
    const child = nodesById.get(childId);
    if (child && !seen.has(child.nodeId)) {
      seen.add(child.nodeId);
      children.push(child);
    }
  }
  for (const child of childrenByParent.get(node.nodeId) || []) {
    if (!seen.has(child.nodeId)) {
      seen.add(child.nodeId);
      children.push(child);
    }
  }
  return children;
}

async function snapshotStr(cdp, sid, compact = false) {
  const { nodes } = await cdp.send('Accessibility.getFullAXTree', {}, sid);
  const nodesById = new Map(nodes.map(node => [node.nodeId, node]));
  const childrenByParent = new Map();
  for (const node of nodes) {
    if (!node.parentId) continue;
    if (!childrenByParent.has(node.parentId)) childrenByParent.set(node.parentId, []);
    childrenByParent.get(node.parentId).push(node);
  }

  const lines = [];
  const visited = new Set();
  function visit(node, depth) {
    if (!node || visited.has(node.nodeId)) return;
    visited.add(node.nodeId);
    if (shouldShowAxNode(node, compact)) lines.push(formatAxNode(node, depth));
    for (const child of orderedAxChildren(node, nodesById, childrenByParent)) {
      visit(child, depth + 1);
    }
  }

  const roots = nodes.filter(node => !node.parentId || !nodesById.has(node.parentId));
  for (const root of roots) visit(root, 0);
  for (const node of nodes) visit(node, 0);

  return lines.join('\n');
}

async function evalStr(cdp, sid, expression) {
  await cdp.send('Runtime.enable', {}, sid);
  const result = await cdp.send('Runtime.evaluate', {
    expression, returnByValue: true, awaitPromise: true,
  }, sid);
  if (result.exceptionDetails) {
    throw new Error(result.exceptionDetails.text || result.exceptionDetails.exception?.description);
  }
  const val = result.result.value;
  return typeof val === 'object' ? JSON.stringify(val, null, 2) : String(val ?? '');
}

async function shotStr(cdp, sid, filePath, targetId) {
  // Get device scale factor so we can report coordinate mapping
  let dpr = 1;
  try {
    const metrics = await cdp.send('Page.getLayoutMetrics', {}, sid);
    dpr = metrics.visualViewport?.clientWidth
      ? metrics.cssVisualViewport?.clientWidth
        ? Math.round((metrics.visualViewport.clientWidth / metrics.cssVisualViewport.clientWidth) * 100) / 100
        : 1
      : 1;
    // Simpler: deviceScaleFactor is on the root Page metrics
    const { deviceScaleFactor } = await cdp.send('Emulation.getDeviceMetricsOverride', {}, sid).catch(() => ({}));
    if (deviceScaleFactor) dpr = deviceScaleFactor;
  } catch {}
  // Fallback: try to get DPR from JS
  if (dpr === 1) {
    try {
      const raw = await evalStr(cdp, sid, 'window.devicePixelRatio');
      const parsed = parseFloat(raw);
      if (parsed > 0) dpr = parsed;
    } catch {}
  }

  const { data } = await cdp.send('Page.captureScreenshot', { format: 'png' }, sid);
  const out = filePath || resolve(RUNTIME_DIR, `screenshot-${(targetId || 'unknown').slice(0, 8)}.png`);
  writeFileSync(out, Buffer.from(data, 'base64'));

  const lines = [out];
  lines.push(`Screenshot saved. Device pixel ratio (DPR): ${dpr}`);
  lines.push(`Coordinate mapping:`);
  lines.push(`  Screenshot pixels → CSS pixels (for CDP Input events): divide by ${dpr}`);
  lines.push(`  e.g. screenshot point (${Math.round(100 * dpr)}, ${Math.round(200 * dpr)}) → CSS (100, 200) → use clickxy <target> 100 200`);
  if (dpr !== 1) {
    lines.push(`  On this ${dpr}x display: CSS px = screenshot px / ${dpr} ≈ screenshot px × ${Math.round(100/dpr)/100}`);
  }
  return lines.join('\n');
}

async function htmlStr(cdp, sid, selector) {
  const expr = selector
    ? `document.querySelector(${JSON.stringify(selector)})?.outerHTML || 'Element not found'`
    : `document.documentElement.outerHTML`;
  return evalStr(cdp, sid, expr);
}

async function waitForDocumentReady(cdp, sid, timeoutMs = NAVIGATION_TIMEOUT) {
  const deadline = Date.now() + timeoutMs;
  let lastState = '';
  let lastError;
  while (Date.now() < deadline) {
    try {
      const state = await evalStr(cdp, sid, 'document.readyState');
      lastState = state;
      if (state === 'complete') return;
    } catch (e) {
      lastError = e;
    }
    await sleep(200);
  }

  if (lastState) {
    throw new Error(`Timed out waiting for navigation to finish (last readyState: ${lastState})`);
  }
  if (lastError) {
    throw new Error(`Timed out waiting for navigation to finish (${lastError.message})`);
  }
  throw new Error('Timed out waiting for navigation to finish');
}

async function navStr(cdp, sid, url) {
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:')
      throw new Error(`Only http/https URLs allowed, got: ${url}`);
  } catch (e) {
    if (e.message.startsWith('Only')) throw e;
    throw new Error(`Invalid URL: ${url}`);
  }
  await cdp.send('Page.enable', {}, sid);
  const loadEvent = cdp.waitForEvent('Page.loadEventFired', NAVIGATION_TIMEOUT);
  const result = await cdp.send('Page.navigate', { url }, sid);
  if (result.errorText) {
    loadEvent.cancel();
    throw new Error(result.errorText);
  }
  if (result.loaderId) {
    await loadEvent.promise;
  } else {
    loadEvent.cancel();
  }
  await waitForDocumentReady(cdp, sid, 5000);
  return `Navigated to ${url}`;
}

async function netStr(cdp, sid) {
  const raw = await evalStr(cdp, sid, `JSON.stringify(performance.getEntriesByType('resource').map(e => ({
    name: e.name.substring(0, 120), type: e.initiatorType,
    duration: Math.round(e.duration), size: e.transferSize
  })))`);
  return JSON.parse(raw).map(e =>
    `${String(e.duration).padStart(5)}ms  ${String(e.size || '?').padStart(8)}B  ${e.type.padEnd(8)}  ${e.name}`
  ).join('\n');
}

// Click element by CSS selector
async function clickStr(cdp, sid, selector) {
  if (!selector) throw new Error('CSS selector required');
  const expr = `
    (function() {
      const el = document.querySelector(${JSON.stringify(selector)});
      if (!el) return { ok: false, error: 'Element not found: ' + ${JSON.stringify(selector)} };
      el.scrollIntoView({ block: 'center' });
      el.click();
      return { ok: true, tag: el.tagName, text: el.textContent.trim().substring(0, 80) };
    })()
  `;
  const result = await evalStr(cdp, sid, expr);
  const r = JSON.parse(result);
  if (!r.ok) throw new Error(r.error);
  return `Clicked <${r.tag}> "${r.text}"`;
}

// Click at CSS pixel coordinates using Input.dispatchMouseEvent
async function clickXyStr(cdp, sid, x, y) {
  const cx = parseFloat(x);
  const cy = parseFloat(y);
  if (isNaN(cx) || isNaN(cy)) throw new Error('x and y must be numbers (CSS pixels)');
  const base = { x: cx, y: cy, button: 'left', clickCount: 1, modifiers: 0 };
  await cdp.send('Input.dispatchMouseEvent', { ...base, type: 'mouseMoved' }, sid);
  await cdp.send('Input.dispatchMouseEvent', { ...base, type: 'mousePressed' }, sid);
  await sleep(50);
  await cdp.send('Input.dispatchMouseEvent', { ...base, type: 'mouseReleased' }, sid);
  return `Clicked at CSS (${cx}, ${cy})`;
}

// Type text using Input.insertText (works in cross-origin iframes, unlike eval)
async function typeStr(cdp, sid, text) {
  if (text == null || text === '') throw new Error('text required');
  await cdp.send('Input.insertText', { text }, sid);
  return `Typed ${text.length} characters`;
}

// Load-more: repeatedly click a button/selector until it disappears
async function loadAllStr(cdp, sid, selector, intervalMs = 1500) {
  if (!selector) throw new Error('CSS selector required');
  let clicks = 0;
  const deadline = Date.now() + 5 * 60 * 1000; // 5-minute hard cap
  while (Date.now() < deadline) {
    const exists = await evalStr(cdp, sid,
      `!!document.querySelector(${JSON.stringify(selector)})`
    );
    if (exists !== 'true') break;
    const clickExpr = `
      (function() {
        const el = document.querySelector(${JSON.stringify(selector)});
        if (!el) return false;
        el.scrollIntoView({ block: 'center' });
        el.click();
        return true;
      })()
    `;
    const clicked = await evalStr(cdp, sid, clickExpr);
    if (clicked !== 'true') break;
    clicks++;
    await sleep(intervalMs);
  }
  return `Clicked "${selector}" ${clicks} time(s) until it disappeared`;
}

// Send a raw CDP command and return the result as JSON
async function evalRawStr(cdp, sid, method, paramsJson) {
  if (!method) throw new Error('CDP method required (e.g. "DOM.getDocument")');
  let params = {};
  if (paramsJson) {
    try { params = JSON.parse(paramsJson); }
    catch { throw new Error(`Invalid JSON params: ${paramsJson}`); }
  }
  const result = await cdp.send(method, params, sid);
  return JSON.stringify(result, null, 2);
}

const SENSITIVE_HEADERS = /^(authorization|cookie|set-cookie|proxy-authorization|x-api-key|api-key)$/i;
const TEXT_MIME = /json|text|event-stream|ndjson|javascript|xml|html|plain|graphql|x-www-form-urlencoded/i;

function redactHeaders(headers = {}) {
  return Object.fromEntries(
    Object.entries(headers).map(([key, value]) => [
      key,
      SENSITIVE_HEADERS.test(key) ? '[redacted]' : value,
    ]),
  );
}

function previewText(value, max = 12000) {
  if (value == null) return value;
  const text = String(value);
  return text.length > max ? text.slice(0, max) + '...[truncated]' : text;
}

function createNetworkRecorder(cdp, sid) {
  const state = {
    enabled: false,
    records: new Map(),
    listenersInstalled: false,
  };

  const ensureRecord = (id) => {
    if (!state.records.has(id)) {
      state.records.set(id, { requestId: id, createdAt: Date.now() });
    }
    return state.records.get(id);
  };

  const installListeners = () => {
    if (state.listenersInstalled) return;
    state.listenersInstalled = true;

    cdp.onEvent('Network.requestWillBeSent', (params) => {
      const record = ensureRecord(params.requestId);
      record.url = params.request?.url || record.url;
      record.method = params.request?.method || record.method;
      record.type = params.type || record.type;
      record.requestHeaders = redactHeaders(params.request?.headers || {});
      record.requestPostData = params.request?.postData;
      record.initiator = params.initiator?.type;
      record.startWallTime = params.wallTime;
      record.startTimestamp = params.timestamp;
    });

    cdp.onEvent('Network.responseReceived', (params) => {
      const record = ensureRecord(params.requestId);
      record.url = params.response?.url || record.url;
      record.type = params.type || record.type;
      record.status = params.response?.status;
      record.mimeType = params.response?.mimeType;
      record.responseHeaders = redactHeaders(params.response?.headers || {});
      record.encodedDataLength = params.response?.encodedDataLength;
      record.responseTimestamp = params.timestamp;
    });

    cdp.onEvent('Network.loadingFinished', (params) => {
      const record = ensureRecord(params.requestId);
      record.finished = true;
      record.encodedDataLength = params.encodedDataLength;
      record.endTimestamp = params.timestamp;
    });

    cdp.onEvent('Network.loadingFailed', (params) => {
      const record = ensureRecord(params.requestId);
      record.failed = true;
      record.errorText = params.errorText;
      record.endTimestamp = params.timestamp;
    });

    cdp.onEvent('Network.webSocketCreated', (params) => {
      const record = ensureRecord(params.requestId);
      record.url = params.url;
      record.type = 'WebSocket';
      record.method = 'WS';
      record.frames = [];
    });

    cdp.onEvent('Network.webSocketFrameSent', (params) => {
      const record = ensureRecord(params.requestId);
      record.frames = record.frames || [];
      record.frames.push({ direction: 'sent', opcode: params.response?.opcode, payloadData: previewText(params.response?.payloadData, 4000) });
    });

    cdp.onEvent('Network.webSocketFrameReceived', (params) => {
      const record = ensureRecord(params.requestId);
      record.frames = record.frames || [];
      record.frames.push({ direction: 'received', opcode: params.response?.opcode, payloadData: previewText(params.response?.payloadData, 4000) });
    });
  };

  return {
    async enable() {
      if (state.enabled) return;
      installListeners();
      await cdp.send('Network.enable', {
        maxTotalBufferSize: 100 * 1024 * 1024,
        maxResourceBufferSize: 20 * 1024 * 1024,
        maxPostDataSize: 20 * 1024 * 1024,
      }, sid);
      state.enabled = true;
    },
    clear() {
      state.records.clear();
    },
    list(filter) {
      const needle = (filter || '').toLowerCase();
      const records = [...state.records.values()]
        .filter(record => {
          if (!needle) return true;
          return JSON.stringify({
            requestId: record.requestId,
            url: record.url,
            method: record.method,
            status: record.status,
            type: record.type,
            mimeType: record.mimeType,
            initiator: record.initiator,
          }).toLowerCase().includes(needle);
        })
        .sort((a, b) => (a.startTimestamp || 0) - (b.startTimestamp || 0));

      if (!records.length) {
        return state.enabled
          ? 'No captured network records matched.'
          : 'Network capture is not enabled. Run netclear <target> before the user action.';
      }

      return records.map(record => {
        const id = String(record.requestId || '').slice(0, 16).padEnd(16);
        const method = String(record.method || '?').padEnd(7);
        const status = String(record.status ?? (record.failed ? 'FAIL' : '?')).padStart(4);
        const type = String(record.type || '?').padEnd(12);
        const size = String(record.encodedDataLength ?? '?').padStart(8);
        const mime = String(record.mimeType || '').slice(0, 24).padEnd(24);
        const url = record.url || '';
        return `${id} ${method} ${status} ${type} ${size}B ${mime} ${url}`;
      }).join('\n');
    },
    async get(requestId, options = {}) {
      const matches = [...state.records.values()].filter(record => String(record.requestId).startsWith(requestId));
      if (matches.length === 0) throw new Error(`No captured request matching "${requestId}". Run netclear before the action, then net to list requests.`);
      if (matches.length > 1) throw new Error(`Ambiguous request id "${requestId}". Use more characters.`);
      const record = matches[0];
      let bodyInfo = null;

      if (record.type === 'WebSocket') {
        bodyInfo = { kind: 'websocketFrames', frames: record.frames || [] };
      } else if (record.finished && !record.failed) {
        try {
          const response = await cdp.send('Network.getResponseBody', { requestId: record.requestId }, sid);
          const body = response.base64Encoded
            ? Buffer.from(response.body || '', 'base64').toString('utf8')
            : response.body || '';
          if (options.responseFile) {
            writeFileSync(options.responseFile, body);
            bodyInfo = { responseFile: options.responseFile, base64Encoded: !!response.base64Encoded };
          } else {
            bodyInfo = {
              base64Encoded: !!response.base64Encoded,
              responseBodyPreview: TEXT_MIME.test(record.mimeType || '') ? previewText(body, options.full ? 200000 : 12000) : '[non-text body omitted]',
            };
          }
        } catch (error) {
          bodyInfo = { responseBodyError: error.message };
        }
      }

      if (options.requestFile && record.requestPostData != null) {
        writeFileSync(options.requestFile, record.requestPostData);
      }

      return JSON.stringify({
        requestId: record.requestId,
        url: record.url,
        method: record.method,
        type: record.type,
        status: record.status,
        mimeType: record.mimeType,
        failed: !!record.failed,
        errorText: record.errorText,
        requestHeaders: record.requestHeaders,
        responseHeaders: record.responseHeaders,
        requestPostDataPreview: options.requestFile ? undefined : previewText(record.requestPostData, options.full ? 200000 : 12000),
        requestFile: options.requestFile && record.requestPostData != null ? options.requestFile : undefined,
        ...bodyInfo,
      }, null, 2);
    },
  };
}

function parseNetGetOptions(args) {
  const requestId = args[0];
  if (!requestId) throw new Error('request id required');
  const options = {};
  for (let i = 1; i < args.length; i++) {
    if (args[i] === '--full') options.full = true;
    else if (args[i] === '--request-file') options.requestFile = args[++i];
    else if (args[i] === '--response-file') options.responseFile = args[++i];
    else throw new Error(`Unknown netget option: ${args[i]}`);
  }
  return { requestId, options };
}

// ---------------------------------------------------------------------------
// Per-tab daemon
// ---------------------------------------------------------------------------

async function runDaemon(targetId) {
  const sp = sockPath(targetId);

  let cdp;
  try {
    ({ cdp } = await openBrowserConnection());
  } catch (e) {
    process.stderr.write(`Daemon: cannot connect to Chrome: ${e.message}\n`);
    process.exit(1);
  }

  let sessionId;
  try {
    const res = await cdp.send('Target.attachToTarget', { targetId, flatten: true });
    sessionId = res.sessionId;
  } catch (e) {
    process.stderr.write(`Daemon: attach failed: ${e.message}\n`);
    cdp.close();
    process.exit(1);
  }
  const networkRecorder = createNetworkRecorder(cdp, sessionId);

  // Shutdown helpers
  let alive = true;
  function shutdown() {
    if (!alive) return;
    alive = false;
    server.close();
    if (!IS_WINDOWS) try { unlinkSync(sp); } catch {}
    cdp.close();
    process.exit(0);
  }

  // Exit if target goes away or Chrome disconnects
  cdp.onEvent('Target.targetDestroyed', (params) => {
    if (params.targetId === targetId) shutdown();
  });
  cdp.onEvent('Target.detachedFromTarget', (params) => {
    if (params.sessionId === sessionId) shutdown();
  });
  cdp.onClose(() => shutdown());
  process.on('SIGTERM', shutdown);
  process.on('SIGINT', shutdown);

  // Idle timer
  let idleTimer = setTimeout(shutdown, IDLE_TIMEOUT);
  function resetIdle() {
    clearTimeout(idleTimer);
    idleTimer = setTimeout(shutdown, IDLE_TIMEOUT);
  }

  // Handle a command
  async function handleCommand({ cmd, args }) {
    resetIdle();
    try {
      let result;
      switch (cmd) {
        case 'list': {
          const pages = await getPages(cdp);
          result = formatPageList(pages);
          break;
        }
        case 'list_raw': {
          const pages = await getPages(cdp);
          result = JSON.stringify(pages);
          break;
        }
        case 'snap': case 'snapshot': result = await snapshotStr(cdp, sessionId, true); break;
        case 'eval': result = await evalStr(cdp, sessionId, args[0]); break;
        case 'shot': case 'screenshot': result = await shotStr(cdp, sessionId, args[0], targetId); break;
        case 'html': result = await htmlStr(cdp, sessionId, args[0]); break;
        case 'nav': case 'navigate': result = await navStr(cdp, sessionId, args[0]); break;
        case 'perfnet': result = await netStr(cdp, sessionId); break;
        case 'net': case 'network': {
          await networkRecorder.enable();
          result = networkRecorder.list(args.join(' '));
          break;
        }
        case 'netclear': case 'network-clear': {
          await networkRecorder.enable();
          networkRecorder.clear();
          result = 'Network capture enabled and cleared.';
          break;
        }
        case 'netget': case 'network-get': {
          await networkRecorder.enable();
          const { requestId, options } = parseNetGetOptions(args);
          result = await networkRecorder.get(requestId, options);
          break;
        }
        case 'click': result = await clickStr(cdp, sessionId, args[0]); break;
        case 'clickxy': result = await clickXyStr(cdp, sessionId, args[0], args[1]); break;
        case 'type': result = await typeStr(cdp, sessionId, args[0]); break;
        case 'loadall': result = await loadAllStr(cdp, sessionId, args[0], args[1] ? parseInt(args[1]) : 1500); break;
        case 'evalraw': result = await evalRawStr(cdp, sessionId, args[0], args[1]); break;
        case 'stop': return { ok: true, result: '', stopAfter: true };
        default: return { ok: false, error: `Unknown command: ${cmd}` };
      }
      return { ok: true, result: result ?? '' };
    } catch (e) {
      return { ok: false, error: e.message };
    }
  }

  // Unix socket server — NDJSON protocol
  // Wire format: each message is one JSON object followed by \n (newline-delimited JSON).
  // Request:  { "id": <number>, "cmd": "<command>", "args": ["arg1", "arg2", ...] }
  // Response: { "id": <number>, "ok": <boolean>, "result": "<string>" }
  //           or { "id": <number>, "ok": false, "error": "<message>" }
  const server = net.createServer((conn) => {
    let buf = '';
    conn.on('data', (chunk) => {
      buf += chunk.toString();
      const lines = buf.split('\n');
      buf = lines.pop(); // keep incomplete last line
      for (const line of lines) {
        if (!line.trim()) continue;
        let req;
        try {
          req = JSON.parse(line);
        } catch {
          conn.write(JSON.stringify({ ok: false, error: 'Invalid JSON request', id: null }) + '\n');
          continue;
        }
        handleCommand(req).then((res) => {
          const payload = JSON.stringify({ ...res, id: req.id }) + '\n';
          if (res.stopAfter) conn.end(payload, shutdown);
          else conn.write(payload);
        });
      }
    });
  });

  server.on('error', (e) => {
    process.stderr.write(`Daemon server listen failed: ${e.message}\n`);
    process.exit(1);
  });

  if (!IS_WINDOWS) try { unlinkSync(sp); } catch {}
  server.listen(sp);
}

// ---------------------------------------------------------------------------
// CLI ↔ daemon communication
// ---------------------------------------------------------------------------

function connectToSocket(sp) {
  return new Promise((resolve, reject) => {
    const conn = net.connect(sp);
    conn.on('connect', () => resolve(conn));
    conn.on('error', reject);
  });
}

async function getOrStartTabDaemon(targetId) {
  const sp = sockPath(targetId);
  // Try existing daemon
  try { return await connectToSocket(sp); } catch {}

  // Clean stale socket
  if (!IS_WINDOWS) try { unlinkSync(sp); } catch {}

  // Spawn daemon
  const child = spawn(process.execPath, [process.argv[1], '_daemon', targetId], {
    detached: true,
    stdio: 'ignore',
  });
  child.unref();

  // Wait for socket (includes time for user to click Allow)
  for (let i = 0; i < DAEMON_CONNECT_RETRIES; i++) {
    await sleep(DAEMON_CONNECT_DELAY);
    try { return await connectToSocket(sp); } catch {}
  }
  throw new Error('Daemon failed to start — did you click Allow in Chrome?');
}

function sendCommand(conn, req) {
  return new Promise((resolve, reject) => {
    let buf = '';
    let settled = false;

    const cleanup = () => {
      conn.off('data', onData);
      conn.off('error', onError);
      conn.off('end', onEnd);
      conn.off('close', onClose);
    };

    const onData = (chunk) => {
      buf += chunk.toString();
      const idx = buf.indexOf('\n');
      if (idx === -1) return;
      settled = true;
      cleanup();
      resolve(JSON.parse(buf.slice(0, idx)));
      conn.end();
    };

    const onError = (error) => {
      if (settled) return;
      settled = true;
      cleanup();
      reject(error);
    };

    const onEnd = () => {
      if (settled) return;
      settled = true;
      cleanup();
      reject(new Error('Connection closed before response'));
    };

    const onClose = () => {
      if (settled) return;
      settled = true;
      cleanup();
      reject(new Error('Connection closed before response'));
    };

    conn.on('data', onData);
    conn.on('error', onError);
    conn.on('end', onEnd);
    conn.on('close', onClose);
    req.id = 1;
    conn.write(JSON.stringify(req) + '\n');
  });
}

// ---------------------------------------------------------------------------
// Stop daemons
// ---------------------------------------------------------------------------

async function stopDaemons(targetPrefix) {
  if (!existsSync(PAGES_CACHE)) return 'No page daemon state found.';
  const pages = JSON.parse(readFileSync(PAGES_CACHE, 'utf8'));
  const targets = targetPrefix
    ? [resolvePrefix(targetPrefix, pages.map(p => p.targetId), 'target')]
    : pages.map(p => p.targetId);

  for (const targetId of targets) {
    const sp = sockPath(targetId);
    try {
      const conn = await connectToSocket(sp);
      await sendCommand(conn, { cmd: 'stop' });
    } catch {
      if (!IS_WINDOWS) try { unlinkSync(sp); } catch {}
    }
  }
  return `Stopped ${targets.length} page daemon(s).`;
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

const USAGE = `cdp - lightweight Chrome DevTools Protocol CLI (no Puppeteer)

Usage: cdp <command> [args]

  list                              List open pages (shows unique target prefixes)
  snap  <target>                    Accessibility tree snapshot
  eval  <target> <expr>             Evaluate JS expression
  shot  <target> [file]             Screenshot (default: screenshot-<target>.png in runtime dir); prints coordinate mapping
  html  <target> [selector]         Get HTML (full page or CSS selector)
  nav   <target> <url>              Navigate to URL and wait for load completion
  net   <target>                    Network performance entries
  click   <target> <selector>       Click an element by CSS selector
  clickxy <target> <x> <y>          Click at CSS pixel coordinates (see coordinate note below)
  type    <target> <text>           Type text at current focus via Input.insertText
                                    Works in cross-origin iframes unlike eval-based approaches
  loadall <target> <selector> [ms]  Repeatedly click a "load more" button until it disappears
                                    Optional interval in ms between clicks (default 1500)
  evalraw <target> <method> [json]  Send a raw CDP command; returns JSON result
                                    e.g. evalraw <t> "DOM.getDocument" '{}'
  open  [url]                       Open a new tab (default: about:blank)
                                    Note: each new tab triggers a fresh "Allow debugging?" prompt
  stop  [target]                    Stop daemon(s)

<target> is a unique targetId prefix from "cdp list". If a prefix is ambiguous,
use more characters.

COORDINATE SYSTEM
  shot captures the viewport at the device's native resolution.
  The screenshot image size = CSS pixels × DPR (device pixel ratio).
  For CDP Input events (clickxy, etc.) you need CSS pixels, not image pixels.

    CSS pixels = screenshot image pixels / DPR

  shot prints the DPR and an example conversion for the current page.
  Typical Retina (DPR=2): CSS px ≈ screenshot px × 0.5
  If your viewer rescales the image further, account for that scaling too.

EVAL SAFETY NOTE
  Avoid index-based DOM selection (querySelectorAll(...)[i]) across multiple
  eval calls when the list can change between calls (e.g. after clicking
  "Ignore" buttons on a feed — indices shift). Prefer stable selectors or
  collect all data in a single eval.

DAEMON IPC (for advanced use / scripting)
  Each tab runs a persistent daemon at Unix socket in the runtime dir (see below).
  Protocol: newline-delimited JSON (one JSON object per line, UTF-8).
    Request:  {"id":<number>, "cmd":"<command>", "args":["arg1","arg2",...]}
    Response: {"id":<number>, "ok":true,  "result":"<string>"}
           or {"id":<number>, "ok":false, "error":"<message>"}
  Commands mirror the CLI: snap, eval, shot, html, nav, net, click, clickxy,
  type, loadall, evalraw, stop. Use evalraw to send arbitrary CDP methods.
  The socket disappears after 20 min of inactivity or when the tab closes.
`;

const USAGE_TEXT = `cdp - lightweight Chrome DevTools Protocol CLI (no Puppeteer)

Usage: cdp <command> [args]

  launch [url]                      Launch or reuse a managed browser instance
                                    Uses persistent profile mode when bundled by MCP Gateway
  list                              List open pages (shows unique target prefixes)
  snap  <target>                    Accessibility tree snapshot
  eval  <target> <expr>             Evaluate JS expression
  shot  <target> [file]             Screenshot (default: screenshot-<target>.png in runtime dir); prints coordinate mapping
  html  <target> [selector]         Get HTML (full page or CSS selector)
  nav   <target> <url>              Navigate to URL and wait for load completion
  net   [target] [filter]           Search/list captured CDP Network records
  netclear [target]                 Enable CDP Network capture and clear records
  netget [target] <id> [options]    Get request/response details by request id
                                    Options: --full --request-file <path> --response-file <path>
  perfnet [target]                  PerformanceResourceTiming entries
  click   <target> <selector>       Click an element by CSS selector
  clickxy <target> <x> <y>          Click at CSS pixel coordinates (see coordinate note below)
  type    <target> <text>           Type text at current focus via Input.insertText
                                    Works in cross-origin iframes unlike eval-based approaches
  loadall <target> <selector> [ms]  Repeatedly click a "load more" button until it disappears
                                    Optional interval in ms between clicks (default 1500)
  evalraw <target> <method> [json]  Send a raw CDP command; returns JSON result
                                    e.g. evalraw <t> "DOM.getDocument" '{}'
  open  [url]                       Open a new tab (default: about:blank)
  stop  [target]                    Stop daemon(s)

<target> is a unique targetId prefix from "cdp list". If a prefix is ambiguous,
use more characters.

BROWSER MODES
  Default mode is "launch": the CLI starts or reuses its own Chromium-based browser
  instance in the runtime dir.

  Profile modes:
    persistent/reuse: keep using the same managed user data directory.
    clone: copy the selected browser's default user data directory before launch.
    empty: create a fresh managed user data directory.

  Set CDP_BROWSER_MODE=attach to connect to an already-running browser with remote
  debugging enabled. This is the mode that can trigger Chrome's "Allow debugging?"
  approval prompt when a tab is first attached.

  Set CDP_BROWSER=chrome|edge|brave|chromium to choose which installed browser
  binary launch mode should use. Set CDP_USER_DATA_DIR to override the managed
  profile location. Set CDP_PROFILE_SOURCE_DIR to override which live profile gets
  cloned. Set CDP_PROFILE_MODE=persistent to reuse the same managed profile.
  Set CDP_PROFILE_MODE=empty to launch with a blank profile.

COORDINATE SYSTEM
  shot captures the viewport at the device's native resolution.
  The screenshot image size = CSS pixels x DPR (device pixel ratio).
  For CDP Input events (clickxy, etc.) you need CSS pixels, not image pixels.

    CSS pixels = screenshot image pixels / DPR

  shot prints the DPR and an example conversion for the current page.
  Typical Retina (DPR=2): CSS px ~= screenshot px x 0.5
  If your viewer rescales the image further, account for that scaling too.

EVAL SAFETY NOTE
  Avoid index-based DOM selection (querySelectorAll(...)[i]) across multiple
  eval calls when the list can change between calls (e.g. after clicking
  "Ignore" buttons on a feed - indices shift). Prefer stable selectors or
  collect all data in a single eval.

DAEMON IPC (for advanced use / scripting)
  Each tab runs a persistent daemon at Unix socket in the runtime dir (see below).
  Protocol: newline-delimited JSON (one JSON object per line, UTF-8).
    Request:  {"id":<number>, "cmd":"<command>", "args":["arg1","arg2",...]}
    Response: {"id":<number>, "ok":true,  "result":"<string>"}
           or {"id":<number>, "ok":false, "error":"<message>"}
  Commands mirror the CLI: snap, eval, shot, html, nav, net, netclear, netget,
  perfnet, click, clickxy, type, loadall, evalraw, stop. Use evalraw to send
  arbitrary CDP methods.
  The socket disappears after 20 min of inactivity or when the tab closes.
`;

const NEEDS_TARGET = new Set([
  'snap','snapshot','eval','shot','screenshot','html','nav','navigate',
  'net','network','netclear','network-clear','netget','network-get','perfnet',
  'click','clickxy','type','loadall','evalraw',
]);
const OPTIONAL_TARGET = new Set([
  'snap','snapshot','eval','shot','screenshot','html',
  'net','network','netclear','network-clear','netget','network-get','perfnet',
]);

async function main() {
  const [cmd, ...args] = process.argv.slice(2);

  // Daemon mode (internal)
  if (cmd === '_daemon') { await runDaemon(args[0]); return; }

  if (!cmd || cmd === 'help' || cmd === '--help' || cmd === '-h') {
    console.log(USAGE_TEXT); process.exit(0);
  }

  if (cmd === 'launch') {
    const url = args[0] || 'about:blank';
    const { session, cdp } = await openBrowserConnection(url);
    if (session.mode !== 'launch') {
      cdp.close();
      console.log('Launch is only meaningful in managed mode. Set CDP_BROWSER_MODE=launch or unset it.');
      return;
    }

    const pages = await cacheOpenPages(cdp);
    if (pages.length) setActiveTarget(pages[0].targetId);
    cdp.close();

    const browserName = session.state?.browserName || 'Managed browser';
    console.log(`${session.launched ? 'Started' : 'Reused'} ${browserName}`);
    if (session.state?.userDataDir) console.log(`Profile: ${session.state.userDataDir}`);
    if (session.state?.sourceUserDataDir) console.log(`Cloned from: ${session.state.sourceUserDataDir}`);
    if (pages.length) console.log(formatPageList(pages));
    return;
  }

  if (cmd === 'list' || cmd === 'ls') {
    const { cdp } = await openBrowserConnection();
    const pages = await cacheOpenPages(cdp);
    if (pages.length === 1) setActiveTarget(pages[0].targetId);
    cdp.close();
    console.log(formatPageList(pages));
    setTimeout(() => process.exit(0), 100);
    return;
  }

  // Open new tab
  if (cmd === 'open') {
    const url = args[0] || 'about:blank';
    const { session, cdp } = await openBrowserConnection(url);

    if (session.launched && session.mode === 'launch') {
      const pages = await cacheOpenPages(cdp);
      if (pages.length) setActiveTarget(pages[0].targetId);
      cdp.close();
      console.log(`Started managed browser at ${url}`);
      if (pages.length) console.log(formatPageList(pages));
      return;
    }

    const { targetId } = await cdp.send('Target.createTarget', { url });
    setActiveTarget(targetId);
    const pages = await cacheOpenPages(cdp);
    if (!pages.some(p => p.targetId === targetId)) {
      pages.push({ targetId, title: url, url });
      writeFileSync(PAGES_CACHE, JSON.stringify(pages), { mode: 0o600 });
    }
    cdp.close();
    console.log(`Opened new tab: ${targetId.slice(0, 8)}  ${url}`);
    if (session.mode === 'attach') {
      console.log('Note: this tab may need "Allow debugging?" approval on first access.');
    }
    return;
  }

  // Stop
  if (cmd === 'stop') {
    const daemonResult = await stopDaemons(args[0]);
    const browserResult = args[0] ? 'Managed browser left running because a target was specified.' : await stopManagedBrowser();
    console.log([daemonResult, browserResult].filter(Boolean).join('\n'));
    return;
  }

  // Page commands — need target prefix
  if (!NEEDS_TARGET.has(cmd)) {
    console.error(`Unknown command: ${cmd}\n`);
    console.log(USAGE_TEXT);
    process.exit(1);
  }

  // Resolve prefix -> full targetId from pages cache. Some commands accept an
  // omitted target and use the active tab selected by open/list/launch.
  if (!existsSync(PAGES_CACHE)) {
    const { cdp } = await openBrowserConnection();
    await cacheOpenPages(cdp);
    cdp.close();
  }
  let pages = JSON.parse(readFileSync(PAGES_CACHE, 'utf8'));
  let targetId;
  let cmdArgs = args.slice(1);
  const targetPrefix = args[0];

  const refreshPages = async () => {
    const { cdp } = await openBrowserConnection();
    pages = await cacheOpenPages(cdp);
    cdp.close();
  };

  if (OPTIONAL_TARGET.has(cmd)) {
    if (targetPrefix) {
      try {
        targetId = resolveTargetFromPages(targetPrefix, pages);
        cmdArgs = args.slice(1);
      } catch {
        await refreshPages();
        try {
          targetId = resolveTargetFromPages(targetPrefix, pages);
          cmdArgs = args.slice(1);
        } catch {
          targetId = resolveTargetFromPages(null, pages);
          cmdArgs = args;
        }
      }
    } else {
      targetId = resolveTargetFromPages(null, pages);
      cmdArgs = [];
    }
  } else {
    if (!targetPrefix) {
      console.error('Error: target ID required. Run "cdp list" first.');
      process.exit(1);
    }
    try {
      targetId = resolveTargetFromPages(targetPrefix, pages);
    } catch {
      await refreshPages();
      targetId = resolveTargetFromPages(targetPrefix, pages);
    }
  }
  setActiveTarget(targetId);

  const conn = await getOrStartTabDaemon(targetId);

  if (cmd === 'eval') {
    const expr = cmdArgs.join(' ');
    if (!expr) { console.error('Error: expression required'); process.exit(1); }
    cmdArgs[0] = expr;
  } else if (cmd === 'type') {
    // Join all remaining args as text (allows spaces)
    const text = cmdArgs.join(' ');
    if (!text) { console.error('Error: text required'); process.exit(1); }
    cmdArgs[0] = text;
  } else if (cmd === 'evalraw') {
    // args: [method, ...jsonParts] — join json parts in case of spaces
    if (!cmdArgs[0]) { console.error('Error: CDP method required'); process.exit(1); }
    if (cmdArgs.length > 2) cmdArgs[1] = cmdArgs.slice(1).join(' ');
  }

  if ((cmd === 'nav' || cmd === 'navigate') && !cmdArgs[0]) {
    console.error('Error: URL required');
    process.exit(1);
  }

  const response = await sendCommand(conn, { cmd, args: cmdArgs });

  if (response.ok) {
    if (response.result) console.log(response.result);
  } else {
    console.error('Error:', response.error);
    process.exitCode = 1;
  }
}

main().catch(e => { console.error(e.message); process.exit(1); });
