# README visual evidence (#1189, #1191)

Open [compare.html](compare.html) locally from a checkout to compare the rendered README at 960px and 390px content widths. It frames the complete [before](before.html) page (baseline `c09baae`) and the [after](after.html) page at the two new sections. GitHub serves HTML as source, so download/check out the files to view them.

These are GitHub Markdown API renderings with the [#1007](../1007/README.md) local CSS, not screenshots of GitHub's website. Each new section was captured with Playwright in Chrome on macOS at both widths and inspected at displayed size; neither width scrolls horizontally (scrollWidth equals viewport). The README source SHA-256 is `0b568feeec837902ad89c6c1e4b2f8da684b148567fa74d0b340cbc6f3264174`.

## Findings

- From any agent chat: the intro says any agent session with taskq available, not only Claude or Codex, runs the same commands from chat. Dialogue wraps at both widths. The `taskq status` table, the #1048 ask lines and the profile table are verbatim output in code blocks; long lines scroll inside the block, which keeps the real output exact and copyable.
- Let the other agent do it: the owner picture scales to the narrow page and stays legible; the #976 board log lines scroll inside the block. Text wraps.
- Header, Attention and Projects images, setup, release verification, license and donate lines are unchanged from the baseline.

This worker inspection is not owner visual acceptance; the owner accepts the result before publication. Windows and GitHub's own responsive CSS are not qualified here.
