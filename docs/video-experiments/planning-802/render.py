"""Silent planning animatic; Pillow/FFmpeg card route retained from ../2d/render.py."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys

from PIL import Image, ImageDraw, ImageFont

SCENES = json.loads(Path(__file__).with_name('scenes.json').read_text(encoding='utf-8'))
FPS, SIZE = 10, 1080
BG, INK, ACCENT = '#F6EFE5', '#172338', '#164CC5'
FONTS = (Path(os.environ.get('WINDIR', 'C:/Windows')) / 'Fonts' if sys.platform == 'win32'
         else Path('/System/Library/Fonts/Supplemental'))
REGULAR = str(FONTS / ('arial.ttf' if sys.platform == 'win32' else 'Arial.ttf'))
BOLD = str(FONTS / ('arialbd.ttf' if sys.platform == 'win32' else 'Arial Bold.ttf'))


def scene_at(t):
    if not 0 <= t < SCENES[-1]['end']:
        raise ValueError('time outside preview')
    return next(i for i, scene in enumerate(SCENES) if t < scene['end'])


def frame(t):
    i = scene_at(t)
    scene = SCENES[i]
    im = Image.new('RGB', (SIZE, SIZE), BG)
    d = ImageDraw.Draw(im)

    def text(x, y, value, size=54, bold=False, color=INK):
        font = ImageFont.truetype(BOLD if bold else REGULAR, size)
        bounds = d.textbbox((x, y), value, font=font)
        assert bounds[0] >= 60 and bounds[2] <= 1020 and bounds[3] <= 1020, value
        d.text((x, y), value, font=font, fill=color)

    text(72, 40, 'ПЛАН · НЕ ФИНАЛ', color=ACCENT)
    text(72, 135, scene['title'], size=64, bold=True)
    d.rounded_rectangle((60, 260, 1020, 620), radius=26, fill='white')
    for row, value in enumerate(scene['screen']):
        text(84, 300 + row * 90, value)
    # Narration stays readable while a quiet time rail marks the scene hold.
    font = ImageFont.truetype(REGULAR, 54)
    lines = []
    line = ''
    for word in scene['narration'].split():
        candidate = (line + ' ' + word).strip()
        if d.textlength(candidate, font=font) > 936:
            assert line, 'word wider than narration area'
            lines.append(line)
            line = word
        else:
            line = candidate
    lines.append(line)
    assert len(lines) <= 4, scene['narration']
    for row, value in enumerate(lines):
        text(72, 680 + row * 65, value)
    text(72, 965, f'{i + 1:02d} / {len(SCENES)} · {t:04.1f} / 58 с', color=ACCENT)
    d.rectangle((72, 1040, 72 + round(936 * t / 58), 1052), fill=ACCENT)
    return im


def check():
    assert 0 < SCENES[-1]['end'] <= 60
    start = 0
    for i, scene in enumerate(SCENES):
        assert scene['end'] - start >= 4
        assert scene_at(start) == scene_at(scene['end'] - 0.01) == i
        start = scene['end']
    for t in (-1, 58):
        try:
            scene_at(t)
        except ValueError:
            pass
        else:
            raise AssertionError('invalid time accepted')
    for n in range(FPS * SCENES[-1]['end']):
        frame(n / FPS)
    print('11 scene boundaries; 58 seconds; all 580 frame text bounds: OK')


def render(out):
    check()
    out.mkdir(parents=True, exist_ok=True)
    cmd = ['ffmpeg', '-y', '-v', 'error', '-f', 'rawvideo', '-pix_fmt', 'rgb24',
           '-s', '1080x1080', '-r', str(FPS), '-i', '-', '-an', '-c:v', 'libx264',
           '-preset', 'fast', '-crf', '20', '-pix_fmt', 'yuv420p',
           '-movflags', '+faststart', str(out / 'planning-preview-802.mp4')]
    with subprocess.Popen(cmd, stdin=subprocess.PIPE) as p:
        for n in range(FPS * SCENES[-1]['end']):
            p.stdin.write(frame(n / FPS).tobytes())
        p.stdin.close()
        if p.wait() != 0:
            raise RuntimeError('ffmpeg encode failed')
    sheet = Image.new('RGB', (360 * 3, 360 * 4), BG)
    start = 0
    for i, scene in enumerate(SCENES):
        master = frame(start + 1)
        master.save(out / f'planning-style-{i + 1:02d}-1080.png')
        mobile = master.resize((360, 360), Image.Resampling.LANCZOS)
        mobile.save(out / f'planning-style-{i + 1:02d}-360.png')
        sheet.paste(mobile, ((i % 3) * 360, (i // 3) * 360))
        start = scene['end']
    sheet.save(out / 'planning-mobile-sheet.png')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    parser.add_argument('--out', type=Path)
    args = parser.parse_args()
    if args.check:
        check()
    elif args.out:
        render(args.out)
    else:
        parser.error('use --check or --out DIR')
