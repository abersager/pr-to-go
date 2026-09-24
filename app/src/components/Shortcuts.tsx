import { useEffect } from "react";
import { COMMANDS, type CommandId, MENUS, type MenuEntry, accelFor, formatAccel, isMac } from "../commands";

const mod = isMac ? "⌘" : "Ctrl+";

/** Keys that work in one place rather than from the menu. */
const LOCAL: { where: string; keys: [string, string][] }[] = [
  {
    where: "While writing a comment",
    keys: [
      [`${mod}↩`, "Done"],
      ["Esc", "Done"],
    ],
  },
  {
    where: "In the find bar",
    keys: [
      ["↩", "Next match"],
      [isMac ? "⇧↩" : "Shift+Enter", "Previous match"],
      ["Esc", "Close"],
    ],
  },
  { where: "In the diff", keys: [[isMac ? "⇧-click" : "Shift+click", "Select a range of lines to comment on"]] },
];

/** The menu's commands, with the submenu they're in ("Verdict: Approve"). */
function commandsIn(entries: MenuEntry[], prefix = ""): [CommandId, string][] {
  return entries.flatMap((e): [CommandId, string][] =>
    typeof e === "string"
      ? e === "-"
        ? []
        : [[e, prefix + COMMANDS[e].label]]
      : "submenu" in e && e.submenu !== "Find"
        ? commandsIn(e.items, `${e.submenu}: `)
        : "submenu" in e
          ? commandsIn(e.items)
          : [],
  );
}

/** Every keyboard shortcut, grouped as in the menu bar. */
export function Shortcuts({ onClose }: { onClose: () => void }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  const groups = MENUS.map((m) => ({
    title: m.title,
    rows: commandsIn(m.items)
      .map(([id, label]) => [accelFor(id), label] as const)
      .filter((r): r is readonly [string, string] => !!r[0]),
  })).filter((g) => g.rows.length > 0);
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal shortcuts" role="dialog" aria-label="Keyboard shortcuts" onClick={(e) => e.stopPropagation()}>
        <div className="rebase-banner">
          <h2>Keyboard shortcuts</h2>
          <span className="spacer" />
          <button className="link" onClick={onClose}>
            Close
          </button>
        </div>
        <div className="shortcut-groups">
          {groups.map((g) => (
            <section key={g.title}>
              <h3 className="small">{g.title}</h3>
              <dl>
                {g.rows.map(([accel, label]) => (
                  <div key={label}>
                    <dt>
                      <kbd>{formatAccel(accel)}</kbd>
                    </dt>
                    <dd>{label}</dd>
                  </div>
                ))}
              </dl>
            </section>
          ))}
          {LOCAL.map((g) => (
            <section key={g.where}>
              <h3 className="small">{g.where}</h3>
              <dl>
                {g.keys.map(([k, label]) => (
                  <div key={label + k}>
                    <dt>
                      <kbd>{k}</kbd>
                    </dt>
                    <dd>{label}</dd>
                  </div>
                ))}
              </dl>
            </section>
          ))}
        </div>
      </div>
    </div>
  );
}
