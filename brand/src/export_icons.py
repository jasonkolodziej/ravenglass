"""Export the app icon (D · Braille, dark and light) for every platform: `python3 export_icons.py OUT`.

Writes into OUT (packaging/icons/ in the repo; run packaging/icons/build.sh):
  png/ravenglass-{dark,light}-{N}.png   N = 16 24 32 48 64 128 256 512 1024
  ravenglass.ico                        dark, 16-256, each size its own image (Windows)
  hicolor/{N}x{N}/apps/ravenglass.png   dark; ravenglass-light.png beside it (Linux)
  hicolor/scalable/apps/ravenglass.svg  dark master; ravenglass-light.svg
  Ravenglass.iconset/                   dark, for `iconutil -c icns` on macOS
  svg/                                  the SVGs every PNG was rendered from

Sizes 16, 24 and 32 are regenerated with a coarse, pixel-aligned dot grid (plan §4: downscaling
the master fills the beak in); 48 and up are the master SVG rendered at that size.
Needs rsvg-convert (librsvg2-bin), Pillow, numpy, scipy and matplotlib.
"""
import os, sys, shutil, subprocess, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
SIZES = [16, 24, 32, 48, 64, 128, 256, 512, 1024]
# size -> (dot rows, pixels from one dot to the next, coverage threshold), picked by eye.
SMALL = {16: (8, 2, 0.55), 24: (10, 2, 0.5), 32: (10, 3, 0.5)}
ICO = [16, 24, 32, 48, 64, 128, 256]
HICOLOR = [16, 24, 32, 48, 64, 128, 256, 512]
ICONSET = {'16x16': 16, '16x16@2x': 32, '32x32': 32, '32x32@2x': 64, '128x128': 128,
           '128x128@2x': 256, '256x256': 256, '256x256@2x': 512, '512x512': 512, '512x512@2x': 1024}


def main(out):
    out = os.path.abspath(out)
    sys.path.insert(0, HERE)
    # build.py writes the rejected concepts into the working directory on import: keep them out.
    scratch = tempfile.mkdtemp()
    os.chdir(scratch)
    from icon_d import icon_svg
    from PIL import Image

    for d in ('png', 'svg', 'hicolor', 'Ravenglass.iconset'):
        shutil.rmtree(os.path.join(out, d), ignore_errors=True)
        os.makedirs(os.path.join(out, d))
    pngs = {}
    for variant in ('dark', 'light'):
        light = variant == 'light'
        master = os.path.join(out, 'svg', f'ravenglass-{variant}.svg')
        open(master, 'w').write(icon_svg(light))
        for n in SIZES:
            src = master
            if n in SMALL:
                dots, pitch, thresh = SMALL[n]
                src = os.path.join(out, 'svg', f'ravenglass-{variant}-{n}.svg')
                open(src, 'w').write(icon_svg(light, size=n, dots=dots, pitch_px=pitch, thresh=thresh))
            png = os.path.join(out, 'png', f'ravenglass-{variant}-{n}.png')
            subprocess.run(['rsvg-convert', '-w', str(n), '-h', str(n), src, '-o', png], check=True)
            pngs[variant, n] = png

    images = [Image.open(pngs['dark', n]).convert('RGBA') for n in ICO]
    images[-1].save(os.path.join(out, 'ravenglass.ico'), format='ICO',
                    sizes=[(n, n) for n in ICO], append_images=images[:-1])

    for n in HICOLOR:
        d = os.path.join(out, 'hicolor', f'{n}x{n}', 'apps')
        os.makedirs(d)
        shutil.copy(pngs['dark', n], os.path.join(d, 'ravenglass.png'))
        shutil.copy(pngs['light', n], os.path.join(d, 'ravenglass-light.png'))
    d = os.path.join(out, 'hicolor', 'scalable', 'apps')
    os.makedirs(d)
    shutil.copy(os.path.join(out, 'svg', 'ravenglass-dark.svg'), os.path.join(d, 'ravenglass.svg'))
    shutil.copy(os.path.join(out, 'svg', 'ravenglass-light.svg'), os.path.join(d, 'ravenglass-light.svg'))

    for name, n in ICONSET.items():
        shutil.copy(pngs['dark', n], os.path.join(out, 'Ravenglass.iconset', f'icon_{name}.png'))
    shutil.rmtree(scratch, ignore_errors=True)


if __name__ == '__main__':
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])
