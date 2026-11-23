from raven import *
import subprocess, numpy as np
from PIL import Image
def braille(rows=26, facets=True, thresh=128):
    VBx,VBy,VBw,VBh=126,140,310,304
    H=rows*4; W=int(round(H*VBw/VBh/2))*2
    segs=facet_lines()
    lines=''.join(f'<line x1="{a[0]:.1f}" y1="{a[1]:.1f}" x2="{b[0]:.1f}" y2="{b[1]:.1f}"/>' for a,b in segs)
    sw=VBw/W*1.15
    svg=f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{VBx} {VBy} {VBw} {VBh}" width="{W}" height="{H}"><rect x="{VBx}" y="{VBy}" width="{VBw}" height="{VBh}" fill="#fff"/><path d="{outline_d()}" fill="#000"/>'
    if facets: svg+=f'<g stroke="#fff" stroke-width="{sw}">{lines}</g>'
    svg+=f'<circle cx="{EYE[0]}" cy="{EYE[1]}" r="{VBw/W*1.6}" fill="#fff"/></svg>'
    open('/tmp/b.svg','w').write(svg); subprocess.run(['rsvg-convert','/tmp/b.svg','-o','/tmp/b.png'],check=True)
    a=np.array(Image.open('/tmp/b.png').convert('L'))<thresh
    bits=[(0,0,0x01),(1,0,0x02),(2,0,0x04),(0,1,0x08),(1,1,0x10),(2,1,0x20),(3,0,0x40),(3,1,0x80)]
    out=[]
    for r in range(0,H,4):
        line=''
        for c in range(0,W,2):
            v=0
            for dy,dx,b in bits:
                if r+dy<H and c+dx<W and a[r+dy,c+dx]: v|=b
            line+=chr(0x2800+v)
        out.append(line.rstrip('⠀'))
    return out
if __name__=='__main__':
    art=braille()
    print('\n'.join(art)); print(len(art), max(len(l) for l in art))
    open('/home/claude/ravenglass-brand/watermark/ravenglass-wallpaper-braille.txt','w').write('\n'.join(art)+'\n')
    open('/home/claude/ravenglass-brand/watermark/ravenglass-wallpaper-braille-small.txt','w').write('\n'.join(braille(rows=12,facets=False))+'\n')
