import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const appBinaryPath = path.join(root, 'src-tauri', 'target', 'debug', 'angelbot.exe');

export const config = {
  runner: 'local',
  specs: ['./specs/**/*.e2e.mjs'],
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
  mochaOpts: { ui: 'bdd', timeout: 90_000 },
};
