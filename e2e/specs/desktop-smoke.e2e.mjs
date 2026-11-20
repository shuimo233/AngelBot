import assert from 'node:assert/strict';
import { sendMessage } from './conversation-fixture.mjs';
import { waitForVisible } from '../helpers/visibility.mjs';

const prompt = 'angelbot-ci-smoke: 请确认收到测试消息，只回复文字。';
const response = 'angelbot-ci-smoke-ok: 已收到测试消息。';

describe('native desktop smoke', () => {
  it('opens Daily and preserves a completed conversation across reload', async () => {
    const location = await browser.tauri.execute(() => window.location.href);
    assert.match(location, /^https?:\/\/tauri\.localhost\//);

    const workspaces = await browser.tauri.execute(({ core }) => core.invoke('get_workspaces'));
    assert.deepEqual(workspaces.map(({ kind }) => kind), ['personal']);
    const { activeSessionId: sessionId, name } = workspaces[0];
    assert.ok(sessionId, 'the fresh Daily workspace needs a durable Main session');

    const readMessages = () => browser.tauri.execute(({ core }, sessionId) =>
      core.invoke('get_messages', { sessionId }), sessionId);
    assert.deepEqual(await readMessages(), []);
    await browser.waitUntil(async () => (await browser.$('.chat-header-session-title').getText()) === name, {
      timeout: 10_000,
      timeoutMsg: 'the Daily conversation did not open',
    });

    await sendMessage(browser, prompt);
    let messages;
    await browser.waitUntil(async () => {
      messages = await readMessages();
      return messages.some((message) => message.role === 'assistant' && message.content === response);
    }, { timeout: 20_000, timeoutMsg: 'the scripted reply did not become durable' });

    assert.deepEqual(messages.map(({ role }) => role), ['user', 'assistant']);
    const [user, assistant] = messages;
    assert.ok(user.id && assistant.id && user.id !== assistant.id);
    assert.equal(user.content, prompt);
    assert.equal(assistant.parentId, user.id);
    assert.equal(assistant.taskRun?.status, 'completed');
    assert.deepEqual(assistant.toolCalls ?? [], []);
    assert.deepEqual(assistant.toolResults ?? [], []);
    const visibleReply = `//*[@data-conversation-message-id="${assistant.id}"]//*[text()="${response}"]`;
    await waitForVisible(browser, visibleReply, { timeout: 10_000 });

    await browser.refresh();
    await waitForVisible(browser, visibleReply, { timeout: 10_000 });
    const conversationIdentity = (items) => items.map(({ id, parentId, sessionId, content }) =>
      ({ id, parentId, sessionId, content }));
    assert.deepEqual(conversationIdentity(await readMessages()), conversationIdentity(messages));
  });
});
