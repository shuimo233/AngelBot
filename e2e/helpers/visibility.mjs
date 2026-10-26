/** Wait for the current rendered node, including across React replacements. */
export async function waitForVisible(browser, selector, options) {
  return browser.waitUntil(async () => (await browser.$(selector)).isDisplayed(), options);
}
