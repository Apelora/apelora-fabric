import { ensureNics, parseAssembly, usedCustomClasses, type ApplicationAssembly } from "@apelora/assembly-model";
import { lookupClass } from "@apelora/module-catalog";

/** Longest address fragment we hand out; larger diagrams should be shared as a file. */
export const MAX_LINK_CHARS = 12_000;
const MAX_INFLATED = 2_000_000;
export const SHARE_PREFIX = "#s=";

const toB64Url = (bytes: Uint8Array): string => {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
};
const fromB64Url = (text: string): Uint8Array => {
  const s = atob(text.replace(/-/g, "+").replace(/_/g, "/"));
  return Uint8Array.from(s, (c) => c.charCodeAt(0));
};

async function pipe(bytes: Uint8Array, stream: CompressionStream | DecompressionStream, limit = Infinity): Promise<Uint8Array> {
  const writer = stream.writable.getWriter();
  void writer.write(bytes as BufferSource).then(() => writer.close()).catch(() => undefined);
  const reader = stream.readable.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.length;
    if (total > limit) { await reader.cancel(); throw new Error("The link expands to more than 2 MB."); }
    chunks.push(value);
  }
  const out = new Uint8Array(total);
  let o = 0;
  for (const c of chunks) { out.set(c, o); o += c.length; }
  return out;
}

/** Packs an assembly (with the custom classes it needs) into a compressed address fragment. */
export async function encodeShare(a: ApplicationAssembly): Promise<{ ok: true; fragment: string } | { ok: false; error: string }> {
  const used = usedCustomClasses(a, lookupClass);
  const doc = { ...a, ...(used.length ? { catalog: { classes: used } } : {}) };
  const bytes = await pipe(new TextEncoder().encode(JSON.stringify(doc)), new CompressionStream("deflate-raw"));
  const fragment = SHARE_PREFIX + toB64Url(bytes);
  if (fragment.length > MAX_LINK_CHARS) return { ok: false, error: `This diagram needs a ${Math.round(fragment.length / 1000)} kB link, more than the ${MAX_LINK_CHARS / 1000} kB limit. Export it as a file to share it.` };
  return { ok: true, fragment };
}

/** Reads a fragment produced by encodeShare. The content is untrusted and goes through the normal import validation. */
export async function decodeShare(fragment: string): Promise<{ ok: true; assembly: ApplicationAssembly; customClasses: NonNullable<ApplicationAssembly["catalog"]>["classes"] } | { ok: false; errors: string[] }> {
  if (!fragment.startsWith(SHARE_PREFIX)) return { ok: false, errors: ["Not a share link."] };
  if (fragment.length > MAX_LINK_CHARS * 2) return { ok: false, errors: ["The link is too long."] };
  try {
    const bytes = await pipe(fromB64Url(fragment.slice(SHARE_PREFIX.length)), new DecompressionStream("deflate-raw"), MAX_INFLATED);
    const r = parseAssembly(new TextDecoder().decode(bytes));
    if (!r.ok) return r;
    return { ok: true, assembly: ensureNics(r.assembly, lookupClass), customClasses: r.assembly.catalog?.classes ?? [] };
  } catch (e) {
    return { ok: false, errors: [`The link could not be read: ${(e as Error).message}`] };
  }
}
