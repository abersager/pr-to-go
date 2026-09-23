import { expect, test } from "@playwright/test";

// Every test starts from the same demo world and a fresh app.
test.beforeEach(async ({ request }) => {
  await request.post("http://127.0.0.1:1421/__demo/reset");
});

test("draft a review: lines, ranges, suggestions, replies, summary, queue", async ({ page }) => {
  await page.goto("/#/pr/1/files/src%2Fretry.rs");
  const diff = page.locator(".diff-scroll");
  await expect(diff).toContainText("pub struct Backoff");

  // A single-line comment, saved as you type.
  await page.locator(".line", { hasText: "pub attempts: u32," }).locator(".ln.commentable").click();
  const composer = page.locator(".composer");
  await expect(composer).toContainText("Comment on line 9");
  await composer.locator("textarea").fill("Could this default to 3?");
  await expect(composer).toContainText("Saved as draft");
  await composer.getByRole("button", { name: "Done" }).click();
  await expect(page.locator(".draft-card")).toContainText("Could this default to 3?");

  // A range with a suggested change.
  await page.locator(".line", { hasText: "pub base: Duration," }).locator(".ln.commentable").click();
  await page.locator(".line", { hasText: "pub max: Duration," }).locator(".ln.commentable").click({ modifiers: ["Shift"] });
  await expect(composer).toContainText("Comment on lines 10–11");
  await expect(page.locator(".line.selected-line")).toHaveCount(2);
  await composer.getByRole("button", { name: "± Suggest change" }).click();
  await expect(composer.locator("textarea")).toHaveValue(/```suggestion\n {4}pub base: Duration,\n {4}pub max: Duration,\n```/);
  await composer.getByRole("button", { name: "Preview" }).click();
  await expect(composer.locator(".suggestion")).toContainText("Suggested change");
  await composer.getByRole("button", { name: "Done" }).click();
  await expect(page.locator(".draft-card")).toHaveCount(2);

  // Reply to Bob's thread.
  const thread = page.locator(".thread", { hasText: "Should attempts be configurable" });
  await thread.getByRole("button", { name: "Reply…" }).click();
  await thread.locator("textarea").fill("Yes, via Backoff.");
  await thread.getByRole("button", { name: "Done" }).click();
  await expect(thread.locator(".draft-card")).toContainText("Yes, via Backoff.");

  // Everything is still there after a reload.
  await page.reload();
  await expect(page.locator(".draft-card")).toHaveCount(3);
  await expect(page.locator(".file-list li", { hasText: "src/retry.rs" }).locator(".count.draft")).toHaveText("3");

  // Summary, verdict, queue.
  await page.getByRole("button", { name: /^Review/ }).click();
  const panel = page.locator(".review-panel");
  await expect(panel).toContainText("3 comments");
  await panel.locator("textarea").fill("Nice work. Two small things.");
  await panel.getByLabel("Approve").check();
  await panel.getByRole("button", { name: "Submit review" }).click();
  await expect(panel.locator(".review-status")).toContainText(/Queued|Sending|Sent|attention/);
  await page.screenshot({ path: "test-results/drafting.png" });
});
