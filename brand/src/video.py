"""Ravenglass 23 s 9:16 marketing video. Frames composed with PIL, one mp4 per scene."""
import sys, os, math, random, subprocess, functools
sys.path.insert(0,'/home/claude/icon')
from PIL import Image, ImageDraw, ImageFont, ImageFilter
from ansi import NIGHT as N, FROST as F
from vidmock import triage_v
from raven import dot_field

W,H,FPS=1080,1920,30
BG=(8,7,15)
def hx(c): c=c.lstrip('#'); return tuple(int(c[i:i+2],16) for i in (0,2,4))
FG,MU,ACC,BLUE,GRAY=hx(N['foreground']),hx(N['silver']),hx(N['aqua']),hx(N['blue']),hx(N['gray'])
FB='/tmp/fs/package/files/ibm-plex-mono-latin-700-normal.woff'
FM='/home/claude/ravenglass/assets/fonts/IBMPlexMono-Medium.ttf'
FR='/home/claude/ravenglass/assets/fonts/IBMPlexMono-Regular.ttf'
@functools.lru_cache(None)
def font(path,size): return ImageFont.truetype(path,size)
TMP='/tmp/vid'; os.makedirs(TMP,exist_ok=True)

def ease(t): t=max(0,min(1,t)); return 1-(1-t)**3
def ease_io(t): t=max(0,min(1,t)); return t*t*(3-2*t)
def lerp(a,b,t): return a+(b-a)*t

# ---------- assets ----------
@functools.lru_cache(None)
def mock(state, width):
    """Render a triage_v state (tuple of kwargs) to an RGB image `width` wide."""
    kw=dict(state); pal=F if kw.pop('frost',False) else N
    key=f"{abs(hash(state))}_{width}"
    svg=f'{TMP}/m_{key}.svg'; png=f'{TMP}/m_{key}.png'
    if not os.path.exists(png):
        open(svg,'w').write(triage_v(pal,**kw)); subprocess.run(['rsvg-convert','-w',str(width),svg,'-o',png],check=True)
    return Image.open(png).convert('RGB')
def S(**kw): return tuple(sorted(kw.items()))

@functools.lru_cache(None)
def icon(size, light=False):
    p=f'{TMP}/icon_{size}_{light}.png'
    if not os.path.exists(p):
        subprocess.run(['rsvg-convert','-w',str(size),'/home/claude/icon/D_braille%s.svg'%('_light' if light else ''),'-o',p],check=True)
    return Image.open(p).convert('RGBA')

MX,MY,MW=40,700,1000
MH=round(MW*880/1400)
@functools.lru_cache(None)
def glow(w=MW,h=MH):
    pad=180; im=Image.new('RGBA',(w+2*pad,h+2*pad),(0,0,0,0)); d=ImageDraw.Draw(im)
    d.rounded_rectangle((pad-10,pad-10,pad+w+10,pad+h+10),60,fill=(124,118,216,255))
    d.rounded_rectangle((pad+120,pad+60,pad+w-120,pad+h-60),60,fill=ACC+(255,))
    return im.filter(ImageFilter.GaussianBlur(55)),pad

def paste_alpha(base, im, xy, a):
    if a<=0: return
    if im.mode!='RGBA': im=im.convert('RGBA')
    if a<1:
        r,g,b,al=im.split(); al=al.point(lambda v:int(v*a)); im=Image.merge('RGBA',(r,g,b,al))
    base.alpha_composite(im, (int(xy[0]),int(xy[1])))

def text(base, xy, s, size, color, path=FR, a=1.0, anchor='la', spacing=0):
    if a<=0: return
    layer=Image.new('RGBA',base.size,(0,0,0,0)); d=ImageDraw.Draw(layer)
    if spacing:
        x,y=xy; f=font(path,size)
        if anchor[0]=='m':
            tw=sum(f.getlength(ch)+spacing for ch in s)-spacing; x-=tw/2; anchor='l'+anchor[1]
        for ch in s:
            d.text((x,y),ch,font=f,fill=color+(int(255*a),),anchor=anchor); x+=f.getlength(ch)+spacing
    else:
        d.text(xy,s,font=font(path,size),fill=color+(int(255*a),),anchor=anchor)
    base.alpha_composite(layer)

