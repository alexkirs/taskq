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
