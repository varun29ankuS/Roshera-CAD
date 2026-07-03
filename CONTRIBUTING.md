# Contributing to Roshera

Thanks for your interest in Roshera — the agent-native geometry kernel.

## License of contributions

Roshera is licensed under the [Functional Source License 1.1 with Apache 2.0
future grant](LICENSE) (FSL-1.1-Apache-2.0). By contributing, you agree that
your contributions are licensed under the same terms, and that the project
maintainer may relicense the project (including your contributions) under any
OSI-approved open-source license in the future — this keeps the already-promised
FSL→Apache-2.0 conversion (and any future move to a more permissive license)
possible without tracking down every past contributor.

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
