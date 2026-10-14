# README visual evidence

Open [compare.html](compare.html) locally from a checkout to compare the rendered README at 960px and 390px content widths. It contains the complete [before](before.html) and [after](after.html) pages, with scrollable frames initially positioned at the images. The baseline is `1c93286fe38869180d4d9b55a10d80fff1cd4418`; its original PNGs are preserved beside this file. GitHub serves HTML as source, so download/check out the files to view them.

These are durable GitHub Markdown API renderings with the same local CSS, not screenshots of GitHub's website. They were opened and visually inspected in Chrome on macOS at both widths. The final README source SHA-256 is `1b3730d89a7ffa5575c9dfefb137b57927bc2e21a2387244cc97a334f26e24a3`. The header, each displayed native PNG, all three conversations and footer were inspected at their displayed sizes. The setup code blocks scroll horizontally at narrow width, preserving the exact copyable phrase.

## Task fit

- A1: Product and the page plan changed before README edits under R13; the contract remains 300 lines.
- A2–A3: three connected, explicitly illustrative storefront conversations cover UC-02/03/07, UC-09 and UC-14. They distinguish filing from starting, owner choices from agent guesses, and publication from deployment. Each links its source cases; no session or journey evidence is invented.
- A4: setup, release verification, license and donate remain. The slogan table is replaced; historical phrase evidence is explicitly limited to no-write interpretation.
- A5: [native macOS capture](https://github.com/taskqorg/taskq/actions/runs/38514195166) at `285e4b0461d91455fa70c7aac768ecfc335b6c70` completed. Downloaded image hashes match the manifest. Attention shows the complete question, numbered choices, input, context and Home/Settings controls. Projects shows both project summaries, unknown Closed totals and complete lower controls. Both are 320px wide and remain at native CSS width in the two inspected layouts.
- A6: before/after rendered pages and original images are retained here. The final header scales to the narrow page; both native screenshots remain readable without accidental clipping. Text paragraphs and dialogue wrap. This worker inspection is not owner visual acceptance.
- A7: the page, plan, fixture, image selection, manifest and phrase-evidence scope agree. Canonical check results and exact-SHA CI are reported on the PR/result, not inferred from screenshots.
- A8: the service reviewer still needs to assess task fit and implementation quality, open the final page/images, and present the result for owner acceptance before publication.

## Implementation quality

The capture script selects two already existing native snapshot outputs in `update.sh`; the fixture adds the synthetic cart question. No runtime, permission, dependency, test or golden trace changed. The UI source blob IDs match the capture manifest. Final PNGs are unedited native outputs; no generated or reconstructed interface was substituted.

The earlier [e177c91 capture](https://github.com/taskqorg/taskq/actions/runs/38513752916) was rejected after opening it: Project cut through its Board row; Home cut through a project summary. The existing sequence also captures Attention and Projects, so changing output selection removes the unwanted service-heavy framing without a runtime redesign. No numeric visual gate was added before owner acceptance.

The transparent dark Summary icon was nearly invisible on the light README background. It is retained in the capture manifest but removed from the page; Projects already presents queue counts. Windows capture and actual GitHub responsive CSS are not qualified by this local browser inspection. No deployment or completed storefront journey is claimed.
