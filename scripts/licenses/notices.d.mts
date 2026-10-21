// SPDX-License-Identifier: AGPL-3.0-only
// Types for notices.mjs, used by the apps' Vite configurations.
import type { Plugin } from "vite";

export function licenseNotices(): Plugin;
export function buildNotices(options?: { strict?: boolean }): {
  data: Record<string, unknown>;
  files: Record<string, { type: string; body: string }>;
};
