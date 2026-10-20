import { useEffect, useState } from "react";

import { loadTheme, saveTheme } from "./editor/persistence";

export const THEMES = ["light", "winter", "night"];

/** The colour theme, kept across pages and visits. */
export function useTheme(): [string, (t: string) => void] {
  const [theme, setTheme] = useState(loadTheme);
  useEffect(() => { document.documentElement.setAttribute("data-theme", theme); saveTheme(theme); }, [theme]);
  return [theme, setTheme];
}
