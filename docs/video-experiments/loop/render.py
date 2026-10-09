"""Selected A style, synthetic 28-second silent CPU loop; macOS fonts."""
import argparse
import importlib.util
from pathlib import Path
import subprocess
from PIL import Image, ImageDraw, ImageFont

SOURCE = Path(__file__).resolve().parents[1] / '2d' / 'render.py'
spec = importlib.util.spec_from_file_location('pilot', SOURCE)
pilot = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pilot)
FPS, SECONDS = pilot.FPS, 28


def stage(t):
    if not 0 <= t < SECONDS:
        raise ValueError('time outside loop')
    return ('chats' if t < 4 else 'request' if t < 7 else
            'tasks' if t < 10 else 'parallel' if t < 14 else
            'question' if t < 16 else 'answer' if t < 18 else
            'review' if t < 20.5 else 'checked' if t < 23 else
            'next' if t < 26 else 'return')


def custom(next_request=False):
    im = Image.new('RGB', (pilot.SIZE, pilot.SIZE), pilot.BG)
    d = ImageDraw.Draw(im)

    def text(x, y, value, size=54, color=pilot.INK, bold=False):
        font = ImageFont.truetype(pilot.BOLD if bold else pilot.FONT, size)
        bounds = d.textbbox((x, y), value, font=font)
        assert bounds[2] <= 1008 and bounds[3] <= 1008, value
        d.text((x, y), value, fill=color, font=font)

    def card(y, lines, color='white'):
        d.rounded_rectangle((72, y, 1008, y+185), radius=26, fill=color)
        for i, line in enumerate(lines):
            text(108, y+22+i*76, line, bold=i == 0)

    text(72, 45, 'taskq', 64, pilot.BLUE, True)
    text(800, 57, 'Демо')
    if next_request:
        text(72, 145, 'Один менеджер', 72, bold=True)
        text(72, 258, 'Статус и история видны', 60, pilot.BLUE)
        card(365, ['Вход: в работе', 'Страница: проверено'])
        card(610, ['Следующий запрос', 'Добавь поиск'], '#DEE8FF')
        text(72, 900, 'Задачи: GitHub / GitLab')
    else:
        text(72, 145, 'Много чатов', 72, bold=True)
        text(72, 258, 'Ручная координация', 66, pilot.BLUE)
        card(365, ['Codex', 'Исправь вход'])
        card(585, ['Claude', 'Обнови страницу'])
        card(805, ['Следующий запрос', 'Добавь поиск'], '#DEE8FF')
    return im


def frame(t):
    s = stage(t)
    if s == 'chats':
        return custom()
    if s == 'next':
        return custom(True)
    if s == 'return':
        # Finish the dissolve before the boundary: identical held frames at 28/0.
        u = min((t-26)/1.5, 1)
        u = u*u*(3-2*u)
        blank = Image.new('RGB', (pilot.SIZE, pilot.SIZE), pilot.BG)
        blank.paste(custom().crop((0, 0, pilot.SIZE, 120)), (0, 0))
        return (Image.blend(custom(True), blank, u*2) if u < .5 else
                Image.blend(blank, custom(), (u-.5)*2))
    times = {'request': 1, 'tasks': 3, 'parallel': 4,
             'question': 7, 'answer': 9.5, 'review': 11.5, 'checked': 13}
    return pilot.frame(times[s])


def check():
    for t, want in [(0, 'chats'), (4, 'request'), (7, 'tasks'),
                    (10, 'parallel'), (14, 'question'), (16, 'answer'),
                    (18, 'review'), (20.5, 'checked'), (23, 'next'), (26, 'return')]:
        assert stage(t) == want
    for t in (-1, 28):
        try:
            stage(t)
        except ValueError:
            pass
        else:
            raise AssertionError('invalid time accepted')
    assert frame(0).tobytes() == frame(SECONDS-1/FPS).tobytes()
    for n in range(FPS * SECONDS):
        frame(n/FPS)
    print('28s timeline, explicit answer before review, bounds, exact seam: OK')


def render(out):
    out.mkdir(parents=True, exist_ok=True)
    cmd = ['ffmpeg', '-y', '-v', 'error', '-f', 'rawvideo', '-pix_fmt', 'rgb24',
           '-s', '1080x1080', '-r', str(FPS), '-i', '-', '-an', '-c:v', 'libx264',
           '-preset', 'medium', '-crf', '20', '-pix_fmt', 'yuv420p',
           '-movflags', '+faststart', str(out/'loop.mp4')]
    with subprocess.Popen(cmd, stdin=subprocess.PIPE) as p:
        for n in range(FPS * SECONDS):
            p.stdin.write(frame(n/FPS).tobytes())
        p.stdin.close()
        if p.wait() != 0:
            raise RuntimeError('MP4 encode failed')
    subprocess.run(['ffmpeg', '-y', '-v', 'error', '-i', str(out/'loop.mp4'),
                    '-an', '-c:v', 'libvpx-vp9', '-b:v', '0', '-crf', '30',
                    str(out/'loop.webm')], check=True)
    frame(10).save(out/'poster.png')
    times = [0, 4, 7, 10, 14, 16, 18, 20.5, 23, 26, 26.75, 27.5]
    sheet = Image.new('RGB', (1080, 1560), 'white')
    for i, t in enumerate(times):
        x, y = i%3*360, i//3*390
        sheet.paste(frame(t).resize((360, 360), Image.Resampling.LANCZOS), (x, y+30))
        ImageDraw.Draw(sheet).text((x+8, y+5), f'{t:.2f}s', fill=pilot.INK)
    sheet.save(out/'screenlist.png')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--check', action='store_true')
    parser.add_argument('--out', type=Path, default=Path('/tmp/taskq-591-media'))
    args = parser.parse_args()
    check() if args.check else render(args.out)
