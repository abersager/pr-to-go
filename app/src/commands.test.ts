import { describe, expect, it } from "vitest";
import { COMMANDS, type CommandId, MENUS, type MenuEntry, isMac, matchesAccel } from "./commands";

const press = (code: string, mods: Partial<Record<"metaKey" | "ctrlKey" | "altKey" | "shiftKey", boolean>> = {}) => ({
  code,
  metaKey: false,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  ...mods,
});
const cmd = isMac ? { metaKey: true } : { ctrlKey: true };

describe("matchesAccel", () => {
  it("matches the physical key and exactly the modifiers", () => {
    expect(matchesAccel("CmdOrCtrl+D", press("KeyD", cmd))).toBe(true);
    expect(matchesAccel("CmdOrCtrl+D", press("KeyD", { ...cmd, shiftKey: true }))).toBe(false);
    expect(matchesAccel("CmdOrCtrl+Shift+D", press("KeyD", { ...cmd, shiftKey: true }))).toBe(true);
    expect(matchesAccel("CmdOrCtrl+Alt+]", press("BracketRight", { ...cmd, altKey: true }))).toBe(true);
    expect(matchesAccel("CmdOrCtrl+Alt+Down", press("ArrowDown", { ...cmd, altKey: true }))).toBe(true);
    expect(matchesAccel("CmdOrCtrl+-", press("Minus", cmd))).toBe(true);
    expect(matchesAccel("CmdOrCtrl+Shift+Enter", press("Enter", { ...cmd, shiftKey: true }))).toBe(true);
  });
});

describe("the command list", () => {
  it("gives no two commands the same shortcut", () => {
    const seen = new Map<string, string>();
    for (const [id, d] of Object.entries(COMMANDS)) {
      if (!d.accel) continue;
      expect(seen.get(d.accel), `${id} and ${seen.get(d.accel)} share ${d.accel}`).toBeUndefined();
      seen.set(d.accel, id);
    }
  });

  it("puts every command in the menu bar", () => {
    const inMenus = new Set<CommandId>();
    const walk = (items: MenuEntry[]) =>
      items.forEach((e) => (typeof e === "string" ? e !== "-" && inMenus.add(e) : "submenu" in e && walk(e.items)));
    MENUS.forEach((m) => walk(m.items));
    expect([...Object.keys(COMMANDS)].filter((id) => !inMenus.has(id as CommandId))).toEqual([]);
  });
});
