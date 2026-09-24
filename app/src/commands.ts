// App commands. One list drives the macOS menu bar, the keyboard shortcuts
// (in the browser build, where there's no native menu) and the shortcuts
// overview. Components say what a command does while they're on screen with
// `useCommand`; the handler registered last wins, and a command nobody can
// run right now is greyed out.

import { useEffect, useLayoutEffect, useRef } from "react";

export type CommandId =
  | "app.settings"
  | "pr.add"
  | "pr.sync"
  | "inbox.syncAll"
  | "pr.openOnGitHub"
  | "net.workOffline"
  | "find.open"
  | "find.next"
  | "find.previous"
  | "find.useSelection"
  | "view.inbox"
  | "view.browse"
  | "view.sidebar"
  | "view.conversation"
  | "view.files"
  | "view.unified"
  | "view.zoomIn"
  | "view.zoomOut"
  | "view.actualSize"
  | "go.nextPr"
  | "go.prevPr"
  | "go.nextFile"
  | "go.prevFile"
  | "go.nextUnviewed"
  | "review.panel"
  | "file.viewedNext"
  | "file.toggleViewed"
  | "file.comment"
  | "review.verdictComment"
  | "review.verdictApprove"
  | "review.verdictRequestChanges"
  | "review.submit"
  | "review.edit"
  | "review.retry"
  | "review.copy"
  | "review.discard"
  | "help.shortcuts"
  | "help.project";

export type CommandDef = {
  label: string;
  /** Label when checked, for items that read "Show …" / "Hide …". */
  labelOn?: string;
  /** Tauri accelerator syntax. */
  accel?: string;
  /** Only on macOS (the key would clash elsewhere). */
  macOnly?: boolean;
  /** Shown with a checkmark when checked. */
  check?: boolean;
};

export const COMMANDS: Record<CommandId, CommandDef> = {
  "app.settings": { label: "Settings…", accel: "CmdOrCtrl+," },
  "pr.add": { label: "Add Pull Request…", accel: "CmdOrCtrl+N" },
  "pr.sync": { label: "Sync Pull Request", accel: "CmdOrCtrl+R" },
  "inbox.syncAll": { label: "Sync All", accel: "CmdOrCtrl+Alt+R" },
  "pr.openOnGitHub": { label: "Open on GitHub", accel: "CmdOrCtrl+Shift+O" },
  "net.workOffline": { label: "Work Offline", accel: "CmdOrCtrl+Alt+O", check: true },
  "find.open": { label: "Find…", accel: "CmdOrCtrl+F" },
  "find.next": { label: "Find Next", accel: "CmdOrCtrl+G" },
  "find.previous": { label: "Find Previous", accel: "CmdOrCtrl+Shift+G" },
  "find.useSelection": { label: "Use Selection for Find", accel: "CmdOrCtrl+E" },
  "view.inbox": { label: "Inbox", accel: "CmdOrCtrl+1", check: true },
  "view.browse": { label: "Browse", accel: "CmdOrCtrl+2", check: true },
  "view.sidebar": { label: "Show Sidebar", labelOn: "Hide Sidebar", accel: "Ctrl+Cmd+S", macOnly: true },
  "view.conversation": { label: "Conversation", accel: "CmdOrCtrl+Alt+1", check: true },
  "view.files": { label: "Files", accel: "CmdOrCtrl+Alt+2", check: true },
  "view.unified": { label: "Unified Diff", accel: "CmdOrCtrl+Alt+U", check: true },
  "view.zoomIn": { label: "Zoom In", accel: "CmdOrCtrl+=" },
  "view.zoomOut": { label: "Zoom Out", accel: "CmdOrCtrl+-" },
  "view.actualSize": { label: "Actual Size", accel: "CmdOrCtrl+0" },
  "go.nextPr": { label: "Next Pull Request", accel: "CmdOrCtrl+Alt+Down" },
  "go.prevPr": { label: "Previous Pull Request", accel: "CmdOrCtrl+Alt+Up" },
  "go.nextFile": { label: "Next File", accel: "CmdOrCtrl+]" },
  "go.prevFile": { label: "Previous File", accel: "CmdOrCtrl+[" },
  "go.nextUnviewed": { label: "Next Unviewed File", accel: "CmdOrCtrl+Alt+]" },
  "review.panel": { label: "Show Review", labelOn: "Hide Review", accel: "CmdOrCtrl+Shift+R" },
  "file.viewedNext": { label: "Mark Viewed and Go to Next File", accel: "CmdOrCtrl+D" },
  "file.toggleViewed": { label: "Viewed", accel: "CmdOrCtrl+Shift+D", check: true },
  "file.comment": { label: "Comment on File…", accel: "CmdOrCtrl+Shift+M" },
  "review.verdictComment": { label: "Comment", accel: "Ctrl+Cmd+1", macOnly: true, check: true },
  "review.verdictApprove": { label: "Approve", accel: "Ctrl+Cmd+2", macOnly: true, check: true },
  "review.verdictRequestChanges": { label: "Request Changes", accel: "Ctrl+Cmd+3", macOnly: true, check: true },
  "review.submit": { label: "Submit Review", accel: "CmdOrCtrl+Shift+Enter" },
  "review.edit": { label: "Edit Queued Review" },
  "review.retry": { label: "Try Sending Now" },
  "review.copy": { label: "Copy Review as Markdown", accel: "CmdOrCtrl+Alt+C" },
  "review.discard": { label: "Discard Review…" },
  "help.shortcuts": { label: "Keyboard Shortcuts", accel: "CmdOrCtrl+/" },
  "help.project": { label: "PR to Go on GitHub" },
};

