import { render, screen } from "@testing-library/react";
import { expect, test, vi } from "vitest";

vi.mock("./api", () => ({
  api: {
    authStatus: () => Promise.resolve({ signedIn: false, login: null, source: null, scopes: null }),
    connectivity: () => Promise.resolve({ online: false, workOffline: false, detail: null, rateRemaining: null }),
  },
  onCoreEvent: () => Promise.resolve(() => {}),
  inTauri: false,
  localUrl: (p: string) => `prtg://localhost/${p}`,
}));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

const { App } = await import("./App");

test("shows sign-in when there's no account", async () => {
  render(<App />);
  expect(await screen.findByRole("heading", { name: "PR to Go" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Sign in with gh" })).toBeTruthy();
});
