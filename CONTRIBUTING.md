# Contributing to Roshera

Thanks for your interest in Roshera — the agent-native geometry kernel.

## License of contributions

Roshera is licensed under the [GNU Affero General Public License v3.0](LICENSE)
(AGPL-3.0-only). By contributing, you agree that your contributions are
licensed under the same terms, and you grant the project maintainer the right
to also license your contributions under other terms, including commercial
licenses. This keeps Roshera's dual licensing (AGPL for everyone, a commercial
license for those who need one) possible without tracking down every past
contributor.

## Developer Certificate of Origin (DCO)

Every commit must be signed off, certifying the
[Developer Certificate of Origin v1.1](https://developercertificate.org/):

```
git commit -s -m "your message"
```

This adds a `Signed-off-by: Your Name <you@example.com>` trailer, asserting
that you wrote the change (or otherwise have the right to submit it) under the
project's license.

## Ground rules

- **Production-grade only** — no TODOs, stubs, or placeholder implementations.
- **The kernel must not lie** — any change touching geometry must keep the
  validity certificate honest; new defect classes need new invariants, and
  tests must fail before the fix (genuine red) or prove their teeth by
  mutation.
- Run `cargo fmt --all` before committing (the pre-commit hook enforces it).
- One build at a time; run hang-prone reproductions under a timeout.

## Questions

29.varuns@gmail.com
