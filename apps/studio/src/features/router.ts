import { useEffect, useState } from "react";

/** Routes of the Studio. `/` stays the Assembly Library (the single local canvas). */
export type Route =
  | { name: "library" }
  | { name: "applications" }
  | { name: "application"; app: string; tab: AppTab };

export const APP_TABS = ["overview", "designer", "deployments", "monitoring", "versions", "settings"] as const;
export type AppTab = (typeof APP_TABS)[number];

const BASE = "/studio/applications";

export function parseRoute(pathname: string): Route {
  const p = pathname.replace(/\/+$/, "") || "/";
  if (p === BASE) return { name: "applications" };
  if (p.startsWith(`${BASE}/`)) {
    const [app, tab] = p.slice(BASE.length + 1).split("/");
    if (app && /^[a-z][a-z0-9-]{0,62}$/.test(app)) {
      return { name: "application", app, tab: (APP_TABS as readonly string[]).includes(tab ?? "") ? (tab as AppTab) : "overview" };
    }
    return { name: "applications" };
  }
  return { name: "library" };
}

export const paths = {
  library: () => "/",
  applications: () => BASE,
  application: (app: string, tab: AppTab = "overview") => (tab === "overview" ? `${BASE}/${app}` : `${BASE}/${app}/${tab}`),
};

/** Moves to a path without reloading the page. */
export function navigate(path: string, { replace = false }: { replace?: boolean } = {}): void {
  if (path === window.location.pathname) return;
  if (replace) window.history.replaceState(null, "", path);
  else window.history.pushState(null, "", path);
  window.dispatchEvent(new PopStateEvent("popstate"));
}

export function useRoute(): Route {
  const [route, setRoute] = useState<Route>(() => parseRoute(window.location.pathname));
  useEffect(() => {
    const on = () => setRoute(parseRoute(window.location.pathname));
    window.addEventListener("popstate", on);
    return () => window.removeEventListener("popstate", on);
  }, []);
  return route;
}

/** A link that navigates in the page and still works with middle-click and "open in new tab". */
export function onLinkClick(e: React.MouseEvent<HTMLAnchorElement>): void {
  if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
  const a = e.currentTarget;
  if (a.target && a.target !== "_self") return;
  e.preventDefault();
  navigate(a.pathname + a.search);
}
