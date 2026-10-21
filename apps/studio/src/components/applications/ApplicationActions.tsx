import type { AppView } from "../../features/applications/model";
import { DeployAction } from "./DeployAction";
import { LifecycleActions } from "./LifecycleActions";

/** Everything one can do with an application from a list or its page: deploy it and run it. */
export function ApplicationActions({ app, size = "xs" }: { app: AppView; size?: "xs" | "sm" }) {
  return (
    <div className="flex flex-wrap items-center gap-2" data-testid="application-actions">
      <DeployAction app={app} size={size} />
      <LifecycleActions app={app} size={size} />
    </div>
  );
}
