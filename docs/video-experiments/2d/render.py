"""Synthetic 14-second CPU preview. Requires Pillow and ffmpeg on PATH."""
import argparse
from pathlib import Path
import subprocess
from PIL import Image, ImageDraw, ImageFont

FPS, SECONDS, SIZE = 30, 14, 1080
BG, INK, BLUE = '#F2F5FA', '#172338', '#164CC5'
FONT = '/System/Library/Fonts/Supplemental/Arial.ttf'
BOLD = '/System/Library/Fonts/Supplemental/Arial Bold.ttf'


def state(t):
    if not 0 <= t < SECONDS:
        raise ValueError('time outside preview')
    return ('request' if t < 3 else 'parallel' if t < 6 else
            'question' if t < 9 else 'answer' if t < 11 else
            'review' if t < 12.5 else 'checked')


def frame(t):
    stage = state(t)
    im = Image.new('RGB', (SIZE, SIZE), BG)
    d = ImageDraw.Draw(im)

    def text(x, y, value, size=54, color=INK, bold=False):
        font = ImageFont.truetype(BOLD if bold else FONT, size)
        # Fail rather than silently clip a caption at mobile-safe margins.
        assert d.textbbox((x, y), value, font=font)[2] <= 1008, value
        d.text((x, y), value, font=font, fill=color)

    def box(x, y, w, h, fill='white'):
        d.rounded_rectangle((x, y, x+w, y+h), radius=26, fill=fill)

    text(72, 45, 'taskq', 64, BLUE, True)
    text(800, 57, 'Демо', 54)
    text(72, 145, 'Один менеджер', 72, bold=True)
    if stage == 'request':
        text(72, 258, 'Запрос владельца', 54, BLUE)
        # Ease a single request card into place; text remains exact raster type.
        u = min(t / .6, 1)
        y = 365 + round(45 * (1-u)**3)
        box(72, y, 936, 290)
        text(108, y+40, 'Исправь вход', 66, bold=True)
        text(108, y+155, 'Обнови страницу', 66, bold=True)
        text(72, 775, 'Задачи: GitHub / GitLab')
    else:
        text(72, 255, 'Независимые задачи', 60, BLUE, True)
        text(72, 325, 'параллельно', 60, BLUE, True)
        login = 'Вопрос владельцу' if stage == 'question' else 'В работе'
        page = 'Проверено' if stage == 'checked' else 'На проверке' if stage == 'review' else 'В работе'
        for y, title, status in [(425, 'Исправить вход', login), (665, 'Обновить страницу', page)]:
            box(72, y, 936, 215)
            text(100, y+18, title, 60, bold=True)
            color = '#795000' if status == 'Вопрос владельцу' else '#17623C' if status == 'Проверено' else BLUE
            text(100, y+92, status, 54, color)
            # Two role labels, not a fictional application screenshot.
            text(100, y+157, 'Работник', 54)
            text(560, y+157, 'Супервизор', 54)
        if stage == 'question':
            box(72, 900, 936, 110, '#FFF0CB')
            text(96, 923, 'Оставить вход по email?', 60)
        elif stage == 'answer':
            box(72, 900, 936, 110, '#DEE8FF')
            text(96, 923, 'Владелец: Да, оставить', 60)
        elif stage in ('review', 'checked'):
            text(72, 925, 'Прогресс виден', 66, bold=True)
    return im


def check():
    expected = [(0, 'request'), (2.99, 'request'), (3, 'parallel'),
                (6, 'question'), (9, 'answer'), (11, 'review'),
                (12.5, 'checked'), (13.99, 'checked')]
    for t, want in expected:
        assert state(t) == want
    for t in (-1, 14):
        try:
            state(t)
        except ValueError:
            pass
        else:
            raise AssertionError('invalid time accepted')
    # Exercise every frame, including all text bounds and transition boundaries.
    for n in range(FPS * SECONDS):
        frame(n / FPS)
    print('timeline, duration boundaries and all frame text bounds: OK')


def render(out):
    out.mkdir(parents=True, exist_ok=True)
    cmd = ['ffmpeg', '-y', '-v', 'error', '-f', 'rawvideo', '-pix_fmt', 'rgb24',
           '-s', '1080x1080', '-r', str(FPS), '-i', '-', '-an', '-c:v', 'libx264',
           '-preset', 'medium', '-crf', '20', '-pix_fmt', 'yuv420p',
           '-movflags', '+faststart', str(out / 'preview.mp4')]
    with subprocess.Popen(cmd, stdin=subprocess.PIPE) as p:
        for n in range(FPS * SECONDS):
            p.stdin.write(frame(n / FPS).tobytes())
        p.stdin.close()
        if p.wait() != 0:
            raise RuntimeError('ffmpeg encode failed')
    frame(8).save(out / 'poster.png')
    times = [0, 2.9, 3, 5.9, 6, 8.9, 9, 10.9, 11, 12.49, 12.5, 13.96]
    sheet = Image.new('RGB', (360*3, 390*4), 'white')
    for i, t in enumerate(times):
        x, y = (i % 3)*360, (i // 3)*390
        sheet.paste(frame(t).resize((360, 360), Image.Resampling.LANCZOS), (x, y+30))
        ImageDraw.Draw(sheet).text((x+8, y+5), f'{t:.2f}s', fill=INK)
    sheet.save(out / 'screenlist.png')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--check', action='store_true')
    parser.add_argument('--out', type=Path, default=Path('/tmp/taskq-585-media'))
    args = parser.parse_args()
    check() if args.check else render(args.out)