export type Predefined =
  | "About"
  | "Services"
  | "Hide"
  | "HideOthers"
  | "ShowAll"
  | "Quit"
  | "CloseWindow"
  | "Undo"
  | "Redo"
  | "Cut"
  | "Copy"
  | "Paste"
  | "SelectAll"
  | "Fullscreen"
  | "Minimize"
  | "Maximize"
  | "BringAllToFront";

export type MenuEntry = CommandId | "-" | { predefined: Predefined } | { submenu: string; items: MenuEntry[] };

export type MenuDef = { title: string; role?: "app" | "window" | "help"; items: MenuEntry[] };

const p = (predefined: Predefined): MenuEntry => ({ predefined });

export const MENUS: MenuDef[] = [
  {
    title: "PR to Go",
    role: "app",
    items: [
      p("About"),
      "-",
      "app.settings",
      "-",
      p("Services"),
      "-",
      p("Hide"),
      p("HideOthers"),
      p("ShowAll"),
      "-",
      p("Quit"),
    ],
  },
  {
    title: "File",
    items: ["pr.add", "-", "pr.sync", "inbox.syncAll", "-", "pr.openOnGitHub", "-", "net.workOffline", "-", p("CloseWindow")],
  },
  {
    title: "Edit",
    items: [
      p("Undo"),
      p("Redo"),
      "-",
      p("Cut"),
      p("Copy"),
      p("Paste"),
      p("SelectAll"),
      "-",
      { submenu: "Find", items: ["find.open", "find.next", "find.previous", "find.useSelection"] },
    ],
  },
  {
    title: "View",
    items: [
      "view.inbox",
      "view.browse",
      "view.sidebar",
      "-",
      "view.conversation",
      "view.files",
      "-",
      "view.unified",
      "-",
      "view.zoomIn",
      "view.zoomOut",
      "view.actualSize",
      "-",
      p("Fullscreen"),
    ],
  },
  { title: "Go", items: ["go.nextPr", "go.prevPr", "-", "go.nextFile", "go.prevFile", "go.nextUnviewed"] },
  {
    title: "Review",
    items: [
      "review.panel",
      "-",
      "file.viewedNext",
      "file.toggleViewed",
      "file.comment",
      "-",
      { submenu: "Verdict", items: ["review.verdictComment", "review.verdictApprove", "review.verdictRequestChanges"] },
      "-",
      "review.submit",
      "review.edit",
      "review.retry",
      "-",
      "review.copy",
      "-",
      "review.discard",
    ],
  },
  { title: "Window", role: "window", items: [p("Minimize"), p("Maximize"), "-", p("BringAllToFront")] },
  { title: "Help", role: "help", items: ["help.shortcuts", "help.project"] },
];

export const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

/** The accelerator to use on this platform, if any. */
export function accelFor(id: CommandId): string | undefined {
  const d = COMMANDS[id];
  return d.macOnly && !isMac ? undefined : d.accel;
}

// ─── Handlers ────────────────────────────────────────────────────────────

type Registration = { run: () => void; enabled: boolean; checked: boolean };

const stacks = new Map<CommandId, Registration[]>();
const listeners = new Set<() => void>();

function changed() {
  for (const l of listeners) l();
}

