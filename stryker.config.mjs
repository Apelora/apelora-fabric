// Mutation testing for the TypeScript rules that decide what is valid, connected or allowed
// (Stryker, Apache-2.0). Run `npm run mutation`; see docs/development/MUTATION-TESTING.md.
/** @type {import('@stryker-mutator/api/core').PartialStrykerOptions} */
export default {
  // The Vitest runner plugin does not yet work with Vitest 5 (mutants are never switched on),
  // so each mutant is tried by running the unit tests; the first failing test kills it.
  testRunner: "command",
  commandRunner: { command: "npx vitest run packages apps --bail=1 --reporter=dot" },
  mutate: [
    "packages/assembly-model/src/network.ts",
    "packages/assembly-model/src/validate.ts",
    "packages/assembly-model/src/runtime.ts",
    "packages/assembly-model/src/cables.ts",
    "packages/assembly-model/src/ops.ts",
    "packages/assembly-model/src/compat.ts",
    "packages/assembly-model/src/geometry.ts",
    "packages/assembly-model/src/tags.ts",
    "packages/api-client/src/index.ts",
    "apps/studio/src/log.ts",
    "apps/studio/src/features/applications/model.ts",
    "apps/fabric-console/src/log.ts",
    "apps/fabric-console/src/lib/api.ts",
  ],
  coverageAnalysis: "off",
  reporters: ["clear-text", "progress"],
  thresholds: { high: 85, low: 70, break: null },
  timeoutMS: 60000,
  concurrency: 3,
  // The workspace packages are symlinked into node_modules, so a copied sandbox would still
  // import the originals. Mutate in place instead (Stryker restores every file afterwards);
  // start from a clean `git status` and never run two mutation runs at once.
  inPlace: true,
};
