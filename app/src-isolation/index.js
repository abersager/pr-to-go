// Tauri's isolation pattern: every IPC call from the UI passes through this
// sandboxed frame before it reaches the app. Only the calls the UI makes are
// let through, so a script that slipped into the page can't reach anything
// else. See https://v2.tauri.app/concept/inter-process-communication/isolation/
const ALLOWED = new Set([
  "core",
  "reveal_logs",
  "plugin:event|listen",
  "plugin:event|unlisten",
  "plugin:opener|open_url",
]);

window.__TAURI_ISOLATION_HOOK__ = (payload) => {
  if (ALLOWED.has(payload.cmd)) return payload;
  console.warn("PR to Go blocked an IPC call:", payload.cmd);
  // An unknown command fails cleanly instead of leaving the caller waiting.
  return { ...payload, cmd: "__blocked__" };
};