def rich(base, xy, parts, size, path, a=1.0):
    """parts: [(text,color)] on one line."""
    x,y=xy; f=font(path,size)
    for s,c in parts:
        text(base,(x,y),s,size,c,path,a); x+=f.getlength(s)

def press(dt):
    if dt<0 or dt>0.5: return 0.0
    return 1.0 if dt<0.18 else 1-ease_io((dt-0.18)/0.32)

def keycap(base, x, y, label, pressed, a=1.0, w=96, h=96, fs=44):
    if a<=0: return
    layer=Image.new('RGBA',base.size,(0,0,0,0)); d=ImageDraw.Draw(layer)
    if pressed>0:
        g=Image.new('RGBA',base.size,(0,0,0,0)); gd=ImageDraw.Draw(g)
        gd.rounded_rectangle((x-6,y-6,x+w+6,y+h+6),22,fill=ACC+(int(200*pressed*a),))
        layer.alpha_composite(g.filter(ImageFilter.GaussianBlur(18)))
    fill=tuple(int(lerp(c1,c2,pressed)) for c1,c2 in zip(hx(N['background']),ACC))
    d.rounded_rectangle((x,y+int(4*pressed),x+w,y+h+int(4*pressed)),16,fill=fill+(int(255*a),),outline=ACC+(int(255*a),),width=3)
    tc=tuple(int(lerp(c1,c2,pressed)) for c1,c2 in zip(ACC,hx(N['black'])))
    d.text((x+w/2,y+h/2+int(4*pressed)),label,font=font(FB,fs),fill=tc+(int(255*a),),anchor='mm')
    base.alpha_composite(layer)

def brand_header(base,a=1.0):
    paste_alpha(base, icon(84), (64,250), a)
    text(base,(172,262),'RAVENGLASS',30,FG,FB,a,spacing=9)
    text(base,(172,304),'Rust native mail client',22,MU,FR,a)

def headline(base, lines, t_in, t, y=410, accent_last=None):
    a=ease((t-t_in)/0.45); dy=(1-a)*24
    for i,l in enumerate(lines):
        if accent_last and i==len(lines)-1:
            rich(base,(64,y+i*96+dy),[(l[:-len(accent_last)],FG),(accent_last,ACC)],84,FB,a)
        else:
            text(base,(64,y+i*96+dy),l,84,FG,FB,a)

def mock_block(base, img, a=1.0, rise=0.0):
    if a<=0: return
    g,pad=glow(); paste_alpha(base,g,(MX-pad,MY-pad+rise),a*0.9)
    d=ImageDraw.Draw(base); 
    m=img.resize((MW,MH),Image.LANCZOS) if img.size!=(MW,MH) else img
    paste_alpha(base,m,(MX,MY+rise),a)
    frame=Image.new('RGBA',base.size,(0,0,0,0)); fd=ImageDraw.Draw(frame)
    fd.rectangle((MX-1,MY-1+rise,MX+MW,MY+MH+rise),outline=GRAY+(int(255*a),),width=2)
    fd.rectangle((MX,MY+rise,MX+5,MY+MH+rise),fill=ACC+(int(255*a),))
    base.alpha_composite(frame)

def fade(t,dur,fin=0.25,fout=0.25):
    return min(1,ease_io(t/fin) if fin else 1, ease_io((dur-t)/fout) if fout else 1)

def new(): return Image.new('RGBA',(W,H),BG+(255,))

