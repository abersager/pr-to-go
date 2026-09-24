import { expect, test } from "@playwright/test";

// The shortcuts are menu items in the desktop app; in the browser build the
// page handles the same keys, which is what these tests drive.
test.beforeEach(async ({ request }) => {
  await request.post("http://127.0.0.1:1421/__demo/reset");
});

test("mark files viewed and move on, with the button or a shortcut", async ({ page }) => {
  await page.goto("/#/pr/1/files/Cargo.toml");
  const title = page.locator(".diff-title");
  await expect(title).toHaveText("Cargo.toml");
  await page.getByRole("button", { name: "Viewed, next file →" }).click();
  await expect(title).toHaveText("README.md");
  await expect(page.locator(".file-list-head")).toContainText("1/6 viewed");

  await page.keyboard.press("ControlOrMeta+D");
  await expect(title).toHaveText("docs/architecture.png");
  await expect(page.locator(".file-list-head")).toContainText("2/6 viewed");

  // Back to a viewed file: the next unviewed one skips what's done.
  await page.keyboard.press("ControlOrMeta+[");
  await expect(title).toHaveText("README.md");
  await expect(page.locator(".viewed-toggle input")).toBeChecked();
  await page.keyboard.press("ControlOrMeta+Alt+]");
  await expect(title).toHaveText("docs/architecture.png");
});

test("find in a diff", async ({ page }) => {
  await page.goto("/#/pr/1/files/src%2Fretry.rs");
  await expect(page.locator(".diff-scroll")).toContainText("pub struct Backoff");
  await page.keyboard.press("ControlOrMeta+F");
  const input = page.getByLabel("Find in this file");
  await expect(input).toBeFocused();
  await input.fill("backoff");
  const count = page.locator(".find-count");
  await expect(count).toHaveText(/^1 of \d+$/);
  await expect(page.locator("mark.find-hit.current")).toHaveCount(1);
  await expect(page.locator("mark.find-hit.current")).toHaveText(/backoff/i);

  await input.press("Enter");
  await expect(count).toHaveText(/^2 of/);
  await page.screenshot({ path: "test-results/find.png" });
  await input.press("Shift+Enter");
  await expect(count).toHaveText(/^1 of/);
  await page.keyboard.press("ControlOrMeta+G");
  await expect(count).toHaveText(/^2 of/);

  await input.fill("no such text");
  await expect(count).toHaveText("Not found");
  await input.press("Escape");
  await expect(input).toBeHidden();
  await expect(page.locator("mark.find-hit")).toHaveCount(0);
});

test("shortcuts switch views and pull requests, and are all listed", async ({ page }) => {
  await page.goto("/#/pr/1");
  await expect(page.getByRole("heading", { name: /Retry failed requests/ })).toBeVisible();
  await page.keyboard.press("ControlOrMeta+Alt+2");
  await expect(page.locator(".file-list")).toBeVisible();
  await page.keyboard.press("ControlOrMeta+Alt+1");
  await expect(page.locator(".description")).toBeVisible();

  await page.keyboard.press("ControlOrMeta+2");
  await expect(page.locator(".browse")).toBeVisible();
  await page.keyboard.press("ControlOrMeta+1");
  await expect(page.locator("#add-pr-input")).toBeVisible();
  await page.keyboard.press("ControlOrMeta+N");
  await expect(page.locator("#add-pr-input")).toBeFocused();

  // The inbox lists #3, #2, #1: the previous PR from #1 is #2.
  await page.keyboard.press("ControlOrMeta+Alt+ArrowUp");
  await expect(page.getByRole("heading", { name: /Dark mode for the settings page/ })).toBeVisible();

  await page.keyboard.press("ControlOrMeta+/");
  const dialog = page.getByRole("dialog", { name: "Keyboard shortcuts" });
  await expect(dialog).toContainText("Mark Viewed and Go to Next File");
  await expect(dialog).toContainText("Find Next");
  await page.screenshot({ path: "test-results/shortcuts.png" });
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
});

test("discarding a review asks first; your own PR explains the verdicts", async ({ page }) => {
  await page.goto("/#/pr/1/files/src%2Fretry.rs");
  await page.locator(".line", { hasText: "pub attempts: u32," }).locator(".ln.commentable").click();
  const composer = page.locator(".composer");
  await composer.locator("textarea").fill("Could this default to 3?");
  await expect(composer).toContainText("Saved as draft");
  await composer.getByRole("button", { name: "Done" }).click();

  await page.keyboard.press("ControlOrMeta+Shift+R");
  const panel = page.locator(".review-panel");
  await expect(panel.locator(".review-status")).toHaveText("Draft — only on this computer");
  await page.screenshot({ path: "test-results/review-files.png" });
  await panel.getByRole("button", { name: "Discard…" }).click();
  const confirm = page.getByRole("alertdialog", { name: "Discard review" });
  await expect(confirm).toContainText("Discard this review and its comment?");
  await expect(confirm.getByRole("button", { name: "Keep it" })).toBeFocused();
  await confirm.getByRole("button", { name: "Keep it" }).click();
  await expect(confirm).toBeHidden();
  await expect(page.locator(".draft-card")).toHaveCount(1);

  await panel.getByRole("button", { name: "Discard…" }).click();
  await confirm.getByRole("button", { name: "Discard review" }).click();
  await expect(page.locator(".draft-card")).toHaveCount(0);
  await expect(panel).toContainText("No comments yet");

  // #3 is the viewer's own PR.
  await page.goto("/#/pr/3");
  await expect(page.getByRole("heading", { name: /Regenerate API fixtures/ })).toBeVisible();
  await page.keyboard.press("ControlOrMeta+Shift+R");
  await expect(page.locator(".own-pr-note")).toContainText("GitHub doesn't let authors approve it");
  await expect(page.getByRole("radio", { name: "Approve" })).toBeDisabled();
});
