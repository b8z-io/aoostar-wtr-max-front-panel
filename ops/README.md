# ops/ — fork planning docs

Fork-specific planning and specs for `b8z-io/aoostar-wtr-max-front-panel`.

Deliberately kept outside `docs/`, which upstream builds into an mdBook — keeping notes here
avoids merge conflicts when pulling from `zehnm/aoostar-rs`.

| Doc | What |
|---|---|
| [`ARCHITECTURE.md`](ARCHITECTURE.md) | Deployment topology, decision rationale, work split, questions for Hermes |
| [`SPEC-staleness.md`](SPEC-staleness.md) | The honest-degradation layer. First code change to the fork |

Sections marked 🔶 are open questions needing knowledge of the live homelab.

**Review protocol:** answer inline in the doc under the question, or open a branch. Keep the
argument in version control rather than in chat, so the reasoning survives.
