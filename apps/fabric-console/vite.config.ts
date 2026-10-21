import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

import { licenseNotices } from "../../scripts/licenses/notices.mjs";

// The console talks only to the Apelora control plane (never to Incus directly).
const api = process.env.APELORA_API_URL ?? "http://127.0.0.1:8080";

export default defineConfig({
  // Licence notices at /licenses (scripts/licenses/notices.mjs), served from this origin.
  plugins: [react(), tailwindcss(), licenseNotices()],
  resolve: { dedupe: ["react", "react-dom"] },
  // `ws`: terminal sessions are websockets. The proxy keeps the browser's Host, so the control
  // plane's Origin check sees this page's own origin (see APELORA_ALLOWED_ORIGINS otherwise).
  server: { host: "127.0.0.1", port: 5174, strictPort: true, proxy: { "/api": { target: api, ws: true } } },
  preview: { host: "127.0.0.1", port: 4174, strictPort: true, proxy: { "/api": { target: api, ws: true } } },
});
