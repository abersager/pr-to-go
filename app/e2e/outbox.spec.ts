import { type Page, expect, test } from "@playwright/test";

// Every test starts from the same demo world and a fresh app.
test.beforeEach(async ({ request }) => {
  await request.post("http://127.0.0.1:1421/__demo/reset");
});

const DEV = "http://127.0.0.1:1421";

async function comment(page: Page, lineText: string, body: string) {
  await page.locator(".line", { hasText: lineText }).first().locator(".ln.commentable").last().click();
  const composer = page.locator(".composer");
  await composer.locator("textarea").fill(body);
  await expect(composer).toContainText("Saved as draft");
  await composer.getByRole("button", { name: "Done" }).click();
}

test("a review queued offline goes out when the connection is back", async ({ page, request }) => {
  await page.goto("/#/pr/2/files/web%2Ftheme.css");
  await expect(page.locator(".diff-scroll")).toContainText("prefers-color-scheme: dark");
  await comment(page, "--bg: #111;", "Is #111 dark enough for OLED?");

  await request.post(`${DEV}/__demo/offline`);
  try {
    await page.getByRole("button", { name: /^Review/ }).click();
    const panel = page.locator(".review-panel");
    await panel.getByRole("button", { name: "Submit review" }).click();
    await expect(panel).toContainText("Waiting for a connection", { timeout: 15_000 });
    await expect(page.locator(".outbox")).toContainText("waiting for a connection");
  } finally {
    await request.post(`${DEV}/__demo/online`);
  }
  // Checking the connection wakes the outbox.
  await page.locator(".pill").click();
  await expect(page.locator(".review-panel .review-status")).toContainText("Sent", { timeout: 20_000 });
  await expect(page.locator(".outbox")).toContainText("sent");
});

test("when the PR is force-pushed, the review stops and asks", async ({ page, request }) => {
  await page.goto("/#/pr/1/files/src%2Fretry.rs");
  await expect(page.locator(".diff-scroll")).toContainText("pub struct Backoff");
  await comment(page, "pub max: Duration,", "Should max be configurable too?");

  await request.post(`${DEV}/__demo/offline`);
  await page.getByRole("button", { name: /^Review/ }).click();
  const panel = page.locator(".review-panel");
  await panel.getByRole("button", { name: "Submit review" }).click();
  await expect(panel.locator(".review-status")).toContainText(/Queued|Checking|Sending/);
  // The author force-pushes while we're offline.
  await request.post(`${DEV}/__demo/push-retry`);
  await request.post(`${DEV}/__demo/online`);
  await page.locator(".pill").click();

  await expect(panel.locator(".review-status")).toContainText("Needs your attention", { timeout: 20_000 });
  await expect(panel.locator(".attention")).toContainText("The pull request changed since you reviewed it");
  await panel.getByLabel("Send to the new version, and decide per comment").check();
  // The comment's line survived the force-push, two lines further down.
  await expect(panel.locator(".proposal")).toContainText("Unchanged; now at line 13");
  await expect(panel.getByLabel("Move there")).toBeChecked();
  await panel.getByRole("button", { name: "Continue sending" }).click();
  await page.locator(".pill").click();
  await expect(panel.locator(".review-status")).toContainText("Sent", { timeout: 20_000 });
  // It landed on the right line of the new version.
  const thread = page.locator(".thread", { hasText: "Should max be configurable too?" });
  await expect(thread).toBeVisible();
  await page.screenshot({ path: "test-results/outbox.png" });
});

test("a draft can be moved onto a new version before sending", async ({ page, request }) => {
  await page.goto("/#/pr/1/files/src%2Fretry.rs");
  await comment(page, "pub max: Duration,", "Is ten seconds enough?");
  await request.post(`${DEV}/__demo/push-retry`);
  await page.getByRole("button", { name: "Sync now" }).click();
  const banner = page.locator(".rebase-banner", { hasText: "still attached to the earlier version" });
  await expect(banner).toBeVisible();
  await banner.getByRole("button", { name: "Move to this version…" }).click();
  const dialog = page.getByRole("dialog", { name: "Move comments to this version" });
  await expect(dialog.locator(".proposal")).toContainText("Unchanged; now at line 13");
  await dialog.getByText("View changes since your review").click();
  await expect(dialog.locator(".simple-diff", { hasText: "pub deadline: Option<Duration>" })).toBeVisible();
  await page.screenshot({ path: "test-results/rebase.png" });
  await dialog.getByRole("button", { name: "Move comments" }).click();
  await expect(banner).toBeHidden();
  const line13 = page.locator(".line", { hasText: "pub max: Duration," });
  await expect(line13.locator(".ln").last()).toHaveText("13");
  await expect(page.locator(".draft-card", { hasText: "Is ten seconds enough?" })).toBeVisible();
});
