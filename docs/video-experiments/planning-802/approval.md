# A planning approval package · #802

**Planning previews only. No scene, timing, style or exact voice is accepted yet.**
The [contract](../../../taskq.md) remains unchanged. This package proposes staging,
not product decisions or final-production activation. See the retained
[status/approvals](../../design/video-status.md) and
[owner-policy fencing evidence](../../design/fencing-video.md).
The [tool answer](https://github.com/alexkirs/taskq/issues/802#issuecomment-6135228277)
allows free Gyan.FFmpeg installation and qualification on Windows only.

## Scene cards

[scenes.json](scenes.json) contains the exact Russian narration, visible text and
end times. Concatenating its narration with spaces preserves the
[approved text basis](https://github.com/alexkirs/taskq/issues/590#issuecomment-6093886451)
verbatim. All preview screens are synthetic diagrams, not actual application
captures. The cards below specify proposed screen actions; the rough animatic
holds their key states and uses cuts, not final interface animation.

| Scene / time | Narration cue | Proposed screen action | Reading hold and transition | Accuracy / proof gap |
|---|---|---|---|---|
| 1 · 0–5 s | Привычный Codex или Claude… | Two supported runtime names/select, many chat tabs reduce to one PM conversation. | 5 s key-state hold; cut to project. | Names stand in for icons; no new logos or unsupported runtime promise. PM is an agent conversation. |
| 2 · 5–10 s | Подключи TaskQ к проекту… | Existing project folder, setup request, then owner work request. | 5 s; cut to plan. | Setup requires inspection and confirmed external effects (§6). No instant/new-project onboarding claim. No real path shown. |
| 3 · 10–16 s | Менеджер предложит план… | Plan for login/page; owner chooses; PM files separate tasks. | 6 s; cut to worker cards. | PM proposes and files; it does not do task work or decide for owner. |
| 4 · 16–20 s | Агенты работают параллельно. | Login remains working while page moves to separate reviewer. | 4 s; cut to capacity. | Replace historical supervisor with worker/reviewer. Different progress does not promise a completion speed. |
| 5 · 20–25 s | Ты задаёшь число исполнителей | Host/project limit 1→2, explicit confirmation, existing task continues. | 5 s; cut to question. | Owner controls capacity; no actual limit/config change. No global shared pool claim. |
| 6 · 25–30 s | и отвечаешь, когда нужен твой выбор. | Email question, two options, explicit owner answer. Final animation would reveal the answer after question hold. | Proposed final sub-holds: 3 s question + 2 s answer; rough card holds both for 5 s. Cut to board. | Answer is synthetic, never a real taskq command. Owner alone chooses. |
| 7 · 30–35 s | На доске видны задачи и этапы. | Board link then status movement; separate milestone navigation indicated as a target. | 5 s; cut to history. | GitHub/GitLab supported. Configured live board layout and milestone navigation unqualified; no shipped custom UI claim. |
| 8 · 35–41 s | В каждой задаче — история… | Open one issue: request, question, answer, result/media link, checks and owner acceptance. | 6 s; cut to result. | Issues/comments carry history; images are linked/uploaded media, not automatic taskq image generation. No private issue/screenshot copied. |
| 9 · 41–45 s | Всё можно открыть и проверить. | Open result/checks/source links. | 4 s; cut to demo site. | Links are the visual proposal. No live result, browser or deployed-site verification claimed. |
| 10 · 45–53 s | Работа завершена: смотри готовый сайт… | Synthetic project-page reveal after reviewer checks and owner acceptance; offer next step. | 8 s; cut to ending. | Rough preview uses a labelled site placeholder. Final asset must show a safe demo site; taskq does not automatically build/deploy/accept sites. |
| 11 · 53–58 s | Один менеджер. TaskQ. | TaskQ wordmark and good-job ending. | 5 s clean hold; stop, no loop seam. | Proposed ending only; 28 s silent loop remains separate #805 work. |

Duration proposal: **58 seconds**, leaving 2 seconds below the ceiling. The basis
has 69 whitespace-delimited words: about 71 words/minute over the whole film,
not a measured spoken duration. Scene 1 is the busiest at about 108 words/minute;
scene 10 gets 8 seconds. The selected natural voice and actual pauses may require
redistribution. Do not speed up voice or silently shorten text to fit.
No final synthesis occurred. The animatic is intentionally silent: the approved
narration is visible beneath each key-state card, not mixed with unrelated samples.

## Warm 2D and mobile test

Retains the [A renderer](../2d/render.py)'s Pillow/FFmpeg CPU card route,
rounded cards, exact raster typography, safe margins, explicit states and clean
cuts. Historical sources and assets are untouched. New palette: warm `#F6EFE5`,
white cards, ink `#172338`, accent `#164CC5`; no new AI illustration or dependency.
Minimum type is 54 master pixels / 18 pixels at 360 width; titles are 64 / 21.3.
Each master is 1080×1080, with at least 60 pixels of side margin. No color-only
status or tiny simulated screenshot text. Arial is an installed host font;
font files are not redistributed. Font portability beyond Windows/macOS is unknown.

Every frame says `ПЛАН · НЕ ФИНАЛ`. Delivery contains individual 1080 and 360 PNGs
for all 11 scenes and a 360-per-cell contact sheet. Inspect at actual 360 width,
not a reduced whole-sheet view. All 580 frames pass text bounds; visual sheet
inspection finds no clipped Cyrillic or overlaps. This is a desktop pixel-scale
readability test, not human phone playback or subjective style approval.
Timing rail moves at 10 fps; this proves rough holds/cuts, not final motion quality.

## Retained voice audition and provenance

Both samples are in the existing **unpublished**
[A draft](https://github.com/alexkirs/taskq/releases/tag/untagged-9df4398faaa4cfb1b3cd).
Authenticated API download links:

- [Dmitriy · 13.375 s](https://api.github.com/repos/alexkirs/taskq/releases/assets/626182342)
- [Eduard · 11.900 s](https://api.github.com/repos/alexkirs/taskq/releases/assets/626182352)
- [Source / complete license notice](https://api.github.com/repos/alexkirs/taskq/releases/assets/626182346)
- [Original voice evidence](https://api.github.com/repos/alexkirs/taskq/releases/assets/626182349)

Fresh downloads match both hashes in original evidence. Both are mono 48 kHz
signed 16-bit PCM WAV and fully decode with FFmpeg 9.0.2. They use the older
audition text, not this 69-word script. No resynthesis, model execution, speed
change, pitch shift, cloning or named-person imitation was performed.
Realism, calmness and perceived age remain owner listening decisions; no exact
voice recommendation can be established from decode/metadata alone.

The retained source pins official Silero revision
`d9355348e2781dc8fa25a135d1602c530afae24c`, CIS Base nostress model URL and expected
weight hash `0405777e332906f0644e08a680f7cfdc2137ea864090079c1fdd30a43c1b8761`.
Fresh upstream reads verify that URL in pinned `models.yml` and byte-for-byte
match of the ZIP's `LICENSE_CIS` with the pinned upstream notice: MIT,
copyright 2025-present Silero Team. Notice hash:
`a27fc071e03323fecda8a6ba416109bf36811801fa0c95e4371cf9105848718d`.
This verifies retained source/license provenance, not regeneration or the actual
historical model runtime. The model was not downloaded/rehashed in this attempt.
Ordinary Russian v5/v5_5 and CIS Ext license terms are not substituted.

| Retained asset | Fresh SHA-256 |
|---|---|
| voice-ru_dmitriy.wav | `ede269a22eb72bc76de36dfd824449bee0716e806fe30a48348a8bc373e280b4` |
| voice-ru_eduard.wav | `a38d5c4faef37c6b0897857b34e35b3357a3e628fa4b6159a03a1eb1f02ac30f` |
| voice-source-590.zip | `3a69dacf5fe60de9f03a7e3958a0b19296c6a933fe8b97c579faba9437053133` |
| voice-evidence.json | `d47e9180f7946f5e0bde47bba938824c2cb7eb48a74b840cdfd49b102911b667` |

## Reproduce and approval gate

Windows qualified: existing Python 3.12 / Pillow 12.3.0, Cyrillic Arial regular
and bold, winget Gyan.FFmpeg 9.0.2 (installer hash verified by winget).
FFmpeg's H.264 encode/full-decode smoke passed before preview rendering.
Add the installed FFmpeg aliases to the current shell's PATH if needed.

```sh
python docs/video-experiments/planning-802/render.py --check
python docs/video-experiments/planning-802/render.py --out .taskq/802-media
ffprobe -v error -show_entries format=duration:stream=codec_name,width,height,nb_frames,r_frame_rate -of json .taskq/802-media/planning-preview-802.mp4
ffmpeg -v error -i .taskq/802-media/planning-preview-802.mp4 -f null -
```

Download details, measured media properties and hashes are in
[delivery.md](delivery.md). Source ZIP contains only this package's public
renderer, scene data and documents; no private source, fonts, weights, recordings,
credentials or environment. Draft release must stay unpublished.

Owner gate through #802 taskq ask: approve/revise scene staging, 58 s holds and
warm style; listen to both retained samples and choose Dmitriy/Eduard or request
a different audition. Approval records must be linked here when received;
none is inferred from historical closures or the tool-install answer.
This gate does **not** authorize #803/#805 production, paid generation, publishing,
README/site changes, access changes or resuming old tasks. Phone playback, live
board capture, final site asset and final voice timing remain explicitly unproved.
