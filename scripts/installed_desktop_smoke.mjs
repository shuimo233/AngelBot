import { createHash } from 'node:crypto';
import { execFile, spawn } from 'node:child_process';
import { once } from 'node:events';
import { createReadStream } from 'node:fs';
import { lstat, mkdir, mkdtemp, readFile, stat, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { basename, dirname, isAbsolute, join, resolve } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import { pathToFileURL } from 'node:url';
import { parseArgs, promisify } from 'node:util';

const exec = promisify(execFile);
const appId = 'com.angelbot.desktop';
const uninstallKey = String.raw`HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\AngelBot`;
const modelName = 'angelbot-installed-smoke';
const prompt = 'AngelBot installed smoke: reply with the fixture acknowledgement and do not use tools.';
const acknowledgement = 'AngelBot installed smoke: acknowledgement received.';
const tauriOrigin = /^https?:\/\/tauri\.localhost(?:\/|$)/;

class SmokeFailure extends Error {}

function check(condition, message) {
  if (!condition) throw new SmokeFailure(message);
}

async function absent(path) {
  try {
    await lstat(path);
    return false;
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
    return true;
  }
}

async function digest(path) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest('hex');
}

async function waitFor(name, probe) {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    const result = await probe();
    if (result) return result;
    await sleep(200);
  }
  throw new SmokeFailure('Timed out waiting for ' + name + '.');
}

