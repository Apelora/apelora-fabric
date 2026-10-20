---
name: mutation-testing
description: Run and act on mutation testing (cargo-mutants for Rust, Stryker for TypeScript) to find rules that no test protects. Use after changing authorisation, lifecycle, validation or network-compiler code, when asked "are these tests any good", or before declaring a security-relevant change tested.
---

# Mutation testing

A mutation tool changes the code in a small way (flips `>` to `>=`, replaces a function body, deletes a `!`) and runs the tests. A change the tests do not notice is a **missed mutant**: a rule nothing protects. Coverage says a line ran; mutation testing says a test would fail if the line were wrong. Full guide: `docs/development/MUTATION-TESTING.md`.

## Commands

```sh
cargo mutants -p apelora-auth --no-shuffle -j 4          # ~1 min; must report 0 missed
cargo mutants -p apelora-domain --no-shuffle -j 4 -f crates/domain/src/access.rs -f crates/domain/src/lifecycle.rs
DATABASE_URL=… cargo mutants -p apelora-control-plane --no-shuffle -j 2 --timeout 150 -f services/control-plane/src/api/sessions.rs   # needs PostgreSQL; slow
npm run mutation                                          # Stryker, TypeScript model rules; mutates files in place
cargo mutants --list -p apelora-auth                      # what would be tried, without running
```

Scope lives in `.cargo/mutants.toml` and `stryker.config.mjs`.

## Reading the result

- **caught**: a test failed. Good. **timeout** counts as caught (an infinite loop is noticed).
- **unviable**: the mutant did not compile. Ignore.
- **MISSED**: read the line. Decide which it is:
  1. *A real gap* (most are): write the smallest test that fails for that mutant. Boundaries (`>` vs `>=`), both sides of an `&&`/`||`, a constant's value, the text of an error people read. Put them next to the rule's tests, or in `crates/auth/src/hardening.rs` for the auth crate.
  2. *Dead or redundant code*: delete it (a branch whose two arms return the same value).
  3. *Equivalent mutant* (the program behaves identically): exclude that one function in `.cargo/mutants.toml` with a comment saying why. Never exclude to make a number look better.

## Rules

- Never weaken or delete a test to kill a mutant, never `#[mutants::skip]` a security rule.
- A new authorisation, approval, lifecycle or validation rule is not done until `cargo mutants` on its crate reports no missed mutants.
- Stryker runs **in place** (workspace packages are symlinked, so a copied sandbox would test the originals). It rewrites source files and restores them at the end: start from a clean `git status`, do not edit those files or run two runs at once, and if a run is killed run `git checkout -- packages apps` after checking `git status`.
- Run mutation testing alone; it is CPU-heavy and a timeout can be a slow machine rather than a finding.
