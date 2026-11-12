# Consumer audit — 2026-07-31

## Result

The shared authentication contracts are implemented and the central gold
standard guard passes across all six tracked Rust applications. The audit also
found and corrected two maintenance defects: the guard still referenced the
pre-migration checkout paths, and SceneTrove's CI followed the moving
`auth-modules` `main` branch instead of an immutable commit.

| Consumer | Integration | Verification | Reproducibility |
|---|---|---|---|
| Blackstart | Direct shared crate | Repo guard and full test suite pass | CI pins `7deebffc155b934467b743dc35763f75ceeec57b` |
| Klaxond | Direct shared crate | Repo guard and full test suite pass | CI pins `7deebffc155b934467b743dc35763f75ceeec57b` |
| CoveyOS | Direct shared crate | Repo guard and locked all-feature check pass | Pin file uses `99458340`; compatible, intentionally behind current head |
| SceneTrove | CI checkout plus path dependency | Repo guard and locked all-feature check pass | Corrected to immutable `7deebffc155b934467b743dc35763f75ceeec57b` |
| Quadro | Direct shared crate | Repo guard and locked all-feature check against current head pass | CI pins `27be036`; compatible, deliberately lagging |
| StudioFlow | Modified vendored fork | Repo guard and 88 vendored crate tests pass | No exact upstream provenance marker; migration needs a dedicated auth review |

`auth-modules` itself passes 90 tests with all features. Every repository-local
wrapper also passes the central negative, security, presence and runtime-method
contract checks.

## Boundaries and follow-up

- A green guard proves the declared shared contracts and detects the known
  forbidden local implementations. It is not a substitute for an application
  threat-model or end-to-end identity-provider test.
- StudioFlow must not be overwritten with upstream blindly. Its vendored crate
  contains application-specific changes and lacks an immutable provenance
  marker. Reconcile its delta module by module, then record the upstream commit
  and local patch policy.
- CoveyOS and Quadro are compatible with the current local crate but remain on
  older immutable revisions. Upgrade them through their normal CI and runtime
  gates when they need the newer shared secret helpers.
- Consumer CI must use immutable commit SHAs. A branch such as `main` is not an
  acceptable supply-chain pin.

## Guard maintenance

The central script resolves repositories below `/home/ansible/repos` and uses
`/home/ansible/repos/studioflow/wt-develop` as the canonical StudioFlow
worktree, falling back to `/opt/studioflow`. Nested `.git` metadata is excluded
so compatibility symlinks and worktree internals do not create false failures.
