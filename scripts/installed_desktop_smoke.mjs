import { createHash } from 'node:crypto';
import { execFile, spawn } from 'node:child_process';
import { once } from 'node:events';
import { createReadStream } from 'node:fs';
import { lstat, mkdir, mkdtemp, stat, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { basename, dirname, isAbsolute, join, resolve } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { DatabaseSync } from 'node:sqlite';
import { parseArgs, promisify } from 'node:util';

const exec = promisify(execFile);
const appId = 'com.angelbot.desktop';
const uninstallKey = String.raw`HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\AngelBot`;
const modelName = 'angelbot-installed-smoke';
const prompt = 'AngelBot installed smoke: reply with the fixture acknowledgement and do not use tools.';
const acknowledgement = 'AngelBot installed smoke: acknowledgement received.';
const uiaScript = fileURLToPath(new URL('./installed_desktop_uia.ps1', import.meta.url));

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

function childEnvironment(work) {
  const inherited = new Set(['systemroot', 'windir', 'systemdrive', 'comspec', 'path', 'homepath',
    'homedrive', 'userprofile', 'appdata', 'localappdata', 'programfiles', 'programfiles(x86)',
    'programw6432', 'programdata', 'username', 'userdomain', 'os', 'processor_architecture']);
  return {
    ...Object.fromEntries(Object.entries(process.env).filter(([name]) => inherited.has(name.toLowerCase()))),
    TEMP: work, TMP: work, NO_PROXY: '127.0.0.1,localhost',
  };
}

async function startApp(binary, env) {
  const child = spawn(binary, [], { cwd: dirname(binary), stdio: 'ignore', env });
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

async function inspectWindow(child, powershell, env, operation, stage, report) {
  check(child.exitCode === null && child.signalCode === null, 'Installed AngelBot exited before the UI Automation check.');
  const args = ['-NoProfile', '-NonInteractive', '-MTA', '-File', uiaScript, '-TargetPid',
    String(child.pid), '-ExpectedExecutable', child.spawnfile, '-Operation', operation,
    '-DailyHeading', 'AngelBot 日常', '-ComposerName', '消息输入',
    '-SendName', '发送'];
  if (operation === 'Send') args.push('-PromptText', prompt);
  if (operation === 'Reply') args.push('-ReplyText', acknowledgement);
  let stdout, passed;
  try {
    ({ stdout } = await exec(powershell, args, {
      env: { ...env, GITHUB_ACTIONS: process.env.GITHUB_ACTIONS, RUNNER_OS: process.env.RUNNER_OS,
        RUNNER_ENVIRONMENT: process.env.RUNNER_ENVIRONMENT },
      windowsHide: true,
      timeout: 85_000,
    }));
    passed = true;
  } catch (error) {
    stdout = error.stdout;
    passed = false;
  }
  let evidence;
  try {
    evidence = JSON.parse(stdout);
  } catch {
    evidence = {};
  }
  const names = ['processAlive', 'windowFound', 'dailyHeadingFound', 'composerFound',
    'composerFocused', 'valuePatternUsed', 'inputMatched', 'sendEnabled', 'sendInvoked', 'replyFound'];
  const checks = Object.fromEntries(names.map((name) => [name, evidence?.[name] === true]));
  report.uia[stage] = checks;
  check(passed && checks.windowFound && checks.dailyHeadingFound && checks.composerFound
    && (operation !== 'Send' || (checks.inputMatched && checks.sendInvoked))
    && (operation !== 'Reply' || checks.replyFound), 'The installed app UI Automation check failed.');
}

// Read SQLite independently of the UI. Never copy session contents or IDs to
// the published report, and never open the database with write access.
export function readSmokeConversation(path) {
  const database = new DatabaseSync(path, { readOnly: true });
  try {
    database.exec('PRAGMA busy_timeout = 1000; BEGIN;');
    const workspaces = database.prepare(`
      SELECT p.kind, p.active_session_id AS sessionId, s.leaf_message_id AS leafId
      FROM projects p JOIN sessions s ON s.id = p.active_session_id`).all();
    const [workspace] = workspaces;
    const messages = workspaces.length === 1 && workspace.kind === 'personal'
      ? database.prepare(`
          SELECT m.id, m.parent_id AS parentId, m.session_id AS sessionId, m.role, m.content,
                 m.is_provisional AS provisional, m.tool_calls AS toolCalls, m.tool_results AS toolResults,
                 t.status AS runStatus
          FROM messages m LEFT JOIN task_runs t ON t.message_id = m.id
          WHERE m.session_id = ?
          ORDER BY m.created_at, m.id`).all(workspace.sessionId)
      : [];
    database.exec('COMMIT');
    return { workspaces, messages };
  } finally {
    database.close();
  }
}

async function main() {
  const { values } = parseArgs({ options: {
    installer: { type: 'string' }, report: { type: 'string' },
  } });
  check(values.installer && values.report, 'Usage: node scripts/installed_desktop_smoke.mjs --installer path --report path');
  check(process.platform === 'win32' && process.env.GITHUB_ACTIONS === 'true'
    && process.env.RUNNER_OS === 'Windows' && process.env.RUNNER_ENVIRONMENT === 'github-hosted',
  'Installed smoke requires a disposable GitHub-hosted Windows runner.');
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
    reopenMethod: 'normal-window-close-and-new-process',
    checks: {}, uia: {},
  };
  let owned, model, failure;
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
    // Seed the same non-secret connection file used by normal application
    // settings. An empty default auth mode would make the frontend block Send.
    const configDirectory = join(runtime, 'model-config');
    await mkdir(configDirectory, { recursive: true });
    await writeFile(join(configDirectory, 'app_config.json'), JSON.stringify({
      provider: 'ollama', model: modelName, base_url: model.baseUrl,
      auth_mode: 'none', protocol: 'openai_chat_completions', max_tokens: 256, temperature: 0,
    }), { flag: 'wx' });

    report.phase = 'first-launch';
    await saveReport();
    owned = await startApp(binary, env);
    await inspectWindow(owned, powershell, env, 'Ready', 'firstLaunch', report);
    const database = join(runtime, 'data.db');
    const readConversation = () => readSmokeConversation(database);
    const initial = readConversation();
    check(initial.workspaces.length === 1 && initial.workspaces[0].kind === 'personal'
      && initial.workspaces[0].sessionId && initial.messages.length === 0,
    'The fresh app did not create one empty Daily Main conversation.');
    const { sessionId } = initial.workspaces[0];
    report.checks.nativeWebViewReady = true;

    report.phase = 'send';
    await saveReport();
    await inspectWindow(owned, powershell, env, 'Send', 'sentFromComposer', report);
    const completed = await waitFor('the completed persisted reply', () => {
      const snapshot = readConversation();
      return snapshot.messages.some((row) => row.role === 'assistant'
        && row.provisional === 0 && row.content === acknowledgement && row.runStatus === 'completed') ? snapshot : null;
    });
    const user = completed.messages.find((row) => row.role === 'user');
    const assistant = completed.messages.find((row) => row.role === 'assistant');
    check(completed.workspaces.length === 1 && completed.workspaces[0].sessionId === sessionId
      && completed.messages.length === 2 && user?.content === prompt
      && assistant?.id && assistant.parentId === user.id
      && completed.workspaces[0].leafId === assistant.id && user.provisional === 0
      && JSON.parse(assistant.toolCalls ?? '[]')?.length === 0
      && JSON.parse(assistant.toolResults ?? '[]')?.length === 0,
    'Expected one completed tool-free reply linked to its synthetic user turn.');
    await inspectWindow(owned, powershell, env, 'Reply', 'firstReply', report);
    report.checks.durableReply = true;

    report.phase = 'reopen';
    await saveReport();
    await closeApp(owned, powershell, env);
    owned = undefined;
    report.checks.normalExit = true;
    owned = await startApp(binary, env);
    await inspectWindow(owned, powershell, env, 'Reply', 'reopenedReply', report);
    const restored = readConversation();
    check(JSON.stringify(restored) === JSON.stringify(completed),
      'Reopen did not restore the same Daily conversation and completed reply.');
    const requests = model.counters();
    check(requests.accepted === 1 && requests.rejected === 0, 'Expected exactly one synthetic model request across both launches.');
    report.checks.reopenedSameConversation = true;
    await closeApp(owned, powershell, env);
    owned = undefined;
    check(await digest(installer) === report.installerSha256, 'The installer changed during acceptance.');
    report.phase = 'complete';
  } catch (error) {
    failure = error;
  } finally {
    report.modelRequests = model?.counters();
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
