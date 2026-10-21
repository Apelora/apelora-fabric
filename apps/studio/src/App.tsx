import { useEffect } from "react";

import { EditorApp } from "./EditorApp";
import { useBackend } from "./features/applications/backend";
import { useRoute } from "./features/router";
import { AppShell } from "./components/applications/AppShell";
import { ApplicationsPage } from "./components/applications/ApplicationsPage";
import { ApplicationRoute } from "./components/applications/ApplicationRoute";

/** `/` is the Assembly Library; `/studio/applications…` is Application Management. */
export function App() {
  const route = useRoute();
  // Back from the identity provider: finish signing in whichever page the provider returned to.
  useEffect(() => { if (new URL(window.location.href).searchParams.has("state")) void useBackend.getState().detect(); }, []);
  if (route.name === "library") return <EditorApp />;
  if (route.name === "application") return <ApplicationRoute key={route.app} app={route.app} tab={route.tab} />;
  return <AppShell route={route}><ApplicationsPage /></AppShell>;
}
