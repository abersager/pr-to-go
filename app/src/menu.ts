// The native menu bar (desktop app only), built from the command list in
// commands.ts. Items run the same handlers as the keyboard shortcuts, and are
// greyed out or checked as the commands' state changes.

import { CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu } from "@tauri-apps/api/menu";
import {
  COMMANDS,
  type CommandId,
  MENUS,
  type MenuEntry,
  accelFor,
  commandState,
  onCommandsChanged,
  runCommand,
} from "./commands";

type Item = MenuItem | CheckMenuItem;
type Shown = { enabled: boolean; checked: boolean; text: string };

const APP = "PR to Go";

let installed = false;

export async function installMenu(): Promise<void> {
  if (installed) return;
  installed = true;
  const items = new Map<CommandId, { item: Item; shown: Shown }>();

  const textFor = (id: CommandId, checked: boolean) => {
    const d = COMMANDS[id];
    return checked && d.labelOn ? d.labelOn : d.label;
  };

  const build = async (entries: MenuEntry[]): Promise<(Item | Submenu | PredefinedMenuItem)[]> => {
    const out: (Item | Submenu | PredefinedMenuItem)[] = [];
    for (const e of entries) {
      if (e === "-") out.push(await PredefinedMenuItem.new({ item: "Separator" }));
      else if (typeof e === "object" && "predefined" in e) {
        // Named explicitly: an unbundled build would say "pr-to-go-app".
        const named: Partial<Record<string, string>> = {
          About: `About ${APP}`,
          Hide: `Hide ${APP}`,
          Quit: `Quit ${APP}`,
        };
        out.push(
          await PredefinedMenuItem.new({
            item: e.predefined === "About" ? { About: { name: APP } } : e.predefined,
            text: named[e.predefined],
          }),
        );
      } else if (typeof e === "object") {
        out.push(await Submenu.new({ text: e.submenu, items: await build(e.items) }));
      } else {
        const def = COMMANDS[e];
        const s = commandState(e);
        const opts = {
          id: e,
          text: textFor(e, s.checked),
          enabled: s.enabled,
          accelerator: accelFor(e),
          action: () => {
            // A check item flips its own checkmark when clicked. Note that,
            // so the next sync puts it back if the command didn't change.
            const entry = items.get(e);
            if (entry && def.check) entry.shown.checked = !entry.shown.checked;
            runCommand(e);
            scheduleSync();
          },
        };
        const item = def.check ? await CheckMenuItem.new({ ...opts, checked: s.checked }) : await MenuItem.new(opts);
        items.set(e, { item, shown: { enabled: s.enabled, checked: s.checked, text: opts.text } });
        out.push(item);
      }
    }
    return out;
  };

  const submenus: Submenu[] = [];
  for (const m of MENUS) {
    const sub = await Submenu.new({ text: m.title, items: await build(m.items) });
    submenus.push(sub);
  }
  const menu = await Menu.new({ items: submenus });
  await menu.setAsAppMenu();
  await Promise.all(
    MENUS.map((m, i) =>
      m.role === "window"
        ? submenus[i].setAsWindowsMenuForNSApp()
        : m.role === "help"
          ? submenus[i].setAsHelpMenuForNSApp()
          : null,
    ),
  );

  // Push state changes to the native items, a batch at a time.
  let pending = false;
  function sync() {
    for (const [id, { item, shown }] of items) {
      const s = commandState(id);
      if (s.enabled !== shown.enabled) {
        shown.enabled = s.enabled;
        void item.setEnabled(s.enabled);
      }
      const text = textFor(id, s.checked);
      if (text !== shown.text) {
        shown.text = text;
        void item.setText(text);
      }
      if (item instanceof CheckMenuItem && s.checked !== shown.checked) {
        shown.checked = s.checked;
        void item.setChecked(s.checked);
      }
    }
  }
  function scheduleSync() {
    if (pending) return;
    pending = true;
    setTimeout(() => {
      pending = false;
      sync();
    }, 50);
  }
  onCommandsChanged(scheduleSync);
  sync();
}