/** Called whenever what's enabled or checked may have changed. */
export function onCommandsChanged(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function top(id: CommandId): Registration | undefined {
  const s = stacks.get(id);
  return s?.[s.length - 1];
}

export function commandState(id: CommandId): { enabled: boolean; checked: boolean } {
  const r = top(id);
  return { enabled: r?.enabled ?? false, checked: r?.checked ?? false };
}

/** Runs a command if something can run it now. Returns whether it ran. */
export function runCommand(id: CommandId): boolean {
  const r = top(id);
  if (!r?.enabled) return false;
  r.run();
  return true;
}

/** Makes `run` the command's handler while the calling component is mounted. */
export function useCommand(
  id: CommandId,
  run: () => void,
  { enabled = true, checked = false }: { enabled?: boolean; checked?: boolean } = {},
) {
  const runRef = useRef(run);
  const reg = useRef<Registration | null>(null);
  useLayoutEffect(() => {
    runRef.current = run;
  });
  useEffect(() => {
    const r: Registration = { run: () => runRef.current(), enabled, checked };
    reg.current = r;
    const s = stacks.get(id) ?? [];
    s.push(r);
    stacks.set(id, s);
    changed();
    return () => {
      const s = stacks.get(id) ?? [];
      s.splice(s.indexOf(r), 1);
      reg.current = null;
      changed();
    };
    // Enabled and checked are updated in place below, so the handler keeps
    // its place in the stack.
  }, [id]);
  useEffect(() => {
    const r = reg.current;
    if (r && (r.enabled !== enabled || r.checked !== checked)) {
      r.enabled = enabled;
      r.checked = checked;
      changed();
    }
  }, [enabled, checked]);
}

// ─── Keys ────────────────────────────────────────────────────────────────

const CODES: Record<string, string> = {
  "[": "BracketLeft",
  "]": "BracketRight",
  ",": "Comma",
  "=": "Equal",
  "-": "Minus",
  "/": "Slash",
  Enter: "Enter",
  Up: "ArrowUp",
  Down: "ArrowDown",
};

function parse(accel: string) {
  const parts = accel.split("+");
  const key = parts[parts.length - 1];
  const mods = parts.slice(0, -1);
  const m = { meta: false, ctrl: false, alt: false, shift: false };
  for (const mod of mods) {
    if (mod === "CmdOrCtrl") {
      if (isMac) m.meta = true;
      else m.ctrl = true;
    } else if (mod === "Cmd") m.meta = true;
    else if (mod === "Ctrl") m.ctrl = true;
    else if (mod === "Alt") m.alt = true;
    else if (mod === "Shift") m.shift = true;
  }
  const code = /^[A-Z]$/.test(key) ? `Key${key}` : /^[0-9]$/.test(key) ? `Digit${key}` : CODES[key];
  return { ...m, key, code };
}

/** Whether a key press is this accelerator (by physical key, so Option
 * combinations work whatever character they type). */
export function matchesAccel(accel: string, e: Pick<KeyboardEvent, "code" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">) {
  const a = parse(accel);
  return (
    e.code === a.code && e.metaKey === a.meta && e.ctrlKey === a.ctrl && e.altKey === a.alt && e.shiftKey === a.shift
  );
}

const SYMBOLS: Record<string, string> = { Up: "↑", Down: "↓", Enter: "↩" };

/** How the shortcut is written on this platform: ⌥⌘R on a Mac. */
export function formatAccel(accel: string): string {
  const a = parse(accel);
  const key = SYMBOLS[a.key] ?? a.key;
  if (isMac) return `${a.ctrl ? "⌃" : ""}${a.alt ? "⌥" : ""}${a.shift ? "⇧" : ""}${a.meta ? "⌘" : ""}${key}`;
  return [a.ctrl && "Ctrl", a.alt && "Alt", a.shift && "Shift", a.meta && "Win", key].filter(Boolean).join("+");
}

/** "Sync Pull Request (⌘R)", for tooltips. */
export function withShortcut(text: string, id: CommandId): string {
  const accel = accelFor(id);
  return accel ? `${text} (${formatAccel(accel)})` : text;
}

/** The browser build has no native menu, so shortcuts are handled here. */
export function handleShortcut(e: KeyboardEvent): boolean {
  for (const id of Object.keys(COMMANDS) as CommandId[]) {
    const accel = accelFor(id);
    if (accel && matchesAccel(accel, e)) {
      if (runCommand(id)) {
        e.preventDefault();
        return true;
      }
      return false;
    }
  }
  return false;
}
