# Contributing

Direct human-written code contributions are welcome. The project currently
develops software primarily through agent-driven workflows, but changes
written by people or agents follow the same accountable review,
evidence-gated provenance, licensing, testing, and repository-validation
requirements.

Work from an issue and preserve the dependency boundaries in `AGENTS.md` and
`policy/architecture.json`. New code, shaders, tests, and assets are MIT.
Independent implementation is the default. Reusing outside material requires an
MIT-compatible license and an exact record in [PROVENANCE.md](PROVENANCE.md);
GPL/AGPL source must not be copied, adapted, or depended on. Visual assets used
in fixtures remain separately licensed inputs and require their own exact
provenance.

Before opening a pull request, run:

```sh
tools/validate.sh
actionlint .github/workflows/*.yml
git diff --check
```

`tools/validate.sh` includes `cargo deny`, which fetches the advisory database,
so a new upstream advisory can fail validation without any change here. See
[cargo-deny in validation](docs/CRATE-RELEASE.md#cargo-deny-in-validation) for
the recovery path.

Public scene, resource, rendering, semantic-output, fixture, and CLI behavior is
versioned API. Pull-request titles and commits use Conventional Commits.