// This is the actual OpenAI-compatible streaming contract. The runner binds
// only to loopback and records counts, with neither request bodies nor keys.
export async function startSmokeModel() {
  let accepted = 0;
  let rejected = 0;
  const server = createServer(async (request, response) => {
    try {
      check(request.method === 'POST' && request.url === '/v1/chat/completions', 'Unexpected model endpoint.');
      check(!request.headers.authorization, 'Unexpected model credentials.');
      const chunks = [];
      let bytes = 0;
      for await (const chunk of request) {
        bytes += chunk.length;
        check(bytes <= 1024 * 1024, 'Oversized model request.');
        chunks.push(chunk);
      }
      const payload = JSON.parse(Buffer.concat(chunks).toString('utf8'));
      const lastUser = payload.messages?.findLast((message) => message.role === 'user');
      check(payload.model === modelName && payload.stream === true && typeof lastUser?.content === 'string'
        && lastUser.content.includes(prompt) && accepted === 0, 'Unexpected model request.');
      accepted += 1;

      const frame = (delta, reason) => 'data: ' + JSON.stringify({
        id: 'installed-smoke', object: 'chat.completion.chunk', model: modelName,
        choices: [{ index: 0, delta, finish_reason: reason }],
      }) + '\n\n';
      response.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache' });
      response.end(frame({ role: 'assistant', content: acknowledgement }, null)
        + frame({}, 'stop') + 'data: [DONE]\n\n');
    } catch {
      rejected += 1;
      response.writeHead(400, { 'Content-Type': 'application/json' });
      response.end(JSON.stringify({ error: { message: 'Unexpected installed smoke fixture request' } }));
    }
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  return {
    baseUrl: 'http://127.0.0.1:' + server.address().port + '/v1',
    counters: () => ({ accepted, rejected }),
    close: () => new Promise((done, fail) => {
      server.close((error) => { if (error) fail(error); else done(); });
      server.closeAllConnections();
    }),
  };
}

class DevTools {
  sequence = 0;
  pending = new Map();

  constructor(socket) {
    this.socket = socket;
    socket.addEventListener('message', ({ data }) => {
      const message = JSON.parse(data);
      this.pending.get(message.id)?.(message);
    });
  }

  static async open(address) {
    const target = new URL(address);
    check(target.protocol === 'ws:' && ['127.0.0.1', 'localhost'].includes(target.hostname),
      'WebView2 debugger must be local.');
    const socket = new WebSocket(target);
    await new Promise((ready, fail) => {
      const timer = setTimeout(() => {
        socket.close();
        fail(new SmokeFailure('WebView2 debugger connection timed out.'));
      }, 5_000);
      socket.addEventListener('open', () => { clearTimeout(timer); ready(); }, { once: true });
      socket.addEventListener('error', () => {
        clearTimeout(timer);
        fail(new SmokeFailure('WebView2 debugger connection failed.'));
      }, { once: true });
    });
    return new DevTools(socket);
  }

  command(method, params = {}) {
    const id = ++this.sequence;
    return new Promise((done, fail) => {
      check(this.socket.readyState === WebSocket.OPEN, 'The WebView2 debugger is closed.');
      const timer = setTimeout(() => {
        this.pending.delete(id);
        fail(new SmokeFailure('WebView2 command ' + method + ' timed out.'));
      }, 5_000);
      this.pending.set(id, (message) => {
        clearTimeout(timer);
        this.pending.delete(id);
        if (message.error) fail(new SmokeFailure('WebView2 command ' + method + ' failed.'));
        else done(message.result);
      });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }

  async evaluate(expression) {
    const { exceptionDetails, result } = await this.command('Runtime.evaluate', {
      expression, awaitPromise: true, returnByValue: true,
    });
    check(!exceptionDetails, 'A WebView2 page operation failed.');
    return result.value;
  }

  invoke(command, args = {}) {
    return this.evaluate('window.__TAURI__.core.invoke(' + JSON.stringify(command) + ', ' + JSON.stringify(args) + ')');
  }

  close() {
    this.socket.close();
  }
}

function childEnvironment(work) {
  const inherited = new Set(['systemroot', 'windir', 'systemdrive', 'comspec', 'path', 'homepath',
    'homedrive', 'userprofile', 'appdata', 'localappdata', 'programfiles', 'programfiles(x86)',
    'programw6432', 'programdata', 'username', 'userdomain', 'os', 'processor_architecture']);
  return {
    ...Object.fromEntries(Object.entries(process.env).filter(([name]) => inherited.has(name.toLowerCase()))),
    TEMP: work, TMP: work, NO_PROXY: '127.0.0.1,localhost',
  };
}

async function connectApp(app, webview) {
  const debuggerFile = join(webview, 'EBWebView', 'DevToolsActivePort');
  const target = await waitFor('the installed app WebView2 target', async () => {
    check(app.exitCode === null && app.signalCode === null, 'Installed AngelBot exited before WebView2 opened.');
    let portFile;
    try {
      portFile = await readFile(debuggerFile, 'utf8');
    } catch (error) {
      if (error.code === 'ENOENT') return null;
      throw error;
    }
    const port = Number(portFile.split(/\r?\n/, 1)[0]);
    if (!Number.isInteger(port) || port < 1 || port > 65535) return null;
    let pages;
    try {
      const response = await fetch('http://127.0.0.1:' + port + '/json/list', { signal: AbortSignal.timeout(2_000) });
      pages = await response.json();
    } catch {
      return null;
    }
    if (!Array.isArray(pages)) return null;
    return pages.find((page) => page.type === 'page' && tauriOrigin.test(page.url)
      && page.webSocketDebuggerUrl)?.webSocketDebuggerUrl;
  });
  return DevTools.open(target);
}

async function startApp(binary, env, webview) {
  await mkdir(webview);
  const child = spawn(binary, [], {
    cwd: dirname(binary), stdio: 'ignore',
    env: {
      ...env,
      WEBVIEW2_USER_DATA_FOLDER: webview,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: '--remote-debugging-port=0 --remote-debugging-address=127.0.0.1',
    },
  });
  await once(child, 'spawn');
  return child;
}

async function closeApp(child, powershell, env) {
  check(child.exitCode === null && child.signalCode === null, 'Installed AngelBot exited before the close action.');
  const exited = once(child, 'exit', { signal: AbortSignal.timeout(15_000) });
  const close = exec(powershell, ['-NoProfile', '-NonInteractive', '-Command',
    'if (-not [Diagnostics.Process]::GetProcessById(' + child.pid + ').CloseMainWindow()) { exit 1 }'],
  { env, windowsHide: true, timeout: 10_000 });
  const [[code, signal]] = await Promise.all([exited, close]);
  check(code === 0 && signal === null, 'Installed AngelBot did not close normally.');
}

async function main() {
  const { values } = parseArgs({ options: {
    installer: { type: 'string' }, report: { type: 'string' },
  } });
  check(values.installer && values.report, 'Usage: node scripts/installed_desktop_smoke.mjs --installer path --report path');
  check(process.platform === 'win32' && process.env.GITHUB_ACTIONS === 'true'
    && process.env.RUNNER_OS === 'Windows' && process.env.RUNNER_ENVIRONMENT === 'github-hosted',
  'Installed smoke requires a disposable GitHub-hosted Windows runner.');
  check(typeof WebSocket === 'function', 'Installed smoke requires Node.js 22 or newer.');
  const { LOCALAPPDATA: local, APPDATA: roaming, RUNNER_TEMP: temp, SystemRoot: windows } = process.env;
  check([local, roaming, temp, windows].every((path) => path && isAbsolute(path)),
    'Windows runner folders are unavailable.');
  const installer = resolve(values.installer);
  const output = resolve(values.report);
  check(basename(installer).endsWith('-setup.exe') && installer.toLowerCase() !== output.toLowerCase(),
    'Provide a Tauri NSIS setup executable and a separate report path.');
  const packageInfo = await stat(installer);
  check(packageInfo.isFile(), 'The NSIS input must be a file.');
  const report = {
    schemaVersion: 1, status: 'running', phase: 'preflight',
    installerSha256: await digest(installer), installerBytes: packageInfo.size,
    reopenMethod: 'normal-window-close-and-fresh-process-with-new-webview-profile',
    checks: {},
  };
  let owned, cdp, model, failure;
  await mkdir(dirname(output), { recursive: true });
  const saveReport = () => writeFile(output, JSON.stringify(report, null, 2) + '\n', 'utf8');
  await saveReport();
  try {
    const installDir = join(local, 'AngelBot');
    const runtime = join(local, appId);
    for (const path of [installDir, runtime, join(roaming, 'AngelBot')]) {
      check(await absent(path), 'Existing AngelBot installation or profile on the runner.');
    }
    const reg = join(windows, 'System32', 'reg.exe');
    const powershell = join(windows, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
    const registration = await exec(reg, ['query', uninstallKey], { windowsHide: true })
      .then(() => true, (error) => { if (error.code === 1) return false; throw error; });
    check(!registration, 'Existing per-user AngelBot registration on the runner.');
    report.checks.cleanProfile = true;

    const work = await mkdtemp(join(temp, 'angelbot-installed-smoke-'));
    const env = childEnvironment(work);
    report.phase = 'install';
    await saveReport();
    await exec(installer, ['/S'], { cwd: work, env, timeout: 180_000, windowsHide: true });
    const binary = join(installDir, 'angelbot.exe');
    check((await stat(binary)).isFile(), 'Installer did not create the AngelBot release executable.');
    const { stdout } = await exec(reg, ['query', uninstallKey, '/v', 'InstallLocation'], { windowsHide: true });
    const installLocation = stdout.match(/^\s*InstallLocation\s+REG_SZ\s+(.+)\s*$/m)?.[1].trim().replace(/^"|"$/g, '');
    check(installLocation && resolve(installLocation).toLowerCase() === resolve(installDir).toLowerCase(),
      'Per-user installer registration does not match the installed executable.');
    report.installedExeSha256 = await digest(binary);
    report.checks.currentUserInstall = true;

    model = await startSmokeModel();
    report.phase = 'first-launch';
    await saveReport();
    const firstWebView = join(work, 'webview-first');
    owned = await startApp(binary, env, firstWebView);
    cdp = await connectApp(owned, firstWebView);
    const composer = "document.querySelector('textarea[aria-label=\"消息输入\"]')";
    const ready = () => cdp.evaluate('Boolean(window.__TAURI__?.core?.invoke && ' + composer
      + '?.getClientRects().length && document.querySelector(".chat-header-session-title")?.textContent === "AngelBot 日常")');
    await waitFor('Daily in the installed WebView2', ready);
    const workspaces = await cdp.invoke('get_workspaces');
    check(workspaces.length === 1 && workspaces[0].kind === 'personal' && workspaces[0].activeSessionId,
      'The fresh app did not create one Daily Main conversation.');
    const sessionId = workspaces[0].activeSessionId;
    const readMessages = () => cdp.invoke('get_messages', { sessionId });
    check((await readMessages()).length === 0, 'Fresh Daily history is not empty.');
    check((await stat(join(runtime, 'data.db'))).isFile(), 'Application database is outside its expected installed profile.');
    report.checks.nativeWebViewReady = true;

    report.phase = 'send';
    await saveReport();
    await cdp.invoke('save_api_config', { config: {
      provider: 'ollama', model: modelName, base_url: model.baseUrl, api_key: '',
      auth_mode: 'none', protocol: 'openai_chat_completions', max_tokens: 256, temperature: 0,
    } });
    // The production frontend reads the saved connection when it mounts.
    await cdp.command('Page.reload');
    await waitFor('the credential-free loopback connection', async () => {
      try {
        return await ready() && await cdp.evaluate("document.querySelector('.model-config-notice') === null");
      } catch (error) {
        if (error instanceof SmokeFailure) return false;
        throw error;
      }
    });
    const config = await cdp.invoke('load_api_config');
    check(config.provider === 'ollama' && config.model === modelName && config.base_url === model.baseUrl
      && config.auth_mode === 'none' && !config.has_api_key, 'The application did not load the intended loopback profile.');
    await cdp.evaluate(composer + '.focus()');
    await cdp.command('Input.insertText', { text: prompt });
    const sendButton = "document.querySelector('button[title=\"发送\"]')";
    await waitFor('an enabled send button', () => cdp.evaluate(sendButton + '?.disabled === false'));
    await cdp.evaluate(sendButton + '.click()');
    const messages = await waitFor('the completed persisted reply', async () => {
      const rows = await readMessages();
      return rows.some((row) => row.role === 'assistant' && row.content === acknowledgement) ? rows : null;
    });
    check(messages.length === 2 && messages[0].role === 'user' && messages[0].content === prompt,
      'The installed app did not save exactly one synthetic user turn and its reply.');
    const [user, assistant] = messages;
    check(assistant.role === 'assistant' && assistant.id && assistant.parentId === user.id
      && assistant.taskRun?.status === 'completed' && !assistant.toolCalls?.length && !assistant.toolResults?.length,
    'Assistant receipt lacks a completed tool-free reply linked to its user turn.');
    const replyVisible = () => cdp.evaluate("Array.from(document.querySelectorAll('[data-conversation-message-id]')).some(row => row.dataset.conversationMessageId === "
      + JSON.stringify(assistant.id) + ' && row.innerText.includes(' + JSON.stringify(acknowledgement) + ') && row.getClientRects().length > 0)');
    await waitFor('the completed reply rendered by the installed frontend', replyVisible);
    report.checks.durableReply = true;

    report.phase = 'reopen';
    await saveReport();
    // The completed reply was durable before a normal window close. A fresh
    // WebView2 profile verifies restoration from the application database.
    cdp.close();
    cdp = undefined;
    await closeApp(owned, powershell, env);
    owned = undefined;
    report.checks.normalExit = true;
    const reopenedWebView = join(work, 'webview-reopened');
    owned = await startApp(binary, env, reopenedWebView);
    cdp = await connectApp(owned, reopenedWebView);
    await waitFor('Daily and its saved reply after reopening the installed process', () =>
      ready().then((isReady) => isReady && replyVisible()));
    const restoredWorkspaces = await cdp.invoke('get_workspaces');
    const restored = await readMessages();
    check(restoredWorkspaces.length === 1 && restoredWorkspaces[0].activeSessionId === sessionId
      && restored.length === messages.length
      && restored.every((row, index) => row.id === messages[index].id && row.parentId === messages[index].parentId
        && row.content === messages[index].content && row.sessionId === sessionId)
      && restored.at(-1).taskRun?.status === 'completed',
    'Reopen did not restore the same Daily conversation and completed reply.');
    const requests = model.counters();
    check(requests.accepted === 1 && requests.rejected === 0, 'Expected exactly one synthetic model request across both launches.');
    report.checks.reopenedSameConversation = true;
    cdp.close();
    cdp = undefined;
    await closeApp(owned, powershell, env);
    owned = undefined;
    check(await digest(installer) === report.installerSha256, 'The installer changed during acceptance.');
    report.phase = 'complete';
  } catch (error) {
    failure = error;
  } finally {
    report.modelRequests = model?.counters();
    cdp?.close();
    if (owned && owned.exitCode === null && owned.signalCode === null) owned.kill();
    try {
      if (model) await model.close();
    } catch (error) {
      failure ??= error;
    }
    report.status = failure ? 'failed' : 'passed';
    await saveReport();
  }
  if (failure) {
    throw new SmokeFailure('Installed smoke failed during ' + report.phase + '. ' + (failure instanceof SmokeFailure
      ? failure.message : 'The installer or application operation failed.'));
  }
  console.log('Installed AngelBot passed Daily send, durable reply and reopen.');
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  main().catch((error) => {
    console.error(error instanceof SmokeFailure ? error.message : 'Installed smoke could not initialize.');
    process.exitCode = 1;
  });
}
