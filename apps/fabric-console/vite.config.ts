import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// The console talks only to the Apelora control plane (never to Incus directly).
const api = process.env.APELORA_API_URL ?? "http://127.0.0.1:8080";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: { dedupe: ["react", "react-dom"] },
  server: { host: "127.0.0.1", port: 5174, strictPort: true, proxy: { "/api": api } },
  preview: { host: "127.0.0.1", port: 4174, strictPort: true, proxy: { "/api": api } },
});
