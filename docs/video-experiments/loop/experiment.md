# Final silent loop: selected A

Synthetic Russian developer demo, approved [brief #584](https://github.com/alexkirs/taskq/issues/584#issuecomment-6085614964) and [selection #589](https://github.com/alexkirs/taskq/issues/589). Reuses the accepted A renderer, fonts, palette, task captions and separate worker/supervisor labels. No private recording or personal screen content. No product, Memory, README, header or website changes.

28 seconds, 1080×1080, 30 fps. MP4 H.264 and WebM VP9, neither with an audio stream. Desktop/mobile local video playback is the supported review target; authenticated draft download is required. No destination requiring GIF has been confirmed, so no GIF is generated. Inline GitHub playback is not promised. Final owner acceptance is pending; the draft must remain unpublished.

Timeline: 0–4 many chats/manual coordination; 4–7 request to one manager and GitHub/GitLab; 7–10 tasks; 10–14 independent parallel work with both roles; 14–16 owner question; 16–18 explicit owner answer; 18–20.5 supervisor review; 20.5–23 page checked, login still working; 23–26 visible status/history and next request; 26–27.5 smooth fade through the background to opening; 27.5–28 opening held. Last and first source frames are identical. This is illustrative editing, not an execution-speed claim. It does not promise automatic app wake, arbitrary local boards or zero setup.

Reproduce from the repository on macOS with existing Pillow and FFmpeg:

```sh
/usr/local/bin/python3 docs/video-experiments/loop/render.py --check
/usr/local/bin/python3 docs/video-experiments/loop/render.py --out /tmp/taskq-591-media
ffprobe -v error -show_entries format=duration:stream=codec_type,codec_name,width,height,nb_frames,r_frame_rate -of json /tmp/taskq-591-media/loop.mp4
ffprobe -v error -show_entries format=duration:stream=codec_type,codec_name,width,height,r_frame_rate -of json /tmp/taskq-591-media/loop.webm
ffmpeg -v error -i /tmp/taskq-591-media/loop.mp4 -f null -
ffmpeg -v error -i /tmp/taskq-591-media/loop.webm -f null -
```

Qualified tools: Python with Pillow 12.0.0, FFmpeg 8.1.1, existing macOS Arial fonts. CPU-only local raster/encode; no browser, Blender, alternate runtime, dependency install or paid provider call. Worker/host monetary cost is unknown. Source remains macOS-specific, like the selected pilot. Media is delivered outside git.

The assert-based check covers stage boundaries, out-of-range times, every frame's caption bounds, and exact last/first source-frame equality. Visual review must additionally inspect the decoded 360px sequence, question/answer/review order, fade and two consecutive loops; frame equality alone does not prove subjective visual acceptance.

## Delivery and evidence

[Draft assets](https://github.com/alexkirs/taskq/releases/tag/untagged-0133389bf1e3abb8becd): [MP4](https://github.com/alexkirs/taskq/releases/download/untagged-0133389bf1e3abb8becd/loop.mp4), [WebM](https://github.com/alexkirs/taskq/releases/download/untagged-0133389bf1e3abb8becd/loop.webm), [poster](https://github.com/alexkirs/taskq/releases/download/untagged-0133389bf1e3abb8becd/poster.png). Download with repository access:

```sh
gh release download taskq-preview-591 --repo alexkirs/taskq --dir /tmp/taskq-591-download
```

Upload/download SHA-256 round-trip matched all seven assets. MP4: `c5616c3fd33fe37952917c66dd16893b8f2f82c4ccbce97b7ccec973d8bc9ff7`; WebM: `74b7cc438fd5c763c94bd522837095e2881f0c78cfabdf5c5e3fc40c1ccf17ba`. Both downloaded videos fully decode. ffprobe: each 28.000 seconds, 1080×1080, 30 fps, one video stream only; MP4 has 840 frames.

Inspected the source screenlist at 360px, both decoded loop sequences sampled at 1 fps, and the final 2-second fade at 8 fps. Confirmed visible roles, owner question then explicit answer, review before checked, login still working, next request and stable boundary. Initial overlapping-text dissolve was corrected to fade through background; regenerated and inspected the final output. Transition deliberately fades captions briefly; logo and Demo remain visible. Sampling does not prove realtime playback on the owner's device or subjective preference; those remain owner review gates.

To reproduce temporal review sheets:

```sh
ffmpeg -y -v error -stream_loop 1 -i /tmp/taskq-591-media/loop.mp4 -vf 'fps=1,scale=360:360,tile=7x4' -frames:v 2 /tmp/taskq-591-media/two-loops-%02d.png
ffmpeg -y -v error -ss 26 -i /tmp/taskq-591-media/loop.mp4 -vf 'fps=8,scale=360:360,tile=4x4' -frames:v 1 /tmp/taskq-591-media/seam.png
```

| Requirement | Check and failure detected | Prior failure | Blindspot / disposition | Cost evidence |
|---|---|---|---|---|
| Silent 28s, readable ordered story and seamless boundary | Renderer assertions; ffprobe; complete decode; 360px screenlist; two-loop and fade samples; download hashes | Overlapping transition text observed and corrected | Retain lightweight check; owner realtime/device acceptance pending | No provider calls; host/worker cost unknown |

`python3 -m unittest tests.test_single` ran 146 tests in 21.488s: FAILED (2 failures, 11 errors, 1 skipped). All reported failures/errors are existing HermesNativeBoundary Linux-pidfd requirements on macOS (`os.pidfd_open` absent). This is not a green repository gate; the supervisor must evaluate CI and the platform limitation before merging. No queue/runtime code was changed or alternate runtime launched to repair it. Final executable source was checked after rebase; subsequent documentation-only evidence additions do not change the rendered media.
