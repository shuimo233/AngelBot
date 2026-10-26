import path from 'node:path';

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
  // This is an end-to-end product journey, not a single RPC assertion.  Each
  // wait in the spec still has its own short, diagnostic timeout; the outer
  // budget simply must cover setup plus the independent delegated-review loop.
  mochaOpts: { ui: 'bdd', timeout: projectRestoreProbe ? 600_000 : replyExportProbe || modelRecoveryProbe ? 300_000 : 90_000 },
};
