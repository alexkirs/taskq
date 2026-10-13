# #802 draft delivery evidence

Planning previews only; all approvals pending. Added new filenames to the existing
[unpublished A draft](https://github.com/alexkirs/taskq/releases/tag/untagged-9df4398faaa4cfb1b3cd)
without replacing historical assets. Fresh release readback confirms `isDraft=true`.
No public release, final synthesis, final assembly or production activation.

| Download (requires repository access) | Bytes | SHA-256 |
|---|---:|---|
| [Silent animatic](https://api.github.com/repos/alexkirs/taskq/releases/assets/635400959) | 683183 | `3176ee37ba3db6c4840e4f4c934c97b4508308e38b07c5931e061762952ee15a` |
| [Mobile sheet](https://api.github.com/repos/alexkirs/taskq/releases/assets/635400973) | 344919 | `57357051d60b29fac317ee67454d382d774dc1d00f570f6ac62da3cf701a5eab` |
| [Source ZIP](https://api.github.com/repos/alexkirs/taskq/releases/assets/635400958) | 8469 | `00d6dfb41a7f372ffced7f1d0f08955df63d32e52fbb4e3bff79a457abb26bd3` |
| [11 master/mobile style pairs](https://api.github.com/repos/alexkirs/taskq/releases/assets/635400951) | 1004404 | `57c95ce7d76dfecc1e37cdfdd3ccefacc3d18dae3a452653205f21bab1f2903d` |
| [SHA256SUMS](https://api.github.com/repos/alexkirs/taskq/releases/assets/635400952) | 367 | Four media/archive hashes above |

Downloaded all five assets into a separate directory after upload; all four
media/archive SHA-256 values match the uploaded manifest and local originals.
Source ZIP has exactly `render.py`, `scenes.json` and `approval.md` at their
repository-relative paths. It is reproducible when extracted into a checkout;
linked historical references and this delivery receipt remain in the repository.
Style ZIP has 22 PNGs: 1080×1080 and 360×360 for each scene; sheet is 1080×1440.
No voice samples, private source, weights, fonts, environment or credentials
were copied into either new archive. Retained voice links/hashes are in
[approval.md](approval.md#retained-voice-audition-and-provenance).

```sh
gh release download untagged-9df4398faaa4cfb1b3cd --repo alexkirs/taskq --pattern 'planning-*-802.*' --pattern 'planning-mobile-sheet.png' --dir .taskq/802-download
```

Windows media qualification: FFmpeg/ffprobe 9.0.2, libx264 CPU encoder;
fresh probe reports H.264, yuv420p, 1080×1080, 10 fps, 580 frames, exactly
58.000000 seconds and no audio stream. Both local and freshly downloaded MP4
fully decode without errors. Decoded 26 s mobile frame independently confirms
the question card. Source assertions check every frame's text bounds, timeline
boundaries, increasing holds of at least 4 seconds and invalid-time rejection.
Mobile sheet inspected at 360 pixels per cell; actual phone playback and owner
listening/creative preferences are unverified. No paid provider calls;
provider charge none, worker/host cost unknown.

Scene/narration data, renderer and approval notes are the candidate source;
the generated binaries stay out of git. Final motion, subtitle synchronization,
natural voice pacing, safe site reveal asset, real board capture and phone QA
belong to separately gated production work. Follow the #802 taskq approval card,
not historical closed tasks, before progressing.
