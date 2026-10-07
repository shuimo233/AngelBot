import '@testing-library/jest-dom';

if (typeof globalThis !== 'undefined' && !('scrollTo' in globalThis)) {
  (globalThis as any).scrollTo = () => {};
}

// jsdom does not implement window.setInterval/clearInterval — stub them
// to prevent uncaught errors in tests (agent-event.ts uses them for polling).
if (typeof globalThis !== 'undefined') {
  (globalThis as any).setInterval = (globalThis as any).setInterval ?? (() => 1);
  (globalThis as any).clearInterval = (globalThis as any).clearInterval ?? (() => {});
}
