import path from 'node:path';
import { mkdir, writeFile } from 'node:fs/promises';

async function boundedDiagnostic(operation) {
  let timer;
  try {
    return await Promise.race([operation, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error('diagnostic exceeded 3s')), 3_000);
    })]);
  } finally { clearTimeout(timer); }
}

const appBinaryPath = process.env.ANGELBOT_DESKTOP_E2E_BINARY_PATH;
const replyExportProbe = process.env.ANGELBOT_DESKTOP_E2E_REPLY_EXPORT === '1';
const modelRecoveryProbe = Boolean(process.env.ANGELBOT_DESKTOP_E2E_MODEL_RECOVERY);
const projectRestoreProbe = Boolean(process.env.ANGELBOT_DESKTOP_E2E_PROJECT_RESTORE);
if (!appBinaryPath || !path.isAbsolute(appBinaryPath)) {
  throw new Error('Run desktop E2E through python scripts/desktop_e2e.py; its isolated binary path is required.');
}

export const config = {
  runner: 'local',
  specs: [projectRestoreProbe ? './specs/project-restore.e2e.mjs'
    : modelRecoveryProbe ? './specs/model-recovery.e2e.mjs'
    : replyExportProbe ? './specs/reply-export.e2e.mjs' : './specs/first-run.e2e.mjs'],
  maxInstances: 1,
  capabilities: [{
    browserName: 'tauri',
    'tauri:options': { application: appBinaryPath },
    'wdio:tauriServiceOptions': {
      appBinaryPath,
      driverProvider: 'embedded',
      captureBackendLogs: true,
    },
  }],
  services: [['@wdio/tauri-service', {
    appBinaryPath,
    driverProvider: 'embedded',
    captureBackendLogs: true,
  }]],
  framework: 'mocha',
  reporters: ['spec'],
  logLevel: 'warn',
  async afterTest(test, _context, { passed, error, duration }) {
    // Only the fixed synthetic CI journey is publishable. Never collect from
    // local/manual/model-recovery profiles or upload their runtime databases.
    if (passed || process.env.GITHUB_ACTIONS !== 'true' || replyExportProbe || modelRecoveryProbe || projectRestoreProbe) return;
    const run = `${process.env.GITHUB_RUN_ID}-${process.env.GITHUB_RUN_ATTEMPT}`.replace(/[^\w-]/g, '_');
    const directory = path.join(process.cwd(), 'src-tauri', 'target', 'e2e', 'failure', run);
    const report = { title: String(test.title).slice(0, 200), duration,
      error: String(error).slice(0, 1500), stack: error?.stack?.slice(0, 2000) };
    try {
      report.state = await boundedDiagnostic(browser.tauri.execute(async ({ core }) => {
        const clip = (value, limit) => typeof value === 'string' ? value.slice(0, limit) : undefined;
        const ids = [...new Set([window.__angelbotE2ePersonalSessionId, window.__angelbotE2eProjectSessionId])]
          .filter(id => typeof id === 'string').slice(0, 2);
        const sessions = [];
        for (const sessionId of ids) {
          try {
            const messages = await core.invoke('get_messages', { sessionId });
            sessions.push({ sessionId, messages: messages.slice(-5).map(message => ({
              role: message.role, content: clip(message.content, 1000),
              run: message.taskRun && { status: message.taskRun.status, resumable: message.taskRun.resumable,
                confirmation: message.taskRun.confirmationState },
              calls: message.toolCalls?.slice(-3).map(call => ({ id: clip(call.id, 128), name: clip(call.name, 100) })),
              results: message.toolResults?.slice(-3).map(result => ({ callId: clip(result.callId, 128),
                success: result.success, confirmation: result.confirmationStatus, output: clip(result.output, 500) })),
            })) });
          } catch (failure) { sessions.push({ sessionId, error: String(failure).slice(0, 500) }); }
        }
        return { sessions, visibleConversation: document.querySelector('.messages')?.textContent?.slice(-4000) };
      }));
    } catch (failure) { report.diagnosticsError = String(failure).slice(0, 500); }
    try {
      await mkdir(directory, { recursive: true });
      try { await boundedDiagnostic(browser.saveScreenshot(path.join(directory, 'page.png'))); }
      catch (failure) { report.screenshotError = String(failure).slice(0, 500); }
      await writeFile(path.join(directory, 'failure.json'), JSON.stringify(report, null, 2), 'utf8');
    } catch (failure) { report.artifactError = String(failure).slice(0, 500); }
    console.error('Synthetic desktop failure evidence:', JSON.stringify(report));
    // Evidence collection must never replace the original test failure.
  },
  // This is an end-to-end product journey, not a single RPC assertion.  Each
  // wait in the spec still has its own short, diagnostic timeout; the outer
  // budget simply must cover setup plus the independent delegated-review loop.
  mochaOpts: { ui: 'bdd', timeout: projectRestoreProbe ? 600_000 : replyExportProbe || modelRecoveryProbe ? 300_000 : 90_000 },
};
