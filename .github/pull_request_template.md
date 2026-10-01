## What changed

<!-- One paragraph. If it fixes an issue, link it. -->

## How it was verified

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] `cargo test`
- [ ] `npm run lint && npm run typecheck && npm run build` (desktop changes)

## Checklist

- [ ] No new `unwrap()`/`expect()`/`todo!()` in production code
- [ ] New user-facing strings are in **both** `en-US.json` and `zh-CN.json`
- [ ] New errors carry a stable code and a user-facing hint
- [ ] No search logic was added to a front end — it belongs in `search-daemon`
- [ ] Nothing on the search path reaches the network