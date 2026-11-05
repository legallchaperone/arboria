// Where the window is.
export type SettingsTab = "general" | "machines" | "project" | "advanced";
export type Route =
  | { k: "inbox" }
  | { k: "project"; root: string }
  | { k: "idea"; root: string; idea: string }
  | { k: "seeds" }
  | { k: "settings"; tab: SettingsTab; root?: string };

/** What the inspector on the right shows. */
export type Inspect =
  | { k: "run"; root: string; run: string }
  | { k: "todo"; root: string; task: string }
  | { k: "command"; title: string; text: string; command: string };
