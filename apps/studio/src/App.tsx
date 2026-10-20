import { EditorApp } from "./EditorApp";
import { useRoute } from "./features/router";
import { AppShell } from "./components/applications/AppShell";
import { ApplicationsPage } from "./components/applications/ApplicationsPage";
import { ApplicationRoute } from "./components/applications/ApplicationRoute";

/** `/` is the Assembly Library; `/studio/applications…` is Application Management. */
export function App() {
  const route = useRoute();
  if (route.name === "library") return <EditorApp />;
  if (route.name === "application") return <ApplicationRoute key={route.app} app={route.app} tab={route.tab} />;
  return <AppShell route={route}><ApplicationsPage /></AppShell>;
}
