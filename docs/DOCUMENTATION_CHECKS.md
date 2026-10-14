# Documentation checks

Documentation-only changes must preserve runtime source, manifests, lockfiles,
workflows, protocol/data formats, fixtures and tests.

## Markdown

Run a pinned standalone checker without adding it to package manifests:

```bash
npx --yes --package markdownlint-cli@0.45.0 markdownlint "*.md" "docs/**/*.md" ".github/**/*.md" --ignore "docs/archive/**" --disable MD013 MD033
```

The initial tool invocation may download its npm package into the local tool
cache; it is a development check, not a runtime dependency. Line length and inline
HTML are allowed. Legacy archives retain historical formatting and are excluded
from style normalization; they remain included in local link/asset checks.

## Links, examples and rendering

Check every tracked Markdown inline/reference link and local image, including
archive paths, exact case on case-sensitive hosts, and heading fragments.
Check external README/docs links and badge/image endpoints; distinguish a
permission-gated reporting page or rate limit from a confirmed missing page.
Do not substitute a successful homepage response for a broken linked resource.

Validate CLI examples and prerequisites against the current CLI help and source.
Keep source-only commands distinct from published packages. Do not run examples
that install services, apply changes or execute untrusted project commands just
to validate formatting. Check license/version statements against LICENSE,
manifests and published release metadata.

Render both README tables/diagrams and code blocks using GitHub-flavored Markdown
at desktop and narrow widths. Inspect existing screenshots and captions; a
welcome-screen image is not proof of a successful analysis or platform smoke.
Use a fresh reader to check installation, maturity, evidence and security claims.

## Existing CI

Run the checks in [Development](DEVELOPMENT.md) and the original PR workflows,
including CodeForge Regression Gate. Do not change a baseline, fixture, test or
core logic to make a documentation-only check pass. Record unresolved failures
and release blockers in [Release preparation](RELEASE_PREPARATION.md).