# ---------- scenes ----------
DOTS=None
def raven_layer(t, cx, cy, px=6.0, assemble=True, a=1.0):
    """Braille raven; dots fly in from scattered starts when assemble=True."""
    global DOTS
    if DOTS is None:
        w,h,ds=dot_field(34); rnd=random.Random(7)
        DOTS=(w,h,[(x,y,e,rnd.uniform(-200,W+200),rnd.uniform(-300,H+300),x/w*0.9+rnd.uniform(0,0.35)) for x,y,e in ds])
    w,h,ds=DOTS; SS=2
    ox,oy=cx-w*px/2, cy-h*px/2
    layer=Image.new('RGBA',(W*SS//2,H*SS//2),(0,0,0,0))  # half-res supersample trick: draw at 1x then smooth
    layer=Image.new('RGBA',(W,H),(0,0,0,0)); d=ImageDraw.Draw(layer); r=px*0.31
    eye_t=None
    for x,y,e,sx,sy,delay in ds:
        k=ease((t-delay)/0.9) if assemble else 1
        if k<=0: continue
        X=lerp(sx,ox+x*px,k); Y=lerp(sy,oy+y*py if (py:=px) else 0,k)
        col=ACC if e else BLUE
        al=int(255*a*min(1,k*1.6)*(1 if e else 0.92))
        rr=r*(1.6 if e else 1)
        d.ellipse((X-rr,Y-rr,X+rr,Y+rr),fill=col+(al,))
    return layer

def scene1(t, dur=4.0):  # intro
    im=new()
    lay=raven_layer(t, W/2, 900, px=6.2)
    im.alpha_composite(lay)
    a1=ease((t-2.35)/0.5); a2=ease((t-2.75)/0.5)
    text(im,(W/2,1430),'RAVENGLASS',64,FG,FB,a1,anchor='mm',spacing=18)
    text(im,(W/2,1510),'Rust native mail client',34,MU,FR,a2,anchor='mm')
    return im, fade(t,dur,0.0,0.3)

def scene2(t, dur=6.0):  # keys
    im=new(); brand_header(im)
    headline(im,['Mail at the','speed of keys.'],0.1,t,accent_last='keys.')
    events=[(1.3,'j'),(1.9,'j'),(2.5,'j'),(3.4,'e')]
    sel,removed,toast=0,(),None
    for te,k in events:
        if t>=te:
            if k=='j': sel+=1
            if k=='e': removed=(sel,); sel+=1; toast='archived 1 · u undo'
    st=S(sel=sel,removed=removed,toast=toast)
    a=ease((t-0.35)/0.5)
    mock_block(im, mock(st,2000), a, rise=(1-a)*40)
    keys=['j','k','e','u']; kx0=(W-(4*96+3*28))/2
    for i,k in enumerate(keys):
        p=max([press(t-te) for te,kk in events if kk==k] or [0])
        keycap(im,kx0+i*124,1400,k,max(0,p),a)
    cap=ease((t-0.8)/0.5)
    rich(im,(W/2-font(FR,30).getlength('j k move · e archive · u undo')/2,1540),[('j k',ACC),(' move · ',MU),('e',ACC),(' archive · ',MU),('u',ACC),(' undo',MU)],30,FR,cap)
    return im, fade(t,dur,0.2,0.25)

def scene3(t, dur=5.0):  # HTML
    im=new(); brand_header(im)
    headline(im,['HTML or text.','Press t.'],0.1,t,accent_last='t.')
    view='html' if t>=2.2 else 'text'
    big_t=mock(S(sel=6,view='text'),2800); big_h=mock(S(sel=6,view='html'),2800)
    z=ease_io((t-0.6)/1.0)
    # crop from full screen to the reader pane, keeping the display aspect
    full=(0,0,2800,1760); rw=1340; rh=rw*MH/MW
    tgt=(1410,70,1410+rw,70+rh)
    box=tuple(lerp(a,b,z) for a,b in zip(full,tgt))
    def view_of(img): return img.crop(tuple(int(v) for v in box)).resize((MW,MH),Image.LANCZOS)
    cur=view_of(big_t)
    if t>=2.2:
        k=ease((t-2.2)/0.2); cur=Image.blend(cur, view_of(big_h), k)
    a=ease((t-0.2)/0.4)
    mock_block(im,cur,a)
    p=press(t-2.2)
    keycap(im,(W-96)/2,1400,'t',p,a)
    cap=ease((t-2.6)/0.5)
    text(im,(W/2,1560),'HTML mail rendered in pure Rust.',30,FG,FR,cap,anchor='mm')
    text(im,(W/2,1606),'Remote images blocked until you say so.',28,MU,FR,cap,anchor='mm')
    return im, fade(t,dur,0.2,0.25)

def scene4(t, dur=4.0):  # themes
    im=new(); brand_header(im)
    headline(im,['Your colours.'],0.1,t,accent_last='colours.')
    cmdfull=':theme frost'
    n=0 if t<0.5 else min(len(cmdfull), 1+int((t-0.5)/0.07))
    switched=t>=1.55
    if not switched:
        night=mock(S(sel=0,cmd=(cmdfull[:n] if n else None)),2000); cur=night
    else:
        k=ease((t-1.55)/0.35)
        cur=Image.blend(mock(S(sel=0,cmd=cmdfull),2000), mock(S(sel=0,frost=True),2000), k)
    mock_block(im,cur,1.0)
    p=press(t-1.45)
    pc=press(t-0.5)
    keycap(im,W/2-160,1400,':',pc,1.0); keycap(im,W/2-36,1400,'enter',p,1.0,w=196,fs=36)
    # palette strip
    ka=ease((t-1.9)/0.5)
    order=["black","gray","silver","white","maroon","red","green","lime","olive","yellow","navy","blue","purple","fuchsia","teal","aqua"]
    sw=(MW-15*6)/16
    lay=Image.new('RGBA',im.size,(0,0,0,0)); d=ImageDraw.Draw(lay)
    for i,nme in enumerate(order):
        x=MX+i*(sw+6); c=hx(F[nme])
        d.rounded_rectangle((x,1540,x+sw,1600),8,fill=c+(int(255*ka),),outline=GRAY+(int(255*ka),),width=2)
    im.alpha_composite(lay)
    text(im,(W/2,1650),'16-colour themes · import from your terminal',28,MU,FR,ka,anchor='mm')
    return im, fade(t,dur,0.2,0.25)

def scene5(t, dur=4.0):  # outro
    im=new()
    g=Image.new('RGBA',(700,700),(0,0,0,0)); gd=ImageDraw.Draw(g); gd.ellipse((150,150,550,550),fill=(124,118,216,170)); g=g.filter(ImageFilter.GaussianBlur(70))
    a0=ease(t/0.6)
    paste_alpha(im,g,(W/2-350,700-350+40),a0)
    s=int(lerp(260,300,ease(t/1.2))); paste_alpha(im,icon(s),(W/2-s/2,740-s/2+40),a0)
    a1=ease((t-0.5)/0.5); a2=ease((t-0.9)/0.5); a3=ease((t-1.3)/0.5)
    text(im,(W/2,1080),'RAVENGLASS',64,FG,FB,a1,anchor='mm',spacing=18)
    text(im,(W/2,1160),'Rust native mail client',34,MU,FR,a2,anchor='mm')
    text(im,(W/2,1300),'macOS · Windows · Linux',32,FG,FR,a3,anchor='mm')
    return im, fade(t,dur,0.25,0.0)

SCENES=[('s1',scene1,4.0),('s2',scene2,6.0),('s3',scene3,5.0),('s4',scene4,4.0),('s5',scene5,4.0)]

def frame_rgb(fn,t,dur):
    im,a=fn(t,dur)
    if a<1:
        im=Image.blend(Image.new('RGBA',im.size,BG+(255,)),im,a)
    return im.convert('RGB')

if __name__=='__main__':
    mode=sys.argv[1]
    if mode=='stills':
        for name,fn,dur,t in [('s1',scene1,4.0,1.4),('s1',scene1,4.0,3.5),('s2',scene2,6.0,3.6),('s3',scene3,5.0,3.0),('s4',scene4,4.0,2.6),('s5',scene5,4.0,3.5)]:
            frame_rgb(fn,t,dur).save(f'/home/claude/video/still_{name}_{t}.png')
    elif mode=='render':
        for name,fn,dur in SCENES:
            if len(sys.argv)>2 and name not in sys.argv[2:]: continue
            tmp=f'/home/claude/video/{name}.part.mp4'
            p=subprocess.Popen(['ffmpeg','-y','-loglevel','error','-f','rawvideo','-pix_fmt','rgb24','-s',f'{W}x{H}','-r',str(FPS),'-i','-',
                                '-c:v','libx264','-preset','veryfast','-crf','18','-pix_fmt','yuv420p','-r',str(FPS),tmp],stdin=subprocess.PIPE)
            for i in range(int(round(dur*FPS))):
                p.stdin.write(frame_rgb(fn,i/FPS,dur).tobytes())
            p.stdin.close()
            if p.wait()==0: os.replace(tmp,f'/home/claude/video/{name}.mp4')
            print(name,'done',flush=True)
