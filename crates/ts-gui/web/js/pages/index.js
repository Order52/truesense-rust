// The pages of the window, loaded on demand (one module per page).

// parent: where the Back button goes (Home unless said otherwise).
export const PAGES = [
  { id: "home", title: "TrueSense", icon: "home", load: () => import("./home.js") },
  { id: "lighting", title: "Lighting", icon: "keyboard", load: () => import("./lighting.js") },
  { id: "curves", title: "Fan curves", icon: "fan", load: () => import("./curves.js") },
  { id: "smart", title: "Smart auto profiles", icon: "smart", load: () => import("./smart.js") },
  { id: "predator", title: "PREDATOR key", icon: "key", load: () => import("./predator.js") },
  { id: "undervolt", title: "Undervolt", icon: "performance", load: () => import("./undervolt.js") },
  { id: "battery", title: "Battery and Screen", icon: "battery", load: () => import("./battery.js") },
  { id: "audio", title: "Audio", icon: "audio", load: () => import("./audio.js") },
  { id: "profiles", title: "Profiles", icon: "profiles", parent: "settings", load: () => import("./profiles.js") },
  { id: "health", title: "Health check", icon: "health", parent: "settings", load: () => import("./health.js") },
  { id: "settings", title: "Settings", icon: "settings", load: () => import("./settings.js") },
];

/** `truesense gui --page` names that are not page ids. */
const ALIASES = { keys: ["lighting", "keys"] };

/** "#/lighting/keys" -> {page, sub}; anything unknown is home. */
export function resolveRoute(hash) {
  const parts = String(hash || "").replace(/^#\/?/, "").split("/").filter(Boolean);
  let [id, sub = ""] = parts;
  if (ALIASES[id]) [id, sub] = ALIASES[id];
  const page = PAGES.find((p) => p.id === id) || PAGES[0];
  return { page, sub };
}
