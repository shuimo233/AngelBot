import assert from 'node:assert/strict';
import { join } from 'node:path';
import { sendMessage } from './conversation-fixture.mjs';

describe('native conversation presentation contracts', () => {
  it('separately verifies personality, composer layout and typography', async () => {
    const phase = process.env.ANGELBOT_DESKTOP_E2E_MODEL_RECOVERY;
    const evidence = process.env.ANGELBOT_DESKTOP_E2E_RECOVERY_EVIDENCE_DIR;

    if (phase === 'first') {
      await browser.execute(() => window.dispatchEvent(new CustomEvent('open-settings', { detail: 'personality' })));
      const settings = await browser.$('[role="dialog"][aria-modal="true"]');
      const preview = await settings.$('.personality-preview');
      const currentReply = await preview.$('.personality-preview-replies > div:first-child dd');
      await currentReply.waitForDisplayed({ timeout: 5_000 });
      const original = await currentReply.getText();
      const editor = await settings.$('.personality-basic-fields textarea');
      assert.equal(await editor.getValue(), '');
      await settings.$('button=简洁协作').click();
      assert.ok((await editor.getValue()).trim());
      const draftReply = await preview.$('.personality-preview-draft dd');
      await draftReply.waitForDisplayed({ timeout: 5_000 });
      const expected = await draftReply.getText();
      assert.notEqual(expected, original);
      assert.equal(await currentReply.getText(), original);
      await settings.$('.settings-save-btn').click();
      await browser.waitUntil(async () => (await currentReply.getText()) === expected && !await draftReply.isExisting(), { timeout: 5_000 });
      await browser.saveScreenshot(join(evidence, 'personality-settings.png'));
      await settings.$('button[aria-label="关闭"]').click();

      await sendMessage(browser, 'native-ui-idle：只用于验收排版，不执行操作。');
      await browser.waitUntil(async () => (await browser.$('.messages').getText()).includes('native-ui-idle-ok'), { timeout: 20_000 });
      const composerLayout = () => browser.execute(() => {
        const bar = document.querySelector('.composer-bar').getBoundingClientRect();
        const input = document.querySelector('.composer-textarea').getBoundingClientRect();
        const toolbar = document.querySelector('.composer-toolbar-row').getBoundingClientRect();
        const actions = document.querySelector('.composer-bar-right').getBoundingClientRect();
        const popup = document.querySelector('.environment-panel')?.getBoundingClientRect();
        const modelNotice = document.querySelector('.model-config-notice')?.getBoundingClientRect();
        const modelButton = document.querySelector('.model-config-notice__actions .is-primary')?.getBoundingClientRect();
        const footer = [...document.querySelectorAll('.message-row.user .message-footer')].at(-1);
        const time = footer.querySelector('.message-time').getBoundingClientRect();
        const edit = footer.querySelector('.action-btn').getBoundingClientRect();
        return {
          viewport: innerWidth,
          height: innerHeight,
          oneSurface: bar.top <= input.top && input.bottom <= toolbar.top && toolbar.bottom <= bar.bottom,
          fullWidthInput: bar.width - input.width <= 28,
          actionsFit: actions.right <= bar.right && bar.bottom <= innerHeight,
          metadataOnOneLine: Math.abs((time.top + time.bottom) - (edit.top + edit.bottom)) <= 2,
          configurationFits: !popup || (popup.left >= bar.left && popup.right <= bar.right),
          modelNoticeFits: !modelButton || modelButton.bottom <= modelNotice.bottom,
        };
      });
      const assertComposerLayout = ({ viewport, height, ...checks }) => {
        assert.ok(Object.values(checks).every(Boolean), JSON.stringify({ viewport, height, ...checks }));
      };
      const normalSize = await browser.getWindowSize();
      const idle = await composerLayout();
      assertComposerLayout(idle);
      await browser.saveScreenshot(join(evidence, 'conversation-composer-idle.png'));
      await browser.setWindowSize(430, 560);
      await browser.waitUntil(async () => (await composerLayout()).viewport <= 430, { timeout: 5_000 });
      const wasDark = await browser.execute(() => {
        const root = document.documentElement;
        const dark = root.classList.contains('dark');
        root.classList.add('dark');
        return dark;
      });
      const configuration = await browser.$('.environment-disclosure-trigger');
      await configuration.click();
      await browser.$('.environment-panel').waitForDisplayed();
      const compactIdle = await composerLayout();
      assertComposerLayout(compactIdle);
      await configuration.click();
      await browser.execute(() => {
        const transcript = document.querySelector('.messages');
        transcript.scrollTop = transcript.scrollHeight;
      });
      const draft = await browser.$('.composer-textarea');
      const blankHeight = (await draft.getSize()).height;
      await draft.setValue('请把前面的结果整理成一份简短清单，分开写已完成事项、需要确认的细节和下一步安排，最后标出最重要的一项。');
      await browser.waitUntil(async () => (await draft.getSize()).height > blankHeight, { timeout: 5_000 });
      assertComposerLayout(await composerLayout());
      await browser.saveScreenshot(join(evidence, 'conversation-composer-compact.png'));
      await browser.execute((dark) => document.documentElement.classList.toggle('dark', dark), wasDark);
      await browser.setWindowSize(normalSize.width, normalSize.height);
      await browser.waitUntil(async () => (await composerLayout()).viewport === idle.viewport, { timeout: 5_000 });

      await sendMessage(browser, 'native-ui-running：保持等待，用于检查运行中的布局。');
      await browser.waitUntil(async () => (await browser.$('.messages').getText()).includes('native-model-waiting'), {
        timeout: 20_000, interval: 100, timeoutMsg: 'Owned model did not enter its pending streaming future',
      });
      const conversationLayout = () => browser.execute(() => {
        const transcript = document.querySelector('.messages');
        const identity = document.querySelector('.chat-header-identity').getBoundingClientRect();
        const title = document.querySelector('.chat-header-session-title').getBoundingClientRect();
        return {
          viewport: innerWidth,
          transcriptStart: transcript.getBoundingClientRect().left + parseFloat(getComputedStyle(transcript).paddingLeft),
          composerStart: document.querySelector('.composer-input-root').getBoundingClientRect().left,
          headerStart: identity.left,
          titleFits: title.width > 0 && title.left >= identity.left && title.right <= identity.right + 1,
          pageWidth: document.documentElement.scrollWidth,
        };
      });
      const assertConversationLayout = layout => {
        assert.ok(Math.abs(layout.transcriptStart - layout.composerStart) <= 1, JSON.stringify(layout));
        assert.ok(layout.titleFits && layout.pageWidth <= layout.viewport + 1, JSON.stringify(layout));
        // The header's identity group includes the assistant avatar in personal chat.
        if (layout.viewport > 959) assert.ok(Math.abs(layout.transcriptStart - layout.headerStart) <= 1, JSON.stringify(layout));
      };
      const wideLayout = await conversationLayout();
      assertConversationLayout(wideLayout);
      const running = await composerLayout();
      assertComposerLayout(running);
      await browser.saveScreenshot(join(evidence, 'active-run.png'));
      await browser.setWindowSize(820, 650);
      await browser.waitUntil(async () => (await conversationLayout()).viewport < wideLayout.viewport, { timeout: 5_000 });
      const compactLayout = await conversationLayout();
      assertConversationLayout(compactLayout);
      const compactRunning = await composerLayout();
      assertComposerLayout(compactRunning);
      await browser.saveScreenshot(join(evidence, 'active-run-compact.png'));
      await browser.setWindowSize(normalSize.width, normalSize.height);
      await browser.$('button[title="停止运行"]').click();
      await browser.$('textarea[placeholder="给 AngelBot 发消息…"]').waitForEnabled({ timeout: 10_000 });
    }
    const typography = () => browser.execute(() => {
      const read = (selector) => {
        const style = getComputedStyle(document.querySelector(selector));
        return { size: style.fontSize, family: style.fontFamily };
      };
      return { input: read('.composer-textarea'), reply: read('.message-row.assistant .message-bubble'), navigation: read('.sidebar') };
    });
    if (phase === 'first') {
      const defaults = await typography();
      assert.equal(defaults.input.size, '14px');
      assert.equal(defaults.reply.size, '14px');
      await browser.execute(() => window.dispatchEvent(new CustomEvent('open-settings', { detail: 'appearance' })));
      const settingsDialog = await browser.$('[role="dialog"][aria-modal="true"]');
      const fontField = await settingsDialog.$('input[list]');
      await fontField.waitForDisplayed({ timeout: 5_000 });
      await fontField.setValue('KaiTi');
      await browser.execute(() => {
        const slider = document.querySelector('#chat-font-size');
        slider.focus();
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(slider, '18');
        slider.dispatchEvent(new Event('input', { bubbles: true }));
      });
      const preview = await browser.execute(() => {
        const style = getComputedStyle(document.querySelector('.appearance-reading-preview'));
        return { size: style.fontSize, family: style.fontFamily };
      });
      assert.equal(preview.size, '18px');
      assert.match(preview.family, /KaiTi/);
      await browser.saveScreenshot(join(evidence, 'chat-typography-settings.png'));
      await settingsDialog.$('button[aria-label="关闭"]').click();
      assert.deepEqual((await typography()).navigation, defaults.navigation);
    }
    const styled = await typography();
    for (const style of [styled.input, styled.reply]) {
      assert.equal(style.size, '18px');
      assert.match(style.family, /KaiTi/);
    }
    await browser.saveScreenshot(join(evidence, `ui-${phase}.png`));
  });
});
