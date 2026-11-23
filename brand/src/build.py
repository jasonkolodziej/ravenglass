from raven import *
SQ='M256,0 C406,0 512,0 512,256 C512,512 406,512 256,512 C106,512 0,512 0,256 C0,0 106,0 256,0 Z'
# superellipse-ish squircle path generator
import math
def squircle(n=5,r=256,cx=256,cy=256):
    p=[]
    for i in range(360):
        t=math.radians(i); c,s=math.cos(t),math.sin(t)
        x=cx+r*math.copysign(abs(c)**(2/n),c); y=cy+r*math.copysign(abs(s)**(2/n),s); p.append(f'{x:.1f},{y:.1f}')
    return 'M'+' L'.join(p)+' Z'
SQ=squircle()
ICE='#a9dcff'
def wrap(body,defs='',w=512,h=512):
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}"><defs>{defs}</defs>{body}</svg>'

def bg_defs():
    return f'''<clipPath id="sq"><path d="{SQ}"/></clipPath>
<linearGradient id="night" x1="0" y1="0" x2="0.35" y2="1"><stop offset="0" stop-color="#2a2350"/><stop offset="1" stop-color="#0b0a16"/></linearGradient>
<linearGradient id="dusk" x1="0.2" y1="0" x2="0.5" y2="1"><stop offset="0" stop-color="#b9c4f2"/><stop offset="0.55" stop-color="#6c66b5"/><stop offset="1" stop-color="#2a2350"/></linearGradient>
<radialGradient id="moon" cx="0.74" cy="0.26" r="0.55"><stop offset="0" stop-color="#6a63b8" stop-opacity="0.55"/><stop offset="1" stop-color="#6a63b8" stop-opacity="0"/></radialGradient>
<filter id="blur" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur stdDeviation="9"/></filter>
<filter id="soft" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="18"/></filter>
<linearGradient id="spec" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#fff" stop-opacity="0.38"/><stop offset="0.45" stop-color="#fff" stop-opacity="0.06"/><stop offset="1" stop-color="#fff" stop-opacity="0.14"/></linearGradient>
<linearGradient id="rim" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#fff" stop-opacity="0.9"/><stop offset="0.5" stop-color="#fff" stop-opacity="0.15"/><stop offset="1" stop-color="#fff" stop-opacity="0.55"/></linearGradient>'''

RAVEN=lambda tr='': facets(hue=0.71,sat=0.30,base=0.035,span=0.20,transform=tr)
def eye(tr='',r=6.5,col=ICE):
    return f'<g {tr}><circle cx="{EYE[0]}" cy="{EYE[1]}" r="{r}" fill="{col}"/><circle cx="{EYE[0]+2}" cy="{EYE[1]-2}" r="{r*0.35}" fill="#fff"/></g>'

# ---------- A: Through the Pane ----------
def concept_a():
    tr='transform="translate(-60,-6) scale(1.12)"'
    pane=(70,330,372,150,38)  # x,y,w,h,r
    x,y,w,h,r=pane
    defs=bg_defs()+f'<clipPath id="pane"><rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{r}"/></clipPath>'
    body=f'''<g clip-path="url(#sq)">
<rect width="512" height="512" fill="url(#dusk)"/><rect width="512" height="512" fill="url(#moon)"/>
{RAVEN(tr)}{eye(tr)}
<g clip-path="url(#pane)"><rect width="512" height="512" fill="url(#dusk)"/><g filter="url(#blur)">{RAVEN(tr)}</g>
<rect x="{x}" y="{y}" width="{w}" height="{h}" fill="#e6ecff" fill-opacity="0.22"/>
<rect x="{x}" y="{y}" width="{w}" height="{h}" fill="url(#spec)"/></g>
<rect x="{x+0.75}" y="{y+0.75}" width="{w-1.5}" height="{h-1.5}" rx="{r}" fill="none" stroke="url(#rim)" stroke-width="1.5"/>
<path d="M{x+r},{y+2} L{x+w-r},{y+2}" stroke="#fff" stroke-opacity="0.55" stroke-width="1.2"/>
</g>'''
    return wrap(body,defs)

# ---------- B: Etched (raven as cut-glass lines) ----------
def etched(stroke=ICE,op=1,width=2.2,outline_w=3,tr=''):
    segs=facet_lines()
    l=''.join(f'<line x1="{a[0]:.1f}" y1="{a[1]:.1f}" x2="{b[0]:.1f}" y2="{b[1]:.1f}"/>' for a,b in segs)
    return f'<g {tr} fill="none" stroke="{stroke}" stroke-opacity="{op}" stroke-linecap="round" stroke-linejoin="round"><g stroke-width="{width}" stroke-opacity="{op*0.55}">{l}</g><path d="{outline_d()}" stroke-width="{outline_w}"/></g>'
def concept_b():
    tr='transform="translate(-24,6)"'
    defs=bg_defs()+'''<linearGradient id="glassbg" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#3b3570"/><stop offset="1" stop-color="#121026"/></linearGradient>
<filter id="glow" x="-30%" y="-30%" width="160%" height="160%"><feGaussianBlur stdDeviation="6" result="b"/><feMerge><feMergeNode in="b"/><feMergeNode in="SourceGraphic"/></feMerge></filter>'''
    body=f'''<g clip-path="url(#sq)"><rect width="512" height="512" fill="url(#glassbg)"/><rect width="512" height="512" fill="url(#moon)"/>
<g filter="url(#glow)">{etched(tr=tr)}</g>{eye(tr,r=7)}
<rect width="512" height="512" fill="url(#spec)" opacity="0.7"/></g>
<path d="{SQ}" fill="none" stroke="#fff" stroke-opacity="0.25" stroke-width="2"/>'''
    return wrap(body,defs)

# ---------- C: Messenger fold (envelope + raven in flight) ----------
FLY='M96,214 L256,318 L416,214 L392,206 L276,268 L268,246 L256,236 L244,246 L236,268 L120,206 Z'
def concept_c():
    defs=bg_defs()+'''<linearGradient id="paper" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#ffffff" stop-opacity="0.30"/><stop offset="1" stop-color="#ffffff" stop-opacity="0.10"/></linearGradient>'''
    body=f'''<g clip-path="url(#sq)"><rect width="512" height="512" fill="url(#night)"/><rect width="512" height="512" fill="url(#moon)"/>
<rect x="88" y="150" width="336" height="236" rx="34" fill="url(#paper)" stroke="url(#rim)" stroke-width="2"/>
<rect x="88" y="150" width="336" height="236" rx="34" fill="url(#spec)" opacity="0.6"/>

<path d="M92,170 L256,288 L420,170 L420,176 L256,300 L92,176 Z" fill="#0b0a16" opacity="0"/>
<path d="M96,172 L118,182 L112,170 L138,186 L134,172 L160,192 L158,180 C200,202 232,224 256,252 C280,224 312,202 354,180 L352,192 L378,172 L374,186 L400,170 L394,182 L416,172 C356,214 300,250 268,288 L280,322 L256,312 L232,322 L244,288 C212,250 156,214 96,172 Z" fill="#100d22"/>
<path d="M248,256 L256,236 L264,256 Z" fill="#100d22"/>
<circle cx="259" cy="247" r="2.4" fill="{ICE}"/>
</g><path d="{SQ}" fill="none" stroke="#fff" stroke-opacity="0.18" stroke-width="2"/>'''
    return wrap(body,defs)

for n,f in (('A_pane',concept_a),('B_etched',concept_b),('C_fold',concept_c)):
    open(f'{n}.svg','w').write(f())
