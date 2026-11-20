export async function sendMessage(browser, text) {
  const composer = await browser.$('textarea[placeholder="给 AngelBot 发消息…"]');
  await composer.waitForDisplayed({ timeout: 20_000 });
  await composer.waitForEnabled({ timeout: 10_000 });
  await composer.setValue(text);
  const button = await browser.$('button[title="发送"]');
  await button.waitForEnabled({ timeout: 10_000 });
  await button.click();
}
