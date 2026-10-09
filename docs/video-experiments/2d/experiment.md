# Experiment A: precise 2D animation

Approved scene: [brief #584](https://github.com/alexkirs/taskq/issues/584#issuecomment-6085614964). Synthetic Russian developer demo, 14 seconds, 1080×1080, 30 fps, silent H.264 MP4. This is a timed explanatory diagram, not a screenshot or a promise of task completion speed.

Render on macOS with an existing Python containing Pillow and FFmpeg on PATH:

```sh
python3 docs/video-experiments/2d/render.py --check
python3 docs/video-experiments/2d/render.py --out /tmp/taskq-585-media
ffprobe -v error -show_entries format=duration:stream=codec_name,width,height,nb_frames,r_frame_rate -of json /tmp/taskq-585-media/preview.mp4
ffmpeg -v error -i /tmp/taskq-585-media/preview.mp4 -f null -
```

The qualified worker used `/usr/local/bin/python3` (Pillow 12.0.0); its worktree default Python did not contain Pillow. Fonts use existing macOS Arial regular/bold. No dependency installation, browser, GPU or provider generation is needed. Source is intentionally macOS-specific. Media stays outside git.

Timeline: request 0-3; parallel tasks with both roles 3-6; owner question 6-9; explicit owner answer 9-11; supervisor review 11-12.5; checked page with login still working 12.5-14. Main text is at least 54 master pixels / 18 display pixels at 360 width. Labels carry status as well as color. One card eases into place; other state transitions use clean cuts to preserve legibility.

Validation: boundary assertions include exact Q/A/review order and out-of-range rejection. All 420 frames exercise text-width assertions. Render and full decode succeeded; ffprobe measured 14.000 seconds, 420 frames, H.264, 1080×1080, 30 fps, no audio. Inspected source screenlist at 360 px per frame and decoded sequence at 2 fps: request, both roles, question, answer, review then checked remain visible without clipping. Sampling does not prove subjective motion quality or owner preference. This pilot is not the final seamless loop.

Delivery: draft release `taskq-preview-585` contains only synthetic preview assets. Download with repository access:

```sh
gh release download taskq-preview-585 --repo alexkirs/taskq --dir /tmp/taskq-585-download
```

Do not publish this draft. Its authenticated release/asset URLs are recorded in the task result; inline browser playback is not promised. The supervisor reviews the exact source head and delivery evidence before acceptance.

Recommendation: use this CPU route for exact Russian typography, predictable state timing and cheap revisions. Remotion, ImageGen and a 3D runtime add no value for this scene. No paid provider calls; worker/host monetary cost unknown. Preparation/render/check elapsed times and delivery hash are recorded in the result. Product behavior, README, website and Memory did not change.
