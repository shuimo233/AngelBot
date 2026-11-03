import assert from 'node:assert/strict';

// Cross-confirmation evidence belongs to the durable ledger, not a compacted
// model prompt. Parent links order slices; call ordinals order this sequential
// script within a slice. Result-array order carries no chronology.
export function assertOwnedDesktopControlJourney(messages, task) {
  const byId = new Map(messages.map((message) => [message.id, message]));
  assert.equal(byId.size, messages.length, 'duplicate durable message identity');
  const users = messages.filter((message) => message.role === 'user' && message.content === task);
  assert.equal(users.length, 1, 'owned control task must have one user origin');
  const descendsFrom = (message, ancestorId) => {
    const seen = new Set();
    while (message?.parentId) {
      assert.ok(!seen.has(message.id), 'cycle in durable task ancestry');
      seen.add(message.id);
      if (message.parentId === ancestorId) return true;
      message = byId.get(message.parentId);
      assert.ok(message, 'missing durable task ancestor');
    }
    return false;
  };
  const taskMessages = messages.filter((message) => descendsFrom(message, users[0].id));
  for (const message of taskMessages.filter((item) => item.toolCalls?.some((call) => call.id.startsWith('cu-')))) {
    assert.ok(message.taskRun?.goal === task, 'continuation must retain the user goal, not nest its own prompt');
    assert.ok(message.taskFacts?.goal === task, 'continuation facts must retain the same user goal');
  }
  const calls = taskMessages.flatMap((message) => (message.toolCalls ?? [])
    .map((call, ordinal) => ({ call, message, ordinal })));
  const results = taskMessages.flatMap((message) => message.toolResults ?? []);
  const phases = ['before', 'after-check', 'after-uncheck'];
  const actionIds = ['cu-check', 'cu-uncheck'];
  const expectedIds = phases.flatMap((phase, index) => [
    `cu-root-${phase}`, `cu-region-${phase}`, ...(index < 2 ? [actionIds[index]] : []),
  ]);
  const isCompaction = (id, name) => /^context-compaction-\d+$/.test(id) && name === 'context_compaction';
  assert.deepEqual(calls.filter(({ call }) => !isCompaction(call.id, call.name))
    .map(({ call }) => call.id).sort(), [...expectedIds].sort(),
  'owned task must contain exactly the expected desktop calls, apart from compression metadata');
  assert.deepEqual(results.filter((result) => !isCompaction(result.callId, result.toolName))
    .map((result) => result.callId).sort(), [...expectedIds].sort(),
  'owned task must contain exactly the paired results, apart from compression metadata');
  const pair = (id, name) => {
    const matchingCalls = calls.filter(({ call }) => call.id === id);
    const matchingResults = results.filter((result) => result.callId === id);
    assert.equal(matchingCalls.length, 1, 'desktop call must be unique');
    assert.equal(matchingResults.length, 1, 'desktop result must pair uniquely');
    const entry = matchingCalls[0];
    const result = matchingResults[0];
    assert.equal(entry.message.role, 'assistant');
    assert.equal(entry.call.name, name);
    assert.equal(result.toolName, name);
    assert.equal(result.success, true, 'desktop result must succeed');
    assert.ok(!result.confirmationRequired, 'desktop result must be settled');
    try {
      return { ...entry, result, args: JSON.parse(entry.call.arguments),
        output: JSON.parse(result.output.replace(/^Result: /, '').replace(/^Confirmation approved: /, '')) };
    } catch {
      assert.fail('desktop evidence must contain structured arguments and output');
    }
  };
  const ordered = (left, right) => left.message.id === right.message.id
    ? left.ordinal < right.ordinal : descendsFrom(right.message, left.message.id);
  const validRef = (reference) => typeof reference === 'string' && /^[A-Za-z0-9_-]{1,128}$/.test(reference);
  const exactArgs = (args, expected) => Object.keys(args).length === Object.keys(expected).length
    && Object.entries(expected).every(([key, value]) => args[key] === value);
  const appId = 'desktop-e2e-window';
  const regionRefs = new Set();
  const controlRefs = new Set();
  let previousAction;
  for (const [index, phase] of phases.entries()) {
    const root = pair(`cu-root-${phase}`, 'observe_trusted_app_window');
    const scoped = pair(`cu-region-${phase}`, 'observe_trusted_app_window');
    assert.ok(exactArgs(root.args, { app_id: appId, mode: 'controls' }), 'root observation scope mismatch');
    assert.equal(root.output.appId, appId);
    assert.ok(!root.output.regionRef, 'root observation must not be scoped');
    const regions = (root.output.regions ?? []).filter((region) => region.name === 'Fixture region A');
    assert.equal(regions.length, 1, 'root must identify one exact region');
    assert.equal(regions[0].role, 'group');
    const regionRef = regions[0].regionRef;
    assert.ok(validRef(regionRef) && !regionRefs.has(regionRef), 'region reference must be fresh');
    regionRefs.add(regionRef);
    assert.ok(exactArgs(scoped.args, { app_id: appId, mode: 'controls', region_ref: regionRef }),
      'scoped observation must use the exact root reference');
    assert.equal(scoped.output.appId, appId);
    assert.ok(scoped.output.regionRef === regionRef, 'scoped result reference mismatch');
    assert.equal(scoped.output.truncated, false);
    assert.equal((scoped.output.diagnostics ?? []).length, 0);
    const controls = (scoped.output.controls ?? []).filter((control) =>
      control.role === 'checkBox' && control.name === 'Region checkbox');
    assert.equal(controls.length, 1, 'scope must identify one exact checkbox');
    assert.equal(controls[0].role, 'checkBox');
    assert.equal(controls[0].enabled, true);
    assert.ok(controls[0].capabilities.includes('toggle'));
    assert.deepEqual(controls[0].state, { toggle: index === 1 ? 'on' : 'off' },
      'fresh observation must independently read the expected checkbox state');
    const controlRef = controls[0].controlRef;
    assert.ok(validRef(controlRef) && !controlRefs.has(controlRef), 'control reference must be fresh');
    controlRefs.add(controlRef);
    assert.ok(ordered(root, scoped), 'root must precede scoped observation');
    if (previousAction) assert.ok(ordered(previousAction, root), 'new observation must follow the previous effect');
    if (index < 2) {
      const operation = ['check', 'uncheck'][index];
      const action = pair(`cu-${operation}`, 'operate_trusted_app_control');
      assert.ok(exactArgs(action.args, { app_id: appId, control_ref: controlRef, action: operation }),
        'effect must target the exact current observed control');
      assert.equal(action.result.confirmationStatus, 'approved');
      assert.match(action.result.output.replace(/^Result: /, ''), /^Confirmation approved: /);
      assert.equal(action.output.adapter, 'windows-native-uia');
      assert.equal(action.output.status, 'verified');
      assert.equal(action.output.target, appId);
      assert.equal(action.output.action, operation);
      assert.equal(typeof action.output.detail, 'string');
      assert.ok(ordered(scoped, action), 'scoped observation must precede its effect');
      previousAction = action;
    }
  }
}
