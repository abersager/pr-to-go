import { expect, test } from "@playwright/test";

// Every test starts from the same demo world and a fresh app.
test.beforeEach(async ({ request }) => {
  await request.post("http://127.0.0.1:1421/__demo/reset");
});

test("review a synced PR: description, checks, diff, threads, context", async ({ page }) => {
  await page.goto("/");
  const pr = page.getByText("Retry failed requests with exponential backoff").first();
  await expect(pr).toBeVisible();
  await pr.click();

  // Conversation: rendered description with its image served locally, checks.
  await expect(page.getByRole("heading", { name: /Retry failed requests/ })).toBeVisible();
  await expect(page.locator(".description li").first()).toContainText("retry::retry");
  const img = page.locator(".description img");
  await expect(img).toHaveAttribute("src", /\/prtg\/asset\//);
  expect(await img.evaluate((i: HTMLImageElement) => i.naturalWidth)).toBeGreaterThan(0);
  await expect(page.locator(".checks")).toContainText("clippy");
  await page.screenshot({ path: "test-results/conversation.png" });

  // Files: pick the retry helper; Bob's comment sits under its line.
  await page.getByRole("button", { name: /^Files/ }).click();
  await page.locator(".file-list li", { hasText: "src/retry.rs" }).click();
  await expect(page.locator(".diff-scroll")).toContainText("pub struct Backoff");
  await expect(page.locator(".thread", { hasText: "Should attempts be configurable" })).toBeVisible();

  // client.rs: the outdated thread is tucked away; expanding context works.
  await page.locator(".file-list li", { hasText: "src/client.rs" }).click();
  await expect(page.locator(".file-threads summary")).toContainText("1 outdated thread");
  const expander = page.locator(".expander button", { hasText: /Show \d+ unchanged lines/ }).first();
  await expander.click();
  await expect(page.locator(".diff-scroll")).toContainText("pub price_cents: u64");
  // Syntax highlighting arrived from the worker.
  await expect(page.locator(".diff-scroll .tk").first()).toBeVisible();
  await page.screenshot({ path: "test-results/diff.png" });

  // Unified mode.
  await page.getByRole("button", { name: "Unified" }).click();
  await expect(page.locator(".line.unified").first()).toBeVisible();

  // The generated file (.gitattributes) GitHub sent no patch for: collapsed
  // at first; shown, it's a local diff with comments off.
  await page.getByRole("button", { name: "Regenerate API fixtures" }).or(page.getByText("Regenerate API fixtures")).first().click();
  await page.getByRole("button", { name: /^Files/ }).click();
  const fixtures = page.locator(".file-list li", { hasText: "widgets.json" });
  await expect(fixtures.locator(".tag")).toHaveText("generated");
  await fixtures.click();
  await expect(page.locator(".generated-note")).toContainText("generated");
  await page.screenshot({ path: "test-results/generated.png" });
  await page.getByRole("button", { name: "Show diff" }).click();
  await expect(page.locator(".notice.warn")).toContainText("computed locally");
});

test("keeps working when GitHub is unreachable", async ({ page, request }) => {
  await page.goto("/");
  await page.getByText("Dark mode for the settings page").first().click();
  await request.post("http://127.0.0.1:1421/__demo/offline");
  try {
    await page.getByRole("button", { name: "Sync now" }).click();
    await expect(page.locator(".pr-sync .error")).toBeVisible();
    await expect(page.locator(".pill")).toContainText("Offline");
    // Still fully reviewable.
    await page.getByRole("button", { name: /^Files/ }).click();
    await page.locator(".file-list li", { hasText: "web/theme.css" }).click();
    await expect(page.locator(".diff-scroll")).toContainText("prefers-color-scheme: dark");
    await page.screenshot({ path: "test-results/offline.png" });
  } finally {
    await request.post("http://127.0.0.1:1421/__demo/online");
  }
});
