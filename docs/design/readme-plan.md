# README page plan

Help a visitor set up TaskQ and understand how a request becomes reviewed work.
[Product](../../taskq.md#product) owns these decisions (R13). Follow specq Core and Documentation; retain shipped-only claims and the 300-line contract limit.

## Page contract

1. Keep `docs/header.webp` and `Agents working.` at the top.
2. Keep the exact existing-project setup phrase and new-project variant. Explain concrete setup approval.
3. State that the repository board holds tasks and the host service runs workers; keep a short shipped-capability list.
4. Show dark native Attention and Projects snapshots individually, with captions and links to their originals. Do not shrink two tall panels into one row. Keep task content and lower controls visible; distinguish unknown values from zero.
5. Use three connected, explicitly illustrative conversations: planning (UC-02/03/07), a worker question (UC-09), and evidence/release review (UC-14). Each has initial context, owner request, meaningful clarification, decision, action and observable outcome. Link the source cases. Never call a proposed result an observed session or imply the whole source UC is shipped.
6. Retain one contract link, release checksum and provenance instructions, MIT license and the final donate line.

## Scenario and evidence boundary

Use a storefront sign-in journey throughout: agree the redirect and cart tasks, resolve what happens to an existing cart, then inspect redirect evidence before accepting publication. Example issue numbers and dialogue are invented and labelled. The manager relays decisions; the worker implements and the reviewer checks the exact candidate. Setup, admission and publication retain their own approval boundaries.

The earlier phrase checks in [readme-phrase-evidence.md](readme-phrase-evidence.md) are historical no-write interpretation checks. They neither verify these conversations nor prove a completed journey. In particular, do not resurrect `Review #12.` as an automatic acceptance command. Proposed UI and unavailable client capabilities are not advertised as shipped.

## Images and acceptance

Use `docs/shots/fixture.sh` and `docs/shots/update.sh` on macOS. The fixture is synthetic, with coherent task/question content; it does not access a real board or run a service. Keep unknown host observations explicit. The separate edge-case fixture remains unchanged.

The existing capture pipeline builds with `--locked`, runs `taskq tray --snapshot` in an isolated install, and records version, source commit, binary hash, environment, image hashes and UI source blob IDs in `docs/shots/manifest.json`. UI sources must be committed before capture. `docs/shots/check.sh` compares those blob IDs with HEAD; this survives squash merging. Retain the existing CI capture/staleness workflow. Windows capture remains unqualified. Keep the captured transparent summary icon in the manifest, but omit it from the page: its light glyphs disappear on a light README background; Projects already shows queue counts.

Open the rendered README at desktop and narrow widths. Inspect every image at its displayed size, including the header. Preserve before/after renders and inspection findings in repository evidence linked from the PR. A hash, successful capture or CI result is not visual acceptance. The reviewer checks task fit and implementation quality separately against A1–A8 and opens the final page and images. The owner accepts the visual result before merge/publication.

Checks: `docs/shots/check.sh`, `cargo test --locked`, `cargo run -- selftest`, and CI on the exact candidate SHA. Report missing evidence as unknown. Preserve closed #923/#925 history; #1007 supersedes their rejected page presentation.
