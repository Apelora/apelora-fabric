import { healthLine, type AppView } from "../../features/applications/model";
import { onLinkClick, paths } from "../../features/router";
import { AppIcon, DesignBadge, RuntimeBadge, timeAgo } from "./common";
import { ApplicationActions } from "./ApplicationActions";
import { MoreMenu } from "./MoreMenu";

export function ApplicationCard({ app }: { app: AppView }) {
  const d = app.deployment;
  return (
    <article className="flex flex-col gap-3 rounded-box border border-base-300 bg-base-100 p-3" data-testid="app-card" data-app={app.name}>
      <div className="flex items-start gap-2.5">
        <AppIcon icon={app.icon} size={36} />
        <div className="min-w-0 flex-1">
          <h3 className="truncate text-sm font-semibold" data-testid="app-name"><a href={paths.application(app.name)} onClick={onLinkClick} className="hover:underline">{app.name}</a></h3>
          <p className="line-clamp-2 min-h-8 text-xs text-base-content/70" data-testid="app-description">{app.description || <span className="text-base-content/40">No description</span>}</p>
        </div>
        <RuntimeBadge app={app} />
      </div>
      <dl className="grid grid-cols-2 gap-x-3 gap-y-1.5 text-xs">
        <div><dt className="text-base-content/70">Project</dt><dd className="truncate" data-testid="app-project">{app.project.name}{app.environment ? ` · ${app.environment.name}` : ""}</dd></div>
        <div><dt className="text-base-content/70">Design</dt><dd><DesignBadge app={app} /></dd></div>
        <div><dt className="text-base-content/70">Modules</dt><dd data-testid="app-modules">{d.modules || app.design.draftVersion ? `${d.modules}` : "0"}</dd></div>
        <div><dt className="text-base-content/70">Health</dt><dd data-testid="app-health">{healthLine(app)}</dd></div>
        <div className="col-span-2"><dt className="text-base-content/70">Last activity</dt><dd title={d.lastActivityAt ?? ""} data-testid="app-activity">{timeAgo(d.lastActivityAt ?? app.updatedAt)}</dd></div>
      </dl>
      <div className="mt-auto flex flex-wrap items-center gap-2 border-t border-base-300 pt-2.5">
        <a href={paths.application(app.name)} onClick={onLinkClick} className="btn btn-xs btn-primary" data-testid="app-open">Open</a>
        <ApplicationActions app={app} />
        <div className="ml-auto"><MoreMenu app={app} /></div>
      </div>
    </article>
  );
}
