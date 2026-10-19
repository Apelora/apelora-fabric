/** A stable muted hue per tag name, so every terminal of one net looks related. */
export function tagColor(name: string): string {
  let h = 0;
  for (const ch of name) h = (h * 31 + ch.charCodeAt(0)) % 360;
  return `oklch(62% 0.12 ${h})`;
}

/** The name flag tied to a plugged terminal: terminals with the same name are linked without a drawn cable. */
export function TagStub({ side, tag, hot, onSelect, onHover }: { side: "input" | "output"; tag: { name: string; members: number }; hot: boolean; onSelect: () => void; onHover: (v: boolean) => void }) {
  return (
    <button
      type="button"
      className="apl-tag nodrag nopan"
      data-testid="tag-stub"
      data-side={side}
      data-tag={tag.name}
      data-hot={hot}
      style={{ ["--apl-tag" as string]: tagColor(tag.name) }}
      title={`Link tag “${tag.name}” — ${tag.members} terminal${tag.members === 1 ? "" : "s"}. Click to select the tag.`}
      aria-label={`Link tag ${tag.name}, ${tag.members} terminals`}
      onClick={(e) => { e.stopPropagation(); onSelect(); }}
      onPointerDown={(e) => e.stopPropagation()}
      onMouseEnter={() => onHover(true)}
      onMouseLeave={() => onHover(false)}
    >{tag.name}{tag.members > 2 && <span className="apl-tag-n">{tag.members}</span>}</button>
  );
}
