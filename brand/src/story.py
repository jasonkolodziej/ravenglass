import re, subprocess
from ansi import NIGHT as N
from raven import dot_field
W,H=1080,1920
def nest(path,prefix,x,y,w,h):
    s=open(path).read(); s=re.sub(r'<\?xml[^>]*\?>','',s).strip()
    for i in set(re.findall(r'id="([^"]+)"',s)):
        s=s.replace(f'id="{i}"',f'id="{prefix}-{i}"').replace(f'url(#{i})',f'url(#{prefix}-{i})')
    s=re.sub(r'<svg ([^>]*?)width="[^"]*" height="[^"]*"',lambda m:f'<svg {m.group(1)}',s,count=1)
    return s.replace('<svg ',f'<svg x="{x}" y="{y}" width="{w}" height="{h}" ',1)
def raven_dots(x0,y0,rows,px,color,op,eye):
    w,h,dots=dot_field(rows); py=px; d=px*0.62
    runs={}
    for x,y,e in dots:
        if not e: runs.setdefault(y,[]).append(x)
    segs=[]
    for y,xs in runs.items():
        xs.sort(); a=b=xs[0]
        for x in xs[1:]+[None]:
            if x is not None and x==b+1: b=x; continue
            segs.append(f'M{x0+a*px:.1f} {y0+y*py:.1f}h{(b-a)*px:.1f}')
            if x is not None: a=b=x
    ex=[(x,y) for x,y,e in dots if e]
    eyes=''.join(f'<circle cx="{x0+x*px:.1f}" cy="{y0+y*py:.1f}" r="{d/2:.2f}" fill="{eye}"/>' for x,y in ex)
    return f'<g opacity="{op}"><path d="{"".join(segs)}" fill="none" stroke="{color}" stroke-width="{d:.2f}" stroke-linecap="round" stroke-dasharray="0 {px:.2f}"/>{eyes}</g>', w*px, h*py
fg,mu,acc,blue,gray=N['foreground'],N['silver'],N['aqua'],N['blue'],N['gray']
# big decorative raven, bottom right, partly off-canvas
rav,rw,rh=raven_dots(470,1672,40,3.4,blue,0.55,acc)
def key(x,y,label,w=64):
    return f'<rect x="{x}" y="{y}" width="{w}" height="64" rx="10" fill="{N["background"]}" stroke="{acc}" stroke-width="2"/><text x="{x+w/2}" y="{y+43}" text-anchor="middle" font-size="30" font-weight="600" fill="{acc}">{label}</text>'
feats=[("j k","move through mail without a mouse"),("e  #","archive and delete in one key"),("t","HTML or text: rendered in pure Rust"),(":","themes, commands, your colours")]
fl=''
for i,(k,t) in enumerate(feats):
    y=1360+i*58
    fl+=f'<text x="64" y="{y}" font-size="30" fill="{acc}" font-weight="600" xml:space="preserve">{k:<4}</text><text x="190" y="{y}" font-size="30" fill="{fg}">{t}</text>'
mock_w=952; mock_h=598; mx=(W-mock_w)/2; my=700
svg=f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}" font-family="IBM Plex Mono">
<defs><filter id="glow" x="-40%" y="-60%" width="180%" height="220%"><feGaussianBlur stdDeviation="55"/></filter>
<radialGradient id="glowfill" cx="0.5" cy="0.5" r="0.5"><stop offset="0" stop-color="{acc}" stop-opacity="0.9"/><stop offset="0.55" stop-color="#6C66B5" stop-opacity="0.75"/><stop offset="1" stop-color="#6C66B5" stop-opacity="0"/></radialGradient></defs>
<rect width="{W}" height="{H}" fill="#08070F"/>
{rav}
{nest('D_braille.svg','ic',64,256,104,104)}
<text x="196" y="300" font-size="34" font-weight="600" letter-spacing="10" fill="{fg}">RAVENGLASS</text>
<text x="196" y="342" font-size="22" fill="{mu}">Rust native mail client</text>
<text x="64" y="480" font-size="92" font-weight="700" fill="{fg}">Mail at the</text>
<text x="64" y="584" font-size="92" font-weight="700" fill="{fg}">speed of <tspan fill="{acc}">keys.</tspan></text>
<text x="64" y="648" font-size="28" fill="{mu}">A keyboard-first inbox.</text>
<g filter="url(#glow)"><rect x="{mx-10}" y="{my-10}" width="{mock_w+20}" height="{mock_h+20}" rx="60" fill="#7C76D8"/><rect x="{mx+120}" y="{my+60}" width="{mock_w-240}" height="{mock_h-120}" rx="60" fill="{acc}"/></g>
<rect x="{mx-1}" y="{my-1}" width="{mock_w+2}" height="{mock_h+2}" fill="none" stroke="{gray}" stroke-width="2"/>
{nest('/home/claude/ravenglass-brand/preview/ui-triage-night.svg','ui',mx,my,mock_w,mock_h)}
<rect x="{mx}" y="{my}" width="5" height="{mock_h}" fill="{acc}"/>
{key(mx+mock_w-330,my+mock_h-40,'j')}{key(mx+mock_w-254,my+mock_h-40,'k')}{key(mx+mock_w-178,my+mock_h-40,'e')}{key(mx+mock_w-102,my+mock_h-40,'r')}
{fl}
<line x1="64" y1="1596" x2="1016" y2="1596" stroke="{gray}" stroke-width="2"/>
<text x="64" y="1650" font-size="26" fill="{mu}">macOS · Windows · Linux</text>
</svg>'''
open('/home/claude/ravenglass/brand/boards/social-story-9x16.svg','w').write(svg)
subprocess.run(['rsvg-convert','/home/claude/ravenglass/brand/boards/social-story-9x16.svg','-o','/home/claude/ravenglass/brand/boards/social-story-9x16.png'],check=True)
