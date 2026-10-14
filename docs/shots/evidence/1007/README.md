# README visual evidence

Candidate inspected: `e177c91e978b49ab4e3be0a0fe246c998e876f13`.
[Native macOS capture](https://github.com/taskqorg/taskq/actions/runs/38513752916) completed successfully. The downloaded image hashes match its manifest. Opened Home and Project PNGs individually at native size.

## Task fit

A5 fails: Project cuts through the Board row at its bottom edge; Home cuts through the next project summary. Successful capture and matching hashes do not establish acceptable framing. The cart question and its numbered choices are present and readable in Project, but its lower controls are missing.

A6 is incomplete: final desktop/narrow page inspection and owner visual acceptance are pending. The `before.html` baseline uses GitHub's Markdown API and a local stylesheet, with preserved images from `1c93286`; it is not a screenshot of GitHub's site. Do not treat this baseline alone as before/after acceptance.

## Implementation quality

The existing native snapshot sequence already captures Attention and Projects. Select those two outputs in `update.sh`; no runtime or permission change is needed. Attention isolates the question; Projects isolates the queue summaries. Their actual framing must still be opened and checked after capture.

Next: capture the updated branch, download and verify its manifest, replace the old page images, then inspect the complete rendered page at desktop and narrow widths. Preserve unknown observations. No publication to main before owner acceptance.
