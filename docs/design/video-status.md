# Promo video status

Review for [#795](https://github.com/alexkirs/taskq/issues/795), 2026-10-13.
Read all task bodies and comment histories for #584–#591. All eight issues are
closed; closure of the phase-0 tail is not acceptance of unfinished video work.
No new generation, rendering, provider login or production publication occurred.

## Inventory

| Task | Outcome and evidence | Artifacts and reusable parts | Missing |
|---|---|---|---|
| [#584](https://github.com/alexkirs/taskq/issues/584) — common brief | [Approved brief](https://github.com/alexkirs/taskq/issues/584#issuecomment-6085614964): Russian developers; 14-second comparison, 58-second explainer, 28-second loop. | Storyboard, narration draft, captions, synthetic demo, mobile typography and delivery criteria. | Original worker access/pricing is historical; subsequent planning approvals limit what may proceed. |
| [#585](https://github.com/alexkirs/taskq/issues/585) — A, precise 2D | [Accepted pilot](https://github.com/alexkirs/taskq/issues/585#issuecomment-6085738377); [merged PR #593](https://github.com/alexkirs/taskq/pull/593). | [Renderer](../video-experiments/2d/render.py), [experiment](../video-experiments/2d/experiment.md), [draft A assets](https://github.com/alexkirs/taskq/releases/tag/untagged-9df4398faaa4cfb1b3cd): preview, poster, screenlist. Reuse exact text overlays, cards, Q/A and review timing. | Only a 14-second pilot; renderer uses macOS Arial paths. New host qualification and final owner playback remain. |
| [#586](https://github.com/alexkirs/taskq/issues/586) — B, ImageGen | [Accepted pilot](https://github.com/alexkirs/taskq/issues/586#issuecomment-6085885594); one generation call, billing unknown. | [Draft B assets](https://github.com/alexkirs/taskq/releases/tag/untagged-9ab42c7decd845ad0ab4): preview, illustration, prompt, poster, screenlist. Illustration can be reused without regeneration. | No tracked renderer; illustration details are small at mobile size. Screenlist crops captions and cannot prove their completeness. Not the selected route. |
| [#587](https://github.com/alexkirs/taskq/issues/587) — C, Google notebook | No generated video or delivered package. [Owner chose manual upload](https://github.com/alexkirs/taskq/issues/587#issuecomment-6093338470); subsequent preflight said package preparation had not started. Closed as phase-0 tail. | Public brief and manual-upload plan only; no Google artifact to compare. | Portable synthetic package, actual export, timing/quality evidence. Historical-writer/provider-rights hold remains unresolved. |
| [#588](https://github.com/alexkirs/taskq/issues/588) — D, Meshy | No model or 3D preview delivered. Later comments report device login completed and authorize free preparation only; this review did not recheck account state. Closed as phase-0 tail. | Metaphor/route proposal and access evidence, not an asset. | Reusable-model inventory, qualified CPU route, preview; any paid stage requires exact approval. C/D never lost a visual comparison: they were not produced. |
| [#589](https://github.com/alexkirs/taskq/issues/589) — comparison | [Accepted comparison](https://github.com/alexkirs/taskq/issues/589#issuecomment-6088863883); [owner selection](https://github.com/alexkirs/taskq/issues/589#issuecomment-6088796117): A and a credible calm older Russian male voice. | A/B comparison, download/hash evidence and route choice. Later planning records refine the narrative; preserve these rather than restart route selection. | Historical route choice does not accept exact voice, final script, storyboard or finished videos. |
| [#590](https://github.com/alexkirs/taskq/issues/590) — narrated final | No final MP4 or subtitles delivered. Free isolated Silero sample preparation was authorized; later production pause and planning-only orders superseded final assembly. Closed as phase-0 tail after failed execution. | In [A draft](https://github.com/alexkirs/taskq/releases/tag/untagged-9df4398faaa4cfb1b3cd): `voice-ru_dmitriy.wav`, `voice-ru_eduard.wav`, `voice-evidence.json`, `voice-source-590.zip`. Reuse samples and source/license provenance after verification. | Owner sample selection, scene/timing/style approval, complete narration, subtitles, assembly, current product accuracy and final acceptance. No later animatic/scene-card artifact was found in the issue or main. |
| [#591](https://github.com/alexkirs/taskq/issues/591) — silent final | [Worker candidate](https://github.com/alexkirs/taskq/issues/591#issuecomment-6088962357), never finally accepted; later planning-only orders and hold. Closed as phase-0 tail. | [Draft loop assets](https://github.com/alexkirs/taskq/releases/tag/untagged-0133389bf1e3abb8becd): MP4/WebM, poster, screenlist, seam, two-loop sheets. [Closed, unmerged PR #599](https://github.com/alexkirs/taskq/pull/599) retains renderer/notes at `212972e504b9a15b20664905778e74cc6d5dd67d`. Reuse timing/seam/source after review. | Candidate uses the older login/page story, not the later existing-project/site narrative. Source is absent from main. Final owner/device acceptance and current checks remain. No later style-preview artifact was found. |

## Approval boundary

The initial #584 storyboard remains approved historical material. Do not silently
replace it, or mistake it for approval of later finals. The owner subsequently
approved a narrative outline and then a bounded planning basis:
[planning approval](https://github.com/alexkirs/taskq/issues/590#issuecomment-6093878629),
[scene/timing order](https://github.com/alexkirs/taskq/issues/590#issuecomment-6093886451),
[style-preview order](https://github.com/alexkirs/taskq/issues/591#issuecomment-6093892459).
That basis uses an **existing project**, clean 2D on a warm background, readable
cards and calm Russian narration. It includes PM plan/choices, workers, a human
question, board/history and a finished-site reveal. It permits planning previews,
not paid generation or final production/publication. Exact final approvals remain
missing. Private source was not accessed or copied into this public review.

The later [historical-writer hold](https://github.com/alexkirs/taskq/issues/591#issuecomment-6114184491)
requires independently proven fencing of old writers and Git/provider rights
before repair, replacement or publication. Closing old issues does not prove
fencing. #795 authorizes review and a continuation decision, not bypassing this
hold. Any continuation must carry it forward as an explicit prerequisite.

Current [contract](../../taskq.md) uses separate worker/reviewer roles and owner
acceptance through the PM. Historical “supervisor” captions and narration require
a product-accuracy review before reuse; do not depict automatic acceptance,
zero setup, unsupported integrations or planned onboarding as shipped behavior.
Keep README/header/site and production untouched. Drafts must remain unpublished.

## Fresh artifact review

Downloaded all assets from the three draft releases with repository access.
Reviewed A/B/loop contact sheets and A source/notes; inspected voice evidence.
A clearly shows owner question, explicit answer and review before checked status.
B adds an illustration without adding workflow information; its contact sheet
crops the question/answer region. Loop sheets show the older story and fade back
to the opening; these samples do not prove subjective motion or device playback.

Download hashes match recorded evidence for A/B previews, loop MP4/WebM and both
voice samples. Voice metadata records 13.375-second Dmitriy and 11.9-second Eduard
samples; no listening/voice preference judgment was performed in this review.
No fresh codec/full-decode test: FFmpeg/ffprobe are unavailable on this host.
Original decode/readability checks are historical evidence, not fresh PASS.
Tool availability, current provider charges and full final-production cost are
unknown. No dependency installation was needed for this review.

## Continuation decision

Owner pick: **pending #795 ask**. Do not file or start production tasks before it.

1. **Recommended: continue A in approval stages.** Preserve the latest approved
   planning basis; first reconcile existing artifacts/current product and obtain
   scene, pacing and style approval. Reuse local 2D source and voice samples;
   optional C/D are not dependencies. Final assembly follows explicit gates.
2. **Loop first.** Review/revise the existing 28-second A candidate against current
   behavior and ask for acceptance; defer the narrated explainer. This reduces the
   immediate deliverable but does not complete both videos.
3. **Reopen visual exploration.** Specify a bounded Google manual-upload or Meshy
   trial before final production. This adds access/cost decisions; A/B and the
   approved brief remain reusable, not discarded.

After the pick, record its answer link here and file only the selected follow-ups.
Each is one reviewable deliverable: approval-ready scene/timing package; if needed,
approved style/voice proof; narrated MP4 with SRT/poster and QA; silent MP4/WebM
with seam/mobile QA. Production tasks depend on recorded approvals and verified
historical-writer fencing. New task links replace this provisional list; do not
reopen old tasks, launch providers or accept old candidates by implication.
