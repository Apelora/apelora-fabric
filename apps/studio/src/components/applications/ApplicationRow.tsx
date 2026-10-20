import { healthLine, type AppView } from "../../features/applications/model";
import { onLinkClick, paths } from "../../features/router";
import { AppIcon, DesignBadge, RuntimeBadge, timeAgo } from "./common";
import { LifecycleActions } from "./LifecycleActions";
import { MoreMenu } from "./MoreMenu";

export function ApplicationRow({ app }: { app: AppView }) {
  const d = app.deployment;
  return (
    <tr className="hover:bg-base-200/40" data-testid="app-row" data-app={app.name}>
      <td>
        <div className="flex items-center gap-2.5">
          <AppIcon icon={app.icon} size={30} />
          <div className="min-w-0">
            <a href={paths.application(app.name)} onClick={onLinkClick} className="block truncate text-sm font-semibold hover:underline" data-testid="app-name">{app.name}</a>
            <span className="text-xs text-base-content/60" data-testid="app-appliances">{d.appliances} appliances</span>
          </div>
        </div>
      </td>
      <td><RuntimeBadge app={app} /></td>
      <td><DesignBadge app={app} /></td>
      <td className="text-xs" data-testid="app-project">{app.project.name}{app.environment ? ` · ${app.environment.name}` : ""}</td>
      <td className="text-xs" data-testid="app-health">{healthLine(app)}</td>
      <td className="whitespace-nowrap text-xs text-base-content/70" title={d.lastActivityAt ?? ""} data-testid="app-activity">{timeAgo(d.lastActivityAt ?? app.updatedAt)}</td>
      <td>
        <div className="flex items-center justify-end gap-2">
          <a href={paths.application(app.name)} onClick={onLinkClick} className="btn btn-xs btn-primary" data-testid="app-open">Open</a>
          <LifecycleActions app={app} />
          <MoreMenu name={app.name} />
        </div>
      </td>
    </tr>
  );
}
